mod cancellation;

pub(super) use cancellation::cancel_work;

use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    ClaimNextReplenishmentWorkRequest, ClaimReplenishmentWorkByIdRequest,
    ConfigureReplenishmentPolicyRequest, ConfigureReplenishmentPolicyResponse,
    ConfirmReplenishmentWorkRequest, HeartbeatReplenishmentClaimRequest, OpaqueCursor,
    PlanReplenishmentRequest, PlanReplenishmentResponse, ReleaseReplenishmentClaimRequest,
    ReplenishmentClaimHeartbeatResponse, ReplenishmentClaimReleaseReason as ApiClaimReleaseReason,
    ReplenishmentClaimReleaseResponse, ReplenishmentClaimResponse,
    ReplenishmentConfirmationResponse, ReplenishmentLocationResponse,
    ReplenishmentPlannedWorkResponse, ReplenishmentPlanningOutcome as ApiPlanningOutcome,
    ReplenishmentPlanningSnapshotResponse, ReplenishmentPolicyLatestPlanResponse,
    ReplenishmentPolicyPage as ApiPolicyPage, ReplenishmentPolicyPageRequest,
    ReplenishmentPolicyReadinessEntryResponse, ReplenishmentPolicyStatus as ApiPolicyStatus,
    ReplenishmentQueueEntryResponse, ReplenishmentQueuePage as ApiWorkPage,
    ReplenishmentQueuePageRequest, ReplenishmentReserveSourceLocationIds,
    ReplenishmentWorkStatus as ApiWorkStatus, RetireReplenishmentPolicyRequest,
    RetireReplenishmentPolicyResponse, Revision,
};
use wareboxes_application::replenishment::{
    ClaimNextReplenishmentWorkCommand, ClaimReplenishmentWorkByIdCommand,
    ConfigureReplenishmentPolicyCommand, ConfigureReplenishmentPolicyResult,
    ConfirmReplenishmentWorkCommand, ConfirmReplenishmentWorkResult,
    HeartbeatReplenishmentClaimCommand, PlanReplenishmentCommand, PlanReplenishmentResult,
    ReleaseReplenishmentClaimCommand, ReplenishmentClaim, ReplenishmentClaimHeartbeatResult,
    ReplenishmentClaimReleaseResult, ReplenishmentLatestPlanReadModel,
    ReplenishmentLocationReadModel, ReplenishmentPolicyPage, ReplenishmentPolicyPageFilter,
    ReplenishmentPolicyReadinessReadModel, ReplenishmentWorkPage, ReplenishmentWorkPageFilter,
    ReplenishmentWorkReadModel, RetireReplenishmentPolicyCommand, RetireReplenishmentPolicyResult,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    CatalogItemId, FacilityId, InventoryOwnerId, LocationId, ReplenishmentClaimReleaseReason,
    ReplenishmentLevel, ReplenishmentPlanningOutcome, ReplenishmentPlanningSnapshot,
    ReplenishmentPolicyDefinition, ReplenishmentPolicyId, ReplenishmentPolicyRevision,
    ReplenishmentPolicyStatus, ReplenishmentPolicyThresholds,
    ReplenishmentReserveSourceLocationIds as DomainSourceIds, ReplenishmentScanValue,
    ReplenishmentUom, ReplenishmentWorkId, ReplenishmentWorkStatus, TenantId,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const OPERATOR_PERMISSION: &str = "wms";
const SUPERVISOR_PERMISSION: &str = "wms_supervisor";
const POLICY_CURSOR_PREFIX: &str = "rp1.";
const WORK_CURSOR_PREFIX: &str = "rw1.";
const MAX_PAGE_LIMIT: u16 = 100;
const MAX_RELEASE_NOTE_LENGTH: usize = 500;

pub async fn configure_policy(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<ConfigureReplenishmentPolicyRequest>,
) -> V1Result<Json<ConfigureReplenishmentPolicyResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    user.require_inventory_owner(body.inventory_owner_id)?;
    user.require_facility(body.facility_id)?;
    let command = configure_command(user.tenant.tenant_id, body)?;
    let context = user.command_context(&idempotency_key);
    let result =
        repo::replenishment::configure_policy(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_configured_policy(result)?))
}

pub async fn retire_policy(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(policy_id): Path<i64>,
    Json(body): Json<RetireReplenishmentPolicyRequest>,
) -> V1Result<Json<RetireReplenishmentPolicyResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    let command = RetireReplenishmentPolicyCommand {
        policy_id: policy_id_value(policy_id)?,
        expected_revision: policy_revision(body.expected_revision)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::replenishment::retire_policy(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_retired_policy(result)?))
}

pub async fn plan_policy(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(policy_id): Path<i64>,
    Json(body): Json<PlanReplenishmentRequest>,
) -> V1Result<Json<PlanReplenishmentResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    let command = PlanReplenishmentCommand {
        policy_id: policy_id_value(policy_id)?,
        expected_policy_revision: policy_revision(body.expected_policy_revision)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::replenishment::plan_policy(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_plan(result)?))
}

