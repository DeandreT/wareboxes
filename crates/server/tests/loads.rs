mod common;

use common::*;

#[tokio::test]
async fn inbound_load_lines_receive_into_inventory_with_close_guards() {
    let db = setup().await;

    let user = auth::register_user(&db, "receiver@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let tenant_id = tenant_for_user(&db, user.id).await;
    let facility = repo::facilities::add_facility(&db, tenant_id, "Inbound DC")
        .await
        .unwrap();
    let dock = repo::locations::add_location(
        &db,
        tenant_id,
        facility,
        None,
        Some("DOCK-1"),
        Some("Dock 1"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let inventory_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "Load Customer",
        "loads@test.com",
    )
    .await
    .unwrap();
    let item = repo::items::add_item(
        &db, tenant_id, "Cases", None, "case", None, None, None, None, None, None,
    )
    .await
    .unwrap();

    let load = repo::loads::add_load(
        &db,
        user.id,
        facility,
        inventory_owner,
        wareboxes_core::models::LoadType::Inbound,
        Some("BOL-100"),
        None,
        Some("Acme Carrier"),
        Some("TRL-9"),
        Some("SEAL-1"),
        Some(dock),
        None,
        None,
    )
    .await
    .unwrap();
    let line = repo::loads::add_line(
        &db,
        user.id,
        load,
        item,
        None,
        10,
        Some("LOT-L"),
        None,
        None,
    )
    .await
    .unwrap();

    let err = repo::loads::update_load(
        &db,
        user.id,
        load,
        Some(LoadStatus::Closed),
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
        Some(db::now_iso()),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));

    let err = repo::loads::receive_line(
        &db,
        tenant_id,
        user.id,
        line,
        dock,
        11,
        0,
        0,
        None,
        None,
        None,
        None,
        None,
        Some("too much"),
        "receive-too-much",
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));

    assert!(repo::loads::update_load(
        &db,
        user.id,
        load,
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
        None,
    )
    .await
    .unwrap());

    let receipt = repo::loads::receive_line(
        &db,
        tenant_id,
        user.id,
        line,
        dock,
        8,
        2,
        0,
        None,
        None,
        None,
        None,
        None,
        Some("normal receipt"),
        "receive-normal",
    )
    .await
    .unwrap();
    assert!(receipt.inventory_transaction_id.is_some());

    let replay = repo::loads::receive_line(
        &db,
        tenant_id,
        user.id,
        line,
        dock,
        8,
        2,
        0,
        None,
        None,
        None,
        None,
        None,
        Some("normal receipt"),
        "receive-normal",
    )
    .await
    .unwrap();
    assert_eq!(replay, receipt);

    let loads = repo::loads::get_loads(&db, false, false).await.unwrap();
    let load_row = loads.iter().find(|l| l.id == load).unwrap();
    assert_eq!(load_row.status, LoadStatus::Received);
    assert!(load_row.receive_completed);
    assert_eq!(load_row.lines.len(), 1);
    assert_eq!(load_row.lines[0].received_qty, 8);
    assert_eq!(load_row.lines[0].rejected_qty, 2);
    assert_eq!(load_row.lines[0].status, LoadLineStatus::Received);
    assert!(load_row
        .activity
        .iter()
        .any(|a| a.action == "line_received"));

    let balances = repo::inventory::get_balances(&db, tenant_id, false)
        .await
        .unwrap();
    assert_eq!(balances.len(), 1);
    assert_eq!(balances[0].location_id, dock);
    assert_eq!(balances[0].qty_on_hand, 8);

    let mixed_lot_load = repo::loads::add_load(
        &db,
        user.id,
        facility,
        inventory_owner,
        wareboxes_core::models::LoadType::Inbound,
        Some("BOL-MIXED"),
        None,
        None,
        None,
        None,
        Some(dock),
        None,
        None,
    )
    .await
    .unwrap();
    let mixed_lot_line = repo::loads::add_line(
        &db,
        user.id,
        mixed_lot_load,
        item,
        None,
        1,
        Some("LOT-OTHER"),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(repo::loads::update_load(
        &db,
        user.id,
        mixed_lot_load,
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
        None,
    )
    .await
    .unwrap());
    let err = repo::loads::receive_line(
        &db,
        tenant_id,
        user.id,
        mixed_lot_line,
        dock,
        1,
        0,
        0,
        None,
        None,
        None,
        None,
        None,
        Some("mixed lot"),
        "receive-mixed-lot",
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));

    assert!(repo::loads::update_load(
        &db,
        user.id,
        load,
        Some(LoadStatus::Closed),
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
        Some(db::now_iso()),
    )
    .await
    .unwrap());
    let closed = repo::loads::get_loads(&db, false, false)
        .await
        .unwrap()
        .into_iter()
        .find(|l| l.id == load)
        .unwrap();
    assert_eq!(closed.status, LoadStatus::Closed);
    assert_eq!(closed.closed_by, Some(user.id));
}

