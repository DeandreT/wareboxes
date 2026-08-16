use axum::http::StatusCode;
use wareboxes_api_contract::v1::{
    AllocationPolicySource, ErrorResponse, InventoryRotation, PlanOrderAllocationResponse,
};

use super::common::{default_tenant_for_user, tenant_tx, ReceivedBalanceSetup};
use super::support::{plan_request, response_json, successful_plan, Rig};

#[tokio::test]
async fn concurrent_activation_and_allocation_have_one_explainable_policy_order() {
    let rig = Rig::new("activation-race").await;
    let approved = rig
        .approve_policy("race-fifo", InventoryRotation::Fifo, true, false)
        .await;
    let access = default_tenant_for_user(&rig.fixture.db, rig.operator_id)
        .await
        .unwrap();
    let item_id = rig
        .fixture
        .item(rig.tenant_id, "Activation Race Item", "each")
        .await;
    rig.fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: rig.owner_id,
                facility_id: rig.facility_id,
                item_id,
                qty: 3,
                key: "ACTIVATION-RACE-STOCK",
            },
        )
        .await;
    let order_id = rig.order("ACTIVATION-RACE-ORDER", &[(item_id, 2)]).await;
    let initial = rig.readiness(order_id).await;
    assert_eq!(
        initial.policy.source,
        AllocationPolicySource::ProductDefault
    );
    let initial_request = plan_request(&initial);

    let (activated, allocation) = tokio::join!(
        rig.activate_approved("race-fifo", &approved),
        rig.plan(order_id, "activation-race-plan", &initial_request),
    );
    assert_eq!(activated.configuration_id, approved.configuration_id);

    let allocation_succeeded = allocation.status() == StatusCode::OK;
    if allocation_succeeded {
        let result: PlanOrderAllocationResponse = response_json(allocation, StatusCode::OK).await;
        assert_eq!(
            result.policy.source,
            AllocationPolicySource::ProductDefault,
            "a successful pre-activation command must freeze the default policy"
        );
    } else {
        let error: ErrorResponse = response_json(allocation, StatusCode::CONFLICT).await;
        assert_eq!(
            error.message,
            "allocation policy changed; refresh allocation readiness"
        );
        let refreshed = rig.readiness(order_id).await;
        assert_eq!(
            refreshed.policy.configuration_id,
            Some(approved.configuration_id)
        );
        successful_plan(
            &rig,
            order_id,
            "activation-race-refreshed",
            &plan_request(&refreshed),
        )
        .await;
    }

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let runs: Vec<(String, Option<i64>)> = sqlx::query_as(
        "SELECT policy_source,policy_configuration_id FROM order_allocation_runs WHERE tenant_id=$1 AND order_id=$2 ORDER BY id",
    )
    .bind(rig.tenant_id.get())
    .bind(order_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(runs.len(), 1);
    if allocation_succeeded {
        assert_eq!(runs[0], ("product_default".into(), None));
    } else {
        assert_eq!(
            runs[0],
            ("configuration".into(), Some(approved.configuration_id))
        );
    }
}
