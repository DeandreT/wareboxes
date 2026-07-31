use wareboxes_domain::{TenantId, UserId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandContext {
    pub tenant_id: TenantId,
    pub actor_id: UserId,
    pub request_id: String,
    pub idempotency_key: Option<String>,
}
