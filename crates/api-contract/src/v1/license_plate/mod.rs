use serde::{Deserialize, Serialize};

pub const MAX_LICENSE_PLATE_HIERARCHY_REASON_LENGTH: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LicensePlateHierarchyAction {
    Attached,
    Detached,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeLicensePlateParentRequest {
    pub parent_license_plate_id: Option<i64>,
    pub expected_revision: i64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeLicensePlateParentResponse {
    pub license_plate_id: i64,
    pub previous_parent_license_plate_id: Option<i64>,
    pub parent_license_plate_id: Option<i64>,
    pub root_license_plate_id: i64,
    pub depth: u8,
    pub resulting_revision: i64,
    pub changed_at: String,
    pub changed_by_user_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LicensePlateHierarchyNodeResponse {
    pub license_plate_id: i64,
    pub barcode: Option<String>,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub location_id: Option<i64>,
    pub parent_license_plate_id: Option<i64>,
    pub root_license_plate_id: i64,
    pub depth: u8,
    pub hierarchy_revision: i64,
    pub direct_child_ids: Vec<i64>,
    pub descendant_ids: Vec<i64>,
    pub direct_unit_quantity: i64,
    pub contained_unit_quantity: i64,
    pub hierarchy_updated_at: Option<String>,
    pub hierarchy_updated_by_user_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LicensePlateHierarchyEventResponse {
    pub event_id: i64,
    pub child_license_plate_id: i64,
    pub previous_parent_license_plate_id: Option<i64>,
    pub parent_license_plate_id: Option<i64>,
    pub resulting_revision: i64,
    pub action: LicensePlateHierarchyAction,
    pub actor_user_id: i64,
    pub occurred_at: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LicensePlateHierarchyResponse {
    pub node: LicensePlateHierarchyNodeResponse,
    pub ancestors: Vec<LicensePlateHierarchyNodeResponse>,
    pub descendants: Vec<LicensePlateHierarchyNodeResponse>,
    pub events: Vec<LicensePlateHierarchyEventResponse>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_request_rejects_unknown_fields() {
        let error = serde_json::from_value::<ChangeLicensePlateParentRequest>(serde_json::json!({
            "parent_license_plate_id": 8,
            "expected_revision": 0,
            "reason": "Consolidated onto pallet",
            "force": true
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }
}
