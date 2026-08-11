use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::inbound_load::{
    CancelInboundLoadCommand, CancelInboundLoadResult, InboundLoadCancelledStatus,
    CANCEL_INBOUND_LOAD_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    FacilityId, InboundLoadCancellationId, InboundLoadPreArrivalStatus, InventoryOwnerId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::{insert_result, PostgresPreparedCommandExt};
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};

pub async fn cancel_inbound_load(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CancelInboundLoadCommand,
) -> AppResult<CancelInboundLoadResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CANCEL_INBOUND_LOAD_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_cancellation_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<CancelInboundLoadResult>(&mut tx)
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
    if row.try_get::<String, _>("type")? != "inbound" {
        return Err(AppError::not_found("inbound load"));
    }
    let previous_status = match row.try_get::<String, _>("status")?.as_str() {
        "planned" => InboundLoadPreArrivalStatus::Planned,
        "scheduled" => InboundLoadPreArrivalStatus::Scheduled,
        _ => {
            return Err(AppError::conflict(
                "inbound load must be planned or scheduled before it can be cancelled",
            ));
        }
    };
    let cancelled_at = now_iso();
    let details = command.details();
    let cancellation_id = InboundLoadCancellationId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO inbound_load_cancellations
                (tenant_id,inventory_owner_id,facility_id,load_id,previous_status,
                 reason_code,note,cancelled_by_user_id,cancelled_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(inventory_owner_id)
        .bind(facility_id)
        .bind(command.load_id().get())
        .bind(match previous_status {
            InboundLoadPreArrivalStatus::Planned => "planned",
            InboundLoadPreArrivalStatus::Scheduled => "scheduled",
        })
        .bind(details.reason().as_str())
        .bind(details.note().map(|note| note.as_str()))
        .bind(context.actor_id.get())
        .bind(cancelled_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;

    let updated = sqlx::query(
        r#"
        UPDATE loads SET status='cancelled'
        WHERE tenant_id=$1 AND id=$2 AND status=$3 AND deleted IS NULL
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.load_id().get())
    .bind(match previous_status {
        InboundLoadPreArrivalStatus::Planned => "planned",
        InboundLoadPreArrivalStatus::Scheduled => "scheduled",
    })
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "inbound load state changed while it was cancelled",
        ));
    }

    sqlx::query(
        r#"
        INSERT INTO load_activity
            (tenant_id,created,load_id,user_id,action,message,metadata_json)
        VALUES ($1,$2,$3,$4,'cancelled','inbound load cancelled',$5)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(cancelled_at)
    .bind(command.load_id().get())
    .bind(context.actor_id.get())
    .bind(
        serde_json::json!({
            "cancellation_id": cancellation_id.get(),
            "reason": details.reason().as_str(),
            "note": details.note().map(|note| note.as_str()),
        })
        .to_string(),
    )
    .execute(&mut *tx)
    .await?;

    let result = CancelInboundLoadResult {
        cancellation_id,
        load_id: command.load_id(),
        previous_status,
        status: InboundLoadCancelledStatus::Cancelled,
        reason: details.reason(),
        note: details.note().map(|note| note.as_str().to_owned()),
        cancelled_by: context.actor_id,
        cancelled_at,
    };
    enqueue_cancelled_event(
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

async fn require_stored_cancellation_visible_before_replay(
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
    .bind(CANCEL_INBOUND_LOAD_OPERATION)
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
async fn enqueue_cancelled_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    inventory_owner_id: i64,
    facility_id: i64,
    result: &CancelInboundLoadResult,
) -> AppResult<()> {
    let ordering_key = format!("inbound-load:{}", result.load_id.get());
    let sequence: i64 = sqlx::query_scalar(
        "SELECT COALESCE((SELECT last_sequence FROM outbox_aggregate_sequences WHERE tenant_id=$1 AND ordering_key=$2),0)+1",
    )
    .bind(access.tenant_id.get())
    .bind(&ordering_key)
    .fetch_one(&mut **tx)
    .await?;
    let event_key = format!("inbound-load:{}:cancelled", result.load_id.get());
    let aggregate_id = result.load_id.to_string();
    let payload = serde_json::json!({
        "cancellation_id": result.cancellation_id.get(),
        "load_id": result.load_id.get(),
        "previous_status": match result.previous_status {
            InboundLoadPreArrivalStatus::Planned => "planned",
            InboundLoadPreArrivalStatus::Scheduled => "scheduled",
        },
        "status": "cancelled",
        "inventory_owner_id": inventory_owner_id,
        "facility_id": facility_id,
        "reason": result.reason.as_str(),
        "note": result.note,
        "cancelled_by": result.cancelled_by.get(),
        "cancelled_at": result.cancelled_at,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(
                InventoryOwnerId::new(inventory_owner_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            facility_id: Some(
                FacilityId::new(facility_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            actor_user_id: Some(context.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "inbound_load",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: "inbound.load.cancelled",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.cancelled_at,
        },
    )
    .await?;
    Ok(())
}
