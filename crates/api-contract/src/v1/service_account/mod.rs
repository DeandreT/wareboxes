use serde::{Deserialize, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceAccountStatus {
    Active,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceAccountAccessRequest {
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
pub struct CreateServiceAccountRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub access: ServiceAccountAccessRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateServiceAccountAccessRequest {
    pub expected_revision: Revision,
    pub access: ServiceAccountAccessRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeServiceAccountStatusRequest {
    pub expected_revision: Revision,
    pub status: ServiceAccountStatus,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueServiceAccountCredentialRequest {
    pub expected_revision: Revision,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub bearer_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeServiceAccountCredentialRequest {
    pub expected_revision: Revision,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceAccountPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ServiceAccountStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceAccountCredentialResponse {
    pub credential_id: i64,
    pub label: String,
    pub token_prefix: String,
    pub created_at: String,
    pub created_by: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_by: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revocation_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceAccountResponse {
    pub service_account_id: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub status: ServiceAccountStatus,
    pub revision: Revision,
    pub access: ServiceAccountAccessRequest,
    pub created_at: String,
    pub created_by: i64,
    pub updated_at: String,
    pub updated_by: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_by: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    pub credentials: Vec<ServiceAccountCredentialResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssuedServiceAccountCredentialResponse {
    pub service_account: ServiceAccountResponse,
    pub credential: ServiceAccountCredentialResponse,
}

pub type ServiceAccountPage = CursorPage<ServiceAccountResponse>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceAccountOptionsResponse {
    pub permission_names: Vec<String>,
    pub can_delegate_all_facilities: bool,
    pub can_delegate_all_inventory_owners: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceAccountEventPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceAccountEventResponse {
    pub event_id: i64,
    pub service_account_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<i64>,
    pub action: String,
    pub account_revision: Revision,
    pub actor_id: i64,
    pub occurred_at: String,
    pub evidence: serde_json::Value,
}

pub type ServiceAccountEventPage = CursorPage<ServiceAccountEventResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_account_create_contract_is_exact() {
        let request = CreateServiceAccountRequest {
            name: "ERP order intake".into(),
            description: None,
            access: ServiceAccountAccessRequest {
                all_facilities: false,
                facility_ids: vec![5],
                all_inventory_owners: false,
                inventory_owner_ids: vec![7],
                permission_names: vec!["orders".into()],
            },
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            json,
            r#"{"name":"ERP order intake","access":{"all_facilities":false,"facility_ids":[5],"all_inventory_owners":false,"inventory_owner_ids":[7],"permission_names":["orders"]}}"#
        );
        assert_eq!(
            serde_json::from_str::<CreateServiceAccountRequest>(&json).unwrap(),
            request
        );
        assert!(serde_json::from_str::<CreateServiceAccountRequest>(
            r#"{"name":"ERP","access":{"facility_ids":[5],"inventory_owner_ids":[7],"permission_names":["orders"]},"password":"forbidden"}"#
        )
        .is_err());
    }
}
