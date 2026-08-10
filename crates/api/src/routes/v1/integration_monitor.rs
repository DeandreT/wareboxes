use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::Response;
use axum::Json;
use wareboxes_api_contract::v1::{
    DiscardOutboxDeadLetterRequest, DiscardOutboxDeadLetterResponse,
    InboundIntegrationDetailResponse, InboundIntegrationPage as ApiInboundPage,
    InboundIntegrationPageRequest, InboundIntegrationProcessingAttemptMappingResponse,
    InboundIntegrationProcessingAttemptResponse, InboundIntegrationProcessingResponse,
    InboundIntegrationReceiptResponse, InboundIntegrationSort as ApiInboundSort,
    InboundPayloadPreviewEncoding as ApiInboundPayloadPreviewEncoding,
    IntegrationOrderProcessingStatus, IntegrationSortDirection as ApiDirection, OpaqueCursor,
    OutboundDeliveryAttemptOutcome as ApiAttemptOutcome, OutboundDeliveryAttemptResponse,
    OutboundDeliveryStatus as ApiStatus, OutboundIntegrationDetailResponse,
    OutboundIntegrationEventResponse, OutboundIntegrationPage as ApiOutboundPage,
    OutboundIntegrationPageRequest, OutboundIntegrationSort as ApiOutboundSort,
    OutboxDeadLetterDiscardResponse, OutboxDeadLetterReplayResponse, ReplayOutboxDeadLetterRequest,
    ReplayOutboxDeadLetterResponse,
};
use wareboxes_application::integration_monitor::{
    DiscardOutboxDeadLetterCommand, DiscardOutboxDeadLetterResult,
    InboundIntegrationProcessingReadModel, InboundIntegrationQuery,
    InboundIntegrationReceiptReadModel, InboundIntegrationSort, InboundPayloadPreviewEncoding,
    IntegrationSortDirection, OutboundDeliveryAttemptReadModel, OutboundDeliveryStatus,
    OutboundIntegrationDetailReadModel, OutboundIntegrationEventReadModel,
    OutboundIntegrationQuery, OutboundIntegrationSort, ReplayOutboxDeadLetterCommand,
    ReplayOutboxDeadLetterResult,
};
use wareboxes_application::outbox::DeliveryAttemptOutcome;
use wareboxes_domain::{
    FacilityId, IntegrationInboxProcessingStatus, InventoryOwnerId, OutboxDeadLetterDiscardReason,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "admin";
const INBOUND_CURSOR_PREFIX: &str = "imi1.";
const OUTBOUND_CURSOR_PREFIX: &str = "imo1.";
const MAX_FILTER_CHARACTERS: usize = 200;

pub async fn inbound(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<InboundIntegrationPageRequest>,
) -> V1Result<Json<ApiInboundPage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let query = inbound_query(&user, &request)?;
    let page = repo::integration_monitor::inbound_page(&state.db, &user.tenant, &query).await?;
    let next_cursor = page
        .next_offset
        .map(|offset| encode_cursor(INBOUND_CURSOR_PREFIX, &inbound_filter_key(&request), offset))
        .transpose()?;
    Ok(Json(ApiInboundPage::new(
        page.items.into_iter().map(map_inbound).collect(),
        next_cursor,
    )))
}

pub async fn inbound_detail(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(receipt_id): Path<i64>,
) -> V1Result<Json<InboundIntegrationDetailResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let detail = repo::integration_monitor::inbound_detail(&state.db, &user.tenant, receipt_id)
        .await?
        .ok_or_else(|| AppError::not_found("inbound integration receipt"))?;
    Ok(Json(InboundIntegrationDetailResponse {
        receipt: map_inbound(detail.receipt),
        processing: detail.processing.map(map_inbound_processing).transpose()?,
        payload_preview: detail.payload_preview,
        payload_preview_encoding: match detail.payload_preview_encoding {
            InboundPayloadPreviewEncoding::Utf8 => ApiInboundPayloadPreviewEncoding::Utf8,
            InboundPayloadPreviewEncoding::Hex => ApiInboundPayloadPreviewEncoding::Hex,
        },
        preview_bytes: detail.preview_bytes,
        preview_truncated: detail.preview_truncated,
    }))
}

