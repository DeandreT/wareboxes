mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use tokio::sync::Barrier;
use tokio::time::timeout;
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InventoryStatus, InventoryStatusChangeReason, TenantAccess};

#[derive(Debug, Clone, Copy)]
struct StatusRefs {
    tenant_id: TenantId,
    user_id: i64,
    inventory_owner_id: i64,
    facility_id: i64,
    location_id: i64,
    item_batch_id: i64,
    source_balance_id: i64,
    destination_balance_id: i64,
}

fn command_context(access: &TenantAccess, key: &str) -> CommandContext {
    CommandContext {
        tenant_id: access.tenant_id,
        actor_id: access.user_id,
        request_id: format!("request-{key}"),
        idempotency_key: Some(key.to_owned()),
    }
}

async fn status_refs(fixture: &Fixture, access: &TenantAccess, label: &str) -> StatusRefs {
    let inventory_owner_id = fixture
        .inventory_owner(access.tenant_id, &format!("{label} Owner"))
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, &format!("{label} Facility"))
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, inventory_owner_id, facility_id)
        .await;
    let item_id = fixture
        .item(access.tenant_id, &format!("{label} Item"), "each")
        .await;
    let balance = fixture
        .received_balance(
            access,
            ReceivedBalanceSetup {
                inventory_owner_id,
                facility_id,
                item_id,
                qty: 10,
                key: label,
            },
        )
        .await;
    let result = repo::inventory::change_inventory_status(
        &fixture.db,
        access,
        &command_context(access, &format!("{label}-initial-status-change")),
        &repo::inventory::ChangeInventoryStatusCommand {
            inventory_balance_id: balance.balance_id,
            qty: 1,
            to_status: InventoryStatus::Quarantine,
            reason: InventoryStatusChangeReason::QualityInspection,
            note: Some("initial status transition"),
            reference_type: None,
            reference_id: None,
        },
    )
    .await
    .unwrap();

    StatusRefs {
        tenant_id: access.tenant_id,
        user_id: access.user_id.get(),
        inventory_owner_id,
        facility_id,
        location_id: balance.location_id,
        item_batch_id: balance.item_batch_id,
        source_balance_id: result.source_inventory_balance_id,
        destination_balance_id: result.target_inventory_balance_id,
    }
}

async fn insert_raw_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    refs: StatusRefs,
    transaction_type: &str,
    key: &str,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO inventory_transactions (
            tenant_id, inventory_owner_id, created, actor_user_id,
            transaction_type, reason, operation, idempotency_key, request_hash
        )
        VALUES (
            $1, $2, $3, $4, $5, 'quality_inspection', $6, $6, $6
        )
        RETURNING id
        "#,
    )
    .bind(refs.tenant_id.get())
    .bind(refs.inventory_owner_id)
    .bind(db::now_iso())
    .bind(refs.user_id)
    .bind(transaction_type)
    .bind(key)
    .fetch_one(&mut **tx)
    .await
    .unwrap()
}

async fn insert_raw_entry(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    refs: StatusRefs,
    transaction_id: i64,
    status: InventoryStatus,
    quantity_delta: i64,
) {
    sqlx::query(
        r#"
        INSERT INTO inventory_entries (
            tenant_id, inventory_owner_id, transaction_id, created,
            facility_id, location_id, item_batch_id, item_id, uom, lot,
            expiration, serial, status, quantity_delta
        )
        SELECT $1, batch.inventory_owner_id, $2, $3, $4, $5, batch.id,
               batch.item_id, batch.uom, batch.lot, batch.expiration,
               batch.serial, $6, $7
        FROM item_batches batch
        WHERE batch.tenant_id = $1
          AND batch.inventory_owner_id = $8
          AND batch.id = $9
        "#,
    )
    .bind(refs.tenant_id.get())
    .bind(transaction_id)
    .bind(db::now_iso())
    .bind(refs.facility_id)
    .bind(refs.location_id)
    .bind(status.as_str())
    .bind(quantity_delta)
    .bind(refs.inventory_owner_id)
    .bind(refs.item_batch_id)
    .execute(&mut **tx)
    .await
    .unwrap();
}

