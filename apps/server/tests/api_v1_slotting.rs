mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{repo, routes, state::AppState};
use wareboxes_api_contract::v1::{
    SlottingProfileResponse, SlottingRecommendationPage, SlottingRecommendationResponse,
    SlottingRecommendationStatus, SlottingRunResponse,
};

#[derive(Debug, Clone, Copy)]
struct DemandItem {
    item_id: i64,
    source_balance_id: i64,
}

struct Rig {
    fixture: Fixture,
    tenant_id: TenantId,
    user_id: i64,
    token: String,
    app: axum::Router,
    inventory_owner_id: i64,
    facility_id: i64,
    reserve_location_id: i64,
    pick_location_id: i64,
    items: [DemandItem; 3],
}

fn init_test_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("wareboxes_api=trace,sqlx=warn")
        .with_test_writer()
        .try_init();
}

impl Rig {
    async fn new() -> Self {
        let fixture = Fixture::new().await;
        let user = fixture.wms_user("slotting-supervisor@test.local").await;
        let tenant_id = tenant_for_user(&fixture.db, user.id).await;
        grant_supervisor(&fixture, tenant_id, user.id).await;
        let inventory_owner_id = fixture.inventory_owner(tenant_id, "Slotting Client").await;
        let facility_id = fixture.facility(tenant_id, "Slotting Facility").await;
        fixture
            .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
            .await;
        let reserve_location_id = wareboxes_persistence_postgres::locations::add_location(
            &fixture.db,
            tenant_id,
            facility_id,
            None,
            Some("SLOT-RESERVE"),
            Some("Slotting reserve"),
            "reserve",
            true,
            false,
            false,
        )
        .await
        .unwrap();
        let pick_location_id = fixture.location(tenant_id, facility_id, "SLOT-PICK").await;
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
            inventory_owner_id,
            facility_id,
            reserve_location_id,
            pick_location_id,
            items: [
                DemandItem {
                    item_id: 1,
                    source_balance_id: 1,
                },
                DemandItem {
                    item_id: 1,
                    source_balance_id: 1,
                },
                DemandItem {
                    item_id: 1,
                    source_balance_id: 1,
                },
            ],
        };
        rig.configure_zone("SLOT-RES", "reserve", 100, reserve_location_id)
            .await;
        rig.configure_zone("SLOT-PICK", "pick", 10, pick_location_id)
            .await;
        let first = rig.create_demand_item("A", 9).await;
        let second = rig.create_demand_item("B", 7).await;
        let third = rig.create_demand_item("C", 5).await;
        rig.items = [first, second, third];
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

