use serde::{Deserialize, Serialize};

/// Creates one directed putaway task for an entire license plate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateLicensePlatePutawayTaskRequest {
    pub license_plate_id: i64,
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

/// Identity of a newly created directed license-plate putaway task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateLicensePlatePutawayTaskResponse {
    pub task_id: i64,
}

/// Confirms the scanned license plate and destination for a directed putaway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmLicensePlatePutawayRequest {
    pub license_plate_barcode: String,
    pub destination_location_barcode: String,
}

/// Result of atomically completing a directed whole-license-plate putaway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LicensePlatePutawayConfirmationResponse {
    pub task_id: i64,
    pub license_plate_id: i64,
    pub license_plate_barcode: String,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub source_location_id: i64,
    pub destination_location_id: i64,
    pub destination_location_barcode: String,
    pub inventory_transaction_id: i64,
    pub moved_balance_count: i64,
    pub confirmed_by: i64,
    pub confirmed_at: String,
}