async fn insert_raw_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    refs: StatusRefs,
    transaction_id: i64,
) {
    sqlx::query(
        r#"
        INSERT INTO inventory_status_transitions (
            tenant_id, inventory_owner_id, facility_id, transaction_id,
            source_balance_id, destination_balance_id, from_status, to_status,
            qty, reason_code, reason_note, created_by, created
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, 'available', 'quarantine',
            1, 'quality_inspection', 'adversarial invariant test', $7, $8
        )
        "#,
    )
    .bind(refs.tenant_id.get())
    .bind(refs.inventory_owner_id)
    .bind(refs.facility_id)
    .bind(transaction_id)
    .bind(refs.source_balance_id)
    .bind(refs.destination_balance_id)
    .bind(refs.user_id)
    .bind(db::now_iso())
    .execute(&mut **tx)
    .await
    .unwrap();
}

fn assert_deferred_rejection(error: &sqlx::Error, expected: &str) {
    assert!(
        error.to_string().contains(expected),
        "unexpected deferred status-change error: {error}"
    );
}

async fn assert_exact_runtime_acls(db: &db::Db) {
    const ACLS: &[(&str, &[&str], &[&str])] = &[
        (
            "inventory_status_transitions",
            &["SELECT", "INSERT"],
            &["UPDATE", "DELETE", "TRUNCATE", "REFERENCES", "TRIGGER"],
        ),
        (
            "inventory_transactions",
            &["SELECT", "INSERT"],
            &["UPDATE", "DELETE", "TRUNCATE", "REFERENCES", "TRIGGER"],
        ),
        (
            "inventory_entries",
            &["SELECT", "INSERT"],
            &["UPDATE", "DELETE", "TRUNCATE", "REFERENCES", "TRIGGER"],
        ),
        (
            "inventory_projection_changes",
            &["SELECT"],
            &[
                "INSERT",
                "UPDATE",
                "DELETE",
                "TRUNCATE",
                "REFERENCES",
                "TRIGGER",
            ],
        ),
        (
            "inventory_balances",
            &["SELECT", "INSERT", "UPDATE"],
            &["DELETE", "TRUNCATE", "REFERENCES", "TRIGGER"],
        ),
        (
            "inventory_reconciliation",
            &["SELECT"],
            &[
                "INSERT",
                "UPDATE",
                "DELETE",
                "TRUNCATE",
                "REFERENCES",
                "TRIGGER",
            ],
        ),
    ];

    let admin_db = admin_db_for(db).await;
    for (relation, allowed, denied) in ACLS {
        for privilege in *allowed {
            let granted: bool =
                sqlx::query_scalar("SELECT has_table_privilege('wareboxes_app', $1, $2)")
                    .bind(relation)
                    .bind(privilege)
                    .fetch_one(&admin_db)
                    .await
                    .unwrap();
            assert!(granted, "wareboxes_app must have {privilege} on {relation}");
        }
        for privilege in *denied {
            let granted: bool =
                sqlx::query_scalar("SELECT has_table_privilege('wareboxes_app', $1, $2)")
                    .bind(relation)
                    .bind(privilege)
                    .fetch_one(&admin_db)
                    .await
                    .unwrap();
            assert!(
                !granted,
                "wareboxes_app must not have {privilege} on {relation}"
            );
        }
    }

    for sequence in [
        "inventory_status_transitions_id_seq",
        "inventory_transactions_id_seq",
        "inventory_entries_id_seq",
        "inventory_balances_id_seq",
    ] {
        let usage: bool =
            sqlx::query_scalar("SELECT has_sequence_privilege('wareboxes_app', $1, 'USAGE')")
                .bind(sequence)
                .fetch_one(&admin_db)
                .await
                .unwrap();
        let select: bool =
            sqlx::query_scalar("SELECT has_sequence_privilege('wareboxes_app', $1, 'SELECT')")
                .bind(sequence)
                .fetch_one(&admin_db)
                .await
                .unwrap();
        let update: bool =
            sqlx::query_scalar("SELECT has_sequence_privilege('wareboxes_app', $1, 'UPDATE')")
                .bind(sequence)
                .fetch_one(&admin_db)
                .await
                .unwrap();
        assert!(usage, "wareboxes_app must have USAGE on {sequence}");
        assert!(!select, "wareboxes_app must not have SELECT on {sequence}");
        assert!(!update, "wareboxes_app must not have UPDATE on {sequence}");
    }
    for privilege in ["USAGE", "SELECT", "UPDATE"] {
        let granted: bool =
            sqlx::query_scalar("SELECT has_sequence_privilege('wareboxes_app', $1, $2)")
                .bind("inventory_projection_changes_id_seq")
                .bind(privilege)
                .fetch_one(&admin_db)
                .await
                .unwrap();
        assert!(
            !granted,
            "wareboxes_app must not have {privilege} on inventory_projection_changes_id_seq"
        );
    }
    admin_db.close().await;
}

