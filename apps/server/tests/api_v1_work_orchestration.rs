mod common;
#[path = "api_v1_work_orchestration/hardening.rs"]
mod hardening;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    OrchestrationPlanMode, OrchestrationSignalWorkspaceResponse, ResourceCapacitySignalResponse,
    WorkOrchestrationPlanPage, WorkOrchestrationPlanResponse, WorkOrchestrationPolicyPage,
    WorkOrchestrationPolicyResponse, WorkOrchestrationWorkerPage,
};

fn init_test_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("wareboxes_api=trace")
        .with_test_writer()
        .try_init();
}

struct Rig {
    fixture: Fixture,
    tenant_id: TenantId,
    user_id: i64,
    token: String,
    app: axum::Router,
    facility_id: i64,
    current_location_id: i64,
    near_location_id: i64,
    near_zone_id: i64,
    far_zone_id: i64,
    task_ids: [i64; 2],
}

impl Rig {
    async fn new() -> Self {
        let fixture = Fixture::new().await;
        let user = fixture
            .wms_user("orchestration-supervisor@test.local")
            .await;
        let tenant_id = tenant_for_user(&fixture.db, user.id).await;
        grant_supervisor(&fixture, tenant_id, user.id).await;
        let facility_id = fixture
            .facility(tenant_id, "Work orchestration facility")
            .await;
        let current_location_id = fixture
            .location(tenant_id, facility_id, "ORCH-CURRENT")
            .await;
        let near_location_id = fixture.location(tenant_id, facility_id, "ORCH-NEAR").await;
        let far_location_id = fixture.location(tenant_id, facility_id, "ORCH-FAR").await;
        let token = wareboxes_api::auth::create_session(&fixture.db, user.id)
            .await
            .unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));
        let mut rig = Self {
            fixture,
            tenant_id,
            user_id: user.id,
            token,
            app,
            facility_id,
            current_location_id,
            near_location_id,
            near_zone_id: 0,
            far_zone_id: 0,
            task_ids: [0, 0],
        };
        rig.configure_zone("ORCH-CUR", 1, current_location_id).await;
        rig.near_zone_id = rig.configure_zone("ORCH-NEAR", 2, near_location_id).await;
        rig.far_zone_id = rig.configure_zone("ORCH-FAR", 90, far_location_id).await;
        rig.task_ids = [
            rig.add_shared_count_task(
                near_location_id,
                "Near low-priority count",
                5,
                "20 minutes",
                "4 hours",
            )
            .await,
            rig.add_shared_count_task(
                far_location_id,
                "Far urgent count",
                100,
                "10 minutes",
                "1 hour",
            )
            .await,
        ];
        rig
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        key: Option<&str>,
        body: Option<Value>,
    ) -> axum::response::Response {
        send_request(
            self.app.clone(),
            &self.token,
            self.tenant_id,
            method,
            path,
            key,
            body,
        )
        .await
    }

    async fn configure_zone(&self, code: &str, sequence: u32, location_id: i64) -> i64 {
        let response = self
            .send(
                Method::POST,
                "/api/v1/storage-zones",
                Some(&format!("orchestration-zone-{code}")),
                Some(json!({
                    "facility_id":self.facility_id,
                    "code":code,
                    "name":format!("{code} zone"),
                    "purpose":"pick",
                    "travel_sequence":sequence,
                    "location_ids":[location_id]
                })),
            )
            .await;
        let status = response.status();
        let body: Value = json_response(response).await;
        assert_eq!(status, StatusCode::OK, "zone response: {body}");
        body["storage_zone_id"].as_i64().unwrap()
    }

    async fn add_shared_count_task(
        &self,
        location_id: i64,
        title: &str,
        priority: i64,
        age: &str,
        due_offset: &str,
    ) -> i64 {
        let mut tx = tenant_tx(&self.fixture.db, self.tenant_id).await;
        let task_id: i64 = sqlx::query_scalar(
            r#"INSERT INTO work_tasks (
              tenant_id,facility_id,created,task_type,status,required_permission,
              priority,title,due_at,task_timeout_seconds
            ) VALUES ($1,$2,transaction_timestamp()-$3::interval,
              'cycle_count_location','open','wms',$4,$5,
              transaction_timestamp()+$6::interval,1800) RETURNING id"#,
        )
        .bind(self.tenant_id.get())
        .bind(self.facility_id)
        .bind(age)
        .bind(priority)
        .bind(title)
        .bind(due_offset)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO cycle_count_location_tasks (tenant_id,task_id,facility_id,location_id) VALUES ($1,$2,$3,$4)",
        )
        .bind(self.tenant_id.get())
        .bind(task_id)
        .bind(self.facility_id)
        .bind(location_id)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        task_id
    }

    async fn add_scheduled_shared_count_task(
        &self,
        location_id: i64,
        title: &str,
        priority: i64,
        schedule_offset: &str,
    ) -> i64 {
        let mut tx = tenant_tx(&self.fixture.db, self.tenant_id).await;
        let task_id: i64 = sqlx::query_scalar(
            r#"INSERT INTO work_tasks (
              tenant_id,facility_id,created,task_type,status,required_permission,
              priority,title,scheduled_for,due_at,task_timeout_seconds
            ) VALUES ($1,$2,transaction_timestamp()-INTERVAL '1 minute',
              'cycle_count_location','open','wms',$3,$4,
              transaction_timestamp()+$5::interval,transaction_timestamp(),1800)
            RETURNING id"#,
        )
        .bind(self.tenant_id.get())
        .bind(self.facility_id)
        .bind(priority)
        .bind(title)
        .bind(schedule_offset)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO cycle_count_location_tasks (tenant_id,task_id,facility_id,location_id) VALUES ($1,$2,$3,$4)",
        )
        .bind(self.tenant_id.get())
        .bind(task_id)
        .bind(self.facility_id)
        .bind(location_id)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        task_id
    }

    fn policy_body(&self, mode: &str, expected_revision: Option<i64>) -> Value {
        json!({
            "facility_id":self.facility_id,
            "mode":mode,
            "priority_weight":20,
            "due_urgency_weight":30,
            "proximity_weight":10,
            "interleaving_weight":5,
            "congestion_penalty_weight":8,
            "bottleneck_penalty_weight":12,
            "due_horizon_minutes":120,
            "max_candidates":2,
            "expected_revision":expected_revision
        })
    }

    fn plan_body(&self, policy_id: i64, revision: i64) -> Value {
        json!({
            "facility_id":self.facility_id,
            "current_location_id":self.current_location_id,
            "expected_policy_id":policy_id,
            "expected_policy_revision":revision
        })
    }
}

