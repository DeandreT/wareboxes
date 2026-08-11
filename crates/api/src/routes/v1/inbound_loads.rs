use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use wareboxes_api_contract::v1::{
    ArriveInboundLoadRequest, ArriveInboundLoadResponse,
    ArrivedInboundLoadStatus as ContractArrivedStatus, CloseInboundLoadRequest,
    CloseInboundLoadResponse, InboundLoadClosedStatus as ContractClosedStatus,
    InboundLoadEntryItemResponse, InboundLoadPreArrivalStatus as ContractPreviousStatus,
    InboundLoadReceivedStatus as ContractReceivedStatus,
    InboundLoadReceivingStatus as ContractReceivingStatus, PlanInboundLoadRequest,
    PlanInboundLoadResponse, PlannedInboundLoadLineResponse, PlannedInboundLoadStatus,
    StartInboundLoadUnloadingRequest, StartInboundLoadUnloadingResponse,
};
use wareboxes_application::inbound_load::{
    ArriveInboundLoadCommand, ArriveInboundLoadResult,
    ArrivedInboundLoadStatus as ApplicationArrivedStatus, CloseInboundLoadCommand,
    CloseInboundLoadResult, PlanInboundLoadCommand, PlanInboundLoadResult,
    PlannedInboundLoadStatus as ApplicationStatus, StartInboundLoadUnloadingCommand,
    StartInboundLoadUnloadingResult,
};
use wareboxes_application::ApplicationError;
use wareboxes_domain::{
    CatalogItemId, FacilityId, InboundExpectedQuantity, InboundLoadId, InboundLoadPlanLine,
    InboundLoadPreArrivalStatus, InboundLoadReference, InboundLoadScanValue, InventoryOwnerId,
    LocationId, NewInboundLoadPlan, Timestamp,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryItemQuery {
    pub search: Option<String>,
    pub limit: Option<i64>,
}

pub async fn entry_items(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(inventory_owner_id): Path<i64>,
    Query(query): Query<EntryItemQuery>,
) -> V1Result<Json<Vec<InboundLoadEntryItemResponse>>> {
    user.require_permission(&state.db, "wms").await?;
    if inventory_owner_id <= 0 {
        return Err(invalid("inventory owner ID must be positive"));
    }
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if search.is_some_and(|value| value.chars().count() > 200) {
        return Err(invalid("item search cannot exceed 200 characters"));
    }
    let items = repo::inbound_load::inbound_load_entry_items(
        &state.db,
        &user.tenant,
        inventory_owner_id,
        search,
        query.limit.unwrap_or(100).clamp(1, 100),
    )
    .await?
    .ok_or_else(|| crate::error::AppError::not_found("inventory owner"))?;
    Ok(Json(
        items
            .into_iter()
            .map(|item| InboundLoadEntryItemResponse {
                item_id: item.item_id,
                description: item.description,
                uom: item.uom,
            })
            .collect(),
    ))
}

pub async fn plan(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<PlanInboundLoadRequest>,
) -> V1Result<Json<PlanInboundLoadResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let command = plan_command(body)?;
    let result = repo::inbound_load::plan_inbound_load(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(plan_response(result)))
}

pub async fn arrive(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(load_id): Path<i64>,
    idempotency_key: IdempotencyKey,
    Json(body): Json<ArriveInboundLoadRequest>,
) -> V1Result<Json<ArriveInboundLoadResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let command = ArriveInboundLoadCommand::new(
        InboundLoadId::new(load_id).map_err(invalid)?,
        InboundLoadScanValue::new(body.load_scan).map_err(invalid)?,
        InboundLoadScanValue::new(body.receiving_location_scan).map_err(invalid)?,
        parse_timestamp(body.arrived_at.as_deref(), "arrived_at")?,
    );
    let result = repo::inbound_load::arrive_inbound_load(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(arrival_response(result)))
}

pub async fn start_unloading(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(load_id): Path<i64>,
    idempotency_key: IdempotencyKey,
    Json(body): Json<StartInboundLoadUnloadingRequest>,
) -> V1Result<Json<StartInboundLoadUnloadingResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let command = StartInboundLoadUnloadingCommand::new(
        InboundLoadId::new(load_id).map_err(invalid)?,
        InboundLoadScanValue::new(body.load_scan).map_err(invalid)?,
        InboundLoadScanValue::new(body.receiving_location_scan).map_err(invalid)?,
        body.seal_scan
            .map(InboundLoadScanValue::new)
            .transpose()
            .map_err(invalid)?,
        parse_timestamp(body.started_at.as_deref(), "started_at")?,
    );
    let result = repo::inbound_load::start_inbound_load_unloading(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(unloading_response(result)))
}

