mod common;

use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use common::*;
use tokio::time::{sleep, timeout};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{
    InboundReceiptExceptionReason, InventoryHoldReason, InventoryTransactionType, TenantAccess,
};

const CONFIRM_OPERATION: &str = "task.confirm_item_location_cycle_count.v1";

async fn wait_until_task_is_locked(db: &db::Db, tenant_id: TenantId, task_id: i64) {
    timeout(Duration::from_secs(2), async {
        loop {
            let mut tx = tenant_tx(db, tenant_id).await;
            let result = sqlx::query(
                "SELECT id FROM work_tasks WHERE tenant_id = $1 AND id = $2 FOR UPDATE NOWAIT",
            )
            .bind(tenant_id.get())
            .bind(task_id)
            .fetch_one(&mut *tx)
            .await;
            tx.rollback().await.unwrap();

            match result {
                Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("55P03") => {
                    break;
                }
                Ok(_) => sleep(Duration::from_millis(10)).await,
                Err(error) => panic!("unexpected cycle count task lock probe error: {error}"),
            }
        }
    })
    .await
    .expect("cycle count confirmation locks its task before inventory");
}

struct CycleCountFixture {
    fixture: Fixture,
    access: TenantAccess,
    facility_id: i64,
    location_id: i64,
    owner_id: i64,
    item_id: i64,
    balance_id: i64,
    task_id: i64,
}

impl CycleCountFixture {
    async fn new(suffix: &str, on_hand: i64, reserved: i64) -> Self {
        let fixture = Fixture::new().await;
        let user = fixture
            .wms_user(&format!("cycle-count-{suffix}@test.com"))
            .await;
        let tenant_id = tenant_for_user(&fixture.db, user.id).await;
        let facility_id = fixture
            .facility(tenant_id, &format!("Cycle Count {suffix} DC"))
            .await;
        let location_id = fixture
            .location(
                tenant_id,
                facility_id,
                &format!("CYCLE-{}", suffix.to_uppercase()),
            )
            .await;
        let owner_id = fixture
            .inventory_owner(tenant_id, &format!("Cycle Count {suffix} Owner"))
            .await;
        fixture
            .assign_owner_to_facility(tenant_id, owner_id, facility_id)
            .await;
        let item_id = fixture
            .item(tenant_id, &format!("Cycle Count {suffix} Item"), "each")
            .await;
        let batch_id = repo::inventory::add_item_batch(
            &fixture.db,
            tenant_id,
            owner_id,
            item_id,
            None,
            Some(&format!("COUNT-{suffix}")),
            None,
            None,
        )
        .await
        .unwrap();
        repo::inventory::receive_inventory(
            &fixture.db,
            tenant_id,
            user.id,
            batch_id,
            location_id,
            on_hand,
            None,
            Some("cycle count fixture"),
            None,
            None,
            &format!("cycle-count-{suffix}-receipt"),
        )
        .await
        .unwrap();
        let balance_id = repo::inventory::get_balances(&fixture.db, tenant_id, false)
            .await
            .unwrap()
            .into_iter()
            .find(|balance| balance.item_batch_id == batch_id && balance.location_id == location_id)
            .unwrap()
            .id;

        if reserved > 0 {
            let order_id = fixture
                .order(tenant_id, &format!("COUNT-{suffix}-ORDER"), owner_id)
                .await;
            fixture
                .allocated_reservation(
                    tenant_id,
                    user.id,
                    order_id,
                    balance_id,
                    reserved,
                    &format!("cycle-count-{suffix}"),
                )
                .await;
        }

        let task_id = repo::tasks::create_item_location_cycle_count_task(
            &fixture.db,
            tenant_id,
            user.id,
            location_id,
            item_id,
            Some("scheduled_count"),
            None,
            None,
            balance_id,
            Some("verify item-location quantity"),
        )
        .await
        .unwrap();
        let access = repo::tenants::access_for_user(&fixture.db, user.id, tenant_id)
            .await
            .unwrap()
            .unwrap();
        let start = Self::command(&access, &format!("cycle-count-{suffix}-start"));
        assert!(
            repo::tasks::start_task_in_scope(&fixture.db, &access, &start, task_id)
                .await
                .unwrap()
        );

        Self {
            fixture,
            access,
            facility_id,
            location_id,
            owner_id,
            item_id,
            balance_id,
            task_id,
        }
    }

