mod common;
#[path = "api_v1_cycle_count_variances/decision_policy.rs"]
mod decision_policy;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    ConfigureCycleCountPolicyResponse, CreateCycleCountTaskResponse, CycleCountClaimResponse,
    CycleCountConfirmationResponse, CycleCountDisposition, CycleCountPolicyPage,
    CycleCountVariancePage, CycleCountVarianceStatus, DecideCycleCountVarianceResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;

struct Rig {
    fixture: Fixture,
    tenant_id: TenantId,
    token: String,
    app: axum::Router,
    user_id: i64,
    owner_id: i64,
    facility_id: i64,
    balance_id: i64,
    location_barcode: String,
    item_barcode: String,
}

impl Rig {
    async fn new(suffix: &str) -> Self {
        let fixture = Fixture::new().await;
        let user = fixture
            .wms_user(&format!("cycle-count-variance-{suffix}@test.local"))
            .await;
        let tenant_id = tenant_for_user(&fixture.db, user.id).await;
        grant_permission(
            &fixture,
            tenant_id,
            user.id,
            "wms_supervisor",
            &format!("cycle-count-variance-supervisor-{suffix}"),
        )
        .await;
        let facility_id = fixture
            .facility(tenant_id, &format!("Count Variance {suffix} DC"))
            .await;
        let owner_id = fixture
            .inventory_owner(tenant_id, &format!("Count Variance {suffix} Client"))
            .await;
        fixture
            .assign_owner_to_facility(tenant_id, owner_id, facility_id)
            .await;
        let item_id = fixture
            .item(
                tenant_id,
                &format!("Count Variance {suffix} Widget"),
                "each",
            )
            .await;
        let item_barcode = format!("COUNT-VARIANCE-{}", suffix.to_uppercase());
        repo::items::add_barcode(
            &fixture.db,
            tenant_id,
            item_id,
            &item_barcode,
            "code128",
            None,
        )
        .await
        .unwrap();
        let batch_id = repo::inventory::add_item_batch(
            &fixture.db,
            tenant_id,
            owner_id,
            item_id,
            None,
            Some(&format!("COUNT-VARIANCE-{suffix}-LOT")),
            None,
            None,
        )
        .await
        .unwrap();
        let location_barcode = format!("COUNT-VARIANCE-{}-01", suffix.to_uppercase());
        let location_id = fixture
            .location(tenant_id, facility_id, &location_barcode)
            .await;
        repo::inventory::receive_inventory(
            &fixture.db,
            tenant_id,
            user.id,
            batch_id,
            location_id,
            10,
            None,
            Some("cycle count variance fixture"),
            None,
            None,
            &format!("cycle-count-variance-{suffix}-receive"),
        )
        .await
        .unwrap();
        let balance_id = repo::inventory::get_balances(&fixture.db, tenant_id, false)
            .await
            .unwrap()
            .into_iter()
            .find(|balance| balance.item_batch_id == batch_id && balance.location_id == location_id)
            .unwrap()
            .id;
        let token = auth::create_session(&fixture.db, user.id).await.unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));
        Self {
            fixture,
            tenant_id,
            token,
            app,
            user_id: user.id,
            owner_id,
            facility_id,
            balance_id,
            location_barcode,
            item_barcode,
        }
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        key: Option<&str>,
        body: Option<Value>,
    ) -> axum::response::Response {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
            .header(TENANT_ID_HEADER, self.tenant_id.to_string());
        if let Some(key) = key {
            request = request.header(IDEMPOTENCY_KEY_HEADER, key);
        }
        let body = match body {
            Some(body) => {
                request = request.header(header::CONTENT_TYPE, "application/json");
                Body::from(body.to_string())
            }
            None => Body::empty(),
        };
        self.app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap()
    }

    async fn configure_policy(&self, recount_limit: u16) -> ConfigureCycleCountPolicyResponse {
        response_json(
            expect_status(
                self.send(
                    Method::POST,
                    "/api/v1/cycle-count-policies",
                    Some("configure-count-policy"),
                    Some(json!({
                        "inventory_owner_id": self.owner_id,
                        "facility_id": self.facility_id,
                        "absolute_tolerance_quantity": 1,
                        "percentage_tolerance_basis_points": 0,
                        "automatic_recount_limit": recount_limit
                    })),
                )
                .await,
                StatusCode::OK,
            )
            .await,
        )
        .await
    }

    async fn create_task(&self, key: &str) -> i64 {
        response_json::<CreateCycleCountTaskResponse>(
            expect_status(
                self.send(
                    Method::POST,
                    "/api/v1/cycle-count-tasks",
                    Some(key),
                    Some(json!({"inventory_balance_id": self.balance_id})),
                )
                .await,
                StatusCode::OK,
            )
            .await,
        )
        .await
        .task_id
    }

    async fn claim(&self, task_id: i64, key: &str) -> CycleCountClaimResponse {
        response_json(
            expect_status(
                self.send(
                    Method::POST,
                    &format!("/api/v1/cycle-count-claims/{task_id}"),
                    Some(key),
                    Some(json!({})),
                )
                .await,
                StatusCode::OK,
            )
            .await,
        )
        .await
    }

    async fn confirm(
        &self,
        task_id: i64,
        counted_quantity: i64,
        key: &str,
    ) -> CycleCountConfirmationResponse {
        response_json(
            expect_status(
                self.send(
                    Method::POST,
                    &format!("/api/v1/cycle-count-tasks/{task_id}/confirmations"),
                    Some(key),
                    Some(json!({
                        "location_barcode": self.location_barcode,
                        "item_barcode": self.item_barcode,
                        "counted_quantity": counted_quantity
                    })),
                )
                .await,
                StatusCode::OK,
            )
            .await,
        )
        .await
    }

    async fn on_hand(&self) -> i64 {
        repo::inventory::get_balances(&self.fixture.db, self.tenant_id, false)
            .await
            .unwrap()
            .into_iter()
            .find(|balance| balance.id == self.balance_id)
            .unwrap()
            .qty_on_hand
    }

    async fn set_scope(&self, facility_ids: Vec<i64>, inventory_owner_ids: Vec<i64>) {
        assert!(repo::tenants::update_user_access_scope(
            &self.fixture.db,
            self.tenant_id,
            &UpdateUserAccessScope {
                user_id: self.user_id,
                all_facilities: false,
                facility_ids,
                all_inventory_owners: false,
                inventory_owner_ids,
            },
        )
        .await
        .unwrap());
    }
}

