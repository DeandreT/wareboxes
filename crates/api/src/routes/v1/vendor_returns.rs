use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    CreateVendorReturnRequest, OpaqueCursor, Revision, VendorReturnEventResponse,
    VendorReturnLifecycleRequest, VendorReturnLineResponse, VendorReturnPageRequest,
    VendorReturnPageResponse, VendorReturnReason as ApiReason, VendorReturnResponse,
    VendorReturnStatus as ApiStatus,
};
use wareboxes_application::vendor_return::{
    CreateVendorReturnCommand, CreateVendorReturnLine, VendorReturnEventReadModel,
    VendorReturnFilter, VendorReturnLifecycleCommand, VendorReturnLineReadModel,
    VendorReturnReadModel,
};
use wareboxes_domain::{
    FacilityId, InventoryBalanceId, InventoryOwnerId, VendorName, VendorReference, VendorReturnId,
    VendorReturnNote, VendorReturnNumber, VendorReturnQuantity, VendorReturnReason,
    VendorReturnRevision, VendorReturnStatus,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const CURSOR_PREFIX: &str = "vr1.";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<VendorReturnPageRequest>,
) -> V1Result<Json<VendorReturnPageResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let before_id = request
        .cursor
        .as_ref()
        .map(|cursor| decode_cursor(cursor, &request))
        .transpose()?;
    let result = repo::vendor_return::list(
        &state.db,
        &user.tenant,
        &VendorReturnFilter {
            inventory_owner_id: request
                .inventory_owner_id
                .map(InventoryOwnerId::new)
                .transpose()
                .map_err(validation)?,
            facility_id: request
                .facility_id
                .map(FacilityId::new)
                .transpose()
                .map_err(validation)?,
            status: request.status.map(status_from_api),
            before_id,
            limit: u32::from(request.limit.get()),
        },
    )
    .await?;
    Ok(Json(VendorReturnPageResponse {
        items: result
            .items
            .into_iter()
            .map(map_return)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor: result
            .next_before_id
            .map(|id| encode_cursor(id, &request))
            .transpose()?,
    }))
}

pub async fn get(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(vendor_return_id): Path<i64>,
) -> V1Result<Json<VendorReturnResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let value = repo::vendor_return::get(
        &state.db,
        &user.tenant,
        VendorReturnId::new(vendor_return_id).map_err(validation)?,
    )
    .await?;
    Ok(Json(map_return(value)?))
}

pub async fn create(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreateVendorReturnRequest>,
) -> V1Result<Json<VendorReturnResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = CreateVendorReturnCommand {
        inventory_owner_id: InventoryOwnerId::new(body.inventory_owner_id).map_err(validation)?,
        facility_id: FacilityId::new(body.facility_id).map_err(validation)?,
        number: VendorReturnNumber::new(body.number).map_err(validation)?,
        vendor_name: VendorName::new(body.vendor_name).map_err(validation)?,
        vendor_reference: body
            .vendor_reference
            .map(VendorReference::new)
            .transpose()
            .map_err(validation)?,
        note: body
            .note
            .map(VendorReturnNote::new)
            .transpose()
            .map_err(validation)?,
        lines: body
            .lines
            .into_iter()
            .map(|line| {
                Ok(CreateVendorReturnLine {
                    inventory_balance_id: InventoryBalanceId::new(line.inventory_balance_id)
                        .map_err(validation)?,
                    quantity: VendorReturnQuantity::new(line.quantity).map_err(validation)?,
                    reason: reason_from_api(line.reason),
                    note: line
                        .note
                        .map(VendorReturnNote::new)
                        .transpose()
                        .map_err(validation)?,
                })
            })
            .collect::<V1Result<Vec<_>>>()?,
    };
    let result = repo::vendor_return::create(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_return(result)?))
}

