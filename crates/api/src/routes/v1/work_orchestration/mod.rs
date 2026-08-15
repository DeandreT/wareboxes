use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    ConfigureWorkOrchestrationPolicyRequest, GenerateWorkOrchestrationPlanRequest, OpaqueCursor,
    OrchestrationPlanMode as ApiPlanMode, OrchestrationScoreEvidenceResponse,
    OrchestrationScoreResponse, OrchestrationSignalWorkspaceRequest,
    OrchestrationSignalWorkspaceResponse, OrchestrationWorkKind as ApiWorkKind,
    RecordResourceCapacitySignalRequest, RecordZoneCongestionSignalRequest,
    ResourceCapacitySignalResponse, Revision, WorkOrchestrationMode as ApiPolicyMode,
    WorkOrchestrationPlanItemResponse, WorkOrchestrationPlanPage as ApiPlanPage,
    WorkOrchestrationPlanPageRequest, WorkOrchestrationPlanResponse,
    WorkOrchestrationPlanSummaryResponse, WorkOrchestrationPolicyPage as ApiPolicyPage,
    WorkOrchestrationPolicyPageRequest, WorkOrchestrationPolicyResponse,
    WorkResourceKind as ApiResourceKind, ZoneCongestionSignalResponse,
};
use wareboxes_application::work_orchestration::{
    ConfigureWorkOrchestrationPolicyCommand, GenerateWorkOrchestrationPlanCommand,
    RecordResourceCapacityCommand, RecordZoneCongestionCommand, ResourceCapacitySignalReadModel,
    WorkOrchestrationPlanCursor, WorkOrchestrationPlanItemReadModel,
    WorkOrchestrationPlanPageQuery, WorkOrchestrationPlanReadModel,
    WorkOrchestrationPlanSummaryReadModel, WorkOrchestrationPolicyCursor,
    WorkOrchestrationPolicyPageQuery, WorkOrchestrationPolicyReadModel,
    WorkOrchestrationSignalCursor, WorkOrchestrationSignalQuery, ZoneCongestionSignalReadModel,
};
use wareboxes_domain::{
    LocationId, OrchestrationPlanMode, OrchestrationWorkKind, ResourceCapacitySignal,
    StorageZoneId, UserId, WorkOrchestrationMode, WorkOrchestrationPlanId,
    WorkOrchestrationPolicyDefinition, WorkOrchestrationPolicyId, WorkOrchestrationPolicyRevision,
    WorkOrchestrationSignalId, WorkResourceKind, ZoneCongestionSignal,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const READ_PERMISSION: &str = "wms";
const SUPERVISOR_PERMISSION: &str = "wms_supervisor";
const POLICY_CURSOR_PREFIX: &str = "wop1.";
const PLAN_CURSOR_PREFIX: &str = "wopl1.";
const SIGNAL_CURSOR_PREFIX: &str = "wos1.";

pub async fn list_policies(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<WorkOrchestrationPolicyPageRequest>,
) -> V1Result<Json<ApiPolicyPage>> {
    user.require_permission(&state.db, READ_PERMISSION).await?;
    let facility_id = request
        .facility_id
        .map(|id| user.require_facility(id))
        .transpose()?;
    let inventory_owner_id = request
        .inventory_owner_id
        .map(|id| user.require_inventory_owner(id))
        .transpose()?;
    let cursor = request
        .cursor
        .as_ref()
        .map(|cursor| decode_policy_cursor(cursor, &request))
        .transpose()?;
    let page = repo::work_orchestration::policy_page(
        &state.db,
        &user.tenant,
        WorkOrchestrationPolicyPageQuery {
            facility_id,
            inventory_owner_id,
            include_facility_defaults: request.include_facility_defaults,
            include_history: request.include_history,
            cursor,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = page
        .next_cursor
        .map(|cursor| encode_policy_cursor(cursor, &request))
        .transpose()?;
    Ok(Json(ApiPolicyPage::new(
        page.items
            .into_iter()
            .map(map_policy)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor,
    )))
}

pub async fn configure_policy(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<ConfigureWorkOrchestrationPolicyRequest>,
) -> V1Result<Json<WorkOrchestrationPolicyResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    let command = ConfigureWorkOrchestrationPolicyCommand {
        definition: WorkOrchestrationPolicyDefinition {
            tenant_id: user.tenant.tenant_id,
            facility_id: user.require_facility(body.facility_id)?,
            inventory_owner_id: body
                .inventory_owner_id
                .map(|id| user.require_inventory_owner(id))
                .transpose()?,
            mode: map_policy_mode(body.mode),
            priority_weight: body.priority_weight,
            due_urgency_weight: body.due_urgency_weight,
            proximity_weight: body.proximity_weight,
            interleaving_weight: body.interleaving_weight,
            congestion_penalty_weight: body.congestion_penalty_weight,
            bottleneck_penalty_weight: body.bottleneck_penalty_weight,
            due_horizon_minutes: body.due_horizon_minutes,
            max_candidates: body.max_candidates,
        },
        expected_revision: body
            .expected_revision
            .map(|revision| WorkOrchestrationPolicyRevision::new(revision.get()))
            .transpose()
            .map_err(validation)?,
    };
    let result = repo::work_orchestration::configure_policy(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_policy(result)?))
}

pub async fn signals(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<OrchestrationSignalWorkspaceRequest>,
) -> V1Result<Json<OrchestrationSignalWorkspaceResponse>> {
    user.require_permission(&state.db, READ_PERMISSION).await?;
    let zone_cursor = request
        .zone_cursor
        .as_ref()
        .map(|cursor| decode_signal_cursor(cursor, &request, "zone"))
        .transpose()?;
    let resource_cursor = request
        .resource_cursor
        .as_ref()
        .map(|cursor| decode_signal_cursor(cursor, &request, "resource"))
        .transpose()?;
    let workspace = repo::work_orchestration::signal_workspace(
        &state.db,
        &user.tenant,
        WorkOrchestrationSignalQuery {
            facility_id: user.require_facility(request.facility_id)?,
            include_history: request.include_history,
            zone_cursor,
            resource_cursor,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_zone_cursor = workspace
        .next_zone_cursor
        .map(|cursor| encode_signal_cursor(cursor, &request, "zone"))
        .transpose()?;
    let next_resource_cursor = workspace
        .next_resource_cursor
        .map(|cursor| encode_signal_cursor(cursor, &request, "resource"))
        .transpose()?;
    Ok(Json(OrchestrationSignalWorkspaceResponse {
        zone_signals: workspace
            .zone_signals
            .into_iter()
            .map(map_zone_signal)
            .collect(),
        resource_signals: workspace
            .resource_signals
            .into_iter()
            .map(map_resource_signal)
            .collect(),
        next_zone_cursor,
        next_resource_cursor,
    }))
}

pub async fn record_zone_signal(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<RecordZoneCongestionSignalRequest>,
) -> V1Result<Json<ZoneCongestionSignalResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    let command = RecordZoneCongestionCommand {
        tenant_id: user.tenant.tenant_id,
        facility_id: user.require_facility(body.facility_id)?,
        storage_zone_id: StorageZoneId::new(body.storage_zone_id).map_err(validation)?,
        signal: ZoneCongestionSignal {
            congestion_basis_points: body.congestion_basis_points,
            queue_depth: body.queue_depth,
            ttl_seconds: body.ttl_seconds,
        },
    };
    let result = repo::work_orchestration::record_zone_congestion(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_zone_signal(result)))
}

pub async fn record_resource_signal(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<RecordResourceCapacitySignalRequest>,
) -> V1Result<Json<ResourceCapacitySignalResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    let command = RecordResourceCapacityCommand {
        tenant_id: user.tenant.tenant_id,
        facility_id: user.require_facility(body.facility_id)?,
        resource_kind: map_resource_kind(body.resource_kind),
        signal: ResourceCapacitySignal {
            available_units: body.available_units,
            demand_units: body.demand_units,
            ttl_seconds: body.ttl_seconds,
        },
    };
    let result = repo::work_orchestration::record_resource_capacity(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_resource_signal(result)))
}

