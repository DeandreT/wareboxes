mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use tokio::sync::{oneshot, Barrier};
use tokio::time::{sleep, timeout};

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

fn assert_active_allocation_guard(error: &sqlx::Error) {
    assert!(
        error
            .to_string()
            .contains("allocated inventory balance dimensions are immutable"),
        "unexpected balance dimension update error: {error}"
    );
}

#[tokio::test]
async fn license_plate_moves_and_reservations_serialize_balance_dimensions() {
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
    let source_location_id = fixture
        .location(tenant_id, facility_id, "LP-RACE-SOURCE")
        .await;
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
    let item_batch_id = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        inventory_owner_id,
        item_id,
        None,
        Some("LP-RACE-LOT"),
        None,
        None,
    )
    .await
    .unwrap();
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

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        r#"
        UPDATE license_plates
        SET location_id = $1
        WHERE tenant_id = $2
          AND inventory_owner_id = $3
          AND id = $4
        "#,
    )
    .bind(source_location_id)
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(license_plate_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    let receive_transaction_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_transactions (
            tenant_id, inventory_owner_id, created, actor_user_id,
            transaction_type, operation, idempotency_key, request_hash
        )
        VALUES ($1, $2, $3, $4, 'receive', $5, $5, $5)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(db::now_iso())
    .bind(user.id)
    .bind("license-plate-reservation-race-receipt")
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO inventory_entries (
            tenant_id, inventory_owner_id, transaction_id, created, facility_id,
            location_id, license_plate_id, item_batch_id, item_id, uom, lot,
            expiration, serial, status, quantity_delta
        )
        SELECT $1, batch.inventory_owner_id, $2, $3, $4, $5, $6, batch.id,
               batch.item_id, batch.uom, batch.lot, batch.expiration, batch.serial,
               'available', 5
        FROM item_batches batch
        WHERE batch.tenant_id = $1 AND batch.id = $7
        "#,
    )
    .bind(tenant_id.get())
    .bind(receive_transaction_id)
    .bind(db::now_iso())
    .bind(facility_id)
    .bind(source_location_id)
    .bind(license_plate_id)
    .bind(item_batch_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    let inventory_balance_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_balances (
            tenant_id, inventory_owner_id, created, facility_id, location_id,
            license_plate_id, item_batch_id, item_id, uom, status,
            qty_on_hand, qty_reserved
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, 'each', 'available', 5, 0
        )
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(db::now_iso())
    .bind(facility_id)
    .bind(source_location_id)
    .bind(license_plate_id)
    .bind(item_batch_id)
    .bind(item_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let order_id = fixture
        .order(tenant_id, "LP-RESERVATION-RACE-ORDER", inventory_owner_id)
        .await;
    let order_item_id = fixture.order_item(tenant_id, order_id, item_id, 1).await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
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
                reservation_id: reservation.reservation_id,
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
    assert_active_allocation_guard(&error);
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
    assert_active_allocation_guard(&error);
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
    assert_active_allocation_guard(&error);
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
}