pub async fn close(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(load_id): Path<i64>,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CloseInboundLoadRequest>,
) -> V1Result<Json<CloseInboundLoadResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let command = CloseInboundLoadCommand::new(
        InboundLoadId::new(load_id).map_err(invalid)?,
        InboundLoadScanValue::new(body.load_scan).map_err(invalid)?,
        InboundLoadScanValue::new(body.receiving_location_scan).map_err(invalid)?,
        parse_timestamp(body.closed_at.as_deref(), "closed_at")?,
    );
    let result = repo::inbound_load::close_inbound_load(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(closure_response(result)))
}

pub(crate) fn plan_command(request: PlanInboundLoadRequest) -> V1Result<PlanInboundLoadCommand> {
    let lines = request
        .lines
        .into_iter()
        .map(|line| {
            InboundLoadPlanLine::new(
                CatalogItemId::new(line.item_id).map_err(invalid)?,
                InboundExpectedQuantity::new(line.expected_quantity).map_err(invalid)?,
                line.lot,
                line.serial,
                parse_timestamp(line.expiration.as_deref(), "expiration")?,
            )
            .map_err(invalid)
        })
        .collect::<V1Result<Vec<_>>>()?;
    let plan = NewInboundLoadPlan::new(
        InventoryOwnerId::new(request.inventory_owner_id).map_err(invalid)?,
        FacilityId::new(request.facility_id).map_err(invalid)?,
        LocationId::new(request.receiving_location_id).map_err(invalid)?,
        InboundLoadReference::new(request.reference).map_err(invalid)?,
        request.invoice_number,
        request.carrier,
        request.trailer_number,
        request.seal_number,
        parse_timestamp(request.expected_at.as_deref(), "expected_at")?,
        parse_timestamp(request.appointment_at.as_deref(), "appointment_at")?,
        lines,
    )
    .map_err(invalid)?;
    Ok(PlanInboundLoadCommand::new(plan))
}

fn plan_response(result: PlanInboundLoadResult) -> PlanInboundLoadResponse {
    PlanInboundLoadResponse {
        load_id: result.load_id.get(),
        execution_barcode: result.execution_barcode,
        reference: result.reference,
        status: match result.status {
            ApplicationStatus::Planned => PlannedInboundLoadStatus::Planned,
        },
        lines: result
            .lines
            .into_iter()
            .map(|line| PlannedInboundLoadLineResponse {
                load_line_id: line.load_line_id.get(),
                item_id: line.item_id,
                expected_quantity: line.expected_quantity,
            })
            .collect(),
        total_expected_quantity: result.total_expected_quantity,
        planned_by: result.planned_by.get(),
        planned_at: result.planned_at.to_rfc3339(),
    }
}

fn arrival_response(result: ArriveInboundLoadResult) -> ArriveInboundLoadResponse {
    ArriveInboundLoadResponse {
        arrival_id: result.arrival_id.get(),
        load_id: result.load_id.get(),
        previous_status: match result.previous_status {
            InboundLoadPreArrivalStatus::Planned => ContractPreviousStatus::Planned,
            InboundLoadPreArrivalStatus::Scheduled => ContractPreviousStatus::Scheduled,
        },
        status: match result.status {
            ApplicationArrivedStatus::Arrived => ContractArrivedStatus::Arrived,
        },
        receiving_location_id: result.receiving_location_id.get(),
        arrived_by: result.arrived_by.get(),
        arrived_at: result.arrived_at.to_rfc3339(),
    }
}

fn unloading_response(
    result: StartInboundLoadUnloadingResult,
) -> StartInboundLoadUnloadingResponse {
    StartInboundLoadUnloadingResponse {
        unloading_start_id: result.unloading_start_id.get(),
        load_id: result.load_id.get(),
        status: match result.status {
            wareboxes_application::inbound_load::InboundLoadReceivingStatus::Receiving => {
                ContractReceivingStatus::Receiving
            }
        },
        receiving_location_id: result.receiving_location_id.get(),
        started_by: result.started_by.get(),
        started_at: result.started_at.to_rfc3339(),
    }
}

fn closure_response(result: CloseInboundLoadResult) -> CloseInboundLoadResponse {
    CloseInboundLoadResponse {
        closure_id: result.closure_id.get(),
        load_id: result.load_id.get(),
        previous_status: match result.previous_status {
            wareboxes_application::inbound_load::InboundLoadReceivedStatus::Received => {
                ContractReceivedStatus::Received
            }
        },
        status: match result.status {
            wareboxes_application::inbound_load::InboundLoadClosedStatus::Closed => {
                ContractClosedStatus::Closed
            }
        },
        receiving_location_id: result.receiving_location_id.get(),
        closed_by: result.closed_by.get(),
        closed_at: result.closed_at.to_rfc3339(),
    }
}

