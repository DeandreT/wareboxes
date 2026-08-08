//! Pure command idempotency identities, fingerprints, and replay decisions.

use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};
use wareboxes_domain::{TenantId, UserId};

use crate::{ApplicationError, ApplicationResult, CommandContext};

const MAX_IDEMPOTENCY_KEY_BYTES: usize = 200;
const REQUEST_HASH_BYTES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSchemaVersion {
    request: u32,
    result: u32,
}

impl CommandSchemaVersion {
    pub const V1: Self = Self {
        request: 1,
        result: 1,
    };

    pub fn new(request: u32, result: u32) -> ApplicationResult<Self> {
        if request == 0 || result == 0 {
            return Err(ApplicationError::Internal(
                "command schema versions must be positive".into(),
            ));
        }
        Ok(Self { request, result })
    }

    pub const fn request(self) -> u32 {
        self.request
    }

    pub const fn result(self) -> u32 {
        self.result
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOperation(String);

impl CommandOperation {
    pub fn new(value: impl Into<String>) -> ApplicationResult<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ApplicationError::Internal(
                "command operation cannot be blank".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub fn new(value: impl Into<String>) -> ApplicationResult<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ApplicationError::IdempotencyKeyRequired);
        }
        if value.len() > MAX_IDEMPOTENCY_KEY_BYTES {
            return Err(ApplicationError::InvalidRequest(
                "idempotency key cannot exceed 200 characters".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRequestHash(String);

impl CommandRequestHash {
    pub fn from_stored(value: impl Into<String>) -> ApplicationResult<Self> {
        let value = value.into();
        if value.len() != REQUEST_HASH_BYTES * 2
            || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(ApplicationError::Internal(
                "stored command request hash is invalid".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn command_request_hash<T: Serialize>(
    actor_id: UserId,
    operation: &CommandOperation,
    schema: CommandSchemaVersion,
    request: &T,
) -> ApplicationResult<CommandRequestHash> {
    let encoded = serde_json::to_vec(request).map_err(|error| {
        ApplicationError::Internal(format!("serializing command request: {error}"))
    })?;
    let operation_length = u64::try_from(operation.as_str().len())
        .map_err(|_| ApplicationError::Internal("command operation length exceeds u64".into()))?;

    let mut hasher = Sha256::new();
    hasher.update(b"wareboxes-command-request-v1\0");
    hasher.update(operation_length.to_be_bytes());
    hasher.update(operation.as_str().as_bytes());
    hasher.update(actor_id.get().to_be_bytes());
    hasher.update(schema.request().to_be_bytes());
    hasher.update(encoded);
    Ok(CommandRequestHash(hex::encode(hasher.finalize())))
}

#[derive(Debug, Clone)]
pub struct PreparedCommand {
    tenant_id: TenantId,
    actor_id: UserId,
    request_id: Option<String>,
    operation: CommandOperation,
    idempotency_key: IdempotencyKey,
    request_hash: CommandRequestHash,
    schema: CommandSchemaVersion,
}

impl PreparedCommand {
    pub fn new_v1<T: Serialize>(
        context: &CommandContext,
        operation: impl Into<String>,
        request: &T,
    ) -> ApplicationResult<Self> {
        Self::new(context, operation, CommandSchemaVersion::V1, request)
    }

    pub fn new<T: Serialize>(
        context: &CommandContext,
        operation: impl Into<String>,
        schema: CommandSchemaVersion,
        request: &T,
    ) -> ApplicationResult<Self> {
        let operation = CommandOperation::new(operation)?;
        let idempotency_key = context
            .idempotency_key
            .as_deref()
            .ok_or(ApplicationError::IdempotencyKeyRequired)
            .and_then(IdempotencyKey::new)?;
        let request_hash = command_request_hash(context.actor_id, &operation, schema, request)?;
        Ok(Self {
            tenant_id: context.tenant_id,
            actor_id: context.actor_id,
            request_id: Some(context.request_id.clone()),
            operation,
            idempotency_key,
            request_hash,
            schema,
        })
    }

    pub fn from_parts_v1<T: Serialize>(
        tenant_id: TenantId,
        actor_id: UserId,
        request_id: Option<&str>,
        idempotency_key: &str,
        operation: impl Into<String>,
        request: &T,
    ) -> ApplicationResult<Self> {
        let operation = CommandOperation::new(operation)?;
        let idempotency_key = IdempotencyKey::new(idempotency_key)?;
        let schema = CommandSchemaVersion::V1;
        let request_hash = command_request_hash(actor_id, &operation, schema, request)?;
        Ok(Self {
            tenant_id,
            actor_id,
            request_id: request_id.map(str::to_owned),
            operation,
            idempotency_key,
            request_hash,
            schema,
        })
    }

    pub fn tenant_id(&self) -> TenantId {
        self.tenant_id
    }

    pub fn actor_id(&self) -> UserId {
        self.actor_id
    }

    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    pub fn operation(&self) -> &CommandOperation {
        &self.operation
    }

    pub fn idempotency_key_value(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    pub fn idempotency_key(&self) -> &str {
        self.idempotency_key.as_str()
    }

    pub fn request_hash_value(&self) -> &CommandRequestHash {
        &self.request_hash
    }

    pub fn request_hash(&self) -> &str {
        self.request_hash.as_str()
    }

    pub fn schema(&self) -> CommandSchemaVersion {
        self.schema
    }

    pub fn resolve_replay<T: DeserializeOwned>(
        &self,
        stored: StoredCommandResult,
    ) -> ApplicationResult<T> {
        if stored.actor_id != self.actor_id || stored.request_hash != self.request_hash {
            return Err(ApplicationError::IdempotencyKeyReused);
        }
        if stored.result_schema_version != self.schema.result() {
            return Err(ApplicationError::Internal(format!(
                "stored command result schema version {} does not match expected version {}",
                stored.result_schema_version,
                self.schema.result()
            )));
        }
        serde_json::from_value(stored.result_json).map_err(|error| {
            ApplicationError::Internal(format!("decoding stored command result: {error}"))
        })
    }

    pub fn completed_result<T: Serialize>(
        &self,
        result: &T,
        inventory_transaction_id: Option<i64>,
    ) -> ApplicationResult<NewCommandResult> {
        let result_json = serde_json::to_value(result).map_err(|error| {
            ApplicationError::Internal(format!("encoding command result: {error}"))
        })?;
        Ok(NewCommandResult {
            tenant_id: self.tenant_id,
            actor_id: self.actor_id,
            request_id: self.request_id.clone(),
            operation: self.operation.clone(),
            idempotency_key: self.idempotency_key.clone(),
            request_hash: self.request_hash.clone(),
            result_json,
            result_schema_version: self.schema.result(),
            inventory_transaction_id,
        })
    }
}

#[derive(Debug, Clone)]
pub struct StoredCommandResult {
    request_hash: CommandRequestHash,
    result_json: serde_json::Value,
    actor_id: UserId,
    result_schema_version: u32,
    inventory_transaction_id: Option<i64>,
}

impl StoredCommandResult {
    pub fn from_persisted(
        request_hash: CommandRequestHash,
        result_json: serde_json::Value,
        actor_id: UserId,
        result_schema_version: u32,
        inventory_transaction_id: Option<i64>,
    ) -> ApplicationResult<Self> {
        if result_schema_version == 0 {
            return Err(ApplicationError::Internal(
                "stored command result schema version is invalid".into(),
            ));
        }
        if inventory_transaction_id.is_some_and(|transaction_id| transaction_id <= 0) {
            return Err(ApplicationError::Internal(
                "stored command inventory transaction is invalid".into(),
            ));
        }
        Ok(Self {
            request_hash,
            result_json,
            actor_id,
            result_schema_version,
            inventory_transaction_id,
        })
    }

    pub const fn inventory_transaction_id(&self) -> Option<i64> {
        self.inventory_transaction_id
    }
}

#[derive(Debug, Clone)]
pub struct NewCommandResult {
    tenant_id: TenantId,
    actor_id: UserId,
    request_id: Option<String>,
    operation: CommandOperation,
    idempotency_key: IdempotencyKey,
    request_hash: CommandRequestHash,
    result_json: serde_json::Value,
    result_schema_version: u32,
    inventory_transaction_id: Option<i64>,
}

impl NewCommandResult {
    pub const fn tenant_id(&self) -> TenantId {
        self.tenant_id
    }

    pub const fn actor_id(&self) -> UserId {
        self.actor_id
    }

    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    pub fn operation(&self) -> &CommandOperation {
        &self.operation
    }

    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    pub fn request_hash(&self) -> &CommandRequestHash {
        &self.request_hash
    }

    pub fn result_json(&self) -> &serde_json::Value {
        &self.result_json
    }

    pub const fn result_schema_version(&self) -> u32 {
        self.result_schema_version
    }

    pub const fn inventory_transaction_id(&self) -> Option<i64> {
        self.inventory_transaction_id
    }
}

#[cfg(test)]
mod tests {
    use wareboxes_domain::{TenantId, UserId};

    use super::*;

    fn context(actor_id: i64, key: Option<&str>) -> CommandContext {
        CommandContext {
            tenant_id: TenantId::new(1).unwrap(),
            actor_id: UserId::new(actor_id).unwrap(),
            request_id: "request-1".into(),
            idempotency_key: key.map(str::to_owned),
        }
    }

    #[test]
    fn command_hash_is_stable_and_separates_actor_operation_and_schema() {
        let first = PreparedCommand::new(
            &context(1, Some("key")),
            "inventory.move.v1",
            CommandSchemaVersion::V1,
            &(12_i64, 4_i64),
        )
        .unwrap();
        let same = PreparedCommand::new(
            &context(1, Some("key")),
            "inventory.move.v1",
            CommandSchemaVersion::V1,
            &(12_i64, 4_i64),
        )
        .unwrap();
        let other_actor = PreparedCommand::new(
            &context(2, Some("key")),
            "inventory.move.v1",
            CommandSchemaVersion::V1,
            &(12_i64, 4_i64),
        )
        .unwrap();
        let other_operation = PreparedCommand::new(
            &context(1, Some("key")),
            "inventory.adjust.v1",
            CommandSchemaVersion::V1,
            &(12_i64, 4_i64),
        )
        .unwrap();
        let other_schema = PreparedCommand::new(
            &context(1, Some("key")),
            "inventory.move.v1",
            CommandSchemaVersion::new(2, 1).unwrap(),
            &(12_i64, 4_i64),
        )
        .unwrap();

        assert_eq!(first.request_hash(), same.request_hash());
        assert_ne!(first.request_hash(), other_actor.request_hash());
        assert_ne!(first.request_hash(), other_operation.request_hash());
        assert_ne!(first.request_hash(), other_schema.request_hash());
    }

    #[test]
    fn replay_rejects_actor_payload_and_result_schema_mismatches() {
        let prepared = PreparedCommand::new(
            &context(1, Some("key")),
            "inventory.move.v1",
            CommandSchemaVersion::V1,
            &12_i64,
        )
        .unwrap();
        let different_request = PreparedCommand::new(
            &context(1, Some("key")),
            "inventory.move.v1",
            CommandSchemaVersion::V1,
            &13_i64,
        )
        .unwrap();
        let encoded = serde_json::json!({"ok": true});

        let actor_error = prepared
            .resolve_replay::<serde_json::Value>(
                StoredCommandResult::from_persisted(
                    prepared.request_hash_value().clone(),
                    encoded.clone(),
                    UserId::new(2).unwrap(),
                    1,
                    None,
                )
                .unwrap(),
            )
            .unwrap_err();
        assert!(matches!(
            actor_error,
            ApplicationError::IdempotencyKeyReused
        ));

        let request_error = prepared
            .resolve_replay::<serde_json::Value>(
                StoredCommandResult::from_persisted(
                    different_request.request_hash_value().clone(),
                    encoded.clone(),
                    prepared.actor_id(),
                    1,
                    None,
                )
                .unwrap(),
            )
            .unwrap_err();
        assert!(matches!(
            request_error,
            ApplicationError::IdempotencyKeyReused
        ));

        let schema_error = prepared
            .resolve_replay::<serde_json::Value>(
                StoredCommandResult::from_persisted(
                    prepared.request_hash_value().clone(),
                    encoded,
                    prepared.actor_id(),
                    2,
                    None,
                )
                .unwrap(),
            )
            .unwrap_err();
        assert!(matches!(schema_error, ApplicationError::Internal(_)));
    }

    #[test]
    fn command_identity_validation_preserves_public_error_semantics() {
        let missing = PreparedCommand::new(
            &context(1, None),
            "inventory.move.v1",
            CommandSchemaVersion::V1,
            &(),
        )
        .unwrap_err();
        assert!(matches!(missing, ApplicationError::IdempotencyKeyRequired));

        let oversized = "x".repeat(MAX_IDEMPOTENCY_KEY_BYTES + 1);
        let invalid = PreparedCommand::new(
            &context(1, Some(&oversized)),
            "inventory.move.v1",
            CommandSchemaVersion::V1,
            &(),
        )
        .unwrap_err();
        assert!(matches!(invalid, ApplicationError::InvalidRequest(_)));
    }

    #[test]
    fn persisted_results_reject_invalid_schema_and_transaction_identities() {
        let prepared =
            PreparedCommand::new_v1(&context(1, Some("key")), "inventory.move.v1", &12_i64)
                .unwrap();
        let invalid_schema = StoredCommandResult::from_persisted(
            prepared.request_hash_value().clone(),
            serde_json::Value::Null,
            prepared.actor_id(),
            0,
            None,
        )
        .unwrap_err();
        assert!(matches!(invalid_schema, ApplicationError::Internal(_)));

        let invalid_transaction = StoredCommandResult::from_persisted(
            prepared.request_hash_value().clone(),
            serde_json::Value::Null,
            prepared.actor_id(),
            1,
            Some(0),
        )
        .unwrap_err();
        assert!(matches!(invalid_transaction, ApplicationError::Internal(_)));
    }
}
