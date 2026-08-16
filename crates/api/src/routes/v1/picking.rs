use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    ClaimNextPickRequest, ClaimPickByIdRequest, ConfirmPickContentRequest, CurrentPickResponse,
    HeartbeatPickClaimRequest, PickClaimContent as ApiPickClaimContent, PickClaimHeartbeatResponse,
    PickClaimReleaseReason as ApiReleaseReason, PickClaimReleaseResponse, PickClaimResponse,
    PickConfirmationHistoryPage as ApiConfirmationHistoryPage, PickConfirmationHistoryPageRequest,
    PickConfirmationHistoryResponse, PickContentConfirmationResponse,
    PickContentState as ApiContentState,
    PickDecisionPolicyResponse as ApiPickDecisionPolicyResponse,
    PickDecisionPolicySource as ApiPickDecisionPolicySource,
    PickExecutionMethod as ApiPickExecutionMethod, PickExecutionResponse, PickOrderStatus,
    PickReversalHistoryResponse, PickReversalReason as ApiReversalReason, ReleasePickClaimRequest,
    ReversePickConfirmationRequest, ReversePickConfirmationResponse, Revision,
};
use wareboxes_application::picking::{
    ClaimNextPickCommand, ClaimPickByIdCommand, ConfirmPickContentCommand,
    ConfirmPickContentResult, HeartbeatPickClaimCommand, PickClaim, PickClaimContent,
    PickClaimHeartbeatResult, PickClaimReleaseResult, PickConfirmationHistoryCursor,
    PickConfirmationHistoryPage, PickConfirmationHistoryQuery, PickConfirmationHistoryReadModel,
    ReleasePickClaimCommand, ReversePickConfirmationCommand, ReversePickConfirmationResult,
};
use wareboxes_application::picking_decision_policy::{
    PickDecisionPolicyReadModel, PickDecisionPolicySource,
};
use wareboxes_domain::{
    ConfigurationScope, OrderStatus, PickClaimReleaseReason, PickConfirmationId, PickContentId,
    PickContentState, PickExecutionMethod, PickReversalNote, PickReversalReason, PickScanValue,
    PickTaskId, Timestamp, MAX_PICK_SCAN_VALUE_LENGTH,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const MAX_RELEASE_NOTE_LENGTH: usize = 500;
const CONFIRMATION_CURSOR_PREFIX: &str = "pc1.";

pub async fn claim_next(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(_body): Json<ClaimNextPickRequest>,
) -> V1Result<Json<CurrentPickResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let context = user.command_context(&idempotency_key);
    let claim =
        repo::picking::claim_next(&state.db, &user.tenant, &context, ClaimNextPickCommand).await?;
    Ok(Json(claim.map(map_claim).transpose()?))
}

pub async fn claim_by_id(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(_body): Json<ClaimPickByIdRequest>,
) -> V1Result<Json<PickClaimResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = ClaimPickByIdCommand {
        task_id: pick_task_id(task_id)?,
    };
    let context = user.command_context(&idempotency_key);
    let claim = repo::picking::claim_by_id(&state.db, &user.tenant, &context, command).await?;
    Ok(Json(map_claim(claim)?))
}

pub async fn current(
    State(state): State<AppState>,
    user: CurrentTenant,
) -> V1Result<Json<CurrentPickResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let claim = repo::picking::current(&state.db, &user.tenant).await?;
    Ok(Json(claim.map(map_claim).transpose()?))
}

pub async fn heartbeat(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(_body): Json<HeartbeatPickClaimRequest>,
) -> V1Result<Json<PickClaimHeartbeatResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = HeartbeatPickClaimCommand {
        task_id: pick_task_id(task_id)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::picking::heartbeat(&state.db, &user.tenant, &context, command).await?;
    Ok(Json(map_heartbeat(result)))
}

pub async fn release(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(body): Json<ReleasePickClaimRequest>,
) -> V1Result<Json<PickClaimReleaseResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    validate_release(&body)?;
    let command = ReleasePickClaimCommand {
        task_id: pick_task_id(task_id)?,
        reason: map_release_reason(body.reason),
        note: body.note,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::picking::release_claim(&state.db, &user.tenant, &context, command).await?;
    Ok(Json(map_release(result)))
}

