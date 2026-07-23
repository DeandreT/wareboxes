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
    let dims_a: i64 =
        sqlx::query_scalar("SELECT dims_id FROM items WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_a.get())
            .bind(item_a)
            .fetch_one(&fixture.db)
            .await
            .unwrap();
    let dims_b: i64 =
        sqlx::query_scalar("SELECT dims_id FROM items WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_b.get())
            .bind(item_b)
            .fetch_one(&fixture.db)
            .await
            .unwrap();
    let dimension_tenants: Vec<i64> =
        sqlx::query_scalar("SELECT tenant_id FROM dims WHERE id = ANY($1) ORDER BY tenant_id")
            .bind(vec![dims_a, dims_b])
            .fetch_all(&fixture.db)
            .await
            .unwrap();
    assert_eq!(dimension_tenants, vec![tenant_a.get(), tenant_b.get()]);

    assert!(
        sqlx::query("UPDATE items SET dims_id = $1 WHERE tenant_id = $2 AND id = $3")
            .bind(dims_b)
            .bind(tenant_a.get())
            .bind(item_a)
            .execute(&fixture.db)
            .await
            .is_err()
    );

    let owner_a = fixture.inventory_owner(tenant_a, "Dimension Owner A").await;
    let facility_a = fixture.facility(tenant_a, "Dimension Facility A").await;
    sqlx::query(
        "INSERT INTO inventory_owner_facilities (tenant_id, created, inventory_owner_id, facility_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(tenant_a.get())
    .bind(db::now_iso())
    .bind(owner_a)
    .bind(facility_a)
    .execute(&fixture.db)
    .await
    .unwrap();
    let plate_a = repo::license_plates::add_license_plate(
        &fixture.db,
        tenant_a,
        owner_a,
        facility_a,
        Some("DIMENSION-LPN-A"),
    )
    .await
    .unwrap();
    assert!(
        sqlx::query("UPDATE license_plates SET dims_id = $1 WHERE tenant_id = $2 AND id = $3",)
            .bind(dims_b)
            .bind(tenant_a.get())
            .bind(plate_a)
            .execute(&fixture.db)
            .await
            .is_err()
    );
    sqlx::query("UPDATE license_plates SET dims_id = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(dims_a)
        .bind(tenant_a.get())
        .bind(plate_a)
        .execute(&fixture.db)
        .await
        .unwrap();

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

    let dimensions_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dims")
        .fetch_one(&fixture.db)
        .await
        .unwrap();
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
    .execute(&fixture.db)
    .await
    .unwrap();
    sqlx::query(
        r#"
        CREATE TRIGGER reject_dimension_test_item
            BEFORE INSERT ON items
            FOR EACH ROW EXECUTE FUNCTION reject_dimension_test_item()
        "#,
    )
    .execute(&fixture.db)
    .await
    .unwrap();
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
    let dimensions_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dims")
        .fetch_one(&fixture.db)
        .await
        .unwrap();
    assert_eq!(dimensions_after, dimensions_before);
}
