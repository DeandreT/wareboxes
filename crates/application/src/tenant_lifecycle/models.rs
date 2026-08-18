use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    DataCellId, DataCellMode, DataCellPlacementRevision, TenantId, TenantRevision, TenantStatus,
    Timestamp, UserId,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantLifecycleReadModel {
    pub tenant_id: TenantId,
    pub slug: String,
    pub name: String,
    pub status: TenantStatus,
    pub revision: TenantRevision,
    pub created_at: Timestamp,
    pub created_by: Option<UserId>,
    pub initial_admin_user_id: Option<UserId>,
    pub initial_admin_email: Option<String>,
    pub status_changed_at: Option<Timestamp>,
    pub status_changed_by: Option<UserId>,
    pub status_reason: Option<String>,
    pub active_member_count: i64,
    pub active_facility_count: i64,
    pub active_inventory_owner_count: i64,
    pub active_service_account_count: i64,
    pub data_cell_id: DataCellId,
    pub data_cell_key: String,
    pub data_cell_name: String,
    pub data_cell_region: String,
    pub data_cell_residency: String,
    pub data_cell_mode: DataCellMode,
    pub placement_revision: DataCellPlacementRevision,
    pub residency_requirement: String,
}

pub type CreateTenantResult = TenantLifecycleReadModel;
pub type ChangeTenantStatusResult = TenantLifecycleReadModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantLifecycleCursor {
    pub after_created_at: Timestamp,
    pub after_tenant_id: TenantId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantLifecyclePageQuery {
    pub status: Option<TenantStatus>,
    pub search: Option<String>,
    pub cursor: Option<TenantLifecycleCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantLifecyclePage {
    pub items: Vec<TenantLifecycleReadModel>,
    pub next_cursor: Option<TenantLifecycleCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantLifecycleEventReadModel {
    pub event_id: i64,
    pub tenant_id: TenantId,
    pub action: String,
    pub previous_status: Option<TenantStatus>,
    pub resulting_status: TenantStatus,
    pub tenant_revision: TenantRevision,
    pub actor_id: UserId,
    pub occurred_at: Timestamp,
    pub reason: Option<String>,
    pub revoked_session_count: i64,
    pub revoked_credential_count: i64,
    pub evidence: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantLifecycleEventCursor {
    pub after_occurred_at: Timestamp,
    pub after_event_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantLifecycleEventPageQuery {
    pub tenant_id: TenantId,
    pub cursor: Option<TenantLifecycleEventCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantLifecycleEventPage {
    pub items: Vec<TenantLifecycleEventReadModel>,
    pub next_cursor: Option<TenantLifecycleEventCursor>,
}
