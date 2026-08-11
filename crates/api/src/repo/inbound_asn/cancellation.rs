use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::inbound_asn::{
    CancelInboundAsnCommand, CancelInboundAsnResult, CANCEL_INBOUND_ASN_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    cancel_inbound_asn, FacilityId, InboundAsnCancellationId, InboundAsnStatus, InventoryOwnerId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::{insert_result, PostgresPreparedCommandExt};
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use super::{parse_status, require_stored_visible_before_replay, revision};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

pub async fn cancel(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CancelInboundAsnCommand,
) -> AppResult<CancelInboundAsnResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CANCEL_INBOUND_ASN_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared.replayed::<CancelInboundAsnResult>(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }

    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id,facility_id,status,revision,load_id
        FROM inbound_asns
        WHERE tenant_id=$1 AND id=$2
          AND ($3 OR facility_id=ANY($4))
          AND ($5 OR inventory_owner_id=ANY($6))
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.asn_id().get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("advance shipping notice"))?;
    let inventory_owner_id: i64 = row.try_get("inventory_owner_id")?;
    let facility_id: i64 = row.try_get("facility_id")?;
    let previous_status = parse_status(row.try_get::<String, _>("status")?.as_str())?;
    let previous_revision = revision(row.try_get("revision")?)?;
    if previous_revision != command.expected_revision() {
        return Err(AppError::conflict(
            "advance shipping notice changed; refresh before cancelling it",
        ));
    }
    if row.try_get::<Option<i64>, _>("load_id")?.is_some() {
        return Err(AppError::conflict(
            "a planned advance shipping notice must be managed through its inbound load",
        ));
    }
    let resulting_revision = cancel_inbound_asn(previous_status, previous_revision)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let cancelled_at = now_iso();
    let details = command.details();
    let cancellation_id = InboundAsnCancellationId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO inbound_asn_cancellations
                (tenant_id,inventory_owner_id,facility_id,asn_id,
                 expected_asn_revision,resulting_asn_revision,reason_code,note,
                 cancelled_by_user_id,cancelled_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(inventory_owner_id)
        .bind(facility_id)
        .bind(command.asn_id().get())
        .bind(previous_revision.get())
        .bind(resulting_revision.get())
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
        UPDATE inbound_asns SET status='cancelled',revision=$1
        WHERE tenant_id=$2 AND id=$3 AND status='open' AND revision=$4 AND load_id IS NULL
        "#,
    )
    .bind(resulting_revision.get())
    .bind(access.tenant_id.get())
    .bind(command.asn_id().get())
    .bind(previous_revision.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "advance shipping notice changed while it was cancelled",
        ));
    }
    let result = CancelInboundAsnResult {
        cancellation_id,
        asn_id: command.asn_id(),
        previous_status,
        status: InboundAsnStatus::Cancelled,
        revision: resulting_revision,
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

#[allow(clippy::too_many_arguments)]
async fn enqueue_cancelled_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    inventory_owner_id: i64,
    facility_id: i64,
    result: &CancelInboundAsnResult,
) -> AppResult<()> {
    let owner = InventoryOwnerId::new(inventory_owner_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let facility =
        FacilityId::new(facility_id).map_err(|error| AppError::internal(error.to_string()))?;
    let event_key = format!("inbound-asn:{}:cancelled", result.asn_id.get());
    let aggregate_id = result.asn_id.to_string();
    let ordering_key = format!("inbound-asn:{}", result.asn_id.get());
    let payload = serde_json::json!({
        "cancellation_id": result.cancellation_id.get(),
        "asn_id": result.asn_id.get(),
        "previous_status": result.previous_status.as_str(),
        "status": result.status.as_str(),
        "revision": result.revision.get(),
        "reason": result.reason.as_str(),
        "note": result.note,
        "cancelled_by": result.cancelled_by.get(),
        "cancelled_at": result.cancelled_at,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(owner),
            facility_id: Some(facility),
            actor_user_id: Some(context.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "inbound_asn",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: result.revision.get(),
            event_type: "inbound.asn.cancelled",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.cancelled_at,
        },
    )
    .await?;
    Ok(())
}
