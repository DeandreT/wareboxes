use serde::{Deserialize, Serialize};
use serde_json::Value;
use wareboxes_domain::{
    CatalogItemId, FacilityId, IntegrationInboxCorrectionId, IntegrationInboxProcessingAttemptId,
    IntegrationInboxProcessingId, IntegrationInboxProcessingRevision,
    IntegrationInboxProcessingStatus, IntegrationOrderItemMappingId,
    IntegrationOrderItemMappingRevision, InventoryOwnerId, OrderId, OrderRevision,
    OutboxDeadLetterDiscardId, OutboxDeadLetterDiscardReason, OutboxDeadLetterReplayId, Timestamp,
    UserId,
};

use crate::outbox::DeliveryAttemptOutcome;
use crate::{ApplicationError, ApplicationResult};

pub const REPLAY_OUTBOX_DEAD_LETTER_OPERATION: &str = "integration.outbox.dead_letter.replay.v1";
pub const DISCARD_OUTBOX_DEAD_LETTER_OPERATION: &str = "integration.outbox.dead_letter.discard.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscardOutboxDeadLetterCommand {
    event_id: i64,
    expected_replay_count: i32,
    reason: OutboxDeadLetterDiscardReason,
}

impl DiscardOutboxDeadLetterCommand {
    pub fn new(
        event_id: i64,
        expected_replay_count: i32,
        reason: OutboxDeadLetterDiscardReason,
    ) -> ApplicationResult<Self> {
        if event_id <= 0 {
            return Err(ApplicationError::InvalidRequest(
                "outbox event ID must be positive".into(),
            ));
        }
        if expected_replay_count < 0 {
            return Err(ApplicationError::InvalidRequest(
                "expected replay count cannot be negative".into(),
            ));
        }
        Ok(Self {
            event_id,
            expected_replay_count,
            reason,
        })
    }

    pub const fn event_id(&self) -> i64 {
        self.event_id
    }

    pub const fn expected_replay_count(&self) -> i32 {
        self.expected_replay_count
    }

