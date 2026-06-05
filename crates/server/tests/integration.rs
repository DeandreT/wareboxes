//! Integration tests for the ported business rules, running against PostgreSQL.
//! Each test gets a fresh database created from `TEST_DATABASE_URL` credentials.

use std::sync::atomic::{AtomicU64, Ordering};

use sqlx::postgres::PgPoolOptions;
use url::Url;
use wareboxes_core::dto::{NewOrder, OrderUpdate};
use wareboxes_core::models::{LoadLineStatus, LoadStatus, LoadType, MovementType, OrderStatus};
use wareboxes_core::CoreError;
use wareboxes_server::error::AppError;
use wareboxes_server::{auth, db, permissions, repo};

const DEFAULT_TEST_DATABASE_URL: &str = "postgres://wareboxes:wareboxes@127.0.0.1:5433/wareboxes";
static NEXT_TEST_DB_ID: AtomicU64 = AtomicU64::new(1);

fn set_db_name(database_url: &str, db_name: &str) -> String {
    let mut parsed = Url::parse(database_url).expect("valid TEST_DATABASE_URL");
    parsed.set_path(&format!("/{db_name}"));
    parsed.to_string()
}

async fn setup() -> db::Db {
    let base_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_string());
    let database_name = format!(
        "wareboxes_test_{}_{}",
        std::process::id(),
        NEXT_TEST_DB_ID.fetch_add(1, Ordering::Relaxed)
    );

    let admin_url = set_db_name(&base_url, "postgres");
    let test_url = set_db_name(&base_url, &database_name);

    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .unwrap_or_else(|e| panic!("connect admin db ({admin_url}): {e}"));

    sqlx::query(&format!("DROP DATABASE IF EXISTS \"{database_name}\""))
        .execute(&admin_pool)
        .await
        .unwrap_or_else(|e| panic!("drop test db ({database_name}): {e}"));
    sqlx::query(&format!("CREATE DATABASE \"{database_name}\""))
        .execute(&admin_pool)
        .await
        .unwrap_or_else(|e| panic!("create test db ({database_name}): {e}"));

    let pool = db::connect(&test_url)
        .await
        .unwrap_or_else(|e| panic!("connect test db ({test_url}): {e}"));
    db::run_migrations(&pool).await.unwrap();
    pool
}

fn new_order(key: &str, account_id: Option<i64>) -> NewOrder {
    NewOrder {
        order_key: key.to_string(),
        rush: Some(false),
        ship_by: None,
        line1: Some("1 Main St".into()),
        line2: None,
        city: "Reno".into(),
        state: "NV".into(),
        postal_code: "89501".into(),
        country: "US".into(),
        account_id,
    }
}

#[tokio::test]
async fn auth_and_hierarchical_rbac() {
    let db = setup().await;

    let user = auth::register_user(&db, "admin@test.com", "supersecret", None, None)
        .await
        .unwrap();
    assert!(
        auth::verify_credentials(&db, "admin@test.com", "supersecret")
            .await
            .unwrap()
            .is_some()
    );
    assert!(auth::verify_credentials(&db, "admin@test.com", "wrong")
        .await
        .unwrap()
        .is_none());

    // Role hierarchy: `child.parent_id = parent`. The ported recursive CTE
    // resolves a user's roles plus their descendants, so a holder of `parent`
    // inherits permissions assigned to `child`.
    let perm = repo::permissions::add_permission(&db, "orders", Some("Orders"))
        .await
        .unwrap();
    let parent = repo::roles::add_role(&db, "parent", Some("p"))
        .await
        .unwrap();
    let child = repo::roles::add_role(&db, "child", Some("c"))
        .await
        .unwrap();
    repo::roles::add_role_permission(&db, child, perm)
        .await
        .unwrap();
    repo::roles::add_role_relationship(&db, parent, child)
        .await
        .unwrap();
    repo::roles::add_role_to_user(&db, user.id, parent)
        .await
        .unwrap();

    // Inherited through the role hierarchy (parent -> child's permission).
    assert!(permissions::user_has_permission(&db, user.id, "orders")
        .await
        .unwrap());
    assert!(
        permissions::user_has_any_permission(&db, user.id, &["nope", "orders"])
            .await
            .unwrap()
    );
    assert!(!permissions::user_has_permission(&db, user.id, "missing")
        .await
        .unwrap());

    // Closures resolved by get_role.
    let child_role = repo::roles::get_role(&db, child).await.unwrap().unwrap();
    assert!(child_role.parent_roles.iter().any(|r| r.id == parent));
    let parent_role = repo::roles::get_role(&db, parent).await.unwrap().unwrap();
    assert!(parent_role.child_roles.iter().any(|r| r.id == child));

    // The per-user self role must be undeletable.
    let self_role = repo::roles::get_roles(&db, true, true)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.name == "admin@test.com")
        .unwrap();
    let err = repo::roles::delete_user_role(&db, user.id, self_role.id)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::BadRequest(_))));
}

