use sqlx::Row;
use wareboxes_application::customer_return::{
    CancelCustomerReturnCommand, CancelCustomerReturnResult, CANCEL_CUSTOMER_RETURN_OPERATION,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    cancel_customer_return, CustomerReturnCancellationId, CustomerReturnCancellationReason,
    CustomerReturnStatus, FacilityId, InventoryOwnerId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::{insert_result, PostgresPreparedCommandExt};

use super::{
    enqueue_return_event, require_stored_visible_before_replay, return_revision, return_status,
};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

pub async fn cancel(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CancelCustomerReturnCommand,
) -> AppResult<CancelCustomerReturnResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CANCEL_CUSTOMER_RETURN_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<CancelCustomerReturnResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let row = sqlx::query(
        r#"
        SELECT customer_return.inbound_asn_id,customer_return.inventory_owner_id,
               customer_return.facility_id,asn.status,asn.revision,asn.load_id
        FROM customer_returns customer_return
        INNER JOIN inbound_asns asn
          ON asn.tenant_id=customer_return.tenant_id
         AND asn.id=customer_return.inbound_asn_id
        WHERE customer_return.tenant_id=$1 AND customer_return.id=$2
          AND ($3 OR customer_return.facility_id=ANY($4))
          AND ($5 OR customer_return.inventory_owner_id=ANY($6))
        FOR UPDATE OF asn
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.customer_return_id().get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("customer return"))?;
    let inbound_asn_id: i64 = row.try_get("inbound_asn_id")?;
    let inventory_owner_id: i64 = row.try_get("inventory_owner_id")?;
    let facility_id: i64 = row.try_get("facility_id")?;
    let previous_status = return_status(row.try_get::<String, _>("status")?.as_str())?;
    let previous_revision = return_revision(row.try_get("revision")?)?;
    if previous_revision != command.expected_revision() {
        return Err(AppError::conflict(
            "customer return changed; refresh before cancelling it",
        ));
    }
    if row.try_get::<Option<i64>, _>("load_id")?.is_some() {
        return Err(AppError::conflict(
            "a planned customer return must be managed through its inbound load",
        ));
    }
    let resulting_revision = cancel_customer_return(previous_status, previous_revision)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let cancelled_at = now_iso();
    let details = command.details();
    let bridge_reason = match details.reason() {
        CustomerReturnCancellationReason::CustomerCancelled => "supplier_cancelled",
        CustomerReturnCancellationReason::DuplicateAuthorization => "duplicate_notice",
        CustomerReturnCancellationReason::ReturnWindowExpired => "order_changed",
        CustomerReturnCancellationReason::Other => "other",
    };
    let inbound_cancellation_id: i64 = sqlx::query_scalar(
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
    .bind(inbound_asn_id)
    .bind(previous_revision.get())
    .bind(resulting_revision.get())
    .bind(bridge_reason)
    .bind(details.note())
    .bind(context.actor_id.get())
    .bind(cancelled_at)
    .fetch_one(&mut *tx)
    .await?;
    let updated = sqlx::query(
        r#"
        UPDATE inbound_asns SET status='cancelled',revision=$1
        WHERE tenant_id=$2 AND id=$3 AND status='open' AND revision=$4 AND load_id IS NULL
        "#,
    )
    .bind(resulting_revision.get())
    .bind(access.tenant_id.get())
    .bind(inbound_asn_id)
    .bind(previous_revision.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "customer return changed while it was cancelled",
        ));
    }
    let cancellation_id = CustomerReturnCancellationId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO customer_return_cancellations
                (tenant_id,inventory_owner_id,facility_id,customer_return_id,inbound_asn_id,
                 inbound_asn_cancellation_id,expected_return_revision,
                 resulting_return_revision,reason_code,note,cancelled_by_user_id,cancelled_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(inventory_owner_id)
        .bind(facility_id)
        .bind(command.customer_return_id().get())
        .bind(inbound_asn_id)
        .bind(inbound_cancellation_id)
        .bind(previous_revision.get())
        .bind(resulting_revision.get())
        .bind(details.reason().as_str())
        .bind(details.note())
        .bind(context.actor_id.get())
        .bind(cancelled_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    let result = CancelCustomerReturnResult {
        cancellation_id,
        customer_return_id: command.customer_return_id(),
        previous_status,
        status: CustomerReturnStatus::Cancelled,
        revision: resulting_revision,
        reason: details.reason(),
        note: details.note().map(str::to_owned),
        cancelled_by: context.actor_id,
        cancelled_at,
    };
    let payload = serde_json::json!({
        "cancellation_id": result.cancellation_id.get(),
        "customer_return_id": result.customer_return_id.get(),
        "previous_status": result.previous_status.as_str(),
        "status": result.status.as_str(),
        "revision": result.revision.get(),
        "reason": result.reason.as_str(),
        "note": result.note,
        "cancelled_by": result.cancelled_by.get(),
        "cancelled_at": result.cancelled_at,
    });
    enqueue_return_event(
        &mut tx,
        access,
        context,
        InventoryOwnerId::new(inventory_owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        FacilityId::new(facility_id).map_err(|error| AppError::internal(error.to_string()))?,
        result.customer_return_id,
        result.revision.get(),
        "inbound.customer_return.cancelled",
        &payload,
        cancelled_at,
    )
    .await?;
    insert_result(&mut tx, &prepared.completed_result(&result, None)?).await?;
    tx.commit().await?;
    Ok(result)
}
