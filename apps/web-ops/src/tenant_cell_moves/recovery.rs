use serde::{Deserialize, Serialize};

use super::PendingCommand;

const STORAGE_SCHEMA_VERSION: u8 = 1;
const STORAGE_KEY_PREFIX: &str = "wareboxes.platform.tenant-cell-moves.pending-command.v1.user";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RecoveryBinding {
    pub(super) user_id: i64,
    pub(super) control_tenant_id: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredPendingCommand {
    schema_version: u8,
    user_id: i64,
    pub(super) control_tenant_id: i64,
    pub(super) command: PendingCommand,
}

impl StoredPendingCommand {
    pub(super) fn new(binding: RecoveryBinding, command: PendingCommand) -> Self {
        Self {
            schema_version: STORAGE_SCHEMA_VERSION,
            user_id: binding.user_id,
            control_tenant_id: binding.control_tenant_id,
            command,
        }
    }
}

pub(super) fn load(user_id: i64) -> Result<Vec<StoredPendingCommand>, String> {
    validate_id(user_id, "user")?;
    let prefix = storage_prefix(user_id);
    let mut records = Vec::new();
    for (key, serialized) in browser::entries(&prefix)? {
        let record = decode_record(user_id, &serialized)?;
        if key != storage_key(user_id, record.command.idempotency_key()) {
            return Err(
                "A stored tenant-cell-move recovery does not match its idempotency-key slot."
                    .into(),
            );
        }
        records.push(record);
    }
    sort_records(&mut records);
    Ok(records)
}

pub(super) fn persist(binding: RecoveryBinding, command: &PendingCommand) -> Result<(), String> {
    validate_binding(binding)?;
    validate_command(command)?;
    let record = StoredPendingCommand::new(binding, command.clone());
    let key = storage_key(binding.user_id, command.idempotency_key());
    if let Some(serialized) = browser::read(&key)? {
        let existing = decode_record(binding.user_id, &serialized)?;
        if existing != record {
            return Err(
                "A different tenant-cell-move recovery already uses this idempotency key. Resolve it before sending the command."
                    .into(),
            );
        }
    }
    let serialized = encode_record(&record)?;
    browser::write(&key, &serialized)
}

pub(super) fn clear(binding: RecoveryBinding, command: &PendingCommand) -> Result<(), String> {
    validate_binding(binding)?;
    validate_command(command)?;
    let key = storage_key(binding.user_id, command.idempotency_key());
    let Some(serialized) = browser::read(&key)? else {
        return Ok(());
    };
    let existing = decode_record(binding.user_id, &serialized)?;
    let expected = StoredPendingCommand::new(binding, command.clone());
    if existing != expected {
        return Err(
            "The stored tenant-cell-move recovery changed in another tab and was not cleared."
                .into(),
        );
    }
    browser::remove(&key)
}

pub(super) fn merge_record(records: &mut Vec<StoredPendingCommand>, record: StoredPendingCommand) {
    if let Some(existing) = records
        .iter_mut()
        .find(|existing| existing.command.idempotency_key() == record.command.idempotency_key())
    {
        *existing = record;
    } else {
        records.push(record);
    }
    sort_records(records);
}

pub(super) fn remove_record(records: &mut Vec<StoredPendingCommand>, command: &PendingCommand) {
    records.retain(|record| record.command.idempotency_key() != command.idempotency_key());
}

fn sort_records(records: &mut [StoredPendingCommand]) {
    records.sort_by(|left, right| {
        left.command
            .idempotency_key()
            .cmp(right.command.idempotency_key())
    });
}

fn validate_binding(binding: RecoveryBinding) -> Result<(), String> {
    validate_id(binding.user_id, "user")?;
    validate_id(binding.control_tenant_id, "control tenant")
}

fn validate_command(command: &PendingCommand) -> Result<(), String> {
    if command.idempotency_key().is_empty() {
        Err("Cannot persist a command without an idempotency key.".into())
    } else {
        Ok(())
    }
}

fn validate_id(value: i64, label: &str) -> Result<(), String> {
    if value > 0 {
        Ok(())
    } else {
        Err(format!("Cannot bind exact retry to an invalid {label} ID."))
    }
}

fn storage_prefix(user_id: i64) -> String {
    format!("{STORAGE_KEY_PREFIX}-{user_id}.")
}

fn storage_key(user_id: i64, idempotency_key: &str) -> String {
    format!(
        "{}{}",
        storage_prefix(user_id),
        urlencoding::encode(idempotency_key)
    )
}

fn encode_record(record: &StoredPendingCommand) -> Result<String, String> {
    serde_json::to_string(record)
        .map_err(|_| "The exact-retry command could not be serialized safely.".into())
}

fn decode_record(user_id: i64, serialized: &str) -> Result<StoredPendingCommand, String> {
    let record = serde_json::from_str::<StoredPendingCommand>(serialized)
        .map_err(|_| "The stored tenant-cell-move recovery is unreadable.".to_owned())?;
    if record.schema_version != STORAGE_SCHEMA_VERSION {
        return Err("The stored tenant-cell-move recovery uses an unsupported version.".into());
    }
    if record.user_id != user_id {
        return Err("The stored tenant-cell-move recovery belongs to another user.".into());
    }
    validate_id(record.control_tenant_id, "control tenant")?;
    validate_command(&record.command)?;
    Ok(record)
}

#[cfg(target_arch = "wasm32")]
mod browser {
    fn storage() -> Result<web_sys::Storage, String> {
        web_sys::window()
            .ok_or_else(|| "Browser exact-retry storage is unavailable.".to_owned())?
            .local_storage()
            .map_err(|_| "Browser exact-retry storage could not be opened.".to_owned())?
            .ok_or_else(|| "Browser exact-retry storage is unsupported.".to_owned())
    }

    pub(super) fn entries(prefix: &str) -> Result<Vec<(String, String)>, String> {
        let storage = storage()?;
        let length = storage
            .length()
            .map_err(|_| "Browser exact-retry storage could not be enumerated.".to_owned())?;
        let mut entries = Vec::new();
        for index in 0..length {
            let Some(key) = storage
                .key(index)
                .map_err(|_| "Browser exact-retry storage could not be enumerated.".to_owned())?
            else {
                continue;
            };
            if !key.starts_with(prefix) {
                continue;
            }
            let Some(value) = storage
                .get_item(&key)
                .map_err(|_| "Browser exact-retry storage could not be read.".to_owned())?
            else {
                continue;
            };
            entries.push((key, value));
        }
        Ok(entries)
    }

    pub(super) fn read(key: &str) -> Result<Option<String>, String> {
        storage()?
            .get_item(key)
            .map_err(|_| "Browser exact-retry storage could not be read.".to_owned())
    }

    pub(super) fn write(key: &str, value: &str) -> Result<(), String> {
        storage()?
            .set_item(key, value)
            .map_err(|_| "Browser exact-retry storage could not be written.".to_owned())
    }

    pub(super) fn remove(key: &str) -> Result<(), String> {
        storage()?
            .remove_item(key)
            .map_err(|_| "Browser exact-retry storage could not be cleared.".to_owned())
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod browser {
    pub(super) fn entries(prefix: &str) -> Result<Vec<(String, String)>, String> {
        if prefix.is_empty() {
            Err("Browser exact-retry storage prefix is empty.".into())
        } else {
            Ok(Vec::new())
        }
    }

    pub(super) fn read(key: &str) -> Result<Option<String>, String> {
        if key.is_empty() {
            Err("Browser exact-retry storage key is empty.".into())
        } else {
            Ok(None)
        }
    }

    pub(super) fn write(key: &str, _value: &str) -> Result<(), String> {
        if key.is_empty() {
            Err("Browser exact-retry storage key is empty.".into())
        } else {
            Ok(())
        }
    }

    pub(super) fn remove(key: &str) -> Result<(), String> {
        if key.is_empty() {
            Err("Browser exact-retry storage key is empty.".into())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::{
        PlanTenantCellMoveRequest, Revision, RollbackTenantCellMoveRequest,
        TenantCellMoveCutoverVerificationEvidence, TenantCellMoveRollbackVerificationEvidence,
        VerifyTenantCellMoveCutoverRequest,
    };

    fn revision(value: i64) -> Revision {
        Revision::new(value).unwrap()
    }

    #[test]
    fn record_round_trip_preserves_command_body_key_and_control_tenant() {
        let binding = RecoveryBinding {
            user_id: 17,
            control_tenant_id: 23,
        };
        let commands = [
            PendingCommand::Plan(
                41,
                PlanTenantCellMoveRequest {
                    target_data_cell_id: 7,
                    expected_placement_revision: revision(3),
                    reason: "regional isolation".into(),
                },
                "plan-key".into(),
            ),
            PendingCommand::VerifyCutover(
                43,
                VerifyTenantCellMoveCutoverRequest {
                    expected_revision: revision(8),
                    verification: TenantCellMoveCutoverVerificationEvidence {
                        tool_version: "verifier-2".into(),
                        routing_reference: "route-change-43".into(),
                        observed_data_cell_id: 7,
                        observed_placement_revision: revision(4),
                        routing_verified: true,
                        target_read_verified: true,
                        write_fence_verified: true,
                        inventory_reconciled: true,
                        idempotency_verified: true,
                        outbox_verified: true,
                    },
                },
                "verify-key".into(),
            ),
            PendingCommand::Rollback(
                43,
                RollbackTenantCellMoveRequest {
                    expected_revision: revision(9),
                    verification: TenantCellMoveRollbackVerificationEvidence {
                        tool_version: "verifier-2".into(),
                        routing_reference: "route-change-44".into(),
                        observed_data_cell_id: 6,
                        expected_rollback_placement_revision: revision(5),
                        routing_verified: true,
                        source_read_verified: true,
                        write_fence_verified: true,
                        inventory_reconciled: true,
                        idempotency_verified: true,
                        outbox_verified: true,
                    },
                    reason: "target health regressed".into(),
                },
                "rollback-key".into(),
            ),
        ];

        for command in commands {
            let expected = StoredPendingCommand::new(binding, command);
            let encoded = encode_record(&expected).unwrap();
            let decoded = decode_record(binding.user_id, &encoded).unwrap();
            assert_eq!(decoded, expected);
            assert_eq!(decoded.control_tenant_id, binding.control_tenant_id);
        }
    }

    #[test]
    fn storage_key_is_user_scoped_and_record_rejects_the_wrong_user() {
        assert_ne!(storage_prefix(17), storage_prefix(18));
        assert_ne!(storage_key(17, "freeze/key"), storage_key(18, "freeze/key"));
        assert_ne!(storage_key(17, "freeze/key"), storage_key(17, "freeze key"));

        let record = StoredPendingCommand::new(
            RecoveryBinding {
                user_id: 17,
                control_tenant_id: 23,
            },
            PendingCommand::Freeze(
                43,
                wareboxes_api_contract::v1::FreezeTenantCellMoveRequest {
                    expected_revision: revision(7),
                },
                "freeze-key".into(),
            ),
        );
        let encoded = encode_record(&record).unwrap();
        assert!(decode_record(18, &encoded).is_err());
    }

    #[test]
    fn competing_tabs_keep_independent_commands_and_original_tenant_bindings() {
        let first = StoredPendingCommand::new(
            RecoveryBinding {
                user_id: 17,
                control_tenant_id: 23,
            },
            PendingCommand::Freeze(
                43,
                wareboxes_api_contract::v1::FreezeTenantCellMoveRequest {
                    expected_revision: revision(7),
                },
                "tab-a-key".into(),
            ),
        );
        let second = StoredPendingCommand::new(
            RecoveryBinding {
                user_id: 17,
                control_tenant_id: 29,
            },
            PendingCommand::Freeze(
                47,
                wareboxes_api_contract::v1::FreezeTenantCellMoveRequest {
                    expected_revision: revision(9),
                },
                "tab-b-key".into(),
            ),
        );

        assert_ne!(
            storage_key(17, first.command.idempotency_key()),
            storage_key(17, second.command.idempotency_key())
        );

        let mut records = Vec::new();
        merge_record(&mut records, second.clone());
        merge_record(&mut records, first.clone());
        assert_eq!(records, vec![first.clone(), second.clone()]);

        merge_record(&mut records, first.clone());
        assert_eq!(records.len(), 2);

        remove_record(&mut records, &first.command);
        assert_eq!(records, vec![second]);
    }

    #[test]
    fn unsupported_or_malformed_records_fail_closed() {
        let record = StoredPendingCommand::new(
            RecoveryBinding {
                user_id: 17,
                control_tenant_id: 23,
            },
            PendingCommand::Freeze(
                43,
                wareboxes_api_contract::v1::FreezeTenantCellMoveRequest {
                    expected_revision: revision(7),
                },
                "freeze-key".into(),
            ),
        );
        let mut value = serde_json::to_value(record).unwrap();
        value["schema_version"] = serde_json::json!(2);

        assert!(decode_record(17, &value.to_string()).is_err());
        assert!(decode_record(17, "not-json").is_err());
    }
}