pub async fn download_inbound_payload(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(receipt_id): Path<i64>,
) -> V1Result<Response> {
    user.require_permission(&state.db, PERMISSION).await?;
    let payload = repo::integration_monitor::inbound_payload(&state.db, &user.tenant, receipt_id)
        .await?
        .ok_or_else(|| AppError::not_found("inbound integration receipt"))?;
    let content_type = HeaderValue::from_str(&payload.content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    let content_length = HeaderValue::from_str(&payload.payload.len().to_string())
        .map_err(|error| AppError::internal(error.to_string()))?;
    let disposition = HeaderValue::from_str(&format!(
        "attachment; filename=\"inbound-receipt-{receipt_id}.bin\""
    ))
    .map_err(|error| AppError::internal(error.to_string()))?;
    let mut response = Response::new(Body::from(payload.payload));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, content_type);
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, content_length);
    response
        .headers_mut()
        .insert(header::CONTENT_DISPOSITION, disposition);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

pub async fn outbound(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<OutboundIntegrationPageRequest>,
) -> V1Result<Json<ApiOutboundPage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let query = outbound_query(&user, &request)?;
    let page = repo::integration_monitor::outbound_page(&state.db, &user.tenant, &query).await?;
    let next_cursor = page
        .next_offset
        .map(|offset| {
            encode_cursor(
                OUTBOUND_CURSOR_PREFIX,
                &outbound_filter_key(&request),
                offset,
            )
        })
        .transpose()?;
    Ok(Json(ApiOutboundPage::new(
        page.items.into_iter().map(map_outbound).collect(),
        next_cursor,
    )))
}

pub async fn outbound_detail(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(event_id): Path<i64>,
) -> V1Result<Json<OutboundIntegrationDetailResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let detail = repo::integration_monitor::outbound_detail(&state.db, &user.tenant, event_id)
        .await?
        .ok_or_else(|| AppError::not_found("outbound integration event"))?;
    Ok(Json(map_detail(detail)))
}

pub async fn replay_outbound_dead_letter(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(event_id): Path<i64>,
    Json(body): Json<ReplayOutboxDeadLetterRequest>,
) -> V1Result<Json<ReplayOutboxDeadLetterResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = ReplayOutboxDeadLetterCommand::new(event_id, body.expected_replay_count)
        .map_err(AppError::from)?;
    let context = user.command_context(&idempotency_key);
    let result =
        repo::integration_monitor::replay_dead_letter(&state.db, &user.tenant, &context, command)
            .await?;
    Ok(Json(map_replay_result(result)))
}

pub async fn discard_outbound_dead_letter(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(event_id): Path<i64>,
    Json(body): Json<DiscardOutboxDeadLetterRequest>,
) -> V1Result<Json<DiscardOutboxDeadLetterResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let reason = OutboxDeadLetterDiscardReason::new(body.reason)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let command = DiscardOutboxDeadLetterCommand::new(event_id, body.expected_replay_count, reason)
        .map_err(AppError::from)?;
    let context = user.command_context(&idempotency_key);
    let result =
        repo::integration_monitor::discard_dead_letter(&state.db, &user.tenant, &context, command)
            .await?;
    Ok(Json(map_discard_result(result)))
}

fn validated_text(value: Option<&str>, label: &str) -> AppResult<Option<String>> {
    let Some(value) = value else { return Ok(None) };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > MAX_FILTER_CHARACTERS {
        return Err(AppError::bad_request(format!(
            "{label} cannot exceed {MAX_FILTER_CHARACTERS} characters"
        )));
    }
    Ok(Some(value.to_owned()))
}

fn scoped_ids(
    user: &CurrentTenant,
    facility_id: Option<i64>,
    inventory_owner_id: Option<i64>,
) -> AppResult<(Option<FacilityId>, Option<InventoryOwnerId>)> {
    let facility_id = facility_id
        .map(|id| user.require_facility(id))
        .transpose()?;
    let inventory_owner_id = inventory_owner_id
        .map(|id| {
            user.require_inventory_owner(id)?;
            InventoryOwnerId::new(id).map_err(|error| AppError::bad_request(error.to_string()))
        })
        .transpose()?;
    Ok((facility_id, inventory_owner_id))
}

