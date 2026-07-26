mod common;

use common::*;

#[tokio::test]
async fn dimensions_are_tenant_scoped_and_item_creation_is_atomic() {
    let fixture = Fixture::new().await;
    let user_a = fixture.user("dimension-tenant-a@test.com").await;
    let user_b = fixture.user("dimension-tenant-b@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, user_a.id).await;
    let tenant_b = tenant_for_user(&fixture.db, user_b.id).await;
    let item_a = fixture
        .item(tenant_a, "Tenant A Dimension Item", "each")
        .await;
    let item_b = fixture
        .item(tenant_b, "Tenant B Dimension Item", "each")
        .await;
    let mut tx = tenant_tx(&fixture.db, tenant_a).await;
    let dims_a: i64 =
        sqlx::query_scalar("SELECT dims_id FROM items WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_a.get())
            .bind(item_a)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let dimension_tenant_a: i64 = sqlx::query_scalar("SELECT tenant_id FROM dims WHERE id = $1")
        .bind(dims_a)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_b).await;
    let dims_b: i64 =
        sqlx::query_scalar("SELECT dims_id FROM items WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_b.get())
            .bind(item_b)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let dimension_tenant_b: i64 = sqlx::query_scalar("SELECT tenant_id FROM dims WHERE id = $1")
        .bind(dims_b)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(dimension_tenant_a, tenant_a.get());
    assert_eq!(dimension_tenant_b, tenant_b.get());

    let mut tx = tenant_tx(&fixture.db, tenant_a).await;
    assert!(
        sqlx::query("UPDATE items SET dims_id = $1 WHERE tenant_id = $2 AND id = $3")
            .bind(dims_b)
            .bind(tenant_a.get())
            .bind(item_a)
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();

    let owner_a = fixture.inventory_owner(tenant_a, "Dimension Owner A").await;
    let facility_a = fixture.facility(tenant_a, "Dimension Facility A").await;
    fixture
        .assign_owner_to_facility(tenant_a, owner_a, facility_a)
        .await;
    let plate_a = repo::license_plates::add_license_plate(
        &fixture.db,
        tenant_a,
        owner_a,
        facility_a,
        Some("DIMENSION-LPN-A"),
    )
    .await
    .unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_a).await;
    assert!(
        sqlx::query("UPDATE license_plates SET dims_id = $1 WHERE tenant_id = $2 AND id = $3",)
            .bind(dims_b)
            .bind(tenant_a.get())
            .bind(plate_a)
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_a).await;
    sqlx::query("UPDATE license_plates SET dims_id = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(dims_a)
        .bind(tenant_a.get())
        .bind(plate_a)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let catalog: (bool, i64, i64) = sqlx::query_as(
        r#"
        SELECT attribute.attnotnull,
               (SELECT COUNT(*)
                FROM pg_constraint
                WHERE conrelid = 'items'::regclass
                  AND contype = 'f'
                  AND pg_get_constraintdef(oid) =
                      'FOREIGN KEY (tenant_id, dims_id) REFERENCES dims(tenant_id, id)'),
               (SELECT COUNT(*)
                FROM pg_constraint
                WHERE conrelid = 'license_plates'::regclass
                  AND contype = 'f'
                  AND pg_get_constraintdef(oid) =
                      'FOREIGN KEY (tenant_id, dims_id) REFERENCES dims(tenant_id, id)')
        FROM pg_attribute attribute
        WHERE attribute.attrelid = 'dims'::regclass
          AND attribute.attname = 'tenant_id'
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(catalog, (true, 1, 1));

    let mut tx = tenant_tx(&fixture.db, tenant_a).await;
    let dimensions_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dims")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let admin_db = admin_db_for(&fixture.db).await;
    sqlx::query(
        r#"
        CREATE FUNCTION reject_dimension_test_item() RETURNS trigger AS $$
        BEGIN
            IF NEW.description = 'Rejected dimension test item' THEN
                RAISE EXCEPTION 'reject test item';
            END IF;
            RETURN NEW;
        END;
        $$ LANGUAGE plpgsql;
        "#,
    )
    .execute(&admin_db)
    .await
    .unwrap();
    sqlx::query(
        r#"
        CREATE TRIGGER reject_dimension_test_item
            BEFORE INSERT ON items
            FOR EACH ROW EXECUTE FUNCTION reject_dimension_test_item()
        "#,
    )
    .execute(&admin_db)
    .await
    .unwrap();
    admin_db.close().await;
    assert!(repo::items::add_item(
        &fixture.db,
        tenant_a,
        "Rejected dimension test item",
        None,
        "each",
        Some(1),
        Some(1),
        Some(1),
        Some("in"),
        Some(1),
        Some("lb"),
    )
    .await
    .is_err());
    let mut tx = tenant_tx(&fixture.db, tenant_a).await;
    let dimensions_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dims")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(dimensions_after, dimensions_before);
}
