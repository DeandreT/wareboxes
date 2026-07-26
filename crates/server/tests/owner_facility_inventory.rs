mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use tokio::sync::{oneshot, Barrier};
use tokio::time::timeout;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InventoryEffects {
    transactions: i64,
    entries: i64,
    balances: i64,
    outbox_events: i64,
}

async fn inventory_effects(db: &db::Db, tenant_id: TenantId) -> InventoryEffects {
    let mut tx = tenant_tx(db, tenant_id).await;
    let (transactions, entries, balances, outbox_events) = sqlx::query_as(
        r#"
        SELECT
            (SELECT COUNT(*) FROM inventory_transactions WHERE tenant_id = $1),
            (SELECT COUNT(*) FROM inventory_entries WHERE tenant_id = $1),
            (SELECT COUNT(*) FROM inventory_balances WHERE tenant_id = $1),
            (SELECT COUNT(*) FROM outbox_events WHERE tenant_id = $1)
        "#,
    )
    .bind(tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    InventoryEffects {
        transactions,
        entries,
        balances,
        outbox_events,
    }
}

async fn assigned_pair(fixture: &Fixture, tenant_id: TenantId, key: &str) -> (i64, i64, i64) {
    let owner = fixture
        .inventory_owner(tenant_id, &format!("{key} owner"))
        .await;
    let facility = fixture
        .facility(tenant_id, &format!("{key} facility"))
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, owner, facility)
        .await;
    let location = fixture
        .location(tenant_id, facility, &format!("{key}-LOCATION"))
        .await;
    (owner, facility, location)
}

