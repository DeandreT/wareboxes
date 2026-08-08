use serde::{Deserialize, Serialize};

use super::Revision;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ClaimNextPickRequest {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ClaimPickByIdRequest {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatPickClaimRequest {}

/// Operator reason for returning active pick work to the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickClaimReleaseReason {
    WorkInterrupted,
    EquipmentUnavailable,
    SourceBlocked,
    InventoryDiscrepancy,
    SafetyIssue,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasePickClaimRequest {
    pub reason: PickClaimReleaseReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickClaimHeartbeatResponse {
    pub task_id: i64,
    pub heartbeat_at: String,
    pub lease_expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickClaimReleaseResponse {
    pub task_id: i64,
    pub released_at: String,
    pub release_count: i64,
    pub reason: PickClaimReleaseReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Completion state of one allocation-backed pick content record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickContentState {
    Pending,
    Completed,
}

/// Order states observable after an individual pick confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickOrderStatus {
    Processing,
    AwaitingShipment,
}

/// One scanner-ready allocation in a claimed pick task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickClaimContent {
    pub content_id: i64,
    pub order_line_id: i64,
    pub inventory_allocation_id: i64,
    pub source_inventory_balance_id: i64,
    pub item_batch_id: i64,
    pub source_location_id: i64,
    pub source_location_barcode: String,
    pub source_location_name: Option<String>,
    pub source_license_plate_id: Option<i64>,
    pub source_license_plate_barcode: Option<String>,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub item_barcodes: Vec<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<String>,
    pub planned_quantity: i64,
    pub state: PickContentState,
}

/// Active typed pick claim without persistence or tenant metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickClaimResponse {
    pub task_id: i64,
    pub order_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub order_key: String,
    pub priority: i64,
    pub ship_by: Option<String>,
    pub lease_expires_at: String,
    pub destination_location_id: i64,
    pub destination_location_barcode: String,
    pub destination_location_name: Option<String>,
    pub content: PickClaimContent,
}

/// The current claim is absent when the RF identity owns no active pick work.
pub type CurrentPickResponse = Option<PickClaimResponse>;

/// Confirms the immutable planned quantity using scanned source and tote identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmPickContentRequest {
    pub source_location_barcode: String,
    pub item_barcode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_license_plate_barcode: Option<String>,
    pub destination_license_plate_barcode: String,
}

/// Result of atomically moving one pick and advancing its workflow state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickContentConfirmationResponse {
    pub result_id: i64,
    pub content_id: i64,
    pub task_id: i64,
    pub order_id: i64,
    pub inventory_transaction_id: i64,
    pub source_inventory_allocation_id: i64,
    pub destination_inventory_allocation_id: i64,
    pub source_inventory_balance_id: i64,
    pub destination_inventory_balance_id: i64,
    pub source_location_id: i64,
    pub destination_location_id: i64,
    pub source_license_plate_id: Option<i64>,
    pub destination_license_plate_id: i64,
    pub picked_quantity: i64,
    pub confirmed_by: i64,
    pub confirmed_at: String,
    pub content_state: PickContentState,
    pub task_completed: bool,
    pub order_ready_to_pack: bool,
    pub order_status: PickOrderStatus,
    pub order_revision: Revision,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn claim_lifecycle_requests_are_empty_or_typed_and_strict() {
        assert_eq!(
            serde_json::from_value::<ClaimNextPickRequest>(json!({})).unwrap(),
            ClaimNextPickRequest {}
        );
        assert_eq!(
            serde_json::from_value::<ClaimPickByIdRequest>(json!({})).unwrap(),
            ClaimPickByIdRequest {}
        );
        assert!(serde_json::from_value::<ClaimPickByIdRequest>(json!({
            "task_id": 3
        }))
        .is_err());
        assert!(serde_json::from_value::<HeartbeatPickClaimRequest>(json!({
            "lease_seconds": 600
        }))
        .is_err());
        assert_eq!(
            serde_json::from_value::<ReleasePickClaimRequest>(json!({
                "reason": "inventory_discrepancy",
                "note": "Stock is missing"
            }))
            .unwrap(),
            ReleasePickClaimRequest {
                reason: PickClaimReleaseReason::InventoryDiscrepancy,
                note: Some("Stock is missing".into()),
            }
        );
    }

    #[test]
    fn confirmation_accepts_scans_but_not_a_client_selected_quantity() {
        let request = serde_json::from_value::<ConfirmPickContentRequest>(json!({
            "source_location_barcode": "A-01",
            "item_barcode": "SKU-1",
            "source_license_plate_barcode": "LP-1",
            "destination_license_plate_barcode": "TOTE-1"
        }))
        .unwrap();
        assert_eq!(request.source_location_barcode, "A-01");

        assert!(serde_json::from_value::<ConfirmPickContentRequest>(json!({
            "source_location_barcode": "A-01",
            "item_barcode": "SKU-1",
            "source_license_plate_barcode": "LP-1",
            "destination_license_plate_barcode": "TOTE-1",
            "picked_quantity": 4
        }))
        .is_err());
    }

    #[test]
    fn claim_is_one_allocation_backed_work_item() {
        let claim = PickClaimResponse {
            task_id: 1,
            order_id: 2,
            inventory_owner_id: 3,
            facility_id: 4,
            order_key: "ORDER-2".into(),
            priority: 80,
            ship_by: Some("2026-08-09T20:00:00Z".into()),
            lease_expires_at: "2026-08-08T20:30:00Z".into(),
            destination_location_id: 5,
            destination_location_barcode: "PACK-01".into(),
            destination_location_name: Some("Pack lane 1".into()),
            content: PickClaimContent {
                content_id: 6,
                order_line_id: 7,
                inventory_allocation_id: 8,
                source_inventory_balance_id: 9,
                item_batch_id: 10,
                source_location_id: 11,
                source_location_barcode: "A-01".into(),
                source_location_name: Some("Forward A-01".into()),
                source_license_plate_id: Some(12),
                source_license_plate_barcode: Some("LP-12".into()),
                item_id: 13,
                item_description: Some("Widget".into()),
                item_barcodes: vec!["SKU-1".into(), "000123".into()],
                uom: "each".into(),
                lot: Some("LOT-1".into()),
                serial: None,
                expiration: Some("2027-01-01T00:00:00Z".into()),
                planned_quantity: 4,
                state: PickContentState::Pending,
            },
        };

        let value = serde_json::to_value(claim).unwrap();
        assert_eq!(value["content"]["inventory_allocation_id"], 8);
        assert_eq!(value["content"]["item_barcodes"][1], "000123");
        assert!(value.get("tenant_id").is_none());
        assert!(value.get("metadata_json").is_none());
        assert!(value.get("contents").is_none());
    }
}
