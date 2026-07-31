mod common;

use common::*;
use wareboxes_application::CommandContext;

#[derive(Clone, Copy)]
struct OrderRefs {
    tenant_id: TenantId,
    inventory_owner_id: i64,
    order_id: i64,
    order_item_id: i64,
    address_id: i64,
    item_id: i64,
}

#[tokio::test]
async fn order_aggregate_requires_a_transaction_local_tenant_context() {
    let fixture = Fixture::new().await;
    let user_a = fixture.user("order-rls-a@test.com").await;
    let user_b = fixture.user("order-rls-b@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, user_a.id).await;
    let tenant_b = tenant_for_user(&fixture.db, user_b.id).await;
    let refs_a = order_refs(&fixture, tenant_a, "Order RLS A").await;
    let refs_b = order_refs(&fixture, tenant_b, "Order RLS B").await;
    let source_a = snapshot(&fixture.db, refs_a).await;
    let source_b = snapshot(&fixture.db, refs_b).await;

    let unbound_counts: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM addresses),
               (SELECT COUNT(*) FROM orders),
               (SELECT COUNT(*) FROM order_items)
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(unbound_counts, (0, 0, 0));

    let mut unbound_connection = fixture.db.acquire().await.unwrap();
    assert_eq!(
        guessed_mutation_counts(&mut unbound_connection, refs_a).await,
        [0, 0, 0, 0, 0, 0]
    );
    drop(unbound_connection);

    let mut unbound_tx = fixture.db.begin().await.unwrap();
    assert!(insert_address(&mut unbound_tx, refs_a.tenant_id, "Unbound")
        .await
        .is_err());
    unbound_tx.rollback().await.unwrap();
    let mut unbound_tx = fixture.db.begin().await.unwrap();
    assert!(insert_order(
        &mut unbound_tx,
        refs_a.tenant_id,
        refs_a.inventory_owner_id,
        refs_a.address_id,
        "ORDER-RLS-UNBOUND"
    )
    .await
    .is_err());
    unbound_tx.rollback().await.unwrap();
    let mut unbound_tx = fixture.db.begin().await.unwrap();
    assert!(insert_order_item(&mut unbound_tx, refs_a, refs_a.order_id)
        .await
        .is_err());
    unbound_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    let visible_ids: (Vec<i64>, Vec<i64>, Vec<i64>) = sqlx::query_as(
        r#"
        SELECT ARRAY(SELECT id FROM addresses ORDER BY id),
               ARRAY(SELECT id FROM orders ORDER BY id),
               ARRAY(SELECT id FROM order_items ORDER BY id)
        "#,
    )
    .fetch_one(&mut *tenant_b_tx)
    .await
    .unwrap();
    assert_eq!(
        visible_ids,
        (
            vec![refs_b.address_id],
            vec![refs_b.order_id],
            vec![refs_b.order_item_id]
        )
    );
    assert_eq!(
        guessed_mutation_counts(&mut tenant_b_tx, refs_a).await,
        [0, 0, 0, 0, 0, 0]
    );
    tenant_b_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_address(&mut tenant_b_tx, tenant_a, "Forged")
        .await
        .is_err());
    tenant_b_tx.rollback().await.unwrap();
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_order(
        &mut tenant_b_tx,
        tenant_a,
        refs_a.inventory_owner_id,
        refs_a.address_id,
        "ORDER-RLS-FORGED"
    )
    .await
    .is_err());
    tenant_b_tx.rollback().await.unwrap();
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_order_item(&mut tenant_b_tx, refs_a, refs_a.order_id)
        .await
        .is_err());
    tenant_b_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_order(
        &mut tenant_b_tx,
        tenant_b,
        refs_b.inventory_owner_id,
        refs_a.address_id,
        "ORDER-RLS-GUESSED-ADDRESS"
    )
    .await
    .is_err());
    tenant_b_tx.rollback().await.unwrap();
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    let forged_line = OrderRefs {
        tenant_id: tenant_b,
        inventory_owner_id: refs_b.inventory_owner_id,
        item_id: refs_b.item_id,
        ..refs_a
    };
    assert!(
        insert_order_item(&mut tenant_b_tx, forged_line, refs_a.order_id)
            .await
            .is_err()
    );
    tenant_b_tx.rollback().await.unwrap();

    let tenant_b_address_count = address_count(&fixture.db, tenant_b).await;
    let guessed_update = OrderUpdate {
        order_id: refs_a.order_id,
        order_key: None,
        rush: None,
        ship_by: None,
        line1: Some("Must not be inserted".to_owned()),
        line2: None,
        city: None,
        state: None,
        postal_code: None,
        country: None,
    };
    let tenant_b_access = repo::tenants::access_for_user(&fixture.db, user_b.id, tenant_b)
        .await
        .unwrap()
        .unwrap();
    let command = CommandContext {
        tenant_id: tenant_b,
        actor_id: tenant_b_access.user_id,
        request_id: "order-rls-guessed-update".to_owned(),
        idempotency_key: Some("order-rls-guessed-update".to_owned()),
    };
    assert!(!repo::orders::update_order_metadata(
        &fixture.db,
        &tenant_b_access,
        &command,
        &guessed_update
    )
    .await
    .unwrap());
    assert_eq!(
        address_count(&fixture.db, tenant_b).await,
        tenant_b_address_count
    );

    assert_eq!(snapshot(&fixture.db, refs_a).await, source_a);
    assert_eq!(snapshot(&fixture.db, refs_b).await, source_b);

    let order_a = repo::orders::get_order(&fixture.db, tenant_a, refs_a.order_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(order_a.address_id, refs_a.address_id);
    assert_eq!(
        order_a
            .order_items
            .iter()
            .map(|line| line.id)
            .collect::<Vec<_>>(),
        vec![refs_a.order_item_id]
    );
}