#[tokio::test]
async fn out_of_tolerance_count_recounts_then_approval_posts_once() {
    let rig = Rig::new("approval").await;
    let policy = rig.configure_policy(1).await;
    assert_eq!(policy.revision.get(), 1);
    let policies: CycleCountPolicyPage = response_json(
        rig.send(
            Method::GET,
            "/api/v1/cycle-count-policies?limit=100",
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(policies.items.len(), 1);

    let task_id = rig.create_task("create-policy-count").await;
    let claim = rig.claim(task_id, "claim-policy-count").await;
    assert_eq!(claim.task_id, task_id);
    let first = rig.confirm(task_id, 7, "confirm-policy-count").await;
    assert_eq!(first.disposition, CycleCountDisposition::RecountRequired);
    assert_eq!(first.variance_quantity, -3);
    assert_eq!(first.inventory_transaction_id, None);
    assert_eq!(rig.on_hand().await, 10);
    let variance_id = first.variance_id.unwrap();
    let recount_task_id = first.next_recount_task_id.unwrap();

    rig.claim(recount_task_id, "claim-automatic-recount").await;
    let recount = rig
        .confirm(recount_task_id, 6, "confirm-automatic-recount")
        .await;
    assert_eq!(recount.variance_id, Some(variance_id));
    assert_eq!(recount.disposition, CycleCountDisposition::ApprovalRequired);
    assert_eq!(recount.inventory_transaction_id, None);
    assert_eq!(rig.on_hand().await, 10);

    let variances: CycleCountVariancePage = response_json(
        expect_status(
            rig.send(
                Method::GET,
                "/api/v1/cycle-count-variances?limit=100&status=awaiting_approval",
                None,
                None,
            )
            .await,
            StatusCode::OK,
        )
        .await,
    )
    .await;
    let variance = variances
        .items
        .iter()
        .find(|variance| variance.variance_id == variance_id)
        .unwrap();
    assert_eq!(variance.status, CycleCountVarianceStatus::AwaitingApproval);
    assert_eq!(variance.system_quantity, 10);
    assert_eq!(variance.counted_quantity, 6);
    assert_eq!(variance.variance_quantity, -4);
    assert_eq!(variance.automatic_recounts_used, 1);

    expect_status(
        rig.send(
            Method::POST,
            &format!("/api/v1/cycle-count-variances/{variance_id}/decisions"),
            Some("stale-count-decision"),
            Some(json!({
                "expected_revision": recount.variance_revision.unwrap().get() - 1,
                "decision": "approve_adjustment",
                "reason": "verified_physical_count"
            })),
        )
        .await,
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(rig.on_hand().await, 10);

    let request = json!({
        "expected_revision": recount.variance_revision.unwrap().get(),
        "decision": "approve_adjustment",
        "reason": "verified_physical_count",
        "note": "Second blind count verified"
    });
    let approved: DecideCycleCountVarianceResponse = response_json(
        expect_status(
            rig.send(
                Method::POST,
                &format!("/api/v1/cycle-count-variances/{variance_id}/decisions"),
                Some("approve-count-variance"),
                Some(request.clone()),
            )
            .await,
            StatusCode::OK,
        )
        .await,
    )
    .await;
    let replay: DecideCycleCountVarianceResponse = response_json(
        rig.send(
            Method::POST,
            &format!("/api/v1/cycle-count-variances/{variance_id}/decisions"),
            Some("approve-count-variance"),
            Some(request),
        )
        .await,
    )
    .await;
    assert_eq!(replay, approved);
    assert_eq!(approved.status, CycleCountVarianceStatus::Posted);
    assert!(approved.inventory_transaction_id.is_some());
    assert_eq!(rig.on_hand().await, 6);

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let effects: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM cycle_count_variance_decisions WHERE variance_case_id=$1),
          (SELECT COUNT(*) FROM inventory_transactions
           WHERE reference_type='cycle_count_variance_case' AND reference_id=$1),
          (SELECT COUNT(*) FROM inventory_entries entry
           JOIN inventory_transactions journal ON journal.tenant_id=entry.tenant_id
            AND journal.inventory_owner_id=entry.inventory_owner_id
            AND journal.id=entry.transaction_id
           WHERE journal.reference_type='cycle_count_variance_case' AND journal.reference_id=$1)
        "#,
    )
    .bind(variance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(effects, (1, 1, 1));
}

#[tokio::test]
async fn supervisor_can_request_an_extra_blind_recount() {
    let rig = Rig::new("manual-recount").await;
    rig.configure_policy(0).await;
    let task_id = rig.create_task("create-manual-recount-count").await;
    rig.claim(task_id, "claim-manual-recount-count").await;
    let first = rig
        .confirm(task_id, 7, "confirm-manual-recount-count")
        .await;
    assert_eq!(first.disposition, CycleCountDisposition::ApprovalRequired);
    let variance_id = first.variance_id.unwrap();
    let requested: DecideCycleCountVarianceResponse = response_json(
        expect_status(
            rig.send(
                Method::POST,
                &format!("/api/v1/cycle-count-variances/{variance_id}/decisions"),
                Some("request-extra-recount"),
                Some(json!({
                    "expected_revision": first.variance_revision.unwrap().get(),
                    "decision": "request_recount",
                    "reason": "suspected_miscount"
                })),
            )
            .await,
            StatusCode::OK,
        )
        .await,
    )
    .await;
    assert_eq!(requested.status, CycleCountVarianceStatus::AwaitingRecount);
    let recount_task = requested.next_task_id.unwrap();
    rig.claim(recount_task, "claim-extra-recount").await;
    let recount = rig.confirm(recount_task, 9, "confirm-extra-recount").await;
    assert_eq!(recount.disposition, CycleCountDisposition::Posted);
    assert!(recount.inventory_transaction_id.is_some());
    assert_eq!(rig.on_hand().await, 9);
}

#[tokio::test]
async fn variance_ledgers_are_rls_governed_immutable_and_replay_concealed() {
    let rig = Rig::new("governance").await;
    rig.configure_policy(0).await;
    let task_id = rig.create_task("create-governed-count").await;
    rig.claim(task_id, "claim-governed-count").await;
    let count = rig.confirm(task_id, 5, "confirm-governed-count").await;
    let variance_id = count.variance_id.unwrap();
    let body = json!({
        "expected_revision": count.variance_revision.unwrap().get(),
        "decision": "approve_adjustment",
        "reason": "verified_physical_count"
    });
    let approved: DecideCycleCountVarianceResponse = response_json(
        expect_status(
            rig.send(
                Method::POST,
                &format!("/api/v1/cycle-count-variances/{variance_id}/decisions"),
                Some("governed-count-decision"),
                Some(body.clone()),
            )
            .await,
            StatusCode::OK,
        )
        .await,
    )
    .await;

    let app_db = app_db_for(&rig.fixture.db).await;
    let unbound: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM cycle_count_policies),
               (SELECT COUNT(*) FROM cycle_count_variance_cases),
               (SELECT COUNT(*) FROM cycle_count_variance_decisions)
        "#,
    )
    .fetch_one(&app_db)
    .await
    .unwrap();
    assert_eq!(unbound, (0, 0, 0));
    let grants: (bool, bool, bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT has_table_privilege(current_user, 'cycle_count_policies', 'SELECT'),
               has_table_privilege(current_user, 'cycle_count_policies', 'INSERT'),
               has_table_privilege(current_user, 'cycle_count_policies', 'DELETE'),
               has_table_privilege(current_user, 'cycle_count_variance_cases', 'DELETE'),
               has_table_privilege(current_user, 'cycle_count_variance_decisions', 'UPDATE'),
               has_table_privilege(current_user, 'cycle_count_variance_decisions', 'DELETE')
        "#,
    )
    .fetch_one(&app_db)
    .await
    .unwrap();
    assert_eq!(grants, (true, true, false, false, false, false));
    app_db.close().await;

    let admin = admin_db_for(&rig.fixture.db).await;
    let rls: Vec<(String, bool, bool)> = sqlx::query_as(
        r#"
        SELECT class.relname, class.relrowsecurity, class.relforcerowsecurity
        FROM pg_class class
        WHERE class.oid IN (
          'cycle_count_policies'::regclass,
          'cycle_count_variance_cases'::regclass,
          'cycle_count_variance_decisions'::regclass
        )
        ORDER BY class.relname
        "#,
    )
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(rls.len(), 3);
    assert!(rls.iter().all(|(_, enabled, forced)| *enabled && *forced));
    let policy_mutation =
        sqlx::query("UPDATE cycle_count_policies SET absolute_tolerance_qty=99 WHERE tenant_id=$1")
            .bind(rig.tenant_id.get())
            .execute(&admin)
            .await
            .expect_err("policy facts remain immutable to a privileged writer");
    assert_eq!(
        policy_mutation
            .as_database_error()
            .unwrap()
            .code()
            .as_deref(),
        Some("55000")
    );
    let decision_mutation = sqlx::query(
        "UPDATE cycle_count_variance_decisions SET reason_code='suspected_miscount' WHERE id=$1",
    )
    .bind(approved.decision_id)
    .execute(&admin)
    .await
    .expect_err("decision evidence remains immutable to a privileged writer");
    assert_eq!(
        decision_mutation
            .as_database_error()
            .unwrap()
            .code()
            .as_deref(),
        Some("55000")
    );
    admin.close().await;

    rig.set_scope(vec![rig.facility_id], Vec::new()).await;
    for replay_body in [
        body,
        json!({
            "expected_revision": count.variance_revision.unwrap().get(),
            "decision": "request_recount",
            "reason": "suspected_miscount"
        }),
    ] {
        expect_status(
            rig.send(
                Method::POST,
                &format!("/api/v1/cycle-count-variances/{variance_id}/decisions"),
                Some("governed-count-decision"),
                Some(replay_body),
            )
            .await,
            StatusCode::NOT_FOUND,
        )
        .await;
    }
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    serde_json::from_slice(&to_bytes(response.into_body(), 512 * 1024).await.unwrap()).unwrap()
}

async fn expect_status(
    response: axum::response::Response,
    expected: StatusCode,
) -> axum::response::Response {
    if response.status() != expected {
        let actual = response.status();
        let body = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
        panic!(
            "expected {expected}, got {actual}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    response
}

async fn grant_permission(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    permission_name: &str,
    role_name: &str,
) {
    let permission = match wareboxes_persistence_postgres::permissions::find_by_name(
        &fixture.db,
        tenant_id,
        permission_name,
    )
    .await
    .unwrap()
    {
        Some(permission) => permission.id,
        None => wareboxes_persistence_postgres::permissions::add_permission(
            &fixture.db,
            tenant_id,
            permission_name,
            Some(permission_name),
        )
        .await
        .unwrap(),
    };
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        role_name,
        Some("Cycle-count variance supervisor role"),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        permission,
    )
    .await
    .unwrap());
    assert!(wareboxes_persistence_postgres::roles::add_role_to_user(
        &fixture.db,
        tenant_id,
        user_id,
        role,
    )
    .await
    .unwrap());
}
