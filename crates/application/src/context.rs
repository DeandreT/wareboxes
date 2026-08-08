use wareboxes_domain::{TenantId, UserId};

use crate::{ApplicationError, ApplicationResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandContext {
    pub tenant_id: TenantId,
    pub actor_id: UserId,
    pub request_id: String,
    pub idempotency_key: Option<String>,
}

impl CommandContext {
    pub fn require_actor(&self, tenant_id: TenantId, actor_id: UserId) -> ApplicationResult<()> {
        if self.tenant_id == tenant_id && self.actor_id == actor_id {
            Ok(())
        } else {
            Err(ApplicationError::Forbidden)
        }
    }
}
