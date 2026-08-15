use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{CreateFulfillmentOrderRequest, FulfillmentOrderDestination, Revision};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct IntegrationOrderEnvelopeLineRequest {
    /// Partner line identity, unique within the order.
    #[schema(min_length = 1, max_length = 200, example = "1")]
    pub line_key: String,
    /// Partner item or SKU identity resolved through the active source mapping.
    #[schema(min_length = 1, max_length = 200, example = "CLIENT-CASE")]
    pub external_item_key: String,
    /// Partner UOM code resolved together with the external item key.
    #[schema(min_length = 1, max_length = 32, example = "CS")]
    pub external_uom: String,
    /// Positive demand quantity expressed in `external_uom`.
    #[schema(minimum = 1, example = 4)]
    pub quantity: i64,
}

/// Partner-facing fulfillment order envelope retained before mapping and processing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct IntegrationOrderEnvelopeRequest {
    /// Partner order identity. Reuse for a different order is rejected.
    #[schema(min_length = 1, max_length = 200, example = "SO-1001")]
    pub order_key: String,
    /// Marks demand for warehouse prioritization; it does not bypass inventory policy.
    #[serde(default)]
    #[schema(default = false, example = false)]
    pub rush: bool,
    /// Optional RFC 3339 shipping deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, format = DateTime, example = "2027-08-12T17:00:00Z")]
    pub ship_by: Option<String>,
    pub destination: FulfillmentOrderDestination,
    /// Demand lines. At least one line is required and line keys must be unique.
    #[schema(min_items = 1)]
    pub lines: Vec<IntegrationOrderEnvelopeLineRequest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationOrderProcessingStatus {
    /// The receipt is durable but requires mapping or operator correction.
    Quarantined,
    /// A fulfillment order was created or the original result was replayed.
    Processed,
}

/// Durable receipt and processing outcome returned by order intake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct IntegrationOrderIntakeResponse {
    #[schema(minimum = 1, example = 501)]
    pub receipt_id: i64,
    #[schema(minimum = 1, example = 601)]
    pub processing_id: i64,
    #[schema(minimum = 1, example = 701)]
    pub processing_attempt_id: i64,
    #[schema(required = true, minimum = 1, example = 801)]
    pub correction_id: Option<i64>,
    #[schema(
        min_length = 64,
        max_length = 64,
        pattern = "^[0-9a-f]{64}$",
        example = "4cacc15b0023683e11cc4c371c585f8aefe1a12221edeb64290fbe35be4e4ccd"
    )]
    pub input_payload_sha256: String,
    #[schema(minimum = 1, example = 42)]
    pub inventory_owner_id: i64,
    #[schema(example = "wareboxes.fulfillment_order")]
    pub adapter_key: String,
    /// Version of the canonical intake adapter contract, not an individual item mapping revision.
    #[schema(minimum = 1, example = 2)]
    pub mapping_version: i32,
    pub status: IntegrationOrderProcessingStatus,
    #[schema(value_type = i64, minimum = 1, example = 1)]
    pub revision: Revision,
    #[schema(minimum = 1, example = 1)]
    pub attempt_count: i32,
    #[schema(minimum = 0, example = 1)]
    pub applied_mapping_count: i32,
    #[schema(required = true, minimum = 1, example = 9001)]
    pub order_id: Option<i64>,
    #[schema(required = true, value_type = Option<i64>, minimum = 1, example = 1)]
    pub order_revision: Option<Revision>,
    #[schema(required = true, example = "item_mapping_not_found")]
    pub error_code: Option<String>,
    #[schema(
        required = true,
        example = "item mapping was not found for CLIENT-CASE / CS"
    )]
    pub error_message: Option<String>,
    #[schema(minimum = 1, example = 7)]
    pub attempted_by: i64,
    #[schema(value_type = String, format = DateTime, example = "2026-08-11T19:30:00Z")]
    pub attempted_at: String,
    #[schema(
        required = true,
        value_type = Option<String>,
        format = DateTime,
        example = "2026-08-11T19:30:00Z"
    )]
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

    #[test]
    fn public_documentation_examples_match_the_v1_contract() {
        let request = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../developer-docs/examples/v1/submit-order.json"
        ));
        let processed_response = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../developer-docs/examples/v1/processed-order-response.json"
        ));

        serde_json::from_str::<IntegrationOrderEnvelopeRequest>(request)
            .expect("documented order submission should satisfy the public contract");
        let response = serde_json::from_str::<IntegrationOrderIntakeResponse>(processed_response)
            .expect("documented processed response should satisfy the public contract");
        assert_eq!(response.status, IntegrationOrderProcessingStatus::Processed);
        assert!(response.order_id.is_some());
        assert!(response.error_code.is_none());
    }
}
