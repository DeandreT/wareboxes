use serde::{Deserialize, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, PutawayWorkflow};

/// Creates one directed putaway task for loose inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePutawayTaskRequest {
    pub source_inventory_balance_id: i64,
    pub destination_location_id: i64,
    pub quantity: i64,
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

/// Identity of a newly created directed putaway task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePutawayTaskResponse {
    pub task_id: i64,
}

/// Confirms the scanned destination for a directed putaway task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmPutawayRequest {
    pub destination_location_barcode: String,
}

/// Result of atomically completing a directed loose-inventory putaway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutawayConfirmationResponse {
    pub task_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub inventory_transaction_id: i64,
    pub source_inventory_balance_id: i64,
    pub destination_inventory_balance_id: i64,
    pub source_location_id: i64,
    pub destination_location_id: i64,
    pub destination_location_barcode: String,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub quantity: i64,
    pub inventory_status: String,
    pub confirmed_by: i64,
    pub confirmed_at: String,
}

/// Stable lifecycle grouping used by the supervisor putaway monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PutawayWorkStatus {
    Pending,
    Claimed,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PutawayCandidateSort {
    #[default]
    ReceivedAt,
    Client,
    Facility,
    Source,
    Item,
    Quantity,
    Workflow,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PutawayWorkSort {
    Priority,
    #[default]
    CreatedAt,
    Client,
    Facility,
    Source,
    Destination,
    Quantity,
    Status,
    Workflow,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PutawaySortDirection {
    #[default]
    Asc,
    Desc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PutawayCandidatePageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<PutawayWorkflow>,
    #[serde(default)]
    pub sort: PutawayCandidateSort,
    #[serde(default)]
    pub direction: PutawaySortDirection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PutawayWorkPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<PutawayWorkflow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<PutawayWorkStatus>,
    #[serde(default)]
    pub sort: PutawayWorkSort,
    #[serde(default)]
    pub direction: PutawaySortDirection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutawayLocationResponse {
    pub location_id: i64,
    pub barcode: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutawayCandidateResponse {
    pub workflow: PutawayWorkflow,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub source_inventory_balance_id: Option<i64>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub source_location: PutawayLocationResponse,
    pub item_count: i64,
    pub balance_count: i64,
    pub item_id: Option<i64>,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub uom: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub available_quantity: i64,
    pub received_at: String,
}

pub type PutawayCandidatePage = CursorPage<PutawayCandidateResponse>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutawayWorkResponse {
    pub task_id: i64,
    pub workflow: PutawayWorkflow,
    pub status: PutawayWorkStatus,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub source_inventory_balance_id: Option<i64>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub source_location: PutawayLocationResponse,
    pub destination_location: PutawayLocationResponse,
    pub item_count: i64,
    pub balance_count: i64,
    pub item_id: Option<i64>,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub uom: Option<String>,
    pub planned_quantity: i64,
    pub priority: i64,
    pub instructions: Option<String>,
    pub assigned_user_id: Option<i64>,
    pub lease_expires_at: Option<String>,
    pub due_at: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

pub type PutawayWorkPage = CursorPage<PutawayWorkResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn putaway_confirmation_requires_only_the_scanned_destination_barcode() {
        assert_eq!(
            serde_json::from_str::<ConfirmPutawayRequest>(
                r#"{"destination_location_barcode":"A-01-01"}"#
            )
            .unwrap(),
            ConfirmPutawayRequest {
                destination_location_barcode: "A-01-01".into(),
            }
        );
        assert!(
            serde_json::from_str::<ConfirmPutawayRequest>(r#"{"destination_location_id":42}"#)
                .is_err()
        );
        assert!(serde_json::from_str::<ConfirmPutawayRequest>(
            r#"{"destination_location_barcode":"A-01-01","task_id":4}"#
        )
        .is_err());
    }

    #[test]
    fn manager_page_requests_are_strict_and_sortable() {
        let request = serde_json::from_str::<PutawayCandidatePageRequest>(
            r#"{"facility_id":4,"workflow":"license_plate","sort":"quantity","direction":"desc"}"#,
        )
        .unwrap();
        assert_eq!(request.sort, PutawayCandidateSort::Quantity);
        assert_eq!(request.direction, PutawaySortDirection::Desc);
        assert!(serde_json::from_str::<PutawayWorkPageRequest>(
            r#"{"status":"pending","unknown":true}"#
        )
        .is_err());
    }
}
