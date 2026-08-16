use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    AllocationPolicyReference as ApiPolicyReference, AllocationPolicyResponse,
    AllocationPolicySource as ApiPolicySource, BackorderPolicyMode, BackorderPolicyResponse,
    ConfigurationScope as ApiConfigurationScope, OrderAllocationDetailResponse,
    OrderAllocationFacilityResponse, OrderAllocationLineResponse, OrderAllocationOutcome,
    OrderAllocationReadinessBlocker, OrderAllocationReadinessRequest,
    OrderAllocationReadinessResponse, OrderAllocationReadinessStatus,
    OrderAllocationShortageReason, OrderAllocationStrategy, PlanOrderAllocationRequest,
    PlanOrderAllocationResponse, Revision,
};
use wareboxes_application::order_allocation::{
    AllocationPolicyExpectation, AllocationPolicyReadModel,
    AllocationPolicySource as AppPolicySource, OrderAllocationDetail, OrderAllocationLineState,
    OrderAllocationReadinessBlocker as AppBlocker, OrderAllocationReadinessReadModel,
    OrderAllocationReadinessStatus as AppReadinessStatus, PlanOrderAllocationCommand,
    PlanOrderAllocationResult,
};
use wareboxes_domain::{
    AllocationOutcome, AllocationShortageReason, AllocationStrategy, ConfigurationScope,
    ConfigurationVersionId, FacilityId, OrderId, OrderRevision,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "orders";

pub async fn plan(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(order_id): Path<i64>,
    Json(body): Json<PlanOrderAllocationRequest>,
) -> V1Result<Json<PlanOrderAllocationResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = plan_command(order_id, body)?;
    let context = user.command_context(&idempotency_key);
    let result =
        repo::order_allocation::plan_order_allocation(&state.db, &user.tenant, &context, &command)
            .await?;

    Ok(Json(map_plan_result(result)?))
}

pub async fn readiness(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(order_id): Path<i64>,
    Query(query): Query<OrderAllocationReadinessRequest>,
) -> V1Result<Json<OrderAllocationReadinessResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let order_id = OrderId::new(order_id).map_err(domain_validation)?;
    let facility_id = FacilityId::new(query.facility_id).map_err(domain_validation)?;
    let readiness = repo::order_allocation::order_allocation_readiness(
        &state.db,
        &user.tenant,
        order_id,
        facility_id,
    )
    .await?;

    Ok(Json(map_readiness(readiness)?))
}

pub(super) fn plan_command(
    order_id: i64,
    request: PlanOrderAllocationRequest,
) -> V1Result<PlanOrderAllocationCommand> {
    Ok(PlanOrderAllocationCommand {
        order_id: OrderId::new(order_id).map_err(domain_validation)?,
        facility_id: FacilityId::new(request.facility_id).map_err(domain_validation)?,
        expected_revision: OrderRevision::new(request.expected_revision.get())
            .map_err(domain_validation)?,
        expected_policy: map_policy_expectation(request.expected_policy)?,
    })
}

pub(super) fn map_plan_result(
    result: PlanOrderAllocationResult,
) -> V1Result<PlanOrderAllocationResponse> {
    if !result.quantities_are_consistent() {
        return Err(V1Error::internal(
            "order allocation produced inconsistent quantities",
        ));
    }

    Ok(PlanOrderAllocationResponse {
        allocation_run_id: result.allocation_run_id.get(),
        order_id: result.order_id.get(),
        inventory_owner_id: result.inventory_owner_id.get(),
        facility_id: result.facility_id.get(),
        policy: map_policy(result.policy)?,
        strategy: map_strategy(result.strategy),
        outcome: map_outcome(result.outcome),
        revision: map_revision(result.revision)?,
        newly_allocated_quantity: result.newly_allocated_quantity,
        original_demand_quantity: result.original_demand_quantity,
        backordered_quantity: result.backordered_quantity,
        demand_quantity: result.demand_quantity,
        allocated_quantity: result.allocated_quantity,
        shortage_quantity: result.shortage_quantity,
        lines: result.lines.into_iter().map(map_line).collect(),
    })
}

