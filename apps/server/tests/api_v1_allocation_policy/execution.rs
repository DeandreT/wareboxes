use axum::http::StatusCode;
use serde_json::Value;
use wareboxes_api_contract::v1::{
    AllocationPolicySource, ConfigurationScope, InventoryRotation, OrderAllocationStrategy,
    PlanOrderAllocationResponse,
};

use super::common::{admin_db_for, tenant_tx, ReceivedBalanceSetup};
use super::support::{plan_request, response_json, successful_plan, Rig};

#[derive(sqlx::FromRow)]
struct FrozenPolicyEvidence {
    policy_source: String,
    policy_configuration_id: Option<i64>,
    policy_configuration_revision: Option<i64>,
    policy_scope_level: Option<String>,
    strategy: String,
    policy_hash: String,
    policy_definition: Value,
    selected_balance_count: i64,
}

#[tokio::test]
async fn active_fifo_policy_drives_execution_and_freezes_replay_stable_evidence() {
    let rig = Rig::new("fifo-evidence").await;
    let active = rig
        .activate_policy("fifo", InventoryRotation::Fifo, true, false)
        .await;
    let access = super::common::default_tenant_for_user(&rig.fixture.db, rig.operator_id)
        .await
        .unwrap();
    let item_id = rig
        .fixture
        .item(rig.tenant_id, "Policy Rotation Item", "each")
        .await;
    let older = rig
        .fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: rig.owner_id,
                facility_id: rig.facility_id,
                item_id,
                qty: 4,
                key: "POLICY-FIFO-OLDER",
            },
        )
        .await;
    let newer = rig
        .fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: rig.owner_id,
                facility_id: rig.facility_id,
                item_id,
                qty: 4,
                key: "POLICY-FIFO-NEWER",
            },
        )
        .await;
    let admin = admin_db_for(&rig.fixture.db).await;
    sqlx::query(
        r#"
        UPDATE item_batches SET created=clock_timestamp()-INTERVAL '10 days',
          expiration=clock_timestamp()+INTERVAL '30 days'
        WHERE tenant_id=$1 AND id=$2
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(older.item_batch_id)
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE item_batches SET created=clock_timestamp()-INTERVAL '1 day',
          expiration=clock_timestamp()+INTERVAL '5 days'
        WHERE tenant_id=$1 AND id=$2
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(newer.item_batch_id)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;

    let order_id = rig.order("POLICY-FIFO-ORDER", &[(item_id, 2)]).await;
    let readiness = rig.readiness(order_id).await;
    assert_eq!(readiness.strategy, OrderAllocationStrategy::Fifo);
    assert_eq!(
        readiness.policy.source,
        AllocationPolicySource::Configuration
    );
    assert_eq!(
        readiness.policy.configuration_id,
        Some(active.configuration_id)
    );
    assert_eq!(
        readiness.policy.configuration_scope,
        Some(ConfigurationScope::OwnerFacility {
            inventory_owner_id: rig.owner_id,
            facility_id: rig.facility_id,
        })
    );
    let request = plan_request(&readiness);
    let result = successful_plan(&rig, order_id, "fifo-plan", &request).await;
    assert_eq!(result.strategy, OrderAllocationStrategy::Fifo);
    assert_eq!(result.policy, readiness.policy);
    assert_eq!(
        result.lines[0].allocations[0].inventory_balance_id, older.balance_id,
        "FIFO must choose the oldest receipt even when newer stock expires first"
    );

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let frozen: FrozenPolicyEvidence = sqlx::query_as(
        r#"
            SELECT policy_source,policy_configuration_id,policy_configuration_revision,
                   policy_scope_level,strategy,policy_hash,policy_definition,
                   (SELECT COUNT(*) FROM inventory_allocations allocation
                    WHERE allocation.tenant_id=run.tenant_id
                      AND allocation.allocation_run_id=run.id
                      AND allocation.inventory_balance_id=$3) AS selected_balance_count
            FROM order_allocation_runs run
            WHERE run.tenant_id=$1 AND run.id=$2
            "#,
    )
    .bind(rig.tenant_id.get())
    .bind(result.allocation_run_id)
    .bind(older.balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let payload: Value = sqlx::query_scalar(
        "SELECT payload FROM outbox_events WHERE tenant_id=$1 AND event_type='order.allocation.planned' AND payload->>'allocation_run_id'=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(result.allocation_run_id.to_string())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(frozen.policy_source, "configuration");
    assert_eq!(
        frozen.policy_configuration_id,
        Some(active.configuration_id)
    );
    assert_eq!(
        frozen.policy_configuration_revision,
        Some(active.revision.get())
    );
    assert_eq!(frozen.policy_scope_level.as_deref(), Some("owner_facility"));
    assert_eq!(frozen.strategy, "fifo");
    assert_eq!(frozen.policy_hash, result.policy.policy_hash);
    assert_eq!(frozen.policy_definition["rotation"], "fifo");
    assert_eq!(frozen.selected_balance_count, 1);
    assert_eq!(
        payload["allocation_policy"]["configuration_id"],
        active.configuration_id
    );

    let default_bypass_order = rig.order("POLICY-DEFAULT-BYPASS", &[(item_id, 1)]).await;
    let mut raw = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let default_bypass = sqlx::query(
        r#"
        INSERT INTO order_allocation_runs
          (tenant_id,inventory_owner_id,order_id,facility_id,created,created_by_user_id,
           strategy,policy_source,policy_definition,policy_hash,outcome,requested_qty,
           allocated_qty,short_qty,expected_revision,resulting_revision)
        VALUES ($1,$2,$3,$4,clock_timestamp(),$5,'fefo','product_default',
          '{"kind":"allocation","rotation":"fefo","allow_partial":true,
            "require_complete_line":false}'::jsonb,
          '6090a99a06ea2e049d7321d5cf2b8f462c6d6e6e2ca527ae87657a7a5fd9d156',
          'not_allocated',1,0,1,1,2)
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(rig.owner_id)
    .bind(default_bypass_order)
    .bind(rig.facility_id)
    .bind(rig.operator_id)
    .execute(&mut *raw)
    .await;
    assert!(
        default_bypass.is_err(),
        "the database must reject product-default evidence while a configuration is active"
    );
    raw.rollback().await.unwrap();

    let retired = rig
        .transition_policy(
            &rig.operator_token,
            active.configuration_id,
            "retirements",
            active.revision.get(),
            "fifo-retire",
        )
        .await;
    assert_eq!(retired.revision.get(), active.revision.get() + 1);
    let replay: PlanOrderAllocationResponse = response_json(
        rig.plan(order_id, "fifo-plan", &request).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay, result);

    let stale_order = rig.order("POLICY-STALE-ORDER", &[(item_id, 1)]).await;
    let stale = rig.plan(stale_order, "stale-policy", &request).await;
    let stale_status = stale.status();
    let stale_body =
        response_json::<wareboxes_api_contract::v1::ErrorResponse>(stale, StatusCode::CONFLICT)
            .await;
    assert_eq!(stale_status, StatusCode::CONFLICT);
    assert_eq!(
        stale_body.message,
        "allocation policy changed; refresh allocation readiness"
    );

    let mut raw = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let forged = sqlx::query(
        r#"
        INSERT INTO order_allocation_runs
          (tenant_id,inventory_owner_id,order_id,facility_id,created,created_by_user_id,
           strategy,policy_source,policy_definition,policy_hash,outcome,requested_qty,
           allocated_qty,short_qty,expected_revision,resulting_revision)
        VALUES ($1,$2,$3,$4,clock_timestamp(),$5,'fefo','product_default',
          '{"kind":"allocation","rotation":"fefo","allow_partial":true,
            "require_complete_line":false}'::jsonb,
          repeat('0',64),'not_allocated',1,0,1,1,2)
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(rig.owner_id)
    .bind(stale_order)
    .bind(rig.facility_id)
    .bind(rig.operator_id)
    .execute(&mut *raw)
    .await;
    assert!(
        forged.is_err(),
        "the database must reject forged policy evidence"
    );
    raw.rollback().await.unwrap();
}
