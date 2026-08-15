use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    CreateValueAddedWorkRequest, OpaqueCursor, Revision,
    ValueAddedInventoryStatus as ApiInventoryStatus, ValueAddedWorkEventResponse,
    ValueAddedWorkInputResponse, ValueAddedWorkKind as ApiKind, ValueAddedWorkLifecycleRequest,
    ValueAddedWorkOutputResponse, ValueAddedWorkPageRequest, ValueAddedWorkPageResponse,
    ValueAddedWorkResponse, ValueAddedWorkStatus as ApiStatus,
};
use wareboxes_application::value_added_work::{
    CreateValueAddedWorkCommand, CreateValueAddedWorkInput, CreateValueAddedWorkOutput,
    ValueAddedWorkEventReadModel, ValueAddedWorkFilter, ValueAddedWorkInputReadModel,
    ValueAddedWorkLifecycleCommand, ValueAddedWorkOutputReadModel, ValueAddedWorkReadModel,
};
use wareboxes_domain::{
    FacilityId, InventoryBalanceId, InventoryOwnerId, ItemBatchId, LicensePlateId, LocationId,
    ValueAddedInventoryStatus, ValueAddedQuantity, ValueAddedRevision, ValueAddedWorkId,
    ValueAddedWorkKind, ValueAddedWorkNote, ValueAddedWorkNumber, ValueAddedWorkStatus,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const CURSOR_PREFIX: &str = "vas1.";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<ValueAddedWorkPageRequest>,
) -> V1Result<Json<ValueAddedWorkPageResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let before_id = request
        .cursor
        .as_ref()
        .map(|cursor| decode_cursor(cursor, &request))
        .transpose()?;
    let result = repo::value_added_work::list(
        &state.db,
        &user.tenant,
        &ValueAddedWorkFilter {
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
    let next_cursor = result
        .next_before_id
        .map(|id| encode_cursor(id, &request))
        .transpose()?;
    Ok(Json(ValueAddedWorkPageResponse {
        items: result
            .items
            .into_iter()
            .map(map_work)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor,
    }))
}

pub async fn get(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(work_id): Path<i64>,
) -> V1Result<Json<ValueAddedWorkResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let result = repo::value_added_work::get(
        &state.db,
        &user.tenant,
        ValueAddedWorkId::new(work_id).map_err(validation)?,
    )
    .await?;
    Ok(Json(map_work(result)?))
}

pub async fn create(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreateValueAddedWorkRequest>,
) -> V1Result<Json<ValueAddedWorkResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = CreateValueAddedWorkCommand {
        inventory_owner_id: InventoryOwnerId::new(body.inventory_owner_id).map_err(validation)?,
        facility_id: FacilityId::new(body.facility_id).map_err(validation)?,
        number: ValueAddedWorkNumber::new(body.number).map_err(validation)?,
        kind: kind_from_api(body.kind),
        note: body
            .note
            .map(ValueAddedWorkNote::new)
            .transpose()
            .map_err(validation)?,
        inputs: body
            .inputs
            .into_iter()
            .map(|input| {
                Ok(CreateValueAddedWorkInput {
                    inventory_balance_id: InventoryBalanceId::new(input.inventory_balance_id)
                        .map_err(validation)?,
                    quantity: ValueAddedQuantity::new(input.quantity).map_err(validation)?,
                })
            })
            .collect::<V1Result<Vec<_>>>()?,
        outputs: body
            .outputs
            .into_iter()
            .map(|output| {
                Ok(CreateValueAddedWorkOutput {
                    location_id: LocationId::new(output.location_id).map_err(validation)?,
                    license_plate_id: output
                        .license_plate_id
                        .map(LicensePlateId::new)
                        .transpose()
                        .map_err(validation)?,
                    item_batch_id: ItemBatchId::new(output.item_batch_id).map_err(validation)?,
                    inventory_status: inventory_status_from_api(output.inventory_status),
                    quantity: ValueAddedQuantity::new(output.quantity).map_err(validation)?,
                })
            })
            .collect::<V1Result<Vec<_>>>()?,
    };
    let result = repo::value_added_work::create(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_work(result)?))
}

pub async fn release(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(work_id): Path<i64>,
    Json(body): Json<ValueAddedWorkLifecycleRequest>,
) -> V1Result<Json<ValueAddedWorkResponse>> {
    lifecycle(state, user, idempotency_key, work_id, body, "release").await
}

