use serde::Serialize;
use wareboxes_domain::{
    DataCellId, DataResidencyCode, TenantId, TenantLifecycleReason, TenantName, TenantRevision,
    TenantSlug, TenantStatus,
};

pub const CREATE_TENANT_OPERATION: &str = "platform.tenant.create.v1";
pub const CHANGE_TENANT_STATUS_OPERATION: &str = "platform.tenant.status.change.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateTenantCommand {
    pub slug: TenantSlug,
    pub name: TenantName,
    pub administrator_email: String,
    pub data_cell_id: DataCellId,
    pub residency_requirement: DataResidencyCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChangeTenantStatusCommand {
    pub tenant_id: TenantId,
    pub expected_revision: TenantRevision,
    pub status: TenantStatus,
    pub reason: TenantLifecycleReason,
}
