use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ClaimNextZonePickRequest {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickZoneWorkspaceRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickZoneQueueResponse {
    pub storage_zone_id: i64,
    pub code: String,
    pub name: String,
    pub revision: i64,
    pub travel_sequence: u32,
    pub open_task_count: i64,
    pub active_task_count: i64,
    pub oldest_open_task_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickZoneWorkspaceResponse {
    pub queues: Vec<PickZoneQueueResponse>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn zone_claim_is_empty_and_workspace_scope_is_exact() {
        assert_eq!(
            serde_json::to_value(ClaimNextZonePickRequest::default()).unwrap(),
            json!({})
        );
        assert!(serde_json::from_value::<ClaimNextZonePickRequest>(json!({"zone_id": 7})).is_err());
        let request = serde_json::from_value::<PickZoneWorkspaceRequest>(json!({
            "inventory_owner_id": 9,
            "facility_id": 4
        }))
        .unwrap();
        assert_eq!(request.inventory_owner_id, 9);
        assert_eq!(request.facility_id, 4);
        assert!(serde_json::from_value::<PickZoneWorkspaceRequest>(json!({
            "inventory_owner_id": 9,
            "facility_id": 4,
            "include_history": true
        }))
        .is_err());
    }
}
