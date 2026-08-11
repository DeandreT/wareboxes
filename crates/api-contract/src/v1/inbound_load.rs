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

/// Exact scanner evidence for transitioning a planned inbound load to arrived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArriveInboundLoadRequest {
    pub load_scan: String,
    pub receiving_location_scan: String,
    /// Optional RFC 3339 actual arrival; omitted to use authoritative server time.
    pub arrived_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundLoadPreArrivalStatus {
    Planned,
    Scheduled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrivedInboundLoadStatus {
    Arrived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArriveInboundLoadResponse {
    pub arrival_id: i64,
    pub load_id: i64,
    pub previous_status: InboundLoadPreArrivalStatus,
    pub status: ArrivedInboundLoadStatus,
    pub receiving_location_id: i64,
    pub arrived_by: i64,
    pub arrived_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartInboundLoadUnloadingRequest {
    pub load_scan: String,
    pub receiving_location_scan: String,
    pub seal_scan: Option<String>,
    /// Optional RFC 3339 start time; omitted to use authoritative server time.
    pub started_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundLoadReceivingStatus {
    Receiving,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartInboundLoadUnloadingResponse {
    pub unloading_start_id: i64,
    pub load_id: i64,
    pub status: InboundLoadReceivingStatus,
    pub receiving_location_id: i64,
    pub started_by: i64,
    pub started_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloseInboundLoadRequest {
    pub load_scan: String,
    pub receiving_location_scan: String,
    /// Optional RFC 3339 closure time; omitted to use authoritative server time.
    pub closed_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundLoadReceivedStatus {
    Received,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundLoadClosedStatus {
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloseInboundLoadResponse {
    pub closure_id: i64,
    pub load_id: i64,
    pub previous_status: InboundLoadReceivedStatus,
    pub status: InboundLoadClosedStatus,
    pub receiving_location_id: i64,
    pub closed_by: i64,
    pub closed_at: String,
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

    #[test]
    fn arrival_request_is_scan_only_and_strict() {
        let request = json!({
            "load_scan": "WB-LOAD-101",
            "receiving_location_scan": "RECV-01",
            "arrived_at": null
        });
        let decoded = serde_json::from_value::<ArriveInboundLoadRequest>(request.clone()).unwrap();
        assert_eq!(decoded.load_scan, "WB-LOAD-101");

        let mut with_status = request;
        with_status["status"] = json!("arrived");
        assert!(serde_json::from_value::<ArriveInboundLoadRequest>(with_status).is_err());
    }

    #[test]
    fn unloading_start_request_is_scan_only_and_strict() {
        let request = json!({
            "load_scan": "WB-LOAD-101",
            "receiving_location_scan": "RECV-01",
            "seal_scan": "SEAL-101",
            "started_at": null
        });
        assert!(
            serde_json::from_value::<StartInboundLoadUnloadingRequest>(request.clone()).is_ok()
        );
        let mut changed = request;
        changed["quantity"] = json!(1);
        assert!(serde_json::from_value::<StartInboundLoadUnloadingRequest>(changed).is_err());
    }

    #[test]
    fn closure_request_is_scan_only_and_strict() {
        let request = json!({
            "load_scan": "WB-LOAD-101",
            "receiving_location_scan": "RECV-01",
            "closed_at": null
        });
        assert!(serde_json::from_value::<CloseInboundLoadRequest>(request.clone()).is_ok());
        let mut changed = request;
        changed["status"] = json!("closed");
        assert!(serde_json::from_value::<CloseInboundLoadRequest>(changed).is_err());
    }
}
