mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use common::*;
use tower::ServiceExt;
use wareboxes_server::auth::TENANT_ID_HEADER;
use wareboxes_server::{routes, state::AppState};

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
        tenant_id,
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
        tenant_id,
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
        tenant_id,
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
        tenant_id,
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

    let loads = repo::loads::get_loads(&db, tenant_id, false, false)
        .await
        .unwrap();
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
        tenant_id,
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
        tenant_id,
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
        tenant_id,
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
        tenant_id,
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
    let closed = repo::loads::get_loads(&db, tenant_id, false, false)
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
        tenant_id,
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
    let line = repo::loads::add_line(
        &fixture.db,
        tenant_id,
        user.id,
        load,
        item,
        None,
        10,
        None,
        None,
        None,
    )
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

    let load = repo::loads::get_load(&fixture.db, tenant_id, load, false)
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
async fn load_aggregate_is_isolated_by_selected_tenant() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("load-scope-operator@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, operator.id).await;
    let second_user = fixture.user("load-scope-second@test.com").await;
    let tenant_b = tenant_for_user(&fixture.db, second_user.id).await;
    sqlx::query(
        "INSERT INTO tenant_memberships (tenant_id, user_id, is_default) VALUES ($1, $2, FALSE)",
    )
    .bind(tenant_b.get())
    .bind(operator.id)
    .execute(&fixture.db)
    .await
    .unwrap();
    let tenant_b_permission =
        repo::permissions::add_permission(&fixture.db, tenant_b, "wms", Some("WMS"))
            .await
            .unwrap();
    let tenant_b_role = repo::roles::add_role(
        &fixture.db,
        tenant_b,
        "load-scope-operator@test.com-wms",
        Some("WMS worker"),
    )
    .await
    .unwrap();
    repo::roles::add_role_permission(&fixture.db, tenant_b, tenant_b_role, tenant_b_permission)
        .await
        .unwrap();
    repo::roles::add_role_to_user(&fixture.db, tenant_b, operator.id, tenant_b_role)
        .await
        .unwrap();

    let facility = fixture.facility(tenant_a, "Scoped Load Facility").await;
    let dock = repo::locations::add_location(
        &fixture.db,
        tenant_a,
        facility,
        None,
        Some("SCOPED-LOAD-DOCK"),
        Some("Scoped Load Dock"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let owner = fixture.inventory_owner(tenant_a, "Scoped Load Owner").await;
    let item = fixture.item(tenant_a, "Scoped Load Item", "case").await;
    let load_id = repo::loads::add_load(
        &fixture.db,
        tenant_a,
        operator.id,
        facility,
        owner,
        LoadType::Inbound,
        Some("SCOPED-LOAD"),
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
    let line_id = repo::loads::add_line(
        &fixture.db,
        tenant_a,
        operator.id,
        load_id,
        item,
        None,
        4,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let note_id =
        repo::loads::add_note(&fixture.db, tenant_a, operator.id, load_id, "tenant A note")
            .await
            .unwrap();
    let file_id = repo::loads::add_file(
        &fixture.db,
        tenant_a,
        operator.id,
        load_id,
        "scope.txt",
        "scope.txt",
        "/tmp/scope.txt",
        Some("text/plain"),
        wareboxes_core::models::LoadFileCategory::General,
    )
    .await
    .unwrap();
    let tenant_a_order = fixture.order(tenant_a, "SCOPED-LOAD-ORDER-A", owner).await;
    sqlx::query(
        "INSERT INTO load_orders (tenant_id, inventory_owner_id, created, load_id, order_id) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(tenant_a.get())
    .bind(owner)
    .bind(db::now_iso())
    .bind(load_id)
    .bind(tenant_a_order)
    .execute(&fixture.db)
    .await
    .unwrap();
    let tenant_b_owner = fixture
        .inventory_owner(tenant_b, "Other Tenant Owner")
        .await;
    let tenant_b_order = fixture
        .order(tenant_b, "SCOPED-LOAD-ORDER-B", tenant_b_owner)
        .await;
    assert!(sqlx::query(
        "INSERT INTO load_orders (tenant_id, inventory_owner_id, created, load_id, order_id) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(tenant_a.get())
    .bind(owner)
    .bind(db::now_iso())
    .bind(load_id)
    .bind(tenant_b_order)
    .execute(&fixture.db)
    .await
    .is_err());

    assert!(repo::loads::get_loads(&fixture.db, tenant_b, true, true)
        .await
        .unwrap()
        .is_empty());
    assert!(repo::loads::get_load(&fixture.db, tenant_b, load_id, true)
        .await
        .unwrap()
        .is_none());
    assert!(
        !repo::loads::active_load_exists(&fixture.db, tenant_b, load_id)
            .await
            .unwrap()
    );
    assert!(!repo::loads::update_load(
        &fixture.db,
        tenant_b,
        operator.id,
        load_id,
        None,
        None,
        Some("cross-tenant update"),
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
    assert!(
        !repo::loads::set_load_deleted(&fixture.db, tenant_b, operator.id, load_id, true,)
            .await
            .unwrap()
    );
    assert!(
        !repo::loads::set_load_note_deleted(&fixture.db, tenant_b, operator.id, note_id, true,)
            .await
            .unwrap()
    );
    assert!(
        !repo::loads::delete_file(&fixture.db, tenant_b, operator.id, file_id)
            .await
            .unwrap()
    );
    assert!(repo::loads::get_file(&fixture.db, tenant_b, file_id)
        .await
        .unwrap()
        .is_none());
    assert!(repo::loads::add_note(
        &fixture.db,
        tenant_b,
        operator.id,
        load_id,
        "cross-tenant note",
    )
    .await
    .is_err());
    assert!(repo::loads::add_line(
        &fixture.db,
        tenant_b,
        operator.id,
        load_id,
        item,
        None,
        1,
        None,
        None,
        None,
    )
    .await
    .is_err());
    assert!(repo::loads::receive_line(
        &fixture.db,
        tenant_b,
        operator.id,
        line_id,
        dock,
        1,
        0,
        0,
        None,
        None,
        None,
        None,
        None,
        Some("cross-tenant receipt"),
        "cross-tenant-receipt",
    )
    .await
    .is_err());

    let scoped_load = repo::loads::get_load(&fixture.db, tenant_a, load_id, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(scoped_load.tenant_id, tenant_a);
    assert_eq!(scoped_load.reference_number.as_deref(), Some("SCOPED-LOAD"));
    assert!(scoped_load
        .notes
        .iter()
        .all(|note| note.tenant_id == tenant_a));
    assert!(scoped_load
        .files
        .iter()
        .all(|file| file.tenant_id == tenant_a));
    assert!(scoped_load
        .lines
        .iter()
        .all(|line| line.tenant_id == tenant_a));
    assert!(scoped_load
        .activity
        .iter()
        .all(|event| event.tenant_id == tenant_a));
    assert_eq!(scoped_load.orders.len(), 1);
    assert_eq!(scoped_load.orders[0].id, tenant_a_order);

    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/loads/{load_id}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/loads/{load_id}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, tenant_b.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
        serde_json::Value::Null
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/loads/{load_id}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, tenant_a.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let route_load = serde_json::from_slice::<Option<wareboxes_core::models::Load>>(&body)
        .unwrap()
        .unwrap();
    assert_eq!(route_load.tenant_id, tenant_a);
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
        tenant_id,
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
    let line = repo::loads::add_line(
        &db, tenant_id, user.id, load, item, None, 12, None, None, None,
    )
    .await
    .unwrap();

    assert!(repo::loads::update_load(
        &db,
        tenant_id,
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

    let load_row = repo::loads::get_loads(&db, tenant_id, false, false)
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
    assert_eq!(plate.facility_id, facility);
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

    repo::orders::add_order(&db, tenant_id, &new_order("LP-RES-1", inventory_owner))
        .await
        .unwrap();
    let order_id = repo::orders::get_orders(&db, tenant_id)
        .await
        .unwrap()
        .into_iter()
        .find(|o| o.order_key == "LP-RES-1")
        .unwrap()
        .id;
    repo::inventory::reserve_inventory(
        &db,
        &repo::inventory::ReserveInventoryCommand {
            tenant_id,
            actor_user_id: user.id,
            order_id,
            order_item_id: None,
            inventory_balance_id: moved.contents[0].inventory_balance_id,
            qty: 1,
            idempotency_key: "load-reservation-setup",
        },
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
