use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    SupportAccessGrantId, SupportAccessPolicy, SupportAccessRevision, SupportAccessStatus,
    TenantId, Timestamp, UserId,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportAccessReadModel {
    pub support_access_grant_id: SupportAccessGrantId,
    pub tenant_id: TenantId,
    pub tenant_slug: String,
    pub tenant_name: String,
    pub status: SupportAccessStatus,
    pub revision: SupportAccessRevision,
    pub reason: String,
    pub access: SupportAccessPolicy,
    pub requested_at: Timestamp,
    pub requested_by: UserId,
    pub requested_by_email: String,
    pub expires_at: Timestamp,
    pub approved_at: Option<Timestamp>,
    pub approved_by: Option<UserId>,
    pub approved_by_email: Option<String>,
    pub rejected_at: Option<Timestamp>,
    pub rejected_by: Option<UserId>,
    pub rejection_reason: Option<String>,
    pub revoked_at: Option<Timestamp>,
    pub revoked_by: Option<UserId>,
    pub revocation_reason: Option<String>,
}

pub type RequestSupportAccessResult = SupportAccessReadModel;
pub type ApproveSupportAccessResult = SupportAccessReadModel;
pub type RejectSupportAccessResult = SupportAccessReadModel;
pub type RevokeSupportAccessResult = SupportAccessReadModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportAccessCursor {
    pub after_requested_at: Timestamp,
    pub after_support_access_grant_id: SupportAccessGrantId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportAccessPageQuery {
    pub tenant_id: Option<TenantId>,
    pub status: Option<SupportAccessStatus>,
    pub cursor: Option<SupportAccessCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportAccessPage {
    pub items: Vec<SupportAccessReadModel>,
    pub next_cursor: Option<SupportAccessCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportAccessResourceOption {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportAccessOptionsReadModel {
    pub tenant_id: TenantId,
    pub tenant_name: String,
    pub facilities: Vec<SupportAccessResourceOption>,
    pub inventory_owners: Vec<SupportAccessResourceOption>,
    pub permission_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportAccessEventReadModel {
    pub event_id: i64,
    pub support_access_grant_id: SupportAccessGrantId,
    pub tenant_id: TenantId,
    pub action: String,
    pub grant_revision: SupportAccessRevision,
    pub actor_id: UserId,
    pub occurred_at: Timestamp,
    pub reason: Option<String>,
    pub evidence: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportAccessEventCursor {
    pub after_occurred_at: Timestamp,
    pub after_event_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportAccessEventPageQuery {
    pub support_access_grant_id: SupportAccessGrantId,
    pub cursor: Option<SupportAccessEventCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportAccessEventPage {
    pub items: Vec<SupportAccessEventReadModel>,
    pub next_cursor: Option<SupportAccessEventCursor>,
}