    fn command(access: &TenantAccess, key: &str) -> CommandContext {
        CommandContext {
            tenant_id: access.tenant_id,
            actor_id: access.user_id,
            request_id: format!("request-{key}"),
            idempotency_key: Some(key.to_owned()),
        }
    }

    async fn balance_quantities(&self) -> (i64, i64) {
        let balance = repo::inventory::get_balances(&self.fixture.db, self.access.tenant_id, false)
            .await
            .unwrap()
            .into_iter()
            .find(|balance| balance.id == self.balance_id)
            .unwrap();
        (balance.qty_on_hand, balance.qty_reserved)
    }

    async fn place_hold(&self, qty: i64) -> i64 {
        repo::inventory::place_inventory_hold(
            &self.fixture.db,
            &self.access,
            &Self::command(&self.access, "place-cycle-count-hold"),
            &repo::inventory::PlaceInventoryHoldCommand {
                inventory_balance_id: self.balance_id,
                qty,
                reason: InventoryHoldReason::InventoryDiscrepancy,
                note: Some("cycle count restriction"),
                reference_type: None,
                reference_id: None,
            },
        )
        .await
        .unwrap()
        .hold_id
    }

    async fn held_quantity(&self) -> i64 {
        repo::inventory::get_balances(&self.fixture.db, self.access.tenant_id, false)
            .await
            .unwrap()
            .into_iter()
            .find(|balance| balance.id == self.balance_id)
            .unwrap()
            .qty_held
    }

