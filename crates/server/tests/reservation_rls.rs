mod common;

use common::*;

#[derive(Clone, Copy)]
struct ReservationRefs {
    tenant_id: TenantId,
    inventory_owner_id: i64,
    order_id: i64,
    inventory_balance_id: i64,
    facility_id: i64,
    item_batch_id: i64,
    location_id: i64,
}

#[tokio::test]
async fn inventory_reservations_require_a_transaction_local_tenant_context() {
    let fixture = Fixture::new().await;
    let user_a = fixture.user("reservation-rls-a@test.com").await;
    let user_b = fixture.user("reservation-rls-b@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, user_a.id).await;
    let tenant_b = tenant_for_user(&fixture.db, user_b.id).await;

    let refs_a = reservation_refs(&fixture, tenant_a, "Reservation RLS A").await;
    let refs_b = reservation_refs(&fixture, tenant_b, "Reservation RLS B").await;

    let mut tenant_a_tx = tenant_tx(&fixture.db, tenant_a).await;
    let reservation_id = insert_reservation(&mut tenant_a_tx, refs_a).await.unwrap();
    tenant_a_tx.commit().await.unwrap();
    let source_row = snapshot(&fixture.db, tenant_a, reservation_id).await;

    let unbound_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inventory_reservations")
        .fetch_one(&fixture.db)
        .await
        .unwrap();
    assert_eq!(unbound_count, 0);

    let mut unbound_tx = fixture.db.begin().await.unwrap();
    assert!(insert_reservation(&mut unbound_tx, refs_a).await.is_err());
    unbound_tx.rollback().await.unwrap();

    let unbound_updates = sqlx::query("UPDATE inventory_reservations SET qty = qty WHERE id = $1")
        .bind(reservation_id)
        .execute(&fixture.db)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(unbound_updates, 0);
    let unbound_deletes = sqlx::query("DELETE FROM inventory_reservations WHERE id = $1")
        .bind(reservation_id)
        .execute(&fixture.db)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(unbound_deletes, 0);

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    let guessed_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM inventory_reservations
        WHERE id = $1 OR inventory_balance_id = $2 OR order_id = $3
        "#,
    )
    .bind(reservation_id)
    .bind(refs_a.inventory_balance_id)
    .bind(refs_a.order_id)
    .fetch_one(&mut *tenant_b_tx)
    .await
    .unwrap();
    assert_eq!(guessed_count, 0);
    let guessed_updates = sqlx::query("UPDATE inventory_reservations SET qty = qty WHERE id = $1")
        .bind(reservation_id)
        .execute(&mut *tenant_b_tx)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(guessed_updates, 0);
    let guessed_deletes = sqlx::query("DELETE FROM inventory_reservations WHERE id = $1")
        .bind(reservation_id)
        .execute(&mut *tenant_b_tx)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(guessed_deletes, 0);
    tenant_b_tx.rollback().await.unwrap();

    let mut guessed_balance_refs = refs_b;
    guessed_balance_refs.inventory_balance_id = refs_a.inventory_balance_id;
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_reservation(&mut tenant_b_tx, guessed_balance_refs)
        .await
        .is_err());
    tenant_b_tx.rollback().await.unwrap();

    let mut guessed_order_refs = refs_b;
    guessed_order_refs.order_id = refs_a.order_id;
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_reservation(&mut tenant_b_tx, guessed_order_refs)
        .await
        .is_err());
    tenant_b_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_reservation(&mut tenant_b_tx, refs_a).await.is_err());
    tenant_b_tx.rollback().await.unwrap();

    assert_eq!(
        snapshot(&fixture.db, tenant_a, reservation_id).await,
        source_row
    );
}

async fn reservation_refs(fixture: &Fixture, tenant_id: TenantId, name: &str) -> ReservationRefs {
    let facility_id = fixture.facility(tenant_id, name).await;
    let location_id = fixture
        .location(tenant_id, facility_id, &format!("{name} Location"))
        .await;
    let item_id = fixture.item(tenant_id, name, "each").await;
    let inventory_owner_id = fixture.inventory_owner(tenant_id, name).await;
    let item_batch_id = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        inventory_owner_id,
        item_id,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let order_id = fixture
        .order(tenant_id, &format!("{name} Order"), inventory_owner_id)
        .await;

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let inventory_balance_id = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_balances
            (tenant_id, inventory_owner_id, created, modified, facility_id, location_id,
             license_plate_id, item_batch_id, item_id, uom, status, qty_on_hand, qty_reserved)
        VALUES ($1, $2, $3, $3, $4, $5, NULL, $6, $7, 'each', 'available', 10, 0)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(db::now_iso())
    .bind(facility_id)
    .bind(location_id)
    .bind(item_batch_id)
    .bind(item_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    ReservationRefs {
        tenant_id,
        inventory_owner_id,
        order_id,
        inventory_balance_id,
        facility_id,
        item_batch_id,
        location_id,
    }
}

async fn insert_reservation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    refs: ReservationRefs,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        INSERT INTO inventory_reservations
            (tenant_id, inventory_owner_id, created, modified, order_id, order_item_id,
             inventory_balance_id, facility_id, item_batch_id, location_id, qty, status)
        VALUES ($1, $2, $3, $3, $4, NULL, $5, $6, $7, $8, 1, 'reserved')
        RETURNING id
        "#,
    )
    .bind(refs.tenant_id.get())
    .bind(refs.inventory_owner_id)
    .bind(db::now_iso())
    .bind(refs.order_id)
    .bind(refs.inventory_balance_id)
    .bind(refs.facility_id)
    .bind(refs.item_batch_id)
    .bind(refs.location_id)
    .fetch_one(&mut **tx)
    .await
}

async fn snapshot(db: &db::Db, tenant_id: TenantId, reservation_id: i64) -> String {
    let mut tx = tenant_tx(db, tenant_id).await;
    let row = sqlx::query_scalar(
        r#"
        SELECT row_to_json(reservation_row)::TEXT
        FROM inventory_reservations AS reservation_row
        WHERE id = $1
        "#,
    )
    .bind(reservation_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    row
}
