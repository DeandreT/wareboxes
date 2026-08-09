mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::{IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    ErrorReason, ErrorResponse, OrderAllocationOutcome, PickWavePage, PickWaveResponse,
    PickWaveStatus, PlanOrderAllocationRequest, PlanOrderAllocationResponse, Revision,
};
use wareboxes_core::dto::UpdateUserAccessScope;

fn request(
    token: &str,
    tenant_id: TenantId,
    method: Method,
    path: &str,
    idempotency_key: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    if let Some(key) = idempotency_key {
        request = request
            .header(IDEMPOTENCY_KEY_HEADER, key)
            .header(REQUEST_ID_HEADER, format!("request-{key}"));
    }
    let body = match body {
        Some(value) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    request.body(body).unwrap()
}

async fn send(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    method: Method,
    path: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> axum::response::Response {
    app.clone()
        .oneshot(request(token, tenant_id, method, path, key, body))
        .await
        .unwrap()
}

async fn json_response<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let bytes = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn expect(
    response: axum::response::Response,
    status: StatusCode,
    operation: &str,
) -> axum::response::Response {
    if response.status() != status {
        let actual = response.status();
        let body: Value = json_response(response).await;
        panic!("{operation}: expected {status}, got {actual}: {body}");
    }
    response
}

async fn grant_permissions(db: &db::Db, tenant_id: TenantId, user_id: i64, suffix: &str) {
    let role = wareboxes_persistence_postgres::roles::add_role(
        db,
        tenant_id,
        &format!("pick-wave-{suffix}"),
        Some("Plan and release pick waves"),
    )
    .await
    .unwrap();
    for name in ["orders", "wms_supervisor"] {
        let permission =
            match wareboxes_persistence_postgres::permissions::find_by_name(db, tenant_id, name)
                .await
                .unwrap()
            {
                Some(permission) => permission.id,
                None => wareboxes_persistence_postgres::permissions::add_permission(
                    db,
                    tenant_id,
                    name,
                    Some("Pick wave workflow"),
                )
                .await
                .unwrap(),
            };
        wareboxes_persistence_postgres::roles::add_role_permission(db, tenant_id, role, permission)
            .await
            .unwrap();
    }
    wareboxes_persistence_postgres::roles::add_role_to_user(db, tenant_id, user_id, role)
        .await
        .unwrap();
}

async fn staging_location(
    fixture: &Fixture,
    tenant_id: TenantId,
    facility_id: i64,
    barcode: &str,
) -> i64 {
    wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some(barcode),
        Some(barcode),
        "staging",
        true,
        false,
        false,
    )
    .await
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
async fn allocated_order(
    fixture: &Fixture,
    app: &axum::Router,
    token: &str,
    access: &wareboxes_core::models::TenantAccess,
    owner_id: i64,
    facility_id: i64,
    key: &str,
    quantity: i64,
) -> (i64, i64) {
    let item_id = fixture
        .item(access.tenant_id, &format!("{key} item"), "each")
        .await;
    repo::items::add_barcode(
        &fixture.db,
        access.tenant_id,
        item_id,
        &format!("{key}-ITEM"),
        "code128",
        None,
    )
    .await
    .unwrap();
    let order_id = fixture.order_header(access.tenant_id, key, owner_id).await;
    fixture
        .order_item(access.tenant_id, order_id, item_id, quantity)
        .await;
    fixture
        .received_balance(
            access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id,
                qty: quantity,
                key,
            },
        )
        .await;
    let body = serde_json::to_value(PlanOrderAllocationRequest {
        facility_id,
        expected_revision: Revision::new(1).unwrap(),
        strategy: wareboxes_api_contract::v1::OrderAllocationStrategy::Fefo,
    })
    .unwrap();
    let response = expect(
        send(
            app,
            token,
            access.tenant_id,
            Method::POST,
            &format!("/api/v1/orders/{order_id}/allocation-runs"),
            Some(&format!("allocate-{key}")),
            Some(body),
        )
        .await,
        StatusCode::OK,
        "allocate order",
    )
    .await;
    let result: PlanOrderAllocationResponse = json_response(response).await;
    assert_eq!(result.outcome, OrderAllocationOutcome::FullyAllocated);
    (order_id, result.revision.get())
}

fn plan_body(facility_id: i64, destination_id: i64, name: &str, orders: &[(i64, i64)]) -> Value {
    json!({
        "facility_id": facility_id,
        "destination_location_id": destination_id,
        "name": name,
        "orders": orders.iter().enumerate().map(|(index, (order_id, revision))| json!({
            "order_id": order_id,
            "expected_revision": revision,
            "sequence": index + 1,
        })).collect::<Vec<_>>()
    })
}

#[tokio::test]
async fn two_order_wave_plans_replays_and_releases_atomically() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("pick-wave-release@test.local").await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    grant_permissions(&fixture.db, access.tenant_id, user.id, "release").await;
    let owner = fixture
        .inventory_owner(access.tenant_id, "Wave Release Owner")
        .await;
    let facility = fixture
        .facility(access.tenant_id, "Wave Release Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner, facility)
        .await;
    let destination = staging_location(&fixture, access.tenant_id, facility, "WAVE-STAGE").await;
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let first = allocated_order(
        &fixture,
        &app,
        &token,
        &access,
        owner,
        facility,
        "WAVE-ORDER-A",
        3,
    )
    .await;
    let second = allocated_order(
        &fixture,
        &app,
        &token,
        &access,
        owner,
        facility,
        "WAVE-ORDER-B",
        5,
    )
    .await;
    let body = plan_body(facility, destination, "Morning parcel", &[first, second]);
    let planned_response = expect(
        send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            "/api/v1/pick-waves",
            Some("wave-plan"),
            Some(body.clone()),
        )
        .await,
        StatusCode::OK,
        "plan wave",
    )
    .await;
    let planned: PickWaveResponse = json_response(planned_response).await;
    assert_eq!(planned.status, PickWaveStatus::Planned);
    assert_eq!(planned.order_count, 2);
    assert_eq!(planned.revision.get(), 1);
    let replay: PickWaveResponse = json_response(
        expect(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                "/api/v1/pick-waves",
                Some("wave-plan"),
                Some(body),
            )
            .await,
            StatusCode::OK,
            "replay plan",
        )
        .await,
    )
    .await;
    assert_eq!(replay, planned);

    let direct = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{}/releases", first.0),
        Some("direct-release-blocked"),
        Some(json!({
            "facility_id": facility,
            "destination_location_id": destination,
            "expected_revision": first.1
        })),
    )
    .await;
    assert_eq!(direct.status(), StatusCode::CONFLICT);

    let release_body = json!({"expected_revision": 1});
    let released: PickWaveResponse = json_response(
        expect(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/pick-waves/{}/releases", planned.wave_id),
                Some("wave-release"),
                Some(release_body.clone()),
            )
            .await,
            StatusCode::OK,
            "release wave",
        )
        .await,
    )
    .await;
    assert_eq!(released.status, PickWaveStatus::Released);
    assert_eq!(released.revision.get(), 2);
    assert_eq!(released.allocation_count, 2);
    assert_eq!(released.pick_task_count, 2);
    assert_eq!(released.released_quantity, 8);
    assert!(released.orders.iter().all(|order| {
        order.status == "processing"
            && order.release_id.is_some()
            && order.resulting_revision == Some(Revision::new(3).unwrap())
    }));
    let release_replay: PickWaveResponse = json_response(
        expect(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/pick-waves/{}/releases", planned.wave_id),
                Some("wave-release"),
                Some(release_body),
            )
            .await,
            StatusCode::OK,
            "replay release",
        )
        .await,
    )
    .await;
    assert_eq!(release_replay, released);

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let effects: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"SELECT
             (SELECT COUNT(*) FROM order_releases WHERE pick_wave_id=$2 AND release_mode='wave'),
             (SELECT COUNT(*) FROM pick_wave_orders WHERE pick_wave_id=$2 AND NOT active),
             (SELECT COUNT(*) FROM orders WHERE id=ANY($3) AND status='processing' AND revision=3 AND wave_id=$2),
             (SELECT COUNT(*) FROM pick_tasks WHERE order_release_id IN
                (SELECT id FROM order_releases WHERE pick_wave_id=$2)),
             (SELECT COUNT(*) FROM outbox_events WHERE aggregate_type='pick_wave'
                AND aggregate_id=$2::text)
           FROM tenants WHERE id=$1"#,
    )
    .bind(access.tenant_id.get())
    .bind(planned.wave_id)
    .bind([first.0, second.0])
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(effects, (2, 2, 2, 2, 2));

    let page: PickWavePage = json_response(
        expect(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::GET,
                &format!("/api/v1/pick-waves?facility_id={facility}&status=released&limit=1"),
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "list waves",
        )
        .await,
    )
    .await;
    assert_eq!(page.items, vec![released]);
}