fn inbound_query(
    user: &CurrentTenant,
    request: &InboundIntegrationPageRequest,
) -> V1Result<InboundIntegrationQuery> {
    let (facility_id, inventory_owner_id) =
        scoped_ids(user, request.facility_id, request.inventory_owner_id)?;
    Ok(InboundIntegrationQuery {
        search: validated_text(request.query.as_deref(), "integration search")?,
        source_key: validated_text(request.source_key.as_deref(), "integration source key")?,
        inventory_owner_id,
        facility_id,
        sort: match request.sort {
            ApiInboundSort::ReceivedAt => InboundIntegrationSort::ReceivedAt,
            ApiInboundSort::Source => InboundIntegrationSort::Source,
            ApiInboundSort::PayloadSize => InboundIntegrationSort::PayloadSize,
        },
        direction: map_direction(request.direction),
        offset: cursor_offset(
            request.cursor.as_ref(),
            INBOUND_CURSOR_PREFIX,
            &inbound_filter_key(request),
            "inbound integration monitor",
        )?,
        limit: request.limit.get(),
    })
}

fn outbound_query(
    user: &CurrentTenant,
    request: &OutboundIntegrationPageRequest,
) -> V1Result<OutboundIntegrationQuery> {
    let (facility_id, inventory_owner_id) =
        scoped_ids(user, request.facility_id, request.inventory_owner_id)?;
    Ok(OutboundIntegrationQuery {
        search: validated_text(request.query.as_deref(), "integration search")?,
        event_type: validated_text(request.event_type.as_deref(), "event type")?,
        status: request.status.map(map_status),
        inventory_owner_id,
        facility_id,
        sort: match request.sort {
            ApiOutboundSort::CreatedAt => OutboundIntegrationSort::CreatedAt,
            ApiOutboundSort::EventType => OutboundIntegrationSort::EventType,
            ApiOutboundSort::Status => OutboundIntegrationSort::Status,
            ApiOutboundSort::Attempts => OutboundIntegrationSort::Attempts,
        },
        direction: map_direction(request.direction),
        offset: cursor_offset(
            request.cursor.as_ref(),
            OUTBOUND_CURSOR_PREFIX,
            &outbound_filter_key(request),
            "outbound integration monitor",
        )?,
        limit: request.limit.get(),
    })
}

fn map_direction(direction: ApiDirection) -> IntegrationSortDirection {
    match direction {
        ApiDirection::Ascending => IntegrationSortDirection::Ascending,
        ApiDirection::Descending => IntegrationSortDirection::Descending,
    }
}

fn map_status(status: ApiStatus) -> OutboundDeliveryStatus {
    match status {
        ApiStatus::Pending => OutboundDeliveryStatus::Pending,
        ApiStatus::Claimed => OutboundDeliveryStatus::Claimed,
        ApiStatus::RetryScheduled => OutboundDeliveryStatus::RetryScheduled,
        ApiStatus::DeadLettered => OutboundDeliveryStatus::DeadLettered,
        ApiStatus::Published => OutboundDeliveryStatus::Published,
        ApiStatus::Discarded => OutboundDeliveryStatus::Discarded,
    }
}

fn api_status(status: OutboundDeliveryStatus) -> ApiStatus {
    match status {
        OutboundDeliveryStatus::Pending => ApiStatus::Pending,
        OutboundDeliveryStatus::Claimed => ApiStatus::Claimed,
        OutboundDeliveryStatus::RetryScheduled => ApiStatus::RetryScheduled,
        OutboundDeliveryStatus::DeadLettered => ApiStatus::DeadLettered,
        OutboundDeliveryStatus::Published => ApiStatus::Published,
        OutboundDeliveryStatus::Discarded => ApiStatus::Discarded,
    }
}

fn map_inbound(value: InboundIntegrationReceiptReadModel) -> InboundIntegrationReceiptResponse {
    InboundIntegrationReceiptResponse {
        id: value.id,
        inventory_owner_id: value.inventory_owner_id.map(InventoryOwnerId::get),
        inventory_owner_name: value.inventory_owner_name,
        facility_id: value.facility_id.map(FacilityId::get),
        facility_name: value.facility_name,
        received_at: value.received_at.to_rfc3339(),
        source_key: value.source_key,
        deduplication_key: value.deduplication_key,
        content_type: value.content_type,
        payload_bytes: value.payload_bytes,
        payload_sha256: value.payload_sha256,
        request_id: value.request_id,
        processing_status: value.processing_status.map(api_processing_status),
        processing_revision: value
            .processing_revision
            .map(|revision| wareboxes_api_contract::v1::Revision::new(revision.get()))
            .transpose()
            .ok()
            .flatten(),
        processing_attempt_count: value.processing_attempt_count,
    }
}