pub async fn list_plans(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<WorkOrchestrationPlanPageRequest>,
) -> V1Result<Json<ApiPlanPage>> {
    user.require_permission(&state.db, READ_PERMISSION).await?;
    let facility_id = request
        .facility_id
        .map(|id| user.require_facility(id))
        .transpose()?;
    let inventory_owner_id = request
        .inventory_owner_id
        .map(|id| user.require_inventory_owner(id))
        .transpose()?;
    let cursor = request
        .cursor
        .as_ref()
        .map(|cursor| decode_plan_cursor(cursor, &request))
        .transpose()?;
    let page = repo::work_orchestration::plan_page(
        &state.db,
        &user.tenant,
        WorkOrchestrationPlanPageQuery {
            facility_id,
            inventory_owner_id,
            plan_mode: request.plan_mode.map(map_plan_mode),
            cursor,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = page
        .next_cursor
        .map(|cursor| encode_plan_cursor(cursor, &request))
        .transpose()?;
    Ok(Json(ApiPlanPage::new(
        page.items
            .into_iter()
            .map(map_plan_summary)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor,
    )))
}

pub async fn generate_plan(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<GenerateWorkOrchestrationPlanRequest>,
) -> V1Result<Json<WorkOrchestrationPlanResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    let command = GenerateWorkOrchestrationPlanCommand {
        tenant_id: user.tenant.tenant_id,
        facility_id: user.require_facility(body.facility_id)?,
        inventory_owner_id: body
            .inventory_owner_id
            .map(|id| user.require_inventory_owner(id))
            .transpose()?,
        current_location_id: LocationId::new(body.current_location_id).map_err(validation)?,
        previous_work_kind: body.previous_work_kind.map(map_work_kind),
        generated_for_user_id: body
            .generated_for_user_id
            .map(|id| UserId::new(id).map_err(validation))
            .transpose()?,
        expected_policy_id: WorkOrchestrationPolicyId::new(body.expected_policy_id)
            .map_err(validation)?,
        expected_policy_revision: WorkOrchestrationPolicyRevision::new(
            body.expected_policy_revision.get(),
        )
        .map_err(validation)?,
    };
    let result = repo::work_orchestration::generate_plan(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_plan(result)?))
}

