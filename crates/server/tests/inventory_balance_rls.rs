mod common;

use common::*;

#[derive(Clone, Copy)]
struct BalanceRefs {
    tenant_id: TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
    source_location_id: i64,
    target_location_id: i64,
    item_batch_id: i64,
    item_id: i64,
}

#[tokio::test]
async fn inventory_balances_require_a_transaction_local_tenant_context() {
    let fixture = Fixture::new().await;
    let user_a = fixture.user("balance-rls-a@test.com").await;
    let user_b = fixture.user("balance-rls-b@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, user_a.id).await;
    let tenant_b = tenant_for_user(&fixture.db, user_b.id).await;
    let refs_a = balance_refs(&fixture, tenant_a, "Balance RLS A").await;
    let refs_b = balance_refs(&fixture, tenant_b, "Balance RLS B").await;

    let balance_a = receive_balance(&fixture, user_a.id, refs_a, 10, "balance-rls-a").await;
    let balance_b = receive_balance(&fixture, user_b.id, refs_b, 20, "balance-rls-b").await;
    let source_a = snapshot(&fixture.db, tenant_a, balance_a).await;
    let source_b = snapshot(&fixture.db, tenant_b, balance_b).await;

    let unbound_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inventory_balances")
        .fetch_one(&fixture.db)
        .await
        .unwrap();
    assert_eq!(unbound_count, 0);

    let unbound_updates =
        sqlx::query("UPDATE inventory_balances SET qty_on_hand = qty_on_hand + 1 WHERE id = $1")
            .bind(balance_a)
            .execute(&fixture.db)
            .await
            .unwrap()
            .rows_affected();
    assert_eq!(unbound_updates, 0);
    assert!(sqlx::query("DELETE FROM inventory_balances WHERE id = $1")
        .bind(balance_a)
        .execute(&fixture.db)
        .await
        .is_err());

    let mut unbound_tx = fixture.db.begin().await.unwrap();
    assert!(
        insert_balance(&mut unbound_tx, refs_a, refs_a.target_location_id, 5)
            .await
            .is_err()
    );
    unbound_tx.rollback().await.unwrap();
    let mut unbound_tx = fixture.db.begin().await.unwrap();
    assert!(upsert_balance(&mut unbound_tx, refs_a, 5).await.is_err());
    unbound_tx.rollback().await.unwrap();

    let mut tenant_a_tx = tenant_tx(&fixture.db, tenant_a).await;
    assert!(sqlx::query(
        "UPDATE inventory_balances SET qty_on_hand = qty_on_hand + 1 WHERE id = $1"
    )
    .bind(balance_a)
    .execute(&mut *tenant_a_tx)
    .await
    .is_err());
    tenant_a_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    let visible_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM inventory_balances ORDER BY id")
        .fetch_all(&mut *tenant_b_tx)
        .await
        .unwrap();
    assert_eq!(visible_ids, vec![balance_b]);
    let guessed_updates =
        sqlx::query("UPDATE inventory_balances SET qty_on_hand = 0 WHERE id = $1")
            .bind(balance_a)
            .execute(&mut *tenant_b_tx)
            .await
            .unwrap()
            .rows_affected();
    assert_eq!(guessed_updates, 0);
    assert!(sqlx::query("DELETE FROM inventory_balances WHERE id = $1")
        .bind(balance_a)
        .execute(&mut *tenant_b_tx)
        .await
        .is_err());
    tenant_b_tx.rollback().await.unwrap();
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(
        insert_balance(&mut tenant_b_tx, refs_a, refs_a.target_location_id, 5)
            .await
            .is_err()
    );
    tenant_b_tx.rollback().await.unwrap();
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(upsert_balance(&mut tenant_b_tx, refs_a, 5).await.is_err());
    tenant_b_tx.rollback().await.unwrap();

    let balances_a = repo::inventory::get_balances(&fixture.db, tenant_a, false)
        .await
        .unwrap();
    assert_eq!(balances_a.len(), 1);
    assert_eq!(balances_a[0].id, balance_a);
    assert_eq!(balances_a[0].qty_on_hand, 10);
    let balances_b = repo::inventory::get_balances(&fixture.db, tenant_b, false)
        .await
        .unwrap();
    assert_eq!(balances_b.len(), 1);
    assert_eq!(balances_b[0].id, balance_b);
    assert_eq!(balances_b[0].qty_on_hand, 20);

    assert!(repo::inventory::set_item_batch_deleted(
        &fixture.db,
        tenant_a,
        refs_a.item_batch_id,
        true
    )
    .await
    .is_err());
    assert_eq!(snapshot(&fixture.db, tenant_a, balance_a).await, source_a);
    assert_eq!(snapshot(&fixture.db, tenant_b, balance_b).await, source_b);
}

async fn receive_balance(
    fixture: &Fixture,
    user_id: i64,
    refs: BalanceRefs,
    qty: i64,
    idempotency_key: &str,
) -> i64 {
    repo::inventory::receive_inventory(
        &fixture.db,
        refs.tenant_id,
        user_id,
        refs.item_batch_id,
        refs.source_location_id,
        qty,
        None,
        Some("inventory balance RLS fixture"),
        None,
        None,
        idempotency_key,
    )
    .await
    .unwrap();

    let mut tx = tenant_tx(&fixture.db, refs.tenant_id).await;
    let balance_id = sqlx::query_scalar(
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
    .bind(refs.tenant_id.get())
    .bind(refs.inventory_owner_id)
    .bind(refs.source_location_id)
    .bind(refs.item_batch_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    balance_id
}

async fn balance_refs(fixture: &Fixture, tenant_id: TenantId, name: &str) -> BalanceRefs {
    let inventory_owner_id = fixture.inventory_owner(tenant_id, name).await;
    let facility_id = fixture.facility(tenant_id, name).await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let source_location_id = fixture
        .location(tenant_id, facility_id, &format!("{name} Source"))
        .await;
    let target_location_id = fixture
        .location(tenant_id, facility_id, &format!("{name} Target"))
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
    BalanceRefs {
        tenant_id,
        inventory_owner_id,
        facility_id,
        source_location_id,
        target_location_id,
        item_batch_id,
        item_id,
    }
}

async fn insert_balance(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    refs: BalanceRefs,
    location_id: i64,
    qty_on_hand: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        INSERT INTO inventory_balances
            (tenant_id, inventory_owner_id, created, modified, facility_id,
             location_id, item_batch_id, item_id, uom, status, qty_on_hand,
             qty_reserved)
        VALUES ($1, $2, $3, $3, $4, $5, $6, $7, 'each', 'available', $8, 0)
        RETURNING id
        "#,
    )
    .bind(refs.tenant_id.get())
    .bind(refs.inventory_owner_id)
    .bind(db::now_iso())
    .bind(refs.facility_id)
    .bind(location_id)
    .bind(refs.item_batch_id)
    .bind(refs.item_id)
    .bind(qty_on_hand)
    .fetch_one(&mut **tx)
    .await
}

async fn upsert_balance(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    refs: BalanceRefs,
    qty_on_hand: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO inventory_balances
            (tenant_id, inventory_owner_id, created, modified, facility_id,
             location_id, item_batch_id, item_id, uom, status, qty_on_hand,
             qty_reserved)
        VALUES ($1, $2, $3, $3, $4, $5, $6, $7, 'each', 'available', $8, 0)
        ON CONFLICT
            (tenant_id, inventory_owner_id, location_id, item_batch_id, uom, status)
            WHERE license_plate_id IS NULL
        DO UPDATE
        SET qty_on_hand = inventory_balances.qty_on_hand + excluded.qty_on_hand,
            modified = excluded.modified
        "#,
    )
    .bind(refs.tenant_id.get())
    .bind(refs.inventory_owner_id)
    .bind(db::now_iso())
    .bind(refs.facility_id)
    .bind(refs.source_location_id)
    .bind(refs.item_batch_id)
    .bind(refs.item_id)
    .bind(qty_on_hand)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

async fn snapshot(db: &db::Db, tenant_id: TenantId, balance_id: i64) -> String {
    let mut tx = tenant_tx(db, tenant_id).await;
    let row = sqlx::query_scalar(
        r#"
        SELECT row_to_json(balance_row)::TEXT
        FROM inventory_balances AS balance_row
        WHERE id = $1
        "#,
    )
    .bind(balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    row
}
