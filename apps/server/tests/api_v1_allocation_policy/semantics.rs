use wareboxes_api_contract::v1::{
    InventoryRotation, OrderAllocationOutcome, OrderAllocationStrategy,
};

use super::common::{default_tenant_for_user, tenant_tx, ReceivedBalanceSetup};
use super::support::{plan_request, successful_plan, Rig};

#[tokio::test]
async fn complete_line_policy_commits_only_fully_satisfied_lines() {
    let rig = Rig::new("complete-lines").await;
    rig.activate_policy("complete-lines", InventoryRotation::Fefo, false, true)
        .await;
    let access = default_tenant_for_user(&rig.fixture.db, rig.operator_id)
        .await
        .unwrap();
    let full_item = rig
        .fixture
        .item(rig.tenant_id, "Complete Line Stock", "each")
        .await;
    let short_item = rig
        .fixture
        .item(rig.tenant_id, "Incomplete Line Stock", "each")
        .await;
    rig.fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: rig.owner_id,
                facility_id: rig.facility_id,
                item_id: full_item,
                qty: 3,
                key: "COMPLETE-LINE-FULL",
            },
        )
        .await;
    rig.fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: rig.owner_id,
                facility_id: rig.facility_id,
                item_id: short_item,
                qty: 2,
                key: "COMPLETE-LINE-SHORT",
            },
        )
        .await;
    let order_id = rig
        .order("COMPLETE-LINE-ORDER", &[(full_item, 3), (short_item, 4)])
        .await;
    let readiness = rig.readiness(order_id).await;
    assert!(readiness.policy.require_complete_line);
    let result = successful_plan(
        &rig,
        order_id,
        "complete-lines-plan",
        &plan_request(&readiness),
    )
    .await;
    assert_eq!(result.outcome, OrderAllocationOutcome::PartiallyAllocated);
    assert_eq!(result.allocated_quantity, 3);
    assert_eq!(result.shortage_quantity, 4);
    assert_eq!(result.lines[0].allocated_quantity, 3);
    assert_eq!(result.lines[1].allocated_quantity, 0);
    assert!(result.lines[1].allocations.is_empty());
}

#[tokio::test]
async fn nonpartial_order_policy_rolls_back_every_selection_when_demand_is_short() {
    let rig = Rig::new("complete-order").await;
    rig.activate_policy("complete-order", InventoryRotation::Fifo, false, false)
        .await;
    let access = default_tenant_for_user(&rig.fixture.db, rig.operator_id)
        .await
        .unwrap();
    let item_id = rig
        .fixture
        .item(rig.tenant_id, "Complete Order Stock", "each")
        .await;
    let balance = rig
        .fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: rig.owner_id,
                facility_id: rig.facility_id,
                item_id,
                qty: 2,
                key: "COMPLETE-ORDER-SHORT",
            },
        )
        .await;
    let order_id = rig.order("COMPLETE-ORDER", &[(item_id, 4)]).await;
    let readiness = rig.readiness(order_id).await;
    assert_eq!(readiness.strategy, OrderAllocationStrategy::Fifo);
    assert!(!readiness.policy.allow_partial);
    assert!(!readiness.policy.require_complete_line);
    let result = successful_plan(
        &rig,
        order_id,
        "complete-order-plan",
        &plan_request(&readiness),
    )
    .await;
    assert_eq!(result.outcome, OrderAllocationOutcome::NotAllocated);
    assert_eq!(result.newly_allocated_quantity, 0);
    assert_eq!(result.allocated_quantity, 0);
    assert_eq!(result.shortage_quantity, 4);
    assert!(result.lines[0].allocations.is_empty());

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let reserved: (i64, i64) = sqlx::query_as(
        "SELECT qty_reserved,(SELECT COUNT(*) FROM inventory_allocations WHERE tenant_id=$1 AND allocation_run_id=$3) FROM inventory_balances WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(balance.balance_id)
    .bind(result.allocation_run_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(reserved, (0, 0));
}