fn api_processing_status(
    status: IntegrationInboxProcessingStatus,
) -> IntegrationOrderProcessingStatus {
    match status {
        IntegrationInboxProcessingStatus::Quarantined => {
            IntegrationOrderProcessingStatus::Quarantined
        }
        IntegrationInboxProcessingStatus::Processed => IntegrationOrderProcessingStatus::Processed,
    }
}

fn map_inbound_processing(
    value: InboundIntegrationProcessingReadModel,
) -> V1Result<InboundIntegrationProcessingResponse> {
    Ok(InboundIntegrationProcessingResponse {
        processing_id: value.processing_id.get(),
        adapter_key: value.adapter_key,
        mapping_version: value.mapping_version,
        status: api_processing_status(value.status),
        revision: wareboxes_api_contract::v1::Revision::new(value.revision.get())
            .map_err(|_| V1Error::internal("inbound processing revision is invalid"))?,
        attempt_count: value.attempt_count,
        input_payload_sha256: hex::encode(value.input_payload_sha256),
        latest_correction_id: value.latest_correction_id.map(|id| id.get()),
        latest_correction_payload: value.latest_correction_payload,
        latest_correction_payload_truncated: value.latest_correction_payload_truncated,
        order_id: value.order_id.map(|id| id.get()),
        order_revision: value
            .order_revision
            .map(|revision| wareboxes_api_contract::v1::Revision::new(revision.get()))
            .transpose()
            .map_err(|_| V1Error::internal("inbound order revision is invalid"))?,
        error_code: value.error_code,
        error_message: value.error_message,
        attempted_by: value.attempted_by.get(),
        attempted_by_name: value.attempted_by_name,
        attempted_at: value.attempted_at.to_rfc3339(),
        processed_at: value.processed_at.map(|time| time.to_rfc3339()),
        attempts: value
            .attempts
            .into_iter()
            .map(|attempt| {
                Ok(InboundIntegrationProcessingAttemptResponse {
                    attempt_id: attempt.attempt_id.get(),
                    attempt_number: attempt.attempt_number,
                    status: api_processing_status(attempt.status),
                    revision: wareboxes_api_contract::v1::Revision::new(attempt.revision.get())
                        .map_err(|_| V1Error::internal("inbound attempt revision is invalid"))?,
                    input_payload_sha256: hex::encode(attempt.input_payload_sha256),
                    correction_id: attempt.correction_id.map(|id| id.get()),
                    correction_reason: attempt.correction_reason,
                    order_id: attempt.order_id.map(|id| id.get()),
                    order_revision: attempt
                        .order_revision
                        .map(|revision| wareboxes_api_contract::v1::Revision::new(revision.get()))
                        .transpose()
                        .map_err(|_| {
                            V1Error::internal("inbound attempt order revision is invalid")
                        })?,
                    error_code: attempt.error_code,
                    error_message: attempt.error_message,
                    attempted_by: attempt.attempted_by.get(),
                    attempted_by_name: attempt.attempted_by_name,
                    attempted_at: attempt.attempted_at.to_rfc3339(),
                    applied_mappings: attempt
                        .applied_mappings
                        .into_iter()
                        .map(|mapping| {
                            Ok(InboundIntegrationProcessingAttemptMappingResponse {
                                line_key: mapping.line_key,
                                mapping_id: mapping.mapping_id.get(),
                                mapping_revision: wareboxes_api_contract::v1::Revision::new(
                                    mapping.mapping_revision.get(),
                                )
                                .map_err(|_| {
                                    V1Error::internal("applied mapping revision is invalid")
                                })?,
                                source_key: mapping.source_key,
                                external_item_key: mapping.external_item_key,
                                external_uom: mapping.external_uom,
                                item_id: mapping.item_id.get(),
                                requested_uom: mapping.requested_uom,
                            })
                        })
                        .collect::<V1Result<Vec<_>>>()?,
                })
            })
            .collect::<V1Result<Vec<_>>>()?,
    })
}

