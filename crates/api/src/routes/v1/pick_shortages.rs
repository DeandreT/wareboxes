use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    AcceptPickShortageAsShortShipRequest, AcceptPickShortageAsShortShipResponse,
    AllocationExecutionStage as ApiExecutionStage, OpaqueCursor, OrderAllocationOutcome,
    PickOrderStatus, PickShortShipReason as ApiShortShipReason, PickShortageAllocationResponse,
    PickShortageDetails as ApiShortageDetails, PickShortageHoldResponse,
    PickShortageMovementResponse, PickShortagePage as ApiShortagePage, PickShortagePageRequest,
    PickShortageQuantitiesResponse, PickShortageQueueSort, PickShortageQueueSortDirection,
    PickShortageReason as ApiShortageReason, PickShortageResolution as ApiShortageResolution,
    PickShortageResponse, PickShortageStatus as ApiShortageStatus, PickShortageTaskResponse,
    ReallocatePickShortageRequest, ReallocatePickShortageResponse, ReportPickShortageOutcome,
    ReportPickShortageRequest, ReportPickShortageResponse, Revision, ShortShipDemandResponse,
};
use wareboxes_application::picking::{
    AcceptPickShortageAsShortShipCommand, AcceptPickShortageAsShortShipResult,
    PickShortageAllocationReadModel, PickShortagePageQuery, PickShortageQuery,
    PickShortageQueueSort as ApplicationQueueSort,
    PickShortageQueueSortDirection as ApplicationSortDirection, PickShortageReadModel,
    PickShortageTaskReadModel, ReallocatePickShortageCommand, ReallocatePickShortageResult,
    ReportPickShortageCommand, ReportPickShortageOutcome as AppShortageOutcome,
    ReportPickShortageResult,
};
use wareboxes_domain::{
    AllocationExecutionStage, AllocationOutcome, FacilityId, InventoryOwnerId, OrderId, OrderKey,
    OrderRevision, OrderStatus, PickContentId, PickQuantity, PickScanValue, PickShortShipNote,
    PickShortShipReason, PickShortageDetails, PickShortageId, PickShortageNote, PickShortageReason,
    PickShortageResolution, PickShortageRevision, PickShortageStatus, PickTaskId,
    ShortShipDemandQuantities,
};

use super::error::{V1Error, V1Result};
use super::order_allocations::{map_policy, map_strategy};
use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const OPERATOR_PERMISSION: &str = "wms";
const SUPERVISOR_PERMISSION: &str = "wms_supervisor";
const CURSOR_PREFIX: &str = "ps3.";

pub async fn report(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path((task_id, content_id)): Path<(i64, i64)>,
    Json(body): Json<ReportPickShortageRequest>,
) -> V1Result<Json<ReportPickShortageResponse>> {
    user.require_permission(&state.db, OPERATOR_PERMISSION)
        .await?;
    let command = report_command(task_id, content_id, body)?;
    let context = user.command_context(&idempotency_key);
    let result =
        repo::picking::report_shortage(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_report(result)?))
}

pub async fn reallocate(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(shortage_id): Path<i64>,
    Json(body): Json<ReallocatePickShortageRequest>,
) -> V1Result<Json<ReallocatePickShortageResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    let command = ReallocatePickShortageCommand {
        shortage_id: shortage_id_value(shortage_id)?,
        expected_shortage_revision: PickShortageRevision::new(
            body.expected_shortage_revision.get(),
        )
        .map_err(domain_validation)?,
        expected_order_revision: OrderRevision::new(body.expected_order_revision.get())
            .map_err(domain_validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::picking::reallocate_shortage(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_reallocation(result)?))
}

pub async fn accept_short_shipment(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(shortage_id): Path<i64>,
    Json(body): Json<AcceptPickShortageAsShortShipRequest>,
) -> V1Result<Json<AcceptPickShortageAsShortShipResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    let command = short_ship_command(shortage_id, body)?;
    let context = user.command_context(&idempotency_key);
    let result =
        repo::picking::accept_short_shipment(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_short_ship_result(result)?))
}