async fn address_count(db: &db::Db, tenant_id: TenantId) -> i64 {
    let mut tx = tenant_tx(db, tenant_id).await;
    let count = sqlx::query_scalar("SELECT COUNT(*) FROM addresses")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    count
}

async fn order_refs(fixture: &Fixture, tenant_id: TenantId, name: &str) -> OrderRefs {
    let inventory_owner_id = fixture.inventory_owner(tenant_id, name).await;
    let item_id = fixture.item(tenant_id, name, "each").await;
    let order_id = fixture.order(tenant_id, name, inventory_owner_id).await;
    let order_item_id = fixture.order_item(tenant_id, order_id, item_id, 2).await;
    let address_id = repo::orders::get_order(&fixture.db, tenant_id, order_id)
        .await
        .unwrap()
        .unwrap()
        .address_id;
    OrderRefs {
        tenant_id,
        inventory_owner_id,
        order_id,
        order_item_id,
        address_id,
        item_id,
    }
}

async fn guessed_mutation_counts(connection: &mut sqlx::PgConnection, refs: OrderRefs) -> [u64; 6] {
    let address_updates = sqlx::query("UPDATE addresses SET line1 = line1 WHERE id = $1")
        .bind(refs.address_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    let order_updates = sqlx::query("UPDATE orders SET status = status WHERE id = $1")
        .bind(refs.order_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    let line_updates = sqlx::query("UPDATE order_items SET qty = qty WHERE id = $1")
        .bind(refs.order_item_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    let line_deletes = sqlx::query("DELETE FROM order_items WHERE id = $1")
        .bind(refs.order_item_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    let order_deletes = sqlx::query("DELETE FROM orders WHERE id = $1")
        .bind(refs.order_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    let address_deletes = sqlx::query("DELETE FROM addresses WHERE id = $1")
        .bind(refs.address_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    [
        address_updates,
        order_updates,
        line_updates,
        line_deletes,
        order_deletes,
        address_deletes,
    ]
}

async fn insert_address(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    line1: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        INSERT INTO addresses (tenant_id, created, line1, country)
        VALUES ($1, $2, $3, 'US')
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(db::now_iso())
    .bind(line1)
    .fetch_one(&mut **tx)
    .await
}

async fn insert_order(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    address_id: i64,
    order_key: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        INSERT INTO orders
            (tenant_id, inventory_owner_id, order_key, created, address_id)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(order_key)
    .bind(db::now_iso())
    .bind(address_id)
    .fetch_one(&mut **tx)
    .await
}

async fn insert_order_item(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    refs: OrderRefs,
    order_id: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        INSERT INTO order_items
            (tenant_id, inventory_owner_id, created, qty, item_id, order_id)
        VALUES ($1, $2, $3, 1, $4, $5)
        RETURNING id
        "#,
    )
    .bind(refs.tenant_id.get())
    .bind(refs.inventory_owner_id)
    .bind(db::now_iso())
    .bind(refs.item_id)
    .bind(order_id)
    .fetch_one(&mut **tx)
    .await
}

async fn snapshot(db: &db::Db, refs: OrderRefs) -> String {
    let mut tx = tenant_tx(db, refs.tenant_id).await;
    let row = sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
            'address', (
                SELECT to_jsonb(address_row)
                FROM addresses address_row
                WHERE id = $1
            ),
            'order', (
                SELECT to_jsonb(order_row)
                FROM orders order_row
                WHERE id = $2
            ),
            'order_item', (
                SELECT to_jsonb(order_item_row)
                FROM order_items order_item_row
                WHERE id = $3
            )
        )::TEXT
        "#,
    )
    .bind(refs.address_id)
    .bind(refs.order_id)
    .bind(refs.order_item_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    row
}