    async fn effect_counts(&self) -> (i64, i64, i64, i64, i64, i64, i64) {
        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        let counts = sqlx::query_as(
            r#"
            SELECT
                (SELECT COUNT(*)
                 FROM inventory_transactions
                 WHERE tenant_id = $1 AND operation = $2),
                (SELECT COUNT(*)
                 FROM inventory_entries entry
                 INNER JOIN inventory_transactions journal_transaction
                    ON journal_transaction.tenant_id = entry.tenant_id
                   AND journal_transaction.inventory_owner_id = entry.inventory_owner_id
                   AND journal_transaction.id = entry.transaction_id
                 WHERE journal_transaction.tenant_id = $1
                   AND journal_transaction.operation = $2),
                (SELECT COUNT(*)
                 FROM cycle_count_item_location_results
                 WHERE tenant_id = $1 AND task_id = $3),
                (SELECT COUNT(*)
                 FROM work_task_progress
                 WHERE tenant_id = $1
                   AND task_id = $3
                   AND action = 'cycle_count_confirmed'),
                (SELECT COUNT(*)
                 FROM command_idempotency_records
                 WHERE tenant_id = $1 AND operation = $2),
                (SELECT COUNT(*)
                 FROM outbox_events
                 WHERE tenant_id = $1
                   AND event_type = 'inventory.cycle_count.confirmed'
                   AND aggregate_id = $3::TEXT),
                (SELECT COUNT(*)
                 FROM outbox_events
                 WHERE tenant_id = $1
                   AND event_type = 'inventory.transaction.recorded'
                   AND payload ->> 'operation' = $2)
            "#,
        )
        .bind(self.access.tenant_id.get())
        .bind(CONFIRM_OPERATION)
        .bind(self.task_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        counts
    }

    async fn task_status(&self) -> String {
        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        let status =
            sqlx::query_scalar("SELECT status FROM work_tasks WHERE tenant_id = $1 AND id = $2")
                .bind(self.access.tenant_id.get())
                .bind(self.task_id)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        tx.rollback().await.unwrap();
        status
    }

    async fn assert_reconciled(&self) {
        assert!(repo::inventory::get_reconciliation_issues(
            &self.fixture.db,
            self.access.tenant_id,
        )
        .await
        .unwrap()
        .is_empty());
        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        let hold_issues: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM inventory_hold_reconciliation")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(hold_issues, 0);
    }
}

#[tokio::test]
async fn adjustment_is_atomic_replay_safe_and_reconciled() {
    let count = CycleCountFixture::new("adjustment", 10, 0).await;
    let command = CycleCountFixture::command(&count.access, "confirm-adjustment");

    let confirmation = repo::tasks::confirm_item_location_cycle_count_in_scope(
        &count.fixture.db,
        &count.access,
        &command,
        count.task_id,
        7,
        Some("three units missing"),
    )
    .await
    .unwrap();
    let replay = repo::tasks::confirm_item_location_cycle_count_in_scope(
        &count.fixture.db,
        &count.access,
        &command,
        count.task_id,
        7,
        Some("three units missing"),
    )
    .await
    .unwrap();

    assert_eq!(replay, confirmation);
    assert_eq!(confirmation.tenant_id, count.access.tenant_id);
    assert_eq!(confirmation.inventory_owner_id.get(), count.owner_id);
    assert_eq!(confirmation.facility_id, count.facility_id);
    assert_eq!(confirmation.location_id, count.location_id);
    assert_eq!(confirmation.item_id, count.item_id);
    assert_eq!(confirmation.inventory_balance_id, count.balance_id);
    assert_eq!(confirmation.previous_on_hand_quantity, 10);
    assert_eq!(confirmation.reserved_quantity, 0);
    assert_eq!(confirmation.counted_quantity, 7);
    assert_eq!(confirmation.variance_quantity, -3);
    assert!(confirmation.inventory_transaction_id.is_some());
    assert_eq!(confirmation.confirmed_by, count.access.user_id.get());
    assert_eq!(confirmation.note.as_deref(), Some("three units missing"));

    let changed_retry = repo::tasks::confirm_item_location_cycle_count_in_scope(
        &count.fixture.db,
        &count.access,
        &command,
        count.task_id,
        8,
        Some("three units missing"),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        changed_retry,
        AppError::Application(ApplicationError::IdempotencyKeyReused)
    ));

    assert_eq!(count.balance_quantities().await, (7, 0));
    assert_eq!(count.task_status().await, "completed");
    assert_eq!(count.effect_counts().await, (1, 1, 1, 1, 1, 1, 1));
    let adjustment = repo::inventory::get_transactions(&count.fixture.db, count.access.tenant_id)
        .await
        .unwrap()
        .into_iter()
        .find(|transaction| transaction.operation == CONFIRM_OPERATION)
        .unwrap();
    assert_eq!(
        adjustment.transaction_type,
        InventoryTransactionType::Adjust
    );
    assert_eq!(adjustment.reference_id, Some(count.task_id));
    assert_eq!(adjustment.entries.len(), 1);
    assert_eq!(adjustment.entries[0].quantity_delta, -3);
    count.assert_reconciled().await;
}

