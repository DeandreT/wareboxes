mod common;

use common::*;

#[derive(Clone, Copy)]
struct TrackingRefs {
    tenant_id: TenantId,
    inventory_owner_id: i64,
    order_id: i64,
}

#[tokio::test]
async fn order_tracking_numbers_require_a_transaction_local_tenant_context() {
    let fixture = Fixture::new().await;
    let user_a = fixture.user("tracking-rls-a@test.com").await;
    let user_b = fixture.user("tracking-rls-b@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, user_a.id).await;
    let tenant_b = tenant_for_user(&fixture.db, user_b.id).await;
    let refs_a = tracking_refs(&fixture, tenant_a, "Tracking RLS A").await;
    let refs_b = tracking_refs(&fixture, tenant_b, "Tracking RLS B").await;

    let mut tenant_a_tx = tenant_tx(&fixture.db, tenant_a).await;
    let tracking_id = insert_tracking(&mut tenant_a_tx, refs_a, "TRACK-A-SOURCE")
        .await
        .unwrap();
    tenant_a_tx.commit().await.unwrap();
    let source_row = snapshot(&fixture.db, tenant_a, tracking_id).await;

    let unbound_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM order_tracking_numbers")
        .fetch_one(&fixture.db)
        .await
        .unwrap();
    assert_eq!(unbound_count, 0);

    let mut unbound_tx = fixture.db.begin().await.unwrap();
    assert!(insert_tracking(&mut unbound_tx, refs_a, "TRACK-A-UNBOUND")
        .await
        .is_err());
    unbound_tx.rollback().await.unwrap();

    let unbound_updates =
        sqlx::query("UPDATE order_tracking_numbers SET carrier = carrier WHERE id = $1")
            .bind(tracking_id)
            .execute(&fixture.db)
            .await
            .unwrap()
            .rows_affected();
    assert_eq!(unbound_updates, 0);
    let unbound_deletes = sqlx::query("DELETE FROM order_tracking_numbers WHERE id = $1")
        .bind(tracking_id)
        .execute(&fixture.db)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(unbound_deletes, 0);

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    let guessed_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM order_tracking_numbers
        WHERE id = $1 OR order_id = $2
        "#,
    )
    .bind(tracking_id)
    .bind(refs_a.order_id)
    .fetch_one(&mut *tenant_b_tx)
    .await
    .unwrap();
    assert_eq!(guessed_count, 0);
    let guessed_updates =
        sqlx::query("UPDATE order_tracking_numbers SET carrier = carrier WHERE id = $1")
            .bind(tracking_id)
            .execute(&mut *tenant_b_tx)
            .await
            .unwrap()
            .rows_affected();
    assert_eq!(guessed_updates, 0);
    let guessed_deletes = sqlx::query("DELETE FROM order_tracking_numbers WHERE id = $1")
        .bind(tracking_id)
        .execute(&mut *tenant_b_tx)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(guessed_deletes, 0);
    tenant_b_tx.rollback().await.unwrap();

    let guessed_order_refs = TrackingRefs {
        order_id: refs_a.order_id,
        ..refs_b
    };
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_tracking(
        &mut tenant_b_tx,
        guessed_order_refs,
        "TRACK-B-GUESSED-ORDER"
    )
    .await
    .is_err());
    tenant_b_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_tracking(&mut tenant_b_tx, refs_a, "TRACK-A-BOUND-B")
        .await
        .is_err());
    tenant_b_tx.rollback().await.unwrap();

    assert_eq!(
        snapshot(&fixture.db, tenant_a, tracking_id).await,
        source_row
    );
}

async fn tracking_refs(fixture: &Fixture, tenant_id: TenantId, name: &str) -> TrackingRefs {
    let inventory_owner_id = fixture.inventory_owner(tenant_id, name).await;
    let order_id = fixture.order(tenant_id, name, inventory_owner_id).await;
    TrackingRefs {
        tenant_id,
        inventory_owner_id,
        order_id,
    }
}

async fn insert_tracking(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    refs: TrackingRefs,
    tracking_number: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        INSERT INTO order_tracking_numbers
            (tenant_id, inventory_owner_id, created, order_id, tracking_number, carrier, service)
        VALUES ($1, $2, $3, $4, $5, 'Test Carrier', 'Ground')
        RETURNING id
        "#,
    )
    .bind(refs.tenant_id.get())
    .bind(refs.inventory_owner_id)
    .bind(db::now_iso())
    .bind(refs.order_id)
    .bind(tracking_number)
    .fetch_one(&mut **tx)
    .await
}

async fn snapshot(db: &db::Db, tenant_id: TenantId, tracking_id: i64) -> String {
    let mut tx = tenant_tx(db, tenant_id).await;
    let row = sqlx::query_scalar(
        r#"
        SELECT row_to_json(tracking_row)::TEXT
        FROM order_tracking_numbers AS tracking_row
        WHERE id = $1
        "#,
    )
    .bind(tracking_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    row
}
