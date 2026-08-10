use serde::{Deserialize, Serialize};

use super::InventoryBalanceStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundInspectionOutcome {
    Approved,
    Damaged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisposeInboundInspectionRequest {
    pub outcome: InboundInspectionOutcome,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisposeInboundInspectionResponse {
    pub disposition_id: i64,
    pub inventory_hold_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub source_inventory_balance_id: i64,
    pub target_inventory_balance_id: i64,
    pub location_id: i64,
    pub license_plate_id: Option<i64>,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub quantity: i64,
    pub outcome: InboundInspectionOutcome,
    pub target_status: InventoryBalanceStatus,
    pub note: String,
    pub inventory_transaction_id: i64,
    pub inspected_by_user_id: i64,
    pub inspected_at: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn disposition_request_is_strict() {
        let value = json!({"outcome":"approved","note":"Seal and contents passed inspection"});
        let request: DisposeInboundInspectionRequest =
            serde_json::from_value(value.clone()).unwrap();
        assert_eq!(request.outcome, InboundInspectionOutcome::Approved);
        assert_eq!(serde_json::to_value(request).unwrap(), value);
        assert!(
            serde_json::from_value::<DisposeInboundInspectionRequest>(json!({
                "outcome":"approved", "note":"Passed", "quantity":2
            }))
            .is_err()
        );
    }
}
