use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    CancelCrossDockWorkRequest, CancelCrossDockWorkResponse, ClaimCrossDockWorkByIdRequest,
    ClaimNextCrossDockWorkRequest, ConfirmCrossDockWorkRequest, ConfirmCrossDockWorkResponse,
    CrossDockCancellationReason as ApiCancellationReason, CrossDockClaimHeartbeatResponse,
    CrossDockClaimReleaseReason as ApiReleaseReason, CrossDockClaimReleaseResponse,
    CrossDockClaimResponse, CrossDockLocationResponse,
    CrossDockPlanningOptionPage as ApiPlanningPage, CrossDockPlanningOptionPageRequest,
    CrossDockPlanningOptionResponse, CrossDockWorkPage as ApiPage, CrossDockWorkPageRequest,
    CrossDockWorkResponse, CrossDockWorkStatus as ApiStatus, HeartbeatCrossDockClaimRequest,
    OpaqueCursor, PlanCrossDockWorkRequest, PlanCrossDockWorkResponse,
    ReleaseCrossDockClaimRequest, Revision,
};
use wareboxes_application::cross_dock::{
    CancelCrossDockWorkCommand, CancelCrossDockWorkResult, ClaimCrossDockWorkByIdCommand,
    ClaimNextCrossDockWorkCommand, ConfirmCrossDockWorkCommand, ConfirmCrossDockWorkResult,
    CrossDockClaim, CrossDockClaimHeartbeatResult, CrossDockClaimReleaseResult,
    CrossDockLocationReadModel, CrossDockPlanningOptionPageFilter,
    CrossDockPlanningOptionReadModel, CrossDockWorkPageFilter, CrossDockWorkReadModel,
    HeartbeatCrossDockClaimCommand, PlanCrossDockWorkCommand, PlanCrossDockWorkResult,
    ReleaseCrossDockClaimCommand,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    CrossDockCancellationDetails, CrossDockCancellationReason, CrossDockClaimReleaseReason,
    CrossDockNote, CrossDockQuantity, CrossDockScanValue, CrossDockWorkId, CrossDockWorkStatus,
    LocationId, OrderId, OrderLineId, OrderRevision, Timestamp,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const CURSOR_PREFIX: &str = "cd1.";
const PLANNING_CURSOR_PREFIX: &str = "cdp1.";

pub async fn plan_work(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(order_id): Path<i64>,
    Json(body): Json<PlanCrossDockWorkRequest>,
) -> V1Result<Json<PlanCrossDockWorkResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let command = PlanCrossDockWorkCommand {
        order_id: id(order_id, OrderId::new)?,
        order_line_id: id(body.order_line_id, OrderLineId::new)?,
        expected_order_revision: id(body.expected_order_revision.get(), OrderRevision::new)?,
        source_receipt_inventory_transaction_id: body.source_receipt_inventory_transaction_id,
        destination_pick_face_location_id: id(
            body.destination_pick_face_location_id,
            LocationId::new,
        )?,
        quantity: CrossDockQuantity::new(body.quantity).map_err(validation)?,
        priority: body.priority,
        assigned_user_id: body
            .assigned_user_id
            .map(|value| id(value, wareboxes_domain::UserId::new))
            .transpose()?,
        due_at: body
            .due_at
            .map(|value| parse_timestamp(&value, "due_at"))
            .transpose()?,
        instructions: body.instructions,
    };
    let result = repo::cross_dock::plan_work(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_plan(result)?))
}

