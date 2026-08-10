mod common;

use common::*;
use wareboxes_domain::{InventoryOwnerId, OwnerScope};

#[tokio::test]
async fn active_barcode_scanner_identity_is_unique_per_tenant() {
    let db = setup().await;
    let user = auth::register_user(&db, "items@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let tenant_id = tenant_for_user(&db, user.id).await;

    let item_one = repo::items::add_item(
        &db,
        tenant_id,
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
        tenant_id,
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

    let blank = repo::items::add_barcode(&db, tenant_id, item_one, "   ", "code128", None)
        .await
        .unwrap_err();
    assert!(matches!(
        blank,
        AppError::Db(sqlx::Error::Database(ref err))
            if err.kind() == sqlx::error::ErrorKind::CheckViolation
    ));

    let value = "036000291452";
    let code128 = repo::items::add_barcode(&db, tenant_id, item_one, value, "code128", None)
        .await
        .unwrap();
    let same_item_different_type =
        repo::items::add_barcode(&db, tenant_id, item_one, value, "upc-a", None)
            .await
            .unwrap_err();
    assert!(matches!(
        same_item_different_type,
        AppError::Db(sqlx::Error::Database(ref err))
            if err.kind() == sqlx::error::ErrorKind::UniqueViolation
    ));

    let other_item_different_type =
        repo::items::add_barcode(&db, tenant_id, item_two, value, "qr", None)
            .await
            .unwrap_err();
    assert!(matches!(
        other_item_different_type,
        AppError::Db(sqlx::Error::Database(ref err))
            if err.kind() == sqlx::error::ErrorKind::UniqueViolation
    ));

    assert!(
        repo::items::set_barcode_deleted(&db, tenant_id, code128, true)
            .await
            .unwrap()
    );
    assert!(
        repo::items::add_barcode(&db, tenant_id, item_two, value, "qr", None)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn pack_conversions_reject_conflicting_quantities_and_cycles() {
    let db = setup().await;
    let user = auth::register_user(&db, "packs@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let tenant_id = tenant_for_user(&db, user.id).await;
    let mut item_ids = Vec::new();
    for description in ["Master case", "Inner pack", "Single unit"] {
        item_ids.push(
            repo::items::add_item(
                &db,
                tenant_id,
                description,
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
            .unwrap(),
        );
    }

    repo::items::add_item_pack_link(&db, tenant_id, item_ids[0], item_ids[1], 4, None)
        .await
        .unwrap();
    let duplicate =
        repo::items::add_item_pack_link(&db, tenant_id, item_ids[0], item_ids[1], 6, None)
            .await
            .unwrap_err();
    assert!(matches!(
        duplicate,
        AppError::Application(ApplicationError::Conflict(_))
    ));

    repo::items::add_item_pack_link(&db, tenant_id, item_ids[1], item_ids[2], 3, None)
        .await
        .unwrap();
    let cycle = repo::items::add_item_pack_link(&db, tenant_id, item_ids[2], item_ids[0], 2, None)
        .await
        .unwrap_err();
    assert!(matches!(
        cycle,
        AppError::Application(ApplicationError::Conflict(_))
    ));
}

#[tokio::test]
async fn item_client_eligibility_is_scoped_reactivatable_and_protected() {
    let db = setup().await;
    let user = auth::register_user(&db, "owner-items@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let tenant_id = tenant_for_user(&db, user.id).await;
    let first_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "First client",
        "first-client@test.com",
    )
    .await
    .unwrap();
    let second_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "Second client",
        "second-client@test.com",
    )
    .await
    .unwrap();
    let item_id = repo::items::add_item(
        &db,
        tenant_id,
        "Owner-scoped item",
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

    let assignment = repo::items::add_inventory_owner_item(&db, tenant_id, first_owner, item_id)
        .await
        .unwrap();
    let duplicate = repo::items::add_inventory_owner_item(&db, tenant_id, first_owner, item_id)
        .await
        .unwrap_err();
    assert!(matches!(
        duplicate,
        AppError::Application(ApplicationError::Conflict(_))
    ));

    let all_owners = OwnerScope {
        all_inventory_owners: true,
        inventory_owner_ids: Vec::new(),
    };
    assert_eq!(
        repo::items::get_inventory_owner_items_in_scope(&db, tenant_id, &all_owners, false,)
            .await
            .unwrap(),
        vec![assignment.clone()]
    );
    let second_owner_only = OwnerScope {
        all_inventory_owners: false,
        inventory_owner_ids: vec![InventoryOwnerId::new(second_owner).unwrap()],
    };
    assert!(repo::items::get_inventory_owner_items_in_scope(
        &db,
        tenant_id,
        &second_owner_only,
        false,
    )
    .await
    .unwrap()
    .is_empty());

    assert!(repo::items::deactivate_inventory_owner_item_in_scope(
        &db,
        tenant_id,
        &all_owners,
        assignment.id,
    )
    .await
    .unwrap());
    let reactivated = repo::items::add_inventory_owner_item(&db, tenant_id, first_owner, item_id)
        .await
        .unwrap();
    assert_eq!(reactivated.id, assignment.id);

    let order_id =
        insert_test_order_header(&db, tenant_id, "owner-item-active-order", first_owner).await;
    let mut tx = tenant_tx(&db, tenant_id).await;
    sqlx::query(
        r#"
        INSERT INTO order_items
            (tenant_id, inventory_owner_id, created, line_key, line_number,
             qty, item_id, order_id, uom)
        VALUES ($1,$2,$3,'line-1',1,1,$4,$5,'each')
        "#,
    )
    .bind(tenant_id.get())
    .bind(first_owner)
    .bind(db::now_iso())
    .bind(item_id)
    .bind(order_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let active_demand = repo::items::deactivate_inventory_owner_item_in_scope(
        &db,
        tenant_id,
        &all_owners,
        assignment.id,
    )
    .await
    .unwrap_err();
    assert!(matches!(
        active_demand,
        AppError::Db(sqlx::Error::Database(ref error))
            if error.code().as_deref() == Some("55000")
    ));
}