fn map_readiness(
    readiness: OrderAllocationReadinessReadModel,
) -> V1Result<OrderAllocationReadinessResponse> {
    if !readiness.quantities_are_consistent() {
        return Err(V1Error::internal(
            "order allocation readiness produced inconsistent quantities",
        ));
    }

    Ok(OrderAllocationReadinessResponse {
        order_id: readiness.order_id.get(),
        inventory_owner_id: readiness.inventory_owner_id.get(),
        order_key: readiness.order_key,
        facility_id: readiness.facility_id.get(),
        eligible_facilities: readiness
            .eligible_facilities
            .into_iter()
            .map(|facility| OrderAllocationFacilityResponse {
                facility_id: facility.facility_id.get(),
                facility_name: facility.facility_name,
            })
            .collect(),
        backorder_policy: readiness
            .backorder_policy
            .map(|policy| {
                Ok::<BackorderPolicyResponse, V1Error>(BackorderPolicyResponse {
                    policy_id: policy.policy_id.get(),
                    inventory_owner_id: policy.inventory_owner_id.get(),
                    facility_id: policy.facility_id.get(),
                    mode: match policy.mode {
                        wareboxes_domain::BackorderPolicyMode::Block => BackorderPolicyMode::Block,
                        wareboxes_domain::BackorderPolicyMode::SplitShortage => {
                            BackorderPolicyMode::SplitShortage
                        }
                    },
                    revision: Revision::new(policy.revision.get())
                        .map_err(|_| V1Error::internal("backorder policy revision is invalid"))?,
                    configured_by: policy.configured_by.get(),
                    configured_at: policy.configured_at.to_rfc3339(),
                })
            })
            .transpose()?,
        revision: map_revision(readiness.revision)?,
        status: map_readiness_status(readiness.status),
        blocking_reasons: readiness
            .blocking_reasons
            .into_iter()
            .map(map_readiness_blocker)
            .collect(),
        policy: map_policy(readiness.policy)?,
        strategy: map_strategy(readiness.strategy),
        outcome: map_outcome(readiness.outcome),
        original_demand_quantity: readiness.original_demand_quantity,
        backordered_quantity: readiness.backordered_quantity,
        demand_quantity: readiness.demand_quantity,
        reserved_quantity: readiness.reserved_quantity,
        allocated_quantity: readiness.allocated_quantity,
        shortage_quantity: readiness.shortage_quantity,
        lines: readiness.lines.into_iter().map(map_line).collect(),
    })
}

fn map_line(line: OrderAllocationLineState) -> OrderAllocationLineResponse {
    OrderAllocationLineResponse {
        order_line_id: line.order_line_id.get(),
        line_key: line.line_key,
        item_id: line.item_id,
        item_description: line.item_description,
        uom: line.uom,
        original_demand_quantity: line.original_demand_quantity,
        backordered_quantity: line.backordered_quantity,
        demand_quantity: line.demand_quantity.get(),
        reservation_id: line.reservation_id.map(|id| id.get()),
        reserved_quantity: line.reserved_quantity,
        allocated_quantity: line.allocated_quantity,
        shortage_quantity: line.shortage_quantity,
        shortage_reason: line.shortage_reason.map(map_shortage_reason),
        allocations: line.allocations.into_iter().map(map_allocation).collect(),
    }
}

fn map_allocation(allocation: OrderAllocationDetail) -> OrderAllocationDetailResponse {
    OrderAllocationDetailResponse {
        allocation_id: allocation.allocation_id.get(),
        reservation_id: allocation.reservation_id.get(),
        inventory_balance_id: allocation.inventory_balance_id.get(),
        item_batch_id: allocation.item_batch_id.get(),
        location_id: allocation.location_id.get(),
        location_name: allocation.location_name,
        location_barcode: allocation.location_barcode,
        license_plate_id: allocation.license_plate_id.map(|id| id.get()),
        license_plate_barcode: allocation.license_plate_barcode,
        lot: allocation.lot,
        serial: allocation.serial,
        expiration: allocation
            .expiration
            .map(|timestamp| timestamp.to_rfc3339()),
        quantity: allocation.quantity.get(),
    }
}

pub(super) const fn map_strategy(strategy: AllocationStrategy) -> OrderAllocationStrategy {
    match strategy {
        AllocationStrategy::Fifo => OrderAllocationStrategy::Fifo,
        AllocationStrategy::Fefo => OrderAllocationStrategy::Fefo,
    }
}

fn map_policy_expectation(policy: ApiPolicyReference) -> V1Result<AllocationPolicyExpectation> {
    policy.validate().map_err(domain_validation)?;
    Ok(AllocationPolicyExpectation {
        source: match policy.source {
            ApiPolicySource::ProductDefault => AppPolicySource::ProductDefault,
            ApiPolicySource::Configuration => AppPolicySource::Configuration,
        },
        configuration_id: policy
            .configuration_id
            .map(ConfigurationVersionId::new)
            .transpose()
            .map_err(domain_validation)?,
        configuration_revision: policy.configuration_revision.map(|revision| revision.get()),
        policy_hash: policy.policy_hash,
    })
}

