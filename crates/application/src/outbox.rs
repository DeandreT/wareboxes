use serde_json::Value;
use wareboxes_domain::{FacilityId, InventoryOwnerId, TenantId, Timestamp};

pub struct NewOutboxEvent<'a> {
    pub tenant_id: TenantId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
    pub actor_user_id: Option<i64>,
    pub event_key: &'a str,
    pub aggregate_type: &'a str,
    pub aggregate_id: &'a str,
    pub ordering_key: &'a str,
    pub aggregate_sequence: i64,
    pub event_type: &'a str,
    pub schema_version: i32,
    pub payload: &'a Value,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutboxEvent {
    pub id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
    pub actor_user_id: Option<i64>,
    pub created: Timestamp,
    pub event_key: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub ordering_key: String,
    pub aggregate_sequence: i64,
    pub event_type: String,
    pub schema_version: i32,
    pub payload: Value,
    pub occurred_at: Timestamp,
    pub available_at: Timestamp,
    pub claimed_at: Option<Timestamp>,
    pub claimed_by: Option<String>,
    pub lease_expires_at: Option<Timestamp>,
    pub claim_version: i64,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub dead_lettered_at: Option<Timestamp>,
    pub replay_count: i32,
    pub discarded_at: Option<Timestamp>,
    pub discard_reason: Option<String>,
    pub discarded_by_user_id: Option<i64>,
    pub published_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryFailureClass {
    Retryable,
    Permanent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryAttemptOutcome {
    Published,
    RetryScheduled,
    PermanentFailure,
    RetryExhausted,
    LeaseLost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAttempt {
    pub tenant_id: TenantId,
    pub outbox_event_id: i64,
    pub event_key: String,
    pub event_type: String,
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

pub struct FailOutboxEvent<'a> {
    pub tenant_id: TenantId,
    pub event_id: i64,
    pub worker_id: &'a str,
    pub claim_version: i64,
    pub failure_class: DeliveryFailureClass,
    pub error: &'a str,
    pub retry_after_seconds: i64,
    pub max_attempts: i32,
}