pub async fn confirm(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path((task_id, content_id)): Path<(i64, i64)>,
    Json(body): Json<ConfirmPickContentRequest>,
) -> V1Result<Json<PickContentConfirmationResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = ConfirmPickContentCommand {
        task_id: pick_task_id(task_id)?,
        content_id: PickContentId::new(content_id).map_err(domain_validation)?,
        source_location_barcode: body
            .source_location_barcode
            .map(|value| scan(value, "source location barcode"))
            .transpose()?,
        item_barcode: body
            .item_barcode
            .map(|value| scan(value, "item barcode"))
            .transpose()?,
        source_license_plate_barcode: body
            .source_license_plate_barcode
            .map(|value| scan(value, "source license plate barcode"))
            .transpose()?,
        destination_license_plate_barcode: body
            .destination_license_plate_barcode
            .map(|value| scan(value, "destination license plate barcode"))
            .transpose()?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::picking::confirm_content(&state.db, &user.tenant, &context, command).await?;
    Ok(Json(map_confirmation(result)?))
}

pub async fn reverse_confirmation(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(confirmation_id): Path<i64>,
    Json(body): Json<ReversePickConfirmationRequest>,
) -> V1Result<Json<ReversePickConfirmationResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let command = ReversePickConfirmationCommand {
        confirmation_id: PickConfirmationId::new(confirmation_id).map_err(domain_validation)?,
        expected_order_revision: wareboxes_domain::OrderRevision::new(
            body.expected_order_revision.get(),
        )
        .map_err(domain_validation)?,
        staged_location_barcode: scan(body.staged_location_barcode, "staged location barcode")?,
        staged_license_plate_barcode: scan(
            body.staged_license_plate_barcode,
            "staged license plate barcode",
        )?,
        item_barcode: scan(body.item_barcode, "item barcode")?,
        lot_scan: body
            .lot_scan
            .map(|value| scan(value, "lot scan"))
            .transpose()?,
        serial_scan: body
            .serial_scan
            .map(|value| scan(value, "serial scan"))
            .transpose()?,
        return_location_barcode: scan(body.return_location_barcode, "return location barcode")?,
        return_license_plate_barcode: body
            .return_license_plate_barcode
            .map(|value| scan(value, "return license plate barcode"))
            .transpose()?,
        reason: map_reversal_reason(body.reason),
        note: body
            .note
            .map(PickReversalNote::new)
            .transpose()
            .map_err(domain_validation)?,
    };
    command.validate_details().map_err(domain_validation)?;
    let context = user.command_context(&idempotency_key);
    let result =
        repo::picking::reverse_confirmation(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_reversal(result)?))
}

pub async fn list_confirmation_history(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(order_id): Path<i64>,
    Query(request): Query<PickConfirmationHistoryPageRequest>,
) -> V1Result<Json<ApiConfirmationHistoryPage>> {
    user.require_permission(&state.db, "orders").await?;
    if request.limit.get() > 100 {
        return Err(invalid(
            "pick confirmation history limit must be between 1 and 100",
        ));
    }
    let order_id = wareboxes_domain::OrderId::new(order_id).map_err(domain_validation)?;
    let query = PickConfirmationHistoryQuery {
        order_id,
        cursor: request
            .cursor
            .as_ref()
            .map(|cursor| decode_confirmation_cursor(cursor, order_id))
            .transpose()?,
        limit: request.limit.get(),
    };
    let page = repo::picking::list_confirmation_history(&state.db, &user.tenant, query).await?;
    Ok(Json(map_confirmation_history_page(page, order_id)?))
}