fn short_ship_command(
    shortage_id: i64,
    body: AcceptPickShortageAsShortShipRequest,
) -> V1Result<AcceptPickShortageAsShortShipCommand> {
    let note = body
        .note
        .map(PickShortShipNote::new)
        .transpose()
        .map_err(domain_validation)?;
    AcceptPickShortageAsShortShipCommand::new(
        shortage_id_value(shortage_id)?,
        PickShortageRevision::new(body.expected_shortage_revision.get())
            .map_err(domain_validation)?,
        OrderRevision::new(body.expected_order_revision.get()).map_err(domain_validation)?,
        map_short_ship_reason_to_domain(body.reason),
        note,
    )
    .map_err(domain_validation)
}

pub async fn get(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(shortage_id): Path<i64>,
) -> V1Result<Json<PickShortageResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    let result = repo::picking::get_shortage(
        &state.db,
        &user.tenant,
        PickShortageQuery {
            shortage_id: shortage_id_value(shortage_id)?,
        },
    )
    .await?;
    Ok(Json(map_shortage(result)?))
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<PickShortagePageRequest>,
) -> V1Result<Json<ApiShortagePage>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    let facility_id = query
        .facility_id
        .map(FacilityId::new)
        .transpose()
        .map_err(domain_validation)?;
    let inventory_owner_id = query
        .inventory_owner_id
        .map(InventoryOwnerId::new)
        .transpose()
        .map_err(domain_validation)?;
    let order_id = query
        .order_id
        .map(OrderId::new)
        .transpose()
        .map_err(domain_validation)?;
    let order_key = query
        .order_key
        .map(|value| OrderKey::new(value.trim().to_owned()))
        .transpose()
        .map_err(domain_validation)?;
    if order_id.is_some() && order_key.is_some() {
        return Err(AppError::bad_request("filter by order_id or order_key, not both").into());
    }
    let status = query.status.map(map_status_to_domain);
    if query.limit.get() > 100 {
        return Err(
            AppError::bad_request("pick shortage page limit must be between 1 and 100").into(),
        );
    }
    let filters = CursorFilters {
        facility_id: facility_id.map(FacilityId::get),
        inventory_owner_id: inventory_owner_id.map(InventoryOwnerId::get),
        order_id: order_id.map(OrderId::get),
        order_key: order_key.as_ref().map(|value| value.as_str().to_owned()),
        status,
        sort: query.sort,
        direction: query.direction,
    };
    let offset = decode_bound_cursor(query.cursor.as_ref(), filters.clone())?;
    let page = repo::picking::list_shortages(
        &state.db,
        &user.tenant,
        PickShortagePageQuery {
            facility_id,
            inventory_owner_id,
            order_id,
            order_key,
            status,
            offset,
            limit: query.limit.get(),
            sort: map_queue_sort(query.sort),
            direction: map_queue_direction(query.direction),
        },
    )
    .await?;
    let items = page
        .items
        .into_iter()
        .map(map_shortage)
        .collect::<V1Result<Vec<_>>>()?;
    let next_cursor = page
        .next_offset
        .map(|offset| encode_cursor(BoundCursor { filters, offset }))
        .transpose()?;
    Ok(Json(ApiShortagePage::new(items, next_cursor)))
}

fn report_command(
    task_id: i64,
    content_id: i64,
    body: ReportPickShortageRequest,
) -> V1Result<ReportPickShortageCommand> {
    let note = body
        .details
        .note
        .map(PickShortageNote::new)
        .transpose()
        .map_err(domain_validation)?;
    let details = PickShortageDetails::new(map_reason_to_domain(body.details.reason), note)
        .map_err(domain_validation)?;
    let outcome = match body.outcome {
        ReportPickShortageOutcome::NoPick {} => AppShortageOutcome::NoPick,
        ReportPickShortageOutcome::Partial {
            picked_quantity,
            destination_license_plate_barcode,
        } => AppShortageOutcome::Partial {
            picked_quantity: PickQuantity::new(picked_quantity).map_err(domain_validation)?,
            destination_license_plate_barcode: scan(
                destination_license_plate_barcode,
                "destination license plate barcode",
            )?,
        },
    };
    Ok(ReportPickShortageCommand {
        task_id: PickTaskId::new(task_id).map_err(domain_validation)?,
        content_id: PickContentId::new(content_id).map_err(domain_validation)?,
        source_location_barcode: scan(body.source_location_barcode, "source location barcode")?,
        source_license_plate_barcode: optional_scan(
            body.source_license_plate_barcode,
            "source license plate barcode",
        )?,
        observed_item_barcode: optional_scan(body.observed_item_barcode, "observed item barcode")?,
        observed_lot: optional_scan(body.observed_lot, "observed lot")?,
        observed_serial: optional_scan(body.observed_serial, "observed serial")?,
        details,
        outcome,
    })
}

