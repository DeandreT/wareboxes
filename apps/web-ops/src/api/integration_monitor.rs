use wareboxes_api_contract::v1::{
    DiscardOutboxDeadLetterRequest, DiscardOutboxDeadLetterResponse,
    InboundIntegrationDetailResponse, InboundIntegrationPage, InboundIntegrationSort,
    IntegrationSortDirection, OpaqueCursor, OutboundDeliveryStatus,
    OutboundIntegrationDetailResponse, OutboundIntegrationPage, OutboundIntegrationSort,
    ReplayOutboxDeadLetterRequest, ReplayOutboxDeadLetterResponse,
    ReprocessIntegrationOrderRequest, ReprocessIntegrationOrderResponse,
};

use super::{internal_get, internal_post_idempotent, ApiError};

#[derive(Clone, Default)]
pub struct InboundIntegrationFilters {
    pub query: Option<String>,
    pub source_key: Option<String>,
    pub facility_id: Option<i64>,
    pub inventory_owner_id: Option<i64>,
}

#[derive(Clone, Default)]
pub struct OutboundIntegrationFilters {
    pub query: Option<String>,
    pub event_type: Option<String>,
    pub status: Option<OutboundDeliveryStatus>,
    pub facility_id: Option<i64>,
    pub inventory_owner_id: Option<i64>,
}

pub async fn inbound_integrations(
    filters: &InboundIntegrationFilters,
    sort: InboundIntegrationSort,
    direction: IntegrationSortDirection,
    cursor: Option<&OpaqueCursor>,
) -> Result<InboundIntegrationPage, ApiError> {
    internal_get(&inbound_path(filters, sort, direction, cursor)).await
}

pub async fn inbound_integration_detail(
    receipt_id: i64,
) -> Result<InboundIntegrationDetailResponse, ApiError> {
    internal_get(&format!("/api/v1/integration-monitor/inbound/{receipt_id}")).await
}

pub fn inbound_payload_download_path(receipt_id: i64) -> String {
    format!("/api/v1/integration-monitor/inbound/{receipt_id}/payload")
}

pub async fn reprocess_inbound_order(
    receipt_id: i64,
    request: &ReprocessIntegrationOrderRequest,
    idempotency_key: &str,
) -> Result<ReprocessIntegrationOrderResponse, ApiError> {
    internal_post_idempotent(
        &format!("/api/v1/integration-monitor/inbound/{receipt_id}/reprocessings"),
        request,
        idempotency_key,
    )
    .await
}

pub async fn outbound_integrations(
    filters: &OutboundIntegrationFilters,
    sort: OutboundIntegrationSort,
    direction: IntegrationSortDirection,
    cursor: Option<&OpaqueCursor>,
) -> Result<OutboundIntegrationPage, ApiError> {
    internal_get(&outbound_path(filters, sort, direction, cursor)).await
}

pub async fn outbound_integration_detail(
    event_id: i64,
) -> Result<OutboundIntegrationDetailResponse, ApiError> {
    internal_get(&format!("/api/v1/integration-monitor/outbound/{event_id}")).await
}

pub async fn replay_outbound_dead_letter(
    event_id: i64,
    request: &ReplayOutboxDeadLetterRequest,
    idempotency_key: &str,
) -> Result<ReplayOutboxDeadLetterResponse, ApiError> {
    internal_post_idempotent(
        &format!("/api/v1/integration-monitor/outbound/{event_id}/replays"),
        request,
        idempotency_key,
    )
    .await
}

pub async fn discard_outbound_dead_letter(
    event_id: i64,
    request: &DiscardOutboxDeadLetterRequest,
    idempotency_key: &str,
) -> Result<DiscardOutboxDeadLetterResponse, ApiError> {
    internal_post_idempotent(
        &format!("/api/v1/integration-monitor/outbound/{event_id}/discards"),
        request,
        idempotency_key,
    )
    .await
}

fn inbound_path(
    filters: &InboundIntegrationFilters,
    sort: InboundIntegrationSort,
    direction: IntegrationSortDirection,
    cursor: Option<&OpaqueCursor>,
) -> String {
    let mut params = base_params(inbound_sort_value(sort), direction);
    push_text(&mut params, "query", filters.query.as_deref());
    push_text(&mut params, "source_key", filters.source_key.as_deref());
    push_id(&mut params, "facility_id", filters.facility_id);
    push_id(
        &mut params,
        "inventory_owner_id",
        filters.inventory_owner_id,
    );
    push_cursor(&mut params, cursor);
    format!("/api/v1/integration-monitor/inbound?{}", params.join("&"))
}