pub async fn get_plan(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(plan_id): Path<i64>,
) -> V1Result<Json<WorkOrchestrationPlanResponse>> {
    user.require_permission(&state.db, READ_PERMISSION).await?;
    let result = repo::work_orchestration::plan_by_id(
        &state.db,
        &user.tenant,
        WorkOrchestrationPlanId::new(plan_id).map_err(validation)?,
    )
    .await?;
    Ok(Json(map_plan(result)?))
}

fn map_policy(
    value: WorkOrchestrationPolicyReadModel,
) -> V1Result<WorkOrchestrationPolicyResponse> {
    Ok(WorkOrchestrationPolicyResponse {
        policy_id: value.policy_id.get(),
        facility_id: value.definition.facility_id.get(),
        inventory_owner_id: value.definition.inventory_owner_id.map(|id| id.get()),
        mode: map_policy_mode_to_api(value.definition.mode),
        priority_weight: value.definition.priority_weight,
        due_urgency_weight: value.definition.due_urgency_weight,
        proximity_weight: value.definition.proximity_weight,
        interleaving_weight: value.definition.interleaving_weight,
        congestion_penalty_weight: value.definition.congestion_penalty_weight,
        bottleneck_penalty_weight: value.definition.bottleneck_penalty_weight,
        due_horizon_minutes: value.definition.due_horizon_minutes,
        max_candidates: value.definition.max_candidates,
        revision: Revision::new(value.revision.get()).map_err(invalid_result)?,
        configured_by: value.configured_by.get(),
        configured_at: value.configured_at.to_rfc3339(),
        effective_from: value.effective_from.to_rfc3339(),
        supersedes_policy_id: value.supersedes_policy_id.map(|id| id.get()),
        effective_to: value.effective_to.map(|value| value.to_rfc3339()),
    })
}

