use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{CursorPage, OpaqueCursor, PageLimit};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationSortDirection {
    Ascending,
    #[default]
    Descending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InboundIntegrationSort {
    #[default]
    ReceivedAt,
    Source,
    PayloadSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OutboundIntegrationSort {
    #[default]
    CreatedAt,
    EventType,
    Status,
    Attempts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundDeliveryStatus {
    Pending,
    Claimed,
    RetryScheduled,
    DeadLettered,
    Published,
    Discarded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct InboundIntegrationPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default)]
    pub sort: InboundIntegrationSort,
    #[serde(default)]
    pub direction: IntegrationSortDirection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboundIntegrationReceiptResponse {
    pub id: i64,
    pub inventory_owner_id: Option<i64>,
    pub inventory_owner_name: Option<String>,
    pub facility_id: Option<i64>,
    pub facility_name: Option<String>,
    pub received_at: String,
    pub source_key: String,
    pub deduplication_key: String,
    pub content_type: String,
    pub payload_bytes: i64,
    pub payload_sha256: String,
    pub request_id: Option<String>,
}

pub type InboundIntegrationPage = CursorPage<InboundIntegrationReceiptResponse>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct OutboundIntegrationPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<OutboundDeliveryStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default)]
    pub sort: OutboundIntegrationSort,
    #[serde(default)]
    pub direction: IntegrationSortDirection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundIntegrationEventResponse {
    pub id: i64,
    pub inventory_owner_id: Option<i64>,
    pub inventory_owner_name: Option<String>,
    pub facility_id: Option<i64>,
    pub facility_name: Option<String>,
    pub created_at: String,
    pub occurred_at: String,
    pub available_at: String,
    pub event_key: String,
    pub event_type: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub aggregate_sequence: i64,
    pub schema_version: i32,
    pub status: OutboundDeliveryStatus,
    pub attempts: i32,
    pub replay_count: i32,
    pub claimed_by: Option<String>,
    pub lease_expires_at: Option<String>,
    pub last_error: Option<String>,
    pub published_at: Option<String>,
    pub dead_lettered_at: Option<String>,
    pub discarded_at: Option<String>,
}

pub type OutboundIntegrationPage = CursorPage<OutboundIntegrationEventResponse>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundDeliveryAttemptOutcome {
    Published,
    RetryScheduled,
    PermanentFailure,
    RetryExhausted,
    LeaseLost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundDeliveryAttemptResponse {
    pub claim_version: i64,
    pub replay_count: i32,
    pub attempt_number: i32,
    pub worker_id: String,
    pub publisher_name: String,
    pub claimed_at: String,
    pub lease_expires_at: String,
    pub outcome: Option<OutboundDeliveryAttemptOutcome>,
    pub completed_at: Option<String>,
    pub error: Option<String>,
    pub retry_after_seconds: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundIntegrationDetailResponse {
    pub event: OutboundIntegrationEventResponse,
    pub payload: Value,
    pub attempts: Vec<OutboundDeliveryAttemptResponse>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_reject_unknown_fields() {
        let error = serde_json::from_str::<OutboundIntegrationPageRequest>(
            r#"{"query":"shipping","unsupported":true}"#,
        )
        .expect_err("unknown fields must fail");
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn status_and_sort_values_are_stable() {
        assert_eq!(
            serde_json::to_string(&OutboundDeliveryStatus::DeadLettered).unwrap(),
            r#""dead_lettered""#
        );
        assert_eq!(
            serde_json::to_string(&InboundIntegrationSort::PayloadSize).unwrap(),
            r#""payload_size""#
        );
    }
}
