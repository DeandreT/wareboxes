use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    InventoryBalanceStatus, InventoryHoldPage, InventoryHoldPageRequest, InventoryHoldReason,
    InventoryHoldResponse, InventoryHoldStatus, OpaqueCursor, PlaceInventoryHoldRequest,
    PlaceInventoryHoldResponse, ReleaseInventoryHoldRequest, ReleaseInventoryHoldResponse,
};
use wareboxes_application::inventory::{
    InventoryBalanceStatus as ApplicationInventoryBalanceStatus, InventoryHoldPageFilter,
    InventoryHoldReadModel, InventoryHoldReason as ApplicationInventoryHoldReason,
    InventoryHoldStatus as ApplicationInventoryHoldStatus,
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
    let page = wareboxes_persistence_postgres::inventory_holds::get_inventory_hold_page(
        &state.db,
        user.tenant.tenant_id,
        &user.tenant.site_scope,
        &user.tenant.owner_scope,
        InventoryHoldPageFilter {
            before_id,
            limit: query.limit.get(),
            status: query.status.map(map_status_filter),
        },
    )
    .await
    .map_err(AppError::from)?;
    let items = page.items.into_iter().map(map_response).collect();
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

fn map_response(row: InventoryHoldReadModel) -> InventoryHoldResponse {
    InventoryHoldResponse {
        id: row.id,
        created_at: row.created_at.to_rfc3339(),
        created_by_user_id: row.created_by_user_id,
        released_at: row.released_at.map(|timestamp| timestamp.to_rfc3339()),
        released_by_user_id: row.released_by_user_id,
        inventory_balance_id: row.inventory_balance_id,
        inventory_owner_id: row.inventory_owner_id.get(),
        inventory_owner_name: row.inventory_owner_name,
        facility_id: row.facility_id.get(),
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
        inventory_status: map_inventory_status(row.inventory_status),
        quantity: row.quantity.get(),
        reason: map_hold_reason(row.reason),
        note: row.note,
        reference_type: row.reference_type,
        reference_id: row.reference_id,
        status: map_hold_status(row.status),
    }
}

fn map_inventory_status(value: ApplicationInventoryBalanceStatus) -> InventoryBalanceStatus {
    match value {
        ApplicationInventoryBalanceStatus::Available => InventoryBalanceStatus::Available,
        ApplicationInventoryBalanceStatus::Hold => InventoryBalanceStatus::Hold,
        ApplicationInventoryBalanceStatus::Damaged => InventoryBalanceStatus::Damaged,
        ApplicationInventoryBalanceStatus::Quarantine => InventoryBalanceStatus::Quarantine,
    }
}

fn map_hold_reason(value: ApplicationInventoryHoldReason) -> InventoryHoldReason {
    match value {
        ApplicationInventoryHoldReason::QualityInspection => InventoryHoldReason::QualityInspection,
        ApplicationInventoryHoldReason::DamageSuspected => InventoryHoldReason::DamageSuspected,
        ApplicationInventoryHoldReason::InventoryDiscrepancy => {
            InventoryHoldReason::InventoryDiscrepancy
        }
        ApplicationInventoryHoldReason::Regulatory => InventoryHoldReason::Regulatory,
        ApplicationInventoryHoldReason::CustomerRequest => InventoryHoldReason::CustomerRequest,
        ApplicationInventoryHoldReason::Other => InventoryHoldReason::Other,
    }
}

fn map_hold_status(value: ApplicationInventoryHoldStatus) -> InventoryHoldStatus {
    match value {
        ApplicationInventoryHoldStatus::Active => InventoryHoldStatus::Active,
        ApplicationInventoryHoldStatus::Released => InventoryHoldStatus::Released,
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

fn map_status_filter(status: InventoryHoldStatus) -> ApplicationInventoryHoldStatus {
    match status {
        InventoryHoldStatus::Active => ApplicationInventoryHoldStatus::Active,
        InventoryHoldStatus::Released => ApplicationInventoryHoldStatus::Released,
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
