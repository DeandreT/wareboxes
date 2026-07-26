mod common;

use common::*;

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
