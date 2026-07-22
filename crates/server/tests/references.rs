mod common;

use common::*;

#[tokio::test]
async fn warehouses_listing_filters_deleted() {
    let db = setup().await;
    let user = auth::register_user(&db, "facilities@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let tenant_id = tenant_for_user(&db, user.id).await;

    let keep = repo::warehouses::add_warehouse(&db, tenant_id, "Main DC")
        .await
        .unwrap();
    let gone = repo::warehouses::add_warehouse(&db, tenant_id, "Old DC")
        .await
        .unwrap();
    sqlx::query("UPDATE warehouses SET deleted = $1 WHERE id = $2")
        .bind(db::now_iso())
        .bind(gone)
        .execute(&db)
        .await
        .unwrap();

    let active = repo::warehouses::get_warehouses(&db, tenant_id, false)
        .await
        .unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, keep);

    let all = repo::warehouses::get_warehouses(&db, tenant_id, true)
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn selector_reference_helpers_filter_deleted_and_inactive_records() {
    let db = setup().await;

    let user = auth::register_user(&db, "selector@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let tenant_id = tenant_for_user(&db, user.id).await;
    let warehouse = repo::warehouses::add_warehouse(&db, tenant_id, "Selector DC")
        .await
        .unwrap();
    let deleted_warehouse = repo::warehouses::add_warehouse(&db, tenant_id, "Deleted DC")
        .await
        .unwrap();
    sqlx::query("UPDATE warehouses SET deleted = $1 WHERE id = $2")
        .bind(db::now_iso())
        .bind(deleted_warehouse)
        .execute(&db)
        .await
        .unwrap();

    let account =
        repo::accounts::add_account(&db, tenant_id, "Selector Account", "ops@selector.test")
            .await
            .unwrap();
    let deleted_account =
        repo::accounts::add_account(&db, tenant_id, "Deleted Account", "gone@test")
            .await
            .unwrap();
    assert!(
        repo::accounts::delete_account(&db, tenant_id, deleted_account)
            .await
            .unwrap()
    );

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
        tenant_id,
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
        tenant_id,
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
        tenant_id,
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
        repo::locations::set_location_deleted(&db, tenant_id, deleted_location, true)
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

    assert!(
        repo::warehouses::active_warehouse_exists(&db, tenant_id, warehouse)
            .await
            .unwrap()
    );
    assert!(
        !repo::warehouses::active_warehouse_exists(&db, tenant_id, deleted_warehouse)
            .await
            .unwrap()
    );
    assert!(
        repo::accounts::active_account_exists(&db, tenant_id, account)
            .await
            .unwrap()
    );
    assert!(
        !repo::accounts::active_account_exists(&db, tenant_id, deleted_account)
            .await
            .unwrap()
    );
    assert!(repo::items::active_item_exists(&db, item).await.unwrap());
    assert!(!repo::items::active_item_exists(&db, deleted_item)
        .await
        .unwrap());
    assert!(repo::loads::active_load_exists(&db, load).await.unwrap());
    assert!(!repo::loads::active_load_exists(&db, 999_999).await.unwrap());
    assert!(
        repo::locations::active_location_exists(&db, tenant_id, active_location)
            .await
            .unwrap()
    );
    assert_eq!(
        repo::locations::location_active_state(&db, tenant_id, active_location)
            .await
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        repo::locations::location_active_state(&db, tenant_id, inactive_location)
            .await
            .unwrap(),
        Some(false)
    );
    assert_eq!(
        repo::locations::location_active_state(&db, tenant_id, deleted_location)
            .await
            .unwrap(),
        None
    );
}