fn map_report(result: ReportPickShortageResult) -> V1Result<ReportPickShortageResponse> {
    Ok(ReportPickShortageResponse {
        shortage_id: result.shortage_id.get(),
        shortage_revision: revision(result.shortage_revision.get())?,
        shortage_status: map_status(result.shortage_status),
        task_id: result.task_id.get(),
        content_id: result.content_id.get(),
        order_id: result.order_id.get(),
        order_revision: revision(result.order_revision.get())?,
        quantities: map_quantities(result.quantities),
        details: map_details(result.details),
        reallocated_quantity: result.reallocated_quantity.get(),
        recovery_terminal_quantity: result.recovery_terminal_quantity.get(),
        remaining_to_allocate_quantity: result.remaining_to_allocate_quantity.get(),
        observed_item_barcode: result.observed_item_barcode.map(PickScanValue::into_inner),
        observed_lot: result.observed_lot.map(PickScanValue::into_inner),
        observed_serial: result.observed_serial.map(PickScanValue::into_inner),
        hold: PickShortageHoldResponse {
            hold_id: result.hold.hold_id.get(),
            inventory_balance_id: result.hold.inventory_balance_id.get(),
            held_quantity: result.hold.held_quantity.get(),
        },
        movement: result.movement.map(map_movement),
        reported_by: result.reported_by.get(),
        reported_at: result.reported_at.to_rfc3339(),
    })
}

fn map_reallocation(
    result: ReallocatePickShortageResult,
) -> V1Result<ReallocatePickShortageResponse> {
    Ok(ReallocatePickShortageResponse {
        reallocation_run_id: result.reallocation_run_id.get(),
        shortage_id: result.shortage_id.get(),
        shortage_revision: revision(result.shortage_revision.get())?,
        shortage_status: map_status(result.shortage_status),
        order_id: result.order_id.get(),
        order_revision: revision(result.order_revision.get())?,
        policy: map_policy(result.policy)?,
        strategy: map_strategy(result.strategy),
        outcome: map_outcome(result.outcome),
        newly_allocated_quantity: result.newly_allocated_quantity.get(),
        reallocated_quantity: result.reallocated_quantity.get(),
        recovery_terminal_quantity: result.recovery_terminal_quantity.get(),
        remaining_to_allocate_quantity: result.remaining_to_allocate_quantity.get(),
        new_allocations: result
            .new_allocations
            .into_iter()
            .map(map_allocation)
            .collect(),
        new_tasks: result.new_tasks.into_iter().map(map_task).collect(),
        executed_by: result.executed_by.get(),
        executed_at: result.executed_at.to_rfc3339(),
    })
}

fn map_short_ship_result(
    result: AcceptPickShortageAsShortShipResult,
) -> V1Result<AcceptPickShortageAsShortShipResponse> {
    Ok(AcceptPickShortageAsShortShipResponse {
        disposition_id: result.disposition_id.get(),
        shortage_id: result.shortage_id.get(),
        previous_shortage_status: map_status(result.previous_shortage_status),
        shortage_status: map_status(result.shortage_status),
        shortage_resolution: map_resolution(result.shortage_resolution),
        shortage_revision: revision(result.shortage_revision.get())?,
        order_id: result.order_id.get(),
        order_line_id: result.order_line_id.get(),
        previous_order_status: map_pick_order_status(result.previous_order_status)?,
        order_status: map_pick_order_status(result.order_status)?,
        order_revision: revision(result.order_revision.get())?,
        order_ready_to_pack: result.order_ready_to_pack,
        shortage_quantities: map_quantities(result.shortage_quantities),
        reallocated_quantity: result.reallocated_quantity.get(),
        recovery_terminal_quantity: result.recovery_terminal_quantity.get(),
        accepted_short_quantity: result.accepted_short_quantity.get(),
        line_demand: map_demand(result.line_demand),
        order_demand: map_demand(result.order_demand),
        inventory_hold_id: result.inventory_hold_id.get(),
        reason: map_short_ship_reason(result.reason),
        note: result.note.map(|note| note.as_str().to_owned()),
        resolved_by: result.resolved_by.get(),
        resolved_at: result.resolved_at.to_rfc3339(),
    })
}

