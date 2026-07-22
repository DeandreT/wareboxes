use axum::extract::{Query, State};
use axum::Json;
use wareboxes_core::dto::{
    AddItemBatch, ItemBatchIdRequest, MoveInventory, ReceiveInventory, ReservationIdRequest,
    ReserveInventory, SplitMoveInventory,
};
use wareboxes_core::models::{
    InventoryBalance, InventoryReconciliationIssue, InventoryReservation, InventoryTransaction,
    ItemBatch,
};

use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::routes::users::ShowDeleted;
use crate::routes::validate;
use crate::state::AppState;

const PERM: &str = "wms";

pub async fn list_batches(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<ItemBatch>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::inventory::get_item_batches(&state.db, user.tenant.tenant_id, q.show_deleted).await?,
    ))
}

pub async fn add_batch(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddItemBatch>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    if !repo::items::active_item_exists(&state.db, user.tenant.tenant_id, body.item_id).await? {
        return Err(AppError::bad_request("Item not found"));
    }
    if !repo::inventory_owners::active_inventory_owner_exists(
        &state.db,
        user.tenant.tenant_id,
        body.inventory_owner_id,
    )
    .await?
    {
        return Err(AppError::bad_request("Inventory owner not found"));
    }
    if let Some(load_id) = body.load_id {
        if !repo::loads::active_load_exists(&state.db, user.tenant.tenant_id, load_id).await? {
            return Err(AppError::bad_request("Load not found"));
        }
    }
    let id = repo::inventory::add_item_batch(
        &state.db,
        user.tenant.tenant_id,
        body.inventory_owner_id,
        body.item_id,
        body.load_id,
        body.lot.as_deref(),
        body.serial.as_deref(),
        body.expiration,
    )
    .await?;
    Ok(Json(id))
}

pub async fn delete_batch(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<ItemBatchIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::inventory::set_item_batch_deleted(
            &state.db,
            user.tenant.tenant_id,
            body.item_batch_id,
            true,
        )
        .await?,
    ))
}

pub async fn list_balances(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<InventoryBalance>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::inventory::get_balances(&state.db, user.tenant.tenant_id, q.show_deleted).await?,
    ))
}

pub async fn list_transactions(
    State(state): State<AppState>,
    user: CurrentTenant,
) -> AppResult<Json<Vec<InventoryTransaction>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::inventory::get_transactions(&state.db, user.tenant.tenant_id).await?,
    ))
}

pub async fn list_reconciliation_issues(
    State(state): State<AppState>,
    user: CurrentTenant,
) -> AppResult<Json<Vec<InventoryReconciliationIssue>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::inventory::get_reconciliation_issues(&state.db, user.tenant.tenant_id).await?,
    ))
}

pub async fn receive(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<ReceiveInventory>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id = repo::inventory::receive_inventory(
        &state.db,
        user.tenant.tenant_id,
        user.user.id,
        body.item_batch_id,
        body.to_location_id,
        body.qty,
        body.status,
        body.reason.as_deref(),
        body.reference_type.as_deref(),
        body.reference_id,
        Some(&body.idempotency_key),
    )
    .await?;
    Ok(Json(id))
}

pub async fn move_stock(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<MoveInventory>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id = repo::inventory::move_inventory(
        &state.db,
        user.tenant.tenant_id,
        user.user.id,
        body.item_batch_id,
        body.from_location_id,
        body.to_location_id,
        body.qty,
        body.status,
        body.reason.as_deref(),
        body.reference_type.as_deref(),
        body.reference_id,
        Some(&body.idempotency_key),
    )
    .await?;
    Ok(Json(id))
}

pub async fn split_move_stock(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<SplitMoveInventory>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let destinations = body
        .destinations
        .iter()
        .map(|destination| (destination.to_location_id, destination.qty))
        .collect::<Vec<_>>();
    let ids = repo::inventory::split_move_inventory(
        &state.db,
        user.tenant.tenant_id,
        user.user.id,
        body.from_inventory_balance_id,
        &destinations,
        body.reason.as_deref(),
        body.reference_type.as_deref(),
        body.reference_id,
        Some(&body.idempotency_key),
    )
    .await?;
    Ok(Json(ids))
}

pub async fn list_reservations(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<InventoryReservation>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::inventory::get_reservations(&state.db, user.tenant.tenant_id, q.show_deleted).await?,
    ))
}

pub async fn reserve(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<ReserveInventory>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id = repo::inventory::reserve_inventory(
        &state.db,
        user.tenant.tenant_id,
        body.order_id,
        body.order_item_id,
        body.inventory_balance_id,
        body.qty,
    )
    .await?;
    Ok(Json(id))
}

pub async fn cancel_reservation(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<ReservationIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::inventory::cancel_reservation(&state.db, user.tenant.tenant_id, body.reservation_id)
            .await?,
    ))
}
