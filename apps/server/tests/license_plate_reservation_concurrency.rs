mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use tokio::sync::{oneshot, Barrier};
use tokio::time::{sleep, timeout};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InboundReceiptExceptionReason, InventoryHoldReason};

async fn wait_until_balance_is_locked(db: &db::Db, tenant_id: TenantId, inventory_balance_id: i64) {
    timeout(Duration::from_secs(2), async {
        loop {
            let mut tx = tenant_tx(db, tenant_id).await;
            let result = sqlx::query(
                r#"
                SELECT id
                FROM inventory_balances
                WHERE tenant_id = $1 AND id = $2
                FOR UPDATE NOWAIT
                "#,
            )
            .bind(tenant_id.get())
            .bind(inventory_balance_id)
            .fetch_one(&mut *tx)
            .await;
            tx.rollback().await.unwrap();

            match result {
                Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("55P03") => {
                    break;
                }
                Ok(_) => sleep(Duration::from_millis(10)).await,
                Err(error) => panic!("unexpected balance lock probe error: {error}"),
            }
        }
    })
    .await
    .expect("license plate move locks its content balance");
}

fn assert_commitment_dimension_guard(error: &sqlx::Error) {
    assert!(
        error
            .to_string()
            .contains("committed inventory balance dimensions are immutable"),
        "unexpected balance dimension update error: {error}"
    );
}