#[tokio::test]
async fn concurrent_load_receipts_preserve_every_accepted_quantity() {
    let fixture = Fixture::new().await;
    let user = fixture.user("concurrent-receiver@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let facility = fixture.facility(tenant_id, "Concurrent Inbound DC").await;
    let dock = repo::locations::add_location(
        &fixture.db,
        tenant_id,
        facility,
        None,
        Some("CONCURRENT-DOCK"),
        Some("Concurrent Dock"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let inventory_owner = fixture.inventory_owner(tenant_id, "Concurrent Owner").await;
    let item = fixture.item(tenant_id, "Concurrent Cases", "case").await;
    let load = repo::loads::add_load(
        &fixture.db,
        user.id,
        facility,
        inventory_owner,
        LoadType::Inbound,
        Some("CONCURRENT-RECEIPT"),
        None,
        None,
        None,
        None,
        Some(dock),
        None,
        None,
    )
    .await
    .unwrap();
    let line = repo::loads::add_line(&fixture.db, user.id, load, item, None, 10, None, None, None)
        .await
        .unwrap();
    sqlx::query("UPDATE loads SET status = 'arrived' WHERE id = $1")
        .bind(load)
        .execute(&fixture.db)
        .await
        .unwrap();

    let receipt_a = repo::loads::receive_line(
        &fixture.db,
        tenant_id,
        user.id,
        line,
        dock,
        5,
        0,
        0,
        None,
        None,
        None,
        None,
        None,
        Some("concurrent receipt"),
        "concurrent-receipt-a",
    );
    let receipt_b = repo::loads::receive_line(
        &fixture.db,
        tenant_id,
        user.id,
        line,
        dock,
        5,
        0,
        0,
        None,
        None,
        None,
        None,
        None,
        Some("concurrent receipt"),
        "concurrent-receipt-b",
    );
    let (receipt_a, receipt_b) = tokio::join!(receipt_a, receipt_b);
    let receipt_a = receipt_a.unwrap();
    let receipt_b = receipt_b.unwrap();
    assert_ne!(
        receipt_a.inventory_transaction_id,
        receipt_b.inventory_transaction_id
    );

    let load = repo::loads::get_load(&fixture.db, load, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(load.status, LoadStatus::Received);
    assert_eq!(load.lines[0].received_qty, 10);

    let balances = repo::inventory::get_balances(&fixture.db, tenant_id, false)
        .await
        .unwrap();
    assert_eq!(
        balances
            .iter()
            .filter(|balance| balance.item_id == item)
            .map(|balance| balance.qty_on_hand)
            .sum::<i64>(),
        10
    );
    assert!(
        repo::inventory::get_reconciliation_issues(&fixture.db, tenant_id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn inbound_receive_can_use_license_plate_and_confirm_missing() {
    let db = setup().await;

    let user = auth::register_user(&db, "lp-receiver@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let tenant_id = tenant_for_user(&db, user.id).await;
    let facility = repo::facilities::add_facility(&db, tenant_id, "LP Inbound DC")
        .await
        .unwrap();
    let dock = repo::locations::add_location(
        &db,
        tenant_id,
        facility,
        None,
        Some("LP-DOCK"),
        Some("LP Dock"),
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
        Some("RSV-01"),
        Some("Reserve"),
        "reserve",
        true,
        false,
        false,
    )
    .await
    .unwrap();
    let inventory_owner =
        repo::inventory_owners::add_inventory_owner(&db, tenant_id, "LP Customer", "lp@test.com")
            .await
            .unwrap();
    let item = repo::items::add_item(
        &db,
        tenant_id,
        "Palletized Cases",
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

    let load = repo::loads::add_load(
        &db,
        user.id,
        facility,
        inventory_owner,
        wareboxes_core::models::LoadType::Inbound,
        Some("BOL-LP"),
        Some("INV-123"),
        None,
        None,
        None,
        Some(dock),
        None,
        None,
    )
    .await
    .unwrap();
    let line = repo::loads::add_line(&db, user.id, load, item, None, 12, None, None, None)
        .await
        .unwrap();

    assert!(repo::loads::update_load(
        &db,
        user.id,
        load,
        Some(LoadStatus::Arrived),
        None,
        None,
        Some("INV-123-A"),
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

    let receipt = repo::loads::receive_line(
        &db,
        tenant_id,
        user.id,
        line,
        dock,
        5,
        0,
        7,
        None,
        Some("LP-0001"),
        None,
        None,
        None,
        Some("short on truck"),
        "receive-short",
    )
    .await
    .unwrap();
    assert!(receipt.inventory_transaction_id.is_some());

    let load_row = repo::loads::get_loads(&db, false, false)
        .await
        .unwrap()
        .into_iter()
        .find(|l| l.id == load)
        .unwrap();
    assert_eq!(load_row.invoice_number.as_deref(), Some("INV-123-A"));
    assert_eq!(load_row.status, LoadStatus::Received);
    assert!(load_row.receive_completed);
    assert_eq!(load_row.lines[0].received_qty, 5);
    assert_eq!(load_row.lines[0].missing_qty, 7);
    assert_eq!(load_row.lines[0].missing_confirmed_by, Some(user.id));
    assert!(load_row.lines[0].missing_confirmed_at.is_some());

    let plates = repo::license_plates::get_license_plates(&db, tenant_id, false)
        .await
        .unwrap();
    assert_eq!(plates.len(), 1);
    let plate = &plates[0];
    assert_eq!(plate.barcode.as_deref(), Some("LP-0001"));
    assert_eq!(plate.location_id, Some(dock));
    assert_eq!(plate.contents.len(), 1);
    assert_eq!(plate.contents[0].location_id, dock);
    assert_eq!(plate.contents[0].qty_on_hand, 5);

    let balances = repo::inventory::get_balances(&db, tenant_id, false)
        .await
        .unwrap();
    assert_eq!(balances.len(), 1);
    assert_eq!(balances[0].license_plate_id, Some(plate.id));
    assert_eq!(balances[0].qty_on_hand, 5);

    let plate_move_transaction = repo::license_plates::move_license_plate(
        &db,
        tenant_id,
        user.id,
        plate.id,
        reserve,
        Some("putaway"),
        Some("lp-move-1"),
    )
    .await
    .unwrap();
    assert!(plate_move_transaction > 0);

    let replayed_plate_move = repo::license_plates::move_license_plate(
        &db,
        tenant_id,
        user.id,
        plate.id,
        reserve,
        Some("putaway"),
        Some("lp-move-1"),
    )
    .await
    .unwrap();
    assert_eq!(replayed_plate_move, plate_move_transaction);

    let moved = repo::license_plates::get_license_plate_by_barcode(&db, tenant_id, "LP-0001")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(moved.location_id, Some(reserve));
    assert_eq!(moved.contents[0].location_id, reserve);

    repo::orders::add_order(&db, &new_order("LP-RES-1", Some(inventory_owner)))
        .await
        .unwrap();
    let order_id = repo::orders::get_orders(&db)
        .await
        .unwrap()
        .into_iter()
        .find(|o| o.order_key == "LP-RES-1")
        .unwrap()
        .id;
    repo::inventory::reserve_inventory(
        &db,
        tenant_id,
        order_id,
        None,
        moved.contents[0].inventory_balance_id,
        1,
    )
    .await
    .unwrap();

    let err = repo::license_plates::move_license_plate(
        &db,
        tenant_id,
        user.id,
        plate.id,
        dock,
        Some("reserved putback"),
        Some("lp-move-reserved"),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));

    let err = repo::license_plates::set_license_plate_deleted(&db, tenant_id, plate.id, true)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));
}