const fn map_demand(demand: ShortShipDemandQuantities) -> ShortShipDemandResponse {
    ShortShipDemandResponse {
        ordered: demand.ordered().get(),
        accepted_short: demand.accepted_short().get(),
        accepted_substitute: demand.accepted_substitute().get(),
        effective: demand.effective().get(),
    }
}

fn map_pick_order_status(status: OrderStatus) -> V1Result<PickOrderStatus> {
    match status {
        OrderStatus::Processing => Ok(PickOrderStatus::Processing),
        OrderStatus::AwaitingPacking => Ok(PickOrderStatus::AwaitingPacking),
        _ => Err(V1Error::internal(
            "short-shipment disposition produced an invalid order status",
        )),
    }
}

fn map_shortage(result: PickShortageReadModel) -> V1Result<PickShortageResponse> {
    Ok(PickShortageResponse {
        shortage_id: result.shortage_id.get(),
        shortage_revision: revision(result.shortage_revision.get())?,
        status: map_status(result.status),
        resolution: result.resolution.map(map_resolution),
        inventory_owner_id: result.inventory_owner_id.get(),
        inventory_owner_name: result.inventory_owner_name,
        facility_id: result.facility_id.get(),
        facility_name: result.facility_name,
        order_id: result.order_id.get(),
        order_key: result.order_key,
        order_revision: revision(result.order_revision.get())?,
        order_line_id: result.order_line_id.get(),
        task_id: result.task_id.get(),
        content_id: result.content_id.get(),
        source_inventory_balance_id: result.source_inventory_balance_id.get(),
        source_location_id: result.source_location_id.get(),
        source_location_barcode: result.source_location_barcode.into_inner(),
        source_location_name: result.source_location_name,
        source_license_plate_id: result.source_license_plate_id.map(|id| id.get()),
        source_license_plate_barcode: result
            .source_license_plate_barcode
            .map(PickScanValue::into_inner),
        item_id: result.item_id,
        item_description: result.item_description,
        uom: result.uom,
        lot: result.lot,
        serial: result.serial,
        expiration: result.expiration.map(|value| value.to_rfc3339()),
        quantities: map_quantities(result.quantities),
        reallocated_quantity: result.reallocated_quantity.get(),
        recovery_terminal_quantity: result.recovery_terminal_quantity.get(),
        remaining_to_allocate_quantity: result.remaining_to_allocate_quantity.get(),
        accepted_short_quantity: result.accepted_short_quantity.get(),
        accepted_substitute_quantity: result.accepted_substitute_quantity.get(),
        observed_item_barcode: result.observed_item_barcode.map(PickScanValue::into_inner),
        observed_lot: result.observed_lot.map(PickScanValue::into_inner),
        observed_serial: result.observed_serial.map(PickScanValue::into_inner),
        details: map_details(result.details),
        hold: PickShortageHoldResponse {
            hold_id: result.hold.hold_id.get(),
            inventory_balance_id: result.hold.inventory_balance_id.get(),
            held_quantity: result.hold.held_quantity.get(),
        },
        reported_by: result.reported_by.get(),
        reported_at: result.reported_at.to_rfc3339(),
        resolved_at: result.resolved_at.map(|value| value.to_rfc3339()),
    })
}

fn map_movement(
    movement: wareboxes_application::picking::PickShortageMovementResult,
) -> PickShortageMovementResponse {
    PickShortageMovementResponse {
        inventory_transaction_id: movement.inventory_transaction_id,
        source_inventory_allocation_id: movement.source_inventory_allocation_id.get(),
        destination_inventory_allocation_id: movement.destination_inventory_allocation_id.get(),
        source_inventory_balance_id: movement.source_inventory_balance_id.get(),
        destination_inventory_balance_id: movement.destination_inventory_balance_id.get(),
        source_location_id: movement.source_location_id.get(),
        destination_location_id: movement.destination_location_id.get(),
        source_license_plate_id: movement.source_license_plate_id.map(|id| id.get()),
        destination_license_plate_id: movement.destination_license_plate_id.get(),
        picked_quantity: movement.picked_quantity.get(),
        destination_stage: map_execution_stage(movement.destination_stage),
    }
}

