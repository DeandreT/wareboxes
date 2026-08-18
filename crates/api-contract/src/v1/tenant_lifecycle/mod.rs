use serde::{Deserialize, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantStatus {
    Active,
    Suspended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateTenantRequest {
    pub slug: String,
    pub name: String,
    pub administrator_email: String,
    pub data_cell_id: i64,
    pub residency_requirement: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeTenantStatusRequest {
    pub expected_revision: Revision,
    pub status: TenantStatus,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantLifecyclePageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TenantStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantLifecycleResponse {
    pub tenant_id: i64,
    pub slug: String,
    pub name: String,
    pub status: TenantStatus,
    pub revision: Revision,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_admin_user_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_admin_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_changed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_changed_by: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_reason: Option<String>,
    pub active_member_count: i64,
    pub active_facility_count: i64,
    pub active_inventory_owner_count: i64,
    pub active_service_account_count: i64,
    pub data_cell_id: i64,
    pub data_cell_key: String,
    pub data_cell_name: String,
    pub data_cell_region: String,
    pub data_cell_residency: String,
    pub data_cell_mode: super::DataCellMode,
    pub placement_revision: Revision,
    pub residency_requirement: String,
}

pub type TenantLifecyclePage = CursorPage<TenantLifecycleResponse>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantLifecycleEventPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantLifecycleEventResponse {
    pub event_id: i64,
    pub tenant_id: i64,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_status: Option<TenantStatus>,
    pub resulting_status: TenantStatus,
    pub tenant_revision: Revision,
    pub actor_id: i64,
    pub occurred_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub revoked_session_count: i64,
    pub revoked_credential_count: i64,
    pub evidence: serde_json::Value,
}

pub type TenantLifecycleEventPage = CursorPage<TenantLifecycleEventResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_creation_contract_rejects_unknown_credentials() {
        let request = CreateTenantRequest {
            slug: "northwest-3pl".into(),
            name: "Northwest 3PL".into(),
            administrator_email: "tenant-admin@example.test".into(),
            data_cell_id: 1,
            residency_requirement: "US".into(),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<CreateTenantRequest>(&json).unwrap(),
            request
        );
        assert!(serde_json::from_str::<CreateTenantRequest>(
            r#"{"slug":"northwest","name":"Northwest","administrator_email":"admin@example.test","password":"forbidden"}"#
        )
        .is_err());
    }
}
