mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use tokio::sync::Barrier;
use tokio::time::{sleep, timeout};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InventorySnapshot {
    transactions: i64,
    entries: i64,
    balances: i64,
    quantity: i64,
}

async fn inventory_snapshot(db: &db::Db, tenant_id: TenantId) -> InventorySnapshot {
    let mut tx = tenant_tx(db, tenant_id).await;
    let (transactions, entries, balances, quantity) = sqlx::query_as(
        r#"
        SELECT
            (SELECT COUNT(*) FROM inventory_transactions WHERE tenant_id = $1),
            (SELECT COUNT(*) FROM inventory_entries WHERE tenant_id = $1),
            (SELECT COUNT(*) FROM inventory_balances WHERE tenant_id = $1),
            (
                SELECT COALESCE(SUM(qty_on_hand), 0)::BIGINT
                FROM inventory_balances
                WHERE tenant_id = $1 AND deleted IS NULL
            )
        "#,
    )
    .bind(tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    InventorySnapshot {
        transactions,
        entries,
        balances,
        quantity,
    }
}

async fn configure_database_timeouts(db: &db::Db) {
    let admin_db = admin_db_for(db).await;
    let database_name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&admin_db)
        .await
        .unwrap();
    let quoted_database_name = database_name.replace('"', "\"\"");
    for (setting, value) in [("lock_timeout", "750ms"), ("statement_timeout", "1500ms")] {
        sqlx::query(&format!(
            "ALTER ROLE wareboxes_app IN DATABASE \"{quoted_database_name}\" SET {setting} = '{value}'"
        ))
        .execute(&admin_db)
        .await
        .unwrap();
    }
    admin_db.close().await;
}

async fn wait_until_balance_is_locked(db: &db::Db, tenant_id: TenantId, inventory_balance_id: i64) {
    timeout(Duration::from_secs(2), async {
        loop {
            let mut tx = tenant_tx(db, tenant_id).await;
            let result = sqlx::query(
                r#"
                SELECT id
                FROM inventory_balances
                WHERE tenant_id = $1 AND id = $2
                FOR UPDATE NOWAIT
                "#,
            )
            .bind(tenant_id.get())
            .bind(inventory_balance_id)
            .fetch_one(&mut *tx)
            .await;
            tx.rollback().await.unwrap();

            match result {
                Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("55P03") => {
                    break;
                }
                Ok(_) => sleep(Duration::from_millis(10)).await,
                Err(error) => panic!("unexpected inventory balance lock probe error: {error}"),
            }
        }
    })
    .await
    .expect("inventory move locks every affected balance");
}