async fn send_request(
    app: axum::Router,
    token: &str,
    tenant_id: TenantId,
    method: Method,
    path: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
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
    app.oneshot(request.body(body).unwrap()).await.unwrap()
}

async fn json_response<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "failed to decode {status} as {}: {error}; body={}",
            std::any::type_name::<T>(),
            String::from_utf8_lossy(&bytes)
        )
    })
}

async fn grant_supervisor(fixture: &Fixture, tenant_id: TenantId, user_id: i64) {
    let permission = match wareboxes_persistence_postgres::permissions::find_by_name(
        &fixture.db,
        tenant_id,
        "wms_supervisor",
    )
    .await
    .unwrap()
    {
        Some(permission) => permission.id,
        None => wareboxes_persistence_postgres::permissions::add_permission(
            &fixture.db,
            tenant_id,
            "wms_supervisor",
            Some("WMS supervisor"),
        )
        .await
        .unwrap(),
    };
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        "orchestration-supervisor-role",
        Some("Work orchestration supervisor tests"),
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        permission,
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_to_user(&fixture.db, tenant_id, user_id, role)
        .await
        .unwrap();
}

async fn bind_database_actor(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, user_id: i64) {
    sqlx::query("SELECT set_config('wareboxes.actor_user_id',$1,true)")
        .bind(user_id.to_string())
        .execute(&mut **tx)
        .await
        .unwrap();
}

async fn clone_plan_with_patch(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    source_plan_id: i64,
    patch: Value,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"INSERT INTO work_orchestration_plans OVERRIDING SYSTEM VALUE
        SELECT (jsonb_populate_record(NULL::public.work_orchestration_plans,
          to_jsonb(original)||jsonb_build_object(
            'id',nextval('work_orchestration_plans_id_seq'))||$3::jsonb)).*
        FROM work_orchestration_plans original
        WHERE original.tenant_id=$1 AND original.id=$2 RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(source_plan_id)
    .bind(patch)
    .fetch_one(&mut **tx)
    .await
}