fn map_allocation(allocation: PickShortageAllocationReadModel) -> PickShortageAllocationResponse {
    PickShortageAllocationResponse {
        allocation_id: allocation.allocation_id.get(),
        inventory_balance_id: allocation.inventory_balance_id.get(),
        item_batch_id: allocation.item_batch_id.get(),
        location_id: allocation.location_id.get(),
        location_name: allocation.location_name,
        location_barcode: allocation.location_barcode.into_inner(),
        license_plate_id: allocation.license_plate_id.map(|id| id.get()),
        license_plate_barcode: allocation
            .license_plate_barcode
            .map(PickScanValue::into_inner),
        lot: allocation.lot,
        serial: allocation.serial,
        expiration: allocation.expiration.map(|value| value.to_rfc3339()),
        quantity: allocation.quantity.get(),
        execution_stage: map_execution_stage(allocation.execution_stage),
    }
}

fn map_task(task: PickShortageTaskReadModel) -> PickShortageTaskResponse {
    PickShortageTaskResponse {
        task_id: task.task_id.get(),
        content_id: task.content_id.get(),
        source_allocation_id: task.source_allocation_id.get(),
        source_inventory_balance_id: task.source_inventory_balance_id.get(),
        source_location_id: task.source_location_id.get(),
        source_location_barcode: task.source_location_barcode.into_inner(),
        source_license_plate_id: task.source_license_plate_id.map(|id| id.get()),
        source_license_plate_barcode: task
            .source_license_plate_barcode
            .map(PickScanValue::into_inner),
        planned_quantity: task.planned_quantity.get(),
    }
}

fn map_quantities(
    quantities: wareboxes_domain::PickShortageQuantities,
) -> PickShortageQuantitiesResponse {
    PickShortageQuantitiesResponse {
        planned: quantities.planned().get(),
        picked: quantities.picked().get(),
        short: quantities.short().get(),
    }
}

fn map_details(details: PickShortageDetails) -> ApiShortageDetails {
    ApiShortageDetails {
        reason: map_reason(details.reason()),
        note: details.note().map(|note| note.as_str().to_owned()),
    }
}

const fn map_reason(reason: PickShortageReason) -> ApiShortageReason {
    match reason {
        PickShortageReason::InventoryMissing => ApiShortageReason::InventoryMissing,
        PickShortageReason::InsufficientQuantity => ApiShortageReason::InsufficientQuantity,
        PickShortageReason::DamagedInventory => ApiShortageReason::DamagedInventory,
        PickShortageReason::WrongInventory => ApiShortageReason::WrongInventory,
        PickShortageReason::LotOrSerialMismatch => ApiShortageReason::LotOrSerialMismatch,
        PickShortageReason::Other => ApiShortageReason::Other,
    }
}

const fn map_reason_to_domain(reason: ApiShortageReason) -> PickShortageReason {
    match reason {
        ApiShortageReason::InventoryMissing => PickShortageReason::InventoryMissing,
        ApiShortageReason::InsufficientQuantity => PickShortageReason::InsufficientQuantity,
        ApiShortageReason::DamagedInventory => PickShortageReason::DamagedInventory,
        ApiShortageReason::WrongInventory => PickShortageReason::WrongInventory,
        ApiShortageReason::LotOrSerialMismatch => PickShortageReason::LotOrSerialMismatch,
        ApiShortageReason::Other => PickShortageReason::Other,
    }
}

const fn map_status(status: PickShortageStatus) -> ApiShortageStatus {
    match status {
        PickShortageStatus::AwaitingInventory => ApiShortageStatus::AwaitingInventory,
        PickShortageStatus::RecoveryInProgress => ApiShortageStatus::RecoveryInProgress,
        PickShortageStatus::Resolved => ApiShortageStatus::Resolved,
    }
}

const fn map_status_to_domain(status: ApiShortageStatus) -> PickShortageStatus {
    match status {
        ApiShortageStatus::AwaitingInventory => PickShortageStatus::AwaitingInventory,
        ApiShortageStatus::RecoveryInProgress => PickShortageStatus::RecoveryInProgress,
        ApiShortageStatus::Resolved => PickShortageStatus::Resolved,
    }
}

const fn map_resolution(resolution: PickShortageResolution) -> ApiShortageResolution {
    match resolution {
        PickShortageResolution::Recovered => ApiShortageResolution::Recovered,
        PickShortageResolution::ShortShip => ApiShortageResolution::ShortShip,
        PickShortageResolution::Substituted => ApiShortageResolution::Substituted,
    }
}