#[tokio::test]
async fn competing_plans_have_one_winner_and_cancel_replay_is_scope_concealed() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("pick-wave-race@test.local").await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    grant_permissions(&fixture.db, access.tenant_id, user.id, "race").await;
    let owner = fixture
        .inventory_owner(access.tenant_id, "Wave Race Owner")
        .await;
    let facility = fixture
        .facility(access.tenant_id, "Wave Race Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner, facility)
        .await;
    let destination =
        staging_location(&fixture, access.tenant_id, facility, "WAVE-RACE-STAGE").await;
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let order = allocated_order(
        &fixture,
        &app,
        &token,
        &access,
        owner,
        facility,
        "WAVE-RACE-ORDER",
        4,
    )
    .await;
    let body = plan_body(facility, destination, "Race wave", &[order]);
    let first = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/pick-waves",
        Some("wave-race-a"),
        Some(body.clone()),
    );
    let second = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/pick-waves",
        Some("wave-race-b"),
        Some(body),
    );
    let (first, second) = tokio::join!(first, second);
    let (winner, loser) = match (first.status(), second.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => (first, second),
        (StatusCode::CONFLICT, StatusCode::OK) => (second, first),
        actual => panic!("expected one wave winner and one conflict, got {actual:?}"),
    };
    let planned: PickWaveResponse = json_response(winner).await;
    let conflict: ErrorResponse = json_response(loser).await;
    assert_eq!(conflict.reason, ErrorReason::Conflict);

    let cancel_body = json!({
        "expected_revision": 1,
        "reason": "operational_change"
    });
    let cancelled: PickWaveResponse = json_response(
        expect(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/pick-waves/{}/cancellations", planned.wave_id),
                Some("wave-cancel"),
                Some(cancel_body.clone()),
            )
            .await,
            StatusCode::OK,
            "cancel wave",
        )
        .await,
    )
    .await;
    assert_eq!(cancelled.status, PickWaveStatus::Cancelled);
    assert_eq!(cancelled.revision.get(), 2);
    let changed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/pick-waves/{}/cancellations", planned.wave_id),
        Some("wave-cancel"),
        Some(json!({
            "expected_revision": 1,
            "reason": "capacity_constraint"
        })),
    )
    .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);

    let page_order = allocated_order(
        &fixture,
        &app,
        &token,
        &access,
        owner,
        facility,
        "WAVE-PAGE-ORDER",
        2,
    )
    .await;
    let page_wave: PickWaveResponse = json_response(
        expect(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                "/api/v1/pick-waves",
                Some("wave-page-plan"),
                Some(plan_body(
                    facility,
                    destination,
                    "Pagination wave",
                    &[page_order],
                )),
            )
            .await,
            StatusCode::OK,
            "plan pagination wave",
        )
        .await,
    )
    .await;
    let first_page: PickWavePage = json_response(
        expect(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::GET,
                &format!("/api/v1/pick-waves?facility_id={facility}&limit=1"),
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "first wave page",
        )
        .await,
    )
    .await;
    assert_eq!(first_page.items[0].wave_id, page_wave.wave_id);
    let cursor = first_page.next_cursor.unwrap();
    let second_page: PickWavePage = json_response(
        expect(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::GET,
                &format!(
                    "/api/v1/pick-waves?facility_id={facility}&limit=1&cursor={}",
                    cursor.as_str()
                ),
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "second wave page",
        )
        .await,
    )
    .await;
    assert_eq!(second_page.items[0].wave_id, cancelled.wave_id);
    let name_sorted: PickWavePage = json_response(
        expect(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::GET,
                &format!(
                    "/api/v1/pick-waves?facility_id={facility}&sort=name&direction=desc&limit=1"
                ),
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "globally sorted wave page",
        )
        .await,
    )
    .await;
    assert_eq!(name_sorted.items[0].name, "Race wave");
    let mismatched = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &format!(
            "/api/v1/pick-waves?facility_id={facility}&status=cancelled&limit=1&cursor={}",
            cursor.as_str()
        ),
        None,
        None,
    )
    .await;
    assert_eq!(mismatched.status(), StatusCode::BAD_REQUEST);

    let other_user = fixture.wms_user("pick-wave-cross-tenant@test.local").await;
    let other_access = default_tenant_for_user(&fixture.db, other_user.id)
        .await
        .unwrap();
    grant_permissions(
        &fixture.db,
        other_access.tenant_id,
        other_user.id,
        "cross-tenant",
    )
    .await;
    let other_token = auth::create_session(&fixture.db, other_user.id)
        .await
        .unwrap();
    let guessed = send(
        &app,
        &other_token,
        other_access.tenant_id,
        Method::GET,
        &format!("/api/v1/pick-waves/{}", planned.wave_id),
        None,
        None,
    )
    .await;
    assert_eq!(guessed.status(), StatusCode::NOT_FOUND);

    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        access.tenant_id,
        &UpdateUserAccessScope {
            user_id: user.id,
            all_facilities: false,
            facility_ids: vec![],
            all_inventory_owners: false,
            inventory_owner_ids: vec![],
        },
    )
    .await
    .unwrap());
    let concealed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/pick-waves/{}/cancellations", planned.wave_id),
        Some("wave-cancel"),
        Some(cancel_body),
    )
    .await;
    assert_eq!(concealed.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn stale_member_rolls_back_the_entire_wave_release() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("pick-wave-rollback@test.local").await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    grant_permissions(&fixture.db, access.tenant_id, user.id, "rollback").await;
    let owner = fixture
        .inventory_owner(access.tenant_id, "Wave Rollback Owner")
        .await;
    let facility = fixture
        .facility(access.tenant_id, "Wave Rollback Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner, facility)
        .await;
    let destination =
        staging_location(&fixture, access.tenant_id, facility, "WAVE-ROLLBACK-STAGE").await;
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let first = allocated_order(
        &fixture,
        &app,
        &token,
        &access,
        owner,
        facility,
        "WAVE-ROLLBACK-A",
        2,
    )
    .await;
    let second = allocated_order(
        &fixture,
        &app,
        &token,
        &access,
        owner,
        facility,
        "WAVE-ROLLBACK-B",
        3,
    )
    .await;
    let planned: PickWaveResponse = json_response(
        expect(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                "/api/v1/pick-waves",
                Some("wave-rollback-plan"),
                Some(plan_body(
                    facility,
                    destination,
                    "Rollback wave",
                    &[first, second],
                )),
            )
            .await,
            StatusCode::OK,
            "plan rollback wave",
        )
        .await,
    )
    .await;
    let admin = admin_db_for(&fixture.db).await;
    sqlx::query("UPDATE orders SET revision=revision+1 WHERE tenant_id=$1 AND id=$2")
        .bind(access.tenant_id.get())
        .bind(second.0)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    let response = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/pick-waves/{}/releases", planned.wave_id),
        Some("wave-rollback-release"),
        Some(json!({"expected_revision": 1})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let effects: (String, i64, i64, i64, i64) = sqlx::query_as(
        r#"SELECT wave.status,wave.revision,
                  (SELECT COUNT(*) FROM order_releases WHERE pick_wave_id=wave.id),
                  (SELECT COUNT(*) FROM pick_tasks WHERE order_release_id IN
                    (SELECT id FROM order_releases WHERE pick_wave_id=wave.id)),
                  (SELECT COUNT(*) FROM pick_wave_orders WHERE pick_wave_id=wave.id AND active)
           FROM pick_waves wave WHERE wave.tenant_id=$1 AND wave.id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(planned.wave_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(effects, ("planned".to_owned(), 1, 0, 0, 2));
}