fn map_zone_signal(value: ZoneCongestionSignalReadModel) -> ZoneCongestionSignalResponse {
    ZoneCongestionSignalResponse {
        signal_id: value.signal_id.get(),
        facility_id: value.facility_id.get(),
        storage_zone_id: value.storage_zone_id.get(),
        storage_zone_code: value.storage_zone_code,
        congestion_basis_points: value.signal.congestion_basis_points,
        queue_depth: value.signal.queue_depth,
        ttl_seconds: value.signal.ttl_seconds,
        recorded_by: value.recorded_by.get(),
        observed_at: value.observed_at.to_rfc3339(),
        expires_at: value.expires_at.to_rfc3339(),
    }
}

fn map_resource_signal(value: ResourceCapacitySignalReadModel) -> ResourceCapacitySignalResponse {
    ResourceCapacitySignalResponse {
        signal_id: value.signal_id.get(),
        facility_id: value.facility_id.get(),
        resource_kind: map_resource_kind_to_api(value.resource_kind),
        available_units: value.signal.available_units,
        demand_units: value.signal.demand_units,
        utilization_basis_points: value.utilization_basis_points,
        ttl_seconds: value.signal.ttl_seconds,
        recorded_by: value.recorded_by.get(),
        observed_at: value.observed_at.to_rfc3339(),
        expires_at: value.expires_at.to_rfc3339(),
    }
}

fn map_plan(value: WorkOrchestrationPlanReadModel) -> V1Result<WorkOrchestrationPlanResponse> {
    Ok(WorkOrchestrationPlanResponse {
        plan_id: value.plan_id.get(),
        facility_id: value.facility_id.get(),
        requested_inventory_owner_id: value.requested_inventory_owner_id.map(|id| id.get()),
        current_location_id: value.current_location_id.get(),
        current_location_label: value.current_location_label,
        previous_work_kind: value.previous_work_kind.map(map_work_kind_to_api),
        generated_for_user_id: value.generated_for_user_id.map(|id| id.get()),
        policy_id: value.policy_id.get(),
        policy_revision: Revision::new(value.policy_revision.get()).map_err(invalid_result)?,
        policy_inventory_owner_id: value.policy_inventory_owner_id.map(|id| id.get()),
        plan_mode: map_plan_mode_to_api(value.plan_mode),
        input_snapshot_at: value.input_snapshot_at.to_rfc3339(),
        configuration_snapshot: value.configuration_snapshot,
        candidate_count: value.candidate_count,
        item_count: value.item_count,
        generated_by: value.generated_by.get(),
        generated_at: value.generated_at.to_rfc3339(),
        items: value.items.into_iter().map(map_plan_item).collect(),
    })
}

fn map_plan_summary(
    value: WorkOrchestrationPlanSummaryReadModel,
) -> V1Result<WorkOrchestrationPlanSummaryResponse> {
    Ok(WorkOrchestrationPlanSummaryResponse {
        plan_id: value.plan_id.get(),
        facility_id: value.facility_id.get(),
        requested_inventory_owner_id: value.requested_inventory_owner_id.map(|id| id.get()),
        current_location_id: value.current_location_id.get(),
        current_location_label: value.current_location_label,
        previous_work_kind: value.previous_work_kind.map(map_work_kind_to_api),
        generated_for_user_id: value.generated_for_user_id.map(|id| id.get()),
        policy_id: value.policy_id.get(),
        policy_revision: Revision::new(value.policy_revision.get()).map_err(invalid_result)?,
        policy_inventory_owner_id: value.policy_inventory_owner_id.map(|id| id.get()),
        plan_mode: map_plan_mode_to_api(value.plan_mode),
        input_snapshot_at: value.input_snapshot_at.to_rfc3339(),
        candidate_count: value.candidate_count,
        item_count: value.item_count,
        generated_by: value.generated_by.get(),
        generated_at: value.generated_at.to_rfc3339(),
    })
}

