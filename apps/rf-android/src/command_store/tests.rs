use super::*;
use crate::expected_receiving::{
    ConfirmationIntent, ConfirmationRecoverySnapshot, ConfirmationRecoverySnapshotInput,
    DockBarcode, ExpectedReceiptCommand, ExpectedReceiptLine, ExpectedReceiptLineInput, FacilityId,
    InventoryOwnerId, ItemBarcode, ItemId, LoadBarcode, LoadId, LoadLineId, LocationId,
    NonNegativeQuantity, PositiveQuantity, ReceiptExceptionReason, ReceivingDock,
    ReceivingLoadStatus, StockDimension,
};
use crate::workflow::{CycleCountCommand, InventoryRelocationCommand, MovementKind, ReleaseReason};

fn scope() -> ExecutionScope {
    ExecutionScope {
        tenant_id: 7,
        operator_id: 9,
        device_id: "device-11".into(),
    }
}

fn draft(command_id: &str, key: &str, command: PutawayCommand) -> DurableCommandDraft {
    DurableCommandDraft {
        schema_version: 1,
        command_id: command_id.into(),
        idempotency_key: key.into(),
        command: command.into(),
    }
}

fn claim_draft(command_id: &str, key: &str) -> DurableCommandDraft {
    draft(
        command_id,
        key,
        PutawayCommand::ClaimNext {
            workflow: MovementKind::Loose,
        },
    )
}

fn relocation_claim_draft(command_id: &str, key: &str) -> DurableCommandDraft {
    DurableCommandDraft {
        schema_version: 1,
        command_id: command_id.into(),
        idempotency_key: key.into(),
        command: RfCommand::InventoryRelocation(InventoryRelocationCommand::ClaimNext {
            workflow: MovementKind::Loose,
        }),
    }
}

fn cycle_count_confirmation_draft(command_id: &str, key: &str) -> DurableCommandDraft {
    DurableCommandDraft {
        schema_version: 1,
        command_id: command_id.into(),
        idempotency_key: key.into(),
        command: RfCommand::CycleCount(CycleCountCommand::Confirm {
            task_id: 81,
            location_barcode: "A-08-01".into(),
            item_barcode: "ITEM-81".into(),
            license_plate_barcode: None,
            counted_quantity: 4,
            note: None,
        }),
    }
}

fn expected_receipt_draft(command_id: &str, key: &str) -> DurableCommandDraft {
    let line = ExpectedReceiptLine::try_new(ExpectedReceiptLineInput {
        load_line_id: LoadLineId::try_from(55).unwrap(),
        item_id: ItemId::try_from(66).unwrap(),
        item_description: Some("Case-picked item".into()),
        uom: StockDimension::new("case").unwrap(),
        item_barcodes: vec![ItemBarcode::new("CASE-66").unwrap()],
        expected: PositiveQuantity::try_from(10).unwrap(),
        received: NonNegativeQuantity::new(2).unwrap(),
        rejected: NonNegativeQuantity::new(0).unwrap(),
        missing: NonNegativeQuantity::new(0).unwrap(),
        remaining: NonNegativeQuantity::new(8).unwrap(),
        lot: None,
        serial: None,
        expiration: None,
    })
    .unwrap();
    let recovery = ConfirmationRecoverySnapshot::try_new(ConfirmationRecoverySnapshotInput {
        load_barcode: LoadBarcode::new("LOAD-11").unwrap(),
        load_id: LoadId::try_from(11).unwrap(),
        inventory_owner_id: InventoryOwnerId::try_from(22).unwrap(),
        facility_id: FacilityId::try_from(33).unwrap(),
        reference_number: Some("ASN-11".into()),
        status: ReceivingLoadStatus::Receiving,
        dock: ReceivingDock::new(
            LocationId::try_from(44).unwrap(),
            DockBarcode::new("DOCK-04").unwrap(),
            Some("Inbound dock 4".into()),
        ),
        selected_line: line,
    })
    .unwrap();
    DurableCommandDraft {
        schema_version: 1,
        command_id: command_id.into(),
        idempotency_key: key.into(),
        command: RfCommand::ExpectedReceipt(Box::new(
            ConfirmationIntent::try_new(
                recovery,
                ExpectedReceiptCommand::Missing {
                    quantity: PositiveQuantity::try_from(3).unwrap(),
                    reason: ReceiptExceptionReason::ShortShipment,
                    note: None,
                },
            )
            .unwrap(),
        )),
    }
}