#[tokio::test]
async fn license_plate_moves_serialize_allocations_and_holds() {
    let fixture = Fixture::new().await;
    let user = fixture
        .wms_user("license-plate-reservation-race@test.local")
        .await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let inventory_owner_id = fixture
        .inventory_owner(tenant_id, "LP Reservation Race Owner")
        .await;
    let facility_id = fixture
        .facility(tenant_id, "LP Reservation Race Facility")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let source_location_id = wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some("LP-RACE-SOURCE"),
        Some("LP Race Source"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let destination_location_id = fixture
        .location(tenant_id, facility_id, "LP-RACE-DESTINATION")
        .await;
    let other_facility_id = fixture
        .facility(tenant_id, "LP Reservation Other Facility")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, other_facility_id)
        .await;
    let other_facility_location_id = fixture
        .location(tenant_id, other_facility_id, "LP-RACE-OTHER-FACILITY")
        .await;

    let item_id = fixture
        .item(tenant_id, "LP Reservation Race Item", "each")
        .await;
    let other_item_id = fixture
        .item(tenant_id, "LP Reservation Other Item", "each")
        .await;
    let other_item_batch_id = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        inventory_owner_id,
        other_item_id,
        None,
        Some("LP-RACE-OTHER-LOT"),
        None,
        None,
    )
    .await
    .unwrap();
    let license_plate_id = repo::license_plates::add_license_plate(
        &fixture.db,
        tenant_id,
        inventory_owner_id,
        facility_id,
        Some("LP-RESERVATION-RACE"),
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
        Some("LP-RESERVATION-RACE-RECEIPT"),
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
        tenant_id,
        user.id,
        load_id,
        item_id,
        None,
        5,
        Some("LP-RACE-LOT"),
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
    start_expected_receipt_unloading(
        &fixture.db,
        &access,
        load_id,
        source_location_id,
        "license-plate-reservation-race-unloading",
    )
    .await;
    let receipt = repo::inbound_receipt::receive_expected_inventory(
        &fixture.db,
        &access,
        &CommandContext {
            tenant_id,
            actor_id: access.user_id,
            request_id: "request-license-plate-reservation-race-receipt".into(),
            idempotency_key: Some("license-plate-reservation-race-receipt".into()),
        },
        load_line_id,
        &repo::inbound_receipt::ReceiveExpectedInventoryCommand {
            receiving_location_id: Some(source_location_id),
            received_qty: 5,
            rejected_qty: 0,
            missing_qty: 0,
            license_plate_id: Some(license_plate_id),
            license_plate_barcode: None,
            lot: Some("LP-RACE-LOT"),
            serial: None,
            expiration: None,
            exception_reason: None::<InboundReceiptExceptionReason>,
            exception_note: None,
        },
    )
    .await
    .unwrap();
    let item_batch_id = receipt.item_batch_id.unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let inventory_balance_id: i64 = sqlx::query_scalar(
        r#"
        SELECT id
        FROM inventory_balances
        WHERE tenant_id = $1
          AND inventory_owner_id = $2
          AND license_plate_id = $3
          AND item_batch_id = $4
          AND deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(license_plate_id)
    .bind(item_batch_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();

    let order_id = fixture
        .order_header(tenant_id, "LP-RESERVATION-RACE-ORDER", inventory_owner_id)
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
            idempotency_key: "license-plate-reservation-race-reserve",
        },
    )
    .await
    .unwrap();
    let reservation_id = reservation.reservation_id;

    let advisory_key = format!(
        "inventory-location-item:{tenant_id}:{inventory_owner_id}:{destination_location_id}:{item_id}"
    );
    let mut move_blocker = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(advisory_key)
        .execute(&mut *move_blocker)
        .await
        .unwrap();

    let move_barrier = Arc::new(Barrier::new(2));
    let spawned_move_barrier = Arc::clone(&move_barrier);
    let move_db = fixture.db.clone();
    let (move_started, move_started_rx) = oneshot::channel();
    let moving_plate = tokio::spawn(async move {
        spawned_move_barrier.wait().await;
        move_started.send(()).unwrap();
        repo::license_plates::move_license_plate(
            &move_db,
            tenant_id,
            user.id,
            license_plate_id,
            destination_location_id,
            Some("concurrency test"),
            "license-plate-reservation-race-move",
        )
        .await
    });
    move_barrier.wait().await;
    move_started_rx.await.unwrap();
    wait_until_balance_is_locked(&fixture.db, tenant_id, inventory_balance_id).await;

    let allocation_barrier = Arc::new(Barrier::new(2));
    let spawned_allocation_barrier = Arc::clone(&allocation_barrier);
    let allocation_db = fixture.db.clone();
    let (allocation_started, allocation_started_rx) = oneshot::channel();
    let mut allocation = tokio::spawn(async move {
        spawned_allocation_barrier.wait().await;
        allocation_started.send(()).unwrap();
        repo::inventory::allocate_inventory(
            &allocation_db,
            &access,
            &repo::inventory::AllocateInventoryCommand {
                reservation_id,
                inventory_balance_id,
                qty: 1,
                idempotency_key: "license-plate-reservation-race-allocation",
            },
        )
        .await
    });
    allocation_barrier.wait().await;
    allocation_started_rx.await.unwrap();
    assert!(
        timeout(Duration::from_millis(250), &mut allocation)
            .await
            .is_err(),
        "allocation committed while the license plate held the balance lock"
    );

    move_blocker.commit().await.unwrap();
    timeout(Duration::from_secs(3), moving_plate)
        .await
        .expect("license plate move completes after its advisory lock is released")
        .unwrap()
        .unwrap();
    let allocation_id = timeout(Duration::from_secs(3), allocation)
        .await
        .expect("allocation completes after the license plate move")
        .unwrap()
        .unwrap()
        .allocation_id;

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let (allocation_facility, allocation_location, allocation_batch): (i64, i64, i64) =
        sqlx::query_as(
            r#"
            SELECT facility_id, location_id, item_batch_id
            FROM inventory_allocations
            WHERE tenant_id = $1 AND id = $2 AND status = 'allocated'
            "#,
        )
        .bind(tenant_id.get())
        .bind(allocation_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(allocation_facility, facility_id);
    assert_eq!(allocation_location, destination_location_id);
    assert_eq!(allocation_batch, item_batch_id);

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let error = sqlx::query(
        r#"
        UPDATE inventory_balances
        SET location_id = $1
        WHERE tenant_id = $2 AND id = $3
        "#,
    )
    .bind(source_location_id)
    .bind(tenant_id.get())
    .bind(inventory_balance_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert_commitment_dimension_guard(&error);
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let error = sqlx::query(
        r#"
        UPDATE inventory_balances
        SET item_batch_id = $1, item_id = $2
        WHERE tenant_id = $3 AND id = $4
        "#,
    )
    .bind(other_item_batch_id)
    .bind(other_item_id)
    .bind(tenant_id.get())
    .bind(inventory_balance_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert_commitment_dimension_guard(&error);
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let error = sqlx::query(
        r#"
        UPDATE inventory_balances
        SET facility_id = $1, location_id = $2
        WHERE tenant_id = $3 AND id = $4
        "#,
    )
    .bind(other_facility_id)
    .bind(other_facility_location_id)
    .bind(tenant_id.get())
    .bind(inventory_balance_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert_commitment_dimension_guard(&error);
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let dimensions_match: bool = sqlx::query_scalar(
        r#"
        SELECT allocation.facility_id = balance.facility_id
           AND allocation.location_id = balance.location_id
           AND allocation.item_batch_id = balance.item_batch_id
           AND balance.item_id = batch.item_id
        FROM inventory_allocations allocation
        INNER JOIN inventory_balances balance
            ON balance.tenant_id = allocation.tenant_id
           AND balance.inventory_owner_id = allocation.inventory_owner_id
           AND balance.id = allocation.inventory_balance_id
        INNER JOIN item_batches batch
            ON batch.tenant_id = balance.tenant_id
           AND batch.inventory_owner_id = balance.inventory_owner_id
           AND batch.id = balance.item_batch_id
        WHERE allocation.tenant_id = $1
          AND allocation.id = $2
          AND allocation.deleted IS NULL
          AND allocation.status = 'allocated'
        "#,
    )
    .bind(tenant_id.get())
    .bind(allocation_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert!(dimensions_match);

    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    repo::inventory::cancel_inventory_allocation(
        &fixture.db,
        &access,
        &repo::inventory::CancelInventoryAllocationCommand {
            allocation_id,
            idempotency_key: "license-plate-hold-release-allocation",
        },
    )
    .await
    .unwrap();

    let allocation_move_barrier = Arc::new(Barrier::new(3));
    let allocation_db = fixture.db.clone();
    let allocation_access = access.clone();
    let allocation_barrier = Arc::clone(&allocation_move_barrier);
    let allocation_attempt = tokio::spawn(async move {
        allocation_barrier.wait().await;
        repo::inventory::allocate_inventory(
            &allocation_db,
            &allocation_access,
            &repo::inventory::AllocateInventoryCommand {
                reservation_id,
                inventory_balance_id,
                qty: 1,
                idempotency_key: "license-plate-allocation-first-race",
            },
        )
        .await
    });
    let move_db = fixture.db.clone();
    let move_barrier = Arc::clone(&allocation_move_barrier);
    let move_attempt = tokio::spawn(async move {
        move_barrier.wait().await;
        repo::license_plates::move_license_plate(
            &move_db,
            tenant_id,
            user.id,
            license_plate_id,
            source_location_id,
            Some("concurrent allocation"),
            "license-plate-allocation-first-move",
        )
        .await
    });
    allocation_move_barrier.wait().await;
    let (allocation_result, move_result) = timeout(Duration::from_secs(3), async {
        (
            allocation_attempt.await.unwrap(),
            move_attempt.await.unwrap(),
        )
    })
    .await
    .expect("allocation and license plate move serialize without deadlock");
    let allocation_id = allocation_result.unwrap().allocation_id;
    let expected_location_id = match move_result {
        Ok(_) => source_location_id,
        Err(AppError::Application(ApplicationError::Conflict(message))) => {
            assert!(message.contains("reserved or held inventory"));
            destination_location_id
        }
        Err(error) => panic!("unexpected concurrent license plate move error: {error:?}"),
    };

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let (allocation_location_id, balance_location_id, qty_reserved): (i64, i64, i64) =
        sqlx::query_as(
            r#"
            SELECT allocation.location_id, balance.location_id, balance.qty_reserved
            FROM inventory_allocations allocation
            INNER JOIN inventory_balances balance
                ON balance.tenant_id = allocation.tenant_id
               AND balance.inventory_owner_id = allocation.inventory_owner_id
               AND balance.id = allocation.inventory_balance_id
            WHERE allocation.tenant_id = $1
              AND allocation.id = $2
              AND allocation.deleted IS NULL
              AND allocation.status = 'allocated'
            "#,
        )
        .bind(tenant_id.get())
        .bind(allocation_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(allocation_location_id, expected_location_id);
    assert_eq!(balance_location_id, expected_location_id);
    assert_eq!(qty_reserved, 1);

    repo::inventory::cancel_inventory_allocation(
        &fixture.db,
        &access,
        &repo::inventory::CancelInventoryAllocationCommand {
            allocation_id,
            idempotency_key: "license-plate-allocation-first-cancel",
        },
    )
    .await
    .unwrap();
    if expected_location_id == source_location_id {
        repo::license_plates::move_license_plate(
            &fixture.db,
            tenant_id,
            user.id,
            license_plate_id,
            destination_location_id,
            Some("restore hold race origin"),
            "license-plate-allocation-first-restore",
        )
        .await
        .unwrap();
    }

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let hold_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_holds (
            tenant_id, inventory_owner_id, created, modified, created_by,
            inventory_balance_id, facility_id, location_id, license_plate_id,
            item_batch_id, item_id, uom, inventory_status, qty, reason_code,
            note, status
        )
        SELECT balance.tenant_id, balance.inventory_owner_id, $1, $1, $2,
               balance.id, balance.facility_id, balance.location_id,
               balance.license_plate_id, balance.item_batch_id, balance.item_id,
               balance.uom, balance.status, 1, 'quality_inspection',
               'license plate movement guard', 'active'
        FROM inventory_balances balance
        WHERE balance.tenant_id = $3 AND balance.id = $4
        RETURNING id
        "#,
    )
    .bind(db::now_iso())
    .bind(user.id)
    .bind(tenant_id.get())
    .bind(inventory_balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let (qty_reserved, qty_held): (i64, i64) = sqlx::query_as(
        "SELECT qty_reserved, qty_held FROM inventory_balances WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id.get())
    .bind(inventory_balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!((qty_reserved, qty_held), (0, 1));

    let held_move = repo::license_plates::move_license_plate(
        &fixture.db,
        tenant_id,
        user.id,
        license_plate_id,
        source_location_id,
        Some("held inventory must remain fixed"),
        "license-plate-held-move",
    )
    .await
    .unwrap_err();
    assert!(matches!(
        held_move,
        AppError::Application(ApplicationError::Conflict(ref message))
            if message.contains("reserved or held inventory")
    ));

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let dimension_error = sqlx::query(
        "UPDATE inventory_balances SET location_id = $1 WHERE tenant_id = $2 AND id = $3",
    )
    .bind(source_location_id)
    .bind(tenant_id.get())
    .bind(inventory_balance_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(dimension_error
        .to_string()
        .contains("committed inventory balance dimensions are immutable"));
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let released_at = db::now_iso();
    sqlx::query(
        r#"
        UPDATE inventory_holds
        SET modified = $1, deleted = $1, released_at = $1,
            released_by = $2, status = 'released'
        WHERE tenant_id = $3 AND id = $4
        "#,
    )
    .bind(released_at)
    .bind(user.id)
    .bind(tenant_id.get())
    .bind(hold_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    repo::license_plates::move_license_plate(
        &fixture.db,
        tenant_id,
        user.id,
        license_plate_id,
        source_location_id,
        Some("full hold release permits movement"),
        "license-plate-released-hold-move",
    )
    .await
    .unwrap();

    let race_barrier = Arc::new(Barrier::new(3));
    let hold_db = fixture.db.clone();
    let hold_access = access.clone();
    let hold_barrier = Arc::clone(&race_barrier);
    let hold_attempt = tokio::spawn(async move {
        hold_barrier.wait().await;
        repo::inventory::place_inventory_hold(
            &hold_db,
            &hold_access,
            &CommandContext {
                tenant_id: hold_access.tenant_id,
                actor_id: hold_access.user_id,
                request_id: "request-license-plate-hold-move-race".into(),
                idempotency_key: Some("license-plate-hold-move-race".into()),
            },
            &repo::inventory::PlaceInventoryHoldCommand {
                inventory_balance_id,
                qty: 1,
                reason: InventoryHoldReason::Regulatory,
                note: Some("concurrent movement restriction"),
                reference_type: None,
                reference_id: None,
            },
        )
        .await
    });
    let move_db = fixture.db.clone();
    let move_barrier = Arc::clone(&race_barrier);
    let move_attempt = tokio::spawn(async move {
        move_barrier.wait().await;
        repo::license_plates::move_license_plate(
            &move_db,
            tenant_id,
            user.id,
            license_plate_id,
            destination_location_id,
            Some("concurrent hold placement"),
            "license-plate-hold-move-race",
        )
        .await
    });
    race_barrier.wait().await;
    let (hold_result, move_result) = timeout(Duration::from_secs(3), async {
        (hold_attempt.await.unwrap(), move_attempt.await.unwrap())
    })
    .await
    .expect("hold placement and license plate move serialize without deadlock");
    let hold_id = hold_result.unwrap().hold_id;
    let expected_location_id = match move_result {
        Ok(_) => destination_location_id,
        Err(AppError::Application(ApplicationError::Conflict(message))) => {
            assert!(message.contains("reserved or held inventory"));
            source_location_id
        }
        Err(error) => panic!("unexpected concurrent license plate move error: {error:?}"),
    };

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let (hold_location_id, balance_location_id, qty_reserved, qty_held, hold_status): (
        i64,
        i64,
        i64,
        i64,
        String,
    ) = sqlx::query_as(
        r#"
        SELECT hold.location_id, balance.location_id,
               balance.qty_reserved, balance.qty_held, hold.status
        FROM inventory_holds hold
        INNER JOIN inventory_balances balance
            ON balance.tenant_id = hold.tenant_id
           AND balance.inventory_owner_id = hold.inventory_owner_id
           AND balance.id = hold.inventory_balance_id
        WHERE hold.tenant_id = $1 AND hold.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(hold_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(hold_location_id, expected_location_id);
    assert_eq!(balance_location_id, expected_location_id);
    assert_eq!((qty_reserved, qty_held), (0, 1));
    assert_eq!(hold_status, "active");
}