async fn assert_assignment_retirement_rejected(
    db: &db::Db,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
    expected_reason: &str,
) {
    let mut tx = tenant_tx(db, tenant_id).await;
    let error = sqlx::query(
        r#"
        UPDATE inventory_owner_facilities
        SET deleted = CURRENT_TIMESTAMP
        WHERE tenant_id = $1
          AND inventory_owner_id = $2
          AND facility_id = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(facility_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(
        error.to_string().contains(expected_reason),
        "unexpected assignment retirement error: {error}"
    );
    tx.rollback().await.unwrap();
}

fn assert_inactive_pair_conflict(error: AppError) {
    match error {
        AppError::Core(CoreError::Conflict(message)) => {
            assert!(
                message.contains("not active") && message.contains("facility"),
                "unexpected conflict: {message}"
            );
        }
        other => panic!("expected inactive owner/facility conflict, got {other:?}"),
    }
}

#[tokio::test]
async fn owner_facility_pair_is_enforced_across_inventory_boundaries() {
    let fixture = Fixture::new().await;
    let user = fixture
        .wms_user("owner-facility-inventory@test.local")
        .await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let inventory_owner_id = fixture
        .inventory_owner(tenant_id, "Owner Facility Inventory")
        .await;
    let assigned_facility_id = fixture.facility(tenant_id, "Assigned Owner Facility").await;
    let unassigned_facility_id = fixture
        .facility(tenant_id, "Unassigned Owner Facility")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, assigned_facility_id)
        .await;
    let assigned_location_id = fixture
        .location(tenant_id, assigned_facility_id, "OWNER-FACILITY-ASSIGNED")
        .await;
    let unassigned_location_id = fixture
        .location(
            tenant_id,
            unassigned_facility_id,
            "OWNER-FACILITY-UNASSIGNED",
        )
        .await;
    let item_id = fixture.item(tenant_id, "Owner Facility Item", "each").await;
    let item_batch_id = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        inventory_owner_id,
        item_id,
        None,
        Some("OWNER-FACILITY-LOT"),
        None,
        None,
    )
    .await
    .unwrap();

    repo::inventory::receive_inventory(
        &fixture.db,
        tenant_id,
        user.id,
        item_batch_id,
        assigned_location_id,
        10,
        None,
        Some("valid assigned receipt"),
        None,
        None,
        "owner-facility-valid-receipt",
    )
    .await
    .unwrap();
    assert!(
        repo::inventory::get_reconciliation_issues(&fixture.db, tenant_id)
            .await
            .unwrap()
            .is_empty()
    );

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let error = sqlx::query(
        r#"
        INSERT INTO inventory_balances (
            tenant_id, inventory_owner_id, created, facility_id, location_id,
            item_batch_id, item_id, uom, status, qty_on_hand, qty_reserved
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'each', 'available', 1, 0)
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(db::now_iso())
    .bind(unassigned_facility_id)
    .bind(unassigned_location_id)
    .bind(item_batch_id)
    .bind(item_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(error.to_string().contains("not active"));
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let transaction_id: i64 = sqlx::query_scalar(
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
    .bind("owner-facility-raw-entry")
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let error = sqlx::query(
        r#"
        INSERT INTO inventory_entries (
            tenant_id, inventory_owner_id, transaction_id, created, facility_id,
            location_id, item_batch_id, item_id, uom, lot, expiration, serial,
            status, quantity_delta
        )
        SELECT $1, batch.inventory_owner_id, $2, $3, $4, $5, batch.id,
               batch.item_id, batch.uom, batch.lot, batch.expiration, batch.serial,
               'available', 1
        FROM item_batches batch
        WHERE batch.tenant_id = $1 AND batch.id = $6
        "#,
    )
    .bind(tenant_id.get())
    .bind(transaction_id)
    .bind(db::now_iso())
    .bind(unassigned_facility_id)
    .bind(unassigned_location_id)
    .bind(item_batch_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(error.to_string().contains("not active"));
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let error = sqlx::query(
        r#"
        INSERT INTO license_plates (
            tenant_id, inventory_owner_id, created, barcode, facility_id, location_id
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(db::now_iso())
    .bind("OWNER-FACILITY-INVALID-LP")
    .bind(unassigned_facility_id)
    .bind(unassigned_location_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(error.to_string().contains("not active"));
    tx.rollback().await.unwrap();

    let order_id = fixture
        .order(
            tenant_id,
            "OWNER-FACILITY-INVALID-RESERVATION",
            inventory_owner_id,
        )
        .await;
    let order_item_id = fixture.order_item(tenant_id, order_id, item_id, 1).await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let error = sqlx::query(
        r#"
        INSERT INTO inventory_reservations (
            tenant_id, inventory_owner_id, created, modified, created_by,
            order_id, order_item_id, facility_id, item_id, uom, qty, status
        )
        VALUES ($1, $2, $3, $3, $4, $5, $6, $7, $8, 'each', 1, 'active')
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(db::now_iso())
    .bind(user.id)
    .bind(order_id)
    .bind(order_item_id)
    .bind(unassigned_facility_id)
    .bind(item_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(
        error.to_string().contains("not active")
            || error.to_string().contains("must match inventory balance")
    );
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let error = sqlx::query(
        r#"
        INSERT INTO outbox_events (
            tenant_id, inventory_owner_id, facility_id, actor_user_id, created,
            event_key, aggregate_type, aggregate_id, ordering_key,
            aggregate_sequence, event_type, schema_version, payload,
            occurred_at, available_at
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, 'inventory', 'invalid-pair',
            $6, 1, 'inventory.invalid-pair', 1, '{}'::jsonb, $5, $5
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(unassigned_facility_id)
    .bind(user.id)
    .bind(db::now_iso())
    .bind("owner-facility-invalid-outbox")
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("outbox_events_owner_facility_fkey"));
    tx.rollback().await.unwrap();

    let before_repository_rejection = inventory_effects(&fixture.db, tenant_id).await;
    let error = repo::inventory::receive_inventory(
        &fixture.db,
        tenant_id,
        user.id,
        item_batch_id,
        unassigned_location_id,
        2,
        None,
        None,
        None,
        None,
        "owner-facility-invalid-repository-receipt",
    )
    .await
    .unwrap_err();
    assert_inactive_pair_conflict(error);
    assert_eq!(
        inventory_effects(&fixture.db, tenant_id).await,
        before_repository_rejection
    );

    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, unassigned_facility_id)
        .await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let move_transaction_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_transactions (
            tenant_id, inventory_owner_id, created, actor_user_id,
            transaction_type, operation, idempotency_key, request_hash
        )
        VALUES ($1, $2, $3, $4, 'move', $5, $5, $5)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(db::now_iso())
    .bind(user.id)
    .bind("owner-facility-cross-facility-move")
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    for (facility_id, location_id, quantity_delta) in [
        (assigned_facility_id, assigned_location_id, -1_i64),
        (unassigned_facility_id, unassigned_location_id, 1_i64),
    ] {
        sqlx::query(
            r#"
            INSERT INTO inventory_entries (
                tenant_id, inventory_owner_id, transaction_id, created,
                facility_id, location_id, item_batch_id, item_id, uom, lot,
                expiration, serial, status, quantity_delta
            )
            SELECT $1, batch.inventory_owner_id, $2, $3, $4, $5, batch.id,
                   batch.item_id, batch.uom, batch.lot, batch.expiration,
                   batch.serial, 'available', $6
            FROM item_batches batch
            WHERE batch.tenant_id = $1 AND batch.id = $7
            "#,
        )
        .bind(tenant_id.get())
        .bind(move_transaction_id)
        .bind(db::now_iso())
        .bind(facility_id)
        .bind(location_id)
        .bind(quantity_delta)
        .bind(item_batch_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    let error = tx.commit().await.unwrap_err();
    assert!(error.to_string().contains("cannot span facilities"));

    assert_assignment_retirement_rejected(
        &fixture.db,
        tenant_id,
        inventory_owner_id,
        assigned_facility_id,
        "committed inventory",
    )
    .await;

    let (hold_owner, hold_facility, hold_location) =
        assigned_pair(&fixture, tenant_id, "HOLD-GUARD").await;
    let hold_batch = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        hold_owner,
        item_id,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    repo::inventory::receive_inventory(
        &fixture.db,
        tenant_id,
        user.id,
        hold_batch,
        hold_location,
        2,
        None,
        None,
        None,
        None,
        "owner-facility-hold-guard-receipt",
    )
    .await
    .unwrap();
    let hold_balance = repo::inventory::get_balances(&fixture.db, tenant_id, false)
        .await
        .unwrap()
        .into_iter()
        .find(|balance| balance.item_batch_id == hold_batch)
        .unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let hold_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_holds (
            tenant_id, inventory_owner_id, created, modified, created_by,
            inventory_balance_id, facility_id, location_id, license_plate_id,
            item_batch_id, item_id, uom, inventory_status, qty, reason_code,
            note, status
        )
        VALUES (
            $1, $2, $3, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
            1, 'regulatory', 'owner-facility retirement guard', 'active'
        )
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(hold_owner)
    .bind(db::now_iso())
    .bind(user.id)
    .bind(hold_balance.id)
    .bind(hold_facility)
    .bind(hold_location)
    .bind(hold_balance.license_plate_id)
    .bind(hold_batch)
    .bind(item_id)
    .bind(&hold_balance.uom)
    .bind(hold_balance.status.as_str())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_assignment_retirement_rejected(
        &fixture.db,
        tenant_id,
        hold_owner,
        hold_facility,
        "active holds",
    )
    .await;

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
    let admin_db = admin_db_for(&fixture.db).await;
    sqlx::query("UPDATE inventory_balances SET qty_on_hand = 0 WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id.get())
        .bind(hold_balance.id)
        .execute(&admin_db)
        .await
        .unwrap();
    admin_db.close().await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    assert_eq!(
        sqlx::query(
            r#"
            UPDATE inventory_owner_facilities
            SET deleted = CURRENT_TIMESTAMP
            WHERE tenant_id = $1
              AND inventory_owner_id = $2
              AND facility_id = $3
            "#,
        )
        .bind(tenant_id.get())
        .bind(hold_owner)
        .bind(hold_facility)
        .execute(&mut *tx)
        .await
        .unwrap()
        .rows_affected(),
        1
    );
    tx.rollback().await.unwrap();

    let (reservation_owner, reservation_facility, reservation_location) =
        assigned_pair(&fixture, tenant_id, "RESERVATION-GUARD").await;
    let reservation_batch = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        reservation_owner,
        item_id,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    repo::inventory::receive_inventory(
        &fixture.db,
        tenant_id,
        user.id,
        reservation_batch,
        reservation_location,
        2,
        None,
        None,
        None,
        None,
        "owner-facility-reservation-guard-receipt",
    )
    .await
    .unwrap();
    let reservation_balance_id = repo::inventory::get_balances(&fixture.db, tenant_id, false)
        .await
        .unwrap()
        .into_iter()
        .find(|balance| balance.item_batch_id == reservation_batch)
        .unwrap()
        .id;
    let reservation_order = fixture
        .order(
            tenant_id,
            "OWNER-FACILITY-RESERVATION-GUARD",
            reservation_owner,
        )
        .await;
    let reservation_allocation = fixture
        .allocated_reservation(
            tenant_id,
            user.id,
            reservation_order,
            reservation_balance_id,
            1,
            "owner-facility-reservation-guard",
        )
        .await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    repo::inventory::cancel_inventory_allocation(
        &fixture.db,
        &access,
        &repo::inventory::CancelInventoryAllocationCommand {
            allocation_id: reservation_allocation.allocation_id,
            idempotency_key: "owner-facility-reservation-guard-release",
        },
    )
    .await
    .unwrap();
    let admin_db = admin_db_for(&fixture.db).await;
    sqlx::query(
        "UPDATE inventory_balances SET qty_on_hand = 0, qty_reserved = 0 WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id.get())
    .bind(reservation_balance_id)
    .execute(&admin_db)
    .await
    .unwrap();
    admin_db.close().await;
    assert_assignment_retirement_rejected(
        &fixture.db,
        tenant_id,
        reservation_owner,
        reservation_facility,
        "active reservations",
    )
    .await;

    let (license_plate_owner, license_plate_facility, license_plate_location) =
        assigned_pair(&fixture, tenant_id, "LICENSE-PLATE-GUARD").await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        r#"
        INSERT INTO license_plates (
            tenant_id, inventory_owner_id, created, barcode, facility_id, location_id
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(tenant_id.get())
    .bind(license_plate_owner)
    .bind(db::now_iso())
    .bind("OWNER-FACILITY-GUARD-LP")
    .bind(license_plate_facility)
    .bind(license_plate_location)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_assignment_retirement_rejected(
        &fixture.db,
        tenant_id,
        license_plate_owner,
        license_plate_facility,
        "active license plates",
    )
    .await;

    let (work_owner, work_facility, _) = assigned_pair(&fixture, tenant_id, "WORK-GUARD").await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        r#"
        INSERT INTO work_tasks (
            tenant_id, facility_id, inventory_owner_id, created, task_type,
            status, required_permission, priority, title, created_by
        )
        VALUES ($1, $2, $3, $4, 'unpack_cancelled_order', 'open', 'wms', 0, $5, $6)
        "#,
    )
    .bind(tenant_id.get())
    .bind(work_facility)
    .bind(work_owner)
    .bind(db::now_iso())
    .bind("Owner facility retirement guard")
    .bind(user.id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_assignment_retirement_rejected(
        &fixture.db,
        tenant_id,
        work_owner,
        work_facility,
        "executable work",
    )
    .await;

    let (concurrent_owner, concurrent_facility, concurrent_location) =
        assigned_pair(&fixture, tenant_id, "CONCURRENT-RETIREMENT").await;
    let concurrent_batch = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        concurrent_owner,
        item_id,
        None,
        Some("CONCURRENT-RETIREMENT-LOT"),
        None,
        None,
    )
    .await
    .unwrap();
    let mut inventory_tx = tenant_tx(&fixture.db, tenant_id).await;
    let concurrent_transaction_id: i64 = sqlx::query_scalar(
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
    .bind(concurrent_owner)
    .bind(db::now_iso())
    .bind(user.id)
    .bind("owner-facility-concurrent-receipt")
    .fetch_one(&mut *inventory_tx)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO inventory_entries (
            tenant_id, inventory_owner_id, transaction_id, created, facility_id,
            location_id, item_batch_id, item_id, uom, lot, expiration, serial,
            status, quantity_delta
        )
        SELECT $1, batch.inventory_owner_id, $2, $3, $4, $5, batch.id,
               batch.item_id, batch.uom, batch.lot, batch.expiration, batch.serial,
               'available', 3
        FROM item_batches batch
        WHERE batch.tenant_id = $1 AND batch.id = $6
        "#,
    )
    .bind(tenant_id.get())
    .bind(concurrent_transaction_id)
    .bind(db::now_iso())
    .bind(concurrent_facility)
    .bind(concurrent_location)
    .bind(concurrent_batch)
    .execute(&mut *inventory_tx)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO inventory_balances (
            tenant_id, inventory_owner_id, created, facility_id, location_id,
            item_batch_id, item_id, uom, status, qty_on_hand, qty_reserved
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'each', 'available', 3, 0)
        "#,
    )
    .bind(tenant_id.get())
    .bind(concurrent_owner)
    .bind(db::now_iso())
    .bind(concurrent_facility)
    .bind(concurrent_location)
    .bind(concurrent_batch)
    .bind(item_id)
    .execute(&mut *inventory_tx)
    .await
    .unwrap();

    let barrier = Arc::new(Barrier::new(2));
    let retirement_barrier = Arc::clone(&barrier);
    let retirement_db = fixture.db.clone();
    let (retirement_started, retirement_started_rx) = oneshot::channel();
    let mut retirement = tokio::spawn(async move {
        let mut tx = tenant_tx(&retirement_db, tenant_id).await;
        retirement_barrier.wait().await;
        retirement_started.send(()).unwrap();
        let result = sqlx::query(
            r#"
            UPDATE inventory_owner_facilities
            SET deleted = CURRENT_TIMESTAMP
            WHERE tenant_id = $1
              AND inventory_owner_id = $2
              AND facility_id = $3
            "#,
        )
        .bind(tenant_id.get())
        .bind(concurrent_owner)
        .bind(concurrent_facility)
        .execute(&mut *tx)
        .await;
        match result {
            Ok(_) => {
                tx.commit().await?;
                Ok(())
            }
            Err(error) => {
                tx.rollback().await?;
                Err(error)
            }
        }
    });
    barrier.wait().await;
    retirement_started_rx.await.unwrap();
    assert!(
        timeout(Duration::from_millis(250), &mut retirement)
            .await
            .is_err(),
        "assignment retirement completed while inventory was uncommitted"
    );

    inventory_tx.commit().await.unwrap();
    let retirement_error = timeout(Duration::from_secs(3), retirement)
        .await
        .expect("assignment retirement completes after inventory commits")
        .unwrap()
        .unwrap_err();
    assert!(
        retirement_error.to_string().contains("committed inventory"),
        "unexpected concurrent retirement error: {retirement_error}"
    );

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let (assignment_is_active, quantity): (bool, i64) = sqlx::query_as(
        r#"
        SELECT assignment.deleted IS NULL, balance.qty_on_hand
        FROM inventory_owner_facilities assignment
        INNER JOIN inventory_balances balance
            ON balance.tenant_id = assignment.tenant_id
           AND balance.inventory_owner_id = assignment.inventory_owner_id
           AND balance.facility_id = assignment.facility_id
        WHERE assignment.tenant_id = $1
          AND assignment.inventory_owner_id = $2
          AND assignment.facility_id = $3
          AND balance.item_batch_id = $4
        "#,
    )
    .bind(tenant_id.get())
    .bind(concurrent_owner)
    .bind(concurrent_facility)
    .bind(concurrent_batch)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert!(assignment_is_active);
    assert_eq!(quantity, 3);
}