#[tokio::test]
async fn exact_count_completes_without_a_journal_transaction() {
    let count = CycleCountFixture::new("exact", 10, 0).await;
    let transaction_count_before =
        repo::inventory::get_transactions(&count.fixture.db, count.access.tenant_id)
            .await
            .unwrap()
            .len();
    let token = auth::create_session(&count.fixture.db, count.access.user_id.get())
        .await
        .unwrap();
    let app = routes::app(AppState::new(count.fixture.db.clone()));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/tasks/cycle-counts/item-location/confirm")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, count.access.tenant_id.to_string())
                .header(IDEMPOTENCY_KEY_HEADER, "confirm-exact")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "task_id": count.task_id,
                        "counted_quantity": 10
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let confirmation = serde_json::from_slice::<
        wareboxes_core::models::ItemLocationCycleCountConfirmation,
    >(&to_bytes(response.into_body(), 128 * 1024).await.unwrap())
    .unwrap();

    assert_eq!(confirmation.previous_on_hand_quantity, 10);
    assert_eq!(confirmation.counted_quantity, 10);
    assert_eq!(confirmation.variance_quantity, 0);
    assert_eq!(confirmation.inventory_transaction_id, None);
    assert_eq!(count.balance_quantities().await, (10, 0));
    assert_eq!(count.task_status().await, "completed");
    assert_eq!(count.effect_counts().await, (0, 0, 1, 1, 1, 1, 0));
    assert_eq!(
        repo::inventory::get_transactions(&count.fixture.db, count.access.tenant_id)
            .await
            .unwrap()
            .len(),
        transaction_count_before
    );
    count.assert_reconciled().await;
}

#[tokio::test]
async fn count_below_reserved_quantity_rolls_back_every_effect() {
    let count = CycleCountFixture::new("reserved", 10, 6).await;
    let command = CycleCountFixture::command(&count.access, "confirm-below-reserved");
    let transaction_count_before =
        repo::inventory::get_transactions(&count.fixture.db, count.access.tenant_id)
            .await
            .unwrap()
            .len();

    let error = repo::tasks::confirm_item_location_cycle_count_in_scope(
        &count.fixture.db,
        &count.access,
        &command,
        count.task_id,
        5,
        Some("physical count is below committed demand"),
    )
    .await
    .unwrap_err();

    assert!(matches!(
        error,
        AppError::Application(ApplicationError::Conflict(_))
    ));
    assert_eq!(count.balance_quantities().await, (10, 6));
    assert_eq!(count.task_status().await, "in_progress");
    assert_eq!(count.effect_counts().await, (0, 0, 0, 0, 0, 0, 0));
    assert_eq!(
        repo::inventory::get_transactions(&count.fixture.db, count.access.tenant_id)
            .await
            .unwrap()
            .len(),
        transaction_count_before
    );
    count.assert_reconciled().await;
}

#[tokio::test]
async fn count_below_held_quantity_rolls_back_until_full_release() {
    let count = CycleCountFixture::new("held", 10, 0).await;
    let hold_id = count.place_hold(6).await;
    let command = CycleCountFixture::command(&count.access, "confirm-below-held");
    let transaction_count_before =
        repo::inventory::get_transactions(&count.fixture.db, count.access.tenant_id)
            .await
            .unwrap()
            .len();

    let error = repo::tasks::confirm_item_location_cycle_count_in_scope(
        &count.fixture.db,
        &count.access,
        &command,
        count.task_id,
        5,
        Some("physical count is below held quantity"),
    )
    .await
    .unwrap_err();

    assert!(matches!(
        error,
        AppError::Application(ApplicationError::Conflict(_))
    ));
    assert_eq!(count.balance_quantities().await, (10, 0));
    assert_eq!(count.held_quantity().await, 6);
    assert_eq!(count.task_status().await, "in_progress");
    assert_eq!(count.effect_counts().await, (0, 0, 0, 0, 0, 0, 0));
    assert_eq!(
        repo::inventory::get_transactions(&count.fixture.db, count.access.tenant_id)
            .await
            .unwrap()
            .len(),
        transaction_count_before
    );
    count.assert_reconciled().await;

    let released = repo::inventory::release_inventory_hold(
        &count.fixture.db,
        &count.access,
        &CycleCountFixture::command(&count.access, "release-cycle-count-hold"),
        &repo::inventory::ReleaseInventoryHoldCommand { hold_id },
    )
    .await
    .unwrap();
    assert_eq!(released.released_qty, 6);
    assert_eq!(count.held_quantity().await, 0);

    let confirmation = repo::tasks::confirm_item_location_cycle_count_in_scope(
        &count.fixture.db,
        &count.access,
        &command,
        count.task_id,
        5,
        Some("hold released after investigation"),
    )
    .await
    .unwrap();
    assert_eq!(confirmation.previous_on_hand_quantity, 10);
    assert_eq!(confirmation.held_quantity, 0);
    assert_eq!(confirmation.counted_quantity, 5);
    assert_eq!(confirmation.variance_quantity, -5);
    assert_eq!(count.balance_quantities().await, (5, 0));
    assert_eq!(count.held_quantity().await, 0);
    assert_eq!(count.task_status().await, "completed");
    assert_eq!(count.effect_counts().await, (1, 1, 1, 1, 1, 1, 1));
    count.assert_reconciled().await;
}