#[tokio::test]
async fn opposing_inventory_moves_lock_balances_canonically_and_replay_exactly() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("move-lock-order@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let inventory_owner_id = fixture
        .inventory_owner(tenant_id, "Move Lock Order Owner")
        .await;
    let facility_id = fixture.facility(tenant_id, "Move Lock Order DC").await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let location_a = fixture
        .location(tenant_id, facility_id, "MOVE-LOCK-A")
        .await;
    let location_b = fixture
        .location(tenant_id, facility_id, "MOVE-LOCK-B")
        .await;
    let item_id = fixture.item(tenant_id, "Move Lock Item", "each").await;
    let item_batch_id = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        inventory_owner_id,
        item_id,
        None,
        Some("MOVE-LOCK-LOT"),
        None,
        None,
    )
    .await
    .unwrap();

    for (location_id, idempotency_key) in [
        (location_a, "move-lock-receive-a"),
        (location_b, "move-lock-receive-b"),
    ] {
        repo::inventory::receive_inventory(
            &fixture.db,
            tenant_id,
            user.id,
            item_batch_id,
            location_id,
            10,
            None,
            None,
            None,
            None,
            idempotency_key,
        )
        .await
        .unwrap();
    }
    let balances = repo::inventory::get_balances(&fixture.db, tenant_id, false)
        .await
        .unwrap();
    let balance_a_id = balances
        .iter()
        .find(|balance| balance.location_id == location_a && balance.item_batch_id == item_batch_id)
        .unwrap()
        .id;
    let balance_b_id = balances
        .iter()
        .find(|balance| balance.location_id == location_b && balance.item_batch_id == item_batch_id)
        .unwrap()
        .id;

    configure_database_timeouts(&fixture.db).await;
    let move_db = app_db_for(&fixture.db).await;
    let split_db = app_db_for(&fixture.db).await;

    let key_a =
        format!("inventory-location-item:{tenant_id}:{inventory_owner_id}:{location_a}:{item_id}");
    let key_b =
        format!("inventory-location-item:{tenant_id}:{inventory_owner_id}:{location_b}:{item_id}");
    let mut blocker_a = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(key_a)
        .execute(&mut *blocker_a)
        .await
        .unwrap();
    let mut blocker_b = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(key_b)
        .execute(&mut *blocker_b)
        .await
        .unwrap();

    let move_key = "move-lock-forward";
    let split_key = "move-lock-reverse";
    let barrier = Arc::new(Barrier::new(3));
    let move_barrier = Arc::clone(&barrier);
    let split_barrier = Arc::clone(&barrier);
    let mut forward = tokio::spawn(async move {
        move_barrier.wait().await;
        repo::inventory::move_inventory(
            &move_db,
            tenant_id,
            user.id,
            item_batch_id,
            location_a,
            location_b,
            1,
            None,
            Some("canonical lock order forward"),
            None,
            None,
            move_key,
        )
        .await
    });
    let mut reverse = tokio::spawn(async move {
        split_barrier.wait().await;
        repo::inventory::split_move_inventory(
            &split_db,
            tenant_id,
            user.id,
            balance_b_id,
            &[(location_a, 1)],
            Some("canonical lock order reverse"),
            None,
            None,
            split_key,
        )
        .await
    });

    barrier.wait().await;
    wait_until_balance_is_locked(&fixture.db, tenant_id, balance_a_id).await;
    wait_until_balance_is_locked(&fixture.db, tenant_id, balance_b_id).await;
    assert!(
        timeout(Duration::from_millis(250), async {
            tokio::select! {
                _ = &mut forward => {}
                _ = &mut reverse => {}
            }
        })
        .await
        .is_err(),
        "an opposing move completed while its destination advisory lock was held"
    );

    blocker_a.commit().await.unwrap();
    blocker_b.commit().await.unwrap();
    let (forward_result, reverse_result) = timeout(Duration::from_secs(2), async {
        (forward.await.unwrap(), reverse.await.unwrap())
    })
    .await
    .expect("opposing inventory moves complete without a deadlock");
    let forward_transaction_id = forward_result.unwrap();
    let reverse_transaction_id = reverse_result.unwrap();

    let balances = repo::inventory::get_balances(&fixture.db, tenant_id, false)
        .await
        .unwrap();
    assert_eq!(
        balances
            .iter()
            .find(|balance| {
                balance.location_id == location_a && balance.item_batch_id == item_batch_id
            })
            .unwrap()
            .qty_on_hand,
        10
    );
    assert_eq!(
        balances
            .iter()
            .find(|balance| {
                balance.location_id == location_b && balance.item_batch_id == item_batch_id
            })
            .unwrap()
            .qty_on_hand,
        10
    );
    assert!(
        repo::inventory::get_reconciliation_issues(&fixture.db, tenant_id)
            .await
            .unwrap()
            .is_empty()
    );

    let before_retries = inventory_snapshot(&fixture.db, tenant_id).await;
    assert_eq!(before_retries.quantity, 20);
    let replayed_move = repo::inventory::move_inventory(
        &fixture.db,
        tenant_id,
        user.id,
        item_batch_id,
        location_a,
        location_b,
        1,
        None,
        Some("canonical lock order forward"),
        None,
        None,
        move_key,
    )
    .await
    .unwrap();
    assert_eq!(replayed_move, forward_transaction_id);

    let replayed_split = repo::inventory::split_move_inventory(
        &fixture.db,
        tenant_id,
        user.id,
        balance_b_id,
        &[(location_a, 1)],
        Some("canonical lock order reverse"),
        None,
        None,
        split_key,
    )
    .await
    .unwrap();
    assert_eq!(replayed_split, reverse_transaction_id);
    assert_eq!(
        inventory_snapshot(&fixture.db, tenant_id).await,
        before_retries
    );
    assert!(
        repo::inventory::get_reconciliation_issues(&fixture.db, tenant_id)
            .await
            .unwrap()
            .is_empty()
    );
}