fn response(status: u16) -> DurableHttpResponse {
    DurableHttpResponse {
        status,
        body: format!(r#"{{"status":{status}}}"#).into_bytes(),
        server_request_id: Some(format!("server-{status}")),
    }
}

fn temporary_database(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("wareboxes-rf-{name}-{}.sqlite3", Uuid::new_v4()))
}

fn remove_database(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
    let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
}

#[test]
fn device_profile_is_created_once_and_survives_reopen() {
    let path = temporary_database("profile");
    let first = {
        let store = CommandStore::open(&path).expect("store should open");
        let profile = store.device_profile().expect("profile should exist");
        Uuid::parse_str(&profile.device_id).expect("device ID should be a random UUID");
        assert_eq!(profile.server_url, None);
        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM rf_device_profile", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("profile count should load"),
            1
        );
        profile
    };

    let reopened = CommandStore::open(&path)
        .expect("store should reopen")
        .device_profile()
        .expect("profile should recover");
    assert_eq!(reopened, first);

    remove_database(&path);
}

#[test]
fn server_url_round_trips_without_accepting_credentials_or_tokens() {
    let mut store = CommandStore::open_in_memory().expect("store should open");
    let device_id = store
        .device_profile()
        .expect("profile should exist")
        .device_id;

    let configured = store
        .set_server_url(Some("  https://warehouse.example/rf/  "))
        .expect("server URL should persist");
    assert_eq!(
        configured,
        DeviceProfile {
            device_id: device_id.clone(),
            server_url: Some("https://warehouse.example/rf".into()),
        }
    );
    assert_eq!(
        store
            .server_url()
            .expect("server URL should load")
            .as_deref(),
        Some("https://warehouse.example/rf"),
    );

    for invalid in [
        "https://operator:secret@warehouse.example",
        "https://warehouse.example?token=secret",
        "https://warehouse.example/#access_token=secret",
        "file:///tmp/server",
        "",
    ] {
        assert!(matches!(
            store.set_server_url(Some(invalid)),
            Err(CommandStoreError::InvalidServerUrl)
        ));
    }
    assert_eq!(
        store
            .device_profile()
            .expect("invalid updates must not change the profile"),
        configured
    );

    let cleared = store.set_server_url(None).expect("server URL should clear");
    assert_eq!(cleared.device_id, device_id);
    assert_eq!(cleared.server_url, None);
}

#[test]
fn server_url_cannot_change_while_any_device_scope_has_unresolved_work() {
    let mut store = CommandStore::open_in_memory().expect("store should open");
    let profile = store
        .set_server_url(Some("https://warehouse.example/rf"))
        .expect("initial server URL should persist");
    let command_scope = ExecutionScope {
        tenant_id: 401,
        operator_id: 902,
        device_id: profile.device_id,
    };
    let record = store
        .persist(&command_scope, claim_draft("command-origin", "key-origin"))
        .expect("command should persist");

    assert_eq!(
        store
            .set_server_url(Some("https://warehouse.example/rf/"))
            .expect("an identical normalized URL should remain valid")
            .server_url
            .as_deref(),
        Some("https://warehouse.example/rf")
    );
    assert!(matches!(
        store.set_server_url(Some("https://other.example/rf")),
        Err(CommandStoreError::ServerUrlChangeBlocked)
    ));
    assert!(matches!(
        store.set_server_url(None),
        Err(CommandStoreError::ServerUrlChangeBlocked)
    ));

    let attempt = store
        .begin_attempt(&command_scope, record.record_id)
        .expect("attempt should start");
    store
        .record_response(
            &command_scope,
            record.record_id,
            &attempt.attempt_id,
            &response(200),
        )
        .expect("response should persist");
    store
        .finalize(
            &command_scope,
            record.record_id,
            CommandStatus::Completed,
            None,
        )
        .expect("command should complete");

    assert_eq!(
        store
            .set_server_url(Some("https://other.example/rf"))
            .expect("completed work should not pin the server URL")
            .server_url
            .as_deref(),
        Some("https://other.example/rf")
    );
}