fn parse_timestamp(value: Option<&str>, field: &str) -> V1Result<Option<Timestamp>> {
    value
        .map(|value| {
            value.parse::<Timestamp>().map_err(|_| {
                invalid(format!(
                    "{field} must be an RFC 3339 timestamp with an explicit offset"
                ))
            })
        })
        .transpose()
}

fn invalid(error: impl std::fmt::Display) -> V1Error {
    V1Error::from(ApplicationError::InvalidRequest(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> PlanInboundLoadRequest {
        PlanInboundLoadRequest {
            inventory_owner_id: 7,
            facility_id: 8,
            receiving_location_id: 9,
            reference: "ASN-100".into(),
            invoice_number: None,
            carrier: Some("Parcel Freight".into()),
            trailer_number: None,
            seal_number: None,
            expected_at: Some("2027-08-11T17:00:00Z".into()),
            appointment_at: None,
            lines: vec![wareboxes_api_contract::v1::PlanInboundLoadLineRequest {
                item_id: 41,
                expected_quantity: 12,
                lot: Some("LOT-A".into()),
                serial: None,
                expiration: Some("2028-08-12T00:00:00Z".into()),
            }],
        }
    }

    #[test]
    fn mapping_preserves_the_complete_plan() {
        let command = plan_command(request()).unwrap();
        assert_eq!(command.plan().reference().as_str(), "ASN-100");
        assert_eq!(command.plan().receiving_location_id().get(), 9);
        assert_eq!(command.plan().lines()[0].item_id().get(), 41);
        assert_eq!(command.plan().lines()[0].expected_quantity().get(), 12);
        assert!(command.plan().lines()[0].expiration().is_some());
    }

    #[test]
    fn mapping_rejects_empty_lines_and_malformed_timestamps() {
        let mut empty = request();
        empty.lines.clear();
        assert!(plan_command(empty).is_err());

        let mut malformed = request();
        malformed.expected_at = Some("tomorrow".into());
        assert!(plan_command(malformed).is_err());
    }

    #[test]
    fn arrival_response_preserves_transition_evidence() {
        let arrived_at = "2027-08-10T12:00:00Z".parse::<Timestamp>().unwrap();
        let response = arrival_response(ArriveInboundLoadResult {
            arrival_id: wareboxes_domain::InboundLoadArrivalId::new(31).unwrap(),
            load_id: InboundLoadId::new(12).unwrap(),
            previous_status: InboundLoadPreArrivalStatus::Scheduled,
            status: ApplicationArrivedStatus::Arrived,
            receiving_location_id: LocationId::new(9).unwrap(),
            arrived_by: wareboxes_domain::UserId::new(4).unwrap(),
            arrived_at,
        });
        assert_eq!(response.arrival_id, 31);
        assert_eq!(response.previous_status, ContractPreviousStatus::Scheduled);
        assert_eq!(response.status, ContractArrivedStatus::Arrived);
    }

    #[test]
    fn unloading_response_preserves_execution_evidence() {
        let response = unloading_response(StartInboundLoadUnloadingResult {
            unloading_start_id: wareboxes_domain::InboundLoadUnloadingStartId::new(41).unwrap(),
            load_id: InboundLoadId::new(12).unwrap(),
            status: wareboxes_application::inbound_load::InboundLoadReceivingStatus::Receiving,
            receiving_location_id: LocationId::new(9).unwrap(),
            started_by: wareboxes_domain::UserId::new(4).unwrap(),
            started_at: "2027-08-10T12:00:00Z".parse().unwrap(),
        });
        assert_eq!(response.unloading_start_id, 41);
        assert_eq!(response.status, ContractReceivingStatus::Receiving);
    }

    #[test]
    fn closure_response_preserves_execution_evidence() {
        let response = closure_response(CloseInboundLoadResult {
            closure_id: wareboxes_domain::InboundLoadClosureId::new(51).unwrap(),
            load_id: InboundLoadId::new(12).unwrap(),
            previous_status:
                wareboxes_application::inbound_load::InboundLoadReceivedStatus::Received,
            status: wareboxes_application::inbound_load::InboundLoadClosedStatus::Closed,
            receiving_location_id: LocationId::new(9).unwrap(),
            closed_by: wareboxes_domain::UserId::new(4).unwrap(),
            closed_at: "2027-08-10T12:00:00Z".parse().unwrap(),
        });
        assert_eq!(response.closure_id, 51);
        assert_eq!(response.previous_status, ContractReceivedStatus::Received);
        assert_eq!(response.status, ContractClosedStatus::Closed);
    }
}