#[tokio::test]
async fn plans_are_explainable_replay_safe_and_fall_back_to_manual_fifo() {
    init_test_tracing();
    let rig = Rig::new().await;
    let policy_body = rig.policy_body("enabled", None);
    let policy: WorkOrchestrationPolicyResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/work-orchestration/policies",
            Some("orchestration-policy-create"),
            Some(policy_body.clone()),
        )
        .await,
    )
    .await;
    assert_eq!(policy.revision.get(), 1);
    let replay: WorkOrchestrationPolicyResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/work-orchestration/policies",
            Some("orchestration-policy-create"),
            Some(policy_body.clone()),
        )
        .await,
    )
    .await;
    assert_eq!(replay, policy);
    let conflict = rig
        .send(
            Method::POST,
            "/api/v1/work-orchestration/policies",
            Some("orchestration-policy-create"),
            Some(json!({"facility_id":rig.facility_id,"mode":"disabled",
              "priority_weight":20,"due_urgency_weight":30,"proximity_weight":10,
              "interleaving_weight":5,"congestion_penalty_weight":8,
              "bottleneck_penalty_weight":12,"due_horizon_minutes":120,
              "max_candidates":100})),
        )
        .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let inventory_owner_id = rig
        .fixture
        .inventory_owner(rig.tenant_id, "Orchestration policy override client")
        .await;
    rig.fixture
        .assign_owner_to_facility(rig.tenant_id, inventory_owner_id, rig.facility_id)
        .await;
    let mut owner_policy_body = rig.policy_body("enabled", None);
    owner_policy_body["inventory_owner_id"] = json!(inventory_owner_id);
    let owner_policy: WorkOrchestrationPolicyResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/work-orchestration/policies",
            Some("orchestration-owner-policy-create"),
            Some(owner_policy_body),
        )
        .await,
    )
    .await;
    assert_eq!(owner_policy.revision.get(), policy.revision.get());
    assert_ne!(owner_policy.policy_id, policy.policy_id);
    let mut stale_facility_policy = rig.plan_body(policy.policy_id, policy.revision.get());
    stale_facility_policy["inventory_owner_id"] = json!(inventory_owner_id);
    let stale_facility_policy = rig
        .send(
            Method::POST,
            "/api/v1/work-orchestration/plans",
            Some("orchestration-owner-stale-facility-policy"),
            Some(stale_facility_policy),
        )
        .await;
    assert_eq!(stale_facility_policy.status(), StatusCode::CONFLICT);

    let near_signal = rig
        .send(
            Method::POST,
            "/api/v1/work-orchestration/signals/congestion",
            Some("orchestration-near-congestion"),
            Some(
                json!({"facility_id":rig.facility_id,"storage_zone_id":rig.near_zone_id,
              "congestion_basis_points":500,"queue_depth":1,"ttl_seconds":3600}),
            ),
        )
        .await;
    assert_eq!(near_signal.status(), StatusCode::OK);
    let far_signal = rig
        .send(
            Method::POST,
            "/api/v1/work-orchestration/signals/congestion",
            Some("orchestration-far-congestion"),
            Some(
                json!({"facility_id":rig.facility_id,"storage_zone_id":rig.far_zone_id,
              "congestion_basis_points":8000,"queue_depth":12,"ttl_seconds":3600}),
            ),
        )
        .await;
    assert_eq!(far_signal.status(), StatusCode::OK);
    let resource_signal = rig
        .send(
            Method::POST,
            "/api/v1/work-orchestration/signals/resources",
            Some("orchestration-inventory-control-capacity"),
            Some(
                json!({"facility_id":rig.facility_id,"resource_kind":"inventory_control",
              "available_units":2,"demand_units":5,"ttl_seconds":3600}),
            ),
        )
        .await;
    assert_eq!(resource_signal.status(), StatusCode::OK);
    let large_signal: ResourceCapacitySignalResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/work-orchestration/signals/resources",
            Some("orchestration-large-resource-capacity"),
            Some(json!({
              "facility_id":rig.facility_id,"resource_kind":"inventory_control",
              "available_units":i64::MAX,"demand_units":i64::MAX,"ttl_seconds":3600
            })),
        )
        .await,
    )
    .await;
    assert_eq!(large_signal.utilization_basis_points, 10_000);
    let signals: OrchestrationSignalWorkspaceResponse = json_response(
        rig.send(
            Method::GET,
            &format!(
                "/api/v1/work-orchestration/signals?facility_id={}",
                rig.facility_id
            ),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(signals.zone_signals.len(), 2);
    assert_eq!(signals.resource_signals.len(), 1);
    assert_eq!(signals.resource_signals[0].utilization_basis_points, 10_000);
    let signal_history: OrchestrationSignalWorkspaceResponse = json_response(
        rig.send(
            Method::GET,
            &format!(
                "/api/v1/work-orchestration/signals?facility_id={}&include_history=true&limit=1",
                rig.facility_id
            ),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(signal_history.zone_signals.len(), 1);
    assert_eq!(signal_history.resource_signals.len(), 1);
    let next_zone_cursor = signal_history.next_zone_cursor.as_ref().unwrap();
    let next_resource_cursor = signal_history.next_resource_cursor.as_ref().unwrap();
    let next_signal_history: OrchestrationSignalWorkspaceResponse = json_response(
        rig.send(
            Method::GET,
            &format!(
                "/api/v1/work-orchestration/signals?facility_id={}&include_history=true&limit=1&zone_cursor={}&resource_cursor={}",
                rig.facility_id,
                next_zone_cursor.as_str(),
                next_resource_cursor.as_str()
            ),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(next_signal_history.zone_signals.len(), 1);
    assert_eq!(next_signal_history.resource_signals.len(), 1);
    assert_ne!(
        next_signal_history.zone_signals[0].signal_id,
        signal_history.zone_signals[0].signal_id
    );
    assert_ne!(
        next_signal_history.resource_signals[0].signal_id,
        signal_history.resource_signals[0].signal_id
    );

    let late_high_score_task_id = rig
        .add_shared_count_task(
            rig.near_location_id,
            "Late high-score count",
            1_000,
            "1 second",
            "10 minutes",
        )
        .await;
    let future_scheduled_task_id = rig
        .add_scheduled_shared_count_task(
            rig.near_location_id,
            "Future scheduled high-priority count",
            2_000,
            "1 hour",
        )
        .await;

    let plan_body = rig.plan_body(policy.policy_id, 1);
    let plan: WorkOrchestrationPlanResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/work-orchestration/plans",
            Some("orchestration-plan-one"),
            Some(plan_body.clone()),
        )
        .await,
    )
    .await;
    assert_eq!(plan.plan_mode, OrchestrationPlanMode::Optimized);
    assert_eq!(plan.candidate_count, 2);
    assert_eq!(plan.item_count, 2);
    assert_eq!(plan.items.len(), 2);
    assert!(plan.items.iter().all(|item| {
        item.resource_signal_id.is_some()
            && item.zone_signal_id.is_some()
            && item.score.total
                == item.score.priority_component
                    + item.score.due_urgency_component
                    + item.score.proximity_component
                    + item.score.interleaving_component
                    - item.score.congestion_penalty
                    - item.score.bottleneck_penalty
    }));
    assert_eq!(plan.items[0].work_task_id, late_high_score_task_id);
    assert!(
        plan.items
            .iter()
            .any(|item| item.work_task_id == late_high_score_task_id),
        "scoring must precede the max-candidate truncation"
    );
    assert!(
        plan.items
            .iter()
            .all(|item| item.work_task_id != future_scheduled_task_id),
        "future-scheduled work must not enter the input snapshot"
    );
    let plan_replay: WorkOrchestrationPlanResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/work-orchestration/plans",
            Some("orchestration-plan-one"),
            Some(plan_body),
        )
        .await,
    )
    .await;
    assert_eq!(plan_replay, plan);
    let detail: WorkOrchestrationPlanResponse = json_response(
        rig.send(
            Method::GET,
            &format!("/api/v1/work-orchestration/plans/{}", plan.plan_id),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(detail, plan);
    let page: WorkOrchestrationPlanPage = json_response(
        rig.send(
            Method::GET,
            "/api/v1/work-orchestration/plans?limit=1",
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(page.items.len(), 1);
    assert!(page.next_cursor.is_none());

    let disabled: WorkOrchestrationPolicyResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/work-orchestration/policies",
            Some("orchestration-policy-disable"),
            Some(rig.policy_body("disabled", Some(1))),
        )
        .await,
    )
    .await;
    assert_eq!(disabled.revision.get(), 2);
    let fallback: WorkOrchestrationPlanResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/work-orchestration/plans",
            Some("orchestration-plan-fallback"),
            Some(rig.plan_body(disabled.policy_id, 2)),
        )
        .await,
    )
    .await;
    assert_eq!(fallback.plan_mode, OrchestrationPlanMode::ManualFifo);
    assert_eq!(fallback.items[0].work_task_id, rig.task_ids[0]);
    assert!(fallback.items.iter().all(|item| item.score.total == 0));
    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let open_tasks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM work_tasks WHERE tenant_id=$1 AND id=ANY($2) AND status='open'",
    )
    .bind(rig.tenant_id.get())
    .bind(rig.task_ids)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(open_tasks, 2, "advisory planning must never claim work");
    tx.rollback().await.unwrap();
}
