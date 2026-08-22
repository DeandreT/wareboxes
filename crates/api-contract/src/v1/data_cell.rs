use serde::{Deserialize, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataCellMode {
    Shared,
    Dedicated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataCellStatus {
    Provisioning,
    Active,
    Draining,
    Retired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterDataCellRequest {
    pub key: String,
    pub name: String,
    pub region: String,
    pub residency: String,
    pub mode: DataCellMode,
    pub max_tenants: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconfigureDataCellRequest {
    pub expected_revision: Revision,
    pub name: String,
    pub max_tenants: u32,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeDataCellStatusRequest {
    pub expected_revision: Revision,
    pub status: DataCellStatus,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataCellPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<DataCellStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataCellResponse {
    pub data_cell_id: i64,
    pub key: String,
    pub name: String,
    pub region: String,
    pub residency: String,
    pub mode: DataCellMode,
    pub status: DataCellStatus,
    pub revision: Revision,
    pub max_tenants: u32,
    pub placement_count: i64,
    pub reserved_inbound_move_count: i64,
    pub reserved_rollback_move_count: i64,
    pub available_tenant_slots: u32,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_by: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_reason: Option<String>,
}

pub type DataCellPage = CursorPage<DataCellResponse>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataCellEventPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataCellEventResponse {
    pub event_id: i64,
    pub data_cell_id: i64,
    pub action: String,
    pub cell_revision: Revision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_status: Option<DataCellStatus>,
    pub resulting_status: DataCellStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<i64>,
    pub occurred_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub evidence: serde_json::Value,
}

pub type DataCellEventPage = CursorPage<DataCellEventResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_contract_rejects_unknown_connection_secrets() {
        let json = r#"{"key":"us-west-a","name":"US West A","region":"us-west-2","residency":"US","mode":"shared","max_tenants":200,"database_url":"secret"}"#;
        assert!(serde_json::from_str::<RegisterDataCellRequest>(json).is_err());
    }
}
