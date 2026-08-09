use serde::{Deserialize, Serialize};

use super::Revision;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplacedFulfillmentOrderStatus {
    Open,
    Held,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaceFulfillmentOrderLineRequest {
    pub line_key: String,
    pub item_id: i64,
    pub quantity: i64,
    pub requested_uom: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaceFulfillmentOrderLinesRequest {
    pub expected_revision: Revision,
    pub lines: Vec<ReplaceFulfillmentOrderLineRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplacedFulfillmentOrderLineResponse {
    pub order_line_id: i64,
    pub line_key: String,
    pub line_number: u32,
    pub item_id: i64,
    pub quantity: i64,
    pub requested_uom: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaceFulfillmentOrderLinesResponse {
    pub amendment_id: i64,
    pub order_id: i64,
    pub inventory_owner_id: i64,
    pub order_status: ReplacedFulfillmentOrderStatus,
    pub previous_revision: Revision,
    pub revision: Revision,
    pub previous_line_count: i64,
    pub previous_quantity: i64,
    pub resulting_quantity: i64,
    pub released_reservation_count: i64,
    pub released_allocation_count: i64,
    pub released_quantity: i64,
    pub lines: Vec<ReplacedFulfillmentOrderLineResponse>,
    pub amended_by: i64,
    pub amended_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exact_replacement_request_rejects_unknown_and_invalid_revision_fields() {
        let value = json!({
            "expected_revision": 3,
            "lines": [{"line_key":"1","item_id":9,"quantity":4,"requested_uom":"case"}]
        });
        let request: ReplaceFulfillmentOrderLinesRequest =
            serde_json::from_value(value.clone()).unwrap();
        assert_eq!(request.lines[0].line_key, "1");
        let mut unknown = value;
        unknown["order_id"] = json!(8);
        assert!(serde_json::from_value::<ReplaceFulfillmentOrderLinesRequest>(unknown).is_err());
    }
}
