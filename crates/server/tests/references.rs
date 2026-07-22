mod common;

use common::*;

#[tokio::test]
async fn facilities_listing_filters_deleted() {
    let db = setup().await;
    let user = auth::register_user(&db, "facilities@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let tenant_id = tenant_for_user(&db, user.id).await;

    let keep = repo::facilities::add_facility(&db, tenant_id, "Main DC")
        .await
        .unwrap();
    let gone = repo::facilities::add_facility(&db, tenant_id, "Old DC")
        .await
        .unwrap();
    sqlx::query("UPDATE facilities SET deleted = $1 WHERE id = $2")
        .bind(db::now_iso())
        .bind(gone)
        .execute(&db)
        .await
        .unwrap();

    let active = repo::facilities::get_facilities(&db, tenant_id, false)
        .await
        .unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, keep);

    let all = repo::facilities::get_facilities(&db, tenant_id, true)
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
    let facility = repo::facilities::add_facility(&db, tenant_id, "Selector DC")
        .await
        .unwrap();
    let deleted_facility = repo::facilities::add_facility(&db, tenant_id, "Deleted DC")
        .await
        .unwrap();
    sqlx::query("UPDATE facilities SET deleted = $1 WHERE id = $2")
        .bind(db::now_iso())
        .bind(deleted_facility)
        .execute(&db)
        .await
        .unwrap();

    let inventory_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "Selector InventoryOwner",
        "ops@selector.test",
    )
    .await
    .unwrap();
    let deleted_inventory_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "Deleted InventoryOwner",
        "gone@test",
    )
    .await
    .unwrap();
    assert!(repo::inventory_owners::delete_inventory_owner(
        &db,
        tenant_id,
        deleted_inventory_owner
    )
    .await
    .unwrap());

    let item = repo::items::add_item(
        &db,
        tenant_id,
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
        tenant_id,
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
    assert!(
        repo::items::set_item_deleted(&db, tenant_id, deleted_item, true)
            .await
            .unwrap()
    );

    let active_location = repo::locations::add_location(
        &db,
        tenant_id,
        facility,
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
        facility,
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
        facility,
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
        facility,
        inventory_owner,
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
        repo::facilities::active_facility_exists(&db, tenant_id, facility)
            .await
            .unwrap()
    );
    assert!(
        !repo::facilities::active_facility_exists(&db, tenant_id, deleted_facility)
            .await
            .unwrap()
    );
    assert!(
        repo::inventory_owners::active_inventory_owner_exists(&db, tenant_id, inventory_owner)
            .await
            .unwrap()
    );
    assert!(!repo::inventory_owners::active_inventory_owner_exists(
        &db,
        tenant_id,
        deleted_inventory_owner
    )
    .await
    .unwrap());
    assert!(repo::items::active_item_exists(&db, tenant_id, item)
        .await
        .unwrap());
    assert!(
        !repo::items::active_item_exists(&db, tenant_id, deleted_item)
            .await
            .unwrap()
    );
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
