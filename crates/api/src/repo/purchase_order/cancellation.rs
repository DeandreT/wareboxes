use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::purchase_order::{
    CancelPurchaseOrderCommand, CancelPurchaseOrderResult, CANCEL_PURCHASE_ORDER_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    cancel_purchase_order, FacilityId, InventoryOwnerId, PurchaseOrderCancellationId,
    PurchaseOrderStatus,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::{insert_result, PostgresPreparedCommandExt};

use super::{enqueue_event, parse_status, require_stored_visible_before_replay, revision};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

pub async fn cancel(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CancelPurchaseOrderCommand,
) -> AppResult<CancelPurchaseOrderResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CANCEL_PURCHASE_ORDER_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<CancelPurchaseOrderResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id,facility_id,status,revision
        FROM purchase_orders
        WHERE tenant_id=$1 AND id=$2
          AND ($3 OR facility_id=ANY($4))
          AND ($5 OR inventory_owner_id=ANY($6))
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.purchase_order_id().get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("purchase order"))?;
    let current_revision = revision(row.try_get("revision")?)?;
    if current_revision != command.expected_revision() {
        return Err(AppError::conflict(
            "purchase order changed; refresh before cancelling",
        ));
    }
    let previous_status = parse_status(row.try_get::<String, _>("status")?.as_str())?;
    let resulting_revision = cancel_purchase_order(previous_status, current_revision)
        .map_err(|error| AppError::conflict(error.to_string()))?;

    let source_statuses = sqlx::query(
        r#"
        SELECT asn.status
        FROM purchase_order_asn_sources source
        INNER JOIN inbound_asns asn
          ON asn.tenant_id=source.tenant_id AND asn.id=source.asn_id
        WHERE source.tenant_id=$1 AND source.purchase_order_id=$2
        ORDER BY asn.id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.purchase_order_id().get())
    .fetch_all(&mut *tx)
    .await?;
    if source_statuses
        .iter()
        .any(|source| source.get::<String, _>("status") != "cancelled")
    {
        return Err(AppError::conflict(
            "cancel every source ASN before cancelling this purchase order",
        ));
    }

    let inventory_owner_id = InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let facility_id = FacilityId::new(row.try_get("facility_id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let cancelled_at = now_iso();
    let cancellation_id = PurchaseOrderCancellationId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO purchase_order_cancellations
                (tenant_id,inventory_owner_id,facility_id,purchase_order_id,
                 previous_status,reason_code,note,expected_revision,resulting_revision,
                 cancelled_by_user_id,cancelled_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(inventory_owner_id.get())
        .bind(facility_id.get())
        .bind(command.purchase_order_id().get())
        .bind(previous_status.as_str())
        .bind(command.details().reason().as_str())
        .bind(command.details().note().map(|note| note.as_str()))
        .bind(current_revision.get())
        .bind(resulting_revision.get())
        .bind(context.actor_id.get())
        .bind(cancelled_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    sqlx::query(
        "UPDATE purchase_orders SET status='cancelled',revision=$3 WHERE tenant_id=$1 AND id=$2",
    )
    .bind(access.tenant_id.get())
    .bind(command.purchase_order_id().get())
    .bind(resulting_revision.get())
    .execute(&mut *tx)
    .await?;

    let result = CancelPurchaseOrderResult {
        cancellation_id,
        purchase_order_id: command.purchase_order_id(),
        previous_status,
        status: PurchaseOrderStatus::Cancelled,
        revision: resulting_revision,
        reason: command.details().reason(),
        note: command
            .details()
            .note()
            .map(|note| note.as_str().to_owned()),
        cancelled_by: context.actor_id,
        cancelled_at,
    };
    enqueue_event(
        &mut tx,
        access,
        context,
        inventory_owner_id,
        facility_id,
        result.purchase_order_id,
        result.revision,
        "cancelled",
        "inbound.purchase_order.cancelled",
        serde_json::json!({
            "cancellation_id": result.cancellation_id.get(),
            "purchase_order_id": result.purchase_order_id.get(),
            "previous_status": result.previous_status.as_str(),
            "status": result.status.as_str(),
            "revision": result.revision.get(),
            "reason": result.reason.as_str(),
            "note": result.note,
            "cancelled_by": result.cancelled_by.get(),
            "cancelled_at": result.cancelled_at.to_rfc3339(),
        }),
        result.cancelled_at,
    )
    .await?;
    insert_result(&mut tx, &prepared.completed_result(&result, None)?).await?;
    tx.commit().await?;
    Ok(result)
}