fn map_plan_item(value: WorkOrchestrationPlanItemReadModel) -> WorkOrchestrationPlanItemResponse {
    WorkOrchestrationPlanItemResponse {
        plan_item_id: value.plan_item_id.get(),
        sequence: value.sequence,
        work_task_id: value.work_task_id,
        work_kind: map_work_kind_to_api(value.work_kind),
        inventory_owner_id: value.inventory_owner_id.map(|id| id.get()),
        title: value.title,
        instructions: value.instructions,
        task_status: value.task_status,
        task_created_at: value.task_created_at.to_rfc3339(),
        source_location_label: value.source_location_label,
        destination_location_label: value.destination_location_label,
        zone_signal_id: value.zone_signal_id.map(|id| id.get()),
        resource_signal_id: value.resource_signal_id.map(|id| id.get()),
        evidence: OrchestrationScoreEvidenceResponse {
            work_kind: map_work_kind_to_api(value.evidence.work_kind),
            task_priority: value.evidence.task_priority,
            due_at: value.evidence.due_at.map(|value| value.to_rfc3339()),
            overdue_seconds: value.evidence.overdue_seconds,
            due_urgency_basis_points: value.evidence.due_urgency_basis_points,
            current_location_id: value.evidence.current_location_id.get(),
            source_location_id: value.evidence.source_location_id.get(),
            destination_location_id: value.evidence.destination_location_id.map(|id| id.get()),
            current_travel_sequence: value.evidence.current_travel_sequence,
            source_travel_sequence: value.evidence.source_travel_sequence,
            destination_travel_sequence: value.evidence.destination_travel_sequence,
            travel_distance: value.evidence.travel_distance,
            proximity_basis_points: value.evidence.proximity_basis_points,
            previous_work_kind: value.evidence.previous_work_kind.map(map_work_kind_to_api),
            interleaving_compatible: value.evidence.interleaving_compatible,
            source_zone_id: value.evidence.source_zone_id,
            source_zone_code: value.evidence.source_zone_code,
            congestion_basis_points: value.evidence.congestion_basis_points,
            congestion_queue_depth: value.evidence.congestion_queue_depth,
            resource_kind: map_resource_kind_to_api(value.evidence.resource_kind),
            resource_available_units: value.evidence.resource_available_units,
            resource_demand_units: value.evidence.resource_demand_units,
            resource_utilization_basis_points: value.evidence.resource_utilization_basis_points,
        },
        score: OrchestrationScoreResponse {
            priority_component: value.score.priority_component,
            due_urgency_component: value.score.due_urgency_component,
            proximity_component: value.score.proximity_component,
            interleaving_component: value.score.interleaving_component,
            congestion_penalty: value.score.congestion_penalty,
            bottleneck_penalty: value.score.bottleneck_penalty,
            total: value.score.total,
        },
    }
}

const fn map_policy_mode(value: ApiPolicyMode) -> WorkOrchestrationMode {
    match value {
        ApiPolicyMode::Enabled => WorkOrchestrationMode::Enabled,
        ApiPolicyMode::Disabled => WorkOrchestrationMode::Disabled,
    }
}

const fn map_policy_mode_to_api(value: WorkOrchestrationMode) -> ApiPolicyMode {
    match value {
        WorkOrchestrationMode::Enabled => ApiPolicyMode::Enabled,
        WorkOrchestrationMode::Disabled => ApiPolicyMode::Disabled,
    }
}

const fn map_plan_mode(value: ApiPlanMode) -> OrchestrationPlanMode {
    match value {
        ApiPlanMode::Optimized => OrchestrationPlanMode::Optimized,
        ApiPlanMode::ManualFifo => OrchestrationPlanMode::ManualFifo,
    }
}