pub async fn policy_page(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<ReplenishmentPolicyPageRequest>,
) -> V1Result<Json<ApiPolicyPage>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    require_page_limit(query.limit.get())?;
    let decoded = query
        .cursor
        .as_ref()
        .map(decode_policy_cursor)
        .transpose()?;
    let facility_id = query
        .facility_id
        .map(|id| user.require_facility(id))
        .transpose()?;
    let inventory_owner_id = query
        .inventory_owner_id
        .map(|id| user.require_inventory_owner(id))
        .transpose()?;
    let item_id = query.item_id.map(item_id_value).transpose()?;
    let pick_face_location_id = query
        .pick_face_location_id
        .map(location_id_value)
        .transpose()?;
    let filters = PolicyCursorFilters {
        facility_id,
        inventory_owner_id,
        item_id,
        pick_face_location_id,
    };
    if decoded
        .as_ref()
        .is_some_and(|cursor| cursor.filters != filters)
    {
        return Err(V1Error::invalid_cursor_for("replenishment policies"));
    }
    let page = repo::replenishment::policy_page(
        &state.db,
        &user.tenant,
        ReplenishmentPolicyPageFilter {
            facility_id,
            inventory_owner_id,
            item_id,
            pick_face_location_id,
            after_policy_id: decoded.map(|cursor| cursor.after_policy_id),
            limit: query.limit.get(),
        },
    )
    .await?;
    Ok(Json(map_policy_page(page, filters)?))
}

pub async fn work_page(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<ReplenishmentQueuePageRequest>,
) -> V1Result<Json<ApiWorkPage>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    require_page_limit(query.limit.get())?;
    let decoded = query.cursor.as_ref().map(decode_work_cursor).transpose()?;
    let facility_id = query
        .facility_id
        .map(|id| user.require_facility(id))
        .transpose()?;
    let inventory_owner_id = query
        .inventory_owner_id
        .map(|id| user.require_inventory_owner(id))
        .transpose()?;
    let item_id = query.item_id.map(item_id_value).transpose()?;
    let pick_face_location_id = query
        .pick_face_location_id
        .map(location_id_value)
        .transpose()?;
    let status = query.status.map(map_work_status_to_domain);
    let filters = WorkCursorFilters {
        facility_id,
        inventory_owner_id,
        item_id,
        pick_face_location_id,
        status,
    };
    if decoded
        .as_ref()
        .is_some_and(|cursor| cursor.filters != filters)
    {
        return Err(V1Error::invalid_cursor_for("replenishment queue"));
    }
    let page = repo::replenishment::work_page(
        &state.db,
        &user.tenant,
        ReplenishmentWorkPageFilter {
            facility_id,
            inventory_owner_id,
            item_id,
            pick_face_location_id,
            status,
            after_work_id: decoded.map(|cursor| cursor.after_work_id),
            limit: query.limit.get(),
        },
    )
    .await?;
    Ok(Json(map_work_page(page, filters)?))
}

#[cfg_attr(not(feature = "ssr"), allow(dead_code))]
pub(crate) async fn pages_for_access(
    state: &AppState,
    access: &TenantAccess,
    limit: u16,
) -> AppResult<(ApiPolicyPage, ApiWorkPage)> {
    let policy_filters = PolicyCursorFilters {
        facility_id: None,
        inventory_owner_id: None,
        item_id: None,
        pick_face_location_id: None,
    };
    let work_filters = WorkCursorFilters {
        facility_id: None,
        inventory_owner_id: None,
        item_id: None,
        pick_face_location_id: None,
        status: None,
    };
    let (policies, work) = tokio::try_join!(
        repo::replenishment::policy_page(
            &state.db,
            access,
            ReplenishmentPolicyPageFilter {
                facility_id: None,
                inventory_owner_id: None,
                item_id: None,
                pick_face_location_id: None,
                after_policy_id: None,
                limit,
            },
        ),
        repo::replenishment::work_page(
            &state.db,
            access,
            ReplenishmentWorkPageFilter {
                facility_id: None,
                inventory_owner_id: None,
                item_id: None,
                pick_face_location_id: None,
                status: None,
                after_work_id: None,
                limit,
            },
        ),
    )?;
    let policies = map_policy_page(policies, policy_filters)
        .map_err(|_| AppError::internal("could not map replenishment policy bootstrap"))?;
    let work = map_work_page(work, work_filters)
        .map_err(|_| AppError::internal("could not map replenishment work bootstrap"))?;
    Ok((policies, work))
}

pub async fn claim_next(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(_body): Json<ClaimNextReplenishmentWorkRequest>,
) -> V1Result<Json<Option<ReplenishmentClaimResponse>>> {
    user.require_permission(&state.db, OPERATOR_PERMISSION)
        .await?;
    let context = user.command_context(&idempotency_key);
    let claim = repo::replenishment::claim_next(
        &state.db,
        &user.tenant,
        &context,
        ClaimNextReplenishmentWorkCommand,
    )
    .await?;
    Ok(Json(claim.map(map_claim).transpose()?))
}

pub async fn claim_by_id(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(work_id): Path<i64>,
    Json(_body): Json<ClaimReplenishmentWorkByIdRequest>,
) -> V1Result<Json<ReplenishmentClaimResponse>> {
    user.require_permission(&state.db, OPERATOR_PERMISSION)
        .await?;
    let context = user.command_context(&idempotency_key);
    let claim = repo::replenishment::claim_by_id(
        &state.db,
        &user.tenant,
        &context,
        ClaimReplenishmentWorkByIdCommand {
            work_id: work_id_value(work_id)?,
        },
    )
    .await?;
    Ok(Json(map_claim(claim)?))
}

pub async fn current_claim(
    State(state): State<AppState>,
    user: CurrentTenant,
) -> V1Result<Json<Option<ReplenishmentClaimResponse>>> {
    user.require_permission(&state.db, OPERATOR_PERMISSION)
        .await?;
    let claim = repo::replenishment::current_claim(&state.db, &user.tenant).await?;
    Ok(Json(claim.map(map_claim).transpose()?))
}

