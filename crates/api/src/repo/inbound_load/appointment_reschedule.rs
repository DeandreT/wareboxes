use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::inbound_load::{
    InboundLoadScheduledStatus, RescheduleInboundLoadAppointmentCommand,
    RescheduleInboundLoadAppointmentResult, RESCHEDULE_INBOUND_LOAD_APPOINTMENT_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    validate_inbound_load_appointment_reschedule, InboundLoadAppointmentId,
    InboundLoadAppointmentRescheduleId, Timestamp,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::{insert_result, PostgresPreparedCommandExt};
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};

pub async fn reschedule_inbound_load_appointment(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &RescheduleInboundLoadAppointmentCommand,
) -> AppResult<RescheduleInboundLoadAppointmentResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(
        context,
        RESCHEDULE_INBOUND_LOAD_APPOINTMENT_OPERATION,
        command,
    )?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_reschedule_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<RescheduleInboundLoadAppointmentResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id,facility_id,type,status,appointment_time
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
    if row.try_get::<String, _>("type")? != "inbound" {
        return Err(AppError::not_found("inbound load"));
    }
    if row.try_get::<String, _>("status")? != "scheduled" {
        return Err(AppError::conflict(
            "inbound load must be scheduled before its appointment can be rescheduled",
        ));
    }
    let current_scheduled_for = row
        .try_get::<Option<Timestamp>, _>("appointment_time")?
        .ok_or_else(|| AppError::conflict("scheduled inbound load has no current appointment"))?;
    if current_scheduled_for != command.expected_scheduled_for() {
        return Err(AppError::conflict(
            "inbound load appointment changed; refresh before rescheduling",
        ));
    }
    let rescheduled_at = now_iso();
    validate_inbound_load_appointment_reschedule(
        current_scheduled_for,
        command.scheduled_for(),
        rescheduled_at,
    )
    .map_err(|error| AppError::bad_request(error.to_string()))?;

    let appointment_id = InboundLoadAppointmentId::new(
        sqlx::query_scalar(
            r#"
            SELECT id
            FROM inbound_load_appointments
            WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3 AND load_id=$4
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(inventory_owner_id)
        .bind(facility_id)
        .bind(command.load_id().get())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| AppError::conflict("scheduled inbound load lacks appointment evidence"))?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    let sequence: i64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(MAX(sequence),0)+1
        FROM inbound_load_appointment_reschedules
        WHERE tenant_id=$1 AND load_id=$2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.load_id().get())
    .fetch_one(&mut *tx)
    .await?;
    let details = command.details();
    let reschedule_id = InboundLoadAppointmentRescheduleId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO inbound_load_appointment_reschedules
                (tenant_id,inventory_owner_id,facility_id,load_id,appointment_id,sequence,
                 previous_scheduled_for,scheduled_for,reason_code,note,
                 rescheduled_by_user_id,rescheduled_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(inventory_owner_id)
        .bind(facility_id)
        .bind(command.load_id().get())
        .bind(appointment_id.get())
        .bind(sequence)
        .bind(current_scheduled_for)
        .bind(command.scheduled_for())
        .bind(details.reason().as_str())
        .bind(details.note().map(|note| note.as_str()))
        .bind(context.actor_id.get())
        .bind(rescheduled_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    let updated = sqlx::query(
        r#"
        UPDATE loads SET appointment_time=$1
        WHERE tenant_id=$2 AND id=$3 AND status='scheduled'
          AND appointment_time=$4 AND deleted IS NULL
        "#,
    )
    .bind(command.scheduled_for())
    .bind(access.tenant_id.get())
    .bind(command.load_id().get())
    .bind(current_scheduled_for)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "inbound load appointment changed while it was rescheduled",
        ));
    }
    let note = details.note().map(|note| note.as_str().to_owned());
    sqlx::query(
        r#"
        INSERT INTO load_activity
            (tenant_id,created,load_id,user_id,action,message,metadata_json)
        VALUES ($1,$2,$3,$4,'appointment_rescheduled',
                'inbound appointment rescheduled',$5)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(rescheduled_at)
    .bind(command.load_id().get())
    .bind(context.actor_id.get())
    .bind(
        serde_json::json!({
            "reschedule_id": reschedule_id.get(),
            "appointment_id": appointment_id.get(),
            "sequence": sequence,
            "previous_scheduled_for": current_scheduled_for,
            "scheduled_for": command.scheduled_for(),
            "reason": details.reason().as_str(),
            "note": note.clone(),
        })
        .to_string(),
    )
    .execute(&mut *tx)
    .await?;
    let result = RescheduleInboundLoadAppointmentResult {
        reschedule_id,
        appointment_id,
        load_id: command.load_id(),
        status: InboundLoadScheduledStatus::Scheduled,
        sequence,
        previous_scheduled_for: current_scheduled_for,
        scheduled_for: command.scheduled_for(),
        reason: details.reason(),
        note,
        rescheduled_by: context.actor_id,
        rescheduled_at,
    };
    enqueue_rescheduled_event(
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

async fn require_stored_reschedule_visible_before_replay(
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
    .bind(RESCHEDULE_INBOUND_LOAD_APPOINTMENT_OPERATION)
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
async fn enqueue_rescheduled_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    inventory_owner_id: i64,
    facility_id: i64,
    result: &RescheduleInboundLoadAppointmentResult,
) -> AppResult<()> {
    let ordering_key = format!("inbound-load:{}", result.load_id.get());
    let sequence: i64 = sqlx::query_scalar(
        "SELECT COALESCE((SELECT last_sequence FROM outbox_aggregate_sequences WHERE tenant_id=$1 AND ordering_key=$2),0)+1",
    )
    .bind(access.tenant_id.get())
    .bind(&ordering_key)
    .fetch_one(&mut **tx)
    .await?;
    let event_key = format!(
        "inbound-load:{}:appointment-rescheduled:{}",
        result.load_id.get(),
        result.sequence
    );
    let aggregate_id = result.load_id.to_string();
    let payload = serde_json::json!({
        "reschedule_id": result.reschedule_id.get(),
        "appointment_id": result.appointment_id.get(),
        "load_id": result.load_id.get(),
        "status": "scheduled",
        "inventory_owner_id": inventory_owner_id,
        "facility_id": facility_id,
        "sequence": result.sequence,
        "previous_scheduled_for": result.previous_scheduled_for,
        "scheduled_for": result.scheduled_for,
        "reason": result.reason.as_str(),
        "note": result.note,
        "rescheduled_by": result.rescheduled_by.get(),
        "rescheduled_at": result.rescheduled_at,
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
            event_type: "inbound.load.appointment_rescheduled",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.rescheduled_at,
        },
    )
    .await?;
    Ok(())
}