pub async fn complete(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(work_id): Path<i64>,
    Json(body): Json<ValueAddedWorkLifecycleRequest>,
) -> V1Result<Json<ValueAddedWorkResponse>> {
    lifecycle(state, user, idempotency_key, work_id, body, "complete").await
}

pub async fn cancel(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(work_id): Path<i64>,
    Json(body): Json<ValueAddedWorkLifecycleRequest>,
) -> V1Result<Json<ValueAddedWorkResponse>> {
    lifecycle(state, user, idempotency_key, work_id, body, "cancel").await
}

async fn lifecycle(
    state: AppState,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    work_id: i64,
    body: ValueAddedWorkLifecycleRequest,
    transition: &str,
) -> V1Result<Json<ValueAddedWorkResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = ValueAddedWorkLifecycleCommand {
        work_id: ValueAddedWorkId::new(work_id).map_err(validation)?,
        expected_revision: ValueAddedRevision::new(body.expected_revision.get())
            .map_err(validation)?,
        note: ValueAddedWorkNote::new(body.note).map_err(validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = match transition {
        "release" => {
            repo::value_added_work::release(&state.db, &user.tenant, &context, &command).await?
        }
        "complete" => {
            repo::value_added_work::complete(&state.db, &user.tenant, &context, &command).await?
        }
        "cancel" => {
            repo::value_added_work::cancel(&state.db, &user.tenant, &context, &command).await?
        }
        _ => return Err(V1Error::internal("invalid value-added lifecycle route")),
    };
    Ok(Json(map_work(result)?))
}

fn map_work(value: ValueAddedWorkReadModel) -> V1Result<ValueAddedWorkResponse> {
    Ok(ValueAddedWorkResponse {
        work_id: value.work_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        inventory_owner_name: value.inventory_owner_name,
        facility_id: value.facility_id.get(),
        facility_name: value.facility_name,
        number: value.number,
        kind: kind_to_api(value.kind),
        status: status_to_api(value.status),
        revision: Revision::new(value.revision.get()).map_err(invalid_result)?,
        note: value.note,
        inputs: value.inputs.into_iter().map(map_input).collect(),
        outputs: value.outputs.into_iter().map(map_output).collect(),
        completion_inventory_transaction_id: value.completion_inventory_transaction_id,
        billable_event_id: value.billable_event_id.map(|id| id.get()),
        created_by: value.created_by.get(),
        created_at: value.created_at.to_rfc3339(),
        released_by: value.released_by.map(|id| id.get()),
        released_at: value.released_at.map(|time| time.to_rfc3339()),
        completed_by: value.completed_by.map(|id| id.get()),
        completed_at: value.completed_at.map(|time| time.to_rfc3339()),
        cancelled_by: value.cancelled_by.map(|id| id.get()),
        cancelled_at: value.cancelled_at.map(|time| time.to_rfc3339()),
        events: value
            .events
            .into_iter()
            .map(map_event)
            .collect::<V1Result<Vec<_>>>()?,
    })
}

fn map_input(value: ValueAddedWorkInputReadModel) -> ValueAddedWorkInputResponse {
    ValueAddedWorkInputResponse {
        input_id: value.input_id.get(),
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
        inventory_status: inventory_status_to_api(value.inventory_status),
        quantity: value.quantity.get(),
        hold_id: value.hold_id.map(|id| id.get()),
    }
}

fn map_output(value: ValueAddedWorkOutputReadModel) -> ValueAddedWorkOutputResponse {
    ValueAddedWorkOutputResponse {
        output_id: value.output_id.get(),
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
        inventory_status: inventory_status_to_api(value.inventory_status),
        quantity: value.quantity.get(),
    }
}

fn map_event(value: ValueAddedWorkEventReadModel) -> V1Result<ValueAddedWorkEventResponse> {
    Ok(ValueAddedWorkEventResponse {
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

const fn kind_from_api(value: ApiKind) -> ValueAddedWorkKind {
    match value {
        ApiKind::Relabel => ValueAddedWorkKind::Relabel,
        ApiKind::Refurbishment => ValueAddedWorkKind::Refurbishment,
        ApiKind::Kit => ValueAddedWorkKind::Kit,
        ApiKind::Dekit => ValueAddedWorkKind::Dekit,
        ApiKind::Assembly => ValueAddedWorkKind::Assembly,
        ApiKind::ValueAddedService => ValueAddedWorkKind::ValueAddedService,
    }
}

const fn kind_to_api(value: ValueAddedWorkKind) -> ApiKind {
    match value {
        ValueAddedWorkKind::Relabel => ApiKind::Relabel,
        ValueAddedWorkKind::Refurbishment => ApiKind::Refurbishment,
        ValueAddedWorkKind::Kit => ApiKind::Kit,
        ValueAddedWorkKind::Dekit => ApiKind::Dekit,
        ValueAddedWorkKind::Assembly => ApiKind::Assembly,
        ValueAddedWorkKind::ValueAddedService => ApiKind::ValueAddedService,
    }
}

const fn status_from_api(value: ApiStatus) -> ValueAddedWorkStatus {
    match value {
        ApiStatus::Draft => ValueAddedWorkStatus::Draft,
        ApiStatus::Released => ValueAddedWorkStatus::Released,
        ApiStatus::Completed => ValueAddedWorkStatus::Completed,
        ApiStatus::Cancelled => ValueAddedWorkStatus::Cancelled,
    }
}

const fn status_to_api(value: ValueAddedWorkStatus) -> ApiStatus {
    match value {
        ValueAddedWorkStatus::Draft => ApiStatus::Draft,
        ValueAddedWorkStatus::Released => ApiStatus::Released,
        ValueAddedWorkStatus::Completed => ApiStatus::Completed,
        ValueAddedWorkStatus::Cancelled => ApiStatus::Cancelled,
    }
}

const fn inventory_status_from_api(value: ApiInventoryStatus) -> ValueAddedInventoryStatus {
    match value {
        ApiInventoryStatus::Available => ValueAddedInventoryStatus::Available,
        ApiInventoryStatus::Hold => ValueAddedInventoryStatus::Hold,
        ApiInventoryStatus::Damaged => ValueAddedInventoryStatus::Damaged,
        ApiInventoryStatus::Quarantine => ValueAddedInventoryStatus::Quarantine,
    }
}

const fn inventory_status_to_api(value: ValueAddedInventoryStatus) -> ApiInventoryStatus {
    match value {
        ValueAddedInventoryStatus::Available => ApiInventoryStatus::Available,
        ValueAddedInventoryStatus::Hold => ApiInventoryStatus::Hold,
        ValueAddedInventoryStatus::Damaged => ApiInventoryStatus::Damaged,
        ValueAddedInventoryStatus::Quarantine => ApiInventoryStatus::Quarantine,
    }
}

fn cursor_filter(request: &ValueAddedWorkPageRequest) -> String {
    format!(
        "{}.{}.{}",
        request
            .inventory_owner_id
            .map_or_else(|| "-".to_owned(), |id| format!("{id:016x}")),
        request
            .facility_id
            .map_or_else(|| "-".to_owned(), |id| format!("{id:016x}")),
        request.status.map_or("-", |status| status_name(status))
    )
}

fn encode_cursor(
    work_id: ValueAddedWorkId,
    request: &ValueAddedWorkPageRequest,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{}.{:016x}",
        cursor_filter(request),
        work_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid value-added work cursor"))
}

fn decode_cursor(
    cursor: &OpaqueCursor,
    request: &ValueAddedWorkPageRequest,
) -> V1Result<ValueAddedWorkId> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("value-added work"))?;
    let (filter, work_id) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for("value-added work"))?;
    if filter != cursor_filter(request) || work_id.len() != 16 {
        return Err(V1Error::invalid_cursor_for("value-added work"));
    }
    let work_id = i64::from_str_radix(work_id, 16)
        .map_err(|_| V1Error::invalid_cursor_for("value-added work"))?;
    ValueAddedWorkId::new(work_id).map_err(|_| V1Error::invalid_cursor_for("value-added work"))
}

const fn status_name(status: ApiStatus) -> &'static str {
    match status {
        ApiStatus::Draft => "draft",
        ApiStatus::Released => "released",
        ApiStatus::Completed => "completed",
        ApiStatus::Cancelled => "cancelled",
    }
}

fn validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

fn invalid_result(error: impl std::fmt::Display) -> V1Error {
    V1Error::internal(error.to_string())
}