macro_rules! lifecycle_handler {
    ($name:ident, $transition:literal) => {
        pub async fn $name(
            State(state): State<AppState>,
            user: CurrentTenant,
            idempotency_key: IdempotencyKey,
            Path(vendor_return_id): Path<i64>,
            Json(body): Json<VendorReturnLifecycleRequest>,
        ) -> V1Result<Json<VendorReturnResponse>> {
            lifecycle(
                state,
                user,
                idempotency_key,
                vendor_return_id,
                body,
                $transition,
            )
            .await
        }
    };
}

lifecycle_handler!(release, "release");
lifecycle_handler!(ship, "ship");
lifecycle_handler!(cancel, "cancel");

async fn lifecycle(
    state: AppState,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    vendor_return_id: i64,
    body: VendorReturnLifecycleRequest,
    transition: &str,
) -> V1Result<Json<VendorReturnResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = VendorReturnLifecycleCommand {
        vendor_return_id: VendorReturnId::new(vendor_return_id).map_err(validation)?,
        expected_revision: VendorReturnRevision::new(body.expected_revision.get())
            .map_err(validation)?,
        note: VendorReturnNote::new(body.note).map_err(validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = match transition {
        "release" => {
            repo::vendor_return::release(&state.db, &user.tenant, &context, &command).await?
        }
        "ship" => repo::vendor_return::ship(&state.db, &user.tenant, &context, &command).await?,
        "cancel" => {
            repo::vendor_return::cancel(&state.db, &user.tenant, &context, &command).await?
        }
        _ => return Err(V1Error::internal("invalid vendor-return lifecycle route")),
    };
    Ok(Json(map_return(result)?))
}

fn map_return(value: VendorReturnReadModel) -> V1Result<VendorReturnResponse> {
    Ok(VendorReturnResponse {
        vendor_return_id: value.vendor_return_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        inventory_owner_name: value.inventory_owner_name,
        facility_id: value.facility_id.get(),
        facility_name: value.facility_name,
        number: value.number,
        vendor_name: value.vendor_name,
        vendor_reference: value.vendor_reference,
        status: status_to_api(value.status),
        revision: Revision::new(value.revision.get()).map_err(invalid_result)?,
        note: value.note,
        lines: value.lines.into_iter().map(map_line).collect(),
        shipment_inventory_transaction_id: value.shipment_inventory_transaction_id,
        billable_event_id: value.billable_event_id.map(|id| id.get()),
        created_by: value.created_by.get(),
        created_at: value.created_at.to_rfc3339(),
        released_by: value.released_by.map(|id| id.get()),
        released_at: value.released_at.map(|time| time.to_rfc3339()),
        shipped_by: value.shipped_by.map(|id| id.get()),
        shipped_at: value.shipped_at.map(|time| time.to_rfc3339()),
        cancelled_by: value.cancelled_by.map(|id| id.get()),
        cancelled_at: value.cancelled_at.map(|time| time.to_rfc3339()),
        events: value
            .events
            .into_iter()
            .map(map_event)
            .collect::<V1Result<Vec<_>>>()?,
    })
}

fn map_line(value: VendorReturnLineReadModel) -> VendorReturnLineResponse {
    VendorReturnLineResponse {
        line_id: value.line_id.get(),
        inventory_balance_id: value.inventory_balance_id.get(),
        location_id: value.location_id.get(),
        location_code: value.location_code,
        license_plate_id: value.license_plate_id.map(|id| id.get()),
        license_plate_number: value.license_plate_number,
        item_batch_id: value.item_batch_id.get(),
        item_id: value.item_id,
        item_description: value.item_description,
        uom: value.uom,
        lot: value.lot,
        serial: value.serial,
        inventory_status: value.inventory_status,
        quantity: value.quantity.get(),
        reason: reason_to_api(value.reason),
        note: value.note,
        hold_id: value.hold_id.map(|id| id.get()),
    }
}

fn map_event(value: VendorReturnEventReadModel) -> V1Result<VendorReturnEventResponse> {
    Ok(VendorReturnEventResponse {
        event_id: value.event_id.get(),
        from_status: value.from_status.map(status_to_api),
        to_status: status_to_api(value.to_status),
        note: value.note,
        resulting_revision: Revision::new(value.resulting_revision.get())
            .map_err(invalid_result)?,
        actor_id: value.actor_id.get(),
        occurred_at: value.occurred_at.to_rfc3339(),
    })
}

const fn reason_from_api(value: ApiReason) -> VendorReturnReason {
    match value {
        ApiReason::Damaged => VendorReturnReason::Damaged,
        ApiReason::Defective => VendorReturnReason::Defective,
        ApiReason::Expired => VendorReturnReason::Expired,
        ApiReason::Recall => VendorReturnReason::Recall,
        ApiReason::Overstock => VendorReturnReason::Overstock,
        ApiReason::VendorRequest => VendorReturnReason::VendorRequest,
        ApiReason::Other => VendorReturnReason::Other,
    }
}

const fn reason_to_api(value: VendorReturnReason) -> ApiReason {
    match value {
        VendorReturnReason::Damaged => ApiReason::Damaged,
        VendorReturnReason::Defective => ApiReason::Defective,
        VendorReturnReason::Expired => ApiReason::Expired,
        VendorReturnReason::Recall => ApiReason::Recall,
        VendorReturnReason::Overstock => ApiReason::Overstock,
        VendorReturnReason::VendorRequest => ApiReason::VendorRequest,
        VendorReturnReason::Other => ApiReason::Other,
    }
}

const fn status_from_api(value: ApiStatus) -> VendorReturnStatus {
    match value {
        ApiStatus::Draft => VendorReturnStatus::Draft,
        ApiStatus::Released => VendorReturnStatus::Released,
        ApiStatus::Shipped => VendorReturnStatus::Shipped,
        ApiStatus::Cancelled => VendorReturnStatus::Cancelled,
    }
}

const fn status_to_api(value: VendorReturnStatus) -> ApiStatus {
    match value {
        VendorReturnStatus::Draft => ApiStatus::Draft,
        VendorReturnStatus::Released => ApiStatus::Released,
        VendorReturnStatus::Shipped => ApiStatus::Shipped,
        VendorReturnStatus::Cancelled => ApiStatus::Cancelled,
    }
}

fn cursor_filter(request: &VendorReturnPageRequest) -> String {
    format!(
        "{}.{}.{}",
        request
            .inventory_owner_id
            .map_or_else(|| "-".to_owned(), |id| format!("{id:016x}")),
        request
            .facility_id
            .map_or_else(|| "-".to_owned(), |id| format!("{id:016x}")),
        request.status.map_or("-", status_name)
    )
}

fn encode_cursor(
    vendor_return_id: VendorReturnId,
    request: &VendorReturnPageRequest,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{}.{:016x}",
        cursor_filter(request),
        vendor_return_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid vendor-return cursor"))
}

fn decode_cursor(
    cursor: &OpaqueCursor,
    request: &VendorReturnPageRequest,
) -> V1Result<VendorReturnId> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("vendor returns"))?;
    let (filter, encoded_id) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for("vendor returns"))?;
    if filter != cursor_filter(request) {
        return Err(V1Error::invalid_cursor_for("vendor-return filters"));
    }
    let id = i64::from_str_radix(encoded_id, 16)
        .map_err(|_| V1Error::invalid_cursor_for("vendor returns"))?;
    VendorReturnId::new(id).map_err(|_| V1Error::invalid_cursor_for("vendor returns"))
}

const fn status_name(value: ApiStatus) -> &'static str {
    match value {
        ApiStatus::Draft => "draft",
        ApiStatus::Released => "released",
        ApiStatus::Shipped => "shipped",
        ApiStatus::Cancelled => "cancelled",
    }
}

fn validation(error: impl std::fmt::Display) -> V1Error {
    V1Error::from(AppError::bad_request(error.to_string()))
}

fn invalid_result(error: impl std::fmt::Display) -> V1Error {
    V1Error::internal(error.to_string())
}