pub(crate) fn map_claim(claim: PickClaim) -> V1Result<PickClaimResponse> {
    Ok(PickClaimResponse {
        task_id: claim.task_id.get(),
        order_id: claim.order_id.get(),
        inventory_owner_id: claim.inventory_owner_id.get(),
        facility_id: claim.facility_id.get(),
        order_key: claim.order_key,
        order_revision: Revision::new(claim.order_revision.get())
            .map_err(|error| V1Error::internal(error.to_string()))?,
        priority: claim.priority,
        ship_by: claim.ship_by.map(|value| value.to_rfc3339()),
        lease_expires_at: claim.lease_expires_at.to_rfc3339(),
        destination_location_id: claim.destination_location_id.get(),
        destination_location_barcode: claim.destination_location_barcode.into_inner(),
        destination_location_name: claim.destination_location_name,
        execution: PickExecutionResponse {
            method: match claim.execution.method {
                PickExecutionMethod::Discrete => ApiPickExecutionMethod::Discrete,
                PickExecutionMethod::Case => ApiPickExecutionMethod::Case,
                PickExecutionMethod::Pallet => ApiPickExecutionMethod::Pallet,
                PickExecutionMethod::ClusterCart => ApiPickExecutionMethod::ClusterCart,
                PickExecutionMethod::BatchCart => ApiPickExecutionMethod::BatchCart,
            },
            cluster_id: claim.execution.cluster_id.map(|id| id.get()),
            cart_barcode: claim.execution.cart_barcode,
            slot_code: claim.execution.slot_code,
            sequence: claim.execution.sequence,
            task_count: claim.execution.task_count,
            batch_total_quantity: claim.execution.batch_total_quantity,
        },
        pick_policy: map_pick_policy(claim.pick_policy),
        suggested_destination_license_plate_barcode: claim
            .suggested_destination_license_plate_barcode
            .map(PickScanValue::into_inner),
        content: map_content(claim.content),
    })
}

fn map_content(content: PickClaimContent) -> ApiPickClaimContent {
    ApiPickClaimContent {
        content_id: content.content_id.get(),
        order_line_id: content.order_line_id.get(),
        inventory_allocation_id: content.inventory_allocation_id.get(),
        source_inventory_balance_id: content.source_inventory_balance_id.get(),
        item_batch_id: content.item_batch_id.get(),
        source_location_id: content.source_location_id.get(),
        source_location_barcode: content.source_location_barcode.into_inner(),
        source_location_name: content.source_location_name,
        source_license_plate_id: content.source_license_plate_id.map(|id| id.get()),
        source_license_plate_barcode: content
            .source_license_plate_barcode
            .map(PickScanValue::into_inner),
        item_id: content.item_id,
        item_description: content.item_description,
        item_barcodes: content
            .item_barcodes
            .into_iter()
            .map(PickScanValue::into_inner)
            .collect(),
        uom: content.uom,
        lot: content.lot,
        serial: content.serial,
        expiration: content.expiration.map(|value| value.to_rfc3339()),
        planned_quantity: content.planned_quantity.get(),
        state: map_content_state(content.state),
    }
}

fn map_heartbeat(result: PickClaimHeartbeatResult) -> PickClaimHeartbeatResponse {
    PickClaimHeartbeatResponse {
        task_id: result.task_id.get(),
        heartbeat_at: result.heartbeat_at.to_rfc3339(),
        lease_expires_at: result.lease_expires_at.to_rfc3339(),
    }
}

fn map_release(result: PickClaimReleaseResult) -> PickClaimReleaseResponse {
    PickClaimReleaseResponse {
        task_id: result.task_id.get(),
        released_at: result.released_at.to_rfc3339(),
        release_count: result.release_count,
        reason: map_release_reason_to_api(result.reason),
        note: result.note,
    }
}

fn map_confirmation(result: ConfirmPickContentResult) -> V1Result<PickContentConfirmationResponse> {
    let order_status = match result.order_status {
        OrderStatus::Processing => PickOrderStatus::Processing,
        OrderStatus::AwaitingPacking => PickOrderStatus::AwaitingPacking,
        _ => {
            return Err(V1Error::internal(
                "pick confirmation produced an invalid order status",
            ))
        }
    };
    Ok(PickContentConfirmationResponse {
        result_id: result.result_id,
        content_id: result.content_id.get(),
        task_id: result.task_id.get(),
        order_id: result.order_id.get(),
        inventory_transaction_id: result.inventory_transaction_id,
        source_inventory_allocation_id: result.source_inventory_allocation_id.get(),
        destination_inventory_allocation_id: result.destination_inventory_allocation_id.get(),
        source_inventory_balance_id: result.source_inventory_balance_id.get(),
        destination_inventory_balance_id: result.destination_inventory_balance_id.get(),
        source_location_id: result.source_location_id.get(),
        destination_location_id: result.destination_location_id.get(),
        source_license_plate_id: result.source_license_plate_id.map(|id| id.get()),
        destination_license_plate_id: result.destination_license_plate_id.get(),
        pick_policy: map_pick_policy(result.pick_policy),
        source_location_scan_verified: result.source_location_scan_verified,
        item_scan_verified: result.item_scan_verified,
        destination_container_scan_verified: result.destination_container_scan_verified,
        picked_quantity: result.picked_quantity.get(),
        confirmed_by: result.confirmed_by.get(),
        confirmed_at: result.confirmed_at.to_rfc3339(),
        content_state: map_content_state(result.content_state),
        task_completed: result.task_completed,
        order_ready_to_pack: result.order_ready_to_pack,
        order_status,
        order_revision: Revision::new(result.order_revision.get())
            .map_err(|error| V1Error::internal(error.to_string()))?,
    })
}

