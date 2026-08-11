mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    CreateInboundAsnResponse, ErrorReason, ErrorResponse, InboundAsnDetailResponse,
    InboundAsnExecutionStatus, InboundAsnPage, InboundAsnStatus, PlanInboundAsnLoadResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;

struct AsnFixture {
    fixture: Fixture,
    tenant_id: TenantId,
    actor_id: i64,
    facility_id: i64,
    owner_id: i64,
    dock_id: i64,
    item_id: i64,
    token: String,
}

fn init_test_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("wareboxes_api=debug")
        .with_test_writer()
        .try_init();
}

async fn fixture(email: &str) -> AsnFixture {
    init_test_tracing();
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user(email).await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let facility_id = fixture.facility(tenant_id, "ASN Distribution Center").await;
    let owner_id = fixture.inventory_owner(tenant_id, "ASN Client").await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;
    let dock_id = wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some("ASN-RECV-01"),
        Some("ASN receiving dock"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let item_id = fixture.item(tenant_id, "ASN canned goods", "case").await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        "INSERT INTO inventory_owner_items(tenant_id,created,inventory_owner_id,item_id) VALUES ($1,$2,$3,$4)",
    )
    .bind(tenant_id.get())
    .bind(db::now_iso())
    .bind(owner_id)
    .bind(item_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    AsnFixture {
        fixture,
        tenant_id,
        actor_id: operator.id,
        facility_id,
        owner_id,
        dock_id,
        item_id,
        token,
    }
}

fn command_request(context: &AsnFixture, path: &str, key: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/{path}"))
        .header(header::AUTHORIZATION, format!("Bearer {}", context.token))
        .header(TENANT_ID_HEADER, context.tenant_id.to_string())
        .header(IDEMPOTENCY_KEY_HEADER, key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn get_request(context: &AsnFixture, path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/{path}"))
        .header(header::AUTHORIZATION, format!("Bearer {}", context.token))
        .header(TENANT_ID_HEADER, context.tenant_id.to_string())
        .body(Body::empty())
        .unwrap()
}

fn create_body(context: &AsnFixture, number: &str, quantity: i64) -> Value {
    json!({
        "inventory_owner_id": context.owner_id,
        "facility_id": context.facility_id,
        "number": number,
        "supplier": "Northstar Foods",
        "expected_at": "2027-08-20T17:00:00Z",
        "lines": [{
            "item_id": context.item_id,
            "expected_quantity": quantity,
            "lot": "LOT-ASN-A",
            "serial": null,
            "expiration": "2028-08-20T00:00:00Z"
        }]
    })
}

fn plan_body(context: &AsnFixture, revision: i64) -> Value {
    json!({
        "expected_revision": revision,
        "receiving_location_id": context.dock_id,
        "carrier": "Parity Freight",
        "trailer_number": "TRL-ASN-1",
        "seal_number": "SEAL-ASN-1"
    })
}

async fn json_body<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn create_asn(context: &AsnFixture, number: &str) -> CreateInboundAsnResponse {
    let response = routes::app(AppState::new(context.fixture.db.clone()))
        .oneshot(command_request(
            context,
            "inbound-asns",
            &format!("create-{number}"),
            &create_body(context, number, 12),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    json_body(response).await
}

#[tokio::test]
async fn create_is_atomic_race_safe_replayable_and_immutable() {
    let context = fixture("asn-create@test.local").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let body = create_body(&context, "ASN-RACE-100", 12);
    let first = app.clone().oneshot(command_request(
        &context,
        "inbound-asns",
        "asn-race-a",
        &body,
    ));
    let second = app.clone().oneshot(command_request(
        &context,
        "inbound-asns",
        "asn-race-b",
        &body,
    ));
    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap();
    let second = second.unwrap();
    let (winner_key, winner_response) = if first.status() == StatusCode::OK {
        assert_eq!(second.status(), StatusCode::CONFLICT);
        ("asn-race-a", first)
    } else {
        assert_eq!(first.status(), StatusCode::CONFLICT);
        assert_eq!(second.status(), StatusCode::OK);
        ("asn-race-b", second)
    };
    let winner = json_body::<CreateInboundAsnResponse>(winner_response).await;
    assert_eq!(winner.status, InboundAsnStatus::Open);
    assert_eq!(winner.revision.get(), 1);
    assert_eq!(winner.lines.len(), 1);

    let replay = app
        .clone()
        .oneshot(command_request(&context, "inbound-asns", winner_key, &body))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(json_body::<CreateInboundAsnResponse>(replay).await, winner);
    let changed = app
        .oneshot(command_request(
            &context,
            "inbound-asns",
            winner_key,
            &create_body(&context, "ASN-RACE-100", 13),
        ))
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let effects: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM inbound_asns WHERE number='ASN-RACE-100'),
          (SELECT COUNT(*) FROM inbound_asn_lines WHERE asn_id=$1),
          (SELECT COUNT(*) FROM outbox_events WHERE event_type='inbound.asn.created'
             AND aggregate_id=$1::TEXT),
          (SELECT COUNT(*) FROM command_idempotency_records
             WHERE operation='inbound.asn.create.v1' AND (result_json->>'asn_id')::BIGINT=$1)
        "#,
    )
    .bind(winner.asn_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(effects, (1, 1, 1, 1));
    let immutable =
        sqlx::query("UPDATE inbound_asn_lines SET expected_quantity=99 WHERE asn_id=$1")
            .bind(winner.asn_id)
            .execute(&mut *tx)
            .await
            .unwrap_err();
    assert!(!immutable.to_string().is_empty());
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn planning_copies_the_exact_source_once_and_races_to_one_load() {
    let context = fixture("asn-plan@test.local").await;
    let created = create_asn(&context, "ASN-PLAN-100").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let body = plan_body(&context, created.revision.get());
    let first = app.clone().oneshot(command_request(
        &context,
        &format!("inbound-asns/{}/load-plans", created.asn_id),
        "asn-plan-a",
        &body,
    ));
    let second = app.clone().oneshot(command_request(
        &context,
        &format!("inbound-asns/{}/load-plans", created.asn_id),
        "asn-plan-b",
        &body,
    ));
    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap();
    let second = second.unwrap();
    let (winner_key, winner_response) = if first.status() == StatusCode::OK {
        assert_eq!(second.status(), StatusCode::CONFLICT);
        ("asn-plan-a", first)
    } else {
        assert_eq!(first.status(), StatusCode::CONFLICT);
        assert_eq!(second.status(), StatusCode::OK);
        ("asn-plan-b", second)
    };
    let winner = json_body::<PlanInboundAsnLoadResponse>(winner_response).await;
    assert_eq!(winner.asn_status, InboundAsnStatus::Planned);
    assert_eq!(winner.asn_revision.get(), 2);
    assert_eq!(winner.lines.len(), 1);
    let replay = app
        .clone()
        .oneshot(command_request(
            &context,
            &format!("inbound-asns/{}/load-plans", created.asn_id),
            winner_key,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        json_body::<PlanInboundAsnLoadResponse>(replay).await,
        winner
    );
    let stale = app
        .oneshot(command_request(
            &context,
            &format!("inbound-asns/{}/load-plans", created.asn_id),
            "asn-plan-stale",
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let detail = routes::app(AppState::new(context.fixture.db.clone()))
        .oneshot(get_request(
            &context,
            &format!("inbound-asns/{}", created.asn_id),
        ))
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    let detail = json_body::<InboundAsnDetailResponse>(detail).await;
    assert_eq!(detail.summary.load_id, Some(winner.load_id));
    assert_eq!(
        detail.summary.execution_status,
        Some(InboundAsnExecutionStatus::Planned)
    );
    assert_eq!(detail.summary.total_received_quantity, 0);
    assert_eq!(detail.summary.total_rejected_quantity, 0);
    assert_eq!(detail.summary.total_missing_quantity, 0);
    assert_eq!(detail.summary.total_remaining_quantity, 12);
    assert_eq!(detail.lines.len(), 1);
    assert_eq!(detail.lines[0].remaining_quantity, 12);

    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let evidence: (i64, i64, i64, i64, i64, i64, String) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM inbound_asn_load_plans WHERE asn_id=$1),
          (SELECT COUNT(*) FROM inbound_asn_load_plan_lines WHERE asn_id=$1),
          (SELECT COUNT(*) FROM loads WHERE id=$2 AND type='inbound' AND status='planned'),
          (SELECT COUNT(*) FROM load_lines WHERE load_id=$2 AND expected_qty=12),
          (SELECT COUNT(*) FROM outbox_events WHERE event_type='inbound.asn.load_planned'
             AND aggregate_id=$1::TEXT),
          (SELECT COUNT(*) FROM outbox_events WHERE event_type='inbound.load.planned'
             AND aggregate_id=$2::TEXT),
          (SELECT reference_number FROM loads WHERE id=$2)
        "#,
    )
    .bind(created.asn_id)
    .bind(winner.load_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(evidence, (1, 1, 1, 1, 1, 1, "ASN-PLAN-100".into()));
    let bypass = sqlx::query("UPDATE inbound_asns SET revision=revision+1 WHERE id=$1")
        .bind(created.asn_id)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(bypass.to_string().contains("typed load planning"));
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    sqlx::query("UPDATE loads SET status='receiving' WHERE id=$1")
        .bind(winner.load_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE load_lines SET received_qty=5,rejected_qty=2,missing_qty=1,missing_confirmed_by=$1,missing_confirmed_at=$2,status='partial' WHERE id=$3",
    )
    .bind(context.actor_id)
    .bind(db::now_iso())
    .bind(winner.lines[0].load_line_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let progress = routes::app(AppState::new(context.fixture.db.clone()))
        .oneshot(get_request(
            &context,
            &format!("inbound-asns/{}", created.asn_id),
        ))
        .await
        .unwrap();
    assert_eq!(progress.status(), StatusCode::OK);
    let progress = json_body::<InboundAsnDetailResponse>(progress).await;
    assert_eq!(
        progress.summary.execution_status,
        Some(InboundAsnExecutionStatus::Receiving)
    );
    assert_eq!(progress.summary.total_received_quantity, 5);
    assert_eq!(progress.summary.total_rejected_quantity, 2);
    assert_eq!(progress.summary.total_missing_quantity, 1);
    assert_eq!(progress.summary.total_remaining_quantity, 4);
    assert_eq!(progress.lines[0].received_quantity, 5);
    assert_eq!(progress.lines[0].rejected_quantity, 2);
    assert_eq!(progress.lines[0].missing_quantity, 1);
    assert_eq!(progress.lines[0].remaining_quantity, 4);
}

#[tokio::test]
async fn queue_cursors_and_replays_are_scope_bound_with_minimal_rls_grants() {
    let context = fixture("asn-scope@test.local").await;
    assert!(repo::tenants::update_user_access_scope(
        &context.fixture.db,
        context.tenant_id,
        &UpdateUserAccessScope {
            user_id: context.actor_id,
            all_facilities: false,
            facility_ids: vec![context.facility_id],
            all_inventory_owners: false,
            inventory_owner_ids: vec![context.owner_id],
        },
    )
    .await
    .unwrap());
    let first = create_asn(&context, "ASN-PAGE-100").await;
    let _second = create_asn(&context, "ASN-PAGE-101").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let page = app
        .clone()
        .oneshot(get_request(&context, "inbound-asns?status=open&limit=1"))
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let page = json_body::<InboundAsnPage>(page).await;
    assert_eq!(page.items.len(), 1);
    let cursor = page.next_cursor.unwrap();
    let next = app
        .clone()
        .oneshot(get_request(
            &context,
            &format!("inbound-asns?status=open&limit=1&cursor={cursor}"),
        ))
        .await
        .unwrap();
    assert_eq!(next.status(), StatusCode::OK);
    assert_eq!(json_body::<InboundAsnPage>(next).await.items.len(), 1);
    let mismatched = app
        .clone()
        .oneshot(get_request(
            &context,
            &format!("inbound-asns?status=planned&limit=1&cursor={cursor}"),
        ))
        .await
        .unwrap();
    assert_eq!(mismatched.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body::<ErrorResponse>(mismatched).await.reason,
        ErrorReason::InvalidCursor
    );

    assert!(repo::tenants::update_user_access_scope(
        &context.fixture.db,
        context.tenant_id,
        &UpdateUserAccessScope {
            user_id: context.actor_id,
            all_facilities: false,
            facility_ids: vec![],
            all_inventory_owners: false,
            inventory_owner_ids: vec![],
        },
    )
    .await
    .unwrap());
    for body in [
        create_body(&context, "ASN-PAGE-100", 12),
        create_body(&context, "ASN-PAGE-100", 13),
    ] {
        let hidden = app
            .clone()
            .oneshot(command_request(
                &context,
                "inbound-asns",
                "create-ASN-PAGE-100",
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    }
    let hidden_detail = app
        .oneshot(get_request(
            &context,
            &format!("inbound-asns/{}", first.asn_id),
        ))
        .await
        .unwrap();
    assert_eq!(hidden_detail.status(), StatusCode::NOT_FOUND);

    let admin = admin_db_for(&context.fixture.db).await;
    for table in [
        "inbound_asns",
        "inbound_asn_lines",
        "inbound_asn_load_plans",
        "inbound_asn_load_plan_lines",
    ] {
        let forced: bool =
            sqlx::query_scalar("SELECT relforcerowsecurity FROM pg_class WHERE oid=$1::regclass")
                .bind(format!("public.{table}"))
                .fetch_one(&admin)
                .await
                .unwrap();
        assert!(forced);
        let can_delete: bool =
            sqlx::query_scalar("SELECT has_table_privilege('wareboxes_app',$1,'DELETE')")
                .bind(format!("public.{table}"))
                .fetch_one(&admin)
                .await
                .unwrap();
        assert!(!can_delete);
    }
    let line_update: bool = sqlx::query_scalar(
        "SELECT has_table_privilege('wareboxes_app','public.inbound_asn_lines','UPDATE')",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(!line_update);
    admin.close().await;
}
