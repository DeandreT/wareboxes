use serde::{Deserialize, Serialize};

use super::{
    AllocationPolicyReference, PlanOrderAllocationResponse, ReleaseOrderResponse, Revision,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamOrderRequest {
    pub facility_id: i64,
    pub destination_location_id: i64,
    pub expected_revision: Revision,
    pub expected_allocation_policy: AllocationPolicyReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamOrderResponse {
    pub allocation: PlanOrderAllocationResponse,
    pub release: ReleaseOrderResponse,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_request_is_strict_and_carries_the_observed_policy() {
        let value = serde_json::json!({
            "facility_id": 3,
            "destination_location_id": 4,
            "expected_revision": 7,
            "expected_allocation_policy": {
                "source": "product_default",
                "policy_hash": super::super::PRODUCT_DEFAULT_ALLOCATION_POLICY_HASH
            }
        });
        let request = serde_json::from_value::<StreamOrderRequest>(value.clone()).unwrap();
        assert_eq!(request.expected_revision.get(), 7);

        let mut changed = value;
        changed["force"] = serde_json::Value::Bool(true);
        assert!(serde_json::from_value::<StreamOrderRequest>(changed).is_err());
    }
}