pub async fn work_page(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<CrossDockWorkPageRequest>,
) -> V1Result<Json<ApiPage>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let facility_id = request
        .facility_id
        .map(|value| user.require_facility(value))
        .transpose()?;
    let owner_id = request
        .inventory_owner_id
        .map(|value| user.require_inventory_owner(value))
        .transpose()?;
    let order_id = request
        .order_id
        .map(|value| id(value, OrderId::new))
        .transpose()?;
    let offset = request
        .cursor
        .as_ref()
        .map(|cursor| decode_cursor(cursor, &request))
        .transpose()?
        .unwrap_or(0);
    let page = repo::cross_dock::work_page(
        &state.db,
        &user.tenant,
        CrossDockWorkPageFilter {
            facility_id,
            inventory_owner_id: owner_id,
            order_id,
            status: request.status.map(status_to_domain),
            offset,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = page
        .next_offset
        .map(|offset| encode_cursor(offset, &request))
        .transpose()?;
    Ok(Json(ApiPage::new(
        page.items
            .into_iter()
            .map(map_work)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor,
    )))
}

pub async fn planning_options(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<CrossDockPlanningOptionPageRequest>,
) -> V1Result<Json<ApiPlanningPage>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let facility_id = request
        .facility_id
        .map(|value| user.require_facility(value))
        .transpose()?;
    let inventory_owner_id = request
        .inventory_owner_id
        .map(|value| user.require_inventory_owner(value))
        .transpose()?;
    let offset = request
        .cursor
        .as_ref()
        .map(|cursor| decode_planning_cursor(cursor, &request))
        .transpose()?
        .unwrap_or(0);
    let page = repo::cross_dock::planning_option_page(
        &state.db,
        &user.tenant,
        CrossDockPlanningOptionPageFilter {
            facility_id,
            inventory_owner_id,
            offset,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = page
        .next_offset
        .map(|next| encode_planning_cursor(next, &request))
        .transpose()?;
    Ok(Json(ApiPlanningPage::new(
        page.items
            .into_iter()
            .map(map_planning_option)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor,
    )))
}

#[cfg_attr(not(feature = "ssr"), allow(dead_code))]
pub(crate) async fn pages_for_access(
    state: &AppState,
    access: &TenantAccess,
    limit: u16,
) -> AppResult<(ApiPlanningPage, ApiPage)> {
    let (planning, work) = tokio::try_join!(
        repo::cross_dock::planning_option_page(
            &state.db,
            access,
            CrossDockPlanningOptionPageFilter {
                facility_id: None,
                inventory_owner_id: None,
                offset: 0,
                limit,
            },
        ),
        repo::cross_dock::work_page(
            &state.db,
            access,
            CrossDockWorkPageFilter {
                facility_id: None,
                inventory_owner_id: None,
                order_id: None,
                status: None,
                offset: 0,
                limit,
            },
        ),
    )?;
    let page_limit = wareboxes_api_contract::v1::PageLimit::new(limit)
        .map_err(|_| AppError::bad_request("cross-dock bootstrap limit must be 1 through 100"))?;
    let planning_request = CrossDockPlanningOptionPageRequest {
        limit: page_limit,
        ..Default::default()
    };
    let work_request = CrossDockWorkPageRequest {
        limit: page_limit,
        ..Default::default()
    };
    let planning_next = planning
        .next_offset
        .map(|offset| encode_planning_cursor(offset, &planning_request))
        .transpose()
        .map_err(|_| AppError::internal("could not encode cross-dock planning cursor"))?;
    let work_next = work
        .next_offset
        .map(|offset| encode_cursor(offset, &work_request))
        .transpose()
        .map_err(|_| AppError::internal("could not encode cross-dock work cursor"))?;
    let planning_items = planning
        .items
        .into_iter()
        .map(map_planning_option)
        .collect::<V1Result<Vec<_>>>()
        .map_err(|_| AppError::internal("could not map cross-dock planning bootstrap"))?;
    let work_items = work
        .items
        .into_iter()
        .map(map_work)
        .collect::<V1Result<Vec<_>>>()
        .map_err(|_| AppError::internal("could not map cross-dock work bootstrap"))?;
    Ok((
        ApiPlanningPage::new(planning_items, planning_next),
        ApiPage::new(work_items, work_next),
    ))
}

pub async fn claim_next(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(_): Json<ClaimNextCrossDockWorkRequest>,
) -> V1Result<Json<Option<CrossDockClaimResponse>>> {
    user.require_permission(&state.db, "wms").await?;
    let result = repo::cross_dock::claim_next(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        ClaimNextCrossDockWorkCommand,
    )
    .await?;
    Ok(Json(result.map(map_claim).transpose()?))
}

pub async fn claim_by_id(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(work_id): Path<i64>,
    Json(_): Json<ClaimCrossDockWorkByIdRequest>,
) -> V1Result<Json<CrossDockClaimResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let command = ClaimCrossDockWorkByIdCommand {
        work_id: work_id_value(work_id)?,
    };
    Ok(Json(map_claim(
        repo::cross_dock::claim_by_id(
            &state.db,
            &user.tenant,
            &user.command_context(&idempotency_key),
            command,
        )
        .await?,
    )?))
}

pub async fn current_claim(
    State(state): State<AppState>,
    user: CurrentTenant,
) -> V1Result<Json<Option<CrossDockClaimResponse>>> {
    user.require_permission(&state.db, "wms").await?;
    Ok(Json(
        repo::cross_dock::current_claim(&state.db, &user.tenant)
            .await?
            .map(map_claim)
            .transpose()?,
    ))
}

pub async fn heartbeat_claim(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(work_id): Path<i64>,
    Json(_): Json<HeartbeatCrossDockClaimRequest>,
) -> V1Result<Json<CrossDockClaimHeartbeatResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let result = repo::cross_dock::heartbeat_claim(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        HeartbeatCrossDockClaimCommand {
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
    Json(body): Json<ReleaseCrossDockClaimRequest>,
) -> V1Result<Json<CrossDockClaimReleaseResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let command = ReleaseCrossDockClaimCommand {
        work_id: work_id_value(work_id)?,
        reason: release_to_domain(body.reason),
        note: body.note,
    };
    Ok(Json(map_release(
        repo::cross_dock::release_claim(
            &state.db,
            &user.tenant,
            &user.command_context(&idempotency_key),
            &command,
        )
        .await?,
    )))
}

pub async fn confirm_work(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(work_id): Path<i64>,
    Json(body): Json<ConfirmCrossDockWorkRequest>,
) -> V1Result<Json<ConfirmCrossDockWorkResponse>> {
    user.require_permission(&state.db, "wms").await?;
    let command = ConfirmCrossDockWorkCommand {
        work_id: work_id_value(work_id)?,
        source_receiving_location_barcode: scan(body.source_receiving_location_barcode)?,
        item_barcode: scan(body.item_barcode)?,
        lot_scan: body.lot_scan.map(scan).transpose()?,
        serial_scan: body.serial_scan.map(scan).transpose()?,
        destination_pick_face_barcode: scan(body.destination_pick_face_barcode)?,
    };
    Ok(Json(map_confirmation(
        repo::cross_dock::confirm_work(
            &state.db,
            &user.tenant,
            &user.command_context(&idempotency_key),
            &command,
        )
        .await?,
    )?))
}

pub async fn cancel_work(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(work_id): Path<i64>,
    Json(body): Json<CancelCrossDockWorkRequest>,
) -> V1Result<Json<CancelCrossDockWorkResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let details = CrossDockCancellationDetails::new(
        cancellation_to_domain(body.reason),
        body.note
            .map(CrossDockNote::new)
            .transpose()
            .map_err(validation)?,
    )
    .map_err(validation)?;
    let command = CancelCrossDockWorkCommand {
        work_id: work_id_value(work_id)?,
        expected_order_revision: id(body.expected_order_revision.get(), OrderRevision::new)?,
        details,
    };
    Ok(Json(map_cancellation(
        repo::cross_dock::cancel_work(
            &state.db,
            &user.tenant,
            &user.command_context(&idempotency_key),
            &command,
        )
        .await?,
    )?))
}

