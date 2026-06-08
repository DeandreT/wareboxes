use axum::extract::{Query, State};
use axum::Json;
use wareboxes_core::dto::{
    AddBarcode, AddItem, AddItemPackLink, AddSku, BarcodeIdRequest, ItemIdRequest,
    ItemPackLinkIdRequest, ItemUpdate,
};
use wareboxes_core::models::{Item, ItemPackLink};

use crate::auth::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::routes::users::ShowDeleted;
use crate::routes::validate;
use crate::state::AppState;

const PERM: &str = "wms";
const PACKAGING_UNITS: &[&str] = &["each", "case"];
const BARCODE_TYPES: &[&str] = &["code128", "gs1-128", "upc-a", "qr"];

fn ensure_packaging_unit(value: &str) -> AppResult<()> {
    PACKAGING_UNITS
        .contains(&value)
        .then_some(())
        .ok_or_else(|| AppError::bad_request("packaging unit must be each or case"))
}

fn ensure_barcode_type(value: &str) -> AppResult<()> {
    BARCODE_TYPES
        .contains(&value)
        .then_some(())
        .ok_or_else(|| AppError::bad_request("barcode type must be code128, gs1-128, upc-a, or qr"))
}

fn normalize_barcode_value(barcode_type: &str, value: &str) -> AppResult<String> {
    wareboxes_barcodes::normalized_value(barcode_type, value)
        .map_err(|err| AppError::bad_request(format!("invalid barcode: {err}")))
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<Item>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::items::get_items(&state.db, q.show_deleted).await?,
    ))
}

pub async fn list_pack_links(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ShowDeleted>,
) -> AppResult<Json<Vec<ItemPackLink>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::items::get_item_pack_links(&state.db, q.show_deleted).await?,
    ))
}

pub async fn add(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AddItem>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    ensure_packaging_unit(&body.packaging_unit)?;
    let id = repo::items::add_item(
        &state.db,
        &body.description,
        body.notes.as_deref(),
        &body.packaging_unit,
        body.length,
        body.width,
        body.height,
        body.length_uom.as_deref(),
        body.weight,
        body.weight_uom.as_deref(),
    )
    .await?;
    Ok(Json(id))
}

pub async fn update(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<ItemUpdate>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    if let Some(packaging_unit) = &body.packaging_unit {
        ensure_packaging_unit(packaging_unit)?;
    }
    let ok = repo::items::update_item(
        &state.db,
        body.item_id,
        body.description.as_deref(),
        body.notes.as_deref(),
        body.packaging_unit.as_deref(),
    )
    .await?;
    Ok(Json(ok))
}

pub async fn delete(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<ItemIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::items::set_item_deleted(&state.db, body.item_id, true).await?,
    ))
}

pub async fn restore(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<ItemIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::items::set_item_deleted(&state.db, body.item_id, false).await?,
    ))
}

pub async fn add_pack_link(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AddItemPackLink>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id = repo::items::add_item_pack_link(
        &state.db,
        body.master_item_id,
        body.single_item_id,
        body.inner_qty,
        body.notes.as_deref(),
    )
    .await?;
    Ok(Json(id))
}

pub async fn delete_pack_link(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<ItemPackLinkIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::items::set_item_pack_link_deleted(&state.db, body.item_pack_link_id, true).await?,
    ))
}

pub async fn add_sku(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AddSku>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let id =
        repo::items::add_sku(&state.db, body.item_id, &body.name, body.notes.as_deref()).await?;
    Ok(Json(id))
}

pub async fn add_barcode(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AddBarcode>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    ensure_barcode_type(&body.r#type)?;
    let name = normalize_barcode_value(&body.r#type, &body.name)?;
    if let Some(owner_item_id) = repo::items::active_barcode_item_by_name(&state.db, &name).await? {
        if owner_item_id != body.item_id {
            return Err(AppError::conflict(
                "barcode value is already assigned to another item",
            ));
        }
    }
    let id = repo::items::add_barcode(
        &state.db,
        body.item_id,
        &name,
        &body.r#type,
        body.notes.as_deref(),
    )
    .await?;
    Ok(Json(id))
}

pub async fn delete_barcode(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<BarcodeIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::items::set_barcode_deleted(&state.db, body.barcode_id, true).await?,
    ))
}
