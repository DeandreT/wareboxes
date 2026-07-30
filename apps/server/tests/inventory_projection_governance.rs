mod common;

use common::*;
use sqlx::Acquire;

fn assert_database_error(error: sqlx::Error, code: &str, message: &str) {
    let database_error = error
        .as_database_error()
        .expect("operation should fail in PostgreSQL");
    assert_eq!(database_error.code().as_deref(), Some(code));
    assert!(
        database_error.message().contains(message),
        "unexpected database error: {}",
        database_error.message()
    );
}

#[tokio::test]
async fn on_hand_projection_changes_require_an_exact_current_journal() {
    let fixture = Fixture::new().await;
    let user = fixture.user("projection-governance@test.com").await;
    let access = default_tenant_for_user(&fixture.db, user.id)
        .await
        .expect("registered user has tenant access");
    let inventory_owner_id = fixture
        .inventory_owner(access.tenant_id, "Projection Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Projection Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, inventory_owner_id, facility_id)
        .await;
    let item_id = fixture
        .item(access.tenant_id, "Projection Item", "each")
        .await;
    let balance = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id,
                facility_id,
                item_id,
                qty: 10,
                key: "projection-governance",
            },
        )
        .await;

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let receipt_transaction_id: i64 = sqlx::query_scalar(
        r#"
        SELECT transaction_id
        FROM inventory_projection_changes
        WHERE tenant_id = $1
          AND inventory_balance_id = $2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(balance.balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let bare_error =
        sqlx::query("UPDATE inventory_balances SET qty_on_hand = qty_on_hand + 1 WHERE id = $1")
            .bind(balance.balance_id)
            .execute(&mut *tx)
            .await
            .unwrap_err();
    assert_database_error(
        bare_error,
        "55000",
        "on-hand inventory changes require a journal transaction",
    );
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    sqlx::query_scalar::<_, String>(
        "SELECT set_config('wareboxes.inventory_transaction_id', $1, true)",
    )
    .bind(receipt_transaction_id.to_string())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let stale_error =
        sqlx::query("UPDATE inventory_balances SET qty_on_hand = qty_on_hand + 1 WHERE id = $1")
            .bind(balance.balance_id)
            .execute(&mut *tx)
            .await
            .unwrap_err();
    assert_database_error(
        stale_error,
        "55000",
        "on-hand inventory changes require a journal transaction",
    );
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let transaction_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_transactions (
            tenant_id,
            inventory_owner_id,
            created,
            actor_user_id,
            transaction_type,
            reason,
            operation,
            request_hash
        )
        VALUES ($1, $2, $3, $4, 'adjust', 'projection test',
                'projection.test.mismatch', 'mismatch')
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(inventory_owner_id)
    .bind(db::now_iso())
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query_scalar::<_, String>(
        "SELECT set_config('wareboxes.inventory_transaction_id', $1, true)",
    )
    .bind(transaction_id.to_string())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query("UPDATE inventory_balances SET qty_on_hand = qty_on_hand + 1 WHERE id = $1")
        .bind(balance.balance_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO inventory_entries (
            tenant_id,
            inventory_owner_id,
            transaction_id,
            created,
            facility_id,
            location_id,
            license_plate_id,
            item_batch_id,
            item_id,
            uom,
            lot,
            expiration,
            serial,
            status,
            quantity_delta
        )
        SELECT
            balance.tenant_id,
            balance.inventory_owner_id,
            $1,
            $2,
            balance.facility_id,
            balance.location_id,
            balance.license_plate_id,
            balance.item_batch_id,
            balance.item_id,
            balance.uom,
            batch.lot,
            batch.expiration,
            batch.serial,
            balance.status,
            2
        FROM inventory_balances balance
        INNER JOIN item_batches batch
            ON batch.tenant_id = balance.tenant_id
           AND batch.inventory_owner_id = balance.inventory_owner_id
           AND batch.id = balance.item_batch_id
        WHERE balance.tenant_id = $3
          AND balance.id = $4
        "#,
    )
    .bind(transaction_id)
    .bind(db::now_iso())
    .bind(access.tenant_id.get())
    .bind(balance.balance_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    let mismatch_error =
        sqlx::query("SET CONSTRAINTS inventory_transactions_conserve_quantity IMMEDIATE")
            .execute(&mut *tx)
            .await
            .unwrap_err();
    assert_database_error(
        mismatch_error,
        "23514",
        "inventory journal entries must exactly match on-hand projection changes",
    );
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let (quantity, changes, transactions, entries, reconciliation): (i64, i64, i64, i64, i64) =
        sqlx::query_as(
            r#"
            SELECT
                (SELECT qty_on_hand FROM inventory_balances WHERE id = $1),
                (SELECT COUNT(*) FROM inventory_projection_changes),
                (SELECT COUNT(*) FROM inventory_transactions),
                (SELECT COUNT(*) FROM inventory_entries),
                (SELECT COUNT(*) FROM inventory_reconciliation)
            "#,
        )
        .bind(balance.balance_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();

    assert_eq!(quantity, 10);
    assert_eq!(changes, 1);
    assert_eq!(transactions, 1);
    assert_eq!(entries, 1);
    assert_eq!(reconciliation, 0);

    let unbound_changes: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM inventory_projection_changes")
            .fetch_one(&fixture.db)
            .await
            .unwrap();
    assert_eq!(unbound_changes, 0);

    let mut connection = fixture.db.acquire().await.unwrap();
    let mut local_context = connection.begin().await.unwrap();
    db::bind_tenant_context(&mut local_context, access.tenant_id)
        .await
        .unwrap();
    sqlx::query_scalar::<_, String>(
        "SELECT set_config('wareboxes.inventory_transaction_id', $1, true)",
    )
    .bind(receipt_transaction_id.to_string())
    .fetch_one(&mut *local_context)
    .await
    .unwrap();
    local_context.rollback().await.unwrap();
    let leaked_transaction_id: Option<String> = sqlx::query_scalar(
        "SELECT NULLIF(current_setting('wareboxes.inventory_transaction_id', true), '')",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(leaked_transaction_id, None);
}