fn map_plan(v: PlanCrossDockWorkResult) -> V1Result<PlanCrossDockWorkResponse> {
    Ok(PlanCrossDockWorkResponse {
        plan_id: v.plan_id.get(),
        work_id: v.work_id.get(),
        order_id: v.order_id.get(),
        order_line_id: v.order_line_id.get(),
        reservation_id: v.reservation_id,
        previous_order_revision: revision(v.previous_order_revision.get())?,
        order_revision: revision(v.order_revision.get())?,
        inventory_owner_id: v.inventory_owner_id.get(),
        facility_id: v.facility_id.get(),
        inbound_load_id: v.inbound_load_id.get(),
        source_receipt_inventory_transaction_id: v.source_receipt_inventory_transaction_id,
        source_inventory_balance_id: v.source_inventory_balance_id.get(),
        source_location_id: v.source_location_id.get(),
        destination_pick_face_location_id: v.destination_pick_face_location_id.get(),
        item_batch_id: v.item_batch_id.get(),
        item_id: v.item_id.get(),
        uom: v.uom.as_str().into(),
        lot: v.lot,
        serial: v.serial,
        expiration: v.expiration.map(|v| v.to_rfc3339()),
        quantity: v.quantity.get(),
        remaining_unallocated_quantity: v.remaining_unallocated_quantity,
        status: status_to_api(v.status),
        planned_by: v.planned_by.get(),
        planned_at: v.planned_at.to_rfc3339(),
    })
}
fn map_location(v: CrossDockLocationReadModel) -> CrossDockLocationResponse {
    CrossDockLocationResponse {
        location_id: v.location_id.get(),
        barcode: v.barcode.as_str().into(),
        name: v.name,
    }
}
fn map_claim(v: CrossDockClaim) -> V1Result<CrossDockClaimResponse> {
    Ok(CrossDockClaimResponse {
        work_id: v.work_id.get(),
        plan_id: v.plan_id.get(),
        inventory_owner_id: v.inventory_owner_id.get(),
        facility_id: v.facility_id.get(),
        order_id: v.order_id.get(),
        order_key: v.order_key,
        order_line_id: v.order_line_id.get(),
        order_line_key: v.order_line_key,
        reservation_id: v.reservation_id,
        priority: v.priority,
        instructions: v.instructions,
        due_at: v.due_at.map(|v| v.to_rfc3339()),
        lease_expires_at: v.lease_expires_at.to_rfc3339(),
        source_receipt_inventory_transaction_id: v.source_receipt_inventory_transaction_id,
        source_inventory_balance_id: v.source_inventory_balance_id.get(),
        item_batch_id: v.item_batch_id.get(),
        item_id: v.item_id.get(),
        item_description: v.item_description,
        item_barcodes: v
            .item_barcodes
            .into_iter()
            .map(|v| v.as_str().into())
            .collect(),
        uom: v.uom.as_str().into(),
        lot: v.lot,
        serial: v.serial,
        expiration: v.expiration.map(|v| v.to_rfc3339()),
        quantity: v.quantity.get(),
        source_receiving_location: map_location(v.source_receiving_location),
        destination_pick_face: map_location(v.destination_pick_face),
    })
}
fn map_work(v: CrossDockWorkReadModel) -> V1Result<CrossDockWorkResponse> {
    Ok(CrossDockWorkResponse {
        work_id: v.work_id.get(),
        plan_id: v.plan_id.get(),
        status: status_to_api(v.status),
        inventory_owner_id: v.inventory_owner_id.get(),
        inventory_owner_name: v.inventory_owner_name,
        facility_id: v.facility_id.get(),
        facility_name: v.facility_name,
        inbound_load_id: v.inbound_load_id.get(),
        order_id: v.order_id.get(),
        order_key: v.order_key,
        order_revision: revision(v.order_revision.get())?,
        order_line_id: v.order_line_id.get(),
        order_line_key: v.order_line_key,
        reservation_id: v.reservation_id,
        priority: v.priority,
        item_id: v.item_id.get(),
        item_description: v.item_description,
        primary_sku: v.primary_sku,
        uom: v.uom.as_str().into(),
        lot: v.lot,
        serial: v.serial,
        expiration: v.expiration.map(|v| v.to_rfc3339()),
        quantity: v.quantity.get(),
        source_inventory_balance_id: v.source_inventory_balance_id.get(),
        source_receiving_location: map_location(v.source_receiving_location),
        destination_pick_face: map_location(v.destination_pick_face),
        claimed_by: v.claimed_by.map(|v| v.get()),
        lease_expires_at: v.lease_expires_at.map(|v| v.to_rfc3339()),
        due_at: v.due_at.map(|v| v.to_rfc3339()),
        created_at: v.created_at.to_rfc3339(),
        completed_at: v.completed_at.map(|v| v.to_rfc3339()),
    })
}
fn map_planning_option(
    v: CrossDockPlanningOptionReadModel,
) -> V1Result<CrossDockPlanningOptionResponse> {
    Ok(CrossDockPlanningOptionResponse {
        order_id: v.order_id.get(),
        order_key: v.order_key,
        order_line_id: v.order_line_id.get(),
        order_line_key: v.order_line_key,
        order_revision: revision(v.order_revision.get())?,
        inventory_owner_id: v.inventory_owner_id.get(),
        inventory_owner_name: v.inventory_owner_name,
        facility_id: v.facility_id.get(),
        facility_name: v.facility_name,
        reservation_id: v.reservation_id,
        item_id: v.item_id.get(),
        item_description: v.item_description,
        primary_sku: v.primary_sku,
        uom: v.uom.as_str().into(),
        lot: v.lot,
        serial: v.serial,
        expiration: v.expiration.map(|value| value.to_rfc3339()),
        unallocated_quantity: v.unallocated_quantity,
        source_receipt_inventory_transaction_id: v.source_receipt_inventory_transaction_id,
        inbound_load_id: v.inbound_load_id.get(),
        inbound_load_reference: v.inbound_load_reference,
        source_inventory_balance_id: v.source_inventory_balance_id.get(),
        source_receiving_location: map_location(v.source_receiving_location),
        source_free_quantity: v.source_free_quantity,
        receipt_remaining_quantity: v.receipt_remaining_quantity,
        maximum_plan_quantity: v.maximum_plan_quantity,
        destination_pick_faces: v
            .destination_pick_faces
            .into_iter()
            .map(map_location)
            .collect(),
    })
}
fn map_heartbeat(v: CrossDockClaimHeartbeatResult) -> CrossDockClaimHeartbeatResponse {
    CrossDockClaimHeartbeatResponse {
        work_id: v.work_id.get(),
        heartbeat_at: v.heartbeat_at.to_rfc3339(),
        lease_expires_at: v.lease_expires_at.to_rfc3339(),
    }
}
fn map_release(v: CrossDockClaimReleaseResult) -> CrossDockClaimReleaseResponse {
    CrossDockClaimReleaseResponse {
        work_id: v.work_id.get(),
        status: status_to_api(v.status),
        released_at: v.released_at.to_rfc3339(),
        release_count: v.release_count,
        reason: release_to_api(v.reason),
        note: v.note,
    }
}
fn map_confirmation(v: ConfirmCrossDockWorkResult) -> V1Result<ConfirmCrossDockWorkResponse> {
    Ok(ConfirmCrossDockWorkResponse {
        confirmation_id: v.confirmation_id.get(),
        work_id: v.work_id.get(),
        plan_id: v.plan_id.get(),
        order_id: v.order_id.get(),
        order_line_id: v.order_line_id.get(),
        reservation_id: v.reservation_id,
        inventory_transaction_id: v.inventory_transaction_id,
        inventory_allocation_id: v.inventory_allocation_id,
        source_inventory_balance_id: v.source_inventory_balance_id.get(),
        destination_inventory_balance_id: v.destination_inventory_balance_id.get(),
        source_location_id: v.source_location_id.get(),
        destination_pick_face_location_id: v.destination_pick_face_location_id.get(),
        item_batch_id: v.item_batch_id.get(),
        item_id: v.item_id.get(),
        uom: v.uom.as_str().into(),
        lot: v.lot,
        serial: v.serial,
        quantity: v.quantity.get(),
        status: status_to_api(v.work_status),
        confirmed_by: v.confirmed_by.get(),
        confirmed_at: v.confirmed_at.to_rfc3339(),
    })
}
fn map_cancellation(v: CancelCrossDockWorkResult) -> V1Result<CancelCrossDockWorkResponse> {
    Ok(CancelCrossDockWorkResponse {
        cancellation_id: v.cancellation_id.get(),
        work_id: v.work_id.get(),
        plan_id: v.plan_id.get(),
        order_id: v.order_id.get(),
        order_line_id: v.order_line_id.get(),
        previous_order_revision: revision(v.previous_order_revision.get())?,
        order_revision: revision(v.order_revision.get())?,
        quantity: v.quantity.get(),
        previous_status: status_to_api(v.previous_status),
        status: status_to_api(v.status),
        reason: cancellation_to_api(v.details.reason),
        note: v.details.note.map(|v| v.as_str().into()),
        cancelled_by: v.cancelled_by.get(),
        cancelled_at: v.cancelled_at.to_rfc3339(),
    })
}

