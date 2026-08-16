use chrono::{DateTime, Duration, Timelike, Utc};
use wareboxes_application::inventory_integrity::{
    InventoryReconciliationAlert, InventoryReconciliationHealth,
};
use wareboxes_persistence_postgres::inventory_reconciliation;

use super::common::*;

fn minute_now() -> DateTime<Utc> {
    Utc::now()
        .with_second(0)
        .and_then(|value| value.with_nanosecond(0))
        .unwrap()
}

async fn set_balance_quantity_without_journal(db: &db::Db, balance_id: i64, quantity: i64) {
    let admin = admin_db_for(db).await;
    sqlx::query(
        "ALTER TABLE inventory_balances DISABLE TRIGGER inventory_balances_capture_projection_change",
    )
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query("UPDATE inventory_balances SET qty_on_hand=$1 WHERE id=$2")
        .bind(quantity)
        .bind(balance_id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "ALTER TABLE inventory_balances ENABLE TRIGGER inventory_balances_capture_projection_change",
    )
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;
}

#[tokio::test]
async fn scheduled_runs_are_exactly_once_immutable_and_emit_only_health_transitions() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("reconciliation-worker@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Reconciliation Client")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Reconciliation Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let item_id = fixture
        .item(access.tenant_id, "Reconciliation Item", "each")
        .await;
    let balance = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id,
                qty: 10,
                key: "reconciliation-control",
            },
        )
        .await;
    let base = minute_now();
    let first_schedule = base - Duration::minutes(3);

    let left = inventory_reconciliation::execute(
        &fixture.db,
        access.tenant_id,
        "reconciliation-a",
        first_schedule,
        60,
    );
    let right = inventory_reconciliation::execute(
        &fixture.db,
        access.tenant_id,
        "reconciliation-b",
        first_schedule,
        60,
    );
    let (left, right) = tokio::join!(left, right);
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left.run_id, right.run_id);
    assert_eq!(u8::from(left.created) + u8::from(right.created), 1);
    assert_eq!(left.health, InventoryReconciliationHealth::Healthy);
    assert_eq!(left.alert, None);

    set_balance_quantity_without_journal(&fixture.db, balance.balance_id, 12).await;
    let detected = inventory_reconciliation::execute(
        &fixture.db,
        access.tenant_id,
        "reconciliation-a",
        base - Duration::minutes(2),
        60,
    )
    .await
    .unwrap();
    assert_eq!(
        detected.health,
        InventoryReconciliationHealth::IssuesDetected
    );
    assert_eq!(detected.journal_projection_issue_count, 1);
    assert_eq!(detected.max_severity_quantity, 2);
    assert_eq!(
        detected.alert,
        Some(InventoryReconciliationAlert::IssuesDetected)
    );

    let replay = inventory_reconciliation::execute(
        &fixture.db,
        access.tenant_id,
        "a-different-worker",
        base - Duration::minutes(2),
        60,
    )
    .await
    .unwrap();
    assert_eq!(replay.run_id, detected.run_id);
    assert!(!replay.created);
    assert_eq!(replay.alert, None);
    assert!(inventory_reconciliation::execute(
        &fixture.db,
        access.tenant_id,
        "reconciliation-a",
        base - Duration::minutes(2),
        120,
    )
    .await
    .is_err());

    set_balance_quantity_without_journal(&fixture.db, balance.balance_id, 13).await;
    let changed = inventory_reconciliation::execute(
        &fixture.db,
        access.tenant_id,
        "reconciliation-a",
        base - Duration::minutes(1),
        60,
    )
    .await
    .unwrap();
    assert_eq!(
        changed.alert,
        Some(InventoryReconciliationAlert::IssuesChanged)
    );
    assert_ne!(changed.issue_digest, detected.issue_digest);

    set_balance_quantity_without_journal(&fixture.db, balance.balance_id, 10).await;
    let restored = inventory_reconciliation::execute(
        &fixture.db,
        access.tenant_id,
        "reconciliation-a",
        base,
        60,
    )
    .await
    .unwrap();
    assert_eq!(restored.health, InventoryReconciliationHealth::Healthy);
    assert_eq!(restored.alert, Some(InventoryReconciliationAlert::Restored));
    assert_eq!(restored.state_revision, 4);

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let (runs, alerts): (i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM inventory_reconciliation_runs),
               (SELECT COUNT(*) FROM outbox_events
                WHERE aggregate_type='inventory_reconciliation')
        "#,
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!((runs, alerts), (4, 3));
    let update = sqlx::query("UPDATE inventory_reconciliation_runs SET worker_id='forged'")
        .execute(&mut *tx)
        .await;
    assert!(update.is_err());
    tx.rollback().await.unwrap();

    let mut direct = tenant_tx(&fixture.db, access.tenant_id).await;
    let insert = sqlx::query(
        r#"
        INSERT INTO inventory_reconciliation_state(
          tenant_id,revision,health,issue_digest,last_run_id,last_scheduled_for,
          last_completed_at,next_due_at,journal_projection_issue_count,
          commitment_issue_count,affected_inventory_owner_count,
          affected_facility_count,max_severity_quantity)
        VALUES($1,99,'healthy',repeat('0',32),$2,$3,$3,$3+INTERVAL '1 minute',0,0,0,0,0)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(restored.run_id.get())
    .bind(base)
    .execute(&mut *direct)
    .await;
    assert!(insert.is_err());
    direct.rollback().await.unwrap();
}