fn map_outbound(value: OutboundIntegrationEventReadModel) -> OutboundIntegrationEventResponse {
    OutboundIntegrationEventResponse {
        id: value.id,
        inventory_owner_id: value.inventory_owner_id.map(InventoryOwnerId::get),
        inventory_owner_name: value.inventory_owner_name,
        facility_id: value.facility_id.map(FacilityId::get),
        facility_name: value.facility_name,
        created_at: value.created_at.to_rfc3339(),
        occurred_at: value.occurred_at.to_rfc3339(),
        available_at: value.available_at.to_rfc3339(),
        event_key: value.event_key,
        event_type: value.event_type,
        aggregate_type: value.aggregate_type,
        aggregate_id: value.aggregate_id,
        aggregate_sequence: value.aggregate_sequence,
        schema_version: value.schema_version,
        status: api_status(value.status),
        attempts: value.attempts,
        replay_count: value.replay_count,
        claimed_by: value.claimed_by,
        lease_expires_at: value.lease_expires_at.map(|value| value.to_rfc3339()),
        last_error: value.last_error,
        published_at: value.published_at.map(|value| value.to_rfc3339()),
        dead_lettered_at: value.dead_lettered_at.map(|value| value.to_rfc3339()),
        discarded_at: value.discarded_at.map(|value| value.to_rfc3339()),
    }
}

fn map_attempt(value: OutboundDeliveryAttemptReadModel) -> OutboundDeliveryAttemptResponse {
    OutboundDeliveryAttemptResponse {
        claim_version: value.claim_version,
        replay_count: value.replay_count,
        attempt_number: value.attempt_number,
        worker_id: value.worker_id,
        publisher_name: value.publisher_name,
        claimed_at: value.claimed_at.to_rfc3339(),
        lease_expires_at: value.lease_expires_at.to_rfc3339(),
        outcome: value.outcome.map(|outcome| match outcome {
            DeliveryAttemptOutcome::Published => ApiAttemptOutcome::Published,
            DeliveryAttemptOutcome::RetryScheduled => ApiAttemptOutcome::RetryScheduled,
            DeliveryAttemptOutcome::PermanentFailure => ApiAttemptOutcome::PermanentFailure,
            DeliveryAttemptOutcome::RetryExhausted => ApiAttemptOutcome::RetryExhausted,
            DeliveryAttemptOutcome::LeaseLost => ApiAttemptOutcome::LeaseLost,
        }),
        completed_at: value.completed_at.map(|value| value.to_rfc3339()),
        error: value.error,
        retry_after_seconds: value.retry_after_seconds,
    }
}

fn map_detail(value: OutboundIntegrationDetailReadModel) -> OutboundIntegrationDetailResponse {
    OutboundIntegrationDetailResponse {
        event: map_outbound(value.event),
        payload: value.payload,
        attempts: value.attempts.into_iter().map(map_attempt).collect(),
        replays: value
            .replays
            .into_iter()
            .map(|replay| OutboxDeadLetterReplayResponse {
                replay_id: replay.replay_id.get(),
                previous_replay_count: replay.previous_replay_count,
                replay_count: replay.replay_count,
                previous_attempts: replay.previous_attempts,
                last_error: replay.last_error,
                replayed_by: replay.replayed_by.get(),
                replayed_by_name: replay.replayed_by_name,
                replayed_at: replay.replayed_at.to_rfc3339(),
            })
            .collect(),
        discard: value
            .discard
            .map(|discard| OutboxDeadLetterDiscardResponse {
                discard_id: discard.discard_id.get(),
                replay_count: discard.replay_count,
                previous_attempts: discard.previous_attempts,
                last_error: discard.last_error,
                reason: discard.reason.as_str().to_owned(),
                discarded_by: discard.discarded_by.get(),
                discarded_by_name: discard.discarded_by_name,
                discarded_at: discard.discarded_at.to_rfc3339(),
            }),
    }
}

fn map_replay_result(value: ReplayOutboxDeadLetterResult) -> ReplayOutboxDeadLetterResponse {
    ReplayOutboxDeadLetterResponse {
        replay_id: value.replay_id.get(),
        event_id: value.event_id,
        event_key: value.event_key,
        event_type: value.event_type,
        previous_replay_count: value.previous_replay_count,
        replay_count: value.replay_count,
        previous_attempts: value.previous_attempts,
        status: api_status(value.status),
        replayed_by: value.replayed_by.get(),
        replayed_at: value.replayed_at.to_rfc3339(),
    }
}

fn map_discard_result(value: DiscardOutboxDeadLetterResult) -> DiscardOutboxDeadLetterResponse {
    DiscardOutboxDeadLetterResponse {
        discard_id: value.discard_id.get(),
        event_id: value.event_id,
        event_key: value.event_key,
        event_type: value.event_type,
        replay_count: value.replay_count,
        previous_attempts: value.previous_attempts,
        reason: value.reason.as_str().to_owned(),
        status: api_status(value.status),
        discarded_by: value.discarded_by.get(),
        discarded_at: value.discarded_at.to_rfc3339(),
    }
}

