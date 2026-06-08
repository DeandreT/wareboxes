mod common;

use common::*;

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
