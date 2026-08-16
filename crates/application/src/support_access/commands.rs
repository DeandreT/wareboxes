use serde::Serialize;
use wareboxes_domain::{
    SupportAccessGrantId, SupportAccessPolicy, SupportAccessReason, SupportAccessRevision,
    TenantId, Timestamp,
};

pub const REQUEST_SUPPORT_ACCESS_OPERATION: &str = "platform.support_access.request.v1";
pub const APPROVE_SUPPORT_ACCESS_OPERATION: &str = "platform.support_access.approve.v1";
pub const REJECT_SUPPORT_ACCESS_OPERATION: &str = "platform.support_access.reject.v1";
pub const REVOKE_SUPPORT_ACCESS_OPERATION: &str = "platform.support_access.revoke.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequestSupportAccessCommand {
    pub tenant_id: TenantId,
    pub reason: SupportAccessReason,
    pub expires_at: Timestamp,
    pub access: SupportAccessPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApproveSupportAccessCommand {
    pub support_access_grant_id: SupportAccessGrantId,
    pub expected_revision: SupportAccessRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RejectSupportAccessCommand {
    pub support_access_grant_id: SupportAccessGrantId,
    pub expected_revision: SupportAccessRevision,
    pub reason: SupportAccessReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RevokeSupportAccessCommand {
    pub support_access_grant_id: SupportAccessGrantId,
    pub expected_revision: SupportAccessRevision,
    pub reason: SupportAccessReason,
}