#[test]
fn command_is_durable_before_an_attempt_can_start() {
    let mut store = CommandStore::open_in_memory().expect("store should open");
    let record = store
        .persist(&scope(), claim_draft("command-1", "key-1"))
        .expect("command should persist");

    assert_eq!(record.status, CommandStatus::Persisted);
    assert_eq!(record.attempt_count, 0);
    assert_eq!(record.response, None);
    let attempt = store
        .begin_attempt(&scope(), record.record_id)
        .expect("persisted command should dispatch");
    assert_eq!(attempt.command.status, CommandStatus::Dispatching);
    assert_eq!(attempt.ordinal, 1);
    assert_ne!(attempt.attempt_id, attempt.request_id);
}

#[test]
fn expected_receipt_uses_the_same_durable_replay_lane_as_putaway() {
    let mut store = CommandStore::open_in_memory().expect("store should open");
    let record = store
        .persist(
            &scope(),
            expected_receipt_draft("receipt-1", "expected-receiving:55:1"),
        )
        .expect("receipt should persist before dispatch");

    assert_eq!(
        record.operation,
        CommandOperation::ExpectedReceiptConfirmation
    );
    assert_eq!(
        record.request.response_kind,
        ResponseKind::ExpectedReceiptConfirmation
    );
    assert_eq!(
        record.request.path,
        "/api/v1/expected-receiving/lines/55/confirmations"
    );
    assert!(record.request.verify_body());

    let attempt = store
        .begin_attempt(&scope(), record.record_id)
        .expect("persisted receipt should dispatch");
    store
        .mark_ambiguous(
            &scope(),
            record.record_id,
            &attempt.attempt_id,
            "connection closed",
        )
        .expect("ambiguous result should remain replayable");
    let retry = store
        .begin_attempt(&scope(), record.record_id)
        .expect("receipt retry should start");
    assert_eq!(retry.command.request, attempt.command.request);
    assert_eq!(
        retry.command.draft.idempotency_key,
        attempt.command.draft.idempotency_key
    );
    assert_ne!(retry.request_id, attempt.request_id);
}

#[test]
fn relocation_command_persists_its_typed_endpoint_and_response_kind() {
    let mut store = CommandStore::open_in_memory().unwrap();
    let draft = relocation_claim_draft("relocation-claim-1", "relocation:key-1");

    let record = store.persist(&scope(), draft.clone()).unwrap();

    assert_eq!(record.operation, CommandOperation::ClaimNext);
    assert_eq!(
        record.request.path,
        "/api/v1/inventory-relocation-claims/next"
    );
    assert_eq!(
        record.request.response_kind,
        ResponseKind::RelocationOptionalClaim
    );
    assert_eq!(record.draft, draft);
}

#[test]
fn cycle_count_confirmation_survives_the_durable_store_boundary() {
    let mut store = CommandStore::open_in_memory().unwrap();
    let draft = cycle_count_confirmation_draft("cycle-count-confirm-1", "cycle-count:confirm:81:1");

    let record = store.persist(&scope(), draft.clone()).unwrap();

    assert_eq!(record.operation, CommandOperation::CycleCountConfirmation);
    assert_eq!(
        record.request.path,
        "/api/v1/cycle-count-tasks/81/confirmations"
    );
    assert_eq!(
        record.request.response_kind,
        ResponseKind::CycleCountConfirmation
    );
    assert_eq!(record.draft, draft);
}

#[test]
fn typed_command_rejects_a_tampered_durable_request_envelope() {
    let mut store = CommandStore::open_in_memory().expect("store should open");
    let record = store
        .persist(
            &scope(),
            expected_receipt_draft("receipt-1", "expected-receiving:55:1"),
        )
        .expect("receipt should persist");
    store
        .connection
        .execute(
            "UPDATE rf_commands SET path = '/api/v1/putaway-claims/next' WHERE record_id = ?1",
            [record.record_id],
        )
        .expect("test should corrupt the request path");

    assert!(matches!(
        store.unresolved_for_device(&scope().device_id),
        Err(CommandStoreError::CorruptRecord(message))
            if message == "durable request does not match its typed command"
    ));
}