#[tokio::test]
async fn status_transition_database_boundary_is_fail_closed() {
    let fixture = Fixture::new().await;
    let user_a = fixture.wms_user("status-invariant-a@test.local").await;
    let access_a = default_tenant_for_user(&fixture.db, user_a.id)
        .await
        .unwrap();
    let refs_a = status_refs(&fixture, &access_a, "STATUS-INVARIANT-A").await;

    let mut direct_update = tenant_tx(&fixture.db, refs_a.tenant_id).await;
    let direct_update_error =
        sqlx::query("UPDATE inventory_balances SET status = 'damaged' WHERE id = $1")
            .bind(refs_a.source_balance_id)
            .execute(&mut *direct_update)
            .await
            .unwrap_err();
    assert!(direct_update_error
        .to_string()
        .contains("status changes require a status-change transaction"));
    direct_update.rollback().await.unwrap();

    let mut missing_audit = tenant_tx(&fixture.db, refs_a.tenant_id).await;
    let transaction_id = insert_raw_transaction(
        &mut missing_audit,
        refs_a,
        "status_change",
        "status-invariant-missing-audit",
    )
    .await;
    insert_raw_entry(
        &mut missing_audit,
        refs_a,
        transaction_id,
        InventoryStatus::Available,
        -1,
    )
    .await;
    insert_raw_entry(
        &mut missing_audit,
        refs_a,
        transaction_id,
        InventoryStatus::Quarantine,
        1,
    )
    .await;
    assert_deferred_rejection(
        &missing_audit.commit().await.unwrap_err(),
        "requires exactly one audit row",
    );

    let mut wrong_transaction_type = tenant_tx(&fixture.db, refs_a.tenant_id).await;
    let transaction_id = insert_raw_transaction(
        &mut wrong_transaction_type,
        refs_a,
        "adjust",
        "status-invariant-wrong-transaction-type",
    )
    .await;
    insert_raw_entry(
        &mut wrong_transaction_type,
        refs_a,
        transaction_id,
        InventoryStatus::Available,
        1,
    )
    .await;
    insert_raw_audit(&mut wrong_transaction_type, refs_a, transaction_id).await;
    assert_deferred_rejection(
        &wrong_transaction_type.commit().await.unwrap_err(),
        "audit requires a status-change transaction",
    );

    let mut malformed_journal = tenant_tx(&fixture.db, refs_a.tenant_id).await;
    let transaction_id = insert_raw_transaction(
        &mut malformed_journal,
        refs_a,
        "status_change",
        "status-invariant-malformed-journal",
    )
    .await;
    insert_raw_entry(
        &mut malformed_journal,
        refs_a,
        transaction_id,
        InventoryStatus::Available,
        -1,
    )
    .await;
    insert_raw_audit(&mut malformed_journal, refs_a, transaction_id).await;
    let malformed_error = malformed_journal.commit().await.unwrap_err();
    let malformed_message = malformed_error.to_string();
    assert!(
        malformed_message.contains("requires exactly two entries")
            || malformed_message.contains("must conserve quantity"),
        "unexpected malformed journal error: {malformed_error}"
    );

    let user_b = fixture.wms_user("status-invariant-b@test.local").await;
    let access_b = default_tenant_for_user(&fixture.db, user_b.id)
        .await
        .unwrap();
    let refs_b = status_refs(&fixture, &access_b, "STATUS-INVARIANT-B").await;

    let unbound_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM inventory_status_transitions")
            .fetch_one(&fixture.db)
            .await
            .unwrap();
    assert_eq!(unbound_count, 0);

    let mut tenant_a_tx = tenant_tx(&fixture.db, refs_a.tenant_id).await;
    let tenant_a_transition_id: i64 =
        sqlx::query_scalar("SELECT id FROM inventory_status_transitions WHERE tenant_id = $1")
            .bind(refs_a.tenant_id.get())
            .fetch_one(&mut *tenant_a_tx)
            .await
            .unwrap();
    let tenant_a_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM inventory_status_transitions")
            .fetch_one(&mut *tenant_a_tx)
            .await
            .unwrap();
    assert_eq!(tenant_a_count, 1);
    tenant_a_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, refs_b.tenant_id).await;
    let guessed_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM inventory_status_transitions WHERE id = $1")
            .bind(tenant_a_transition_id)
            .fetch_one(&mut *tenant_b_tx)
            .await
            .unwrap();
    assert_eq!(guessed_count, 0);
    let tenant_b_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM inventory_status_transitions")
            .fetch_one(&mut *tenant_b_tx)
            .await
            .unwrap();
    assert_eq!(tenant_b_count, 1);
    let forged_insert = sqlx::query(
        r#"
        INSERT INTO inventory_status_transitions (
            tenant_id, inventory_owner_id, facility_id, transaction_id,
            source_balance_id, destination_balance_id, from_status, to_status,
            qty, reason_code, created_by
        )
        VALUES (
            $1, $2, $3, 9223372036854775807, $4, $5,
            'available', 'quarantine', 1, 'quality_inspection', $6
        )
        "#,
    )
    .bind(refs_a.tenant_id.get())
    .bind(refs_a.inventory_owner_id)
    .bind(refs_a.facility_id)
    .bind(refs_a.source_balance_id)
    .bind(refs_a.destination_balance_id)
    .bind(refs_a.user_id)
    .execute(&mut *tenant_b_tx)
    .await;
    assert!(forged_insert.is_err());
    tenant_b_tx.rollback().await.unwrap();

    let mut verify_a = tenant_tx(&fixture.db, refs_a.tenant_id).await;
    let (source_status, transition_count, reconciliation_count): (String, i64, i64) =
        sqlx::query_as(
            r#"
            SELECT
                (SELECT status FROM inventory_balances WHERE id = $1),
                (SELECT COUNT(*) FROM inventory_status_transitions),
                (SELECT COUNT(*) FROM inventory_reconciliation)
            "#,
        )
        .bind(refs_a.source_balance_id)
        .fetch_one(&mut *verify_a)
        .await
        .unwrap();
    verify_a.rollback().await.unwrap();
    assert_eq!(source_status, "available");
    assert_eq!(transition_count, 1);
    assert_eq!(reconciliation_count, 0);

    assert_exact_runtime_acls(&fixture.db).await;
}