const fn map_plan_mode_to_api(value: OrchestrationPlanMode) -> ApiPlanMode {
    match value {
        OrchestrationPlanMode::Optimized => ApiPlanMode::Optimized,
        OrchestrationPlanMode::ManualFifo => ApiPlanMode::ManualFifo,
    }
}

const fn map_work_kind(value: ApiWorkKind) -> OrchestrationWorkKind {
    match value {
        ApiWorkKind::CycleCountItemLocation => OrchestrationWorkKind::CycleCountItemLocation,
        ApiWorkKind::CycleCountLocation => OrchestrationWorkKind::CycleCountLocation,
        ApiWorkKind::Putaway => OrchestrationWorkKind::Putaway,
        ApiWorkKind::LicensePlatePutaway => OrchestrationWorkKind::LicensePlatePutaway,
        ApiWorkKind::InventoryRelocation => OrchestrationWorkKind::InventoryRelocation,
        ApiWorkKind::Replenishment => OrchestrationWorkKind::Replenishment,
        ApiWorkKind::CrossDock => OrchestrationWorkKind::CrossDock,
    }
}

const fn map_work_kind_to_api(value: OrchestrationWorkKind) -> ApiWorkKind {
    match value {
        OrchestrationWorkKind::CycleCountItemLocation => ApiWorkKind::CycleCountItemLocation,
        OrchestrationWorkKind::CycleCountLocation => ApiWorkKind::CycleCountLocation,
        OrchestrationWorkKind::Putaway => ApiWorkKind::Putaway,
        OrchestrationWorkKind::LicensePlatePutaway => ApiWorkKind::LicensePlatePutaway,
        OrchestrationWorkKind::InventoryRelocation => ApiWorkKind::InventoryRelocation,
        OrchestrationWorkKind::Replenishment => ApiWorkKind::Replenishment,
        OrchestrationWorkKind::CrossDock => ApiWorkKind::CrossDock,
    }
}

const fn map_resource_kind(value: ApiResourceKind) -> WorkResourceKind {
    match value {
        ApiResourceKind::GeneralLabor => WorkResourceKind::GeneralLabor,
        ApiResourceKind::InventoryControl => WorkResourceKind::InventoryControl,
        ApiResourceKind::MaterialHandling => WorkResourceKind::MaterialHandling,
        ApiResourceKind::DockDoor => WorkResourceKind::DockDoor,
        ApiResourceKind::PackStation => WorkResourceKind::PackStation,
        ApiResourceKind::Automation => WorkResourceKind::Automation,
    }
}

const fn map_resource_kind_to_api(value: WorkResourceKind) -> ApiResourceKind {
    match value {
        WorkResourceKind::GeneralLabor => ApiResourceKind::GeneralLabor,
        WorkResourceKind::InventoryControl => ApiResourceKind::InventoryControl,
        WorkResourceKind::MaterialHandling => ApiResourceKind::MaterialHandling,
        WorkResourceKind::DockDoor => ApiResourceKind::DockDoor,
        WorkResourceKind::PackStation => ApiResourceKind::PackStation,
        WorkResourceKind::Automation => ApiResourceKind::Automation,
    }
}

fn policy_filter(request: &WorkOrchestrationPolicyPageRequest) -> String {
    format!(
        "{:016x}.{:016x}.{}.{}",
        request.facility_id.unwrap_or_default(),
        request.inventory_owner_id.unwrap_or_default(),
        u8::from(request.include_facility_defaults),
        u8::from(request.include_history)
    )
}

fn encode_policy_cursor(
    cursor: WorkOrchestrationPolicyCursor,
    request: &WorkOrchestrationPolicyPageRequest,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{POLICY_CURSOR_PREFIX}{}.{:016x}.{:016x}",
        policy_filter(request),
        cursor.after_configured_at.timestamp_micros(),
        cursor.after_policy_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid work orchestration policy cursor"))
}