const fn map_short_ship_reason(reason: PickShortShipReason) -> ApiShortShipReason {
    match reason {
        PickShortShipReason::ClientAuthorized => ApiShortShipReason::ClientAuthorized,
        PickShortShipReason::InventoryUnavailable => ApiShortShipReason::InventoryUnavailable,
        PickShortShipReason::ShipByCommitment => ApiShortShipReason::ShipByCommitment,
        PickShortShipReason::Other => ApiShortShipReason::Other,
    }
}

const fn map_short_ship_reason_to_domain(reason: ApiShortShipReason) -> PickShortShipReason {
    match reason {
        ApiShortShipReason::ClientAuthorized => PickShortShipReason::ClientAuthorized,
        ApiShortShipReason::InventoryUnavailable => PickShortShipReason::InventoryUnavailable,
        ApiShortShipReason::ShipByCommitment => PickShortShipReason::ShipByCommitment,
        ApiShortShipReason::Other => PickShortShipReason::Other,
    }
}

const fn map_execution_stage(stage: AllocationExecutionStage) -> ApiExecutionStage {
    match stage {
        AllocationExecutionStage::PickSource => ApiExecutionStage::PickSource,
        AllocationExecutionStage::Staged => ApiExecutionStage::Staged,
        AllocationExecutionStage::Packed => ApiExecutionStage::Packed,
    }
}

const fn map_outcome(outcome: AllocationOutcome) -> OrderAllocationOutcome {
    match outcome {
        AllocationOutcome::FullyAllocated => OrderAllocationOutcome::FullyAllocated,
        AllocationOutcome::PartiallyAllocated => OrderAllocationOutcome::PartiallyAllocated,
        AllocationOutcome::NotAllocated => OrderAllocationOutcome::NotAllocated,
    }
}

fn scan(value: String, label: &str) -> V1Result<PickScanValue> {
    PickScanValue::new(value)
        .map_err(|error| AppError::bad_request(format!("invalid {label}: {error}")).into())
}

fn optional_scan(value: Option<String>, label: &str) -> V1Result<Option<PickScanValue>> {
    value.map(|value| scan(value, label)).transpose()
}

fn shortage_id_value(value: i64) -> V1Result<PickShortageId> {
    PickShortageId::new(value).map_err(domain_validation)
}

fn revision(value: i64) -> V1Result<Revision> {
    Revision::new(value).map_err(|_| V1Error::internal("repository produced an invalid revision"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CursorFilters {
    facility_id: Option<i64>,
    inventory_owner_id: Option<i64>,
    order_id: Option<i64>,
    order_key: Option<String>,
    status: Option<PickShortageStatus>,
    sort: PickShortageQueueSort,
    direction: PickShortageQueueSortDirection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoundCursor {
    filters: CursorFilters,
    offset: u64,
}

fn decode_cursor(cursor: &OpaqueCursor) -> V1Result<BoundCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("pick shortages"))?;
    let parts = encoded.split('.').collect::<Vec<_>>();
    if parts.len() != 8 {
        return Err(V1Error::invalid_cursor_for("pick shortages"));
    }
    Ok(BoundCursor {
        filters: CursorFilters {
            facility_id: parse_optional_id(parts[0])?,
            inventory_owner_id: parse_optional_id(parts[1])?,
            order_id: parse_optional_id(parts[2])?,
            order_key: parse_optional_text(parts[3])?,
            status: parse_status_code(parts[4])?,
            sort: parse_queue_sort_code(parts[5])?,
            direction: parse_queue_direction_code(parts[6])?,
        },
        offset: parse_offset(parts[7])?,
    })
}

fn encode_cursor(cursor: BoundCursor) -> AppResult<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{}.{}.{}.{}.{}.{}.{}.{:016x}",
        encode_optional_id(cursor.filters.facility_id),
        encode_optional_id(cursor.filters.inventory_owner_id),
        encode_optional_id(cursor.filters.order_id),
        encode_optional_text(cursor.filters.order_key.as_deref()),
        status_code(cursor.filters.status),
        queue_sort_code(cursor.filters.sort),
        queue_direction_code(cursor.filters.direction),
        cursor.offset,
    ))
    .map_err(|_| AppError::internal("generated an invalid pick shortage cursor"))
}