#[tokio::test]
async fn order_status_guards_and_soft_delete() {
    let db = setup().await;

    assert!(repo::orders::add_order(&db, &new_order("O1", None))
        .await
        .unwrap());
    let orders = repo::orders::get_orders(&db).await.unwrap();
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].status, OrderStatus::Open);
    let id = orders[0].id;

    // 'open' is mutable & not closed/confirmed -> deletable.
    assert!(repo::orders::delete_order(&db, id).await.unwrap());
    assert!(repo::orders::get_orders(&db).await.unwrap().is_empty());

    // A shipped order is no longer mutable or deletable.
    assert!(repo::orders::add_order(&db, &new_order("O2", None))
        .await
        .unwrap());
    let id2 = repo::orders::get_orders(&db).await.unwrap()[0].id;
    let to_shipped = OrderUpdate {
        order_id: id2,
        order_key: None,
        status: Some(OrderStatus::Shipped),
        rush: None,
        confirmed: None,
        closed: None,
        ship_by: None,
        wave_id: None,
        account_id: None,
        line1: None,
        line2: None,
        city: None,
        state: None,
        postal_code: None,
        country: None,
    };
    // 'open' -> allowed.
    assert!(repo::orders::update_order(&db, &to_shipped).await.unwrap());
    // Now 'shipped': further update is rejected by the status guard.
    let mut again = to_shipped.clone();
    again.status = Some(OrderStatus::Held);
    assert!(!repo::orders::update_order(&db, &again).await.unwrap());
    assert!(!repo::orders::delete_order(&db, id2).await.unwrap());
}

#[tokio::test]
async fn order_pagination_filters_and_reports_total() {
    let db = setup().await;

    for key in ["PAGE-A", "PAGE-B", "OTHER-C"] {
        repo::orders::add_order(&db, &new_order(key, None))
            .await
            .unwrap();
    }

    let page = repo::orders::get_orders_page(&db, 2, 0, None, Some("PAGE"))
        .await
        .unwrap();
    assert_eq!(page.page.total, 2);
    assert_eq!(page.page.limit, 2);
    assert_eq!(page.page.offset, 0);
    assert_eq!(page.page.items.len(), 2);
    assert!(page
        .page
        .items
        .iter()
        .all(|order| order.order_key.starts_with("PAGE")));
    assert_eq!(page.summaries.len(), 1);
    assert_eq!(page.summaries[0].key, "open");
    assert_eq!(page.summaries[0].count, 2);

    let second_page = repo::orders::get_orders_page(&db, 2, 2, None, None)
        .await
        .unwrap();
    assert_eq!(second_page.page.total, 3);
    assert_eq!(second_page.page.items.len(), 1);

    let open_page = repo::orders::get_orders_page(&db, 10, 0, Some(OrderStatus::Open), None)
        .await
        .unwrap();
    assert_eq!(open_page.page.total, 3);
}