fn decode_policy_cursor(
    cursor: &OpaqueCursor,
    request: &WorkOrchestrationPolicyPageRequest,
) -> V1Result<WorkOrchestrationPolicyCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(POLICY_CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("work orchestration policy"))?;
    let mut parts = encoded.rsplitn(3, '.');
    let id = parts
        .next()
        .ok_or_else(|| V1Error::invalid_cursor_for("work orchestration policy"))?;
    let micros = parts
        .next()
        .ok_or_else(|| V1Error::invalid_cursor_for("work orchestration policy"))?;
    let filter = parts
        .next()
        .ok_or_else(|| V1Error::invalid_cursor_for("work orchestration policy"))?;
    if filter != policy_filter(request) {
        return Err(V1Error::invalid_cursor_for("work orchestration policy"));
    }
    Ok(WorkOrchestrationPolicyCursor {
        after_configured_at: decode_timestamp(micros, "work orchestration policy")?,
        after_policy_id: WorkOrchestrationPolicyId::new(
            i64::from_str_radix(id, 16)
                .map_err(|_| V1Error::invalid_cursor_for("work orchestration policy"))?,
        )
        .map_err(|_| V1Error::invalid_cursor_for("work orchestration policy"))?,
    })
}

fn signal_filter(request: &OrchestrationSignalWorkspaceRequest, stream: &str) -> String {
    format!(
        "{:016x}.{}.{stream}",
        request.facility_id,
        u8::from(request.include_history)
    )
}

fn encode_signal_cursor(
    cursor: WorkOrchestrationSignalCursor,
    request: &OrchestrationSignalWorkspaceRequest,
    stream: &str,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{SIGNAL_CURSOR_PREFIX}{}.{:016x}.{:016x}",
        signal_filter(request, stream),
        cursor.after_observed_at.timestamp_micros(),
        cursor.after_signal_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid work orchestration signal cursor"))
}

fn decode_signal_cursor(
    cursor: &OpaqueCursor,
    request: &OrchestrationSignalWorkspaceRequest,
    stream: &str,
) -> V1Result<WorkOrchestrationSignalCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(SIGNAL_CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("work orchestration signal"))?;
    let mut parts = encoded.rsplitn(3, '.');
    let id = parts
        .next()
        .ok_or_else(|| V1Error::invalid_cursor_for("work orchestration signal"))?;
    let micros = parts
        .next()
        .ok_or_else(|| V1Error::invalid_cursor_for("work orchestration signal"))?;
    let filter = parts
        .next()
        .ok_or_else(|| V1Error::invalid_cursor_for("work orchestration signal"))?;
    if filter != signal_filter(request, stream) {
        return Err(V1Error::invalid_cursor_for("work orchestration signal"));
    }
    Ok(WorkOrchestrationSignalCursor {
        after_observed_at: decode_timestamp(micros, "work orchestration signal")?,
        after_signal_id: WorkOrchestrationSignalId::new(
            i64::from_str_radix(id, 16)
                .map_err(|_| V1Error::invalid_cursor_for("work orchestration signal"))?,
        )
        .map_err(|_| V1Error::invalid_cursor_for("work orchestration signal"))?,
    })
}

fn plan_filter(request: &WorkOrchestrationPlanPageRequest) -> String {
    let mode = match request.plan_mode {
        Some(ApiPlanMode::Optimized) => "optimized",
        Some(ApiPlanMode::ManualFifo) => "manual_fifo",
        None => "all",
    };
    format!(
        "{:016x}.{:016x}.{mode}",
        request.facility_id.unwrap_or_default(),
        request.inventory_owner_id.unwrap_or_default()
    )
}

fn encode_plan_cursor(
    cursor: WorkOrchestrationPlanCursor,
    request: &WorkOrchestrationPlanPageRequest,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{PLAN_CURSOR_PREFIX}{}.{:016x}.{:016x}",
        plan_filter(request),
        cursor.after_generated_at.timestamp_micros(),
        cursor.after_plan_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid work orchestration plan cursor"))
}