fn decode_bound_cursor(cursor: Option<&OpaqueCursor>, filters: CursorFilters) -> V1Result<u64> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let decoded = decode_cursor(cursor)?;
    if decoded.filters != filters {
        return Err(V1Error::invalid_cursor_for("pick shortages"));
    }
    Ok(decoded.offset)
}

fn parse_optional_id(encoded: &str) -> V1Result<Option<i64>> {
    if encoded == "a" {
        return Ok(None);
    }
    parse_id_hex(encoded).map(Some)
}

fn parse_id_hex(encoded: &str) -> V1Result<i64> {
    if encoded.len() != 16 {
        return Err(V1Error::invalid_cursor_for("pick shortages"));
    }
    i64::from_str_radix(encoded, 16)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| V1Error::invalid_cursor_for("pick shortages"))
}

fn parse_offset(encoded: &str) -> V1Result<u64> {
    if encoded.len() != 16 {
        return Err(V1Error::invalid_cursor_for("pick shortages"));
    }
    u64::from_str_radix(encoded, 16).map_err(|_| V1Error::invalid_cursor_for("pick shortages"))
}

fn encode_optional_id(value: Option<i64>) -> String {
    value.map_or_else(|| "a".to_owned(), |value| format!("{value:016x}"))
}

fn encode_optional_text(value: Option<&str>) -> String {
    value.map_or_else(|| "a".to_owned(), hex::encode)
}

fn parse_optional_text(encoded: &str) -> V1Result<Option<String>> {
    if encoded == "a" {
        return Ok(None);
    }
    if encoded.is_empty() {
        return Err(V1Error::invalid_cursor_for("pick shortages"));
    }
    let bytes = hex::decode(encoded).map_err(|_| V1Error::invalid_cursor_for("pick shortages"))?;
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| V1Error::invalid_cursor_for("pick shortages"))
}

const fn status_code(status: Option<PickShortageStatus>) -> &'static str {
    match status {
        None => "a",
        Some(PickShortageStatus::AwaitingInventory) => "w",
        Some(PickShortageStatus::RecoveryInProgress) => "p",
        Some(PickShortageStatus::Resolved) => "r",
    }
}

fn parse_status_code(value: &str) -> V1Result<Option<PickShortageStatus>> {
    match value {
        "a" => Ok(None),
        "w" => Ok(Some(PickShortageStatus::AwaitingInventory)),
        "p" => Ok(Some(PickShortageStatus::RecoveryInProgress)),
        "r" => Ok(Some(PickShortageStatus::Resolved)),
        _ => Err(V1Error::invalid_cursor_for("pick shortages")),
    }
}

const fn queue_sort_code(sort: PickShortageQueueSort) -> &'static str {
    match sort {
        PickShortageQueueSort::Reported => "r",
        PickShortageQueueSort::Order => "o",
        PickShortageQueueSort::Status => "s",
        PickShortageQueueSort::ShortQuantity => "q",
        PickShortageQueueSort::RemainingQuantity => "m",
        PickShortageQueueSort::InventoryOwner => "c",
        PickShortageQueueSort::Item => "i",
        PickShortageQueueSort::Facility => "f",
    }
}

fn parse_queue_sort_code(value: &str) -> V1Result<PickShortageQueueSort> {
    match value {
        "r" => Ok(PickShortageQueueSort::Reported),
        "o" => Ok(PickShortageQueueSort::Order),
        "s" => Ok(PickShortageQueueSort::Status),
        "q" => Ok(PickShortageQueueSort::ShortQuantity),
        "m" => Ok(PickShortageQueueSort::RemainingQuantity),
        "c" => Ok(PickShortageQueueSort::InventoryOwner),
        "i" => Ok(PickShortageQueueSort::Item),
        "f" => Ok(PickShortageQueueSort::Facility),
        _ => Err(V1Error::invalid_cursor_for("pick shortages")),
    }
}

const fn queue_direction_code(direction: PickShortageQueueSortDirection) -> &'static str {
    match direction {
        PickShortageQueueSortDirection::Ascending => "a",
        PickShortageQueueSortDirection::Descending => "d",
    }
}

fn parse_queue_direction_code(value: &str) -> V1Result<PickShortageQueueSortDirection> {
    match value {
        "a" => Ok(PickShortageQueueSortDirection::Ascending),
        "d" => Ok(PickShortageQueueSortDirection::Descending),
        _ => Err(V1Error::invalid_cursor_for("pick shortages")),
    }
}

