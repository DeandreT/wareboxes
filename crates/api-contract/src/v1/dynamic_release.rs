use serde::{Deserialize, Serialize};

use super::{PickWaveResponse, Revision, WavePolicyExpectation, WavePolicyResponse};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicReleaseReadinessRequest {
    pub facility_id: i64,
    pub inventory_owner_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunDynamicReleaseRequest {
    pub facility_id: i64,
    pub inventory_owner_id: i64,
    pub destination_location_id: i64,
    pub expected_policy: WavePolicyExpectation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicReleaseCandidateResponse {
    pub order_id: i64,
    pub order_key: String,
    pub revision: Revision,
    pub rank: u32,
    pub rush: bool,
    pub ship_by: Option<String>,
    pub order_created_at: String,
    pub demand_quantity: i64,
    pub allocated_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicReleaseReadinessResponse {
    pub facility_id: i64,
    pub inventory_owner_id: i64,
    pub input_snapshot_at: String,
    pub policy: WavePolicyResponse,
    pub eligible_order_count: i64,
    pub selected_order_count: i64,
    pub deferred_order_count: i64,
    pub selected_orders: Vec<DynamicReleaseCandidateResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicReleaseRunResponse {
    pub run_id: i64,
    pub facility_id: i64,
    pub inventory_owner_id: i64,
    pub destination_location_id: i64,
    pub input_snapshot_at: String,
    pub policy: WavePolicyResponse,
    pub eligible_order_count: i64,
    pub selected_order_count: i64,
    pub deferred_order_count: i64,
    pub selected_orders: Vec<DynamicReleaseCandidateResponse>,
    pub wave: Option<PickWaveResponse>,
    pub released_by: i64,
    pub released_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_request_is_strict_and_policy_bound() {
        let value = serde_json::json!({
            "facility_id": 1,
            "inventory_owner_id": 2,
            "destination_location_id": 3,
            "expected_policy": WavePolicyExpectation::product_default()
        });
        let request = serde_json::from_value::<RunDynamicReleaseRequest>(value.clone()).unwrap();
        assert_eq!(request.inventory_owner_id, 2);

        let mut extra = value;
        extra["max_orders"] = serde_json::json!(999);
        assert!(serde_json::from_value::<RunDynamicReleaseRequest>(extra).is_err());
    }
}
