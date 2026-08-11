use serde::{Deserialize, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PurchaseOrderStatus {
    Draft,
    Released,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePurchaseOrderLineRequest {
    pub item_id: i64,
    pub ordered_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePurchaseOrderRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub number: String,
    pub supplier: String,
    /// RFC 3339 requested delivery timestamp.
    pub expected_by: Option<String>,
    pub lines: Vec<CreatePurchaseOrderLineRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatedPurchaseOrderLineResponse {
    pub line_id: i64,
    pub item_id: i64,
    pub ordered_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePurchaseOrderResponse {
    pub purchase_order_id: i64,
    pub number: String,
    pub status: PurchaseOrderStatus,
    pub revision: Revision,
    pub lines: Vec<CreatedPurchaseOrderLineResponse>,
    pub total_ordered_quantity: i64,
    pub created_by: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasePurchaseOrderRequest {
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasePurchaseOrderResponse {
    pub release_id: i64,
    pub purchase_order_id: i64,
    pub previous_status: PurchaseOrderStatus,
    pub status: PurchaseOrderStatus,
    pub revision: Revision,
    pub released_by: i64,
    pub released_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PurchaseOrderLineResponse {
    pub line_id: i64,
    pub sequence: i64,
    pub item_id: i64,
    pub item_description: String,
    pub uom: String,
    pub ordered_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PurchaseOrderSummaryResponse {
    pub purchase_order_id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub number: String,
    pub supplier: String,
    pub expected_by: Option<String>,
    pub status: PurchaseOrderStatus,
    pub revision: Revision,
    pub line_count: i64,
    pub total_ordered_quantity: i64,
    pub created_by: i64,
    pub created_at: String,
    pub released_by: Option<i64>,
    pub released_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PurchaseOrderDetailResponse {
    #[serde(flatten)]
    pub summary: PurchaseOrderSummaryResponse,
    pub lines: Vec<PurchaseOrderLineResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PurchaseOrderPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<PurchaseOrderStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

pub type PurchaseOrderPage = CursorPage<PurchaseOrderSummaryResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    fn create_value() -> serde_json::Value {
        serde_json::json!({
            "inventory_owner_id": 7,
            "facility_id": 8,
            "number": "PO-100",
            "supplier": "Northstar Foods",
            "expected_by": "2027-08-11T17:00:00Z",
            "lines": [{"item_id": 41, "ordered_quantity": 12}]
        })
    }

    #[test]
    fn create_request_is_strict_and_nested() {
        let request = serde_json::from_value::<CreatePurchaseOrderRequest>(create_value()).unwrap();
        assert_eq!(request.number, "PO-100");
        assert_eq!(request.lines[0].ordered_quantity, 12);
        let mut unknown = create_value();
        unknown["tenant_id"] = serde_json::json!(99);
        assert!(serde_json::from_value::<CreatePurchaseOrderRequest>(unknown).is_err());
        let mut line_unknown = create_value();
        line_unknown["lines"][0]["received_quantity"] = serde_json::json!(0);
        assert!(serde_json::from_value::<CreatePurchaseOrderRequest>(line_unknown).is_err());
    }

    #[test]
    fn release_request_requires_only_the_revision() {
        let request = serde_json::json!({"expected_revision": 1});
        assert!(serde_json::from_value::<ReleasePurchaseOrderRequest>(request.clone()).is_ok());
        let mut invalid = request;
        invalid["status"] = serde_json::json!("released");
        assert!(serde_json::from_value::<ReleasePurchaseOrderRequest>(invalid).is_err());
    }
}
