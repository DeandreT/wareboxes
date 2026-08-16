use super::*;
use crate::command::{DeviceCommand, ScaleCommand, COMMAND_SCHEMA_VERSION};
use crate::types::{CorrelationId, IdempotencyKey};

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-08-15T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn descriptor() -> DeviceDescriptor {
    DeviceDescriptor {
        tenant_id: TenantId::new("tenant-1").unwrap(),
        facility_id: FacilityId::new("facility-1").unwrap(),
        device_id: DeviceId::new("scale-1").unwrap(),
        class: DeviceClass::Scale,
        display_name: "Pack scale 1".into(),
    }
}

fn request(key: &str) -> CommandRequest {
    CommandRequest {
        schema_version: COMMAND_SCHEMA_VERSION,
        command_id: crate::types::CommandId::new(format!("command-{key}")).unwrap(),
        tenant_id: TenantId::new("tenant-1").unwrap(),
        facility_id: FacilityId::new("facility-1").unwrap(),
        device_id: DeviceId::new("scale-1").unwrap(),
        correlation_id: CorrelationId::new(format!("correlation-{key}")).unwrap(),
        idempotency_key: IdempotencyKey::new(key).unwrap(),
        recovery_policy: RecoveryPolicy::ManualReview,
        command: DeviceCommand::Scale(ScaleCommand::Tare),
    }
}

fn configured_store() -> EdgeStore {
    let mut store = EdgeStore::open_in_memory().unwrap();
    let actor = ActorId::new("operator-1").unwrap();
    store
        .register_device(descriptor(), &actor, "initial safe registration", now())
        .unwrap();
    store
        .change_control_mode(
            &descriptor().device_id,
            ControlAction::ResumeAutomation(
                crate::types::SafetyConfirmation::after_physical_safety_checklist(),
            ),
            &actor,
            "commissioning checklist complete",
            now(),
        )
        .unwrap();
    store
}

#[test]
fn exact_submission_replays_and_changed_identity_is_rejected() {
    let mut store = configured_store();
    let original = request("tare-1");
    assert!(!store.submit(original.clone(), now()).unwrap().is_replay());
    assert!(store.submit(original.clone(), now()).unwrap().is_replay());

    let mut changed = original;
    changed.command = DeviceCommand::Scale(ScaleCommand::ReadStableWeight {
        requested_unit: crate::command::WeightUnit::Gram,
        timeout_ms: std::num::NonZeroU32::new(1_000).unwrap(),
    });
    assert!(matches!(
        store.submit(changed, now()),
        Err(StoreError::IdentityConflict)
    ));
}

#[test]
fn disabled_device_quarantines_new_and_existing_commands() {
    let mut store = configured_store();
    let actor = ActorId::new("operator-1").unwrap();
    let accepted = store.submit(request("tare-1"), now()).unwrap();
    assert_eq!(accepted.record().state, CommandState::Queued);

    store
        .change_control_mode(
            &descriptor().device_id,
            ControlAction::EnterManualFallback,
            &actor,
            "conveyor guarding opened",
            now(),
        )
        .unwrap();
    assert_eq!(
        store.command("command-tare-1").unwrap().state,
        CommandState::ManualReview
    );
    assert_eq!(
        store
            .submit(request("tare-2"), now())
            .unwrap()
            .record()
            .state,
        CommandState::ManualReview
    );
}

#[test]
fn manual_resolution_is_terminal_and_audited() {
    let mut store = configured_store();
    let actor = ActorId::new("operator-1").unwrap();
    store.submit(request("tare-1"), now()).unwrap();
    store
        .change_control_mode(
            &descriptor().device_id,
            ControlAction::Disable,
            &actor,
            "maintenance lockout",
            now(),
        )
        .unwrap();
    let resolved = store
        .resolve_manually(
            "command-tare-1",
            &actor,
            "weight was verified on backup scale",
            now(),
        )
        .unwrap();
    assert_eq!(resolved.state, CommandState::ResolvedManually);
    assert_eq!(store.command_events("command-tare-1").unwrap().len(), 3);
    assert_eq!(
        store.control_events(&descriptor().device_id).unwrap().len(),
        3
    );
}

#[test]
fn immutable_command_and_audit_content_rejects_direct_tampering() {
    let mut store = configured_store();
    store.submit(request("tamper-1"), now()).unwrap();
    assert!(store
        .connection
        .execute(
            "UPDATE edge_commands SET request_json = X'00' WHERE command_id = 'command-tamper-1'",
            [],
        )
        .is_err());
    assert!(store
        .connection
        .execute(
            "DELETE FROM edge_command_events WHERE command_id = 'command-tamper-1'",
            [],
        )
        .is_err());
    assert_eq!(
        store.command("command-tamper-1").unwrap().state,
        CommandState::Queued
    );
}

#[cfg(unix)]
#[test]
fn persistent_database_is_owner_readable_and_writable_only() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("edge.sqlite3");
    drop(EdgeStore::open(&path).unwrap());
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn version_one_store_is_migrated_in_place() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("edge-v1.sqlite3");
    let schema = include_str!("schema.sql");
    let (version_one_schema, _) = schema
        .split_once("CREATE TABLE edge_cloud_deliveries")
        .unwrap();
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch(version_one_schema).unwrap();
    connection.pragma_update(None, "user_version", 1).unwrap();
    drop(connection);

    let mut store = EdgeStore::open(&path).unwrap();
    let actor = ActorId::new("operator-1").unwrap();
    store
        .register_device(descriptor(), &actor, "migration verification", now())
        .unwrap();
    let cloud_table_exists: bool = store
        .connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='edge_cloud_deliveries')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(cloud_table_exists);
    assert_eq!(
        store
            .connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        STORE_SCHEMA_VERSION
    );
}