#[tokio::test]
async fn concurrent_different_keys_have_one_winner() {
    let count = CycleCountFixture::new("concurrent", 10, 0).await;
    let first_command = CycleCountFixture::command(&count.access, "confirm-concurrent-a");
    let second_command = CycleCountFixture::command(&count.access, "confirm-concurrent-b");

    let (first, second) = tokio::join!(
        repo::tasks::confirm_item_location_cycle_count_in_scope(
            &count.fixture.db,
            &count.access,
            &first_command,
            count.task_id,
            7,
            Some("first scanner"),
        ),
        repo::tasks::confirm_item_location_cycle_count_in_scope(
            &count.fixture.db,
            &count.access,
            &second_command,
            count.task_id,
            8,
            Some("second scanner"),
        ),
    );

    let winner = match (first, second) {
        (Ok(winner), Err(error)) | (Err(error), Ok(winner)) => {
            assert!(matches!(
                error,
                AppError::Application(ApplicationError::Conflict(_))
            ));
            winner
        }
        outcomes => panic!("expected one successful confirmation and one conflict: {outcomes:?}"),
    };
    assert!(matches!(winner.counted_quantity, 7 | 8));
    assert_eq!(
        count.balance_quantities().await,
        (winner.counted_quantity, 0)
    );
    assert_eq!(count.task_status().await, "completed");
    assert_eq!(count.effect_counts().await, (1, 1, 1, 1, 1, 1, 1));
    count.assert_reconciled().await;
}

