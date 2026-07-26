use serde::{Deserialize, Serialize};

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
}
