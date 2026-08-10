use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    InboundIntegrationPage as ApiInboundPage, InboundIntegrationPageRequest,
    InboundIntegrationReceiptResponse, InboundIntegrationSort as ApiInboundSort,
    IntegrationSortDirection as ApiDirection, OpaqueCursor,
    OutboundDeliveryAttemptOutcome as ApiAttemptOutcome, OutboundDeliveryAttemptResponse,
    OutboundDeliveryStatus as ApiStatus, OutboundIntegrationDetailResponse,
    OutboundIntegrationEventResponse, OutboundIntegrationPage as ApiOutboundPage,
    OutboundIntegrationPageRequest, OutboundIntegrationSort as ApiOutboundSort,
};
use wareboxes_application::integration_monitor::{
    InboundIntegrationQuery, InboundIntegrationReceiptReadModel, InboundIntegrationSort,
    IntegrationSortDirection, OutboundDeliveryAttemptReadModel, OutboundDeliveryStatus,
    OutboundIntegrationDetailReadModel, OutboundIntegrationEventReadModel,
    OutboundIntegrationQuery, OutboundIntegrationSort,
};
use wareboxes_application::outbox::DeliveryAttemptOutcome;
use wareboxes_domain::{FacilityId, InventoryOwnerId};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::repo;
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
    }
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
}