#[tokio::test]
async fn warehouses_listing_filters_deleted() {
    let db = setup().await;

    let keep = repo::warehouses::add_warehouse(&db, "Main DC")
        .await
        .unwrap();
    let gone = repo::warehouses::add_warehouse(&db, "Old DC")
        .await
        .unwrap();
    sqlx::query("UPDATE warehouses SET deleted = $1 WHERE id = $2")
        .bind(db::now_iso())
        .bind(gone)
        .execute(&db)
        .await
        .unwrap();

    let active = repo::warehouses::get_warehouses(&db, false).await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, keep);

    let all = repo::warehouses::get_warehouses(&db, true).await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn account_delete_blocked_by_open_orders() {
    let db = setup().await;

    let acc = repo::accounts::add_account(&db, "Acme", "ops@acme.test")
        .await
        .unwrap();
    repo::orders::add_order(&db, &new_order("A1", Some(acc)))
        .await
        .unwrap();

    let err = repo::accounts::delete_account(&db, acc).await.unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));

    // Ship the order, then deletion is allowed.
    let oid = repo::orders::get_orders(&db).await.unwrap()[0].id;
    let upd = OrderUpdate {
        order_id: oid,
        order_key: None,
        status: Some(OrderStatus::Shipped),
        rush: None,
        confirmed: None,
        closed: None,
        ship_by: None,
        wave_id: None,
        account_id: None,
        line1: None,
        line2: None,
        city: None,
        state: None,
        postal_code: None,
        country: None,
    };
    assert!(repo::orders::update_order(&db, &upd).await.unwrap());
    assert!(repo::accounts::delete_account(&db, acc).await.unwrap());
}