pub(super) fn map_policy(policy: AllocationPolicyReadModel) -> V1Result<AllocationPolicyResponse> {
    Ok(AllocationPolicyResponse {
        source: match policy.source {
            AppPolicySource::ProductDefault => ApiPolicySource::ProductDefault,
            AppPolicySource::Configuration => ApiPolicySource::Configuration,
        },
        configuration_id: policy.configuration_id.map(|id| id.get()),
        configuration_revision: policy
            .configuration_revision
            .map(Revision::new)
            .transpose()
            .map_err(|_| V1Error::internal("allocation policy revision is invalid"))?,
        configuration_scope: policy.configuration_scope.map(|scope| match scope {
            ConfigurationScope::Tenant => ApiConfigurationScope::Tenant,
            ConfigurationScope::InventoryOwner { inventory_owner_id } => {
                ApiConfigurationScope::InventoryOwner {
                    inventory_owner_id: inventory_owner_id.get(),
                }
            }
            ConfigurationScope::Facility { facility_id } => ApiConfigurationScope::Facility {
                facility_id: facility_id.get(),
            },
            ConfigurationScope::OwnerFacility {
                inventory_owner_id,
                facility_id,
            } => ApiConfigurationScope::OwnerFacility {
                inventory_owner_id: inventory_owner_id.get(),
                facility_id: facility_id.get(),
            },
        }),
        strategy: map_strategy(policy.strategy),
        allow_partial: policy.allow_partial,
        require_complete_line: policy.require_complete_line,
        policy_hash: policy.policy_hash,
    })
}

const fn map_outcome(outcome: AllocationOutcome) -> OrderAllocationOutcome {
    match outcome {
        AllocationOutcome::FullyAllocated => OrderAllocationOutcome::FullyAllocated,
        AllocationOutcome::PartiallyAllocated => OrderAllocationOutcome::PartiallyAllocated,
        AllocationOutcome::NotAllocated => OrderAllocationOutcome::NotAllocated,
    }
}

const fn map_shortage_reason(reason: AllocationShortageReason) -> OrderAllocationShortageReason {
    match reason {
        AllocationShortageReason::NoEligibleInventory => {
            OrderAllocationShortageReason::NoEligibleInventory
        }
        AllocationShortageReason::InsufficientEligibleInventory => {
            OrderAllocationShortageReason::InsufficientEligibleInventory
        }
    }
}

const fn map_readiness_status(status: AppReadinessStatus) -> OrderAllocationReadinessStatus {
    match status {
        AppReadinessStatus::Ready => OrderAllocationReadinessStatus::Ready,
        AppReadinessStatus::AlreadyFullyAllocated => {
            OrderAllocationReadinessStatus::AlreadyFullyAllocated
        }
        AppReadinessStatus::Blocked => OrderAllocationReadinessStatus::Blocked,
    }
}

const fn map_readiness_blocker(blocker: AppBlocker) -> OrderAllocationReadinessBlocker {
    match blocker {
        AppBlocker::ActiveHold => OrderAllocationReadinessBlocker::ActiveHold,
        AppBlocker::CrossDockInProgress => OrderAllocationReadinessBlocker::CrossDockInProgress,
        AppBlocker::OrderStatusNotAllocatable => {
            OrderAllocationReadinessBlocker::OrderStatusNotAllocatable
        }
        AppBlocker::OwnerFacilityUnavailable => {
            OrderAllocationReadinessBlocker::FacilityNotEligible
        }
    }
}

fn map_revision(revision: OrderRevision) -> V1Result<Revision> {
    Revision::new(revision.get())
        .map_err(|_| V1Error::internal("order allocation produced an invalid revision"))
}

fn domain_validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

#[cfg(test)]
mod tests {
    use wareboxes_application::order_allocation::{
        AllocationPolicyReadModel, OrderAllocationFacilityReadModel,
        OrderAllocationReadinessReadModel,
    };
    use wareboxes_domain::{
        AllocationQuantity, AllocationRunId, InventoryAllocationId, InventoryBalanceId,
        InventoryOwnerId, InventoryReservationId, ItemBatchId, LicensePlateId, LocationId,
        OrderLineId,
    };

    use super::*;