    async fn configure_zone(&self, code: &str, purpose: &str, sequence: u32, location_id: i64) {
        let response = self
            .send(
                Method::POST,
                "/api/v1/storage-zones",
                Some(&format!("slot-zone-{code}")),
                Some(json!({
                    "facility_id": self.facility_id,
                    "code": code,
                    "name": format!("{code} zone"),
                    "purpose": purpose,
                    "travel_sequence": sequence,
                    "location_ids": [location_id]
                })),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    async fn create_demand_item(&self, suffix: &str, demand: i64) -> DemandItem {
        let item_id = self
            .fixture
            .item(self.tenant_id, &format!("Slotting item {suffix}"), "case")
            .await;
        let batch_id = repo::inventory::add_item_batch(
            &self.fixture.db,
            self.tenant_id,
            self.inventory_owner_id,
            item_id,
            None,
            Some(&format!("SLOT-LOT-{suffix}")),
            None,
            None,
        )
        .await
        .unwrap();
        repo::inventory::receive_inventory(
            &self.fixture.db,
            self.tenant_id,
            self.user_id,
            batch_id,
            self.reserve_location_id,
            20,
            None,
            Some("slotting test stock"),
            None,
            None,
            &format!("slot-receive-{suffix}"),
        )
        .await
        .unwrap();
        let mut tx = tenant_tx(&self.fixture.db, self.tenant_id).await;
        let source_balance_id: i64 = sqlx::query_scalar(
            "SELECT id FROM inventory_balances WHERE tenant_id=$1 AND item_batch_id=$2 AND location_id=$3",
        )
        .bind(self.tenant_id.get())
        .bind(batch_id)
        .bind(self.reserve_location_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        let policy = self
            .send(
                Method::POST,
                "/api/v1/item-storage-policies",
                Some(&format!("slot-policy-{suffix}")),
                Some(json!({
                    "inventory_owner_id": self.inventory_owner_id,
                    "facility_id": self.facility_id,
                    "item_id": item_id,
                    "uom": "case",
                    "allowed_zone_purposes": ["reserve", "pick"],
                    "max_quantity_per_location": 100
                })),
            )
            .await;
        assert_eq!(policy.status(), StatusCode::OK);
        let order_id = self
            .fixture
            .order_header(
                self.tenant_id,
                &format!("SLOT-ORDER-{suffix}"),
                self.inventory_owner_id,
            )
            .await;
        self.fixture
            .order_item(self.tenant_id, order_id, item_id, demand)
            .await;
        self.fixture
            .reservation_for_balance(
                self.tenant_id,
                self.user_id,
                order_id,
                source_balance_id,
                demand,
                &format!("slot-demand-{suffix}"),
            )
            .await;
        DemandItem {
            item_id,
            source_balance_id,
        }
    }

    fn profile_body(&self, mode: &str, expected_revision: Option<i64>) -> Value {
        json!({
            "inventory_owner_id": self.inventory_owner_id,
            "facility_id": self.facility_id,
            "mode": mode,
            "demand_lookback_days": 30,
            "demand_weight": 10,
            "travel_weight": 5,
            "activity_weight": 2,
            "minimum_demand_quantity": 1,
            "max_recommendations": 100,
            "default_task_priority": 25,
            "expected_revision": expected_revision
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
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
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
        "slotting-supervisor-role",
        Some("Slotting supervisor tests"),
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

async fn add_membership(db: &db::Db, tenant_id: TenantId, user_id: i64) {
    let mut tx = tenant_tx(db, tenant_id).await;
    sqlx::query(
        "INSERT INTO tenant_memberships (tenant_id,user_id,is_default) VALUES ($1,$2,false)",
    )
    .bind(tenant_id.get())
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

async fn assert_forged_recommendation_rejected(
    admin: &db::Db,
    tenant_id: TenantId,
    recommendation_id: i64,
    actor_id: i64,
    patch: Value,
) {
    let mut tx = admin.begin().await.unwrap();
    sqlx::query("SELECT set_config('wareboxes.actor_user_id',$1,true)")
        .bind(actor_id.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let error = sqlx::query(
        r#"
        INSERT INTO slotting_recommendations OVERRIDING SYSTEM VALUE
        SELECT (jsonb_populate_record(
          NULL::public.slotting_recommendations,
          to_jsonb(original)||$3::jsonb||jsonb_build_object(
            'id',nextval('slotting_recommendations_id_seq'))
        )).*
        FROM slotting_recommendations original
        WHERE original.tenant_id=$1 AND original.id=$2
        "#,
    )
    .bind(tenant_id.get())
    .bind(recommendation_id)
    .bind(patch)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("recommendation evidence is invalid"),
        "unexpected forged recommendation error: {error}"
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn advisory_lifecycle_is_explainable_replay_safe_and_keeps_manual_execution_available() {
    init_test_tracing();
    let rig = Rig::new().await;
    let profile_body = rig.profile_body("enabled", None);
    let created: SlottingProfileResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/slotting/profiles",
            Some("slot-profile-create"),
            Some(profile_body.clone()),
        )
        .await,
    )
    .await;
    assert_eq!(created.revision.get(), 1);
    let replayed: SlottingProfileResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/slotting/profiles",
            Some("slot-profile-create"),
            Some(profile_body),
        )
        .await,
    )
    .await;
    assert_eq!(replayed, created);

    let run_body = json!({
        "inventory_owner_id": rig.inventory_owner_id,
        "facility_id": rig.facility_id,
        "expected_profile_revision": 1
    });
    let run: SlottingRunResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/slotting/runs",
            Some("slot-run-one"),
            Some(run_body.clone()),
        )
        .await,
    )
    .await;
    assert_eq!(run.candidate_count, 3);
    assert_eq!(run.recommendation_count, 3);
    let run_replay: SlottingRunResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/slotting/runs",
            Some("slot-run-one"),
            Some(run_body.clone()),
        )
        .await,
    )
    .await;
    assert_eq!(run_replay, run);

    let first_page: SlottingRecommendationPage = json_response(
        rig.send(
            Method::GET,
            "/api/v1/slotting/recommendations?status=pending&limit=1",
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(first_page.items.len(), 1);
    assert!(first_page.next_cursor.is_some());
    let cursor = first_page.next_cursor.as_ref().unwrap();
    let second_page: SlottingRecommendationPage = json_response(
        rig.send(
            Method::GET,
            &format!("/api/v1/slotting/recommendations?status=pending&limit=1&cursor={cursor}"),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(second_page.items.len(), 1);
    assert_ne!(
        first_page.items[0].slotting_recommendation_id,
        second_page.items[0].slotting_recommendation_id
    );

    let all: SlottingRecommendationPage = json_response(
        rig.send(
            Method::GET,
            "/api/v1/slotting/recommendations?status=pending&limit=10",
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(all.items.len(), 3);
    for recommendation in &all.items {
        assert_eq!(recommendation.source_zone_code, "SLOT-RES");
        assert_eq!(recommendation.destination_zone_code, "SLOT-PICK");
        assert_eq!(recommendation.evidence.source_travel_sequence, 100);
        assert_eq!(recommendation.evidence.destination_travel_sequence, 10);
        assert_eq!(
            recommendation.evidence.destination_inbound_planned_quantity,
            0
        );
        assert_eq!(
            recommendation.score.total,
            recommendation.score.demand_component
                + recommendation.score.travel_component
                + recommendation.score.activity_component
        );
        assert_eq!(recommendation.status, SlottingRecommendationStatus::Pending);
    }

    let first = all
        .items
        .iter()
        .find(|recommendation| recommendation.item_id == rig.items[0].item_id)
        .unwrap();
    let accepted: SlottingRecommendationResponse = json_response(
        rig.send(
            Method::POST,
            &format!(
                "/api/v1/slotting/recommendations/{}/acceptances",
                first.slotting_recommendation_id
            ),
            Some("slot-accept-one"),
            Some(json!({"expected_revision":1,"instructions":"Move to forward pick"})),
        )
        .await,
    )
    .await;
    assert_eq!(accepted.status, SlottingRecommendationStatus::Accepted);
    assert!(accepted.inventory_relocation_task_id.is_some());
    let accepted_replay: SlottingRecommendationResponse = json_response(
        rig.send(
            Method::POST,
            &format!(
                "/api/v1/slotting/recommendations/{}/acceptances",
                first.slotting_recommendation_id
            ),
            Some("slot-accept-one"),
            Some(json!({"expected_revision":1,"instructions":"Move to forward pick"})),
        )
        .await,
    )
    .await;
    assert_eq!(accepted_replay, accepted);

    let second = all
        .items
        .iter()
        .find(|recommendation| recommendation.item_id == rig.items[1].item_id)
        .unwrap();
    let accept_path = format!(
        "/api/v1/slotting/recommendations/{}/acceptances",
        second.slotting_recommendation_id
    );
    let (race_a, race_b) = tokio::join!(
        rig.send(
            Method::POST,
            &accept_path,
            Some("slot-accept-race-a"),
            Some(json!({"expected_revision":1}))
        ),
        rig.send(
            Method::POST,
            &accept_path,
            Some("slot-accept-race-b"),
            Some(json!({"expected_revision":1}))
        )
    );
    assert_eq!(
        usize::from(race_a.status() == StatusCode::OK)
            + usize::from(race_b.status() == StatusCode::OK),
        1
    );
    assert!(race_a.status() == StatusCode::CONFLICT || race_b.status() == StatusCode::CONFLICT);

    let third = all
        .items
        .iter()
        .find(|recommendation| recommendation.item_id == rig.items[2].item_id)
        .unwrap();
    let dismissed: SlottingRecommendationResponse = json_response(
        rig.send(
            Method::POST,
            &format!(
                "/api/v1/slotting/recommendations/{}/dismissals",
                third.slotting_recommendation_id
            ),
            Some("slot-dismiss-three"),
            Some(json!({
                "expected_revision":1,
                "reason":"operational_constraint",
                "note":"Forward location reserved for launch stock"
            })),
        )
        .await,
    )
    .await;
    assert_eq!(dismissed.status, SlottingRecommendationStatus::Dismissed);
    assert!(dismissed.inventory_relocation_task_id.is_none());

    let disabled: SlottingProfileResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/slotting/profiles",
            Some("slot-profile-disable"),
            Some(rig.profile_body("disabled", Some(1))),
        )
        .await,
    )
    .await;
    assert_eq!(disabled.revision.get(), 2);
    let disabled_run = rig
        .send(
            Method::POST,
            "/api/v1/slotting/runs",
            Some("slot-run-disabled"),
            Some(json!({
                "inventory_owner_id":rig.inventory_owner_id,
                "facility_id":rig.facility_id,
                "expected_profile_revision":2
            })),
        )
        .await;
    assert_eq!(disabled_run.status(), StatusCode::CONFLICT);

    let manual = rig
        .send(
            Method::POST,
            "/api/v1/inventory-relocation-tasks",
            Some("manual-while-slotting-disabled"),
            Some(json!({
                "work": {
                    "workflow":"loose_balance",
                    "source_inventory_balance_id":rig.items[2].source_balance_id,
                    "quantity":1
                },
                "destination_location_id":rig.pick_location_id,
                "instructions":"Safe manual fallback"
            })),
        )
        .await;
    assert_eq!(manual.status(), StatusCode::OK);

    let admin = admin_db_for(&rig.fixture.db).await;
    let source_quantity: i64 = sqlx::query_scalar(
        "SELECT qty_on_hand FROM inventory_balances WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(rig.items[0].source_balance_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        source_quantity, 20,
        "acceptance must not move stock directly"
    );
    let accepted_task: (String, String, i64) = sqlx::query_as(
        r#"
        SELECT task.task_type,detail.workflow,detail.source_inventory_balance_id
        FROM work_tasks task JOIN inventory_relocation_tasks detail
          ON detail.tenant_id=task.tenant_id AND detail.task_id=task.id
        WHERE task.tenant_id=$1 AND task.id=$2
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(accepted.inventory_relocation_task_id.unwrap())
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        accepted_task,
        (
            "inventory_relocation".into(),
            "loose_balance".into(),
            rig.items[0].source_balance_id
        )
    );
    admin.close().await;
}

#[tokio::test]
async fn slotting_rls_exact_grants_and_immutable_evidence_fail_closed() {
    init_test_tracing();
    let rig = Rig::new().await;
    let created: SlottingProfileResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/slotting/profiles",
            Some("slot-rls-profile"),
            Some(rig.profile_body("enabled", None)),
        )
        .await,
    )
    .await;
    let run: SlottingRunResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/slotting/runs",
            Some("slot-rls-run"),
            Some(json!({
                "inventory_owner_id":rig.inventory_owner_id,
                "facility_id":rig.facility_id,
                "expected_profile_revision":created.revision
            })),
        )
        .await,
    )
    .await;
    assert_eq!(run.recommendation_count, 3);

    let non_supervisor = rig.fixture.user("slotting-raw-worker@test.local").await;
    add_membership(&rig.fixture.db, rig.tenant_id, non_supervisor.id).await;

    let mut own_tenant = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let cross_tenant_target_id: i64 = sqlx::query_scalar(
        "SELECT id FROM slotting_recommendations WHERE slotting_run_id=$1 ORDER BY id LIMIT 1",
    )
    .bind(run.slotting_run_id)
    .fetch_one(&mut *own_tenant)
    .await
    .unwrap();
    own_tenant.rollback().await.unwrap();

    let other_user = rig
        .fixture
        .wms_user("slotting-other-tenant@test.local")
        .await;
    let other_tenant_id = tenant_for_user(&rig.fixture.db, other_user.id).await;
    grant_supervisor(&rig.fixture, other_tenant_id, other_user.id).await;
    let other_token = wareboxes_api::auth::create_session(&rig.fixture.db, other_user.id)
        .await
        .unwrap();
    let guessed_decision = send_request(
        rig.app.clone(),
        &other_token,
        other_tenant_id,
        Method::POST,
        &format!("/api/v1/slotting/recommendations/{cross_tenant_target_id}/dismissals"),
        Some("slotting-cross-tenant-guess"),
        Some(json!({
            "expected_revision":1,
            "reason":"stale_evidence"
        })),
    )
    .await;
    assert_eq!(guessed_decision.status(), StatusCode::NOT_FOUND);

    let mut other_tenant = tenant_tx(&rig.fixture.db, other_tenant_id).await;
    let cross_tenant_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM slotting_recommendations WHERE slotting_run_id=$1",
    )
    .bind(run.slotting_run_id)
    .fetch_one(&mut *other_tenant)
    .await
    .unwrap();
    assert_eq!(cross_tenant_count, 0);
    other_tenant.rollback().await.unwrap();

    let mut unbound = rig.fixture.db.begin().await.unwrap();
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM slotting_profiles),(SELECT count(*) FROM slotting_runs),(SELECT count(*) FROM slotting_recommendations)",
    )
    .fetch_one(&mut *unbound)
    .await
    .unwrap();
    assert_eq!(counts, (0, 0, 0));
    unbound.rollback().await.unwrap();

    let admin = admin_db_for(&rig.fixture.db).await;
    let grants: Vec<bool> = sqlx::query_scalar(
        r#"
        SELECT ARRAY[
          has_table_privilege('wareboxes_app','slotting_profiles','SELECT'),
          has_table_privilege('wareboxes_app','slotting_profiles','INSERT'),
          has_table_privilege('wareboxes_app','slotting_profiles','UPDATE'),
          has_column_privilege('wareboxes_app','slotting_profiles','effective_to','UPDATE'),
          has_column_privilege('wareboxes_app','slotting_profiles','mode','UPDATE'),
          has_table_privilege('wareboxes_app','slotting_runs','SELECT'),
          has_table_privilege('wareboxes_app','slotting_runs','INSERT'),
          has_table_privilege('wareboxes_app','slotting_runs','UPDATE'),
          has_table_privilege('wareboxes_app','slotting_recommendations','SELECT'),
          has_table_privilege('wareboxes_app','slotting_recommendations','INSERT'),
          has_table_privilege('wareboxes_app','slotting_recommendations','UPDATE'),
          has_column_privilege('wareboxes_app','slotting_recommendations','status','UPDATE'),
          has_column_privilege('wareboxes_app','slotting_recommendations','total_score','UPDATE'),
          has_sequence_privilege('wareboxes_app','slotting_profiles_id_seq','USAGE'),
          has_sequence_privilege('wareboxes_app','slotting_runs_id_seq','USAGE'),
          has_sequence_privilege('wareboxes_app','slotting_recommendations_id_seq','USAGE'),
          has_function_privilege('wareboxes_app',
            'public.slotting_destination_planned_quantity(bigint,bigint,bigint,bigint,bigint,text)',
            'EXECUTE'),
          has_table_privilege('wareboxes_app','slotting_profiles','DELETE'),
          has_table_privilege('wareboxes_app','slotting_runs','DELETE'),
          has_table_privilege('wareboxes_app','slotting_recommendations','DELETE')
        ]
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        grants,
        vec![
            true, true, false, true, false, true, true, false, true, true, false, true, false,
            true, true, true, true, false, false, false
        ]
    );
    let rls_is_forced: bool = sqlx::query_scalar(
        r#"
        SELECT bool_and(class.relrowsecurity AND class.relforcerowsecurity)
        FROM pg_class class
        WHERE class.oid=ANY(ARRAY[
          'slotting_profiles'::regclass,'slotting_runs'::regclass,
          'slotting_recommendations'::regclass])
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(rls_is_forced);
    let recommendation_id = cross_tenant_target_id;

    let mut actorless = admin.begin().await.unwrap();
    let actorless_error = sqlx::query(
        r#"
        INSERT INTO slotting_runs (
          tenant_id,inventory_owner_id,facility_id,slotting_profile_id,profile_revision,
          demand_window_started_at,input_snapshot_at,configuration_snapshot,
          candidate_count,recommendation_count,generated_by_user_id,generated_at)
        SELECT tenant_id,inventory_owner_id,facility_id,slotting_profile_id,profile_revision,
          transaction_timestamp()-make_interval(days=>(
            configuration_snapshot->'definition'->>'demand_lookback_days')::integer),
          transaction_timestamp(),configuration_snapshot,0,0,generated_by_user_id,
          transaction_timestamp()
        FROM slotting_runs WHERE tenant_id=$1 AND id=$2
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(run.slotting_run_id)
    .execute(&mut *actorless)
    .await
    .unwrap_err();
    assert!(
        actorless_error
            .to_string()
            .contains("configuration snapshot is invalid"),
        "unexpected actorless run error: {actorless_error}"
    );
    actorless.rollback().await.unwrap();

    let mut forged_snapshot = admin.begin().await.unwrap();
    sqlx::query("SELECT set_config('wareboxes.actor_user_id',$1,true)")
        .bind(rig.user_id.to_string())
        .execute(&mut *forged_snapshot)
        .await
        .unwrap();
    let forged_snapshot_error = sqlx::query(
        r#"
        INSERT INTO slotting_runs (
          tenant_id,inventory_owner_id,facility_id,slotting_profile_id,profile_revision,
          demand_window_started_at,input_snapshot_at,configuration_snapshot,
          candidate_count,recommendation_count,generated_by_user_id,generated_at)
        SELECT tenant_id,inventory_owner_id,facility_id,slotting_profile_id,profile_revision,
          transaction_timestamp()-make_interval(days=>(
            configuration_snapshot->'definition'->>'demand_lookback_days')::integer),
          transaction_timestamp(),
          jsonb_set(configuration_snapshot,'{definition,demand_weight}','999'::jsonb),
          0,0,generated_by_user_id,transaction_timestamp()
        FROM slotting_runs WHERE tenant_id=$1 AND id=$2
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(run.slotting_run_id)
    .execute(&mut *forged_snapshot)
    .await
    .unwrap_err();
    assert!(
        forged_snapshot_error
            .to_string()
            .contains("configuration snapshot is invalid"),
        "unexpected forged snapshot error: {forged_snapshot_error}"
    );
    forged_snapshot.rollback().await.unwrap();

    let mut unprivileged_profile_write = admin.begin().await.unwrap();
    sqlx::query("SELECT set_config('wareboxes.actor_user_id',$1,true)")
        .bind(non_supervisor.id.to_string())
        .execute(&mut *unprivileged_profile_write)
        .await
        .unwrap();
    let permission_error = sqlx::query(
        "UPDATE slotting_profiles SET effective_to=transaction_timestamp() WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(created.slotting_profile_id)
    .execute(&mut *unprivileged_profile_write)
    .await
    .unwrap_err();
    assert!(
        permission_error
            .to_string()
            .contains("only active slotting profile closure is allowed"),
        "unexpected unprivileged profile error: {permission_error}"
    );
    unprivileged_profile_write.rollback().await.unwrap();

    let mut successor_required = admin.begin().await.unwrap();
    sqlx::query("SELECT set_config('wareboxes.actor_user_id',$1,true)")
        .bind(rig.user_id.to_string())
        .execute(&mut *successor_required)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE slotting_profiles SET effective_to=transaction_timestamp() WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(created.slotting_profile_id)
    .execute(&mut *successor_required)
    .await
    .unwrap();
    let successor_error = successor_required.commit().await.unwrap_err();
    assert!(
        successor_error
            .to_string()
            .contains("profile closure requires an active successor"),
        "unexpected successor invariant error: {successor_error}"
    );

    for patch in [
        json!({"reason":"travel_reduction"}),
        json!({"source_zone_code":"FORGED"}),
        json!({"destination_on_hand":1}),
        json!({"destination_capacity":99}),
    ] {
        assert_forged_recommendation_rejected(
            &admin,
            rig.tenant_id,
            recommendation_id,
            rig.user_id,
            patch,
        )
        .await;
    }
    assert!(sqlx::query(
        "UPDATE slotting_recommendations SET total_score=total_score+1 WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(recommendation_id)
    .execute(&admin)
    .await
    .is_err());
    assert!(
        sqlx::query("DELETE FROM slotting_runs WHERE tenant_id=$1 AND id=$2")
            .bind(rig.tenant_id.get())
            .bind(run.slotting_run_id)
            .execute(&admin)
            .await
            .is_err()
    );
    let evidence: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT count(*) FROM command_idempotency_records WHERE tenant_id=$1
            AND operation='optimization.slotting.run.v1' AND idempotency_key='slot-rls-run'),
          (SELECT count(*) FROM outbox_events WHERE tenant_id=$1
            AND event_type='optimization.slotting.run.generated' AND aggregate_id=$2::text),
          (SELECT count(*) FROM slotting_recommendations WHERE tenant_id=$1 AND slotting_run_id=$2)
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(run.slotting_run_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(evidence, (1, 1, 3));
    admin.close().await;
}

#[tokio::test]
async fn concurrent_acceptances_cannot_overcommit_destination_capacity() {
    init_test_tracing();
    let rig = Rig::new().await;
    let second_reserve_location_id = wareboxes_persistence_postgres::locations::add_location(
        &rig.fixture.db,
        rig.tenant_id,
        rig.facility_id,
        None,
        Some("SLOT-RESERVE-2"),
        Some("Slotting reserve two"),
        "reserve",
        true,
        false,
        false,
    )
    .await
    .unwrap();
    let zone_response = rig
        .send(
            Method::POST,
            "/api/v1/storage-zones",
            Some("slot-zone-reserve-expand"),
            Some(json!({
                "facility_id":rig.facility_id,
                "code":"SLOT-RES",
                "name":"SLOT-RES zone",
                "purpose":"reserve",
                "travel_sequence":100,
                "location_ids":[rig.reserve_location_id,second_reserve_location_id],
                "expected_revision":1
            })),
        )
        .await;
    assert_eq!(zone_response.status(), StatusCode::OK);

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let item_batch_id: i64 = sqlx::query_scalar(
        "SELECT item_batch_id FROM inventory_balances WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(rig.items[0].source_balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    repo::inventory::receive_inventory(
        &rig.fixture.db,
        rig.tenant_id,
        rig.user_id,
        item_batch_id,
        second_reserve_location_id,
        20,
        None,
        Some("second slotting source"),
        None,
        None,
        "slot-receive-capacity-race",
    )
    .await
    .unwrap();

    let policy_response = rig
        .send(
            Method::POST,
            "/api/v1/item-storage-policies",
            Some("slot-policy-capacity-race"),
            Some(json!({
                "inventory_owner_id":rig.inventory_owner_id,
                "facility_id":rig.facility_id,
                "item_id":rig.items[0].item_id,
                "uom":"case",
                "allowed_zone_purposes":["reserve","pick"],
                "max_quantity_per_location":25,
                "expected_revision":1
            })),
        )
        .await;
    assert_eq!(policy_response.status(), StatusCode::OK);

    let extra_order_id = rig
        .fixture
        .order_header(
            rig.tenant_id,
            "SLOT-CAPACITY-RACE-ORDER",
            rig.inventory_owner_id,
        )
        .await;
    rig.fixture
        .order_item(rig.tenant_id, extra_order_id, rig.items[0].item_id, 20)
        .await;
    rig.fixture
        .reservation_for_balance(
            rig.tenant_id,
            rig.user_id,
            extra_order_id,
            rig.items[0].source_balance_id,
            20,
            "slot-capacity-race-demand",
        )
        .await;

    let profile: SlottingProfileResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/slotting/profiles",
            Some("slot-capacity-profile"),
            Some(rig.profile_body("enabled", None)),
        )
        .await,
    )
    .await;
    let run_body = json!({
        "inventory_owner_id":rig.inventory_owner_id,
        "facility_id":rig.facility_id,
        "expected_profile_revision":profile.revision
    });
    let first_run: SlottingRunResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/slotting/runs",
            Some("slot-capacity-run-one"),
            Some(run_body.clone()),
        )
        .await,
    )
    .await;
    let second_run: SlottingRunResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/slotting/runs",
            Some("slot-capacity-run-two"),
            Some(run_body),
        )
        .await,
    )
    .await;

    let first_page: SlottingRecommendationPage = json_response(
        rig.send(
            Method::GET,
            &format!(
                "/api/v1/slotting/recommendations?slotting_run_id={}&limit=100",
                first_run.slotting_run_id
            ),
            None,
            None,
        )
        .await,
    )
    .await;
    let second_page: SlottingRecommendationPage = json_response(
        rig.send(
            Method::GET,
            &format!(
                "/api/v1/slotting/recommendations?slotting_run_id={}&limit=100",
                second_run.slotting_run_id
            ),
            None,
            None,
        )
        .await,
    )
    .await;
    let first = first_page
        .items
        .iter()
        .find(|recommendation| recommendation.item_id == rig.items[0].item_id)
        .unwrap();
    let second = second_page
        .items
        .iter()
        .find(|recommendation| recommendation.item_id == rig.items[0].item_id)
        .unwrap();
    assert_ne!(
        first.source_inventory_balance_id,
        second.source_inventory_balance_id
    );
    assert_eq!(first.recommended_quantity, 20);
    assert_eq!(second.recommended_quantity, 20);
    assert_eq!(first.evidence.destination_capacity, Some(25));
    assert_eq!(second.evidence.destination_capacity, Some(25));
    assert_eq!(first.evidence.destination_inbound_planned_quantity, 0);
    assert_eq!(second.evidence.destination_inbound_planned_quantity, 0);

    let first_path = format!(
        "/api/v1/slotting/recommendations/{}/acceptances",
        first.slotting_recommendation_id
    );
    let second_path = format!(
        "/api/v1/slotting/recommendations/{}/acceptances",
        second.slotting_recommendation_id
    );
    let (first_result, second_result) = tokio::join!(
        rig.send(
            Method::POST,
            &first_path,
            Some("slot-capacity-accept-one"),
            Some(json!({"expected_revision":1}))
        ),
        rig.send(
            Method::POST,
            &second_path,
            Some("slot-capacity-accept-two"),
            Some(json!({"expected_revision":1}))
        )
    );
    assert_eq!(
        usize::from(first_result.status() == StatusCode::OK)
            + usize::from(second_result.status() == StatusCode::OK),
        1
    );
    assert_eq!(
        usize::from(first_result.status() == StatusCode::CONFLICT)
            + usize::from(second_result.status() == StatusCode::CONFLICT),
        1
    );

    let admin = admin_db_for(&rig.fixture.db).await;
    let (planned_quantity, accepted_count, task_count): (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          public.slotting_destination_planned_quantity($1,$2,$3,$4,$5,'case'),
          (SELECT count(*) FROM slotting_recommendations
            WHERE tenant_id=$1 AND item_id=$5 AND destination_location_id=$4
              AND status='accepted'),
          (SELECT count(*) FROM inventory_relocation_tasks
            WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
              AND destination_location_id=$4 AND item_id=$5 AND uom='case'
              AND closed_at IS NULL)
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(rig.inventory_owner_id)
    .bind(rig.facility_id)
    .bind(rig.pick_location_id)
    .bind(rig.items[0].item_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!((planned_quantity, accepted_count, task_count), (20, 1, 1));
    assert!(planned_quantity <= 25);
    admin.close().await;
}
