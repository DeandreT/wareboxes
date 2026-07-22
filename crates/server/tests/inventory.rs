mod common;

use common::*;

#[tokio::test]
async fn inventory_receive_move_and_reserve_updates_balances_and_ledger() {
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
    let batch = repo::inventory::add_item_batch(&db, item, None, Some("LOT-1"), None, None)
        .await
        .unwrap();

    let receive_move = repo::inventory::receive_inventory(
        &db,
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

    repo::inventory::move_inventory(
        &db,
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

    let balances = repo::inventory::get_balances(&db, false).await.unwrap();
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
        &db, user.id, batch, pick_face, receiving, 31, None, None, None, None, None,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));

    let acc = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "Inventory Customer",
        "ic@test.com",
    )
    .await
    .unwrap();
    repo::orders::add_order(&db, &new_order("INV-1", Some(acc)))
        .await
        .unwrap();
    let order_id = repo::orders::get_orders(&db).await.unwrap()[0].id;
    let reservation =
        repo::inventory::reserve_inventory(&db, user.id, order_id, None, pick_balance.id, 20)
            .await
            .unwrap();
    let balances = repo::inventory::get_balances(&db, false).await.unwrap();
    let pick_balance = balances
        .iter()
        .find(|b| b.location_id == pick_face && b.item_batch_id == batch)
        .unwrap();
    assert_eq!(pick_balance.qty_on_hand, 30);
    assert_eq!(pick_balance.qty_reserved, 20);
    let reservations = repo::inventory::get_reservations(&db, false).await.unwrap();
    assert_eq!(reservations.len(), 1);
    assert_eq!(reservations[0].inventory_balance_id, pick_balance.id);

    let err = repo::inventory::move_inventory(
        &db, user.id, batch, pick_face, receiving, 11, None, None, None, None, None,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));

    assert!(repo::inventory::cancel_reservation(&db, reservation)
        .await
        .unwrap());
    let balances = repo::inventory::get_balances(&db, false).await.unwrap();
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
    let split_moves = repo::inventory::split_move_inventory(
        &db,
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
    assert_eq!(split_moves.len(), 2);
    let balances = repo::inventory::get_balances(&db, false).await.unwrap();
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

    let movements = repo::inventory::get_movements(&db).await.unwrap();
    assert!(movements.iter().any(|m| {
        m.movement_type == MovementType::Receive
            && m.to_location_id == Some(receiving)
            && m.reason.as_deref() == Some("initial receipt")
            && m.idempotency_key.as_deref() == Some("receipt-42")
    }));
    assert!(movements
        .iter()
        .any(|m| m.movement_type == MovementType::Move && m.from_location_id == Some(receiving)));
    assert!(movements
        .iter()
        .any(|m| m.movement_type == MovementType::Reserve && m.reference_id == Some(order_id)));
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
    let lot_a = repo::inventory::add_item_batch(&db, item, None, Some("LOT-A"), None, None)
        .await
        .unwrap();
    let lot_b = repo::inventory::add_item_batch(&db, item, None, Some("LOT-B"), None, None)
        .await
        .unwrap();
    let exp_a =
        repo::inventory::add_item_batch(&db, item, None, Some("LOT-A"), None, Some(db::now_iso()))
            .await
            .unwrap();

    repo::inventory::receive_inventory(
        &db, user.id, lot_a, receiving, 10, None, None, None, None, None,
    )
    .await
    .unwrap();

    let err = repo::inventory::receive_inventory(
        &db, user.id, lot_b, receiving, 5, None, None, None, None, None,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));

    let err = repo::inventory::receive_inventory(
        &db, user.id, exp_a, receiving, 5, None, None, None, None, None,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));

    repo::inventory::receive_inventory(
        &db, user.id, lot_b, reserve, 5, None, None, None, None, None,
    )
    .await
    .unwrap();

    let err = repo::inventory::move_inventory(
        &db, user.id, lot_b, reserve, receiving, 1, None, None, None, None, None,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));
}