#[test]
fn exact_duplicate_returns_the_original_record() {
    let mut store = CommandStore::open_in_memory().expect("store should open");
    let draft = claim_draft("command-1", "key-1");
    let first = store
        .persist(&scope(), draft.clone())
        .expect("first command should persist");
    let replay = store
        .persist(&scope(), draft)
        .expect("exact command should replay");

    assert_eq!(replay, first);
}

#[test]
fn changed_content_cannot_reuse_a_command_identity() {
    let mut store = CommandStore::open_in_memory().expect("store should open");
    store
        .persist(&scope(), claim_draft("command-1", "key-1"))
        .expect("first command should persist");
    let changed = draft(
        "command-1",
        "key-1",
        PutawayCommand::ClaimNext {
            workflow: MovementKind::LicensePlate,
        },
    );

    assert!(matches!(
        store.persist(&scope(), changed),
        Err(CommandStoreError::IdentityConflict)
    ));
}

#[test]
fn one_device_cannot_create_concurrent_unresolved_commands_across_operators() {
    let mut store = CommandStore::open_in_memory().expect("store should open");
    let original = store
        .persist(&scope(), claim_draft("command-1", "key-1"))
        .expect("first command should persist");
    let other_operator = ExecutionScope {
        tenant_id: 17,
        operator_id: 29,
        device_id: scope().device_id,
    };

    assert!(matches!(
        store.persist(&other_operator, claim_draft("command-2", "key-2")),
        Err(CommandStoreError::UnresolvedCommandExists)
    ));
    assert_eq!(
        store
            .unresolved_for_device(&other_operator.device_id)
            .expect("device recovery should find the original command"),
        vec![original]
    );
}

#[test]
fn ambiguous_retry_keeps_the_request_and_changes_attempt_identity() {
    let mut store = CommandStore::open_in_memory().expect("store should open");
    let record = store
        .persist(
            &scope(),
            draft(
                "command-1",
                "confirm-key-1",
                PutawayCommand::ConfirmLoose {
                    task_id: 42,
                    destination_location_barcode: "A-01-02".into(),
                },
            ),
        )
        .expect("command should persist");
    let first = store
        .begin_attempt(&scope(), record.record_id)
        .expect("first attempt should start");
    store
        .mark_ambiguous(
            &scope(),
            record.record_id,
            &first.attempt_id,
            "connection closed",
        )
        .expect("ambiguous result should persist");
    let retry = store
        .begin_attempt(&scope(), record.record_id)
        .expect("retry should start");

    assert_eq!(retry.command.request, first.command.request);
    assert_eq!(
        retry.command.draft.idempotency_key,
        first.command.draft.idempotency_key
    );
    assert_ne!(retry.attempt_id, first.attempt_id);
    assert_ne!(retry.request_id, first.request_id);
    assert_eq!(retry.ordinal, 2);
}

#[test]
fn retryable_response_stays_unresolved_and_retries_the_exact_request() {
    let mut store = CommandStore::open_in_memory().expect("store should open");
    let record = store
        .persist(
            &scope(),
            draft(
                "command-1",
                "confirm-key-1",
                PutawayCommand::ConfirmLoose {
                    task_id: 42,
                    destination_location_barcode: "A-01-02".into(),
                },
            ),
        )
        .expect("command should persist");
    let first = store
        .begin_attempt(&scope(), record.record_id)
        .expect("first attempt should start");

    let retryable = store
        .record_retryable_response(
            &scope(),
            record.record_id,
            &first.attempt_id,
            &response(429),
        )
        .expect("rate limit should be retryable");
    assert_eq!(retryable.status, CommandStatus::Retryable);
    assert_eq!(retryable.request, record.request);
    assert_eq!(retryable.draft, record.draft);
    assert_eq!(retryable.response, None);
    assert_eq!(
        store
            .unresolved(&scope())
            .expect("retryable command should load"),
        vec![retryable.clone()]
    );

    let retry = store
        .begin_attempt(&scope(), record.record_id)
        .expect("retryable command should dispatch again");
    assert_eq!(retry.command.request, first.command.request);
    assert_eq!(retry.command.draft, first.command.draft);
    assert_eq!(retry.command.record_id, first.command.record_id);
    assert_ne!(retry.attempt_id, first.attempt_id);
    assert_ne!(retry.request_id, first.request_id);
    assert_eq!(retry.ordinal, first.ordinal + 1);
}

