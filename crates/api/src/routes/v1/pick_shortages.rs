use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    AllocationExecutionStage as ApiExecutionStage, OpaqueCursor, OrderAllocationOutcome,
    OrderAllocationStrategy, PickShortageAllocationResponse,
    PickShortageDetails as ApiShortageDetails, PickShortageHoldResponse,
    PickShortageMovementResponse, PickShortagePage as ApiShortagePage, PickShortagePageRequest,
    PickShortageQuantitiesResponse, PickShortageReason as ApiShortageReason, PickShortageResponse,
    PickShortageStatus as ApiShortageStatus, PickShortageTaskResponse,
    ReallocatePickShortageRequest, ReallocatePickShortageResponse, ReportPickShortageOutcome,
    ReportPickShortageRequest, ReportPickShortageResponse, Revision,
};
use wareboxes_application::picking::{
    PickShortageAllocationReadModel, PickShortageCursor, PickShortagePageQuery, PickShortageQuery,
    PickShortageReadModel, PickShortageTaskReadModel, ReallocatePickShortageCommand,
    ReallocatePickShortageResult, ReportPickShortageCommand,
    ReportPickShortageOutcome as AppShortageOutcome, ReportPickShortageResult,
};
use wareboxes_domain::{
    AllocationExecutionStage, AllocationOutcome, AllocationStrategy, FacilityId, InventoryOwnerId,
    OrderId, OrderRevision, PickContentId, PickQuantity, PickScanValue, PickShortageDetails,
    PickShortageId, PickShortageNote, PickShortageReason, PickShortageRevision, PickShortageStatus,
    PickTaskId, Timestamp,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const OPERATOR_PERMISSION: &str = "wms";
const SUPERVISOR_PERMISSION: &str = "wms_supervisor";
const CURSOR_PREFIX: &str = "ps1.";

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
        strategy: map_strategy_to_domain(body.strategy),
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::picking::reallocate_shortage(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_reallocation(result)?))
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
    let cursor = query.cursor.as_ref().map(decode_cursor).transpose()?;
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
    let status = query.status.map(map_status_to_domain);
    if query.limit.get() > 100 {
        return Err(
            AppError::bad_request("pick shortage page limit must be between 1 and 100").into(),
        );
    }
    if cursor.as_ref().is_some_and(|cursor| {
        cursor.facility_id != facility_id.map(|id| id.get())
            || cursor.inventory_owner_id != inventory_owner_id.map(|id| id.get())
            || cursor.order_id != order_id.map(|id| id.get())
            || cursor.status != status
    }) {
        return Err(V1Error::invalid_cursor_for("pick shortages"));
    }
    let page = repo::picking::list_shortages(
        &state.db,
        &user.tenant,
        PickShortagePageQuery {
            facility_id,
            inventory_owner_id,
            order_id,
            status,
            cursor: cursor.map(|cursor| cursor.cursor),
            limit: query.limit.get(),
        },
    )
    .await?;
    let items = page
        .items
        .into_iter()
        .map(map_shortage)
        .collect::<V1Result<Vec<_>>>()?;
    let next_cursor = page
        .next_cursor
        .map(|cursor| {
            encode_cursor(ScopedCursor {
                facility_id: facility_id.map(|id| id.get()),
                inventory_owner_id: inventory_owner_id.map(|id| id.get()),
                order_id: order_id.map(|id| id.get()),
                status,
                cursor,
            })
        })
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

