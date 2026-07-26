mod common;

use common::*;

#[derive(Clone, Copy)]
struct HoldRefs {
    tenant_id: TenantId,
    user_id: i64,
    inventory_owner_id: i64,
    inventory_balance_id: i64,
    facility_id: i64,
    location_id: i64,
    item_batch_id: i64,
    item_id: i64,
}

#[tokio::test]
async fn inventory_holds_require_transaction_local_tenant_context() {
    let fixture = Fixture::new().await;
    let user_a = fixture.user("hold-rls-a@test.com").await;
    let user_b = fixture.user("hold-rls-b@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, user_a.id).await;
    let tenant_b = tenant_for_user(&fixture.db, user_b.id).await;
    let refs_a = hold_refs(&fixture, tenant_a, user_a.id, "Hold RLS A").await;
    let refs_b = hold_refs(&fixture, tenant_b, user_b.id, "Hold RLS B").await;

    let mut tenant_a_tx = tenant_tx(&fixture.db, tenant_a).await;
    let hold_id = insert_hold(&mut tenant_a_tx, refs_a).await.unwrap();
    tenant_a_tx.commit().await.unwrap();
    let source_snapshot =
        snapshot(&fixture.db, tenant_a, hold_id, refs_a.inventory_balance_id).await;

    let unbound_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inventory_holds")
        .fetch_one(&fixture.db)
        .await
        .unwrap();
    assert_eq!(unbound_count, 0);
    assert_eq!(
        sqlx::query("UPDATE inventory_holds SET qty = qty WHERE id = $1")
            .bind(hold_id)
            .execute(&fixture.db)
            .await
            .unwrap()
            .rows_affected(),
        0
    );
    let mut unbound_tx = fixture.db.begin().await.unwrap();
    assert!(insert_hold(&mut unbound_tx, refs_a).await.is_err());
    unbound_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    let guessed_visibility: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM inventory_holds WHERE id = $1 OR inventory_balance_id = $2",
    )
    .bind(hold_id)
    .bind(refs_a.inventory_balance_id)
    .fetch_one(&mut *tenant_b_tx)
    .await
    .unwrap();
    assert_eq!(guessed_visibility, 0);
    assert_eq!(
        sqlx::query("UPDATE inventory_holds SET note = note WHERE id = $1")
            .bind(hold_id)
            .execute(&mut *tenant_b_tx)
            .await
            .unwrap()
            .rows_affected(),
        0
    );
    tenant_b_tx.rollback().await.unwrap();

    let mut forged_refs = refs_b;
    forged_refs.inventory_balance_id = refs_a.inventory_balance_id;
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_hold(&mut tenant_b_tx, forged_refs).await.is_err());
    tenant_b_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_hold(&mut tenant_b_tx, refs_a).await.is_err());
    tenant_b_tx.rollback().await.unwrap();

    let mut tenant_a_tx = tenant_tx(&fixture.db, tenant_a).await;
    let immutable_error = sqlx::query("UPDATE inventory_holds SET qty = qty + 1 WHERE id = $1")
        .bind(hold_id)
        .execute(&mut *tenant_a_tx)
        .await
        .unwrap_err();
    assert!(immutable_error
        .to_string()
        .contains("inventory hold dimensions are immutable"));
    tenant_a_tx.rollback().await.unwrap();

    assert_eq!(
        snapshot(&fixture.db, tenant_a, hold_id, refs_a.inventory_balance_id).await,
        source_snapshot
    );
}

async fn hold_refs(fixture: &Fixture, tenant_id: TenantId, user_id: i64, key: &str) -> HoldRefs {
    let access = default_tenant_for_user(&fixture.db, user_id).await.unwrap();
    let inventory_owner_id = fixture.inventory_owner(tenant_id, key).await;
    let facility_id = fixture.facility(tenant_id, key).await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let item_id = fixture.item(tenant_id, key, "each").await;
    let balance = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id,
                facility_id,
                item_id,
                qty: 10,
                key,
            },
        )
        .await;
    HoldRefs {
        tenant_id,
        user_id,
        inventory_owner_id,
        inventory_balance_id: balance.balance_id,
        facility_id,
        location_id: balance.location_id,
        item_batch_id: balance.item_batch_id,
        item_id,
    }
}

async fn insert_hold(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    refs: HoldRefs,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        INSERT INTO inventory_holds (
            tenant_id, inventory_owner_id, created, modified, created_by,
            inventory_balance_id, facility_id, location_id, item_batch_id,
            item_id, uom, inventory_status, qty, reason_code, note, status
        )
        VALUES (
            $1, $2, $3, $3, $4, $5, $6, $7, $8, $9,
            'each', 'available', 2, 'quality_inspection', 'adversarial RLS', 'active'
        )
        RETURNING id
        "#,
    )
    .bind(refs.tenant_id.get())
    .bind(refs.inventory_owner_id)
    .bind(db::now_iso())
    .bind(refs.user_id)
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
    hold_id: i64,
    inventory_balance_id: i64,
) -> (String, String) {
    let mut tx = tenant_tx(db, tenant_id).await;
    let snapshot = sqlx::query_as(
        r#"
        SELECT
            (SELECT row_to_json(row)::TEXT FROM inventory_holds row WHERE id = $1),
            (SELECT row_to_json(row)::TEXT FROM inventory_balances row WHERE id = $2)
        "#,
    )
    .bind(hold_id)
    .bind(inventory_balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    snapshot
}
