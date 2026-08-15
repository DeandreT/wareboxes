//! Replay-safe employee-to-interactive-user identity commands.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    EmployeeId, EmployeeIdentityChangeId, EmployeeIdentityChangeKind, EmployeeIdentityReason,
    Timestamp, UserId,
};

pub const LINK_EMPLOYEE_IDENTITY_OPERATION: &str = "workforce.employee_identity.link.v1";
pub const UNLINK_EMPLOYEE_IDENTITY_OPERATION: &str = "workforce.employee_identity.unlink.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkEmployeeIdentityCommand {
    pub employee_id: EmployeeId,
    pub user_id: UserId,
    /// The currently observed identity. `None` means the employee must be unlinked.
    pub expected_user_id: Option<UserId>,
    pub reason: EmployeeIdentityReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnlinkEmployeeIdentityCommand {
    pub employee_id: EmployeeId,
    pub expected_user_id: UserId,
    pub reason: EmployeeIdentityReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmployeeIdentityChangeResult {
    pub change_id: EmployeeIdentityChangeId,
    pub employee_id: EmployeeId,
    pub previous_user_id: Option<UserId>,
    pub user_id: Option<UserId>,
    pub kind: EmployeeIdentityChangeKind,
    pub reason: EmployeeIdentityReason,
    pub changed_by: UserId,
    pub changed_at: Timestamp,
    pub resulting_revision: i64,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn link_hash_shape_includes_expected_identity_and_reason() {
        let command = LinkEmployeeIdentityCommand {
            employee_id: EmployeeId::new(7).unwrap(),
            user_id: UserId::new(9).unwrap(),
            expected_user_id: Some(UserId::new(8).unwrap()),
            reason: EmployeeIdentityReason::new("account replacement").unwrap(),
        };
        assert_eq!(
            serde_json::to_value(command).unwrap(),
            json!({
                "employee_id": 7,
                "user_id": 9,
                "expected_user_id": 8,
                "reason": "account replacement"
            })
        );
    }
}
