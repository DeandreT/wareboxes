use serde::{Deserialize, Serialize};

use super::InventoryBalanceStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryRelocationWorkflow {
    LooseBalance,
    LicensePlate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "workflow", rename_all = "snake_case", deny_unknown_fields)]
pub enum InventoryRelocationWorkRequest {
    LooseBalance {
        source_inventory_balance_id: i64,
        quantity: i64,
    },
    LicensePlate {
        license_plate_id: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInventoryRelocationTaskRequest {
    pub work: InventoryRelocationWorkRequest,
    pub destination_location_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_user_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_for: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInventoryRelocationTaskResponse {
    pub task_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimNextInventoryRelocationRequest {
    pub workflow: InventoryRelocationWorkflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ClaimInventoryRelocationByIdRequest {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatInventoryRelocationClaimRequest {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryRelocationClaimReleaseReason {
    WorkInterrupted,
    EquipmentUnavailable,
    DestinationBlocked,
    SafetyIssue,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseInventoryRelocationClaimRequest {
    pub reason: InventoryRelocationClaimReleaseReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryRelocationClaimHeartbeatResponse {
    pub task_id: i64,
    pub heartbeat_at: String,
    pub lease_expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryRelocationClaimReleaseResponse {
    pub task_id: i64,
    pub released_at: String,
    pub release_count: i64,
    pub reason: InventoryRelocationClaimReleaseReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryRelocationLocation {
    pub location_id: i64,
    pub barcode: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "workflow", rename_all = "snake_case", deny_unknown_fields)]
pub enum InventoryRelocationClaimWork {
    LooseBalance {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryRelocationClaimResponse {
    pub task_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub priority: i64,
    pub instructions: Option<String>,
    pub due_at: Option<String>,
    pub lease_expires_at: String,
    pub source_location: InventoryRelocationLocation,
    pub destination_location: InventoryRelocationLocation,
    pub work: InventoryRelocationClaimWork,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmInventoryRelocationRequest {
    pub destination_location_barcode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_plate_barcode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "workflow", rename_all = "snake_case", deny_unknown_fields)]
pub enum InventoryRelocationResult {
    LooseBalance {
        source_inventory_balance_id: i64,
        destination_inventory_balance_id: i64,
        item_batch_id: i64,
        item_id: i64,
        inventory_status: InventoryBalanceStatus,
        uom: String,
        quantity: i64,
    },
    LicensePlate {
        license_plate_id: i64,
        license_plate_barcode: String,
        moved_balance_count: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryRelocationConfirmationResponse {
    pub task_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub source_location_id: i64,
    pub destination_location_id: i64,
    pub destination_location_barcode: String,
    pub inventory_transaction_id: i64,
    pub confirmed_by: i64,
    pub confirmed_at: String,
    pub result: InventoryRelocationResult,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_requests_are_typed_and_reject_mixed_shapes() {
        let loose = serde_json::from_str::<InventoryRelocationWorkRequest>(
            r#"{"workflow":"loose_balance","source_inventory_balance_id":4,"quantity":2}"#,
        )
        .unwrap();
        assert!(matches!(
            loose,
            InventoryRelocationWorkRequest::LooseBalance {
                source_inventory_balance_id: 4,
                quantity: 2
            }
        ));
        assert!(serde_json::from_str::<InventoryRelocationWorkRequest>(
            r#"{"workflow":"license_plate","license_plate_id":5,"quantity":2}"#
        )
        .is_err());
    }

    #[test]
    fn confirmation_accepts_only_scanned_identifiers() {
        assert_eq!(
            serde_json::from_str::<ConfirmInventoryRelocationRequest>(
                r#"{"destination_location_barcode":"A-01","license_plate_barcode":"LP-9"}"#
            )
            .unwrap(),
            ConfirmInventoryRelocationRequest {
                destination_location_barcode: "A-01".into(),
                license_plate_barcode: Some("LP-9".into()),
            }
        );
        assert!(serde_json::from_str::<ConfirmInventoryRelocationRequest>(
            r#"{"destination_location_id":10}"#
        )
        .is_err());
    }
}