#[tokio::test]
async fn license_plate_cycle_count_locks_plate_before_balance() {
    let fixture = Fixture::new().await;
    let user = fixture
        .wms_user("cycle-count-license-plate-lock-order@test.com")
        .await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let inventory_owner_id = fixture
        .inventory_owner(tenant_id, "Cycle Count LP Lock Owner")
        .await;
    let facility_id = fixture
        .facility(tenant_id, "Cycle Count LP Lock Facility")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let location_id = wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some("CYCLE-COUNT-LP-LOCK"),
        Some("Cycle Count LP Lock"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let item_id = fixture
        .item(tenant_id, "Cycle Count LP Lock Item", "each")
        .await;
    let license_plate_id = repo::license_plates::add_license_plate(
        &fixture.db,
        tenant_id,
        inventory_owner_id,
        facility_id,
        Some("CYCLE-COUNT-LP-LOCK"),
    )
    .await
    .unwrap();

    let load_id = repo::loads::add_load(
        &fixture.db,
        tenant_id,
        user.id,
        facility_id,
        inventory_owner_id,
        LoadType::Inbound,
        Some("CYCLE-COUNT-LP-LOCK-RECEIPT"),
        None,
        None,
        None,
        None,
        Some(location_id),
        None,
        None,
    )
    .await
    .unwrap();
    let load_line_id = repo::loads::add_line(
        &fixture.db,
        tenant_id,
        user.id,
        load_id,
        item_id,
        None,
        10,
        Some("CYCLE-COUNT-LP-LOCK-LOT"),
        None,
        None,
    )
    .await
    .unwrap();
    repo::loads::update_load(
        &fixture.db,
        tenant_id,
        user.id,
        load_id,
        Some(LoadStatus::Arrived),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    repo::inbound_receipt::receive_expected_inventory(
        &fixture.db,
        &access,
        &CycleCountFixture::command(&access, "cycle-count-lp-lock-receipt"),
        load_line_id,
        &repo::inbound_receipt::ReceiveExpectedInventoryCommand {
            receiving_location_id: Some(location_id),
            received_qty: 10,
            rejected_qty: 0,
            missing_qty: 0,
            license_plate_id: Some(license_plate_id),
            license_plate_barcode: None,
            lot: Some("CYCLE-COUNT-LP-LOCK-LOT"),
            serial: None,
            expiration: None,
            exception_reason: None::<InboundReceiptExceptionReason>,
            exception_note: None,
        },
    )
    .await
    .unwrap();
    let mut setup_tx = tenant_tx(&fixture.db, tenant_id).await;
    let inventory_balance_id: i64 = sqlx::query_scalar(
        r#"
        SELECT id
        FROM inventory_balances
        WHERE tenant_id = $1
          AND inventory_owner_id = $2
          AND license_plate_id = $3
          AND item_id = $4
          AND deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(license_plate_id)
    .bind(item_id)
    .fetch_one(&mut *setup_tx)
    .await
    .unwrap();
    setup_tx.rollback().await.unwrap();

    let task_id = repo::tasks::create_item_location_cycle_count_task(
        &fixture.db,
        tenant_id,
        user.id,
        location_id,
        item_id,
        Some("lock_order_regression"),
        None,
        None,
        inventory_balance_id,
        Some("count license-plated inventory"),
    )
    .await
    .unwrap();
    let start_context = CycleCountFixture::command(&access, "cycle-count-lp-lock-start");
    assert!(
        repo::tasks::start_task_in_scope(&fixture.db, &access, &start_context, task_id)
            .await
            .unwrap()
    );

    let mut plate_blocker = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("SELECT id FROM license_plates WHERE tenant_id = $1 AND id = $2 FOR UPDATE")
        .bind(tenant_id.get())
        .bind(license_plate_id)
        .fetch_one(&mut *plate_blocker)
        .await
        .unwrap();

    let confirm_db = fixture.db.clone();
    let confirm_access = access.clone();
    let confirm_context = CycleCountFixture::command(&access, "cycle-count-lp-lock-confirm");
    let mut confirmation = tokio::spawn(async move {
        repo::tasks::confirm_item_location_cycle_count_in_scope(
            &confirm_db,
            &confirm_access,
            &confirm_context,
            task_id,
            7,
            Some("three units missing"),
        )
        .await
    });

    wait_until_task_is_locked(&fixture.db, tenant_id, task_id).await;
    sleep(Duration::from_millis(50)).await;
    assert!(!confirmation.is_finished());

    let mut balance_probe = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        "SELECT id FROM inventory_balances WHERE tenant_id = $1 AND id = $2 FOR UPDATE NOWAIT",
    )
    .bind(tenant_id.get())
    .bind(inventory_balance_id)
    .fetch_one(&mut *balance_probe)
    .await
    .expect("confirmation must not lock the balance before its license plate");
    balance_probe.rollback().await.unwrap();

    plate_blocker.commit().await.unwrap();
    let result = timeout(Duration::from_secs(2), &mut confirmation)
        .await
        .expect("cycle count confirmation completes after the license plate lock is released")
        .unwrap()
        .unwrap();
    assert_eq!(result.inventory_balance_id, inventory_balance_id);
    assert_eq!(result.license_plate_id, Some(license_plate_id));
    assert_eq!(result.previous_on_hand_quantity, 10);
    assert_eq!(result.counted_quantity, 7);
    assert_eq!(result.variance_quantity, -3);
    assert!(
        repo::inventory::get_reconciliation_issues(&fixture.db, tenant_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        repo::inventory::get_inventory_hold_reconciliation_issues_in_scope(&fixture.db, &access)
            .await
            .unwrap()
            .is_empty()
    );
}
