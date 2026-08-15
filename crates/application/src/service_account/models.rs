use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    ServiceAccountAccessPolicy, ServiceAccountCredentialId, ServiceAccountId,
    ServiceAccountRevision, ServiceAccountStatus, TenantId, Timestamp, UserId,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceAccountCredentialReadModel {
    pub credential_id: ServiceAccountCredentialId,
    pub label: String,
    pub token_prefix: String,
    pub created_at: Timestamp,
    pub created_by: UserId,
    pub expires_at: Option<Timestamp>,
    pub revoked_at: Option<Timestamp>,
    pub revoked_by: Option<UserId>,
    pub revocation_reason: Option<String>,
    pub last_used_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceAccountReadModel {
    pub service_account_id: ServiceAccountId,
    pub tenant_id: TenantId,
    pub name: String,
    pub description: Option<String>,
    pub status: ServiceAccountStatus,
    pub revision: ServiceAccountRevision,
    pub access: ServiceAccountAccessPolicy,
    pub created_at: Timestamp,
    pub created_by: UserId,
    pub updated_at: Timestamp,
    pub updated_by: UserId,
    pub disabled_at: Option<Timestamp>,
    pub disabled_by: Option<UserId>,
    pub disabled_reason: Option<String>,
    pub last_used_at: Option<Timestamp>,
    pub credentials: Vec<ServiceAccountCredentialReadModel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuedServiceAccountCredential {
    pub service_account: ServiceAccountReadModel,
    pub credential: ServiceAccountCredentialReadModel,
}

pub type CreateServiceAccountResult = ServiceAccountReadModel;
pub type UpdateServiceAccountAccessResult = ServiceAccountReadModel;
pub type ChangeServiceAccountStatusResult = ServiceAccountReadModel;
pub type RevokeServiceAccountCredentialResult = ServiceAccountReadModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceAccountCursor {
    pub after_created_at: Timestamp,
    pub after_service_account_id: ServiceAccountId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceAccountPageQuery {
    pub status: Option<ServiceAccountStatus>,
    pub cursor: Option<ServiceAccountCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceAccountPage {
    pub items: Vec<ServiceAccountReadModel>,
    pub next_cursor: Option<ServiceAccountCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceAccountEventReadModel {
    pub event_id: i64,
    pub service_account_id: ServiceAccountId,
    pub credential_id: Option<ServiceAccountCredentialId>,
    pub action: String,
    pub account_revision: ServiceAccountRevision,
    pub actor_id: UserId,
    pub occurred_at: Timestamp,
    pub evidence: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceAccountEventCursor {
    pub after_occurred_at: Timestamp,
    pub after_event_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceAccountEventPageQuery {
    pub service_account_id: ServiceAccountId,
    pub cursor: Option<ServiceAccountEventCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceAccountEventPage {
    pub items: Vec<ServiceAccountEventReadModel>,
    pub next_cursor: Option<ServiceAccountEventCursor>,
}