const fn map_queue_sort(sort: PickShortageQueueSort) -> ApplicationQueueSort {
    match sort {
        PickShortageQueueSort::Reported => ApplicationQueueSort::Reported,
        PickShortageQueueSort::Order => ApplicationQueueSort::Order,
        PickShortageQueueSort::Status => ApplicationQueueSort::Status,
        PickShortageQueueSort::ShortQuantity => ApplicationQueueSort::ShortQuantity,
        PickShortageQueueSort::RemainingQuantity => ApplicationQueueSort::RemainingQuantity,
        PickShortageQueueSort::InventoryOwner => ApplicationQueueSort::InventoryOwner,
        PickShortageQueueSort::Item => ApplicationQueueSort::Item,
        PickShortageQueueSort::Facility => ApplicationQueueSort::Facility,
    }
}

const fn map_queue_direction(
    direction: PickShortageQueueSortDirection,
) -> ApplicationSortDirection {
    match direction {
        PickShortageQueueSortDirection::Ascending => ApplicationSortDirection::Ascending,
        PickShortageQueueSortDirection::Descending => ApplicationSortDirection::Descending,
    }
}

fn domain_validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortage_cursor_round_trips_filters_sort_and_offset() {
        let expected = BoundCursor {
            filters: CursorFilters {
                facility_id: Some(3),
                inventory_owner_id: Some(5),
                order_id: Some(7),
                order_key: Some("ORDER-007".to_owned()),
                status: Some(PickShortageStatus::RecoveryInProgress),
                sort: PickShortageQueueSort::RemainingQuantity,
                direction: PickShortageQueueSortDirection::Ascending,
            },
            offset: 100,
        };

        let encoded = encode_cursor(expected.clone()).unwrap();
        assert_eq!(decode_cursor(&encoded).unwrap(), expected);
    }

    #[test]
    fn shortage_cursor_rejects_other_resources_and_invalid_identities() {
        for value in [
            "sq1.a.a.a.a.a.8000000000000000.0000000000000001",
            "ps3.a.a.a.x.a.r.d.0000000000000001",
            "ps3.a.a.a.a.a.x.d.0000000000000001",
            "ps3.a.a.a.a.a.r.x.0000000000000001",
            "ps3.a.a.a.a.a.r.d.not-an-offset",
        ] {
            let cursor = OpaqueCursor::new(value).unwrap();
            assert!(decode_cursor(&cursor).is_err(), "{value}");
        }
    }

    #[test]
    fn shortage_cursor_rejects_a_different_sort() {
        let filters = CursorFilters {
            facility_id: None,
            inventory_owner_id: None,
            order_id: None,
            order_key: None,
            status: None,
            sort: PickShortageQueueSort::Reported,
            direction: PickShortageQueueSortDirection::Descending,
        };
        let cursor = encode_cursor(BoundCursor {
            filters: filters.clone(),
            offset: 100,
        })
        .unwrap();
        let changed = CursorFilters {
            sort: PickShortageQueueSort::Order,
            ..filters
        };

        assert!(decode_bound_cursor(Some(&cursor), changed).is_err());
    }

    #[test]
    fn short_ship_command_maps_revisions_reason_and_note() {
        let command = short_ship_command(
            11,
            AcceptPickShortageAsShortShipRequest {
                expected_shortage_revision: Revision::new(3).unwrap(),
                expected_order_revision: Revision::new(8).unwrap(),
                reason: ApiShortShipReason::ClientAuthorized,
                note: Some("Client approved reduced quantity".to_owned()),
            },
        )
        .unwrap();

        assert_eq!(command.shortage_id().get(), 11);
        assert_eq!(command.expected_shortage_revision().get(), 3);
        assert_eq!(command.expected_order_revision().get(), 8);
        assert_eq!(command.reason(), PickShortShipReason::ClientAuthorized);
        assert_eq!(
            command.note().map(PickShortShipNote::as_str),
            Some("Client approved reduced quantity")
        );
    }

    #[test]
    fn short_ship_command_enforces_other_note_and_positive_path_id() {
        let request = AcceptPickShortageAsShortShipRequest {
            expected_shortage_revision: Revision::new(3).unwrap(),
            expected_order_revision: Revision::new(8).unwrap(),
            reason: ApiShortShipReason::Other,
            note: None,
        };
        assert!(short_ship_command(11, request.clone()).is_err());
        assert!(short_ship_command(0, request).is_err());
    }
}
