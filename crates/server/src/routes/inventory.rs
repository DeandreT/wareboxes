use axum::extract::{Query, State};
use axum::Json;
use wareboxes_core::dto::{
    AddItemBatch, ItemBatchIdRequest, MoveInventory, ReceiveInventory, ReservationIdRequest,
    ReserveInventory, SplitMoveInventory,
};
use wareboxes_core::models::{
    InventoryBalance, InventoryReconciliationIssue, InventoryReservation, InventoryStatus,
    InventoryTransaction, ItemBatch,
};
use wareboxes_domain::FacilityId;

use crate::auth::CurrentTenant;
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::routes::users::ShowDeleted;
use crate::routes::validate;
use crate::state::AppState;

const PERM: &str = "wms";

async fn require_scoped_location(
    db: &Db,
    user: &CurrentTenant,
    location_id: i64,
    label: &'static str,
) -> AppResult<FacilityId> {
    let facility_id = repo::locations::active_location_facility_in_scope(
        db,
        user.tenant.tenant_id,
        &user.tenant.site_scope,
        location_id,
    )
    .await?
    .ok_or_else(|| AppError::bad_request(format!("{label} not found")))?;
    if repo::locations::location_active_state(db, user.tenant.tenant_id, location_id).await?
        != Some(true)
    {
        return Err(AppError::bad_request(format!("{label} is inactive")));
    }
    FacilityId::new(facility_id).map_err(|error| AppError::internal(error.to_string()))
}

pub async fn list_batches(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<ItemBatch>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::inventory::get_item_batches_in_scope(&state.db, &user.tenant, q.show_deleted).await?,
    ))
}

pub async fn add_batch(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddItemBatch>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    user.require_inventory_owner(body.inventory_owner_id)?;
    if !repo::items::active_item_exists(&state.db, user.tenant.tenant_id, body.item_id).await? {
        return Err(AppError::bad_request("Item not found"));
    }
    if !repo::inventory_owners::active_inventory_owner_exists_in_scope(
        &state.db,
        user.tenant.tenant_id,
        &user.tenant.owner_scope,
        body.inventory_owner_id,
    )
    .await?
    {
        return Err(AppError::bad_request("Inventory owner not found"));
    }
    if let Some(load_id) = body.load_id {
        let Some(dimensions) =
            repo::access::load_dimensions(&state.db, &user.tenant, load_id, false).await?
        else {
            return Err(AppError::bad_request("Load not found"));
        };
        if dimensions.inventory_owner_id.get() != body.inventory_owner_id {
            return Err(AppError::bad_request(
                "Load and item batch must have the same inventory owner",
            ));
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
    if repo::access::item_batch_owner(&state.db, &user.tenant, body.item_batch_id, false)
        .await?
        .is_none()
    {
        return Ok(Json(false));
    }
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
        repo::inventory::get_balances_in_scope(&state.db, &user.tenant, q.show_deleted).await?,
    ))
}

pub async fn list_transactions(
    State(state): State<AppState>,
    user: CurrentTenant,
) -> AppResult<Json<Vec<InventoryTransaction>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::inventory::get_transactions_in_scope(&state.db, &user.tenant).await?,
    ))
}

pub async fn list_reconciliation_issues(
    State(state): State<AppState>,
    user: CurrentTenant,
) -> AppResult<Json<Vec<InventoryReconciliationIssue>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::inventory::get_reconciliation_issues_in_scope(&state.db, &user.tenant).await?,
    ))
}

pub async fn receive(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<ReceiveInventory>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    if repo::access::item_batch_owner(&state.db, &user.tenant, body.item_batch_id, false)
        .await?
        .is_none()
    {
        return Err(AppError::bad_request("Item batch not found"));
    }
    require_scoped_location(
        &state.db,
        &user,
        body.to_location_id,
        "Destination location",
    )
    .await?;
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
    let status = body.status.unwrap_or(InventoryStatus::Available);
    let Some(source) = repo::access::inventory_position_dimensions(
        &state.db,
        &user.tenant,
        body.item_batch_id,
        body.from_location_id,
        status.as_str(),
    )
    .await?
    else {
        return Err(AppError::conflict("source inventory balance was not found"));
    };
    let destination_facility = require_scoped_location(
        &state.db,
        &user,
        body.to_location_id,
        "Destination location",
    )
    .await?;
    if source.facility_id != destination_facility {
        return Err(AppError::conflict(
            "inventory moves cannot cross facilities; use an inventory transfer workflow",
        ));
    }
    let id = repo::inventory::move_inventory(
        &state.db,
        user.tenant.tenant_id,
        user.user.id,
        body.item_batch_id,
        body.from_location_id,
        body.to_location_id,
        body.qty,
        Some(status),
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
    let Some(source) = repo::access::inventory_balance_dimensions(
        &state.db,
        &user.tenant,
        body.from_inventory_balance_id,
        false,
    )
    .await?
    else {
        return Err(AppError::conflict("source inventory balance was not found"));
    };
    for destination in &body.destinations {
        let destination_facility = require_scoped_location(
            &state.db,
            &user,
            destination.to_location_id,
            "Destination location",
        )
        .await?;
        if source.facility_id != destination_facility {
            return Err(AppError::conflict(
                "inventory moves cannot cross facilities; use an inventory transfer workflow",
            ));
        }
    }
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
        repo::inventory::get_reservations_in_scope(&state.db, &user.tenant, q.show_deleted).await?,
    ))
}

pub async fn reserve(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<ReserveInventory>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    if repo::access::inventory_balance_dimensions(
        &state.db,
        &user.tenant,
        body.inventory_balance_id,
        true,
    )
    .await?
    .is_none()
    {
        return Err(AppError::conflict(
            "insufficient available inventory to reserve",
        ));
    }
    if !repo::access::order_is_accessible(&state.db, &user.tenant, body.order_id, true).await? {
        return Err(AppError::conflict(
            "order and inventory balance must have the same tenant and inventory owner",
        ));
    }
    let id = repo::inventory::reserve_inventory(
        &state.db,
        user.tenant.tenant_id,
        body.order_id,
        body.order_item_id,
        body.inventory_balance_id,
        body.qty,
        &body.idempotency_key,
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
    if repo::access::inventory_reservation_dimensions(
        &state.db,
        &user.tenant,
        body.reservation_id,
        true,
    )
    .await?
    .is_none()
    {
        return Ok(Json(false));
    }
    Ok(Json(
        repo::inventory::cancel_reservation(
            &state.db,
            user.tenant.tenant_id,
            body.reservation_id,
            &body.idempotency_key,
        )
        .await?,
    ))
}
