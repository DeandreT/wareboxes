use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use chrono::{Timelike, Utc};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::{repo, routes, state::AppState};
use wareboxes_api_contract::v1::{
    InventoryReconciliationCoverage, InventoryReconciliationHealth,
    InventoryReconciliationMonitorState, InventoryReconciliationStatusResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_persistence_postgres::inventory_reconciliation;

use super::common::*;

async fn mismatch(db: &db::Db, balance_ids: &[i64]) {
    let admin = admin_db_for(db).await;
    sqlx::query(
        "ALTER TABLE inventory_balances DISABLE TRIGGER inventory_balances_capture_projection_change",
    )
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query("UPDATE inventory_balances SET qty_on_hand=qty_on_hand+1 WHERE id=ANY($1)")
        .bind(balance_ids)
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
async fn status_reports_schedule_but_recomputes_issue_counts_inside_current_access_scope() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("reconciliation-scope@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    let visible_owner = fixture
        .inventory_owner(access.tenant_id, "Visible Reconciliation Client")
        .await;
    let hidden_owner = fixture
        .inventory_owner(access.tenant_id, "Hidden Reconciliation Client")
        .await;
    let visible_facility = fixture
        .facility(access.tenant_id, "Visible Reconciliation Facility")
        .await;
    let hidden_facility = fixture
        .facility(access.tenant_id, "Hidden Reconciliation Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, visible_owner, visible_facility)
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, hidden_owner, hidden_facility)
        .await;
    let item_id = fixture
        .item(access.tenant_id, "Scoped Reconciliation Item", "each")
        .await;
    let visible = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: visible_owner,
                facility_id: visible_facility,
                item_id,
                qty: 5,
                key: "reconciliation-visible",
            },
        )
        .await;
    let hidden = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: hidden_owner,
                facility_id: hidden_facility,
                item_id,
                qty: 7,
                key: "reconciliation-hidden",
            },
        )
        .await;
    mismatch(&fixture.db, &[visible.balance_id, hidden.balance_id]).await;
    let scheduled_for = Utc::now()
        .with_second(0)
        .and_then(|value| value.with_nanosecond(0))
        .unwrap();
    let run = inventory_reconciliation::execute(
        &fixture.db,
        access.tenant_id,
        "scope-worker",
        scheduled_for,
        60,
    )
    .await
    .unwrap();
    assert_eq!(run.journal_projection_issue_count, 2);

    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        access.tenant_id,
        &UpdateUserAccessScope {
            user_id: operator.id,
            all_facilities: false,
            facility_ids: vec![visible_facility],
            all_inventory_owners: false,
            inventory_owner_ids: vec![visible_owner],
        },
    )
    .await
    .unwrap());
    let token = wareboxes_api::auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let request = Request::builder()
        .uri("/api/v1/inventory/reconciliation/status")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, access.tenant_id.to_string())
        .body(Body::empty())
        .unwrap();
    let response = routes::app(AppState::new(fixture.db.clone()))
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let status: InventoryReconciliationStatusResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        status.coverage,
        InventoryReconciliationCoverage::AccessScope
    );
    assert_eq!(
        status.monitor_state,
        InventoryReconciliationMonitorState::Current
    );
    assert_eq!(status.health, InventoryReconciliationHealth::IssuesDetected);
    assert_eq!(status.last_run_id, Some(run.run_id.get()));
    assert_eq!(status.journal_projection_issue_count, 1);
    assert_eq!(status.affected_inventory_owner_count, 1);
    assert_eq!(status.affected_facility_count, 1);

    let unbound_runs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM inventory_reconciliation_runs")
            .fetch_one(&fixture.db)
            .await
            .unwrap();
    assert_eq!(unbound_runs, 0);
    let other_user = fixture.user("reconciliation-other@test.local").await;
    let other_tenant = tenant_for_user(&fixture.db, other_user.id).await;
    let mut other_tx = tenant_tx(&fixture.db, other_tenant).await;
    let other_runs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inventory_reconciliation_runs")
        .fetch_one(&mut *other_tx)
        .await
        .unwrap();
    assert_eq!(other_runs, 0);
    let cross_tenant =
        sqlx::query("SELECT * FROM execute_inventory_reconciliation($1,'forged',$2,60)")
            .bind(access.tenant_id.get())
            .bind(scheduled_for)
            .fetch_one(&mut *other_tx)
            .await;
    assert!(cross_tenant.is_err());
    other_tx.rollback().await.unwrap();
}