fn map_pick_policy(value: PickDecisionPolicyReadModel) -> ApiPickDecisionPolicyResponse {
    ApiPickDecisionPolicyResponse {
        source: match value.source {
            PickDecisionPolicySource::ProductDefault => ApiPickDecisionPolicySource::ProductDefault,
            PickDecisionPolicySource::Configuration => ApiPickDecisionPolicySource::Configuration,
        },
        configuration_id: value.configuration_id.map(|id| id.get()),
        configuration_revision: value.configuration_revision,
        configuration_scope: value.configuration_scope.map(|scope| match scope {
            ConfigurationScope::Tenant => wareboxes_api_contract::v1::ConfigurationScope::Tenant,
            ConfigurationScope::InventoryOwner { inventory_owner_id } => {
                wareboxes_api_contract::v1::ConfigurationScope::InventoryOwner {
                    inventory_owner_id: inventory_owner_id.get(),
                }
            }
            ConfigurationScope::Facility { facility_id } => {
                wareboxes_api_contract::v1::ConfigurationScope::Facility {
                    facility_id: facility_id.get(),
                }
            }
            ConfigurationScope::OwnerFacility {
                inventory_owner_id,
                facility_id,
            } => wareboxes_api_contract::v1::ConfigurationScope::OwnerFacility {
                inventory_owner_id: inventory_owner_id.get(),
                facility_id: facility_id.get(),
            },
        }),
        require_source_location_scan: value.require_source_location_scan,
        require_item_scan: value.require_item_scan,
        require_destination_container_scan: value.require_destination_container_scan,
        policy_hash: value.policy_hash,
    }
}

fn map_reversal(
    result: ReversePickConfirmationResult,
) -> V1Result<ReversePickConfirmationResponse> {
    if result.order_status != OrderStatus::Processing {
        return Err(V1Error::internal(
            "pick reversal produced an invalid order status",
        ));
    }
    Ok(ReversePickConfirmationResponse {
        reversal_id: result.reversal_id.get(),
        confirmation_id: result.confirmation_id.get(),
        task_id: result.task_id.get(),
        content_id: result.content_id.get(),
        order_id: result.order_id.get(),
        inventory_transaction_id: result.inventory_transaction_id,
        source_inventory_allocation_id: result.source_inventory_allocation_id.get(),
        staged_inventory_allocation_id: result.staged_inventory_allocation_id.get(),
        source_inventory_balance_id: result.source_inventory_balance_id.get(),
        staged_inventory_balance_id: result.staged_inventory_balance_id.get(),
        source_location_id: result.source_location_id.get(),
        staged_location_id: result.staged_location_id.get(),
        source_license_plate_id: result.source_license_plate_id.map(|id| id.get()),
        staged_license_plate_id: result.staged_license_plate_id.get(),
        reversed_quantity: result.reversed_quantity.get(),
        content_state: map_content_state(result.content_state),
        order_status: PickOrderStatus::Processing,
        order_revision: Revision::new(result.order_revision.get())
            .map_err(|error| V1Error::internal(error.to_string()))?,
        reason: map_reversal_reason_to_api(result.reason),
        note: result.note.map(|note| note.as_str().to_owned()),
        reversed_by: result.reversed_by.get(),
        reversed_at: result.reversed_at.to_rfc3339(),
    })
}

