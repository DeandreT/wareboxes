use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    InventoryBalanceStatus, InventoryHoldPage, InventoryHoldPageRequest, InventoryHoldReason,
    InventoryHoldResponse, InventoryHoldStatus, OpaqueCursor, PlaceInventoryHoldRequest,
    PlaceInventoryHoldResponse, ReleaseInventoryHoldRequest, ReleaseInventoryHoldResponse,
};
use wareboxes_core::models::InventoryHoldReason as CoreHoldReason;

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const CURSOR_PREFIX: &str = "ih1.";
const MAX_NOTE_LENGTH: usize = 1_000;
const MAX_REFERENCE_TYPE_LENGTH: usize = 100;

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<InventoryHoldPageRequest>,
) -> V1Result<Json<InventoryHoldPage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let before_id = query.cursor.as_ref().map(decode_cursor).transpose()?;
    let status = query.status.map(map_status_value);
    let page = repo::inventory_hold_v1::get_inventory_hold_page(
        &state.db,
        &user.tenant,
        before_id,
        query.limit.get(),
        status,
    )
    .await?;
    let items = page
        .rows
        .into_iter()
        .map(map_response)
        .collect::<V1Result<Vec<_>>>()?;
    let next_cursor = page.next_before_id.map(encode_cursor).transpose()?;

    Ok(Json(InventoryHoldPage::new(items, next_cursor)))
}

pub async fn place(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<PlaceInventoryHoldRequest>,
) -> V1Result<Json<PlaceInventoryHoldResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    validate_place(&body)?;
    let context = user.command_context(&idempotency_key);
    let result = repo::inventory::place_inventory_hold(
        &state.db,
        &user.tenant,
        &context,
        &repo::inventory::PlaceInventoryHoldCommand {
            inventory_balance_id: body.inventory_balance_id,
            qty: body.quantity,
            reason: map_reason(body.reason),
            note: body.note.as_deref(),
            reference_type: body.reference_type.as_deref(),
            reference_id: body.reference_id,
        },
    )
    .await?;

    Ok(Json(PlaceInventoryHoldResponse {
        hold_id: result.hold_id,
    }))
}

pub async fn release(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(hold_id): Path<i64>,
    Json(_body): Json<ReleaseInventoryHoldRequest>,
) -> V1Result<Json<ReleaseInventoryHoldResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(hold_id, "inventory hold ID")?;
    let context = user.command_context(&idempotency_key);
    let result = repo::inventory::release_inventory_hold(
        &state.db,
        &user.tenant,
        &context,
        &repo::inventory::ReleaseInventoryHoldCommand { hold_id },
    )
    .await?;

    Ok(Json(ReleaseInventoryHoldResponse {
        hold_id: result.hold_id,
        released_quantity: result.released_qty,
    }))
}

fn validate_place(body: &PlaceInventoryHoldRequest) -> V1Result<()> {
    require_positive(body.inventory_balance_id, "inventory balance ID")?;
    require_positive(body.quantity, "quantity")?;
    validate_optional_text(body.note.as_deref(), "note", MAX_NOTE_LENGTH)?;
    validate_optional_text(
        body.reference_type.as_deref(),
        "reference_type",
        MAX_REFERENCE_TYPE_LENGTH,
    )?;
    match (&body.reference_type, body.reference_id) {
        (None, None) | (Some(_), Some(1..)) => {}
        _ => {
            return Err(invalid(
                "reference_type and positive reference_id must be provided together",
            ));
        }
    }
    if body.reason == InventoryHoldReason::Other && body.note.is_none() {
        return Err(invalid("note is required when reason is other"));
    }
    Ok(())
}

fn validate_optional_text(value: Option<&str>, field: &str, maximum: usize) -> V1Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.trim() != value || value.is_empty() {
        return Err(invalid(format!(
            "{field} must be trimmed and nonempty when provided"
        )));
    }
    if value.chars().count() > maximum {
        return Err(invalid(format!(
            "{field} cannot exceed {maximum} characters"
        )));
    }
    Ok(())
}