#[tokio::test]
async fn competing_status_changes_serialize_without_overdraw_or_deadlock() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("status-change-race@test.local").await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let inventory_owner_id = fixture
        .inventory_owner(access.tenant_id, "Status Change Race Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Status Change Race Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, inventory_owner_id, facility_id)
        .await;
    let item_id = fixture
        .item(access.tenant_id, "Status Change Race Item", "each")
        .await;
    let balance = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id,
                facility_id,
                item_id,
                qty: 10,
                key: "STATUS-CHANGE-RACE",
            },
        )
        .await;

    let barrier = Arc::new(Barrier::new(3));
    let mut attempts = Vec::new();
    for (key, to_status, reason) in [
        (
            "status-change-race-quarantine",
            InventoryStatus::Quarantine,
            InventoryStatusChangeReason::QualityInspection,
        ),
        (
            "status-change-race-damaged",
            InventoryStatus::Damaged,
            InventoryStatusChangeReason::DamageConfirmed,
        ),
    ] {
        let db = fixture.db.clone();
        let access = access.clone();
        let barrier = Arc::clone(&barrier);
        attempts.push(tokio::spawn(async move {
            barrier.wait().await;
            repo::inventory::change_inventory_status(
                &db,
                &access,
                &command_context(&access, key),
                &repo::inventory::ChangeInventoryStatusCommand {
                    inventory_balance_id: balance.balance_id,
                    qty: 6,
                    to_status,
                    reason,
                    note: None,
                    reference_type: None,
                    reference_id: None,
                },
            )
            .await
        }));
    }

    barrier.wait().await;
    let results = timeout(Duration::from_secs(3), async {
        let first = attempts.remove(0).await.unwrap();
        let second = attempts.remove(0).await.unwrap();
        [first, second]
    })
    .await
    .expect("competing status changes complete without a deadlock");

    let mut accepted = Vec::new();
    let mut rejected = 0;
    for result in results {
        match result {
            Ok(result) => accepted.push(result),
            Err(error) => {
                assert!(
                    matches!(error, AppError::Application(ApplicationError::Conflict(_))),
                    "unexpected competing status-change error: {error:?}"
                );
                rejected += 1;
            }
        }
    }
    assert_eq!(accepted.len(), 1);
    assert_eq!(rejected, 1);
    assert_eq!(accepted[0].qty, 6);

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let balances: Vec<(String, i64)> = sqlx::query_as(
        r#"
        SELECT status, qty_on_hand
        FROM inventory_balances
        WHERE tenant_id = $1
          AND inventory_owner_id = $2
          AND location_id = $3
          AND item_batch_id = $4
          AND deleted IS NULL
        ORDER BY status
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(inventory_owner_id)
    .bind(balance.location_id)
    .bind(balance.item_batch_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    let (transition_count, transaction_count, entry_count, reconciliation_count): (
        i64,
        i64,
        i64,
        i64,
    ) = sqlx::query_as(
        r#"
        SELECT
            (SELECT COUNT(*) FROM inventory_status_transitions),
            (
                SELECT COUNT(*)
                FROM inventory_transactions
                WHERE transaction_type = 'status_change'
            ),
            (
                SELECT COUNT(*)
                FROM inventory_entries entry
                INNER JOIN inventory_transactions transaction
                    ON transaction.tenant_id = entry.tenant_id
                   AND transaction.inventory_owner_id =
                       entry.inventory_owner_id
                   AND transaction.id = entry.transaction_id
                WHERE transaction.transaction_type = 'status_change'
            ),
            (SELECT COUNT(*) FROM inventory_reconciliation)
        "#,
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();

    assert_eq!(
        balances.iter().map(|(_, quantity)| quantity).sum::<i64>(),
        10
    );
    assert!(balances.iter().all(|(_, quantity)| *quantity >= 0));
    assert_eq!(
        balances
            .iter()
            .find(|(status, _)| status == "available")
            .map(|(_, quantity)| *quantity),
        Some(4)
    );
    assert_eq!(transition_count, 1);
    assert_eq!(transaction_count, 1);
    assert_eq!(entry_count, 2);
    assert_eq!(reconciliation_count, 0);
}
