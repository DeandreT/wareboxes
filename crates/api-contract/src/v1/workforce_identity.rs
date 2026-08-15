use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkEmployeeIdentityRequest {
    pub user_id: i64,
    /// Omit only when the employee is expected to be currently unlinked.
    pub expected_user_id: Option<i64>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnlinkEmployeeIdentityRequest {
    pub expected_user_id: i64,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmployeeIdentityChangeKind {
    Linked,
    Relinked,
    Unlinked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmployeeIdentityChangeResponse {
    pub change_id: i64,
    pub employee_id: i64,
    pub previous_user_id: Option<i64>,
    pub user_id: Option<i64>,
    pub kind: EmployeeIdentityChangeKind,
    pub reason: String,
    pub changed_by: i64,
    pub changed_at: String,
    pub resulting_revision: i64,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn requests_reject_unknown_fields() {
        assert!(
            serde_json::from_value::<LinkEmployeeIdentityRequest>(json!({
                "user_id": 8,
                "expected_user_id": null,
                "reason": "interactive access",
                "force": true
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<UnlinkEmployeeIdentityRequest>(json!({
                "expected_user_id": 8,
                "reason": "access ended"
            }))
            .is_ok()
        );
    }
}