fn map_shortage(result: PickShortageReadModel) -> V1Result<PickShortageResponse> {
    Ok(PickShortageResponse {
        shortage_id: result.shortage_id.get(),
        shortage_revision: revision(result.shortage_revision.get())?,
        status: map_status(result.status),
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

const fn map_execution_stage(stage: AllocationExecutionStage) -> ApiExecutionStage {
    match stage {
        AllocationExecutionStage::PickSource => ApiExecutionStage::PickSource,
        AllocationExecutionStage::Staged => ApiExecutionStage::Staged,
        AllocationExecutionStage::Packed => ApiExecutionStage::Packed,
    }
}

const fn map_strategy_to_domain(strategy: OrderAllocationStrategy) -> AllocationStrategy {
    match strategy {
        OrderAllocationStrategy::Fefo => AllocationStrategy::Fefo,
    }
}

const fn map_strategy(strategy: AllocationStrategy) -> OrderAllocationStrategy {
    match strategy {
        AllocationStrategy::Fefo => OrderAllocationStrategy::Fefo,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScopedCursor {
    facility_id: Option<i64>,
    inventory_owner_id: Option<i64>,
    order_id: Option<i64>,
    status: Option<PickShortageStatus>,
    cursor: PickShortageCursor,
}

fn decode_cursor(cursor: &OpaqueCursor) -> V1Result<ScopedCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("pick shortages"))?;
    let parts = encoded.split('.').collect::<Vec<_>>();
    if parts.len() != 6 {
        return Err(V1Error::invalid_cursor_for("pick shortages"));
    }
    let status = match parts[3] {
        "a" => None,
        "w" => Some(PickShortageStatus::AwaitingInventory),
        "p" => Some(PickShortageStatus::RecoveryInProgress),
        "r" => Some(PickShortageStatus::Resolved),
        _ => return Err(V1Error::invalid_cursor_for("pick shortages")),
    };
    let sortable = parse_time_hex(parts[4])?;
    let micros = (sortable ^ (1_u64 << 63)) as i64;
    let reported_at = Timestamp::from_timestamp_micros(micros)
        .ok_or_else(|| V1Error::invalid_cursor_for("pick shortages"))?;
    Ok(ScopedCursor {
        facility_id: parse_optional_id(parts[0])?,
        inventory_owner_id: parse_optional_id(parts[1])?,
        order_id: parse_optional_id(parts[2])?,
        status,
        cursor: PickShortageCursor {
            reported_at,
            shortage_id: shortage_id_value(parse_id_hex(parts[5])?)?,
        },
    })
}

fn encode_cursor(cursor: ScopedCursor) -> AppResult<OpaqueCursor> {
    let status = match cursor.status {
        None => "a",
        Some(PickShortageStatus::AwaitingInventory) => "w",
        Some(PickShortageStatus::RecoveryInProgress) => "p",
        Some(PickShortageStatus::Resolved) => "r",
    };
    let sortable = (cursor.cursor.reported_at.timestamp_micros() as u64) ^ (1_u64 << 63);
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{}.{}.{}.{status}.{sortable:016x}.{:016x}",
        encode_optional_id(cursor.facility_id),
        encode_optional_id(cursor.inventory_owner_id),
        encode_optional_id(cursor.order_id),
        cursor.cursor.shortage_id.get(),
    ))
    .map_err(|_| AppError::internal("generated an invalid pick shortage cursor"))
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

fn parse_time_hex(encoded: &str) -> V1Result<u64> {
    if encoded.len() != 16 {
        return Err(V1Error::invalid_cursor_for("pick shortages"));
    }
    u64::from_str_radix(encoded, 16).map_err(|_| V1Error::invalid_cursor_for("pick shortages"))
}

fn encode_optional_id(value: Option<i64>) -> String {
    value.map_or_else(|| "a".to_owned(), |value| format!("{value:016x}"))
}

fn domain_validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortage_cursor_round_trips_filters_time_and_identity() {
        let expected = ScopedCursor {
            facility_id: Some(3),
            inventory_owner_id: Some(5),
            order_id: Some(7),
            status: Some(PickShortageStatus::RecoveryInProgress),
            cursor: PickShortageCursor {
                reported_at: "2026-08-08T21:30:00Z".parse().unwrap(),
                shortage_id: PickShortageId::new(11).unwrap(),
            },
        };

        let encoded = encode_cursor(expected).unwrap();
        assert_eq!(decode_cursor(&encoded).unwrap(), expected);
    }

    #[test]
    fn shortage_cursor_rejects_other_resources_and_invalid_identities() {
        for value in [
            "sq1.a.a.a.a.8000000000000000.0000000000000001",
            "ps1.a.a.a.x.8000000000000000.0000000000000001",
            "ps1.a.a.a.a.8000000000000000.0000000000000000",
        ] {
            let cursor = OpaqueCursor::new(value).unwrap();
            assert!(decode_cursor(&cursor).is_err(), "{value}");
        }
    }
}