#[test]
fn only_the_active_attempt_can_record_a_retryable_response() {
    let mut store = CommandStore::open_in_memory().expect("store should open");
    let record = store
        .persist(&scope(), claim_draft("command-1", "key-1"))
        .expect("command should persist");
    let first = store
        .begin_attempt(&scope(), record.record_id)
        .expect("first attempt should start");

    assert!(matches!(
        store.record_retryable_response(
            &scope(),
            record.record_id,
            "different-attempt",
            &response(503),
        ),
        Err(CommandStoreError::AttemptMismatch {
            record_id,
            attempt_id,
        }) if record_id == record.record_id && attempt_id == "different-attempt"
    ));
    assert_eq!(
        store
            .unresolved(&scope())
            .expect("dispatching command should remain")
            .first()
            .map(|record| record.status),
        Some(CommandStatus::Dispatching)
    );

    store
        .record_retryable_response(
            &scope(),
            record.record_id,
            &first.attempt_id,
            &response(503),
        )
        .expect("active attempt should transition");
    let second = store
        .begin_attempt(&scope(), record.record_id)
        .expect("second attempt should start");
    assert!(matches!(
        store.record_retryable_response(
            &scope(),
            record.record_id,
            &first.attempt_id,
            &response(503),
        ),
        Err(CommandStoreError::AttemptMismatch { attempt_id, .. })
            if attempt_id == first.attempt_id
    ));
    store
        .record_retryable_response(
            &scope(),
            record.record_id,
            &second.attempt_id,
            &response(503),
        )
        .expect("current attempt should transition");
}

#[test]
fn retryable_http_statuses_use_the_explicit_retry_api() {
    for status in [401, 408, 429, 500, 503, 599] {
        let mut store = CommandStore::open_in_memory().expect("store should open");
        let record = store
            .persist(&scope(), claim_draft("command-1", "key-1"))
            .expect("command should persist");
        let attempt = store
            .begin_attempt(&scope(), record.record_id)
            .expect("attempt should start");
        let retryable_response = response(status);

        assert!(matches!(
            store.record_response(
                &scope(),
                record.record_id,
                &attempt.attempt_id,
                &retryable_response,
            ),
            Err(CommandStoreError::RetryableHttpStatus(actual)) if actual == status
        ));
        assert_eq!(
            store
                .record_retryable_response(
                    &scope(),
                    record.record_id,
                    &attempt.attempt_id,
                    &retryable_response,
                )
                .expect("known status should be retryable")
                .status,
            CommandStatus::Retryable
        );
    }

    let mut store = CommandStore::open_in_memory().expect("store should open");
    let record = store
        .persist(&scope(), claim_draft("command-1", "key-1"))
        .expect("command should persist");
    let attempt = store
        .begin_attempt(&scope(), record.record_id)
        .expect("attempt should start");
    assert!(matches!(
        store.record_retryable_response(
            &scope(),
            record.record_id,
            &attempt.attempt_id,
            &response(400),
        ),
        Err(CommandStoreError::NonRetryableHttpStatus(400))
    ));
}

#[test]
fn retryable_command_survives_reopen_and_can_start_a_new_attempt() {
    let path = temporary_database("retry");
    let record_id = {
        let mut store = CommandStore::open(&path).expect("store should open");
        let record = store
            .persist(&scope(), claim_draft("command-1", "key-1"))
            .expect("command should persist");
        let attempt = store
            .begin_attempt(&scope(), record.record_id)
            .expect("attempt should start");
        store
            .record_retryable_response(
                &scope(),
                record.record_id,
                &attempt.attempt_id,
                &response(401),
            )
            .expect("unauthorized response should wait for retry");
        record.record_id
    };

    let mut store = CommandStore::open(&path).expect("store should reopen");
    let unresolved = store
        .unresolved(&scope())
        .expect("retryable command should recover");
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0].status, CommandStatus::Retryable);
    let retry = store
        .begin_attempt(&scope(), record_id)
        .expect("recovered command should dispatch");
    assert_eq!(retry.ordinal, 2);

    drop(store);
    remove_database(&path);
}

