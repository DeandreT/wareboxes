mod common;

use common::*;

#[tokio::test]
async fn activity_rows_require_a_transaction_local_tenant_context() {
    let fixture = Fixture::new().await;
    let user_a = fixture.user("activity-rls-a@test.com").await;
    let user_b = fixture.user("activity-rls-b@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, user_a.id).await;
    let tenant_b = tenant_for_user(&fixture.db, user_b.id).await;
    let owner_a = fixture
        .inventory_owner(tenant_a, "Activity RLS Owner A")
        .await;
    let owner_b = fixture
        .inventory_owner(tenant_b, "Activity RLS Owner B")
        .await;
    let facility_a = fixture.facility(tenant_a, "Activity RLS Facility A").await;
    let facility_b = fixture.facility(tenant_b, "Activity RLS Facility B").await;
    let order_a = fixture.order(tenant_a, "ACTIVITY-RLS-ORDER", owner_a).await;
    let order_b = fixture.order(tenant_b, "ACTIVITY-RLS-ORDER", owner_b).await;
    let load_a = add_load(
        &fixture,
        tenant_a,
        user_a.id,
        facility_a,
        owner_a,
        "ACTIVITY-RLS-LOAD",
    )
    .await;
    let load_b = add_load(
        &fixture,
        tenant_b,
        user_b.id,
        facility_b,
        owner_b,
        "ACTIVITY-RLS-LOAD",
    )
    .await;
    assert_ne!(order_a, order_b);
    assert_ne!(load_a, load_b);

    let mut tenant_a_tx = tenant_tx(&fixture.db, tenant_a).await;
    let source_order_activity: (i64, String, bool) = sqlx::query_as(
        "SELECT id, action, deleted IS NULL FROM order_activity WHERE order_id = $1",
    )
    .bind(order_a)
    .fetch_one(&mut *tenant_a_tx)
    .await
    .unwrap();
    let source_load_activity: (i64, String, bool) =
        sqlx::query_as("SELECT id, action, deleted IS NULL FROM load_activity WHERE load_id = $1")
            .bind(load_a)
            .fetch_one(&mut *tenant_a_tx)
            .await
            .unwrap();
    tenant_a_tx.rollback().await.unwrap();

    let unbound_counts: (i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM order_activity),
               (SELECT COUNT(*) FROM load_activity)
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(unbound_counts, (0, 0));
    let unbound_order_updates =
        sqlx::query("UPDATE order_activity SET action = 'unbound' WHERE id = $1")
            .bind(source_order_activity.0)
            .execute(&fixture.db)
            .await
            .unwrap()
            .rows_affected();
    assert_eq!(unbound_order_updates, 0);
    let unbound_load_updates =
        sqlx::query("UPDATE load_activity SET action = 'unbound' WHERE id = $1")
            .bind(source_load_activity.0)
            .execute(&fixture.db)
            .await
            .unwrap()
            .rows_affected();
    assert_eq!(unbound_load_updates, 0);
    let unbound_order_deletes = sqlx::query("DELETE FROM order_activity WHERE id = $1")
        .bind(source_order_activity.0)
        .execute(&fixture.db)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(unbound_order_deletes, 0);
    let unbound_load_deletes = sqlx::query("DELETE FROM load_activity WHERE id = $1")
        .bind(source_load_activity.0)
        .execute(&fixture.db)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(unbound_load_deletes, 0);

    let mut unbound_tx = fixture.db.begin().await.unwrap();
    assert!(insert_order_activity(
        &mut unbound_tx,
        tenant_a,
        owner_a,
        order_a,
        "unbound insert",
    )
    .await
    .is_err());
    unbound_tx.rollback().await.unwrap();
    let mut unbound_tx = fixture.db.begin().await.unwrap();
    assert!(insert_load_activity(
        &mut unbound_tx,
        tenant_a,
        user_a.id,
        load_a,
        "unbound_insert",
    )
    .await
    .is_err());
    unbound_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    let guessed_counts: (i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM order_activity WHERE order_id = $1),
               (SELECT COUNT(*) FROM load_activity WHERE load_id = $2)
        "#,
    )
    .bind(order_a)
    .bind(load_a)
    .fetch_one(&mut *tenant_b_tx)
    .await
    .unwrap();
    assert_eq!(guessed_counts, (0, 0));
    let cross_tenant_order_updates =
        sqlx::query("UPDATE order_activity SET action = 'cross-tenant' WHERE order_id = $1")
            .bind(order_a)
            .execute(&mut *tenant_b_tx)
            .await
            .unwrap()
            .rows_affected();
    assert_eq!(cross_tenant_order_updates, 0);
    let cross_tenant_load_updates =
        sqlx::query("UPDATE load_activity SET action = 'cross-tenant' WHERE load_id = $1")
            .bind(load_a)
            .execute(&mut *tenant_b_tx)
            .await
            .unwrap()
            .rows_affected();
    assert_eq!(cross_tenant_load_updates, 0);
    let cross_tenant_order_deletes = sqlx::query("DELETE FROM order_activity WHERE order_id = $1")
        .bind(order_a)
        .execute(&mut *tenant_b_tx)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(cross_tenant_order_deletes, 0);
    let cross_tenant_load_deletes = sqlx::query("DELETE FROM load_activity WHERE load_id = $1")
        .bind(load_a)
        .execute(&mut *tenant_b_tx)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(cross_tenant_load_deletes, 0);
    tenant_b_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_order_activity(
        &mut tenant_b_tx,
        tenant_b,
        owner_b,
        order_a,
        "guessed aggregate",
    )
    .await
    .is_err());
    tenant_b_tx.rollback().await.unwrap();
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_load_activity(
        &mut tenant_b_tx,
        tenant_b,
        user_b.id,
        load_a,
        "guessed_aggregate",
    )
    .await
    .is_err());
    tenant_b_tx.rollback().await.unwrap();
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_order_activity(
        &mut tenant_b_tx,
        tenant_a,
        owner_a,
        order_a,
        "cross-tenant insert",
    )
    .await
    .is_err());
    tenant_b_tx.rollback().await.unwrap();
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_load_activity(
        &mut tenant_b_tx,
        tenant_a,
        user_a.id,
        load_a,
        "cross_tenant_insert",
    )
    .await
    .is_err());
    tenant_b_tx.rollback().await.unwrap();

    let mut tenant_a_tx = tenant_tx(&fixture.db, tenant_a).await;
    let unchanged_order_activity: (i64, String, bool) = sqlx::query_as(
        "SELECT id, action, deleted IS NULL FROM order_activity WHERE order_id = $1",
    )
    .bind(order_a)
    .fetch_one(&mut *tenant_a_tx)
    .await
    .unwrap();
    let unchanged_load_activity: (i64, String, bool) =
        sqlx::query_as("SELECT id, action, deleted IS NULL FROM load_activity WHERE load_id = $1")
            .bind(load_a)
            .fetch_one(&mut *tenant_a_tx)
            .await
            .unwrap();
    tenant_a_tx.rollback().await.unwrap();
    assert_eq!(unchanged_order_activity, source_order_activity);
    assert_eq!(unchanged_load_activity, source_load_activity);
}

async fn add_load(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    facility_id: i64,
    inventory_owner_id: i64,
    reference_number: &str,
) -> i64 {
    repo::loads::add_load(
        &fixture.db,
        tenant_id,
        user_id,
        facility_id,
        inventory_owner_id,
        LoadType::Inbound,
        Some(reference_number),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap()
}

async fn insert_order_activity(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    order_id: i64,
    action: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO order_activity
            (tenant_id, inventory_owner_id, created, order_id, action)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(db::now_iso())
    .bind(order_id)
    .bind(action)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

async fn insert_load_activity(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    user_id: i64,
    load_id: i64,
    action: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO load_activity
            (tenant_id, created, load_id, user_id, action)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(tenant_id.get())
    .bind(db::now_iso())
    .bind(load_id)
    .bind(user_id)
    .bind(action)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}
