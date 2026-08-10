use serde::{Deserialize, Serialize};

use super::{CreateFulfillmentOrderRequest, FulfillmentOrderDestination, Revision};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationOrderEnvelopeLineRequest {
    pub line_key: String,
    pub external_item_key: String,
    pub external_uom: String,
    pub quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationOrderEnvelopeRequest {
    pub order_key: String,
    #[serde(default)]
    pub rush: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ship_by: Option<String>,
    pub destination: FulfillmentOrderDestination,
    pub lines: Vec<IntegrationOrderEnvelopeLineRequest>,
}

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
    pub correction_id: Option<i64>,
    pub input_payload_sha256: String,
    pub inventory_owner_id: i64,
    pub adapter_key: String,
    pub mapping_version: i32,
    pub status: IntegrationOrderProcessingStatus,
    pub revision: Revision,
    pub attempt_count: i32,
    pub applied_mapping_count: i32,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectIntegrationOrderRequest {
    pub expected_revision: Revision,
    pub reason: String,
    pub order: CreateFulfillmentOrderRequest,
}

pub type ReprocessIntegrationOrderResponse = IntegrationOrderIntakeResponse;
pub type CorrectIntegrationOrderResponse = IntegrationOrderIntakeResponse;

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

    #[test]
    fn correction_request_is_strict_and_carries_a_typed_order() {
        let value = json!({
            "expected_revision": 2,
            "reason": "corrected item mapping",
            "order": {
                "inventory_owner_id": 7,
                "order_key": "EXT-200",
                "rush": false,
                "ship_by": null,
                "destination": {
                    "recipient_name": "Receiving",
                    "company": null,
                    "phone": null,
                    "email": null,
                    "line1": "10 Main St",
                    "line2": null,
                    "city": "Reno",
                    "region": "NV",
                    "postal_code": "89501",
                    "country": "US"
                },
                "lines": [{
                    "line_key": "1",
                    "item_id": 11,
                    "quantity": 2,
                    "requested_uom": "case"
                }]
            }
        });
        assert!(serde_json::from_value::<CorrectIntegrationOrderRequest>(value.clone()).is_ok());
        let mut unknown = value;
        unknown["force"] = json!(true);
        assert!(serde_json::from_value::<CorrectIntegrationOrderRequest>(unknown).is_err());
    }

    #[test]
    fn external_order_envelope_contains_no_internal_owner_or_item_ids() {
        let value = json!({
            "order_key": "EXT-200",
            "rush": false,
            "ship_by": null,
            "destination": {
                "recipient_name": "Receiving",
                "company": null,
                "phone": null,
                "email": null,
                "line1": "10 Main St",
                "line2": null,
                "city": "Reno",
                "region": "NV",
                "postal_code": "89501",
                "country": "US"
            },
            "lines": [{
                "line_key": "1",
                "external_item_key": "CLIENT-SKU-11",
                "external_uom": "CS",
                "quantity": 2
            }]
        });
        assert!(serde_json::from_value::<IntegrationOrderEnvelopeRequest>(value.clone()).is_ok());
        let mut internal = value;
        internal["inventory_owner_id"] = json!(7);
        assert!(serde_json::from_value::<IntegrationOrderEnvelopeRequest>(internal).is_err());
    }
}
