use serde::{Deserialize, Serialize};
use serde_json::Value;
use wareboxes_domain::{FacilityId, InventoryOwnerId, OutboxDeadLetterReplayId, Timestamp, UserId};

use crate::outbox::DeliveryAttemptOutcome;
use crate::{ApplicationError, ApplicationResult};

pub const REPLAY_OUTBOX_DEAD_LETTER_OPERATION: &str = "integration.outbox.dead_letter.replay.v1";

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
}
