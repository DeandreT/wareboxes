use serde::{Deserialize, Serialize};

use super::InventoryBalanceStatus;

/// Typed putaway workflow selected when claiming the next available task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PutawayWorkflow {
    Loose,
    LicensePlate,
}

/// Claims the next available task for one putaway workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimNextPutawayRequest {
    pub workflow: PutawayWorkflow,
}

/// Empty command body used to claim a specific putaway task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ClaimPutawayByIdRequest {}

/// Empty command body used to renew an active putaway claim lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatPutawayClaimRequest {}

/// Result of renewing one active putaway claim lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutawayClaimHeartbeatResponse {
    pub task_id: i64,
    pub heartbeat_at: String,
    pub lease_expires_at: String,
}

/// Operator reason for returning active putaway work to the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PutawayClaimReleaseReason {
    WorkInterrupted,
    EquipmentUnavailable,
    DestinationBlocked,
    SafetyIssue,
    Other,
}

/// Command used by an operator to release an active putaway claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasePutawayClaimRequest {
    pub reason: PutawayClaimReleaseReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Result of returning one active putaway claim to the work queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutawayClaimReleaseResponse {
    pub task_id: i64,
    pub released_at: String,
    pub release_count: i64,
    pub reason: PutawayClaimReleaseReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Source location presented to the putaway operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutawayClaimSourceLocation {
    pub location_id: i64,
    pub barcode: Option<String>,
    pub name: Option<String>,
}

/// Scannable destination location for the claimed putaway task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutawayClaimDestinationLocation {
    pub location_id: i64,
    pub barcode: String,
    pub name: Option<String>,
}

/// Workflow-specific inventory the operator must put away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "workflow", rename_all = "snake_case", deny_unknown_fields)]
pub enum PutawayClaimWork {
    Loose {
        source_inventory_balance_id: i64,
        item_batch_id: i64,
        item_id: i64,
        item_description: Option<String>,
        uom: String,
        lot: Option<String>,
        serial: Option<String>,
        expiration: Option<String>,
        inventory_status: InventoryBalanceStatus,
        quantity: i64,
    },
    LicensePlate {
        license_plate_id: i64,
        license_plate_barcode: String,
        planned_balance_count: i64,
    },
}

/// Active typed putaway claim without persistence or tenant metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutawayClaimResponse {
    pub task_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub priority: i64,
    pub instructions: Option<String>,
    pub due_at: Option<String>,
    pub lease_expires_at: String,
    pub source_location: PutawayClaimSourceLocation,
    pub destination_location: PutawayClaimDestinationLocation,
    pub work: PutawayClaimWork,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_requests_reject_unknown_fields() {
        assert!(serde_json::from_str::<ClaimNextPutawayRequest>(
            r#"{"workflow":"loose","tenant_id":1}"#
        )
        .is_err());
        assert!(serde_json::from_str::<ClaimPutawayByIdRequest>(r#"{"task_id":1}"#).is_err());
        assert_eq!(
            serde_json::from_str::<ClaimPutawayByIdRequest>("{}").unwrap(),
            ClaimPutawayByIdRequest {}
        );
        assert_eq!(
            serde_json::from_str::<HeartbeatPutawayClaimRequest>("{}").unwrap(),
            HeartbeatPutawayClaimRequest {}
        );
        assert!(serde_json::from_str::<HeartbeatPutawayClaimRequest>(
            r#"{"lease_expires_at":"client-controlled"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<ReleasePutawayClaimRequest>("{}").is_err());
        assert_eq!(
            serde_json::from_str::<ReleasePutawayClaimRequest>(
                r#"{"reason":"destination_blocked","note":"Lane is obstructed"}"#
            )
            .unwrap(),
            ReleasePutawayClaimRequest {
                reason: PutawayClaimReleaseReason::DestinationBlocked,
                note: Some("Lane is obstructed".into()),
            }
        );
        assert!(
            serde_json::from_str::<ReleasePutawayClaimRequest>(r#"{"assigned_user_id":1}"#)
                .is_err()
        );
    }

    #[test]
    fn claim_response_is_typed_and_excludes_internal_metadata() {
        let response = PutawayClaimResponse {
            task_id: 11,
            inventory_owner_id: 22,
            facility_id: 33,
            priority: 80,
            instructions: Some("Scan the directed location".into()),
            due_at: Some("2026-07-27T01:00:00+00:00".into()),
            lease_expires_at: "2026-07-27T00:30:00+00:00".into(),
            source_location: PutawayClaimSourceLocation {
                location_id: 44,
                barcode: Some("RECEIVING-01".into()),
                name: Some("Receiving".into()),
            },
            destination_location: PutawayClaimDestinationLocation {
                location_id: 55,
                barcode: "A-01-01".into(),
                name: Some("A-01-01".into()),
            },
            work: PutawayClaimWork::Loose {
                source_inventory_balance_id: 66,
                item_batch_id: 77,
                item_id: 88,
                item_description: Some("Case-picked item".into()),
                uom: "case".into(),
                lot: Some("LOT-01".into()),
                serial: None,
                expiration: None,
                inventory_status: InventoryBalanceStatus::Available,
                quantity: 4,
            },
        };

        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["work"]["workflow"], "loose");
        assert_eq!(value["destination_location"]["barcode"], "A-01-01");
        assert!(value.get("tenant_id").is_none());
        assert!(value.get("status").is_none());
        assert!(value.get("required_permission").is_none());
        assert!(value.get("assigned_user_id").is_none());
    }

    #[test]
    fn license_plate_claim_does_not_publish_a_mixed_uom_total() {
        let work = PutawayClaimWork::LicensePlate {
            license_plate_id: 91,
            license_plate_barcode: "LP-91".into(),
            planned_balance_count: 3,
        };

        let value = serde_json::to_value(work).unwrap();
        assert_eq!(value["workflow"], "license_plate");
        assert_eq!(value["planned_balance_count"], 3);
        assert!(value.get("total_quantity").is_none());
    }

    #[test]
    fn lifecycle_responses_publish_only_operator_facing_state() {
        let heartbeat = serde_json::to_value(PutawayClaimHeartbeatResponse {
            task_id: 91,
            heartbeat_at: "2026-07-27T00:05:00+00:00".into(),
            lease_expires_at: "2026-07-27T00:35:00+00:00".into(),
        })
        .unwrap();
        assert_eq!(
            heartbeat,
            serde_json::json!({
                "task_id": 91,
                "heartbeat_at": "2026-07-27T00:05:00+00:00",
                "lease_expires_at": "2026-07-27T00:35:00+00:00",
            })
        );
        let release = serde_json::to_value(PutawayClaimReleaseResponse {
            task_id: 91,
            released_at: "2026-07-27T00:06:00+00:00".into(),
            release_count: 2,
            reason: PutawayClaimReleaseReason::EquipmentUnavailable,
            note: Some("Forklift needs service".into()),
        })
        .unwrap();
        assert_eq!(
            release,
            serde_json::json!({
                "task_id": 91,
                "released_at": "2026-07-27T00:06:00+00:00",
                "release_count": 2,
                "reason": "equipment_unavailable",
                "note": "Forklift needs service",
            })
        );
        assert!(heartbeat.get("tenant_id").is_none());
        assert!(heartbeat.get("previous_lease_expires_at").is_none());
        assert!(release.get("inventory_owner_id").is_none());
        assert!(release.get("assigned_user_id").is_none());
    }
}
