mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use tokio::sync::Barrier;
use tokio::time::timeout;
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InboundReceiptExceptionReason, InventoryHoldReason};

const RACE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
struct PlateSetup {
    license_plate_id: i64,
    inventory_balance_id: i64,
    source_location_id: i64,
    destination_location_id: i64,
    license_plate_barcode: String,
    destination_location_barcode: String,
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct TaskState {
    task_status: String,
    plate_location_id: Option<i64>,
    balance_location_id: i64,
    qty_on_hand: i64,
    qty_reserved: i64,
    qty_held: i64,
    results: i64,
    transactions: i64,
    entries: i64,
    entry_net: i64,
    progress: i64,
    command_records: i64,
    projection_changes: i64,
    inventory_mismatches: i64,
    hold_mismatches: i64,
    allocation_mismatches: i64,
}

fn command(access: &wareboxes_core::models::TenantAccess, key: &str) -> CommandContext {
    CommandContext {
        tenant_id: access.tenant_id,
        actor_id: access.user_id,
        request_id: format!("request-{key}"),
        idempotency_key: Some(key.to_owned()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn received_plate(
    fixture: &Fixture,
    access: &wareboxes_core::models::TenantAccess,
    inventory_owner_id: i64,
    facility_id: i64,
    source_location_id: i64,
    item_id: i64,
    key: &str,
) -> PlateSetup {
    let license_plate_id = repo::license_plates::add_license_plate(
        &fixture.db,
        access.tenant_id,
        inventory_owner_id,
        facility_id,
        Some(key),
    )
    .await
    .unwrap();
    let destination_location_barcode = format!("{key}-DESTINATION");
    let destination_location_id = fixture
        .location(access.tenant_id, facility_id, &destination_location_barcode)
        .await;
    let load_id = repo::loads::add_load(
        &fixture.db,
        access.tenant_id,
        access.user_id.get(),
        facility_id,
        inventory_owner_id,
        LoadType::Inbound,
        Some(key),
        None,
        None,
        None,
        None,
        Some(source_location_id),
        None,
        None,
    )
    .await
    .unwrap();
    let load_line_id = repo::loads::add_line(
        &fixture.db,
        access.tenant_id,
        access.user_id.get(),
        load_id,
        item_id,
        None,
        10,
        Some(key),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(repo::loads::update_load(
        &fixture.db,
        access.tenant_id,
        access.user_id.get(),
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
    .unwrap());
    let receipt = repo::inbound_receipt::receive_expected_inventory(
        &fixture.db,
        access,
        &command(access, &format!("{key}-receipt")),
        load_line_id,
        &repo::inbound_receipt::ReceiveExpectedInventoryCommand {
            receiving_location_id: Some(source_location_id),
            received_qty: 10,
            rejected_qty: 0,
            missing_qty: 0,
            license_plate_id: Some(license_plate_id),
            license_plate_barcode: None,
            lot: Some(key),
            serial: None,
            expiration: None,
            exception_reason: None::<InboundReceiptExceptionReason>,
            exception_note: None,
        },
    )
    .await
    .unwrap();

    PlateSetup {
        license_plate_id,
        inventory_balance_id: receipt
            .inventory_balance_id
            .expect("containerized receipt identifies its balance"),
        source_location_id,
        destination_location_id,
        license_plate_barcode: key.to_owned(),
        destination_location_barcode,
    }
}

async fn plan_and_claim(
    fixture: &Fixture,
    access: &wareboxes_core::models::TenantAccess,
    plate: &PlateSetup,
    key: &str,
) -> i64 {
    let task_id = repo::tasks::create_license_plate_putaway_task_in_scope(
        &fixture.db,
        access,
        &command(access, &format!("{key}-create")),
        plate.license_plate_id,
        plate.destination_location_id,
        50,
        Some(access.user_id.get()),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(repo::tasks::start_task_in_scope(
        &fixture.db,
        access,
        &command(access, &format!("{key}-start")),
        task_id,
    )
    .await
    .unwrap());
    task_id
}

async fn confirm(
    db: &db::Db,
    access: &wareboxes_core::models::TenantAccess,
    task_id: i64,
    plate: &PlateSetup,
    key: &str,
) -> Result<wareboxes_core::models::LicensePlatePutawayConfirmation, AppError> {
    repo::tasks::confirm_license_plate_putaway_in_scope(
        db,
        access,
        &command(access, key),
        task_id,
        &plate.license_plate_barcode,
        &plate.destination_location_barcode,
    )
    .await
}

async fn task_state(
    db: &db::Db,
    tenant_id: TenantId,
    task_id: i64,
    plate: &PlateSetup,
) -> TaskState {
    let mut tx = tenant_tx(db, tenant_id).await;
    let state = sqlx::query_as(
        r#"
        SELECT
            (
                SELECT status
                FROM work_tasks
                WHERE tenant_id = $1 AND id = $2
            ) AS task_status,
            (
                SELECT location_id
                FROM license_plates
                WHERE tenant_id = $1 AND id = $3
            ) AS plate_location_id,
            (
                SELECT location_id
                FROM inventory_balances
                WHERE tenant_id = $1 AND id = $4
            ) AS balance_location_id,
            (
                SELECT qty_on_hand
                FROM inventory_balances
                WHERE tenant_id = $1 AND id = $4
            ) AS qty_on_hand,
            (
                SELECT qty_reserved
                FROM inventory_balances
                WHERE tenant_id = $1 AND id = $4
            ) AS qty_reserved,
            (
                SELECT qty_held
                FROM inventory_balances
                WHERE tenant_id = $1 AND id = $4
            ) AS qty_held,
            (
                SELECT COUNT(*)
                FROM license_plate_putaway_results
                WHERE tenant_id = $1 AND task_id = $2
            ) AS results,
            (
                SELECT COUNT(*)
                FROM inventory_transactions
                WHERE tenant_id = $1
                  AND operation =
                      'task.confirm_license_plate_putaway.v1'
                  AND reference_type = 'license_plate_putaway_task'
                  AND reference_id = $2
            ) AS transactions,
            (
                SELECT COUNT(*)
                FROM inventory_entries entry
                INNER JOIN inventory_transactions transaction
                    ON transaction.tenant_id = entry.tenant_id
                   AND transaction.inventory_owner_id =
                       entry.inventory_owner_id
                   AND transaction.id = entry.transaction_id
                WHERE transaction.tenant_id = $1
                  AND transaction.operation =
                      'task.confirm_license_plate_putaway.v1'
                  AND transaction.reference_type =
                      'license_plate_putaway_task'
                  AND transaction.reference_id = $2
            ) AS entries,
            (
                SELECT COALESCE(SUM(entry.quantity_delta), 0)::BIGINT
                FROM inventory_entries entry
                INNER JOIN inventory_transactions transaction
                    ON transaction.tenant_id = entry.tenant_id
                   AND transaction.inventory_owner_id =
                       entry.inventory_owner_id
                   AND transaction.id = entry.transaction_id
                WHERE transaction.tenant_id = $1
                  AND transaction.operation =
                      'task.confirm_license_plate_putaway.v1'
                  AND transaction.reference_type =
                      'license_plate_putaway_task'
                  AND transaction.reference_id = $2
            ) AS entry_net,
            (
                SELECT COUNT(*)
                FROM work_task_progress
                WHERE tenant_id = $1
                  AND task_id = $2
                  AND action = 'license_plate_putaway_confirmed'
            ) AS progress,
            (
                SELECT COUNT(*)
                FROM command_idempotency_records
                WHERE tenant_id = $1
                  AND operation =
                      'task.confirm_license_plate_putaway.v1'
                  AND result_json->>'task_id' = $2::TEXT
            ) AS command_records,
            (
                SELECT COUNT(*)
                FROM inventory_projection_changes change
                INNER JOIN inventory_transactions transaction
                    ON transaction.tenant_id = change.tenant_id
                   AND transaction.inventory_owner_id =
                       change.inventory_owner_id
                   AND transaction.id = change.transaction_id
                WHERE transaction.tenant_id = $1
                  AND transaction.operation =
                      'task.confirm_license_plate_putaway.v1'
                  AND transaction.reference_id = $2
            ) AS projection_changes,
            (SELECT COUNT(*) FROM inventory_reconciliation)
                AS inventory_mismatches,
            (SELECT COUNT(*) FROM inventory_hold_reconciliation)
                AS hold_mismatches,
            (
                SELECT COUNT(*)
                FROM inventory_balances balance
                WHERE balance.tenant_id = $1
                  AND balance.qty_reserved IS DISTINCT FROM COALESCE((
                      SELECT SUM(allocation.qty)
                      FROM inventory_allocations allocation
                      WHERE allocation.tenant_id = balance.tenant_id
                        AND allocation.inventory_balance_id = balance.id
                        AND allocation.deleted IS NULL
                        AND allocation.status = 'allocated'
                  ), 0)
            ) AS allocation_mismatches
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .bind(plate.license_plate_id)
    .bind(plate.inventory_balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    state
}

fn assert_conflict(error: AppError, expected: &str) {
    assert!(
        matches!(
            error,
            AppError::Core(CoreError::Conflict(ref message)) if message.contains(expected)
        ),
        "unexpected concurrency rejection: {error:?}"
    );
}

fn assert_task_state(
    state: &TaskState,
    moved: bool,
    source_location_id: i64,
    destination_location_id: i64,
) {
    let expected_location = if moved {
        destination_location_id
    } else {
        source_location_id
    };
    assert_eq!(state.plate_location_id, Some(expected_location));
    assert_eq!(state.balance_location_id, expected_location);
    assert_eq!(state.qty_on_hand, 10);
    assert_eq!(state.entry_net, 0);
    assert_eq!(state.inventory_mismatches, 0);
    assert_eq!(state.hold_mismatches, 0);
    assert_eq!(state.allocation_mismatches, 0);
    if moved {
        assert_eq!(state.task_status, "completed");
        assert_eq!(state.results, 1);
        assert_eq!(state.transactions, 1);
        assert_eq!(state.entries, 2);
        assert_eq!(state.progress, 1);
        assert_eq!(state.command_records, 1);
        assert_eq!(state.projection_changes, 2);
    } else {
        assert_eq!(state.task_status, "in_progress");
        assert_eq!(state.results, 0);
        assert_eq!(state.transactions, 0);
        assert_eq!(state.entries, 0);
        assert_eq!(state.progress, 0);
        assert_eq!(state.command_records, 0);
        assert_eq!(state.projection_changes, 0);
    }
}

#[tokio::test]
async fn license_plate_putaway_races_serialize_without_duplicate_inventory_effects() {
    let fixture = Fixture::new().await;
    let user = fixture
        .wms_user("license-plate-putaway-concurrency@test.local")
        .await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let tenant_id = access.tenant_id;
    let inventory_owner_id = fixture
        .inventory_owner(tenant_id, "License Plate Putaway Concurrency Owner")
        .await;
    let facility_id = fixture
        .facility(tenant_id, "License Plate Putaway Concurrency Facility")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let source_location_id = wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some("LP-PUTAWAY-CONCURRENCY-RECEIVING"),
        Some("License Plate Putaway Concurrency Receiving"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let item_id = fixture
        .item(tenant_id, "License Plate Putaway Concurrency Item", "each")
        .await;

    let confirmation_plate = received_plate(
        &fixture,
        &access,
        inventory_owner_id,
        facility_id,
        source_location_id,
        item_id,
        "LP-PUTAWAY-CONFIRMATION-RACE",
    )
    .await;
    let confirmation_task = plan_and_claim(
        &fixture,
        &access,
        &confirmation_plate,
        "lp-putaway-confirmation-race",
    )
    .await;
    let confirmation_barrier = Arc::new(Barrier::new(3));
    let mut attempts = Vec::new();
    for key in [
        "lp-putaway-confirmation-race-a",
        "lp-putaway-confirmation-race-b",
    ] {
        let db = fixture.db.clone();
        let access = access.clone();
        let plate = confirmation_plate.clone();
        let barrier = Arc::clone(&confirmation_barrier);
        attempts.push(tokio::spawn(async move {
            barrier.wait().await;
            confirm(&db, &access, confirmation_task, &plate, key).await
        }));
    }
    confirmation_barrier.wait().await;
    let results = timeout(RACE_TIMEOUT, async {
        let first = attempts.remove(0).await.unwrap();
        let second = attempts.remove(0).await.unwrap();
        [first, second]
    })
    .await
    .expect("distinct confirmation keys serialize without deadlock");
    let mut winner = None;
    let mut conflicts = 0;
    for result in results {
        match result {
            Ok(result) => {
                assert!(winner.replace(result).is_none());
            }
            Err(error) => {
                assert_conflict(error, "active claim");
                conflicts += 1;
            }
        }
    }
    let winner = winner.expect("exactly one putaway confirmation wins");
    assert_eq!(conflicts, 1);
    assert_eq!(winner.task_id, confirmation_task);
    assert_eq!(
        winner.destination_location_id,
        confirmation_plate.destination_location_id
    );
    let confirmation_state = task_state(
        &fixture.db,
        tenant_id,
        confirmation_task,
        &confirmation_plate,
    )
    .await;
    assert_task_state(
        &confirmation_state,
        true,
        confirmation_plate.source_location_id,
        confirmation_plate.destination_location_id,
    );
    assert_eq!(
        (confirmation_state.qty_reserved, confirmation_state.qty_held),
        (0, 0)
    );

    let guarded_plate = received_plate(
        &fixture,
        &access,
        inventory_owner_id,
        facility_id,
        source_location_id,
        item_id,
        "LP-PUTAWAY-ACTIVE-TASK-GUARD",
    )
    .await;
    let guarded_task = plan_and_claim(
        &fixture,
        &access,
        &guarded_plate,
        "lp-putaway-active-task-guard",
    )
    .await;
    let generic_move = repo::license_plates::move_license_plate(
        &fixture.db,
        tenant_id,
        user.id,
        guarded_plate.license_plate_id,
        guarded_plate.destination_location_id,
        Some("active directed putaway must own movement"),
        "lp-putaway-active-task-generic-move",
    )
    .await
    .unwrap_err();
    assert_conflict(generic_move, "active directed putaway work");
    let guarded_before_confirmation =
        task_state(&fixture.db, tenant_id, guarded_task, &guarded_plate).await;
    assert_task_state(
        &guarded_before_confirmation,
        false,
        guarded_plate.source_location_id,
        guarded_plate.destination_location_id,
    );
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let generic_move_transactions = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COUNT(*)
        FROM inventory_transactions
        WHERE tenant_id = $1
          AND operation = 'move_license_plate'
          AND reference_id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(guarded_plate.license_plate_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(generic_move_transactions, 0);
    confirm(
        &fixture.db,
        &access,
        guarded_task,
        &guarded_plate,
        "lp-putaway-active-task-confirm",
    )
    .await
    .unwrap();

    let hold_plate = received_plate(
        &fixture,
        &access,
        inventory_owner_id,
        facility_id,
        source_location_id,
        item_id,
        "LP-PUTAWAY-HOLD-RACE",
    )
    .await;
    let hold_task = plan_and_claim(&fixture, &access, &hold_plate, "lp-putaway-hold-race").await;
    let hold_barrier = Arc::new(Barrier::new(3));
    let confirm_db = fixture.db.clone();
    let confirm_access = access.clone();
    let confirm_plate = hold_plate.clone();
    let confirm_barrier = Arc::clone(&hold_barrier);
    let confirmation_attempt = tokio::spawn(async move {
        confirm_barrier.wait().await;
        confirm(
            &confirm_db,
            &confirm_access,
            hold_task,
            &confirm_plate,
            "lp-putaway-hold-race-confirm",
        )
        .await
    });
    let hold_db = fixture.db.clone();
    let hold_access = access.clone();
    let hold_balance_id = hold_plate.inventory_balance_id;
    let placement_barrier = Arc::clone(&hold_barrier);
    let hold_attempt = tokio::spawn(async move {
        placement_barrier.wait().await;
        repo::inventory::place_inventory_hold(
            &hold_db,
            &hold_access,
            &command(&hold_access, "lp-putaway-hold-race-place"),
            &repo::inventory::PlaceInventoryHoldCommand {
                inventory_balance_id: hold_balance_id,
                qty: 1,
                reason: InventoryHoldReason::QualityInspection,
                note: Some("concurrent license plate putaway"),
                reference_type: Some("license_plate_putaway_task"),
                reference_id: Some(hold_task),
            },
        )
        .await
    });
    hold_barrier.wait().await;
    let (hold_confirmation, hold_result) = timeout(RACE_TIMEOUT, async {
        (
            confirmation_attempt.await.unwrap(),
            hold_attempt.await.unwrap(),
        )
    })
    .await
    .expect("hold placement and putaway confirmation serialize without deadlock");
    let hold_result = hold_result.expect("hold placement wins or follows the physical move");
    let hold_moved = match hold_confirmation {
        Ok(_) => true,
        Err(error) => {
            assert_conflict(error, "reserved or held inventory");
            false
        }
    };
    let hold_state = task_state(&fixture.db, tenant_id, hold_task, &hold_plate).await;
    assert_task_state(
        &hold_state,
        hold_moved,
        hold_plate.source_location_id,
        hold_plate.destination_location_id,
    );
    assert_eq!((hold_state.qty_reserved, hold_state.qty_held), (0, 1));
    let expected_hold_location = if hold_moved {
        hold_plate.destination_location_id
    } else {
        hold_plate.source_location_id
    };
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let hold_location: i64 = sqlx::query_scalar(
        r#"
        SELECT location_id
        FROM inventory_holds
        WHERE tenant_id = $1
          AND id = $2
          AND inventory_balance_id = $3
          AND status = 'active'
        "#,
    )
    .bind(tenant_id.get())
    .bind(hold_result.hold_id)
    .bind(hold_plate.inventory_balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(hold_location, expected_hold_location);

    repo::inventory::release_inventory_hold(
        &fixture.db,
        &access,
        &command(&access, "lp-putaway-hold-race-release"),
        &repo::inventory::ReleaseInventoryHoldCommand {
            hold_id: hold_result.hold_id,
        },
    )
    .await
    .unwrap();
    if !hold_moved {
        confirm(
            &fixture.db,
            &access,
            hold_task,
            &hold_plate,
            "lp-putaway-hold-race-recovery-confirm",
        )
        .await
        .unwrap();
    }

    let allocation_plate = received_plate(
        &fixture,
        &access,
        inventory_owner_id,
        facility_id,
        source_location_id,
        item_id,
        "LP-PUTAWAY-ALLOCATION-RACE",
    )
    .await;
    let order_id = fixture
        .order(
            tenant_id,
            "LP-PUTAWAY-ALLOCATION-RACE-ORDER",
            inventory_owner_id,
        )
        .await;
    let order_item_id = fixture.order_item(tenant_id, order_id, item_id, 1).await;
    let reservation = repo::inventory::create_inventory_reservation(
        &fixture.db,
        &access,
        &repo::inventory::CreateInventoryReservationCommand {
            order_id,
            order_item_id,
            facility_id,
            qty: 1,
            idempotency_key: "lp-putaway-allocation-race-reservation",
        },
    )
    .await
    .unwrap();
    let allocation_task = plan_and_claim(
        &fixture,
        &access,
        &allocation_plate,
        "lp-putaway-allocation-race",
    )
    .await;
    let allocation_barrier = Arc::new(Barrier::new(3));
    let confirm_db = fixture.db.clone();
    let confirm_access = access.clone();
    let confirm_plate = allocation_plate.clone();
    let confirm_barrier = Arc::clone(&allocation_barrier);
    let confirmation_attempt = tokio::spawn(async move {
        confirm_barrier.wait().await;
        confirm(
            &confirm_db,
            &confirm_access,
            allocation_task,
            &confirm_plate,
            "lp-putaway-allocation-race-confirm",
        )
        .await
    });
    let allocation_db = fixture.db.clone();
    let allocation_access = access.clone();
    let allocation_balance_id = allocation_plate.inventory_balance_id;
    let allocation_attempt_barrier = Arc::clone(&allocation_barrier);
    let allocation_attempt = tokio::spawn(async move {
        allocation_attempt_barrier.wait().await;
        repo::inventory::allocate_inventory(
            &allocation_db,
            &allocation_access,
            &repo::inventory::AllocateInventoryCommand {
                reservation_id: reservation.reservation_id,
                inventory_balance_id: allocation_balance_id,
                qty: 1,
                idempotency_key: "lp-putaway-allocation-race-allocate",
            },
        )
        .await
    });
    allocation_barrier.wait().await;
    let (allocation_confirmation, allocation_result) = timeout(RACE_TIMEOUT, async {
        (
            confirmation_attempt.await.unwrap(),
            allocation_attempt.await.unwrap(),
        )
    })
    .await
    .expect("allocation and putaway confirmation serialize without deadlock");
    let allocation_result =
        allocation_result.expect("allocation wins or follows the physical move");
    let allocation_moved = match allocation_confirmation {
        Ok(_) => true,
        Err(error) => {
            assert_conflict(error, "reserved or held inventory");
            false
        }
    };
    let allocation_state =
        task_state(&fixture.db, tenant_id, allocation_task, &allocation_plate).await;
    assert_task_state(
        &allocation_state,
        allocation_moved,
        allocation_plate.source_location_id,
        allocation_plate.destination_location_id,
    );
    assert_eq!(
        (allocation_state.qty_reserved, allocation_state.qty_held),
        (1, 0)
    );
    let expected_allocation_location = if allocation_moved {
        allocation_plate.destination_location_id
    } else {
        allocation_plate.source_location_id
    };
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let allocation_location: i64 = sqlx::query_scalar(
        r#"
        SELECT location_id
        FROM inventory_allocations
        WHERE tenant_id = $1
          AND id = $2
          AND inventory_balance_id = $3
          AND status = 'allocated'
        "#,
    )
    .bind(tenant_id.get())
    .bind(allocation_result.allocation_id)
    .bind(allocation_plate.inventory_balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(allocation_location, expected_allocation_location);
}