fn status_to_domain(v: ApiStatus) -> CrossDockWorkStatus {
    match v {
        ApiStatus::Pending => CrossDockWorkStatus::Pending,
        ApiStatus::InProgress => CrossDockWorkStatus::InProgress,
        ApiStatus::Completed => CrossDockWorkStatus::Completed,
        ApiStatus::Cancelled => CrossDockWorkStatus::Cancelled,
    }
}
fn status_to_api(v: CrossDockWorkStatus) -> ApiStatus {
    match v {
        CrossDockWorkStatus::Pending => ApiStatus::Pending,
        CrossDockWorkStatus::InProgress => ApiStatus::InProgress,
        CrossDockWorkStatus::Completed => ApiStatus::Completed,
        CrossDockWorkStatus::Cancelled => ApiStatus::Cancelled,
    }
}
fn release_to_domain(v: ApiReleaseReason) -> CrossDockClaimReleaseReason {
    match v {
        ApiReleaseReason::WorkInterrupted => CrossDockClaimReleaseReason::WorkInterrupted,
        ApiReleaseReason::EndOfShift => CrossDockClaimReleaseReason::EndOfShift,
        ApiReleaseReason::EquipmentIssue => CrossDockClaimReleaseReason::EquipmentIssue,
        ApiReleaseReason::Other => CrossDockClaimReleaseReason::Other,
    }
}
fn release_to_api(v: CrossDockClaimReleaseReason) -> ApiReleaseReason {
    match v {
        CrossDockClaimReleaseReason::WorkInterrupted => ApiReleaseReason::WorkInterrupted,
        CrossDockClaimReleaseReason::EndOfShift => ApiReleaseReason::EndOfShift,
        CrossDockClaimReleaseReason::EquipmentIssue => ApiReleaseReason::EquipmentIssue,
        CrossDockClaimReleaseReason::Other => ApiReleaseReason::Other,
    }
}
fn cancellation_to_domain(v: ApiCancellationReason) -> CrossDockCancellationReason {
    match v {
        ApiCancellationReason::DemandChanged => CrossDockCancellationReason::DemandChanged,
        ApiCancellationReason::ReceiptReassigned => CrossDockCancellationReason::ReceiptReassigned,
        ApiCancellationReason::OperationalChange => CrossDockCancellationReason::OperationalChange,
        ApiCancellationReason::Other => CrossDockCancellationReason::Other,
    }
}
fn cancellation_to_api(v: CrossDockCancellationReason) -> ApiCancellationReason {
    match v {
        CrossDockCancellationReason::DemandChanged => ApiCancellationReason::DemandChanged,
        CrossDockCancellationReason::ReceiptReassigned => ApiCancellationReason::ReceiptReassigned,
        CrossDockCancellationReason::OperationalChange => ApiCancellationReason::OperationalChange,
        CrossDockCancellationReason::Other => ApiCancellationReason::Other,
    }
}
fn work_id_value(v: i64) -> V1Result<CrossDockWorkId> {
    id(v, CrossDockWorkId::new)
}
fn scan(v: String) -> V1Result<CrossDockScanValue> {
    CrossDockScanValue::new(v).map_err(validation)
}
fn revision(v: i64) -> V1Result<Revision> {
    Revision::new(v).map_err(invalid_result)
}
fn id<T, E>(v: i64, ctor: fn(i64) -> Result<T, E>) -> V1Result<T>
where
    E: std::fmt::Display,
{
    ctor(v).map_err(validation)
}
fn parse_timestamp(v: &str, field: &str) -> V1Result<Timestamp> {
    v.parse::<Timestamp>()
        .map_err(|e| AppError::bad_request(format!("{field} is invalid: {e}")).into())
}
fn validation(e: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(e.to_string()).into()
}
fn invalid_result(e: impl std::fmt::Display) -> V1Error {
    V1Error::internal(e.to_string())
}
fn cursor_filter(v: &CrossDockWorkPageRequest) -> String {
    format!(
        "{}.{}.{}.{}",
        v.facility_id
            .map_or_else(|| "-".into(), |v| format!("{v:x}")),
        v.inventory_owner_id
            .map_or_else(|| "-".into(), |v| format!("{v:x}")),
        v.order_id.map_or_else(|| "-".into(), |v| format!("{v:x}")),
        v.status.map_or("all", |v| match v {
            ApiStatus::Pending => "pending",
            ApiStatus::InProgress => "in_progress",
            ApiStatus::Completed => "completed",
            ApiStatus::Cancelled => "cancelled",
        })
    )
}
fn encode_cursor(offset: u64, v: &CrossDockWorkPageRequest) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!("{CURSOR_PREFIX}{}.{offset:016x}", cursor_filter(v)))
        .map_err(|_| V1Error::internal("generated invalid cross-dock cursor"))
}
fn decode_cursor(cursor: &OpaqueCursor, v: &CrossDockWorkPageRequest) -> V1Result<u64> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("cross-dock queue"))?;
    let (filter, offset) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for("cross-dock queue"))?;
    if filter != cursor_filter(v) || offset.len() != 16 {
        return Err(V1Error::invalid_cursor_for("cross-dock queue"));
    }
    u64::from_str_radix(offset, 16).map_err(|_| V1Error::invalid_cursor_for("cross-dock queue"))
}