#[test]
fn recorded_response_survives_reopen_and_completion() {
    let path = temporary_database("response");
    let (record_id, expected) = {
        let mut store = CommandStore::open(&path).expect("store should open");
        let record = store
            .persist(
                &scope(),
                draft(
                    "release-1",
                    "release-key-1",
                    PutawayCommand::Release {
                        task_id: 42,
                        reason: ReleaseReason::WorkInterrupted,
                        note: None,
                    },
                ),
            )
            .expect("command should persist");
        let attempt = store
            .begin_attempt(&scope(), record.record_id)
            .expect("attempt should start");
        let expected = DurableHttpResponse {
            status: 200,
            body: br#"{"task_id":42}"#.to_vec(),
            server_request_id: Some("server-1".into()),
        };
        let recorded = store
            .record_response(&scope(), record.record_id, &attempt.attempt_id, &expected)
            .expect("response should persist");

        assert_eq!(recorded.status, CommandStatus::ResponseRecorded);
        assert_eq!(recorded.response.as_ref(), Some(&expected));
        (record.record_id, expected)
    };

    let mut store = CommandStore::open(&path).expect("store should reopen");
    let recovered = store
        .unresolved(&scope())
        .expect("recorded response should recover");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].response.as_ref(), Some(&expected));
    let completed = store
        .finalize(&scope(), record_id, CommandStatus::Completed, None)
        .expect("recorded response should complete");
    assert_eq!(completed.status, CommandStatus::Completed);
    assert_eq!(completed.response, Some(expected));

    drop(store);
    remove_database(&path);
}

#[test]
fn tampered_response_body_is_rejected_during_recovery() {
    let mut store = CommandStore::open_in_memory().expect("store should open");
    let record = store
        .persist(&scope(), claim_draft("command-1", "key-1"))
        .expect("command should persist");
    let attempt = store
        .begin_attempt(&scope(), record.record_id)
        .expect("attempt should start");
    store
        .record_response(
            &scope(),
            record.record_id,
            &attempt.attempt_id,
            &response(200),
        )
        .expect("response should persist");
    store
        .connection
        .execute(
            "UPDATE rf_commands SET response_body = ?1 WHERE record_id = ?2",
            params![b"tampered".as_slice(), record.record_id],
        )
        .expect("test should corrupt the response body");

    assert!(matches!(
        store.unresolved(&scope()),
        Err(CommandStoreError::CorruptRecord(message))
            if message == "response body hash does not match"
    ));
}

#[test]
fn partial_response_fields_are_rejected_during_recovery() {
    let mut store = CommandStore::open_in_memory().expect("store should open");
    let record = store
        .persist(&scope(), claim_draft("command-1", "key-1"))
        .expect("command should persist");
    store
        .connection
        .pragma_update(None, "ignore_check_constraints", "ON")
        .expect("test should disable checks");
    store
        .connection
        .execute(
            "UPDATE rf_commands SET response_status = 200 WHERE record_id = ?1",
            [record.record_id],
        )
        .expect("test should create a partial response");

    assert!(matches!(
        store.unresolved(&scope()),
        Err(CommandStoreError::CorruptRecord(message))
            if message == "recorded response fields are incomplete"
    ));
}

#[test]
fn another_scope_cannot_dispatch_a_record() {
    let mut store = CommandStore::open_in_memory().expect("store should open");
    let record = store
        .persist(&scope(), claim_draft("command-1", "key-1"))
        .expect("command should persist");
    let other_scope = ExecutionScope {
        tenant_id: 8,
        ..scope()
    };

    assert!(matches!(
        store.begin_attempt(&other_scope, record.record_id),
        Err(CommandStoreError::ScopeMismatch)
    ));
}

#[test]
fn unfinished_dispatch_is_ambiguous_after_reopen() {
    let path = temporary_database("store");
    let record_id = {
        let mut store = CommandStore::open(&path).expect("store should open");
        let record = store
            .persist(&scope(), claim_draft("command-1", "key-1"))
            .expect("command should persist");
        store
            .begin_attempt(&scope(), record.record_id)
            .expect("attempt should start");
        record.record_id
    };

    let store = CommandStore::open(&path).expect("store should reopen");
    let unresolved = store
        .unresolved(&scope())
        .expect("unresolved command should load");
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0].record_id, record_id);
    assert_eq!(unresolved[0].status, CommandStatus::Ambiguous);

    drop(store);
    remove_database(&path);
}