    fn line_state() -> OrderAllocationLineState {
        OrderAllocationLineState {
            order_line_id: OrderLineId::new(12).unwrap(),
            line_key: "1".into(),
            item_id: 41,
            item_description: Some("Case-picked item".into()),
            uom: "case".into(),
            original_demand_quantity: 8,
            backordered_quantity: 0,
            demand_quantity: AllocationQuantity::new(8).unwrap(),
            reservation_id: Some(InventoryReservationId::new(22).unwrap()),
            reserved_quantity: 8,
            allocated_quantity: 5,
            shortage_quantity: 3,
            shortage_reason: Some(AllocationShortageReason::InsufficientEligibleInventory),
            allocations: vec![OrderAllocationDetail {
                allocation_id: InventoryAllocationId::new(31).unwrap(),
                reservation_id: InventoryReservationId::new(22).unwrap(),
                inventory_balance_id: InventoryBalanceId::new(42).unwrap(),
                item_batch_id: ItemBatchId::new(52).unwrap(),
                location_id: LocationId::new(62).unwrap(),
                location_name: Some("Forward pick A-01".into()),
                location_barcode: Some("A-01".into()),
                license_plate_id: Some(LicensePlateId::new(72).unwrap()),
                license_plate_barcode: Some("LP-00072".into()),
                lot: Some("LOT-7".into()),
                serial: None,
                expiration: Some("2027-08-10T00:00:00Z".parse().unwrap()),
                quantity: AllocationQuantity::new(5).unwrap(),
            }],
        }
    }

    #[test]
    fn request_mapping_rejects_nonpositive_path_and_facility_ids() {
        let request = PlanOrderAllocationRequest {
            facility_id: 8,
            expected_revision: Revision::new(3).unwrap(),
            expected_policy: ApiPolicyReference::product_default(),
        };
        let command = plan_command(7, request.clone()).unwrap();
        assert_eq!(command.order_id.get(), 7);
        assert_eq!(command.facility_id.get(), 8);
        assert_eq!(command.expected_revision.get(), 3);

        assert!(plan_command(0, request.clone()).is_err());
        assert!(plan_command(
            7,
            PlanOrderAllocationRequest {
                facility_id: 0,
                ..request
            },
        )
        .is_err());
    }

    #[test]
    fn plan_result_mapping_preserves_traceability_and_rfc3339_timestamps() {
        let response = map_plan_result(PlanOrderAllocationResult {
            allocation_run_id: AllocationRunId::new(81).unwrap(),
            order_id: OrderId::new(7).unwrap(),
            inventory_owner_id: InventoryOwnerId::new(9).unwrap(),
            facility_id: FacilityId::new(8).unwrap(),
            policy: AllocationPolicyReadModel::product_default(),
            strategy: AllocationStrategy::Fefo,
            outcome: AllocationOutcome::PartiallyAllocated,
            revision: OrderRevision::new(4).unwrap(),
            newly_allocated_quantity: 5,
            original_demand_quantity: 8,
            backordered_quantity: 0,
            demand_quantity: 8,
            allocated_quantity: 5,
            shortage_quantity: 3,
            lines: vec![line_state()],
        })
        .unwrap();

        assert_eq!(response.allocation_run_id, 81);
        assert_eq!(response.outcome, OrderAllocationOutcome::PartiallyAllocated);
        assert_eq!(
            response.lines[0].allocations[0].location_barcode.as_deref(),
            Some("A-01")
        );
        assert_eq!(
            response.lines[0].allocations[0].expiration.as_deref(),
            Some("2027-08-10T00:00:00+00:00")
        );
    }

    #[test]
    fn readiness_mapping_preserves_facility_labels_and_typed_blockers() {
        let response = map_readiness(OrderAllocationReadinessReadModel {
            order_id: OrderId::new(7).unwrap(),
            inventory_owner_id: InventoryOwnerId::new(9).unwrap(),
            order_key: "SO-1001".into(),
            facility_id: FacilityId::new(8).unwrap(),
            eligible_facilities: vec![OrderAllocationFacilityReadModel {
                facility_id: FacilityId::new(8).unwrap(),
                facility_name: "Reno DC".into(),
            }],
            backorder_policy: None,
            revision: OrderRevision::new(4).unwrap(),
            status: AppReadinessStatus::Blocked,
            blocking_reasons: vec![AppBlocker::OwnerFacilityUnavailable],
            policy: AllocationPolicyReadModel::product_default(),
            strategy: AllocationStrategy::Fefo,
            outcome: AllocationOutcome::PartiallyAllocated,
            original_demand_quantity: 8,
            backordered_quantity: 0,
            demand_quantity: 8,
            reserved_quantity: 8,
            allocated_quantity: 5,
            shortage_quantity: 3,
            lines: vec![line_state()],
        })
        .unwrap();

        assert_eq!(response.eligible_facilities[0].facility_name, "Reno DC");
        assert_eq!(
            response.blocking_reasons,
            vec![OrderAllocationReadinessBlocker::FacilityNotEligible]
        );
    }
}