fn planning_cursor_filter(v: &CrossDockPlanningOptionPageRequest) -> String {
    format!(
        "{}.{}",
        v.facility_id
            .map_or_else(|| "-".into(), |value| format!("{value:x}")),
        v.inventory_owner_id
            .map_or_else(|| "-".into(), |value| format!("{value:x}")),
    )
}
fn encode_planning_cursor(
    offset: u64,
    v: &CrossDockPlanningOptionPageRequest,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{PLANNING_CURSOR_PREFIX}{}.{offset:016x}",
        planning_cursor_filter(v)
    ))
    .map_err(|_| V1Error::internal("generated invalid cross-dock planning cursor"))
}
fn decode_planning_cursor(
    cursor: &OpaqueCursor,
    v: &CrossDockPlanningOptionPageRequest,
) -> V1Result<u64> {
    let encoded = cursor
        .as_str()
        .strip_prefix(PLANNING_CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("cross-dock planning options"))?;
    let (filter, offset) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for("cross-dock planning options"))?;
    if filter != planning_cursor_filter(v) || offset.len() != 16 {
        return Err(V1Error::invalid_cursor_for("cross-dock planning options"));
    }
    u64::from_str_radix(offset, 16)
        .map_err(|_| V1Error::invalid_cursor_for("cross-dock planning options"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::PageLimit;
    #[test]
    fn cursor_binds_filters() {
        let request = CrossDockWorkPageRequest {
            facility_id: Some(1),
            inventory_owner_id: None,
            order_id: Some(3),
            status: Some(ApiStatus::Pending),
            cursor: None,
            limit: PageLimit::default(),
        };
        let cursor = encode_cursor(100, &request).unwrap();
        assert_eq!(decode_cursor(&cursor, &request).unwrap(), 100);
        let mut changed = request;
        changed.status = Some(ApiStatus::Completed);
        assert!(decode_cursor(&cursor, &changed).is_err());
    }
}
