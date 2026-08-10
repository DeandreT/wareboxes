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
    pub processing_status: Option<super::IntegrationOrderProcessingStatus>,
    pub processing_revision: Option<super::Revision>,
    pub processing_attempt_count: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundPayloadPreviewEncoding {
    Utf8,
    Hex,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboundIntegrationDetailResponse {
    pub receipt: InboundIntegrationReceiptResponse,
    pub processing: Option<InboundIntegrationProcessingResponse>,
    pub payload_preview: String,
    pub payload_preview_encoding: InboundPayloadPreviewEncoding,
    pub preview_bytes: i64,
    pub preview_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboundIntegrationProcessingAttemptMappingResponse {
    pub line_key: String,
    pub mapping_id: i64,
    pub mapping_revision: super::Revision,
    pub source_key: String,
    pub external_item_key: String,
    pub external_uom: String,
    pub item_id: i64,
    pub requested_uom: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboundIntegrationProcessingAttemptResponse {
    pub attempt_id: i64,
    pub attempt_number: i32,
    pub status: super::IntegrationOrderProcessingStatus,
    pub revision: super::Revision,
    pub input_payload_sha256: String,
    pub correction_id: Option<i64>,
    pub correction_reason: Option<String>,
    pub order_id: Option<i64>,
    pub order_revision: Option<super::Revision>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_by: i64,
    pub attempted_by_name: String,
    pub attempted_at: String,
    pub applied_mappings: Vec<InboundIntegrationProcessingAttemptMappingResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboundIntegrationProcessingResponse {
    pub processing_id: i64,
    pub adapter_key: String,
    pub mapping_version: i32,
    pub status: super::IntegrationOrderProcessingStatus,
    pub revision: super::Revision,
    pub attempt_count: i32,
    pub input_payload_sha256: String,
    pub latest_correction_id: Option<i64>,
    pub latest_correction_payload: Option<String>,
    pub latest_correction_payload_truncated: bool,
    pub order_id: Option<i64>,
    pub order_revision: Option<super::Revision>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_by: i64,
    pub attempted_by_name: String,
    pub attempted_at: String,
    pub processed_at: Option<String>,
    pub attempts: Vec<InboundIntegrationProcessingAttemptResponse>,
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
    pub replays: Vec<OutboxDeadLetterReplayResponse>,
    pub discard: Option<OutboxDeadLetterDiscardResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboxDeadLetterReplayResponse {
    pub replay_id: i64,
    pub previous_replay_count: i32,
    pub replay_count: i32,
    pub previous_attempts: i32,
    pub last_error: String,
    pub replayed_by: i64,
    pub replayed_by_name: String,
    pub replayed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayOutboxDeadLetterRequest {
    pub expected_replay_count: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayOutboxDeadLetterResponse {
    pub replay_id: i64,
    pub event_id: i64,
    pub event_key: String,
    pub event_type: String,
    pub previous_replay_count: i32,
    pub replay_count: i32,
    pub previous_attempts: i32,
    pub status: OutboundDeliveryStatus,
    pub replayed_by: i64,
    pub replayed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboxDeadLetterDiscardResponse {
    pub discard_id: i64,
    pub replay_count: i32,
    pub previous_attempts: i32,
    pub last_error: String,
    pub reason: String,
    pub discarded_by: i64,
    pub discarded_by_name: String,
    pub discarded_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscardOutboxDeadLetterRequest {
    pub expected_replay_count: i32,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscardOutboxDeadLetterResponse {
    pub discard_id: i64,
    pub event_id: i64,
    pub event_key: String,
    pub event_type: String,
    pub replay_count: i32,
    pub previous_attempts: i32,
    pub reason: String,
    pub status: OutboundDeliveryStatus,
    pub discarded_by: i64,
    pub discarded_at: String,
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
        assert_eq!(
            serde_json::to_string(&InboundPayloadPreviewEncoding::Hex).unwrap(),
            r#""hex""#
        );
    }

    #[test]
    fn replay_request_is_strict_and_optimistic() {
        let request: ReplayOutboxDeadLetterRequest =
            serde_json::from_str(r#"{"expected_replay_count":2}"#).unwrap();
        assert_eq!(request.expected_replay_count, 2);
        assert!(serde_json::from_str::<ReplayOutboxDeadLetterRequest>(
            r#"{"expected_replay_count":2,"force":true}"#
        )
        .is_err());
    }

    #[test]
    fn discard_request_is_strict_and_carries_operator_rationale() {
        let request: DiscardOutboxDeadLetterRequest =
            serde_json::from_str(r#"{"expected_replay_count":2,"reason":"destination retired"}"#)
                .unwrap();
        assert_eq!(request.expected_replay_count, 2);
        assert_eq!(request.reason, "destination retired");
        assert!(serde_json::from_str::<DiscardOutboxDeadLetterRequest>(
            r#"{"expected_replay_count":2,"reason":"destination retired","force":true}"#
        )
        .is_err());
    }
}