fn map_confirmation_history_page(
    page: PickConfirmationHistoryPage,
    order_id: wareboxes_domain::OrderId,
) -> V1Result<ApiConfirmationHistoryPage> {
    let next_cursor = page
        .next_cursor
        .map(|cursor| encode_confirmation_cursor(cursor, order_id))
        .transpose()?;
    Ok(ApiConfirmationHistoryPage::new(
        page.items
            .into_iter()
            .map(map_confirmation_history)
            .collect(),
        next_cursor,
    ))
}

fn map_confirmation_history(
    item: PickConfirmationHistoryReadModel,
) -> PickConfirmationHistoryResponse {
    PickConfirmationHistoryResponse {
        confirmation_id: item.confirmation_id.get(),
        task_id: item.task_id.get(),
        content_id: item.content_id.get(),
        order_id: item.order_id.get(),
        item_id: item.item_id,
        item_description: item.item_description,
        uom: item.uom,
        lot: item.lot,
        serial: item.serial,
        picked_quantity: item.picked_quantity.get(),
        source_location_id: item.source_location_id.get(),
        source_location_name: item.source_location_name,
        source_license_plate_required: item.source_license_plate_required,
        staged_location_id: item.staged_location_id.get(),
        staged_location_name: item.staged_location_name,
        staged_license_plate_id: item.staged_license_plate_id.get(),
        pick_policy: map_pick_policy(item.pick_policy),
        source_location_scan_verified: item.source_location_scan_verified,
        item_scan_verified: item.item_scan_verified,
        destination_container_scan_verified: item.destination_container_scan_verified,
        confirmed_by: item.confirmed_by.get(),
        confirmed_at: item.confirmed_at.to_rfc3339(),
        reversal: item.reversal.map(|reversal| PickReversalHistoryResponse {
            reversal_id: reversal.reversal_id.get(),
            reason: map_reversal_reason_to_api(reversal.reason),
            note: reversal.note.map(|note| note.as_str().to_owned()),
            reversed_by: reversal.reversed_by.get(),
            reversed_at: reversal.reversed_at.to_rfc3339(),
        }),
    }
}

fn decode_confirmation_cursor(
    cursor: &wareboxes_api_contract::v1::OpaqueCursor,
    expected_order_id: wareboxes_domain::OrderId,
) -> V1Result<PickConfirmationHistoryCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CONFIRMATION_CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("pick confirmation history"))?;
    let mut parts = encoded.split('.');
    let order_id = parts
        .next()
        .filter(|part| part.len() == 16)
        .and_then(|part| i64::from_str_radix(part, 16).ok())
        .and_then(|value| wareboxes_domain::OrderId::new(value).ok())
        .filter(|value| *value == expected_order_id)
        .ok_or_else(|| V1Error::invalid_cursor_for("pick confirmation history"))?;
    let _ = order_id;
    let sortable = parts
        .next()
        .filter(|part| part.len() == 16)
        .and_then(|part| u64::from_str_radix(part, 16).ok())
        .ok_or_else(|| V1Error::invalid_cursor_for("pick confirmation history"))?;
    let confirmation_id = parts
        .next()
        .filter(|part| part.len() == 16)
        .and_then(|part| i64::from_str_radix(part, 16).ok())
        .and_then(|value| PickConfirmationId::new(value).ok())
        .ok_or_else(|| V1Error::invalid_cursor_for("pick confirmation history"))?;
    if parts.next().is_some() {
        return Err(V1Error::invalid_cursor_for("pick confirmation history"));
    }
    let micros = (sortable ^ (1_u64 << 63)) as i64;
    let confirmed_at = Timestamp::from_timestamp_micros(micros)
        .ok_or_else(|| V1Error::invalid_cursor_for("pick confirmation history"))?;
    Ok(PickConfirmationHistoryCursor {
        confirmed_at,
        confirmation_id,
    })
}

