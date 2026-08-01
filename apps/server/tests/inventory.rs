mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use common::*;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::{routes, state::AppState};

#[tokio::test]
async fn inventory_commands_write_replay_safe_journal_and_balance_projection() {
    let db = setup().await;
    let fixture = Fixture { db: db.clone() };

    let user = auth::register_user(&db, "wms@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let tenant_id = tenant_for_user(&db, user.id).await;
    let facility =
        wareboxes_persistence_postgres::facilities::add_facility(&db, tenant_id, "Main DC")
            .await
            .unwrap();
    let receiving = wareboxes_persistence_postgres::locations::add_location(
        &db,
        tenant_id,
        facility,
        None,
        Some("RCV-01"),
        Some("Receiving"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let pick_face = wareboxes_persistence_postgres::locations::add_location(
        &db,
        tenant_id,
        facility,
        None,
        Some("A-01-01"),
        Some("Aisle 1 Bin 1"),
        "bin",
        true,
        true,
        false,
    )
    .await
    .unwrap();
    let item = repo::items::add_item(
        &db, tenant_id, "Widget", None, "each", None, None, None, None, None, None,
    )
    .await
    .unwrap();
    let inventory_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "Inventory Customer",
        "ic@test.com",
    )
    .await
    .unwrap();
    repo::inventory_owners::replace_inventory_owner_facilities(
        &db,
        tenant_id,
        inventory_owner,
        &[facility],
    )
    .await
    .unwrap();
    let batch = repo::inventory::add_item_batch(
        &db,
        tenant_id,
        inventory_owner,
        item,
        None,
        Some("LOT-1"),
        None,
        None,
    )
    .await
    .unwrap();

    let missing_key = repo::inventory::receive_inventory(
        &db, tenant_id, user.id, batch, receiving, 1, None, None, None, None, "",
    )
    .await
    .unwrap_err();
    assert!(matches!(
        missing_key,
        AppError::Application(ApplicationError::IdempotencyKeyRequired)
    ));
    assert!(repo::inventory::get_transactions(&db, tenant_id)
        .await
        .unwrap()
        .is_empty());

    let receive_move = repo::inventory::receive_inventory(
        &db,
        tenant_id,
        user.id,
        batch,
        receiving,
        100,
        None,
        Some("initial receipt"),
        Some("load"),
        Some(42),
        "receipt-42",
    )
    .await
    .unwrap();
    assert!(receive_move > 0);

    let replayed_receive = repo::inventory::receive_inventory(
        &db,
        tenant_id,
        user.id,
        batch,
        receiving,
        100,
        None,
        Some("initial receipt"),
        Some("load"),
        Some(42),
        "receipt-42",
    )
    .await
    .unwrap();
    assert_eq!(replayed_receive, receive_move);

    let changed_retry = repo::inventory::receive_inventory(
        &db,
        tenant_id,
        user.id,
        batch,
        receiving,
        101,
        None,
        Some("initial receipt"),
        Some("load"),
        Some(42),
        "receipt-42",
    )
    .await
    .unwrap_err();
    assert!(matches!(
        changed_retry,
        AppError::Application(ApplicationError::IdempotencyKeyReused)
    ));

    let peer = auth::register_user(&db, "wms-peer@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let mut membership_tx = tenant_tx(&db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
        .bind(tenant_id.get())
        .bind(peer.id)
        .execute(&mut *membership_tx)
        .await
        .unwrap();
    membership_tx.commit().await.unwrap();
    let cross_actor_retry = repo::inventory::receive_inventory(
        &db,
        tenant_id,
        peer.id,
        batch,
        receiving,
        100,
        None,
        Some("initial receipt"),
        Some("load"),
        Some(42),
        "receipt-42",
    )
    .await
    .unwrap_err();
    assert!(matches!(
        cross_actor_retry,
        AppError::Application(ApplicationError::IdempotencyKeyReused)
    ));

    repo::inventory::move_inventory(
        &db,
        tenant_id,
        user.id,
        batch,
        receiving,
        pick_face,
        30,
        None,
        Some("replenishment"),
        None,
        None,
        "move-inventory-initial",
    )
    .await
    .unwrap();

    let balances = repo::inventory::get_balances(&db, tenant_id, false)
        .await
        .unwrap();
    let receiving_balance = balances
        .iter()
        .find(|b| b.location_id == receiving && b.item_batch_id == batch)
        .unwrap();
    let pick_balance = balances
        .iter()
        .find(|b| b.location_id == pick_face && b.item_batch_id == batch)
        .unwrap();
    assert_eq!(receiving_balance.qty_on_hand, 70);
    assert_eq!(receiving_balance.qty_reserved, 0);
    assert_eq!(pick_balance.qty_on_hand, 30);
    assert_eq!(pick_balance.qty_reserved, 0);

    let err = repo::inventory::move_inventory(
        &db,
        tenant_id,
        user.id,
        batch,
        pick_face,
        receiving,
        31,
        None,
        None,
        None,
        None,
        "move-inventory-insufficient-unreserved",
    )
    .await
    .unwrap_err();
    assert!(matches!(
        err,
        AppError::Application(ApplicationError::Conflict(_))
    ));

    repo::orders::add_order(&db, tenant_id, &new_order("INV-1", inventory_owner))
        .await
        .unwrap();
    let order_id = repo::orders::get_orders(&db, tenant_id).await.unwrap()[0].id;
    let order_item_id = fixture.order_item(tenant_id, order_id, item, 20).await;
    let access = default_tenant_for_user(&db, user.id).await.unwrap();
    let reservation_command = repo::inventory::CreateInventoryReservationCommand {
        order_id,
        order_item_id,
        facility_id: facility,
        qty: 20,
        idempotency_key: "reserve-inv-1",
    };
    let reservation =
        repo::inventory::create_inventory_reservation(&db, &access, &reservation_command)
            .await
            .unwrap();
    let replayed_reservation =
        repo::inventory::create_inventory_reservation(&db, &access, &reservation_command)
            .await
            .unwrap();
    assert_eq!(replayed_reservation, reservation);
    let changed_reservation_retry = repo::inventory::create_inventory_reservation(
        &db,
        &access,
        &repo::inventory::CreateInventoryReservationCommand {
            qty: 19,
            ..reservation_command
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        changed_reservation_retry,
        AppError::Application(ApplicationError::IdempotencyKeyReused)
    ));
    let _allocation = repo::inventory::allocate_inventory(
        &db,
        &access,
        &repo::inventory::AllocateInventoryCommand {
            reservation_id: reservation.reservation_id,
            inventory_balance_id: pick_balance.id,
            qty: 20,
            idempotency_key: "allocate-inv-1",
        },
    )
    .await
    .unwrap();
    let balances = repo::inventory::get_balances(&db, tenant_id, false)
        .await
        .unwrap();
    let pick_balance = balances
        .iter()
        .find(|b| b.location_id == pick_face && b.item_batch_id == batch)
        .unwrap();
    assert_eq!(pick_balance.qty_on_hand, 30);
    assert_eq!(pick_balance.qty_reserved, 20);
    let reservations = repo::inventory::get_reservations(&db, tenant_id, false)
        .await
        .unwrap();
    assert_eq!(reservations.len(), 1);
    assert_eq!(reservations[0].allocated_qty, 20);
    assert_eq!(
        repo::inventory::get_allocations_in_scope(&db, &access, false)
            .await
            .unwrap()[0]
            .inventory_balance_id,
        pick_balance.id
    );

    let err = repo::inventory::move_inventory(
        &db,
        tenant_id,
        user.id,
        batch,
        pick_face,
        receiving,
        11,
        None,
        None,
        None,
        None,
        "move-inventory-insufficient-reserved",
    )
    .await
    .unwrap_err();
    assert!(matches!(
        err,
        AppError::Application(ApplicationError::Conflict(_))
    ));

    let cancel_command = repo::inventory::CancelInventoryReservationCommand {
        reservation_id: reservation.reservation_id,
        idempotency_key: "cancel-inv-1",
    };
    let cancelled = repo::inventory::cancel_inventory_reservation(&db, &access, &cancel_command)
        .await
        .unwrap();
    assert_eq!(cancelled.released_qty, 20);
    assert_eq!(
        repo::inventory::cancel_inventory_reservation(&db, &access, &cancel_command)
            .await
            .unwrap(),
        cancelled
    );
    let second_reservation = repo::inventory::create_inventory_reservation(
        &db,
        &access,
        &repo::inventory::CreateInventoryReservationCommand {
            order_id,
            order_item_id,
            facility_id: facility,
            qty: 1,
            idempotency_key: "reserve-inv-2",
        },
    )
    .await
    .unwrap();
    let changed_cancel_retry = repo::inventory::cancel_inventory_reservation(
        &db,
        &access,
        &repo::inventory::CancelInventoryReservationCommand {
            reservation_id: second_reservation.reservation_id,
            idempotency_key: "cancel-inv-1",
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        changed_cancel_retry,
        AppError::Application(ApplicationError::IdempotencyKeyReused)
    ));
    let balances = repo::inventory::get_balances(&db, tenant_id, false)
        .await
        .unwrap();
    let pick_balance = balances
        .iter()
        .find(|b| b.location_id == pick_face && b.item_batch_id == batch)
        .unwrap();
    assert_eq!(pick_balance.qty_reserved, 0);

    let split_a = wareboxes_persistence_postgres::locations::add_location(
        &db,
        tenant_id,
        facility,
        None,
        Some("A-01-02"),
        Some("Aisle 1 Bin 2"),
        "bin",
        true,
        true,
        false,
    )
    .await
    .unwrap();
    let split_b = wareboxes_persistence_postgres::locations::add_location(
        &db,
        tenant_id,
        facility,
        None,
        Some("A-01-03"),
        Some("Aisle 1 Bin 3"),
        "bin",
        true,
        true,
        false,
    )
    .await
    .unwrap();
    let receiving_balance = balances
        .iter()
        .find(|b| b.location_id == receiving && b.item_batch_id == batch)
        .unwrap();
    let split_transaction = repo::inventory::split_move_inventory(
        &db,
        tenant_id,
        user.id,
        receiving_balance.id,
        &[(split_a, 4), (split_b, 6)],
        Some("split putaway"),
        None,
        None,
        "split-putaway-1",
    )
    .await
    .unwrap();
    assert!(split_transaction > 0);
    let balances = repo::inventory::get_balances(&db, tenant_id, false)
        .await
        .unwrap();
    let receiving_balance = balances
        .iter()
        .find(|b| b.location_id == receiving && b.item_batch_id == batch)
        .unwrap();
    assert_eq!(receiving_balance.qty_on_hand, 60);
    assert_eq!(
        balances
            .iter()
            .find(|b| b.location_id == split_a && b.item_batch_id == batch)
            .unwrap()
            .qty_on_hand,
        4
    );
    assert_eq!(
        balances
            .iter()
            .find(|b| b.location_id == split_b && b.item_batch_id == batch)
            .unwrap()
            .qty_on_hand,
        6
    );

    let transactions = repo::inventory::get_transactions(&db, tenant_id)
        .await
        .unwrap();
    assert!(transactions.iter().any(|transaction| {
        transaction.transaction_type == InventoryTransactionType::Receive
            && transaction.reason.as_deref() == Some("initial receipt")
            && transaction.idempotency_key.as_deref() == Some("receipt-42")
            && transaction
                .entries
                .iter()
                .any(|entry| entry.location_id == receiving && entry.quantity_delta == 100)
    }));
    assert!(transactions.iter().any(|transaction| {
        transaction.transaction_type == InventoryTransactionType::Move
            && transaction
                .entries
                .iter()
                .map(|entry| entry.quantity_delta)
                .sum::<i64>()
                == 0
    }));
    assert!(!transactions
        .iter()
        .any(|transaction| transaction.operation.contains("reserve")));

    assert!(repo::inventory::get_reconciliation_issues(&db, tenant_id)
        .await
        .unwrap()
        .is_empty());

    let transaction_id = transactions[0].id;
    let entry_id = transactions[0].entries[0].id;
    let mut tx = tenant_tx(&db, tenant_id).await;
    assert!(
        sqlx::query("UPDATE inventory_transactions SET reason = 'tampered' WHERE id = $1")
            .bind(transaction_id)
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let mut tx = tenant_tx(&db, tenant_id).await;
    assert!(sqlx::query("DELETE FROM inventory_entries WHERE id = $1")
        .bind(entry_id)
        .execute(&mut *tx)
        .await
        .is_err());
    tx.rollback().await.unwrap();
    let mut tx = tenant_tx(&db, tenant_id).await;
    assert!(sqlx::query(
        r#"
        INSERT INTO inventory_entries
            (tenant_id, inventory_owner_id, transaction_id, created, facility_id,
             location_id, license_plate_id, item_batch_id, item_id, uom, lot,
             expiration, serial, status, quantity_delta)
        SELECT tenant_id, inventory_owner_id, transaction_id, created, facility_id,
               location_id, license_plate_id, item_batch_id, item_id, uom, lot,
               expiration, serial, status, quantity_delta
        FROM inventory_entries
        WHERE id = $1
        "#,
    )
    .bind(entry_id)
    .execute(&mut *tx)
    .await
    .is_err());
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn inventory_repositories_reject_cross_tenant_and_cross_owner_access() {
    let fixture = Fixture::new().await;
    let tenant_a_user = fixture.user("inventory-tenant-a@test.com").await;
    let tenant_b_user = fixture.user("inventory-tenant-b@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, tenant_a_user.id).await;
    let tenant_b = tenant_for_user(&fixture.db, tenant_b_user.id).await;

    let facility = fixture.facility(tenant_a, "Tenant A DC").await;
    let location = fixture.location(tenant_a, facility, "TENANT-A-BIN").await;
    let owner_a = fixture.inventory_owner(tenant_a, "Owner A").await;
    let owner_b = fixture.inventory_owner(tenant_a, "Owner B").await;
    repo::inventory_owners::replace_inventory_owner_facilities(
        &fixture.db,
        tenant_a,
        owner_a,
        &[facility],
    )
    .await
    .unwrap();
    let item = fixture.item(tenant_a, "Tenant A Item", "each").await;
    let batch = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_a,
        owner_a,
        item,
        None,
        Some("TENANT-A-LOT"),
        None,
        None,
    )
    .await
    .unwrap();
    repo::inventory::receive_inventory(
        &fixture.db,
        tenant_a,
        tenant_a_user.id,
        batch,
        location,
        10,
        None,
        None,
        None,
        None,
        "tenant-a-receipt",
    )
    .await
    .unwrap();

    assert!(repo::inventory::get_balances(&fixture.db, tenant_b, false)
        .await
        .unwrap()
        .is_empty());
    assert!(repo::inventory::get_transactions(&fixture.db, tenant_b)
        .await
        .unwrap()
        .is_empty());
    assert!(repo::inventory::receive_inventory(
        &fixture.db,
        tenant_b,
        tenant_b_user.id,
        batch,
        location,
        1,
        None,
        None,
        None,
        None,
        "guessed-batch",
    )
    .await
    .is_err());

    let tenant_a_transactions = repo::inventory::get_transactions(&fixture.db, tenant_a)
        .await
        .unwrap();
    let transaction_id = tenant_a_transactions[0].id;
    let entry_id = tenant_a_transactions[0].entries[0].id;
    let admin_db = admin_db_for(&fixture.db).await;
    sqlx::query(
        "ALTER TABLE inventory_balances DISABLE TRIGGER inventory_balances_capture_projection_change",
    )
    .execute(&admin_db)
    .await
    .unwrap();
    sqlx::query("UPDATE inventory_balances SET qty_on_hand = qty_on_hand + 1 WHERE tenant_id = $1")
        .bind(tenant_a.get())
        .execute(&admin_db)
        .await
        .unwrap();
    sqlx::query(
        "ALTER TABLE inventory_balances ENABLE TRIGGER inventory_balances_capture_projection_change",
    )
    .execute(&admin_db)
    .await
    .unwrap();

    let unbound_visibility: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM inventory_transactions),
               (SELECT COUNT(*) FROM inventory_entries),
               (SELECT COUNT(*) FROM inventory_reconciliation)
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(unbound_visibility, (0, 0, 0));

    let mut tenant_a_tx = tenant_tx(&fixture.db, tenant_a).await;
    let tenant_a_visibility: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM inventory_transactions),
               (SELECT COUNT(*) FROM inventory_entries),
               (SELECT COUNT(*) FROM inventory_reconciliation)
        "#,
    )
    .fetch_one(&mut *tenant_a_tx)
    .await
    .unwrap();
    assert_eq!(tenant_a_visibility, (1, 1, 1));
    tenant_a_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    let tenant_b_visibility: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM inventory_transactions),
               (SELECT COUNT(*) FROM inventory_entries),
               (SELECT COUNT(*) FROM inventory_reconciliation)
        "#,
    )
    .fetch_one(&mut *tenant_b_tx)
    .await
    .unwrap();
    assert_eq!(tenant_b_visibility, (0, 0, 0));
    assert!(
        sqlx::query("UPDATE inventory_transactions SET reason = 'cross-tenant' WHERE id = $1")
            .bind(transaction_id)
            .execute(&mut *tenant_b_tx)
            .await
            .is_err()
    );
    tenant_b_tx.rollback().await.unwrap();
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(sqlx::query("DELETE FROM inventory_entries WHERE id = $1")
        .bind(entry_id)
        .execute(&mut *tenant_b_tx)
        .await
        .is_err());
    tenant_b_tx.rollback().await.unwrap();

    let mut unbound_tx = fixture.db.begin().await.unwrap();
    assert!(sqlx::query(
        r#"
        INSERT INTO inventory_transactions
            (tenant_id, inventory_owner_id, created, transaction_type,
             operation, request_hash)
        VALUES ($1, $2, $3, 'adjust', 'rls.unbound', 'request-hash')
        "#,
    )
    .bind(tenant_a.get())
    .bind(owner_a)
    .bind(db::now_iso())
    .execute(&mut *unbound_tx)
    .await
    .is_err());
    unbound_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(sqlx::query(
        r#"
        INSERT INTO inventory_entries
            (tenant_id, inventory_owner_id, transaction_id, created, facility_id,
             location_id, item_batch_id, item_id, uom, status, quantity_delta)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'each', 'available', 1)
        "#,
    )
    .bind(tenant_a.get())
    .bind(owner_a)
    .bind(transaction_id)
    .bind(db::now_iso())
    .bind(facility)
    .bind(location)
    .bind(batch)
    .bind(item)
    .execute(&mut *tenant_b_tx)
    .await
    .is_err());
    tenant_b_tx.rollback().await.unwrap();

    assert_eq!(
        repo::inventory::get_reconciliation_issues(&fixture.db, tenant_a)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        repo::inventory::get_reconciliation_issues(&fixture.db, tenant_b)
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query(
        "ALTER TABLE inventory_balances DISABLE TRIGGER inventory_balances_capture_projection_change",
    )
    .execute(&admin_db)
    .await
    .unwrap();
    sqlx::query("UPDATE inventory_balances SET qty_on_hand = qty_on_hand - 1 WHERE tenant_id = $1")
        .bind(tenant_a.get())
        .execute(&admin_db)
        .await
        .unwrap();
    sqlx::query(
        "ALTER TABLE inventory_balances ENABLE TRIGGER inventory_balances_capture_projection_change",
    )
    .execute(&admin_db)
    .await
    .unwrap();
    admin_db.close().await;

    let other_owner_order = fixture.order(tenant_a, "OTHER-OWNER-ORDER", owner_b).await;
    let balance = repo::inventory::get_balances(&fixture.db, tenant_a, false)
        .await
        .unwrap()
        .pop()
        .unwrap();
    fixture
        .assign_owner_to_facility(tenant_a, owner_b, facility)
        .await;
    let other_owner_order_item = fixture
        .order_item(tenant_a, other_owner_order, balance.item_id, 1)
        .await;
    let access = default_tenant_for_user(&fixture.db, tenant_a_user.id)
        .await
        .unwrap();
    let other_owner_reservation = repo::inventory::create_inventory_reservation(
        &fixture.db,
        &access,
        &repo::inventory::CreateInventoryReservationCommand {
            order_id: other_owner_order,
            order_item_id: other_owner_order_item,
            facility_id: facility,
            qty: 1,
            idempotency_key: "owner-mismatch-reservation",
        },
    )
    .await
    .unwrap();
    let owner_mismatch = repo::inventory::allocate_inventory(
        &fixture.db,
        &access,
        &repo::inventory::AllocateInventoryCommand {
            reservation_id: other_owner_reservation.reservation_id,
            inventory_balance_id: balance.id,
            qty: 1,
            idempotency_key: "owner-mismatch-allocation",
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        owner_mismatch,
        AppError::Application(ApplicationError::Conflict(_))
    ));
}

