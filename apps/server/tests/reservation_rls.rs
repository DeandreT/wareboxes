mod common;

use common::*;

#[derive(Clone, Copy)]
struct ReservationRefs {
    tenant_id: TenantId,
    user_id: i64,
    inventory_owner_id: i64,
    order_id: i64,
    order_item_id: i64,
    inventory_balance_id: i64,
    facility_id: i64,
    item_batch_id: i64,
    item_id: i64,
    location_id: i64,
}

#[tokio::test]
async fn reservations_and_allocations_require_transaction_local_tenant_context() {
    let fixture = Fixture::new().await;
    let user_a = fixture.user("reservation-rls-a@test.com").await;
    let user_b = fixture.user("reservation-rls-b@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, user_a.id).await;
    let tenant_b = tenant_for_user(&fixture.db, user_b.id).await;
    let refs_a = reservation_refs(&fixture, tenant_a, user_a.id, "Reservation RLS A").await;
    let refs_b = reservation_refs(&fixture, tenant_b, user_b.id, "Reservation RLS B").await;

    let mut tenant_a_tx = tenant_tx(&fixture.db, tenant_a).await;
    let reservation_id = insert_reservation(&mut tenant_a_tx, refs_a).await.unwrap();
    let allocation_id = insert_allocation(&mut tenant_a_tx, refs_a, reservation_id)
        .await
        .unwrap();
    tenant_a_tx.commit().await.unwrap();
    let source_snapshot = snapshot(&fixture.db, tenant_a, reservation_id, allocation_id).await;

    let unbound_visibility: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM inventory_reservations), (SELECT COUNT(*) FROM inventory_allocations)",
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(unbound_visibility, (0, 0));
    let unbound_updates = sqlx::query("UPDATE inventory_allocations SET qty = qty WHERE id = $1")
        .bind(allocation_id)
        .execute(&fixture.db)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(unbound_updates, 0);

    let mut unbound_tx = fixture.db.begin().await.unwrap();
    assert!(insert_reservation(&mut unbound_tx, refs_a).await.is_err());
    unbound_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    let guessed_visibility: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT COUNT(*) FROM inventory_reservations WHERE id = $1 OR order_id = $2),
            (SELECT COUNT(*) FROM inventory_allocations
             WHERE id = $3 OR inventory_balance_id = $4)
        "#,
    )
    .bind(reservation_id)
    .bind(refs_a.order_id)
    .bind(allocation_id)
    .bind(refs_a.inventory_balance_id)
    .fetch_one(&mut *tenant_b_tx)
    .await
    .unwrap();
    assert_eq!(guessed_visibility, (0, 0));
    assert_eq!(
        sqlx::query("UPDATE inventory_reservations SET qty = qty WHERE id = $1")
            .bind(reservation_id)
            .execute(&mut *tenant_b_tx)
            .await
            .unwrap()
            .rows_affected(),
        0
    );
    assert_eq!(
        sqlx::query("UPDATE inventory_allocations SET qty = qty WHERE id = $1")
            .bind(allocation_id)
            .execute(&mut *tenant_b_tx)
            .await
            .unwrap()
            .rows_affected(),
        0
    );
    tenant_b_tx.rollback().await.unwrap();

    let mut guessed_refs = refs_b;
    guessed_refs.order_id = refs_a.order_id;
    guessed_refs.order_item_id = refs_a.order_item_id;
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_reservation(&mut tenant_b_tx, guessed_refs)
        .await
        .is_err());
    tenant_b_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_allocation(&mut tenant_b_tx, refs_a, reservation_id)
        .await
        .is_err());
    tenant_b_tx.rollback().await.unwrap();
    assert_eq!(
        snapshot(&fixture.db, tenant_a, reservation_id, allocation_id).await,
        source_snapshot
    );
}

async fn reservation_refs(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    name: &str,
) -> ReservationRefs {
    let facility_id = fixture.facility(tenant_id, name).await;
    let inventory_owner_id = fixture.inventory_owner(tenant_id, name).await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let location_id = fixture
        .location(tenant_id, facility_id, &format!("{name} Location"))
        .await;
    let item_id = fixture.item(tenant_id, name, "each").await;
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
        .order_header(tenant_id, &format!("{name} Order"), inventory_owner_id)
        .await;
    let order_item_id = fixture.order_item(tenant_id, order_id, item_id, 10).await;
    repo::inventory::receive_inventory(
        &fixture.db,
        tenant_id,
        user_id,
        item_batch_id,
        location_id,
        10,
        None,
        Some("reservation RLS fixture"),
        None,
        None,
        &format!("reservation-rls-receipt-{tenant_id}"),
    )
    .await
    .unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let inventory_balance_id = sqlx::query_scalar(
        r#"
        SELECT id
        FROM inventory_balances
        WHERE tenant_id = $1
          AND inventory_owner_id = $2
          AND location_id = $3
          AND item_batch_id = $4
          AND deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(location_id)
    .bind(item_batch_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    ReservationRefs {
        tenant_id,
        user_id,
        inventory_owner_id,
        order_id,
        order_item_id,
        inventory_balance_id,
        facility_id,
        item_batch_id,
        item_id,
        location_id,
    }
}

async fn insert_reservation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    refs: ReservationRefs,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        INSERT INTO inventory_reservations (
            tenant_id, inventory_owner_id, created, modified, created_by,
            order_id, order_item_id, facility_id, item_id, uom, qty, status
        )
        VALUES ($1, $2, $3, $3, $4, $5, $6, $7, $8, 'each', 1, 'active')
        RETURNING id
        "#,
    )
    .bind(refs.tenant_id.get())
    .bind(refs.inventory_owner_id)
    .bind(db::now_iso())
    .bind(refs.user_id)
    .bind(refs.order_id)
    .bind(refs.order_item_id)
    .bind(refs.facility_id)
    .bind(refs.item_id)
    .fetch_one(&mut **tx)
    .await
}

async fn insert_allocation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    refs: ReservationRefs,
    reservation_id: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        INSERT INTO inventory_allocations (
            tenant_id, inventory_owner_id, created, modified, created_by,
            reservation_id, inventory_balance_id, facility_id, location_id,
            item_batch_id, item_id, uom, inventory_status, qty, status
        )
        VALUES (
            $1, $2, $3, $3, $4, $5, $6, $7, $8, $9, $10,
            'each', 'available', 1, 'allocated'
        )
        RETURNING id
        "#,
    )
    .bind(refs.tenant_id.get())
    .bind(refs.inventory_owner_id)
    .bind(db::now_iso())
    .bind(refs.user_id)
    .bind(reservation_id)
    .bind(refs.inventory_balance_id)
    .bind(refs.facility_id)
    .bind(refs.location_id)
    .bind(refs.item_batch_id)
    .bind(refs.item_id)
    .fetch_one(&mut **tx)
    .await
}

async fn snapshot(
    db: &db::Db,
    tenant_id: TenantId,
    reservation_id: i64,
    allocation_id: i64,
) -> (String, String) {
    let mut tx = tenant_tx(db, tenant_id).await;
    let snapshot = sqlx::query_as(
        r#"
        SELECT
            (SELECT row_to_json(row)::TEXT FROM inventory_reservations row WHERE id = $1),
            (SELECT row_to_json(row)::TEXT FROM inventory_allocations row WHERE id = $2)
        "#,
    )
    .bind(reservation_id)
    .bind(allocation_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    snapshot
}
