use serde::{Deserialize, Serialize};

use super::Revision;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationOrderProcessingStatus {
    Quarantined,
    Processed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationOrderIntakeResponse {
    pub receipt_id: i64,
    pub processing_id: i64,
    pub processing_attempt_id: i64,
    pub inventory_owner_id: i64,
    pub adapter_key: String,
    pub mapping_version: i32,
    pub status: IntegrationOrderProcessingStatus,
    pub revision: Revision,
    pub attempt_count: i32,
    pub order_id: Option<i64>,
    pub order_revision: Option<Revision>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_by: i64,
    pub attempted_at: String,
    pub processed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReprocessIntegrationOrderRequest {
    pub expected_revision: Revision,
}

pub type ReprocessIntegrationOrderResponse = IntegrationOrderIntakeResponse;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reprocessing_request_is_strict_and_revision_bound() {
        let request: ReprocessIntegrationOrderRequest =
            serde_json::from_value(json!({"expected_revision": 3})).unwrap();
        assert_eq!(request.expected_revision.get(), 3);
        assert!(
            serde_json::from_value::<ReprocessIntegrationOrderRequest>(json!({
                "expected_revision": 3,
                "force": true
            }))
            .is_err()
        );
    }
}
