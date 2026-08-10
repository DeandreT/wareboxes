use serde::{Deserialize, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryRecallStatus {
    Active,
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryRecallReason {
    Regulatory,
    SupplierNotice,
    CustomerRequest,
    QualityConcern,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInventoryRecallRequest {
    pub facility_id: i64,
    pub item_batch_id: i64,
    pub reason: InventoryRecallReason,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseInventoryRecallRequest {
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct InventoryRecallPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<InventoryRecallStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryRecallResponse {
    pub recall_id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub primary_sku: Option<String>,
    pub item_description: Option<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub expiration: Option<String>,
    pub serial: Option<String>,
    pub status: InventoryRecallStatus,
    pub revision: Revision,
    pub reason: InventoryRecallReason,
    pub note: Option<String>,
    pub affected_position_count: u32,
    pub held_quantity: i64,
    pub created_by: i64,
    pub created_at: String,
    pub released_by: Option<i64>,
    pub released_at: Option<String>,
}

pub type InventoryRecallPage = CursorPage<InventoryRecallResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_request_is_strict() {
        let valid = serde_json::json!({
            "facility_id": 2,
            "item_batch_id": 4,
            "reason": "regulatory",
            "note": null
        });
        assert!(serde_json::from_value::<CreateInventoryRecallRequest>(valid.clone()).is_ok());
        let mut invalid = valid.as_object().unwrap().clone();
        invalid.insert("partial".into(), serde_json::json!(true));
        assert!(
            serde_json::from_value::<CreateInventoryRecallRequest>(serde_json::Value::Object(
                invalid
            ))
            .is_err()
        );
    }
}
