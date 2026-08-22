use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    DataCellCapacity, DataCellId, DataCellKey, DataCellMode, DataCellName,
    DataCellPlacementRevision, DataCellReason, DataCellRegion, DataCellRevision, DataCellStatus,
    DataResidencyCode, TenantId, Timestamp, UserId,
};

pub const REGISTER_DATA_CELL_OPERATION: &str = "platform.data_cell.register.v1";
pub const RECONFIGURE_DATA_CELL_OPERATION: &str = "platform.data_cell.reconfigure.v1";
pub const CHANGE_DATA_CELL_STATUS_OPERATION: &str = "platform.data_cell.status.change.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegisterDataCellCommand {
    pub key: DataCellKey,
    pub name: DataCellName,
    pub region: DataCellRegion,
    pub residency: DataResidencyCode,
    pub mode: DataCellMode,
    pub max_tenants: DataCellCapacity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReconfigureDataCellCommand {
    pub data_cell_id: DataCellId,
    pub expected_revision: DataCellRevision,
    pub name: DataCellName,
    pub max_tenants: DataCellCapacity,
    pub reason: DataCellReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChangeDataCellStatusCommand {
    pub data_cell_id: DataCellId,
    pub expected_revision: DataCellRevision,
    pub status: DataCellStatus,
    pub reason: DataCellReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataCellReadModel {
    pub data_cell_id: DataCellId,
    pub key: String,
    pub name: String,
    pub region: String,
    pub residency: String,
    pub mode: DataCellMode,
    pub status: DataCellStatus,
    pub revision: DataCellRevision,
    pub max_tenants: u32,
    pub placement_count: i64,
    pub reserved_inbound_move_count: i64,
    pub reserved_rollback_move_count: i64,
    pub created_at: Timestamp,
    pub created_by: Option<UserId>,
    pub changed_at: Option<Timestamp>,
    pub changed_by: Option<UserId>,
    pub change_reason: Option<String>,
}

pub type RegisterDataCellResult = DataCellReadModel;
pub type ReconfigureDataCellResult = DataCellReadModel;
pub type ChangeDataCellStatusResult = DataCellReadModel;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataCellPageQuery {
    pub status: Option<DataCellStatus>,
    pub region: Option<String>,
    pub cursor: Option<DataCellCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataCellCursor {
    pub after_created_at: Timestamp,
    pub after_data_cell_id: DataCellId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataCellPage {
    pub items: Vec<DataCellReadModel>,
    pub next_cursor: Option<DataCellCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataCellEventReadModel {
    pub event_id: i64,
    pub data_cell_id: DataCellId,
    pub action: String,
    pub cell_revision: DataCellRevision,
    pub previous_status: Option<DataCellStatus>,
    pub resulting_status: DataCellStatus,
    pub actor_id: Option<UserId>,
    pub occurred_at: Timestamp,
    pub reason: Option<String>,
    pub evidence: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataCellEventCursor {
    pub after_occurred_at: Timestamp,
    pub after_event_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataCellEventPageQuery {
    pub data_cell_id: DataCellId,
    pub cursor: Option<DataCellEventCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataCellEventPage {
    pub items: Vec<DataCellEventReadModel>,
    pub next_cursor: Option<DataCellEventCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantCellPlacementReadModel {
    pub tenant_id: TenantId,
    pub data_cell_id: DataCellId,
    pub cell_key: String,
    pub cell_name: String,
    pub cell_region: String,
    pub cell_residency: String,
    pub cell_mode: DataCellMode,
    pub placement_revision: DataCellPlacementRevision,
    pub residency_requirement: String,
    pub placed_at: Timestamp,
    pub placed_by: Option<UserId>,
}
