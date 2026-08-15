use serde::Serialize;
use wareboxes_domain::{
    ServiceAccountAccessPolicy, ServiceAccountBearerToken, ServiceAccountCredentialId,
    ServiceAccountCredentialLabel, ServiceAccountDescription, ServiceAccountId, ServiceAccountName,
    ServiceAccountReason, ServiceAccountRevision, ServiceAccountStatus, TenantId, Timestamp,
};

pub const CREATE_SERVICE_ACCOUNT_OPERATION: &str = "identity.service_account.create.v1";
pub const UPDATE_SERVICE_ACCOUNT_ACCESS_OPERATION: &str =
    "identity.service_account.access.update.v1";
pub const CHANGE_SERVICE_ACCOUNT_STATUS_OPERATION: &str =
    "identity.service_account.status.change.v1";
pub const REVOKE_SERVICE_ACCOUNT_CREDENTIAL_OPERATION: &str =
    "identity.service_account.credential.revoke.v1";
pub const ISSUE_SERVICE_ACCOUNT_CREDENTIAL_OPERATION: &str =
    "identity.service_account.credential.issue.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateServiceAccountCommand {
    pub tenant_id: TenantId,
    pub name: ServiceAccountName,
    pub description: Option<ServiceAccountDescription>,
    pub access: ServiceAccountAccessPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateServiceAccountAccessCommand {
    pub service_account_id: ServiceAccountId,
    pub expected_revision: ServiceAccountRevision,
    pub access: ServiceAccountAccessPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChangeServiceAccountStatusCommand {
    pub service_account_id: ServiceAccountId,
    pub expected_revision: ServiceAccountRevision,
    pub status: ServiceAccountStatus,
    pub reason: ServiceAccountReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IssueServiceAccountCredentialCommand {
    pub service_account_id: ServiceAccountId,
    pub expected_revision: ServiceAccountRevision,
    pub label: ServiceAccountCredentialLabel,
    pub expires_at: Option<Timestamp>,
    pub bearer_token: ServiceAccountBearerToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RevokeServiceAccountCredentialCommand {
    pub service_account_id: ServiceAccountId,
    pub credential_id: ServiceAccountCredentialId,
    pub expected_revision: ServiceAccountRevision,
    pub reason: ServiceAccountReason,
}
