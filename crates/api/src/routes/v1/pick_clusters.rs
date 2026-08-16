use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    CancelPickClusterRequest, ChangePickCartStatusRequest, ClaimNextClusterPickRequest,
    CreatePickCartRequest, CurrentPickResponse, PickCartResponse, PickCartSlotResponse,
    PickCartStatus as ApiCartStatus, PickClusterCandidateResponse, PickClusterMemberResponse,
    PickClusterResponse, PickClusterStatus as ApiClusterStatus, PickClusterWorkspaceRequest,
    PickClusterWorkspaceResponse, PickRouteMode as ApiPickRouteMode, PlanPickClusterRequest,
};
use wareboxes_application::pick_cluster::{
    CancelPickClusterCommand, ChangePickCartStatusCommand, ClaimNextClusterPickCommand,
    CreatePickCartCommand, PickCartReadModel, PickClusterCandidateReadModel, PickClusterReadModel,
    PickClusterTaskAssignment, PickClusterWorkspace, PickClusterWorkspaceQuery,
    PlanPickClusterCommand,
};
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, PickCartBarcode, PickCartId, PickCartName, PickCartSlotCode,
    PickCartSlotId, PickCartStatus, PickClusterId, PickClusterStatus, PickTaskId,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

pub async fn workspace(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<PickClusterWorkspaceRequest>,
) -> V1Result<Json<PickClusterWorkspaceResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let query = PickClusterWorkspaceQuery {
        facility_id: facility_id(request.facility_id)?,
        inventory_owner_id: owner_id(request.inventory_owner_id)?,
        include_history: request.include_history,
    };
    let result = repo::picking::cluster_workspace(&state.db, &user.tenant, query).await?;
    Ok(Json(map_workspace(result)))
}

pub async fn create_cart(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreatePickCartRequest>,
) -> V1Result<Json<PickCartResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let command = CreatePickCartCommand {
        facility_id: facility_id(body.facility_id)?,
        barcode: PickCartBarcode::new(body.barcode).map_err(domain_validation)?,
        name: PickCartName::new(body.name).map_err(domain_validation)?,
        slot_codes: body
            .slot_codes
            .into_iter()
            .map(PickCartSlotCode::new)
            .collect::<Result<Vec<_>, _>>()
            .map_err(domain_validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::picking::create_cart(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_cart(result)))
}

pub async fn change_cart_status(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(cart_id_value): Path<i64>,
    Json(body): Json<ChangePickCartStatusRequest>,
) -> V1Result<Json<PickCartResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let command = ChangePickCartStatusCommand {
        cart_id: cart_id(cart_id_value)?,
        expected_revision: body.expected_revision,
        status: map_cart_status(body.status),
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::picking::change_cart_status(&state.db, &user.tenant, &context, command).await?;
    Ok(Json(map_cart(result)))
}

pub async fn plan(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<PlanPickClusterRequest>,
) -> V1Result<Json<PickClusterResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let command = PlanPickClusterCommand {
        inventory_owner_id: owner_id(body.inventory_owner_id)?,
        facility_id: facility_id(body.facility_id)?,
        cart_id: cart_id(body.cart_id)?,
        assignments: body
            .assignments
            .into_iter()
            .map(|assignment| {
                Ok(PickClusterTaskAssignment {
                    task_id: PickTaskId::new(assignment.task_id).map_err(domain_validation)?,
                    slot_id: PickCartSlotId::new(assignment.slot_id).map_err(domain_validation)?,
                })
            })
            .collect::<V1Result<Vec<_>>>()?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::picking::plan_cluster(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_cluster(result)))
}

pub async fn claim_next(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(cluster_id_value): Path<i64>,
    Json(_body): Json<ClaimNextClusterPickRequest>,
) -> V1Result<Json<CurrentPickResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let command = ClaimNextClusterPickCommand {
        cluster_id: cluster_id(cluster_id_value)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::picking::claim_next_cluster(&state.db, &user.tenant, &context, command).await?;
    Ok(Json(result.map(super::picking::map_claim).transpose()?))
}

pub async fn cancel(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(cluster_id_value): Path<i64>,
    Json(body): Json<CancelPickClusterRequest>,
) -> V1Result<Json<PickClusterResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let command = CancelPickClusterCommand {
        cluster_id: cluster_id(cluster_id_value)?,
        expected_revision: body.expected_revision,
        note: body.note,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::picking::cancel_cluster(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_cluster(result)))
}

fn map_workspace(value: PickClusterWorkspace) -> PickClusterWorkspaceResponse {
    PickClusterWorkspaceResponse {
        carts: value.carts.into_iter().map(map_cart).collect(),
        candidates: value.candidates.into_iter().map(map_candidate).collect(),
        clusters: value.clusters.into_iter().map(map_cluster).collect(),
    }
}

