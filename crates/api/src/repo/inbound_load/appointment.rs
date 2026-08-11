use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::inbound_load::{
    InboundLoadPlannedStatus, InboundLoadScheduledStatus, ScheduleInboundLoadCommand,
    ScheduleInboundLoadResult, SCHEDULE_INBOUND_LOAD_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{validate_inbound_load_appointment, InboundLoadAppointmentId};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::{insert_result, PostgresPreparedCommandExt};
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};

pub async fn schedule_inbound_load(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ScheduleInboundLoadCommand,
) -> AppResult<ScheduleInboundLoadResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, SCHEDULE_INBOUND_LOAD_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_appointment_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<ScheduleInboundLoadResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, facility_id, type, status
        FROM loads
        WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL
          AND ($3 OR facility_id=ANY($4))
          AND ($5 OR inventory_owner_id=ANY($6))
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.load_id().get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("inbound load"))?;
    let inventory_owner_id: i64 = row.try_get("inventory_owner_id")?;
    let facility_id: i64 = row.try_get("facility_id")?;
    let load_type: String = row.try_get("type")?;
    let status: String = row.try_get("status")?;
    if load_type != "inbound" {
        return Err(AppError::not_found("inbound load"));
    }
    if status != "planned" {
        return Err(AppError::conflict(
            "inbound load must be planned before it can be scheduled",
        ));
    }
    let scheduled_at = now_iso();
    validate_inbound_load_appointment(command.scheduled_for(), scheduled_at)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let appointment_id = InboundLoadAppointmentId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO inbound_load_appointments
                (tenant_id,inventory_owner_id,facility_id,load_id,scheduled_for,
                 scheduled_by_user_id,scheduled_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(inventory_owner_id)
        .bind(facility_id)
        .bind(command.load_id().get())
        .bind(command.scheduled_for())
        .bind(context.actor_id.get())
        .bind(scheduled_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    let updated = sqlx::query(
        r#"
        UPDATE loads SET status='scheduled', appointment_time=$1
        WHERE tenant_id=$2 AND id=$3 AND status='planned' AND deleted IS NULL
        "#,
    )
    .bind(command.scheduled_for())
    .bind(access.tenant_id.get())
    .bind(command.load_id().get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "inbound load state changed while its appointment was scheduled",
        ));
    }
    sqlx::query(
        r#"
        INSERT INTO load_activity
            (tenant_id,created,load_id,user_id,action,message,metadata_json)
        VALUES ($1,$2,$3,$4,'scheduled','inbound appointment scheduled',$5)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scheduled_at)
    .bind(command.load_id().get())
    .bind(context.actor_id.get())
    .bind(
        serde_json::json!({
            "appointment_id": appointment_id.get(),
            "scheduled_for": command.scheduled_for(),
        })
        .to_string(),
    )
    .execute(&mut *tx)
    .await?;
    let result = ScheduleInboundLoadResult {
        appointment_id,
        load_id: command.load_id(),
        previous_status: InboundLoadPlannedStatus::Planned,
        status: InboundLoadScheduledStatus::Scheduled,
        scheduled_for: command.scheduled_for(),
        scheduled_by: context.actor_id,
        scheduled_at,
    };
    enqueue_scheduled_event(
        &mut tx,
        access,
        context,
        inventory_owner_id,
        facility_id,
        &result,
    )
    .await?;
    insert_result(&mut tx, &prepared.completed_result(&result, None)?).await?;
    tx.commit().await?;
    Ok(result)
}

async fn require_stored_appointment_visible_before_replay(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let stored_load_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT (result_json->>'load_id')::BIGINT
        FROM command_idempotency_records
        WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(SCHEDULE_INBOUND_LOAD_OPERATION)
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(load_id) = stored_load_id else {
        return Ok(());
    };
    let visible: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM loads
            WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL
              AND ($3 OR facility_id=ANY($4))
              AND ($5 OR inventory_owner_id=ANY($6))
        )
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(&mut **tx)
    .await?;
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("inbound load"))
    }
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_scheduled_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    inventory_owner_id: i64,
    facility_id: i64,
    result: &ScheduleInboundLoadResult,
) -> AppResult<()> {
    let ordering_key = format!("inbound-load:{}", result.load_id.get());
    let sequence: i64 = sqlx::query_scalar(
        "SELECT COALESCE((SELECT last_sequence FROM outbox_aggregate_sequences WHERE tenant_id=$1 AND ordering_key=$2),0)+1",
    )
    .bind(access.tenant_id.get())
    .bind(&ordering_key)
    .fetch_one(&mut **tx)
    .await?;
    let event_key = format!("inbound-load:{}:scheduled", result.load_id.get());
    let aggregate_id = result.load_id.to_string();
    let payload = serde_json::json!({
        "appointment_id": result.appointment_id.get(),
        "load_id": result.load_id.get(),
        "previous_status": "planned",
        "status": "scheduled",
        "inventory_owner_id": inventory_owner_id,
        "facility_id": facility_id,
        "scheduled_for": result.scheduled_for,
        "scheduled_by": result.scheduled_by.get(),
        "scheduled_at": result.scheduled_at,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(
                wareboxes_domain::InventoryOwnerId::new(inventory_owner_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            facility_id: Some(
                wareboxes_domain::FacilityId::new(facility_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            actor_user_id: Some(context.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "inbound_load",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: "inbound.load.scheduled",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.scheduled_at,
        },
    )
    .await?;
    Ok(())
}