fn map_response(
    row: repo::inventory_hold_v1::InventoryHoldPageRow,
) -> V1Result<InventoryHoldResponse> {
    Ok(InventoryHoldResponse {
        id: row.id,
        created_at: row.created_at.to_rfc3339(),
        created_by_user_id: row.created_by_user_id,
        released_at: row.released_at.map(|timestamp| timestamp.to_rfc3339()),
        released_by_user_id: row.released_by_user_id,
        inventory_balance_id: row.inventory_balance_id,
        inventory_owner_id: row.inventory_owner_id,
        inventory_owner_name: row.inventory_owner_name,
        facility_id: row.facility_id,
        facility_name: row.facility_name,
        location_id: row.location_id,
        location_barcode: row.location_barcode,
        location_name: row.location_name,
        license_plate_id: row.license_plate_id,
        license_plate_barcode: row.license_plate_barcode,
        item_batch_id: row.item_batch_id,
        lot: row.lot,
        serial: row.serial,
        expiration: row.expiration.map(|timestamp| timestamp.to_rfc3339()),
        item_id: row.item_id,
        item_description: row.item_description,
        uom: row.uom,
        inventory_status: map_inventory_status(&row.inventory_status)?,
        quantity: row.quantity,
        reason: map_hold_reason(&row.reason)?,
        note: row.note,
        reference_type: row.reference_type,
        reference_id: row.reference_id,
        status: map_hold_status(&row.status)?,
    })
}

fn map_inventory_status(value: &str) -> V1Result<InventoryBalanceStatus> {
    match value {
        "available" => Ok(InventoryBalanceStatus::Available),
        "hold" => Ok(InventoryBalanceStatus::Hold),
        "damaged" => Ok(InventoryBalanceStatus::Damaged),
        "quarantine" => Ok(InventoryBalanceStatus::Quarantine),
        _ => Err(V1Error::internal("unknown inventory balance status")),
    }
}

fn map_hold_reason(value: &str) -> V1Result<InventoryHoldReason> {
    match value {
        "quality_inspection" => Ok(InventoryHoldReason::QualityInspection),
        "damage_suspected" => Ok(InventoryHoldReason::DamageSuspected),
        "inventory_discrepancy" => Ok(InventoryHoldReason::InventoryDiscrepancy),
        "regulatory" => Ok(InventoryHoldReason::Regulatory),
        "customer_request" => Ok(InventoryHoldReason::CustomerRequest),
        "other" => Ok(InventoryHoldReason::Other),
        _ => Err(V1Error::internal("unknown inventory hold reason")),
    }
}

fn map_hold_status(value: &str) -> V1Result<InventoryHoldStatus> {
    match value {
        "active" => Ok(InventoryHoldStatus::Active),
        "released" => Ok(InventoryHoldStatus::Released),
        _ => Err(V1Error::internal("unknown inventory hold status")),
    }
}

fn map_reason(reason: InventoryHoldReason) -> CoreHoldReason {
    match reason {
        InventoryHoldReason::QualityInspection => CoreHoldReason::QualityInspection,
        InventoryHoldReason::DamageSuspected => CoreHoldReason::DamageSuspected,
        InventoryHoldReason::InventoryDiscrepancy => CoreHoldReason::InventoryDiscrepancy,
        InventoryHoldReason::Regulatory => CoreHoldReason::Regulatory,
        InventoryHoldReason::CustomerRequest => CoreHoldReason::CustomerRequest,
        InventoryHoldReason::Other => CoreHoldReason::Other,
    }
}

fn map_status_value(status: InventoryHoldStatus) -> &'static str {
    match status {
        InventoryHoldStatus::Active => "active",
        InventoryHoldStatus::Released => "released",
    }
}

fn decode_cursor(cursor: &OpaqueCursor) -> V1Result<i64> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .filter(|encoded| encoded.len() == 16)
        .ok_or_else(|| V1Error::invalid_cursor_for("inventory hold"))?;
    let id = i64::from_str_radix(encoded, 16)
        .map_err(|_| V1Error::invalid_cursor_for("inventory hold"))?;
    if id <= 0 {
        return Err(V1Error::invalid_cursor_for("inventory hold"));
    }
    Ok(id)
}

fn encode_cursor(id: i64) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!("{CURSOR_PREFIX}{id:016x}"))
        .map_err(|_| V1Error::internal("generated an invalid inventory hold cursor"))
}

fn require_positive(value: i64, label: &str) -> V1Result<()> {
    if value > 0 {
        Ok(())
    } else {
        Err(invalid(format!("{label} must be positive")))
    }
}

fn invalid(message: impl Into<String>) -> V1Error {
    AppError::bad_request(message).into()
}
