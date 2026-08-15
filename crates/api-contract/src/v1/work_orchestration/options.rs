use serde::{Deserialize, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkOrchestrationWorkerPageRequest {
    pub facility_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkOrchestrationWorkerOptionResponse {
    pub employee_id: i64,
    pub user_id: i64,
    pub display_name: String,
    pub title: String,
}

pub type WorkOrchestrationWorkerPage = CursorPage<WorkOrchestrationWorkerOptionResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_option_contract_is_exact() {
        let option = WorkOrchestrationWorkerOptionResponse {
            employee_id: 41,
            user_id: 73,
            display_name: "Ada Lovelace".into(),
            title: "Inventory controller".into(),
        };
        let json = serde_json::to_string(&option).unwrap();
        assert_eq!(
            json,
            r#"{"employee_id":41,"user_id":73,"display_name":"Ada Lovelace","title":"Inventory controller"}"#
        );
        assert_eq!(
            serde_json::from_str::<WorkOrchestrationWorkerOptionResponse>(&json).unwrap(),
            option
        );
        assert!(serde_json::from_str::<WorkOrchestrationWorkerOptionResponse>(
            r#"{"employee_id":41,"user_id":73,"display_name":"Ada Lovelace","title":"Inventory controller","tenant_id":1}"#
        )
        .is_err());
    }
}