fn encode_confirmation_cursor(
    cursor: PickConfirmationHistoryCursor,
    order_id: wareboxes_domain::OrderId,
) -> V1Result<wareboxes_api_contract::v1::OpaqueCursor> {
    let sortable = (cursor.confirmed_at.timestamp_micros() as u64) ^ (1_u64 << 63);
    wareboxes_api_contract::v1::OpaqueCursor::new(format!(
        "{CONFIRMATION_CURSOR_PREFIX}{:016x}.{sortable:016x}.{:016x}",
        order_id.get(),
        cursor.confirmation_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid pick confirmation cursor"))
}

fn validate_release(body: &ReleasePickClaimRequest) -> V1Result<()> {
    if let Some(note) = body.note.as_deref() {
        if note.trim() != note || note.is_empty() {
            return Err(invalid("note must be trimmed and nonempty when provided"));
        }
        if note.chars().count() > MAX_RELEASE_NOTE_LENGTH {
            return Err(invalid(format!(
                "note cannot exceed {MAX_RELEASE_NOTE_LENGTH} characters"
            )));
        }
    }
    if body.reason == ApiReleaseReason::Other && body.note.is_none() {
        return Err(invalid("note is required when reason is other"));
    }
    Ok(())
}

fn scan(value: String, label: &str) -> V1Result<PickScanValue> {
    PickScanValue::new(value).map_err(|error| {
        invalid(format!(
            "invalid {label}: {error}; maximum length is {MAX_PICK_SCAN_VALUE_LENGTH}"
        ))
    })
}

fn pick_task_id(value: i64) -> V1Result<PickTaskId> {
    PickTaskId::new(value).map_err(domain_validation)
}

fn map_content_state(state: PickContentState) -> ApiContentState {
    match state {
        PickContentState::Pending => ApiContentState::Pending,
        PickContentState::Completed => ApiContentState::Completed,
        PickContentState::Shorted => ApiContentState::Shorted,
    }
}

fn map_release_reason(reason: ApiReleaseReason) -> PickClaimReleaseReason {
    match reason {
        ApiReleaseReason::WorkInterrupted => PickClaimReleaseReason::WorkInterrupted,
        ApiReleaseReason::EquipmentUnavailable => PickClaimReleaseReason::EquipmentUnavailable,
        ApiReleaseReason::SourceBlocked => PickClaimReleaseReason::SourceBlocked,
        ApiReleaseReason::InventoryDiscrepancy => PickClaimReleaseReason::InventoryDiscrepancy,
        ApiReleaseReason::SafetyIssue => PickClaimReleaseReason::SafetyIssue,
        ApiReleaseReason::Other => PickClaimReleaseReason::Other,
    }
}

fn map_release_reason_to_api(reason: PickClaimReleaseReason) -> ApiReleaseReason {
    match reason {
        PickClaimReleaseReason::WorkInterrupted => ApiReleaseReason::WorkInterrupted,
        PickClaimReleaseReason::EquipmentUnavailable => ApiReleaseReason::EquipmentUnavailable,
        PickClaimReleaseReason::SourceBlocked => ApiReleaseReason::SourceBlocked,
        PickClaimReleaseReason::InventoryDiscrepancy => ApiReleaseReason::InventoryDiscrepancy,
        PickClaimReleaseReason::SafetyIssue => ApiReleaseReason::SafetyIssue,
        PickClaimReleaseReason::Other => ApiReleaseReason::Other,
    }
}

fn map_reversal_reason(reason: ApiReversalReason) -> PickReversalReason {
    match reason {
        ApiReversalReason::MisPick => PickReversalReason::MisPick,
        ApiReversalReason::WrongQuantity => PickReversalReason::WrongQuantity,
        ApiReversalReason::WrongLotOrSerial => PickReversalReason::WrongLotOrSerial,
        ApiReversalReason::DamagedDuringPick => PickReversalReason::DamagedDuringPick,
        ApiReversalReason::OrderException => PickReversalReason::OrderException,
        ApiReversalReason::Other => PickReversalReason::Other,
    }
}

fn map_reversal_reason_to_api(reason: PickReversalReason) -> ApiReversalReason {
    match reason {
        PickReversalReason::MisPick => ApiReversalReason::MisPick,
        PickReversalReason::WrongQuantity => ApiReversalReason::WrongQuantity,
        PickReversalReason::WrongLotOrSerial => ApiReversalReason::WrongLotOrSerial,
        PickReversalReason::DamagedDuringPick => ApiReversalReason::DamagedDuringPick,
        PickReversalReason::OrderException => ApiReversalReason::OrderException,
        PickReversalReason::Other => ApiReversalReason::Other,
    }
}

fn domain_validation(error: impl std::fmt::Display) -> V1Error {
    invalid(error.to_string())
}

fn invalid(message: impl Into<String>) -> V1Error {
    AppError::bad_request(message).into()
}