pub async fn heartbeat_claim(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(work_id): Path<i64>,
    Json(_body): Json<HeartbeatReplenishmentClaimRequest>,
) -> V1Result<Json<ReplenishmentClaimHeartbeatResponse>> {
    user.require_permission(&state.db, OPERATOR_PERMISSION)
        .await?;
    let context = user.command_context(&idempotency_key);
    let result = repo::replenishment::heartbeat_claim(
        &state.db,
        &user.tenant,
        &context,
        HeartbeatReplenishmentClaimCommand {
            work_id: work_id_value(work_id)?,
        },
    )
    .await?;
    Ok(Json(map_heartbeat(result)))
}

pub async fn release_claim(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(work_id): Path<i64>,
    Json(body): Json<ReleaseReplenishmentClaimRequest>,
) -> V1Result<Json<ReplenishmentClaimReleaseResponse>> {
    user.require_permission(&state.db, OPERATOR_PERMISSION)
        .await?;
    validate_release(&body)?;
    let command = ReleaseReplenishmentClaimCommand {
        work_id: work_id_value(work_id)?,
        reason: map_release_reason_to_domain(body.reason),
        note: body.note,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::replenishment::release_claim(&state.db, &user.tenant, &context, command).await?;
    Ok(Json(map_release(result)?))
}

pub async fn confirm_work(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(work_id): Path<i64>,
    Json(body): Json<ConfirmReplenishmentWorkRequest>,
) -> V1Result<Json<ReplenishmentConfirmationResponse>> {
    user.require_permission(&state.db, OPERATOR_PERMISSION)
        .await?;
    let command = ConfirmReplenishmentWorkCommand {
        work_id: work_id_value(work_id)?,
        source_location_barcode: scan(body.source_location_barcode, "source location barcode")?,
        item_barcode: scan(body.item_barcode, "item barcode")?,
        lot_scan: optional_scan(body.lot_scan, "lot")?,
        serial_scan: optional_scan(body.serial_scan, "serial")?,
        destination_pick_face_barcode: scan(
            body.destination_pick_face_barcode,
            "destination pick-face barcode",
        )?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::replenishment::confirm_work(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_confirmation(result)?))
}

fn configure_command(
    tenant_id: TenantId,
    body: ConfigureReplenishmentPolicyRequest,
) -> V1Result<ConfigureReplenishmentPolicyCommand> {
    let scope = wareboxes_domain::ReplenishmentPolicyScope {
        tenant_id,
        inventory_owner_id: InventoryOwnerId::new(body.inventory_owner_id)
            .map_err(domain_validation)?,
        facility_id: FacilityId::new(body.facility_id).map_err(domain_validation)?,
        item_id: item_id_value(body.item_id)?,
        uom: ReplenishmentUom::new(body.uom).map_err(domain_validation)?,
        pick_face_location_id: location_id_value(body.pick_face_location_id)?,
    };
    let thresholds = ReplenishmentPolicyThresholds::new(
        ReplenishmentLevel::new(body.minimum_quantity).map_err(domain_validation)?,
        ReplenishmentLevel::new(body.target_quantity).map_err(domain_validation)?,
    )
    .map_err(domain_validation)?;
    let reserve_source_location_ids = DomainSourceIds::new(
        body.reserve_source_location_ids
            .into_inner()
            .into_iter()
            .map(location_id_value)
            .collect::<V1Result<Vec<_>>>()?,
    )
    .map_err(domain_validation)?;
    let definition =
        ReplenishmentPolicyDefinition::new(scope, thresholds, reserve_source_location_ids)
            .map_err(domain_validation)?;

    Ok(ConfigureReplenishmentPolicyCommand {
        definition,
        expected_revision: body.expected_revision.map(policy_revision).transpose()?,
    })
}

fn map_configured_policy(
    result: ConfigureReplenishmentPolicyResult,
) -> V1Result<ConfigureReplenishmentPolicyResponse> {
    let scope = result.definition.scope();
    let thresholds = result.definition.thresholds();
    Ok(ConfigureReplenishmentPolicyResponse {
        policy_id: result.policy_id.get(),
        inventory_owner_id: scope.inventory_owner_id.get(),
        facility_id: scope.facility_id.get(),
        item_id: scope.item_id.get(),
        uom: scope.uom.as_str().to_owned(),
        pick_face_location_id: scope.pick_face_location_id.get(),
        minimum_quantity: thresholds.minimum().get(),
        target_quantity: thresholds.target().get(),
        reserve_source_location_ids: map_source_ids(
            result.definition.reserve_source_location_ids(),
        )?,
        status: map_policy_status(result.status),
        previous_revision: result
            .previous_revision
            .map(|value| revision(value.get()))
            .transpose()?,
        revision: revision(result.revision.get())?,
        configured_by: result.configured_by.get(),
        configured_at: result.configured_at.to_rfc3339(),
    })
}

fn map_retired_policy(
    result: RetireReplenishmentPolicyResult,
) -> V1Result<RetireReplenishmentPolicyResponse> {
    Ok(RetireReplenishmentPolicyResponse {
        policy_id: result.policy_id.get(),
        inventory_owner_id: result.scope.inventory_owner_id.get(),
        facility_id: result.scope.facility_id.get(),
        item_id: result.scope.item_id.get(),
        uom: result.scope.uom.as_str().to_owned(),
        pick_face_location_id: result.scope.pick_face_location_id.get(),
        revision: revision(result.revision.get())?,
        status: map_policy_status(result.status),
        retired_by: result.retired_by.get(),
        retired_at: result.retired_at.to_rfc3339(),
    })
}

fn map_plan(result: PlanReplenishmentResult) -> V1Result<PlanReplenishmentResponse> {
    if !result.quantities_and_sequence_are_consistent() {
        return Err(V1Error::internal(
            "repository produced an inconsistent replenishment plan",
        ));
    }
    Ok(PlanReplenishmentResponse {
        plan_id: result.plan_id.get(),
        policy_id: result.policy_id.get(),
        policy_revision: revision(result.policy_revision.get())?,
        inventory_owner_id: result.scope.inventory_owner_id.get(),
        facility_id: result.scope.facility_id.get(),
        item_id: result.scope.item_id.get(),
        uom: result.scope.uom.as_str().to_owned(),
        pick_face_location_id: result.scope.pick_face_location_id.get(),
        snapshot: map_snapshot(result.snapshot),
        required_level: result.required_level.get(),
        target_gap: result.target_gap.get(),
        planned_quantity: result.planned.get(),
        remaining_quantity: result.remaining.get(),
        outcome: map_planning_outcome(result.outcome),
        work: result
            .work
            .into_iter()
            .map(|work| ReplenishmentPlannedWorkResponse {
                work_id: work.work_id.get(),
                sequence: work.sequence,
                source_inventory_balance_id: work.source_inventory_balance_id.get(),
                item_batch_id: work.item_batch_id.get(),
                source_location_id: work.source_location_id.get(),
                source_location_barcode: work.source_location_barcode.as_str().to_owned(),
                source_location_name: work.source_location_name,
                lot: work.lot,
                serial: work.serial,
                expiration: work.expiration.map(|value| value.to_rfc3339()),
                source_received_at: work.source_received_at.to_rfc3339(),
                quantity: work.quantity.get(),
            })
            .collect(),
        planned_by: result.planned_by.get(),
        planned_at: result.planned_at.to_rfc3339(),
    })
}

fn map_policy_page(
    page: ReplenishmentPolicyPage,
    filters: PolicyCursorFilters,
) -> V1Result<ApiPolicyPage> {
    let items = page
        .items
        .into_iter()
        .map(map_policy_readiness)
        .collect::<V1Result<Vec<_>>>()?;
    let next_cursor = page
        .next_after_policy_id
        .map(|after_policy_id| {
            encode_policy_cursor(PolicyCursor {
                filters,
                after_policy_id,
            })
        })
        .transpose()?;
    Ok(ApiPolicyPage::new(items, next_cursor))
}

fn map_policy_readiness(
    entry: ReplenishmentPolicyReadinessReadModel,
) -> V1Result<ReplenishmentPolicyReadinessEntryResponse> {
    if !entry.quantities_are_consistent() {
        return Err(V1Error::internal(
            "repository produced inconsistent replenishment readiness",
        ));
    }
    let scope = entry.definition.scope();
    let thresholds = entry.definition.thresholds();
    Ok(ReplenishmentPolicyReadinessEntryResponse {
        policy_id: entry.policy_id.get(),
        revision: revision(entry.revision.get())?,
        status: ApiPolicyStatus::Active,
        inventory_owner_id: scope.inventory_owner_id.get(),
        inventory_owner_name: entry.inventory_owner_name,
        facility_id: scope.facility_id.get(),
        facility_name: entry.facility_name,
        item_id: scope.item_id.get(),
        item_description: entry.item_description,
        primary_sku: entry.primary_sku,
        uom: scope.uom.as_str().to_owned(),
        pick_face: map_location(entry.pick_face),
        minimum_quantity: thresholds.minimum().get(),
        target_quantity: thresholds.target().get(),
        reserve_source_location_ids: map_source_ids(
            entry.definition.reserve_source_location_ids(),
        )?,
        snapshot: map_snapshot(entry.snapshot),
        required_level: entry.required_level.get(),
        target_gap: entry.target_gap.get(),
        suggested_outcome: map_planning_outcome(entry.suggested_outcome),
        suggested_quantity: entry.suggested_quantity.get(),
        suggested_remaining_quantity: entry.suggested_remaining.get(),
        active_work_count: entry.active_work_count,
        active_work_quantity: entry.active_work_quantity.get(),
        latest_plan: entry.latest_plan.map(map_latest_plan),
    })
}

fn map_latest_plan(
    plan: ReplenishmentLatestPlanReadModel,
) -> ReplenishmentPolicyLatestPlanResponse {
    ReplenishmentPolicyLatestPlanResponse {
        plan_id: plan.plan_id.get(),
        outcome: map_planning_outcome(plan.outcome),
        planned_quantity: plan.planned.get(),
        remaining_quantity: plan.remaining.get(),
        planned_by: plan.planned_by.get(),
        planned_at: plan.planned_at.to_rfc3339(),
    }
}

fn map_work_page(page: ReplenishmentWorkPage, filters: WorkCursorFilters) -> V1Result<ApiWorkPage> {
    let items = page
        .items
        .into_iter()
        .map(map_work)
        .collect::<V1Result<Vec<_>>>()?;
    let next_cursor = page
        .next_after_work_id
        .map(|after_work_id| {
            encode_work_cursor(WorkCursor {
                filters,
                after_work_id,
            })
        })
        .transpose()?;
    Ok(ApiWorkPage::new(items, next_cursor))
}

fn map_work(work: ReplenishmentWorkReadModel) -> V1Result<ReplenishmentQueueEntryResponse> {
    Ok(ReplenishmentQueueEntryResponse {
        work_id: work.work_id.get(),
        plan_id: work.plan_id.get(),
        policy_id: work.policy_id.get(),
        policy_revision: revision(work.policy_revision.get())?,
        status: map_work_status(work.status),
        inventory_owner_id: work.inventory_owner_id.get(),
        inventory_owner_name: work.inventory_owner_name,
        facility_id: work.facility_id.get(),
        facility_name: work.facility_name,
        sequence: work.sequence,
        priority: work.priority,
        item_id: work.item_id.get(),
        item_description: work.item_description,
        primary_sku: work.primary_sku,
        uom: work.uom.as_str().to_owned(),
        lot: work.lot,
        serial: work.serial,
        expiration: work.expiration.map(|value| value.to_rfc3339()),
        quantity: work.quantity.get(),
        source_inventory_balance_id: work.source_inventory_balance_id.get(),
        item_batch_id: work.item_batch_id.get(),
        source_location: map_location(work.source_location),
        destination_pick_face: map_location(work.destination_pick_face),
        claimed_by: work.claimed_by.map(|value| value.get()),
        lease_expires_at: work.lease_expires_at.map(|value| value.to_rfc3339()),
        due_at: work.due_at.map(|value| value.to_rfc3339()),
        created_at: work.created_at.to_rfc3339(),
        completed_at: work.completed_at.map(|value| value.to_rfc3339()),
    })
}

fn map_claim(claim: ReplenishmentClaim) -> V1Result<ReplenishmentClaimResponse> {
    Ok(ReplenishmentClaimResponse {
        work_id: claim.work_id.get(),
        plan_id: claim.plan_id.get(),
        policy_id: claim.policy_id.get(),
        policy_revision: revision(claim.policy_revision.get())?,
        inventory_owner_id: claim.inventory_owner_id.get(),
        facility_id: claim.facility_id.get(),
        sequence: claim.sequence,
        priority: claim.priority,
        instructions: claim.instructions,
        due_at: claim.due_at.map(|value| value.to_rfc3339()),
        lease_expires_at: claim.lease_expires_at.to_rfc3339(),
        source_inventory_balance_id: claim.source_inventory_balance_id.get(),
        item_batch_id: claim.item_batch_id.get(),
        item_id: claim.item_id.get(),
        item_description: claim.item_description,
        item_barcodes: claim
            .item_barcodes
            .into_iter()
            .map(|value| value.as_str().to_owned())
            .collect(),
        uom: claim.uom.as_str().to_owned(),
        lot: claim.lot,
        serial: claim.serial,
        expiration: claim.expiration.map(|value| value.to_rfc3339()),
        quantity: claim.quantity.get(),
        source_location: map_location(claim.source_location),
        destination_pick_face: map_location(claim.destination_pick_face),
    })
}

fn map_location(location: ReplenishmentLocationReadModel) -> ReplenishmentLocationResponse {
    ReplenishmentLocationResponse {
        location_id: location.location_id.get(),
        barcode: location.barcode.as_str().to_owned(),
        name: location.name,
    }
}

fn map_heartbeat(result: ReplenishmentClaimHeartbeatResult) -> ReplenishmentClaimHeartbeatResponse {
    ReplenishmentClaimHeartbeatResponse {
        work_id: result.work_id.get(),
        heartbeat_at: result.heartbeat_at.to_rfc3339(),
        lease_expires_at: result.lease_expires_at.to_rfc3339(),
    }
}

fn map_release(
    result: ReplenishmentClaimReleaseResult,
) -> V1Result<ReplenishmentClaimReleaseResponse> {
    if result.status != ReplenishmentWorkStatus::Pending {
        return Err(V1Error::internal(
            "claim release produced an invalid replenishment work status",
        ));
    }
    Ok(ReplenishmentClaimReleaseResponse {
        work_id: result.work_id.get(),
        status: map_work_status(result.status),
        released_at: result.released_at.to_rfc3339(),
        release_count: result.release_count,
        reason: map_release_reason(result.reason),
        note: result.note,
    })
}

fn map_confirmation(
    result: ConfirmReplenishmentWorkResult,
) -> V1Result<ReplenishmentConfirmationResponse> {
    if result.work_status != ReplenishmentWorkStatus::Completed {
        return Err(V1Error::internal(
            "confirmation produced an invalid replenishment work status",
        ));
    }
    Ok(ReplenishmentConfirmationResponse {
        confirmation_id: result.confirmation_id.get(),
        work_id: result.work_id.get(),
        plan_id: result.plan_id.get(),
        policy_id: result.policy_id.get(),
        inventory_transaction_id: result.inventory_transaction_id,
        source_inventory_balance_id: result.source_inventory_balance_id.get(),
        destination_inventory_balance_id: result.destination_inventory_balance_id.get(),
        item_batch_id: result.item_batch_id.get(),
        item_id: result.item_id.get(),
        uom: result.uom.as_str().to_owned(),
        lot: result.lot,
        serial: result.serial,
        source_location_id: result.source_location_id.get(),
        destination_pick_face_location_id: result.destination_pick_face_location_id.get(),
        quantity: result.quantity.get(),
        work_status: map_work_status(result.work_status),
        confirmed_by: result.confirmed_by.get(),
        confirmed_at: result.confirmed_at.to_rfc3339(),
    })
}

fn map_snapshot(snapshot: ReplenishmentPlanningSnapshot) -> ReplenishmentPlanningSnapshotResponse {
    ReplenishmentPlanningSnapshotResponse {
        pick_face_free: snapshot.pick_face_free().get(),
        active_inbound: snapshot.active_inbound().get(),
        projected_free: snapshot.projected_free().get(),
        unallocated_demand: snapshot.unallocated_demand().get(),
        reserve_free: snapshot.reserve_free().get(),
    }
}

fn map_source_ids(sources: &DomainSourceIds) -> V1Result<ReplenishmentReserveSourceLocationIds> {
    ReplenishmentReserveSourceLocationIds::new(
        sources.as_slice().iter().map(|value| value.get()).collect(),
    )
    .map_err(|_| V1Error::internal("repository produced invalid reserve source locations"))
}

const fn map_policy_status(status: ReplenishmentPolicyStatus) -> ApiPolicyStatus {
    match status {
        ReplenishmentPolicyStatus::Active => ApiPolicyStatus::Active,
        ReplenishmentPolicyStatus::Retired => ApiPolicyStatus::Retired,
    }
}

const fn map_planning_outcome(outcome: ReplenishmentPlanningOutcome) -> ApiPlanningOutcome {
    match outcome {
        ReplenishmentPlanningOutcome::NotNeeded => ApiPlanningOutcome::NotNeeded,
        ReplenishmentPlanningOutcome::InsufficientReserve => {
            ApiPlanningOutcome::InsufficientReserve
        }
        ReplenishmentPlanningOutcome::PartiallyPlanned => ApiPlanningOutcome::PartiallyPlanned,
        ReplenishmentPlanningOutcome::FullyPlanned => ApiPlanningOutcome::FullyPlanned,
    }
}

const fn map_work_status(status: ReplenishmentWorkStatus) -> ApiWorkStatus {
    match status {
        ReplenishmentWorkStatus::Pending => ApiWorkStatus::Pending,
        ReplenishmentWorkStatus::Claimed => ApiWorkStatus::Claimed,
        ReplenishmentWorkStatus::Completed => ApiWorkStatus::Completed,
        ReplenishmentWorkStatus::Cancelled => ApiWorkStatus::Cancelled,
    }
}

const fn map_work_status_to_domain(status: ApiWorkStatus) -> ReplenishmentWorkStatus {
    match status {
        ApiWorkStatus::Pending => ReplenishmentWorkStatus::Pending,
        ApiWorkStatus::Claimed => ReplenishmentWorkStatus::Claimed,
        ApiWorkStatus::Completed => ReplenishmentWorkStatus::Completed,
        ApiWorkStatus::Cancelled => ReplenishmentWorkStatus::Cancelled,
    }
}

const fn map_release_reason_to_domain(
    reason: ApiClaimReleaseReason,
) -> ReplenishmentClaimReleaseReason {
    match reason {
        ApiClaimReleaseReason::WorkInterrupted => ReplenishmentClaimReleaseReason::WorkInterrupted,
        ApiClaimReleaseReason::EquipmentUnavailable => {
            ReplenishmentClaimReleaseReason::EquipmentUnavailable
        }
        ApiClaimReleaseReason::SourceBlocked => ReplenishmentClaimReleaseReason::SourceBlocked,
        ApiClaimReleaseReason::DestinationBlocked => {
            ReplenishmentClaimReleaseReason::DestinationBlocked
        }
        ApiClaimReleaseReason::InventoryMismatch => {
            ReplenishmentClaimReleaseReason::InventoryMismatch
        }
        ApiClaimReleaseReason::SafetyIssue => ReplenishmentClaimReleaseReason::SafetyIssue,
        ApiClaimReleaseReason::Other => ReplenishmentClaimReleaseReason::Other,
    }
}

const fn map_release_reason(reason: ReplenishmentClaimReleaseReason) -> ApiClaimReleaseReason {
    match reason {
        ReplenishmentClaimReleaseReason::WorkInterrupted => ApiClaimReleaseReason::WorkInterrupted,
        ReplenishmentClaimReleaseReason::EquipmentUnavailable => {
            ApiClaimReleaseReason::EquipmentUnavailable
        }
        ReplenishmentClaimReleaseReason::SourceBlocked => ApiClaimReleaseReason::SourceBlocked,
        ReplenishmentClaimReleaseReason::DestinationBlocked => {
            ApiClaimReleaseReason::DestinationBlocked
        }
        ReplenishmentClaimReleaseReason::InventoryMismatch => {
            ApiClaimReleaseReason::InventoryMismatch
        }
        ReplenishmentClaimReleaseReason::SafetyIssue => ApiClaimReleaseReason::SafetyIssue,
        ReplenishmentClaimReleaseReason::Other => ApiClaimReleaseReason::Other,
    }
}

fn validate_release(body: &ReleaseReplenishmentClaimRequest) -> V1Result<()> {
    if let Some(note) = body.note.as_deref() {
        if note.is_empty() || note.trim() != note || note.chars().any(char::is_control) {
            return Err(invalid(
                "note must be trimmed, nonempty, and control-free when provided",
            ));
        }
        if note.chars().count() > MAX_RELEASE_NOTE_LENGTH {
            return Err(invalid(format!(
                "note cannot exceed {MAX_RELEASE_NOTE_LENGTH} characters"
            )));
        }
    }
    if body.reason == ApiClaimReleaseReason::Other && body.note.is_none() {
        return Err(invalid("note is required when reason is other"));
    }
    Ok(())
}

fn scan(value: String, label: &str) -> V1Result<ReplenishmentScanValue> {
    ReplenishmentScanValue::new(value).map_err(|error| invalid(format!("invalid {label}: {error}")))
}

fn optional_scan(value: Option<String>, label: &str) -> V1Result<Option<ReplenishmentScanValue>> {
    value.map(|value| scan(value, label)).transpose()
}

fn require_page_limit(limit: u16) -> V1Result<()> {
    if limit <= MAX_PAGE_LIMIT {
        Ok(())
    } else {
        Err(invalid(format!(
            "replenishment page limit must be between 1 and {MAX_PAGE_LIMIT}"
        )))
    }
}

fn item_id_value(value: i64) -> V1Result<CatalogItemId> {
    CatalogItemId::new(value).map_err(domain_validation)
}

fn location_id_value(value: i64) -> V1Result<LocationId> {
    LocationId::new(value).map_err(domain_validation)
}

fn policy_id_value(value: i64) -> V1Result<ReplenishmentPolicyId> {
    ReplenishmentPolicyId::new(value).map_err(domain_validation)
}

fn work_id_value(value: i64) -> V1Result<ReplenishmentWorkId> {
    ReplenishmentWorkId::new(value).map_err(domain_validation)
}

fn policy_revision(value: Revision) -> V1Result<ReplenishmentPolicyRevision> {
    ReplenishmentPolicyRevision::new(value.get()).map_err(domain_validation)
}

fn revision(value: i64) -> V1Result<Revision> {
    Revision::new(value).map_err(|_| V1Error::internal("repository produced an invalid revision"))
}

fn domain_validation(error: impl std::fmt::Display) -> V1Error {
    invalid(error.to_string())
}

fn invalid(message: impl Into<String>) -> V1Error {
    AppError::bad_request(message).into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PolicyCursorFilters {
    facility_id: Option<FacilityId>,
    inventory_owner_id: Option<InventoryOwnerId>,
    item_id: Option<CatalogItemId>,
    pick_face_location_id: Option<LocationId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PolicyCursor {
    filters: PolicyCursorFilters,
    after_policy_id: ReplenishmentPolicyId,
}

fn decode_policy_cursor(cursor: &OpaqueCursor) -> V1Result<PolicyCursor> {
    const RESOURCE: &str = "replenishment policies";
    let parts = cursor_parts(cursor, POLICY_CURSOR_PREFIX, 5, RESOURCE)?;
    Ok(PolicyCursor {
        filters: PolicyCursorFilters {
            facility_id: parse_optional_cursor_id(parts[0], FacilityId::new, RESOURCE)?,
            inventory_owner_id: parse_optional_cursor_id(
                parts[1],
                InventoryOwnerId::new,
                RESOURCE,
            )?,
            item_id: parse_optional_cursor_id(parts[2], CatalogItemId::new, RESOURCE)?,
            pick_face_location_id: parse_optional_cursor_id(parts[3], LocationId::new, RESOURCE)?,
        },
        after_policy_id: parse_cursor_id(parts[4], ReplenishmentPolicyId::new, RESOURCE)?,
    })
}

fn encode_policy_cursor(cursor: PolicyCursor) -> AppResult<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{POLICY_CURSOR_PREFIX}{}.{}.{}.{}.{:016x}",
        encode_optional_id(cursor.filters.facility_id.map(|value| value.get())),
        encode_optional_id(cursor.filters.inventory_owner_id.map(|value| value.get())),
        encode_optional_id(cursor.filters.item_id.map(|value| value.get())),
        encode_optional_id(
            cursor
                .filters
                .pick_face_location_id
                .map(|value| value.get())
        ),
        cursor.after_policy_id.get(),
    ))
    .map_err(|_| AppError::internal("generated an invalid replenishment policy cursor"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkCursorFilters {
    facility_id: Option<FacilityId>,
    inventory_owner_id: Option<InventoryOwnerId>,
    item_id: Option<CatalogItemId>,
    pick_face_location_id: Option<LocationId>,
    status: Option<ReplenishmentWorkStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkCursor {
    filters: WorkCursorFilters,
    after_work_id: ReplenishmentWorkId,
}

fn decode_work_cursor(cursor: &OpaqueCursor) -> V1Result<WorkCursor> {
    const RESOURCE: &str = "replenishment queue";
    let parts = cursor_parts(cursor, WORK_CURSOR_PREFIX, 6, RESOURCE)?;
    let status = match parts[4] {
        "a" => None,
        "p" => Some(ReplenishmentWorkStatus::Pending),
        "c" => Some(ReplenishmentWorkStatus::Claimed),
        "d" => Some(ReplenishmentWorkStatus::Completed),
        "x" => Some(ReplenishmentWorkStatus::Cancelled),
        _ => return Err(V1Error::invalid_cursor_for(RESOURCE)),
    };
    Ok(WorkCursor {
        filters: WorkCursorFilters {
            facility_id: parse_optional_cursor_id(parts[0], FacilityId::new, RESOURCE)?,
            inventory_owner_id: parse_optional_cursor_id(
                parts[1],
                InventoryOwnerId::new,
                RESOURCE,
            )?,
            item_id: parse_optional_cursor_id(parts[2], CatalogItemId::new, RESOURCE)?,
            pick_face_location_id: parse_optional_cursor_id(parts[3], LocationId::new, RESOURCE)?,
            status,
        },
        after_work_id: parse_cursor_id(parts[5], ReplenishmentWorkId::new, RESOURCE)?,
    })
}

fn encode_work_cursor(cursor: WorkCursor) -> AppResult<OpaqueCursor> {
    let status = match cursor.filters.status {
        None => "a",
        Some(ReplenishmentWorkStatus::Pending) => "p",
        Some(ReplenishmentWorkStatus::Claimed) => "c",
        Some(ReplenishmentWorkStatus::Completed) => "d",
        Some(ReplenishmentWorkStatus::Cancelled) => "x",
    };
    OpaqueCursor::new(format!(
        "{WORK_CURSOR_PREFIX}{}.{}.{}.{}.{status}.{:016x}",
        encode_optional_id(cursor.filters.facility_id.map(|value| value.get())),
        encode_optional_id(cursor.filters.inventory_owner_id.map(|value| value.get())),
        encode_optional_id(cursor.filters.item_id.map(|value| value.get())),
        encode_optional_id(
            cursor
                .filters
                .pick_face_location_id
                .map(|value| value.get())
        ),
        cursor.after_work_id.get(),
    ))
    .map_err(|_| AppError::internal("generated an invalid replenishment queue cursor"))
}

fn cursor_parts<'a>(
    cursor: &'a OpaqueCursor,
    prefix: &str,
    expected_len: usize,
    resource: &str,
) -> V1Result<Vec<&'a str>> {
    let encoded = cursor
        .as_str()
        .strip_prefix(prefix)
        .ok_or_else(|| V1Error::invalid_cursor_for(resource))?;
    let parts = encoded.split('.').collect::<Vec<_>>();
    if parts.len() == expected_len {
        Ok(parts)
    } else {
        Err(V1Error::invalid_cursor_for(resource))
    }
}

fn parse_optional_cursor_id<T, E>(
    encoded: &str,
    constructor: impl FnOnce(i64) -> Result<T, E>,
    resource: &str,
) -> V1Result<Option<T>> {
    if encoded == "a" {
        Ok(None)
    } else {
        parse_cursor_id(encoded, constructor, resource).map(Some)
    }
}

fn parse_cursor_id<T, E>(
    encoded: &str,
    constructor: impl FnOnce(i64) -> Result<T, E>,
    resource: &str,
) -> V1Result<T> {
    if encoded.len() != 16 {
        return Err(V1Error::invalid_cursor_for(resource));
    }
    let value = i64::from_str_radix(encoded, 16)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| V1Error::invalid_cursor_for(resource))?;
    constructor(value).map_err(|_| V1Error::invalid_cursor_for(resource))
}

fn encode_optional_id(value: Option<i64>) -> String {
    value.map_or_else(|| "a".to_owned(), |value| format!("{value:016x}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_mapping_builds_the_tenant_scoped_canonical_policy() {
        let command = configure_command(
            TenantId::new(1).unwrap(),
            ConfigureReplenishmentPolicyRequest {
                inventory_owner_id: 2,
                facility_id: 3,
                item_id: 4,
                uom: "each".into(),
                pick_face_location_id: 5,
                minimum_quantity: 6,
                target_quantity: 20,
                reserve_source_location_ids: ReplenishmentReserveSourceLocationIds::new(vec![
                    9, 7, 9,
                ])
                .unwrap(),
                expected_revision: Some(Revision::new(3).unwrap()),
            },
        )
        .unwrap();

        assert_eq!(command.definition.scope().tenant_id.get(), 1);
        assert_eq!(command.definition.thresholds().minimum().get(), 6);
        assert_eq!(
            command
                .definition
                .reserve_source_location_ids()
                .as_slice()
                .iter()
                .map(|value| value.get())
                .collect::<Vec<_>>(),
            vec![7, 9]
        );
        assert_eq!(command.expected_revision.map(|value| value.get()), Some(3));
    }

    #[test]
    fn policy_cursor_round_trips_and_binds_every_filter() {
        let expected = PolicyCursor {
            filters: PolicyCursorFilters {
                facility_id: Some(FacilityId::new(3).unwrap()),
                inventory_owner_id: Some(InventoryOwnerId::new(4).unwrap()),
                item_id: Some(CatalogItemId::new(5).unwrap()),
                pick_face_location_id: Some(LocationId::new(6).unwrap()),
            },
            after_policy_id: ReplenishmentPolicyId::new(7).unwrap(),
        };
        let cursor = encode_policy_cursor(expected).unwrap();
        assert_eq!(decode_policy_cursor(&cursor).unwrap(), expected);
    }

    #[test]
    fn work_cursor_round_trips_and_rejects_other_resource_or_status() {
        let expected = WorkCursor {
            filters: WorkCursorFilters {
                facility_id: Some(FacilityId::new(3).unwrap()),
                inventory_owner_id: None,
                item_id: Some(CatalogItemId::new(5).unwrap()),
                pick_face_location_id: None,
                status: Some(ReplenishmentWorkStatus::Claimed),
            },
            after_work_id: ReplenishmentWorkId::new(8).unwrap(),
        };
        let cursor = encode_work_cursor(expected).unwrap();
        assert_eq!(decode_work_cursor(&cursor).unwrap(), expected);

        for value in [
            "rp1.a.a.a.a.0000000000000001",
            "rw1.a.a.a.a.q.0000000000000001",
            "rw1.a.a.a.a.a.0000000000000000",
        ] {
            assert!(decode_work_cursor(&OpaqueCursor::new(value).unwrap()).is_err());
        }
    }

    #[test]
    fn release_and_scan_validation_reject_ambiguous_inputs() {
        assert!(validate_release(&ReleaseReplenishmentClaimRequest {
            reason: ApiClaimReleaseReason::Other,
            note: None,
        })
        .is_err());
        assert!(scan(" PICK-01 ".into(), "pick face").is_err());
        assert!(optional_scan(Some(String::new()), "lot").is_err());
    }

    #[test]
    fn snapshot_mapping_preserves_the_conserved_projection() {
        let snapshot = ReplenishmentPlanningSnapshot::new(
            ReplenishmentLevel::new(7).unwrap(),
            ReplenishmentLevel::new(3).unwrap(),
            ReplenishmentLevel::new(8).unwrap(),
            ReplenishmentLevel::new(20).unwrap(),
        )
        .unwrap();

        assert_eq!(
            map_snapshot(snapshot),
            ReplenishmentPlanningSnapshotResponse {
                pick_face_free: 7,
                active_inbound: 3,
                projected_free: 10,
                unallocated_demand: 8,
                reserve_free: 20,
            }
        );
    }
}