fn cursor_offset(
    cursor: Option<&OpaqueCursor>,
    prefix: &str,
    expected_filter: &str,
    resource: &'static str,
) -> V1Result<u64> {
    let Some(cursor) = cursor else { return Ok(0) };
    let encoded = cursor
        .as_str()
        .strip_prefix(prefix)
        .ok_or_else(|| V1Error::invalid_cursor_for(resource))?;
    let (filter, offset) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for(resource))?;
    if filter != expected_filter || offset.len() != 16 {
        return Err(V1Error::invalid_cursor_for(resource));
    }
    u64::from_str_radix(offset, 16).map_err(|_| V1Error::invalid_cursor_for(resource))
}

fn encode_cursor(prefix: &str, filter: &str, offset: u64) -> AppResult<OpaqueCursor> {
    OpaqueCursor::new(format!("{prefix}{filter}.{offset:016x}"))
        .map_err(|_| AppError::internal("generated an invalid integration monitor cursor"))
}

fn text_key(value: Option<&str>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| hex::encode(value.trim()))
}

fn inbound_filter_key(request: &InboundIntegrationPageRequest) -> String {
    format!(
        "q{}-s{}-o{}-f{}-k{}-d{}",
        text_key(request.query.as_deref()),
        text_key(request.source_key.as_deref()),
        request.inventory_owner_id.unwrap_or(0),
        request.facility_id.unwrap_or(0),
        match request.sort {
            ApiInboundSort::ReceivedAt => "received",
            ApiInboundSort::Source => "source",
            ApiInboundSort::PayloadSize => "size",
        },
        match request.direction {
            ApiDirection::Ascending => "asc",
            ApiDirection::Descending => "desc",
        }
    )
}

fn outbound_filter_key(request: &OutboundIntegrationPageRequest) -> String {
    format!(
        "q{}-e{}-s{}-o{}-f{}-k{}-d{}",
        text_key(request.query.as_deref()),
        text_key(request.event_type.as_deref()),
        request.status.map_or("all", |value| match value {
            ApiStatus::Pending => "pending",
            ApiStatus::Claimed => "claimed",
            ApiStatus::RetryScheduled => "retry",
            ApiStatus::DeadLettered => "dead",
            ApiStatus::Published => "published",
            ApiStatus::Discarded => "discarded",
        }),
        request.inventory_owner_id.unwrap_or(0),
        request.facility_id.unwrap_or(0),
        match request.sort {
            ApiOutboundSort::CreatedAt => "created",
            ApiOutboundSort::EventType => "event",
            ApiOutboundSort::Status => "status",
            ApiOutboundSort::Attempts => "attempts",
        },
        match request.direction {
            ApiDirection::Ascending => "asc",
            ApiDirection::Descending => "desc",
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_are_bound_to_filters_and_sorting() {
        let request = OutboundIntegrationPageRequest {
            query: Some("shipment".to_owned()),
            status: Some(ApiStatus::DeadLettered),
            sort: ApiOutboundSort::Attempts,
            ..Default::default()
        };
        let cursor =
            encode_cursor(OUTBOUND_CURSOR_PREFIX, &outbound_filter_key(&request), 100).unwrap();
        assert_eq!(
            cursor_offset(
                Some(&cursor),
                OUTBOUND_CURSOR_PREFIX,
                &outbound_filter_key(&request),
                "outbound integration monitor"
            )
            .unwrap(),
            100
        );
        let changed = OutboundIntegrationPageRequest {
            status: Some(ApiStatus::Published),
            ..request
        };
        assert!(cursor_offset(
            Some(&cursor),
            OUTBOUND_CURSOR_PREFIX,
            &outbound_filter_key(&changed),
            "outbound integration monitor"
        )
        .is_err());
    }

    #[test]
    fn blank_searches_normalize_to_no_filter() {
        assert_eq!(validated_text(Some("   "), "search").unwrap(), None);
        assert_eq!(
            validated_text(Some(" event "), "search").unwrap(),
            Some("event".into())
        );
    }

    #[test]
    fn discard_reason_limit_matches_domain_contract() {
        assert_eq!(
            wareboxes_domain::MAX_OUTBOX_DEAD_LETTER_DISCARD_REASON_LENGTH,
            1_000
        );
    }
}
