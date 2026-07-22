mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use common::*;
use tower::ServiceExt;
use wareboxes_server::auth::TENANT_ID_HEADER;
use wareboxes_server::{routes, state::AppState};

#[tokio::test]
async fn inventory_commands_write_replay_safe_journal_and_balance_projection() {
    let db = setup().await;

    let user = auth::register_user(&db, "wms@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let tenant_id = tenant_for_user(&db, user.id).await;
    let facility = repo::facilities::add_facility(&db, tenant_id, "Main DC")
        .await
        .unwrap();
    let receiving = repo::locations::add_location(
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
    let pick_face = repo::locations::add_location(
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
        Some("receipt-42"),
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
        Some("receipt-42"),
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
        Some("receipt-42"),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        changed_retry,
        AppError::Core(CoreError::Conflict(_))
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
        None,
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
        &db, tenant_id, user.id, batch, pick_face, receiving, 31, None, None, None, None, None,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));

    repo::orders::add_order(&db, tenant_id, &new_order("INV-1", inventory_owner))
        .await
        .unwrap();
    let order_id = repo::orders::get_orders(&db, tenant_id).await.unwrap()[0].id;
    let reservation =
        repo::inventory::reserve_inventory(&db, tenant_id, order_id, None, pick_balance.id, 20)
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
    assert_eq!(reservations[0].inventory_balance_id, pick_balance.id);

    let err = repo::inventory::move_inventory(
        &db, tenant_id, user.id, batch, pick_face, receiving, 11, None, None, None, None, None,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));

    assert!(
        repo::inventory::cancel_reservation(&db, tenant_id, reservation)
            .await
            .unwrap()
    );
    let balances = repo::inventory::get_balances(&db, tenant_id, false)
        .await
        .unwrap();
    let pick_balance = balances
        .iter()
        .find(|b| b.location_id == pick_face && b.item_batch_id == batch)
        .unwrap();
    assert_eq!(pick_balance.qty_reserved, 0);

    let split_a = repo::locations::add_location(
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
    let split_b = repo::locations::add_location(
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
        Some("split-putaway-1"),
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
    assert!(
        sqlx::query("UPDATE inventory_transactions SET reason = 'tampered' WHERE id = $1")
            .bind(transaction_id)
            .execute(&db)
            .await
            .is_err()
    );
    assert!(sqlx::query("DELETE FROM inventory_entries WHERE id = $1")
        .bind(entry_id)
        .execute(&db)
        .await
        .is_err());
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
    .execute(&db)
    .await
    .is_err());
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
        Some("tenant-a-receipt"),
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
        Some("guessed-batch"),
    )
    .await
    .is_err());

    let other_owner_order = fixture.order(tenant_a, "OTHER-OWNER-ORDER", owner_b).await;
    let balance = repo::inventory::get_balances(&fixture.db, tenant_a, false)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let owner_mismatch = repo::inventory::reserve_inventory(
        &fixture.db,
        tenant_a,
        other_owner_order,
        None,
        balance.id,
        1,
    )
    .await
    .unwrap_err();
    assert!(matches!(
        owner_mismatch,
        AppError::Core(CoreError::Conflict(_))
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
                Some("concurrent-receipt-key"),
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
        Some("move-all-key"),
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
        Some("move-all-key"),
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
async fn inventory_rejects_mixed_lot_or_expiration_in_same_location() {
    let db = setup().await;

    let user = auth::register_user(&db, "lot-guard@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let tenant_id = tenant_for_user(&db, user.id).await;
    let facility = repo::facilities::add_facility(&db, tenant_id, "Lot Guard DC")
        .await
        .unwrap();
    let receiving = repo::locations::add_location(
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
    let reserve = repo::locations::add_location(
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
        &db, tenant_id, user.id, lot_a, receiving, 10, None, None, None, None, None,
    )
    .await
    .unwrap();

    let err = repo::inventory::receive_inventory(
        &db, tenant_id, user.id, lot_b, receiving, 5, None, None, None, None, None,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));

    let err = repo::inventory::receive_inventory(
        &db, tenant_id, user.id, exp_a, receiving, 5, None, None, None, None, None,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));

    repo::inventory::receive_inventory(
        &db, tenant_id, user.id, lot_b, reserve, 5, None, None, None, None, None,
    )
    .await
    .unwrap();

    let err = repo::inventory::move_inventory(
        &db, tenant_id, user.id, lot_b, reserve, receiving, 1, None, None, None, None, None,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));
}
