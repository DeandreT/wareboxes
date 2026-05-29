use axum::extract::{Query, State};
use axum::Json;
use wareboxes_core::dto::{
    AddItemBatch, ItemBatchIdRequest, MoveInventory, ReceiveInventory, ReservationIdRequest,
    ReserveInventory,
};
use wareboxes_core::models::{InventoryBalance, InventoryReservation, ItemBatch, Movement};

use crate::auth::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::routes::users::ShowDeleted;
use crate::routes::validate;
use crate::state::AppState;

const PERM: &str = "wms";

pub async fn list_batches(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<ItemBatch>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::inventory::get_item_batches(&state.db, q.show_deleted).await?,
    ))
}

pub async fn add_batch(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AddItemBatch>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    if !repo::items::active_item_exists(&state.db, body.item_id).await? {
        return Err(AppError::bad_request("Item not found"));
    }
    if let Some(load_id) = body.load_id {
        if !repo::loads::active_load_exists(&state.db, load_id).await? {
            return Err(AppError::bad_request("Load not found"));
        }
    }
    let id = repo::inventory::add_item_batch(
        &state.db,
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
    user: CurrentUser,
    Json(body): Json<ItemBatchIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::inventory::set_item_batch_deleted(&state.db, body.item_batch_id, true).await?,
    ))
}

pub async fn list_balances(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<InventoryBalance>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::inventory::get_balances(&state.db, q.show_deleted).await?,
    ))
}

pub async fn list_movements(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Vec<Movement>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(repo::inventory::get_movements(&state.db).await?))
}

pub async fn receive(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<ReceiveInventory>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id = repo::inventory::receive_inventory(
        &state.db,
        user.user.id,
        body.item_batch_id,
        body.to_location_id,
        body.qty,
        body.status,
        body.reason.as_deref(),
        body.reference_type.as_deref(),
        body.reference_id,
        body.idempotency_key.as_deref(),
    )
    .await?;
    Ok(Json(id))
}

pub async fn move_stock(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<MoveInventory>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id = repo::inventory::move_inventory(
        &state.db,
        user.user.id,
        body.item_batch_id,
        body.from_location_id,
        body.to_location_id,
        body.qty,
        body.status,
        body.reason.as_deref(),
        body.reference_type.as_deref(),
        body.reference_id,
        body.idempotency_key.as_deref(),
    )
    .await?;
    Ok(Json(id))
}

pub async fn list_reservations(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<InventoryReservation>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::inventory::get_reservations(&state.db, q.show_deleted).await?,
    ))
}

pub async fn reserve(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<ReserveInventory>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id = repo::inventory::reserve_inventory(
        &state.db,
        user.user.id,
        body.order_id,
        body.order_item_id,
        body.item_batch_id,
        body.location_id,
        body.qty,
    )
    .await?;
    Ok(Json(id))
}

pub async fn cancel_reservation(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<ReservationIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::inventory::cancel_reservation(&state.db, body.reservation_id).await?,
    ))
}