#[tokio::test]
async fn selector_reference_helpers_filter_deleted_and_inactive_records() {
    let db = setup().await;

    let user = auth::register_user(&db, "selector@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let warehouse = repo::warehouses::add_warehouse(&db, "Selector DC")
        .await
        .unwrap();
    let deleted_warehouse = repo::warehouses::add_warehouse(&db, "Deleted DC")
        .await
        .unwrap();
    sqlx::query("UPDATE warehouses SET deleted = $1 WHERE id = $2")
        .bind(db::now_iso())
        .bind(deleted_warehouse)
        .execute(&db)
        .await
        .unwrap();

    let account = repo::accounts::add_account(&db, "Selector Account", "ops@selector.test")
        .await
        .unwrap();
    let deleted_account = repo::accounts::add_account(&db, "Deleted Account", "gone@test")
        .await
        .unwrap();
    assert!(repo::accounts::delete_account(&db, deleted_account)
        .await
        .unwrap());

    let item = repo::items::add_item(
        &db,
        "Selector Item",
        None,
        "each",
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let deleted_item = repo::items::add_item(
        &db,
        "Deleted Item",
        None,
        "each",
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(repo::items::set_item_deleted(&db, deleted_item, true)
        .await
        .unwrap());

    let active_location = repo::locations::add_location(
        &db,
        warehouse,
        None,
        Some("ACTIVE"),
        Some("Active"),
        "bin",
        true,
        true,
        true,
    )
    .await
    .unwrap();
    let inactive_location = repo::locations::add_location(
        &db,
        warehouse,
        None,
        Some("INACTIVE"),
        Some("Inactive"),
        "bin",
        false,
        false,
        false,
    )
    .await
    .unwrap();
    let deleted_location = repo::locations::add_location(
        &db,
        warehouse,
        None,
        Some("DELETED"),
        Some("Deleted"),
        "bin",
        true,
        false,
        false,
    )
    .await
    .unwrap();
    assert!(
        repo::locations::set_location_deleted(&db, deleted_location, true)
            .await
            .unwrap()
    );

    let load = repo::loads::add_load(
        &db,
        user.id,
        warehouse,
        account,
        LoadType::Inbound,
        Some("SEL-LOAD"),
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

    assert!(repo::warehouses::active_warehouse_exists(&db, warehouse)
        .await
        .unwrap());
    assert!(
        !repo::warehouses::active_warehouse_exists(&db, deleted_warehouse)
            .await
            .unwrap()
    );
    assert!(repo::accounts::active_account_exists(&db, account)
        .await
        .unwrap());
    assert!(!repo::accounts::active_account_exists(&db, deleted_account)
        .await
        .unwrap());
    assert!(repo::items::active_item_exists(&db, item).await.unwrap());
    assert!(!repo::items::active_item_exists(&db, deleted_item)
        .await
        .unwrap());
    assert!(repo::loads::active_load_exists(&db, load).await.unwrap());
    assert!(!repo::loads::active_load_exists(&db, 999_999).await.unwrap());
    assert!(
        repo::locations::active_location_exists(&db, active_location)
            .await
            .unwrap()
    );
    assert_eq!(
        repo::locations::location_active_state(&db, active_location)
            .await
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        repo::locations::location_active_state(&db, inactive_location)
            .await
            .unwrap(),
        Some(false)
    );
    assert_eq!(
        repo::locations::location_active_state(&db, deleted_location)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn order_and_load_mutations_write_activity_history() {
    let db = setup().await;

    repo::orders::add_order(&db, &new_order("ACT-ORDER", None))
        .await
        .unwrap();
    let order_id = repo::orders::get_orders(&db).await.unwrap()[0].id;
    let update = OrderUpdate {
        order_id,
        order_key: None,
        status: Some(OrderStatus::Held),
        rush: None,
        confirmed: None,
        closed: None,
        ship_by: None,
        wave_id: None,
        account_id: None,
        line1: None,
        line2: None,
        city: None,
        state: None,
        postal_code: None,
        country: None,
    };
    assert!(repo::orders::update_order(&db, &update).await.unwrap());
    assert!(repo::orders::delete_order(&db, order_id).await.unwrap());
    assert!(repo::orders::restore_order(&db, order_id).await.unwrap());

    let order_actions = sqlx::query_scalar::<_, String>(
        "SELECT action FROM order_activity WHERE order_id = $1 ORDER BY id",
    )
    .bind(order_id)
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(
        order_actions,
        vec![
            "created order",
            "updated order status to held",
            "deleted order",
            "restored order",
        ]
    );

    let user = auth::register_user(&db, "activity@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let warehouse = repo::warehouses::add_warehouse(&db, "Activity DC")
        .await
        .unwrap();
    let account = repo::accounts::add_account(&db, "Activity Account", "activity@test")
        .await
        .unwrap();
    let load_id = repo::loads::add_load(
        &db,
        user.id,
        warehouse,
        account,
        LoadType::Inbound,
        Some("ACT-LOAD"),
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
    assert!(repo::loads::update_load(
        &db,
        user.id,
        load_id,
        Some(LoadStatus::Arrived),
        None,
        None,
        Some("INV-ACT"),
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
    let note_id = repo::loads::add_note(&db, user.id, load_id, "activity note")
        .await
        .unwrap();
    assert!(
        repo::loads::set_load_note_deleted(&db, user.id, note_id, true)
            .await
            .unwrap()
    );
    assert!(repo::loads::set_load_deleted(&db, user.id, load_id, true)
        .await
        .unwrap());

    let load_actions = sqlx::query_scalar::<_, String>(
        "SELECT action FROM load_activity WHERE load_id = $1 ORDER BY id",
    )
    .bind(load_id)
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(
        load_actions,
        vec![
            "created",
            "updated",
            "note_added",
            "note_deleted",
            "deleted"
        ]
    );
}

#[tokio::test]
async fn barcode_uniqueness_allows_same_item_different_type_only() {
    let db = setup().await;

    let item_one = repo::items::add_item(
        &db,
        "Barcode Item 1",
        None,
        "each",
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let item_two = repo::items::add_item(
        &db,
        "Barcode Item 2",
        None,
        "each",
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let value = "036000291452";
    let code128 = repo::items::add_barcode(&db, item_one, value, "code128", None)
        .await
        .unwrap();
    let upc = repo::items::add_barcode(&db, item_one, value, "upc-a", None)
        .await
        .unwrap();
    assert_ne!(code128, upc);

    let same_item_same_type = repo::items::add_barcode(&db, item_one, value, "code128", None)
        .await
        .unwrap_err();
    assert!(matches!(
        same_item_same_type,
        AppError::Db(sqlx::Error::Database(ref err))
            if err.kind() == sqlx::error::ErrorKind::UniqueViolation
    ));

    let other_item_different_type = repo::items::add_barcode(&db, item_two, value, "qr", None)
        .await
        .unwrap_err();
    assert!(matches!(
        other_item_different_type,
        AppError::Db(sqlx::Error::Database(ref err))
            if err.kind() == sqlx::error::ErrorKind::UniqueViolation
    ));

    assert!(repo::items::set_barcode_deleted(&db, code128, true)
        .await
        .unwrap());
    assert!(repo::items::set_barcode_deleted(&db, upc, true)
        .await
        .unwrap());
    assert!(repo::items::add_barcode(&db, item_two, value, "qr", None)
        .await
        .is_ok());
}

#[tokio::test]
async fn inventory_receive_move_and_reserve_updates_balances_and_ledger() {
    let db = setup().await;

    let user = auth::register_user(&db, "wms@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let warehouse = repo::warehouses::add_warehouse(&db, "Main DC")
        .await
        .unwrap();
    let receiving = repo::locations::add_location(
        &db,
        warehouse,
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
        warehouse,
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
        &db, "Widget", None, "each", None, None, None, None, None, None,
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

    let acc = repo::accounts::add_account(&db, "Inventory Customer", "ic@test.com")
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
        warehouse,
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
        warehouse,
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
    let warehouse = repo::warehouses::add_warehouse(&db, "Lot Guard DC")
        .await
        .unwrap();
    let receiving = repo::locations::add_location(
        &db,
        warehouse,
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
        warehouse,
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

#[tokio::test]
async fn inbound_load_lines_receive_into_inventory_with_close_guards() {
    let db = setup().await;

    let user = auth::register_user(&db, "receiver@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let warehouse = repo::warehouses::add_warehouse(&db, "Inbound DC")
        .await
        .unwrap();
    let dock = repo::locations::add_location(
        &db,
        warehouse,
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
    let account = repo::accounts::add_account(&db, "Load Customer", "loads@test.com")
        .await
        .unwrap();
    let item = repo::items::add_item(
        &db, "Cases", None, "case", None, None, None, None, None, None,
    )
    .await
    .unwrap();

    let load = repo::loads::add_load(
        &db,
        user.id,
        warehouse,
        account,
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

    let movement = repo::loads::receive_line(
        &db,
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
    )
    .await
    .unwrap();
    assert!(movement > 0);

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

    let balances = repo::inventory::get_balances(&db, false).await.unwrap();
    assert_eq!(balances.len(), 1);
    assert_eq!(balances[0].location_id, dock);
    assert_eq!(balances[0].qty_on_hand, 8);

    let mixed_lot_load = repo::loads::add_load(
        &db,
        user.id,
        warehouse,
        account,
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
async fn inbound_receive_can_use_license_plate_and_confirm_missing() {
    let db = setup().await;

    let user = auth::register_user(&db, "lp-receiver@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let warehouse = repo::warehouses::add_warehouse(&db, "LP Inbound DC")
        .await
        .unwrap();
    let dock = repo::locations::add_location(
        &db,
        warehouse,
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
        warehouse,
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
    let account = repo::accounts::add_account(&db, "LP Customer", "lp@test.com")
        .await
        .unwrap();
    let item = repo::items::add_item(
        &db,
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
        warehouse,
        account,
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

    let movement = repo::loads::receive_line(
        &db,
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
    )
    .await
    .unwrap();
    assert!(movement > 0);

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

    let plates = repo::license_plates::get_license_plates(&db, false)
        .await
        .unwrap();
    assert_eq!(plates.len(), 1);
    let plate = &plates[0];
    assert_eq!(plate.barcode.as_deref(), Some("LP-0001"));
    assert_eq!(plate.location_id, Some(dock));
    assert_eq!(plate.contents.len(), 1);
    assert_eq!(plate.contents[0].location_id, dock);
    assert_eq!(plate.contents[0].qty_on_hand, 5);

    let balances = repo::inventory::get_balances(&db, false).await.unwrap();
    assert_eq!(balances.len(), 1);
    assert_eq!(balances[0].license_plate_id, Some(plate.id));
    assert_eq!(balances[0].qty_on_hand, 5);

    assert!(repo::license_plates::move_license_plate(
        &db,
        user.id,
        plate.id,
        reserve,
        Some("putaway"),
        Some("lp-move-1"),
    )
    .await
    .unwrap());

    let moved = repo::license_plates::get_license_plate_by_barcode(&db, "LP-0001")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(moved.location_id, Some(reserve));
    assert_eq!(moved.contents[0].location_id, reserve);

    let acc = repo::accounts::add_account(&db, "LP Customer", "lp-customer@test.com")
        .await
        .unwrap();
    repo::orders::add_order(&db, &new_order("LP-RES-1", Some(acc)))
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
        user.id,
        order_id,
        None,
        moved.contents[0].inventory_balance_id,
        1,
    )
    .await
    .unwrap();

    let err = repo::license_plates::move_license_plate(
        &db,
        user.id,
        plate.id,
        dock,
        Some("reserved putback"),
        Some("lp-move-reserved"),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));

    let err = repo::license_plates::set_license_plate_deleted(&db, plate.id, true)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));
}
