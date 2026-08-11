use serde::{Deserialize, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundAsnStatus {
    Open,
    Planned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInboundAsnLineRequest {
    pub item_id: i64,
    pub expected_quantity: i64,
    pub lot: Option<String>,
    pub serial: Option<String>,
    /// RFC 3339 timestamp, or `None` for stock without an expiration identity.
    pub expiration: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInboundAsnRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub number: String,
    pub supplier: String,
    /// RFC 3339 expected arrival supplied by the trading partner.
    pub expected_at: Option<String>,
    pub lines: Vec<CreateInboundAsnLineRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatedInboundAsnLineResponse {
    pub line_id: i64,
    pub item_id: i64,
    pub expected_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInboundAsnResponse {
    pub asn_id: i64,
    pub number: String,
    pub status: InboundAsnStatus,
    pub revision: Revision,
    pub lines: Vec<CreatedInboundAsnLineResponse>,
    pub total_expected_quantity: i64,
    pub created_by: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanInboundAsnLoadRequest {
    pub expected_revision: Revision,
    pub receiving_location_id: i64,
    pub carrier: Option<String>,
    pub trailer_number: Option<String>,
    pub seal_number: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedInboundAsnLoadLineResponse {
    pub asn_line_id: i64,
    pub load_line_id: i64,
    pub item_id: i64,
    pub expected_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanInboundAsnLoadResponse {
    pub plan_id: i64,
    pub asn_id: i64,
    pub asn_status: InboundAsnStatus,
    pub asn_revision: Revision,
    pub load_id: i64,
    pub execution_barcode: String,
    pub lines: Vec<PlannedInboundAsnLoadLineResponse>,
    pub total_expected_quantity: i64,
    pub planned_by: i64,
    pub planned_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboundAsnLineResponse {
    pub line_id: i64,
    pub sequence: i64,
    pub item_id: i64,
    pub item_description: String,
    pub uom: String,
    pub expected_quantity: i64,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboundAsnSummaryResponse {
    pub asn_id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub number: String,
    pub supplier: String,
    pub expected_at: Option<String>,
    pub status: InboundAsnStatus,
    pub revision: Revision,
    pub line_count: i64,
    pub total_expected_quantity: i64,
    pub load_id: Option<i64>,
    pub created_by: i64,
    pub created_at: String,
    pub planned_by: Option<i64>,
    pub planned_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboundAsnDetailResponse {
    #[serde(flatten)]
    pub summary: InboundAsnSummaryResponse,
    pub lines: Vec<InboundAsnLineResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct InboundAsnPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<InboundAsnStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

pub type InboundAsnPage = CursorPage<InboundAsnSummaryResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    fn create_value() -> serde_json::Value {
        serde_json::json!({
            "inventory_owner_id": 7,
            "facility_id": 8,
            "number": "ASN-100",
            "supplier": "Northstar Foods",
            "expected_at": "2027-08-11T17:00:00Z",
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
    fn create_request_is_strict_and_nested() {
        let request = serde_json::from_value::<CreateInboundAsnRequest>(create_value()).unwrap();
        assert_eq!(request.number, "ASN-100");
        assert_eq!(request.lines[0].expected_quantity, 12);

        let mut unknown = create_value();
        unknown["tenant_id"] = serde_json::json!(99);
        assert!(serde_json::from_value::<CreateInboundAsnRequest>(unknown).is_err());
        let mut line_unknown = create_value();
        line_unknown["lines"][0]["received_quantity"] = serde_json::json!(0);
        assert!(serde_json::from_value::<CreateInboundAsnRequest>(line_unknown).is_err());
    }

    #[test]
    fn planning_request_is_revisioned_and_server_derives_lines() {
        let request = serde_json::json!({
            "expected_revision": 1,
            "receiving_location_id": 9,
            "carrier": "Parcel Freight",
            "trailer_number": null,
            "seal_number": null
        });
        assert!(serde_json::from_value::<PlanInboundAsnLoadRequest>(request.clone()).is_ok());
        let mut invalid = request;
        invalid["lines"] = serde_json::json!([]);
        assert!(serde_json::from_value::<PlanInboundAsnLoadRequest>(invalid).is_err());
    }
}