#[tokio::test]
async fn concurrent_inventory_retries_apply_effects_once() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("inventory-concurrency@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let inventory_owner = fixture
        .inventory_owner(tenant_id, "Concurrency Owner")
        .await;
    let facility = fixture.facility(tenant_id, "Concurrency DC").await;
    repo::inventory_owners::replace_inventory_owner_facilities(
        &fixture.db,
        tenant_id,
        inventory_owner,
        &[facility],
    )
    .await
    .unwrap();
    let receiving = fixture
        .location(tenant_id, facility, "CONCURRENT-RECEIVING")
        .await;
    let destination = fixture
        .location(tenant_id, facility, "CONCURRENT-DESTINATION")
        .await;
    let item = fixture.item(tenant_id, "Concurrent Item", "each").await;
    let batch = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        inventory_owner,
        item,
        None,
        Some("CONCURRENT-LOT"),
        None,
        None,
    )
    .await
    .unwrap();

    let actor_id = user.id;
    let mut retries = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let db = fixture.db.clone();
        retries.spawn(async move {
            repo::inventory::receive_inventory(
                &db,
                tenant_id,
                actor_id,
                batch,
                receiving,
                25,
                None,
                Some("concurrent receipt"),
                None,
                None,
                "concurrent-receipt-key",
            )
            .await
        });
    }

    let mut transaction_ids = std::collections::BTreeSet::new();
    while let Some(result) = retries.join_next().await {
        transaction_ids.insert(result.unwrap().unwrap());
    }
    assert_eq!(transaction_ids.len(), 1);

    let balances = repo::inventory::get_balances(&fixture.db, tenant_id, false)
        .await
        .unwrap();
    assert_eq!(balances.len(), 1);
    assert_eq!(balances[0].qty_on_hand, 25);

    let move_transaction = repo::inventory::move_inventory(
        &fixture.db,
        tenant_id,
        actor_id,
        batch,
        receiving,
        destination,
        25,
        None,
        None,
        None,
        None,
        "move-all-key",
    )
    .await
    .unwrap();
    let replayed_move = repo::inventory::move_inventory(
        &fixture.db,
        tenant_id,
        actor_id,
        batch,
        receiving,
        destination,
        25,
        None,
        None,
        None,
        None,
        "move-all-key",
    )
    .await
    .unwrap();
    assert_eq!(replayed_move, move_transaction);
    assert!(
        repo::inventory::get_reconciliation_issues(&fixture.db, tenant_id)
            .await
            .unwrap()
            .is_empty()
    );

    let destination_balance = repo::inventory::get_balances(&fixture.db, tenant_id, false)
        .await
        .unwrap()
        .into_iter()
        .find(|balance| balance.location_id == destination)
        .unwrap();
    let destination_balance_id = destination_balance.id;
    let order_id = fixture
        .order(tenant_id, "CONCURRENT-ORDER", inventory_owner)
        .await;
    let order_item_id = fixture
        .order_item(tenant_id, order_id, destination_balance.item_id, 10)
        .await;
    let access = default_tenant_for_user(&fixture.db, actor_id)
        .await
        .unwrap();
    let mut reservation_retries = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let db = fixture.db.clone();
        let access = access.clone();
        reservation_retries.spawn(async move {
            repo::inventory::create_inventory_reservation(
                &db,
                &access,
                &repo::inventory::CreateInventoryReservationCommand {
                    order_id,
                    order_item_id,
                    facility_id: destination_balance.facility_id,
                    qty: 10,
                    idempotency_key: "concurrent-reservation-key",
                },
            )
            .await
        });
    }

    let mut reservation_ids = std::collections::BTreeSet::new();
    while let Some(result) = reservation_retries.join_next().await {
        reservation_ids.insert(result.unwrap().unwrap().reservation_id);
    }
    assert_eq!(reservation_ids.len(), 1);
    let reservation_id = *reservation_ids.first().unwrap();
    let mut allocation_retries = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let db = fixture.db.clone();
        let access = access.clone();
        allocation_retries.spawn(async move {
            repo::inventory::allocate_inventory(
                &db,
                &access,
                &repo::inventory::AllocateInventoryCommand {
                    reservation_id,
                    inventory_balance_id: destination_balance_id,
                    qty: 10,
                    idempotency_key: "concurrent-allocation-key",
                },
            )
            .await
        });
    }
    let mut allocation_ids = std::collections::BTreeSet::new();
    while let Some(result) = allocation_retries.join_next().await {
        allocation_ids.insert(result.unwrap().unwrap().allocation_id);
    }
    assert_eq!(allocation_ids.len(), 1);
    let reservations = repo::inventory::get_reservations(&fixture.db, tenant_id, false)
        .await
        .unwrap();
    assert_eq!(reservations.len(), 1);
    let destination_balance = repo::inventory::get_balances(&fixture.db, tenant_id, false)
        .await
        .unwrap()
        .into_iter()
        .find(|balance| balance.location_id == destination)
        .unwrap();
    assert_eq!(destination_balance.qty_reserved, 10);

    let mut cancellation_retries = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let db = fixture.db.clone();
        let access = access.clone();
        cancellation_retries.spawn(async move {
            repo::inventory::cancel_inventory_reservation(
                &db,
                &access,
                &repo::inventory::CancelInventoryReservationCommand {
                    reservation_id,
                    idempotency_key: "concurrent-cancellation-key",
                },
            )
            .await
        });
    }
    while let Some(result) = cancellation_retries.join_next().await {
        assert_eq!(result.unwrap().unwrap().released_qty, 10);
    }
    let destination_balance = repo::inventory::get_balances(&fixture.db, tenant_id, false)
        .await
        .unwrap()
        .into_iter()
        .find(|balance| balance.location_id == destination)
        .unwrap();
    assert_eq!(destination_balance.qty_reserved, 0);

    let outsider = fixture.user("inventory-route-outsider@test.com").await;
    let outsider_tenant = tenant_for_user(&fixture.db, outsider.id).await;
    let token = auth::create_session(&fixture.db, actor_id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/inventory/balances")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, tenant_id.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let routed_balances: Vec<wareboxes_core::models::InventoryBalance> =
        serde_json::from_slice(&body).unwrap();
    assert_eq!(routed_balances.len(), 2);

    let cross_tenant = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/inventory/transactions")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, outsider_tenant.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross_tenant.status(), StatusCode::FORBIDDEN);

    let missing_tenant = app
        .oneshot(
            Request::builder()
                .uri("/api/inventory/reconciliation")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_tenant.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn concurrent_initial_receipt_and_batch_deletion_preserve_batch_stock_invariant() {
    let fixture = Fixture::new().await;
    let user = fixture.user("inventory-batch-delete-race@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let inventory_owner = fixture
        .inventory_owner(tenant_id, "Batch Delete Race Owner")
        .await;
    let facility = fixture.facility(tenant_id, "Batch Delete Race DC").await;
    repo::inventory_owners::replace_inventory_owner_facilities(
        &fixture.db,
        tenant_id,
        inventory_owner,
        &[facility],
    )
    .await
    .unwrap();
    let receiving = fixture
        .location(tenant_id, facility, "BATCH-DELETE-RACE-RECEIVING")
        .await;
    let item = fixture
        .item(tenant_id, "Batch Delete Race Item", "each")
        .await;

    for attempt in 0..6 {
        let batch = repo::inventory::add_item_batch(
            &fixture.db,
            tenant_id,
            inventory_owner,
            item,
            None,
            Some(&format!("BATCH-DELETE-RACE-{attempt}")),
            None,
            None,
        )
        .await
        .unwrap();
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));

        let receipt_db = fixture.db.clone();
        let receipt_barrier = barrier.clone();
        let receipt_key = format!("batch-delete-race-receipt-{attempt}");
        let actor_id = user.id;
        let receipt = tokio::spawn(async move {
            receipt_barrier.wait().await;
            repo::inventory::receive_inventory(
                &receipt_db,
                tenant_id,
                actor_id,
                batch,
                receiving,
                1,
                None,
                Some("initial receipt racing batch deletion"),
                None,
                None,
                &receipt_key,
            )
            .await
        });

        let deletion_db = fixture.db.clone();
        let deletion_barrier = barrier.clone();
        let deletion = tokio::spawn(async move {
            deletion_barrier.wait().await;
            repo::inventory::set_item_batch_deleted(&deletion_db, tenant_id, batch, true).await
        });

        barrier.wait().await;
        let receipt_result = receipt.await.unwrap();
        let deletion_result = deletion.await.unwrap();
        let batch_deleted = repo::inventory::get_item_batches(&fixture.db, tenant_id, true)
            .await
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.id == batch)
            .unwrap()
            .deleted
            .is_some();
        let stocked_qty = repo::inventory::get_balances(&fixture.db, tenant_id, false)
            .await
            .unwrap()
            .into_iter()
            .filter(|balance| balance.item_batch_id == batch)
            .map(|balance| balance.qty_on_hand)
            .sum::<i64>();

        match (receipt_result, deletion_result) {
            (Ok(_), Err(_)) => {
                assert!(!batch_deleted);
                assert_eq!(stocked_qty, 1);
            }
            (Err(_), Ok(true)) => {
                assert!(batch_deleted);
                assert_eq!(stocked_qty, 0);
            }
            outcome => panic!("receipt/delete race produced an invalid outcome: {outcome:?}"),
        }
    }

    assert!(
        repo::inventory::get_reconciliation_issues(&fixture.db, tenant_id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn inventory_rejects_mixed_lot_or_expiration_in_same_location() {
    let db = setup().await;

    let user = auth::register_user(&db, "lot-guard@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let tenant_id = tenant_for_user(&db, user.id).await;
    let facility =
        wareboxes_persistence_postgres::facilities::add_facility(&db, tenant_id, "Lot Guard DC")
            .await
            .unwrap();
    let receiving = wareboxes_persistence_postgres::locations::add_location(
        &db,
        tenant_id,
        facility,
        None,
        Some("LG-RCV"),
        Some("Lot Guard Receiving"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let reserve = wareboxes_persistence_postgres::locations::add_location(
        &db,
        tenant_id,
        facility,
        None,
        Some("LG-RSV"),
        Some("Lot Guard Reserve"),
        "rack",
        true,
        true,
        false,
    )
    .await
    .unwrap();
    let item = repo::items::add_item(
        &db,
        tenant_id,
        "Lot Guard Item",
        None,
        "case",
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let inventory_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "Lot Guard Owner",
        "lot-owner@test.com",
    )
    .await
    .unwrap();
    repo::inventory_owners::replace_inventory_owner_facilities(
        &db,
        tenant_id,
        inventory_owner,
        &[facility],
    )
    .await
    .unwrap();
    let lot_a = repo::inventory::add_item_batch(
        &db,
        tenant_id,
        inventory_owner,
        item,
        None,
        Some("LOT-A"),
        None,
        None,
    )
    .await
    .unwrap();
    let lot_b = repo::inventory::add_item_batch(
        &db,
        tenant_id,
        inventory_owner,
        item,
        None,
        Some("LOT-B"),
        None,
        None,
    )
    .await
    .unwrap();
    let exp_a = repo::inventory::add_item_batch(
        &db,
        tenant_id,
        inventory_owner,
        item,
        None,
        Some("LOT-A"),
        None,
        Some(db::now_iso()),
    )
    .await
    .unwrap();

    repo::inventory::receive_inventory(
        &db,
        tenant_id,
        user.id,
        lot_a,
        receiving,
        10,
        None,
        None,
        None,
        None,
        "location-restriction-receive-lot-a",
    )
    .await
    .unwrap();

    let err = repo::inventory::receive_inventory(
        &db,
        tenant_id,
        user.id,
        lot_b,
        receiving,
        5,
        None,
        None,
        None,
        None,
        "location-restriction-receive-lot-b",
    )
    .await
    .unwrap_err();
    assert!(matches!(
        err,
        AppError::Application(ApplicationError::Conflict(_))
    ));

    let err = repo::inventory::receive_inventory(
        &db,
        tenant_id,
        user.id,
        exp_a,
        receiving,
        5,
        None,
        None,
        None,
        None,
        "location-restriction-receive-expired",
    )
    .await
    .unwrap_err();
    assert!(matches!(
        err,
        AppError::Application(ApplicationError::Conflict(_))
    ));

    repo::inventory::receive_inventory(
        &db,
        tenant_id,
        user.id,
        lot_b,
        reserve,
        5,
        None,
        None,
        None,
        None,
        "location-restriction-receive-reserve",
    )
    .await
    .unwrap();

    let err = repo::inventory::move_inventory(
        &db,
        tenant_id,
        user.id,
        lot_b,
        reserve,
        receiving,
        1,
        None,
        None,
        None,
        None,
        "location-restriction-move",
    )
    .await
    .unwrap_err();
    assert!(matches!(
        err,
        AppError::Application(ApplicationError::Conflict(_))
    ));
}
