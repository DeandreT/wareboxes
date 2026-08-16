use serde::{Deserialize, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportAccessStatus {
    Pending,
    Active,
    Rejected,
    Revoked,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportAccessPolicyRequest {
    #[serde(default)]
    pub all_facilities: bool,
    #[serde(default)]
    pub facility_ids: Vec<i64>,
    #[serde(default)]
    pub all_inventory_owners: bool,
    #[serde(default)]
    pub inventory_owner_ids: Vec<i64>,
    pub permission_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestSupportAccessRequest {
    pub tenant_id: i64,
    pub reason: String,
    pub expires_at: String,
    pub access: SupportAccessPolicyRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApproveSupportAccessRequest {
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RejectSupportAccessRequest {
    pub expected_revision: Revision,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeSupportAccessRequest {
    pub expected_revision: Revision,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportAccessPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SupportAccessStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportAccessOptionsRequest {
    pub tenant_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportAccessResourceOptionResponse {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportAccessOptionsResponse {
    pub tenant_id: i64,
    pub tenant_name: String,
    pub facilities: Vec<SupportAccessResourceOptionResponse>,
    pub inventory_owners: Vec<SupportAccessResourceOptionResponse>,
    pub permission_names: Vec<String>,
    pub max_duration_hours: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportAccessResponse {
    pub support_access_grant_id: i64,
    pub tenant_id: i64,
    pub tenant_slug: String,
    pub tenant_name: String,
    pub status: SupportAccessStatus,
    pub revision: Revision,
    pub reason: String,
    pub access: SupportAccessPolicyRequest,
    pub requested_at: String,
    pub requested_by: i64,
    pub requested_by_email: String,
    pub expires_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_by_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejected_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejected_by: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_by: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revocation_reason: Option<String>,
}

pub type SupportAccessPage = CursorPage<SupportAccessResponse>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportAccessEventPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportAccessEventResponse {
    pub event_id: i64,
    pub support_access_grant_id: i64,
    pub tenant_id: i64,
    pub action: String,
    pub grant_revision: Revision,
    pub actor_id: i64,
    pub occurred_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub evidence: serde_json::Value,
}

pub type SupportAccessEventPage = CursorPage<SupportAccessEventResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_request_contract_is_exact() {
        let request = RequestSupportAccessRequest {
            tenant_id: 7,
            reason: "Investigate inventory reconciliation incident INC-42".into(),
            expires_at: "2026-08-16T22:00:00Z".into(),
            access: SupportAccessPolicyRequest {
                all_facilities: false,
                facility_ids: vec![3],
                all_inventory_owners: false,
                inventory_owner_ids: vec![4],
                permission_names: vec!["wms".into()],
            },
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<RequestSupportAccessRequest>(&json).unwrap(),
            request
        );
        assert!(
            serde_json::from_str::<RequestSupportAccessRequest>(&format!(
                "{}{}",
                json.trim_end_matches('}'),
                r#",\"admin\":true}"#
            ))
            .is_err()
        );
    }
}