fn map_cart(value: PickCartReadModel) -> PickCartResponse {
    PickCartResponse {
        cart_id: value.cart_id.get(),
        facility_id: value.facility_id.get(),
        barcode: value.barcode.as_str().to_owned(),
        name: value.name.as_str().to_owned(),
        status: match value.status {
            PickCartStatus::Active => ApiCartStatus::Active,
            PickCartStatus::OutOfService => ApiCartStatus::OutOfService,
            PickCartStatus::Retired => ApiCartStatus::Retired,
        },
        revision: value.revision,
        slots: value
            .slots
            .into_iter()
            .map(|slot| PickCartSlotResponse {
                slot_id: slot.slot_id.get(),
                code: slot.code.as_str().to_owned(),
                sequence: slot.sequence,
            })
            .collect(),
        created_by: value.created_by.get(),
        created_at: value.created_at.to_rfc3339(),
        status_changed_by: value.status_changed_by.map(|user| user.get()),
        status_changed_at: value.status_changed_at.map(|time| time.to_rfc3339()),
    }
}

fn map_candidate(value: PickClusterCandidateReadModel) -> PickClusterCandidateResponse {
    PickClusterCandidateResponse {
        task_id: value.task_id.get(),
        order_id: value.order_id.get(),
        order_key: value.order_key,
        source_location_id: value.source_location_id,
        source_inventory_balance_id: value.source_inventory_balance_id,
        source_location_barcode: value.source_location_barcode,
        source_location_name: value.source_location_name,
        source_travel_sequence: value.source_travel_sequence,
        item_id: value.item_id,
        item_batch_id: value.item_batch_id,
        item_description: value.item_description,
        uom: value.uom,
        inventory_status: value.inventory_status,
        planned_quantity: value.planned_quantity,
        priority: value.priority,
        ship_by: value.ship_by.map(|time| time.to_rfc3339()),
        created_at: value.created_at.to_rfc3339(),
    }
}

fn map_cluster(value: PickClusterReadModel) -> PickClusterResponse {
    PickClusterResponse {
        cluster_id: value.cluster_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        facility_id: value.facility_id.get(),
        cart_id: value.cart_id.get(),
        cart_barcode: value.cart_barcode.as_str().to_owned(),
        cart_name: value.cart_name.as_str().to_owned(),
        mode: match value.mode {
            wareboxes_domain::PickRouteMode::ClusterCart => ApiPickRouteMode::ClusterCart,
            wareboxes_domain::PickRouteMode::BatchCart => ApiPickRouteMode::BatchCart,
        },
        batch_source_inventory_balance_id: value.batch_source_inventory_balance_id,
        batch_source_location_id: value.batch_source_location_id,
        batch_source_location_barcode: value.batch_source_location_barcode,
        batch_item_batch_id: value.batch_item_batch_id,
        batch_uom: value.batch_uom,
        batch_inventory_status: value.batch_inventory_status,
        batch_total_quantity: value.batch_total_quantity,
        status: match value.status {
            PickClusterStatus::Planned => ApiClusterStatus::Planned,
            PickClusterStatus::InProgress => ApiClusterStatus::InProgress,
            PickClusterStatus::Completed => ApiClusterStatus::Completed,
            PickClusterStatus::Cancelled => ApiClusterStatus::Cancelled,
        },
        revision: value.revision,
        task_count: value.task_count,
        order_count: value.order_count,
        completed_task_count: value.completed_task_count,
        assigned_user_id: value.assigned_user_id.map(|user| user.get()),
        planned_by: value.planned_by.get(),
        planned_at: value.planned_at.to_rfc3339(),
        started_at: value.started_at.map(|time| time.to_rfc3339()),
        completed_at: value.completed_at.map(|time| time.to_rfc3339()),
        cancelled_by: value.cancelled_by.map(|user| user.get()),
        cancelled_at: value.cancelled_at.map(|time| time.to_rfc3339()),
        cancellation_note: value.cancellation_note,
        members: value
            .members
            .into_iter()
            .map(|member| PickClusterMemberResponse {
                member_id: member.member_id.get(),
                sequence: member.sequence,
                task_id: member.task_id.get(),
                task_status: member.task_status,
                order_id: member.order_id.get(),
                order_key: member.order_key,
                slot_id: member.slot_id.get(),
                slot_code: member.slot_code.as_str().to_owned(),
                source_location_id: member.source_location_id,
                source_inventory_balance_id: member.source_inventory_balance_id,
                source_location_barcode: member.source_location_barcode,
                source_location_name: member.source_location_name,
                item_id: member.item_id,
                item_batch_id: member.item_batch_id,
                item_description: member.item_description,
                uom: member.uom,
                inventory_status: member.inventory_status,
                planned_quantity: member.planned_quantity,
            })
            .collect(),
    }
}

fn map_cart_status(value: ApiCartStatus) -> PickCartStatus {
    match value {
        ApiCartStatus::Active => PickCartStatus::Active,
        ApiCartStatus::OutOfService => PickCartStatus::OutOfService,
        ApiCartStatus::Retired => PickCartStatus::Retired,
    }
}

fn facility_id(value: i64) -> V1Result<FacilityId> {
    FacilityId::new(value).map_err(domain_validation)
}

fn owner_id(value: i64) -> V1Result<InventoryOwnerId> {
    InventoryOwnerId::new(value).map_err(domain_validation)
}

fn cart_id(value: i64) -> V1Result<PickCartId> {
    PickCartId::new(value).map_err(domain_validation)
}

fn cluster_id(value: i64) -> V1Result<PickClusterId> {
    PickClusterId::new(value).map_err(domain_validation)
}

fn domain_validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}