    pub const fn reason(&self) -> &OutboxDeadLetterDiscardReason {
        &self.reason
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscardOutboxDeadLetterResult {
    pub discard_id: OutboxDeadLetterDiscardId,
    pub event_id: i64,
    pub event_key: String,
    pub event_type: String,
    pub replay_count: i32,
    pub previous_attempts: i32,
    pub reason: OutboxDeadLetterDiscardReason,
    pub status: OutboundDeliveryStatus,
    pub discarded_by: UserId,
    pub discarded_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayOutboxDeadLetterCommand {
    event_id: i64,
    expected_replay_count: i32,
}

impl ReplayOutboxDeadLetterCommand {
    pub fn new(event_id: i64, expected_replay_count: i32) -> ApplicationResult<Self> {
        if event_id <= 0 {
            return Err(ApplicationError::InvalidRequest(
                "outbox event ID must be positive".into(),
            ));
        }
        if expected_replay_count < 0 {
            return Err(ApplicationError::InvalidRequest(
                "expected replay count cannot be negative".into(),
            ));
        }
        Ok(Self {
            event_id,
            expected_replay_count,
        })
    }

    pub const fn event_id(self) -> i64 {
        self.event_id
    }

    pub const fn expected_replay_count(self) -> i32 {
        self.expected_replay_count
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayOutboxDeadLetterResult {
    pub replay_id: OutboxDeadLetterReplayId,
    pub event_id: i64,
    pub event_key: String,
    pub event_type: String,
    pub previous_replay_count: i32,
    pub replay_count: i32,
    pub previous_attempts: i32,
    pub status: OutboundDeliveryStatus,
    pub replayed_by: UserId,
    pub replayed_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationSortDirection {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundIntegrationSort {
    ReceivedAt,
    Source,
    PayloadSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundIntegrationSort {
    CreatedAt,
    EventType,
    Status,
    Attempts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutboundDeliveryStatus {
    Pending,
    Claimed,
    RetryScheduled,
    DeadLettered,
    Published,
    Discarded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxDeadLetterReplayReadModel {
    pub replay_id: OutboxDeadLetterReplayId,
    pub previous_replay_count: i32,
    pub replay_count: i32,
    pub previous_attempts: i32,
    pub last_error: String,
    pub replayed_by: UserId,
    pub replayed_by_name: String,
    pub replayed_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxDeadLetterDiscardReadModel {
    pub discard_id: OutboxDeadLetterDiscardId,
    pub replay_count: i32,
    pub previous_attempts: i32,
    pub last_error: String,
    pub reason: OutboxDeadLetterDiscardReason,
    pub discarded_by: UserId,
    pub discarded_by_name: String,
    pub discarded_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundIntegrationQuery {
    pub search: Option<String>,
    pub source_key: Option<String>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
    pub sort: InboundIntegrationSort,
    pub direction: IntegrationSortDirection,
    pub offset: u64,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundIntegrationReceiptReadModel {
    pub id: i64,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub inventory_owner_name: Option<String>,
    pub facility_id: Option<FacilityId>,
    pub facility_name: Option<String>,
    pub received_at: Timestamp,
    pub source_key: String,
    pub deduplication_key: String,
    pub content_type: String,
    pub payload_bytes: i64,
    pub payload_sha256: String,
    pub request_id: Option<String>,
    pub processing_status: Option<IntegrationInboxProcessingStatus>,
    pub processing_revision: Option<IntegrationInboxProcessingRevision>,
    pub processing_attempt_count: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundPayloadPreviewEncoding {
    Utf8,
    Hex,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundIntegrationDetailReadModel {
    pub receipt: InboundIntegrationReceiptReadModel,
    pub processing: Option<InboundIntegrationProcessingReadModel>,
    pub payload_preview: String,
    pub payload_preview_encoding: InboundPayloadPreviewEncoding,
    pub preview_bytes: i64,
    pub preview_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundIntegrationProcessingAttemptMappingReadModel {
    pub line_key: String,
    pub mapping_id: IntegrationOrderItemMappingId,
    pub mapping_revision: IntegrationOrderItemMappingRevision,
    pub source_key: String,
    pub external_item_key: String,
    pub external_uom: String,
    pub item_id: CatalogItemId,
    pub requested_uom: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundIntegrationProcessingAttemptReadModel {
    pub attempt_id: IntegrationInboxProcessingAttemptId,
    pub attempt_number: i32,
    pub status: IntegrationInboxProcessingStatus,
    pub revision: IntegrationInboxProcessingRevision,
    pub input_payload_sha256: [u8; 32],
    pub correction_id: Option<IntegrationInboxCorrectionId>,
    pub correction_reason: Option<String>,
    pub order_id: Option<OrderId>,
    pub order_revision: Option<OrderRevision>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_by: UserId,
    pub attempted_by_name: String,
    pub attempted_at: Timestamp,
    pub applied_mappings: Vec<InboundIntegrationProcessingAttemptMappingReadModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundIntegrationProcessingReadModel {
    pub processing_id: IntegrationInboxProcessingId,
    pub adapter_key: String,
    pub mapping_version: i32,
    pub status: IntegrationInboxProcessingStatus,
    pub revision: IntegrationInboxProcessingRevision,
    pub attempt_count: i32,
    pub input_payload_sha256: [u8; 32],
    pub latest_correction_id: Option<IntegrationInboxCorrectionId>,
    pub latest_correction_payload: Option<String>,
    pub latest_correction_payload_truncated: bool,
    pub order_id: Option<OrderId>,
    pub order_revision: Option<OrderRevision>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_by: UserId,
    pub attempted_by_name: String,
    pub attempted_at: Timestamp,
    pub processed_at: Option<Timestamp>,
    pub attempts: Vec<InboundIntegrationProcessingAttemptReadModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundIntegrationPayloadReadModel {
    pub content_type: String,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundIntegrationPage {
    pub items: Vec<InboundIntegrationReceiptReadModel>,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundIntegrationQuery {
    pub search: Option<String>,
    pub event_type: Option<String>,
    pub status: Option<OutboundDeliveryStatus>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
    pub sort: OutboundIntegrationSort,
    pub direction: IntegrationSortDirection,
    pub offset: u64,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundIntegrationEventReadModel {
    pub id: i64,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub inventory_owner_name: Option<String>,
    pub facility_id: Option<FacilityId>,
    pub facility_name: Option<String>,
    pub created_at: Timestamp,
    pub occurred_at: Timestamp,
    pub available_at: Timestamp,
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
    pub lease_expires_at: Option<Timestamp>,
    pub last_error: Option<String>,
    pub published_at: Option<Timestamp>,
    pub dead_lettered_at: Option<Timestamp>,
    pub discarded_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundIntegrationPage {
    pub items: Vec<OutboundIntegrationEventReadModel>,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundDeliveryAttemptReadModel {
    pub claim_version: i64,
    pub replay_count: i32,
    pub attempt_number: i32,
    pub worker_id: String,
    pub publisher_name: String,
    pub claimed_at: Timestamp,
    pub lease_expires_at: Timestamp,
    pub outcome: Option<DeliveryAttemptOutcome>,
    pub completed_at: Option<Timestamp>,
    pub error: Option<String>,
    pub retry_after_seconds: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutboundIntegrationDetailReadModel {
    pub event: OutboundIntegrationEventReadModel,
    pub payload: Value,
    pub attempts: Vec<OutboundDeliveryAttemptReadModel>,
    pub replays: Vec<OutboxDeadLetterReplayReadModel>,
    pub discard: Option<OutboxDeadLetterDiscardReadModel>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_command_requires_positive_identity_and_nonnegative_generation() {
        assert!(ReplayOutboxDeadLetterCommand::new(41, 0).is_ok());
        assert!(ReplayOutboxDeadLetterCommand::new(0, 0).is_err());
        assert!(ReplayOutboxDeadLetterCommand::new(41, -1).is_err());
    }

    #[test]
    fn discard_command_requires_positive_identity_and_nonnegative_generation() {
        let reason = OutboxDeadLetterDiscardReason::new("destination retired").unwrap();
        assert!(DiscardOutboxDeadLetterCommand::new(41, 0, reason.clone()).is_ok());
        assert!(DiscardOutboxDeadLetterCommand::new(0, 0, reason.clone()).is_err());
        assert!(DiscardOutboxDeadLetterCommand::new(41, -1, reason).is_err());
    }
}
