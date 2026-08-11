use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboundLoadEntryItemResponse {
    pub item_id: i64,
    pub description: Option<String>,
    pub uom: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanInboundLoadLineRequest {
    pub item_id: i64,
    pub expected_quantity: i64,
    pub lot: Option<String>,
    pub serial: Option<String>,
    /// RFC 3339 timestamp, or `None` when the item is not expiration-controlled.
    pub expiration: Option<String>,
}

/// Atomically plans one inbound load and its complete initial expected contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanInboundLoadRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub receiving_location_id: i64,
    pub reference: String,
    pub invoice_number: Option<String>,
    pub carrier: Option<String>,
    pub trailer_number: Option<String>,
    pub seal_number: Option<String>,
    /// RFC 3339 timestamp for the supplier's expected arrival.
    pub expected_at: Option<String>,
    /// RFC 3339 timestamp for the warehouse appointment.
    pub appointment_at: Option<String>,
    pub lines: Vec<PlanInboundLoadLineRequest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlannedInboundLoadStatus {
    Planned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedInboundLoadLineResponse {
    pub load_line_id: i64,
    pub item_id: i64,
    pub expected_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanInboundLoadResponse {
    pub load_id: i64,
    pub execution_barcode: String,
    pub reference: String,
    pub status: PlannedInboundLoadStatus,
    pub lines: Vec<PlannedInboundLoadLineResponse>,
    pub total_expected_quantity: i64,
    pub planned_by: i64,
    pub planned_at: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn request_value() -> serde_json::Value {
        json!({
            "inventory_owner_id": 7,
            "facility_id": 8,
            "receiving_location_id": 9,
            "reference": "ASN-100",
            "invoice_number": "INV-100",
            "carrier": "Parcel Freight",
            "trailer_number": null,
            "seal_number": null,
            "expected_at": "2027-08-11T17:00:00Z",
            "appointment_at": "2027-08-12T17:00:00Z",
            "lines": [{
                "item_id": 41,
                "expected_quantity": 12,
                "lot": "LOT-A",
                "serial": null,
                "expiration": "2028-08-12T00:00:00Z"
            }]
        })
    }

    #[test]
    fn planning_request_has_an_exact_nested_shape() {
        let request = serde_json::from_value::<PlanInboundLoadRequest>(request_value()).unwrap();
        assert_eq!(request.reference, "ASN-100");
        assert_eq!(request.lines[0].expected_quantity, 12);

        let mut unknown = request_value();
        unknown["tenant_id"] = json!(99);
        assert!(serde_json::from_value::<PlanInboundLoadRequest>(unknown).is_err());

        let mut unknown_line = request_value();
        unknown_line["lines"][0]["received_quantity"] = json!(0);
        assert!(serde_json::from_value::<PlanInboundLoadRequest>(unknown_line).is_err());

        let mut body_key = request_value();
        body_key["idempotency_key"] = json!("body-key");
        assert!(serde_json::from_value::<PlanInboundLoadRequest>(body_key).is_err());
    }

    #[test]
    fn planning_response_preserves_server_line_identities() {
        let response = PlanInboundLoadResponse {
            load_id: 101,
            execution_barcode: "WB-LOAD-ABC".into(),
            reference: "ASN-100".into(),
            status: PlannedInboundLoadStatus::Planned,
            lines: vec![PlannedInboundLoadLineResponse {
                load_line_id: 201,
                item_id: 41,
                expected_quantity: 12,
            }],
            total_expected_quantity: 12,
            planned_by: 3,
            planned_at: "2027-08-10T17:00:00Z".into(),
        };
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["lines"][0]["load_line_id"], json!(201));
        assert_eq!(value["status"], json!("planned"));
    }
}