fn decode_plan_cursor(
    cursor: &OpaqueCursor,
    request: &WorkOrchestrationPlanPageRequest,
) -> V1Result<WorkOrchestrationPlanCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(PLAN_CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("work orchestration plan"))?;
    let mut parts = encoded.rsplitn(3, '.');
    let id = parts
        .next()
        .ok_or_else(|| V1Error::invalid_cursor_for("work orchestration plan"))?;
    let micros = parts
        .next()
        .ok_or_else(|| V1Error::invalid_cursor_for("work orchestration plan"))?;
    let filter = parts
        .next()
        .ok_or_else(|| V1Error::invalid_cursor_for("work orchestration plan"))?;
    if filter != plan_filter(request) {
        return Err(V1Error::invalid_cursor_for("work orchestration plan"));
    }
    Ok(WorkOrchestrationPlanCursor {
        after_generated_at: decode_timestamp(micros, "work orchestration plan")?,
        after_plan_id: WorkOrchestrationPlanId::new(
            i64::from_str_radix(id, 16)
                .map_err(|_| V1Error::invalid_cursor_for("work orchestration plan"))?,
        )
        .map_err(|_| V1Error::invalid_cursor_for("work orchestration plan"))?,
    })
}

fn decode_timestamp(value: &str, label: &str) -> V1Result<chrono::DateTime<chrono::Utc>> {
    let micros = i64::from_str_radix(value, 16).map_err(|_| V1Error::invalid_cursor_for(label))?;
    chrono::DateTime::from_timestamp_micros(micros)
        .ok_or_else(|| V1Error::invalid_cursor_for(label))
}

fn validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

fn invalid_result(error: impl std::fmt::Display) -> V1Error {
    V1Error::internal(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::PageLimit;

    #[test]
    fn plan_cursor_round_trips_and_is_bound_to_filters() {
        let request = WorkOrchestrationPlanPageRequest {
            facility_id: Some(2),
            inventory_owner_id: Some(3),
            plan_mode: Some(ApiPlanMode::Optimized),
            cursor: None,
            limit: PageLimit::default(),
        };
        let cursor = WorkOrchestrationPlanCursor {
            after_generated_at: chrono::Utc::now(),
            after_plan_id: WorkOrchestrationPlanId::new(7).unwrap(),
        };
        let encoded = encode_plan_cursor(cursor, &request).unwrap();
        let decoded = decode_plan_cursor(&encoded, &request).unwrap();
        assert_eq!(decoded.after_plan_id, cursor.after_plan_id);
        assert_eq!(
            decoded.after_generated_at.timestamp_micros(),
            cursor.after_generated_at.timestamp_micros()
        );
        let mut changed = request;
        changed.plan_mode = Some(ApiPlanMode::ManualFifo);
        assert!(decode_plan_cursor(&encoded, &changed).is_err());
    }

    #[test]
    fn signal_cursor_round_trips_and_is_bound_to_stream_and_filters() {
        let request = OrchestrationSignalWorkspaceRequest {
            facility_id: 2,
            include_history: true,
            zone_cursor: None,
            resource_cursor: None,
            limit: PageLimit::default(),
        };
        let cursor = WorkOrchestrationSignalCursor {
            after_observed_at: chrono::Utc::now(),
            after_signal_id: WorkOrchestrationSignalId::new(7).unwrap(),
        };
        let encoded = encode_signal_cursor(cursor, &request, "zone").unwrap();
        let decoded = decode_signal_cursor(&encoded, &request, "zone").unwrap();
        assert_eq!(decoded.after_signal_id, cursor.after_signal_id);
        assert_eq!(
            decoded.after_observed_at.timestamp_micros(),
            cursor.after_observed_at.timestamp_micros()
        );
        assert!(decode_signal_cursor(&encoded, &request, "resource").is_err());
        let mut changed = request;
        changed.include_history = false;
        assert!(decode_signal_cursor(&encoded, &changed, "zone").is_err());
    }
}