fn outbound_path(
    filters: &OutboundIntegrationFilters,
    sort: OutboundIntegrationSort,
    direction: IntegrationSortDirection,
    cursor: Option<&OpaqueCursor>,
) -> String {
    let mut params = base_params(outbound_sort_value(sort), direction);
    push_text(&mut params, "query", filters.query.as_deref());
    push_text(&mut params, "event_type", filters.event_type.as_deref());
    if let Some(status) = filters.status {
        params.push(format!("status={}", status_value(status)));
    }
    push_id(&mut params, "facility_id", filters.facility_id);
    push_id(
        &mut params,
        "inventory_owner_id",
        filters.inventory_owner_id,
    );
    push_cursor(&mut params, cursor);
    format!("/api/v1/integration-monitor/outbound?{}", params.join("&"))
}

fn base_params(sort: &str, direction: IntegrationSortDirection) -> Vec<String> {
    vec![
        "limit=100".to_owned(),
        format!("sort={sort}"),
        format!("direction={}", direction_value(direction)),
    ]
}

fn push_text(params: &mut Vec<String>, key: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        params.push(format!("{key}={}", urlencoding::encode(value)));
    }
}

fn push_id(params: &mut Vec<String>, key: &str, value: Option<i64>) {
    if let Some(value) = value {
        params.push(format!("{key}={value}"));
    }
}

fn push_cursor(params: &mut Vec<String>, cursor: Option<&OpaqueCursor>) {
    if let Some(cursor) = cursor {
        params.push(format!("cursor={}", urlencoding::encode(cursor.as_str())));
    }
}

const fn direction_value(value: IntegrationSortDirection) -> &'static str {
    match value {
        IntegrationSortDirection::Ascending => "ascending",
        IntegrationSortDirection::Descending => "descending",
    }
}

const fn inbound_sort_value(value: InboundIntegrationSort) -> &'static str {
    match value {
        InboundIntegrationSort::ReceivedAt => "received_at",
        InboundIntegrationSort::Source => "source",
        InboundIntegrationSort::PayloadSize => "payload_size",
    }
}

const fn outbound_sort_value(value: OutboundIntegrationSort) -> &'static str {
    match value {
        OutboundIntegrationSort::CreatedAt => "created_at",
        OutboundIntegrationSort::EventType => "event_type",
        OutboundIntegrationSort::Status => "status",
        OutboundIntegrationSort::Attempts => "attempts",
    }
}

const fn status_value(value: OutboundDeliveryStatus) -> &'static str {
    match value {
        OutboundDeliveryStatus::Pending => "pending",
        OutboundDeliveryStatus::Claimed => "claimed",
        OutboundDeliveryStatus::RetryScheduled => "retry_scheduled",
        OutboundDeliveryStatus::DeadLettered => "dead_lettered",
        OutboundDeliveryStatus::Published => "published",
        OutboundDeliveryStatus::Discarded => "discarded",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbound_path_binds_server_sort_filters_and_cursor() {
        let cursor = OpaqueCursor::new("imo1.filter.0000000000000064").unwrap();
        let path = outbound_path(
            &OutboundIntegrationFilters {
                query: Some("shipping event".to_owned()),
                status: Some(OutboundDeliveryStatus::DeadLettered),
                facility_id: Some(4),
                inventory_owner_id: Some(8),
                ..Default::default()
            },
            OutboundIntegrationSort::Attempts,
            IntegrationSortDirection::Descending,
            Some(&cursor),
        );
        assert_eq!(
            path,
            "/api/v1/integration-monitor/outbound?limit=100&sort=attempts&direction=descending&query=shipping%20event&status=dead_lettered&facility_id=4&inventory_owner_id=8&cursor=imo1.filter.0000000000000064"
        );
    }

    #[test]
    fn replay_path_is_event_scoped() {
        assert_eq!(
            format!("/api/v1/integration-monitor/outbound/{}/replays", 44),
            "/api/v1/integration-monitor/outbound/44/replays"
        );
    }

    #[test]
    fn discard_path_is_event_scoped() {
        assert_eq!(
            format!("/api/v1/integration-monitor/outbound/{}/discards", 44),
            "/api/v1/integration-monitor/outbound/44/discards"
        );
    }

    #[test]
    fn inbound_payload_download_is_receipt_scoped() {
        assert_eq!(
            inbound_payload_download_path(37),
            "/api/v1/integration-monitor/inbound/37/payload"
        );
    }

    #[test]
    fn inbound_reprocessing_is_receipt_scoped() {
        assert_eq!(
            format!("/api/v1/integration-monitor/inbound/{}/reprocessings", 37),
            "/api/v1/integration-monitor/inbound/37/reprocessings"
        );
    }
}
