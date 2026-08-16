#[path = "api_v1_picking/batch_cart.rs"]
mod batch_cart;
#[path = "api_v1_picking/case_pick.rs"]
mod case_pick;
#[path = "api_v1_picking/cluster_cart.rs"]
mod cluster_cart;
mod common;
#[path = "api_v1_picking/decision_policy.rs"]
mod decision_policy;
#[path = "api_v1_picking/pallet_pick.rs"]
mod pallet_pick;
#[path = "api_v1_picking/zone_pick.rs"]
mod zone_pick;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use sqlx::Row;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::{IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    AllocationPolicyReference, ErrorReason, ErrorResponse, OrderAllocationOutcome,
    PickClaimHeartbeatResponse, PickClaimReleaseResponse, PickClaimResponse,
    PickContentConfirmationResponse, PickContentState, PickDecisionPolicySource, PickOrderStatus,
    PlanOrderAllocationRequest, PlanOrderAllocationResponse, ReleaseOrderResponse, Revision,
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
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
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
    idempotency_key: Option<&str>,
    body: Option<Value>,
) -> axum::response::Response {
    app.clone()
        .oneshot(request(
            token,
            tenant_id,
            method,
            path,
            idempotency_key,
            body,
        ))
        .await
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn expect_status(
    response: axum::response::Response,
    expected: StatusCode,
    operation: &str,
) -> axum::response::Response {
    if response.status() != expected {
        let actual = response.status();
        let body = response_json::<Value>(response).await;
        panic!("{operation}: expected {expected}, got {actual}: {body}");
    }
    response
}

async fn grant_orders(db: &db::Db, tenant_id: TenantId, user_id: i64, role_name: &str) {
    let permission =
        match wareboxes_persistence_postgres::permissions::find_by_name(db, tenant_id, "orders")
            .await
            .unwrap()
        {
            Some(permission) => permission.id,
            None => wareboxes_persistence_postgres::permissions::add_permission(
                db,
                tenant_id,
                "orders",
                Some("Fulfillment orders"),
            )
            .await
            .unwrap(),
        };
    let role = wareboxes_persistence_postgres::roles::add_role(
        db,
        tenant_id,
        role_name,
        Some("Release fulfillment orders"),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        db, tenant_id, role, permission
    )
    .await
    .unwrap());
    assert!(
        wareboxes_persistence_postgres::roles::add_role_to_user(db, tenant_id, user_id, role)
            .await
            .unwrap()
    );
}

async fn add_wms_operator(
    fixture: &Fixture,
    tenant_id: TenantId,
    email: &str,
    role_name: &str,
) -> wareboxes_core::models::User {
    let user = fixture.user(email).await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
        .bind(tenant_id.get())
        .bind(user.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let permission =
        wareboxes_persistence_postgres::permissions::find_by_name(&fixture.db, tenant_id, "wms")
            .await
            .unwrap()
            .expect("tenant has a WMS permission");
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        role_name,
        Some("RF picking operator"),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        permission.id,
    )
    .await
    .unwrap());
    assert!(wareboxes_persistence_postgres::roles::add_role_to_user(
        &fixture.db,
        tenant_id,
        user.id,
        role,
    )
    .await
    .unwrap());
    user
}

async fn set_scope(
    db: &db::Db,
    tenant_id: TenantId,
    user_id: i64,
    facility_ids: Vec<i64>,
    inventory_owner_ids: Vec<i64>,
) {
    assert!(repo::tenants::update_user_access_scope(
        db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id,
            all_facilities: false,
            facility_ids,
            all_inventory_owners: false,
            inventory_owner_ids,
        },
    )
    .await
    .unwrap());
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

async fn plate_at(
    fixture: &Fixture,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
    location_id: i64,
    barcode: &str,
) -> i64 {
    let plate_id = repo::license_plates::add_license_plate(
        &fixture.db,
        tenant_id,
        inventory_owner_id,
        facility_id,
        Some(barcode),
    )
    .await
    .unwrap();
    let admin = admin_db_for(&fixture.db).await;
    sqlx::query("UPDATE license_plates SET location_id = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(location_id)
        .bind(tenant_id.get())
        .bind(plate_id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    plate_id
}

#[derive(Debug, Clone)]
struct AllocatedOrder {
    order_id: i64,
    source_location_ids: Vec<i64>,
    source_location_barcodes: Vec<String>,
    item_barcodes: Vec<String>,
    allocation: PlanOrderAllocationResponse,
}

#[allow(clippy::too_many_arguments)]
async fn allocated_order(
    fixture: &Fixture,
    app: &axum::Router,
    token: &str,
    access: &wareboxes_core::models::TenantAccess,
    inventory_owner_id: i64,
    facility_id: i64,
    key: &str,
    requested_quantities: &[i64],
    available_quantities: &[i64],
) -> AllocatedOrder {
    assert_eq!(requested_quantities.len(), available_quantities.len());
    let order_id = fixture
        .order_header(access.tenant_id, key, inventory_owner_id)
        .await;
    let mut source_location_ids = Vec::new();
    let mut source_location_barcodes = Vec::new();
    let mut item_barcodes = Vec::new();
    for (index, (&requested, &available)) in requested_quantities
        .iter()
        .zip(available_quantities)
        .enumerate()
    {
        let item_id = fixture
            .item(access.tenant_id, &format!("{key} item {index}"), "each")
            .await;
        let item_barcode = format!("{key}-ITEM-{index}");
        repo::items::add_barcode(
            &fixture.db,
            access.tenant_id,
            item_id,
            &item_barcode,
            "code128",
            None,
        )
        .await
        .unwrap();
        fixture
            .order_item(access.tenant_id, order_id, item_id, requested)
            .await;
        let source_barcode = format!("{key}-SOURCE-{index}");
        let balance = fixture
            .received_balance(
                access,
                ReceivedBalanceSetup {
                    inventory_owner_id,
                    facility_id,
                    item_id,
                    qty: available,
                    key: &source_barcode,
                },
            )
            .await;
        source_location_ids.push(balance.location_id);
        source_location_barcodes.push(source_barcode);
        item_barcodes.push(item_barcode);
    }

    let body = serde_json::to_value(PlanOrderAllocationRequest {
        facility_id,
        expected_revision: Revision::new(1).unwrap(),
        expected_policy: AllocationPolicyReference::product_default(),
    })
    .unwrap();
    let response = send(
        app,
        token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{order_id}/allocation-runs"),
        Some(&format!("{key}-allocate")),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    AllocatedOrder {
        order_id,
        source_location_ids,
        source_location_barcodes,
        item_barcodes,
        allocation: response_json(response).await,
    }
}

fn release_body(facility_id: i64, destination_location_id: i64, revision: i64) -> Value {
    json!({
        "facility_id": facility_id,
        "destination_location_id": destination_location_id,
        "expected_revision": revision
    })
}

async fn release(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    order_id: i64,
    key: Option<&str>,
    body: Value,
) -> axum::response::Response {
    send(
        app,
        token,
        tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{order_id}/releases"),
        key,
        Some(body),
    )
    .await
}

#[tokio::test]
async fn release_is_complete_revisioned_replay_safe_and_creates_one_task_per_allocation() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("pick-release@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "pick-release-orders",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Pick Release Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Pick Release Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let destination_id = staging_location(
        &fixture,
        access.tenant_id,
        facility_id,
        "PICK-RELEASE-STAGE",
    )
    .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let order = allocated_order(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        "PICK-RELEASE",
        &[3, 2],
        &[3, 2],
    )
    .await;
    assert_eq!(
        order.allocation.outcome,
        OrderAllocationOutcome::FullyAllocated
    );
    assert_eq!(order.allocation.revision.get(), 2);

    let body = release_body(facility_id, destination_id, 2);
    let missing_key = release(
        &app,
        &token,
        access.tenant_id,
        order.order_id,
        None,
        body.clone(),
    )
    .await;
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json::<ErrorResponse>(missing_key).await.reason,
        ErrorReason::IdempotencyKeyRequired
    );

    let pickable_destination = release(
        &app,
        &token,
        access.tenant_id,
        order.order_id,
        Some("pick-release-wrong-destination"),
        release_body(facility_id, order.source_location_ids[0], 2),
    )
    .await;
    expect_status(
        pickable_destination,
        StatusCode::CONFLICT,
        "pickable release destination",
    )
    .await;

    let first = release(
        &app,
        &token,
        access.tenant_id,
        order.order_id,
        Some("pick-release-first"),
        body.clone(),
    );
    let second = release(
        &app,
        &token,
        access.tenant_id,
        order.order_id,
        Some("pick-release-race"),
        body.clone(),
    );
    let (first, second) = tokio::join!(first, second);
    let (success, conflict, success_key) = match (first.status(), second.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => (first, second, "pick-release-first"),
        (StatusCode::CONFLICT, StatusCode::OK) => (second, first, "pick-release-race"),
        statuses => panic!("expected one release and one conflict, got {statuses:?}"),
    };
    assert_eq!(
        response_json::<ErrorResponse>(conflict).await.reason,
        ErrorReason::Conflict
    );
    let released: ReleaseOrderResponse = response_json(success).await;
    assert_eq!(released.order_id, order.order_id);
    assert_eq!(released.inventory_owner_id, owner_id);
    assert_eq!(released.allocation_count, 2);
    assert_eq!(released.pick_task_count, 2);
    assert_eq!(released.released_quantity, 5);
    assert_eq!(released.revision.get(), 3);

    let replay = release(
        &app,
        &token,
        access.tenant_id,
        order.order_id,
        Some(success_key),
        body.clone(),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<ReleaseOrderResponse>(replay).await,
        released
    );
    let changed = release(
        &app,
        &token,
        access.tenant_id,
        order.order_id,
        Some(success_key),
        release_body(facility_id, destination_id, 1),
    )
    .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let durable: (String, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT orders.status, orders.revision,
               (SELECT COUNT(*) FROM order_releases release
                WHERE release.tenant_id = orders.tenant_id AND release.order_id = orders.id),
               (SELECT COUNT(*) FROM order_release_allocations snapshot
                WHERE snapshot.tenant_id = orders.tenant_id AND snapshot.order_id = orders.id),
               (SELECT COUNT(*) FROM pick_tasks task
                WHERE task.tenant_id = orders.tenant_id AND task.order_id = orders.id),
               (SELECT COUNT(*) FROM pick_task_contents content
                WHERE content.tenant_id = orders.tenant_id AND content.order_id = orders.id),
               (SELECT COALESCE(SUM(content.planned_qty), 0)::BIGINT
                FROM pick_task_contents content
                WHERE content.tenant_id = orders.tenant_id AND content.order_id = orders.id),
               (SELECT COUNT(*) FROM command_idempotency_records command
                WHERE command.tenant_id = orders.tenant_id
                  AND command.operation = 'order.release.v1'
                  AND (command.result_json->>'order_id')::BIGINT = orders.id)
        FROM orders WHERE orders.tenant_id = $1 AND orders.id = $2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(order.order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(durable, ("processing".into(), 3, 1, 2, 2, 2, 5, 1));
    let one_task_per_allocation: bool = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) = COUNT(DISTINCT task.source_allocation_id)
           AND COUNT(*) = COUNT(DISTINCT content.source_allocation_id)
           AND COUNT(*) = COUNT(DISTINCT content.task_id)
        FROM pick_tasks task
        INNER JOIN pick_task_contents content
          ON content.tenant_id = task.tenant_id AND content.task_id = task.id
        WHERE task.tenant_id = $1 AND task.order_id = $2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(order.order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert!(one_task_per_allocation);

    let partial = allocated_order(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        "PICK-PARTIAL",
        &[4],
        &[2],
    )
    .await;
    assert_eq!(
        partial.allocation.outcome,
        OrderAllocationOutcome::PartiallyAllocated
    );
    let rejected = release(
        &app,
        &token,
        access.tenant_id,
        partial.order_id,
        Some("pick-partial-release"),
        release_body(facility_id, destination_id, 2),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(rejected).await.message,
        "order is not fully allocated"
    );

    let stale = allocated_order(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        "PICK-STALE",
        &[1],
        &[1],
    )
    .await;
    let stale_release = release(
        &app,
        &token,
        access.tenant_id,
        stale.order_id,
        Some("pick-stale-release"),
        release_body(facility_id, destination_id, 1),
    )
    .await;
    assert_eq!(stale_release.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(stale_release).await.message,
        "order revision does not match expected revision"
    );

    let held = allocated_order(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        "PICK-HELD",
        &[1],
        &[1],
    )
    .await;
    let hold = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{}/holds", held.order_id),
        Some("pick-held-place"),
        Some(json!({
            "reason": "customer_request",
            "note": "Pause before release"
        })),
    )
    .await;
    assert_eq!(hold.status(), StatusCode::OK);
    let held_release = release(
        &app,
        &token,
        access.tenant_id,
        held.order_id,
        Some("pick-held-release"),
        release_body(facility_id, destination_id, 2),
    )
    .await;
    assert_eq!(held_release.status(), StatusCode::CONFLICT);
    let held_error: ErrorResponse = response_json(held_release).await;
    assert_eq!(held_error.reason, ErrorReason::Conflict);

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let rejected_release_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM order_releases WHERE tenant_id = $1 AND order_id IN ($2, $3, $4)",
    )
    .bind(access.tenant_id.get())
    .bind(partial.order_id)
    .bind(stale.order_id)
    .bind(held.order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(rejected_release_count, 0);
}

#[tokio::test]
async fn claim_current_heartbeat_release_and_reclaim_are_typed_and_replay_safe() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("pick-claim@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "pick-claim-orders",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Pick Claim Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Pick Claim Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let destination_id =
        staging_location(&fixture, access.tenant_id, facility_id, "PICK-CLAIM-STAGE").await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let order = allocated_order(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        "PICK-CLAIM",
        &[2, 1],
        &[2, 1],
    )
    .await;
    let released = release(
        &app,
        &token,
        access.tenant_id,
        order.order_id,
        Some("pick-claim-release-order"),
        release_body(facility_id, destination_id, 2),
    )
    .await;
    let released = expect_status(released, StatusCode::OK, "claim test order release").await;
    drop(released);

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let task_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM pick_tasks WHERE tenant_id = $1 AND order_id = $2 ORDER BY id",
    )
    .bind(access.tenant_id.get())
    .bind(order.order_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(task_ids.len(), 2);

    let claim_path = format!("/api/v1/picking-claims/{}", task_ids[0]);
    let second_operator = add_wms_operator(
        &fixture,
        access.tenant_id,
        "pick-claim-second@test.local",
        "pick-claim-second-wms",
    )
    .await;
    set_scope(
        &fixture.db,
        access.tenant_id,
        second_operator.id,
        vec![facility_id],
        vec![owner_id],
    )
    .await;
    let second_token = auth::create_session(&fixture.db, second_operator.id)
        .await
        .unwrap();
    let first_racer = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &claim_path,
        Some("pick-claim-race-first"),
        Some(json!({})),
    );
    let second_racer = send(
        &app,
        &second_token,
        access.tenant_id,
        Method::POST,
        &claim_path,
        Some("pick-claim-race-second"),
        Some(json!({})),
    );
    let (first_racer, second_racer) = tokio::join!(first_racer, second_racer);
    let (winner, loser, winner_token) = match (first_racer.status(), second_racer.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => (first_racer, second_racer, token.as_str()),
        (StatusCode::CONFLICT, StatusCode::OK) => {
            (second_racer, first_racer, second_token.as_str())
        }
        statuses => panic!("expected one claim winner and one conflict, got {statuses:?}"),
    };
    assert_eq!(
        response_json::<PickClaimResponse>(winner).await.task_id,
        task_ids[0]
    );
    assert_eq!(
        response_json::<ErrorResponse>(loser).await.reason,
        ErrorReason::Conflict
    );
    let race_release = send(
        &app,
        winner_token,
        access.tenant_id,
        Method::POST,
        &format!("{claim_path}/releases"),
        Some("pick-claim-race-release"),
        Some(json!({
            "reason": "work_interrupted",
            "note": "Concurrency test handoff"
        })),
    )
    .await;
    assert_eq!(race_release.status(), StatusCode::OK);

    let claimed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &claim_path,
        Some("pick-claim-by-id"),
        Some(json!({})),
    )
    .await;
    assert_eq!(claimed.status(), StatusCode::OK);
    let claimed: PickClaimResponse = response_json(claimed).await;
    assert_eq!(claimed.task_id, task_ids[0]);
    assert_eq!(claimed.order_id, order.order_id);
    assert_eq!(claimed.destination_location_id, destination_id);
    assert_eq!(claimed.destination_location_barcode, "PICK-CLAIM-STAGE");
    assert_eq!(claimed.content.state, PickContentState::Pending);
    assert!(claimed.content.source_license_plate_id.is_none());
    assert!(claimed.content.source_license_plate_barcode.is_none());
    assert!(order
        .source_location_barcodes
        .contains(&claimed.content.source_location_barcode));
    assert!(claimed
        .content
        .item_barcodes
        .iter()
        .any(|barcode| order.item_barcodes.contains(barcode)));

    let current = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        "/api/v1/picking-claims/current",
        None,
        None,
    )
    .await;
    assert_eq!(current.status(), StatusCode::OK);
    assert_eq!(
        response_json::<Option<PickClaimResponse>>(current).await,
        Some(claimed.clone())
    );

    let replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &claim_path,
        Some("pick-claim-by-id"),
        Some(json!({})),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(response_json::<PickClaimResponse>(replay).await, claimed);

    let blocked_next = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/picking-claims/next",
        Some("pick-claim-next-blocked"),
        Some(json!({})),
    )
    .await;
    assert_eq!(blocked_next.status(), StatusCode::CONFLICT);

    let heartbeat_path = format!("{claim_path}/heartbeats");
    let heartbeat = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &heartbeat_path,
        Some("pick-heartbeat"),
        Some(json!({})),
    )
    .await;
    assert_eq!(heartbeat.status(), StatusCode::OK);
    let heartbeat: PickClaimHeartbeatResponse = response_json(heartbeat).await;
    assert_eq!(heartbeat.task_id, claimed.task_id);
    let heartbeat_replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &heartbeat_path,
        Some("pick-heartbeat"),
        Some(json!({})),
    )
    .await;
    assert_eq!(heartbeat_replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<PickClaimHeartbeatResponse>(heartbeat_replay).await,
        heartbeat
    );

    let release_path = format!("{claim_path}/releases");
    let release_body = json!({
        "reason": "equipment_unavailable",
        "note": "Scanner battery replacement"
    });
    let released_claim = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &release_path,
        Some("pick-release-claim"),
        Some(release_body.clone()),
    )
    .await;
    assert_eq!(released_claim.status(), StatusCode::OK);
    let released_claim: PickClaimReleaseResponse = response_json(released_claim).await;
    assert_eq!(released_claim.release_count, 2);
    let release_replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &release_path,
        Some("pick-release-claim"),
        Some(release_body),
    )
    .await;
    assert_eq!(release_replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<PickClaimReleaseResponse>(release_replay).await,
        released_claim
    );

    let reclaimed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/picking-claims/next",
        Some("pick-reclaim-next"),
        Some(json!({})),
    )
    .await;
    assert_eq!(reclaimed.status(), StatusCode::OK);
    assert_eq!(
        response_json::<Option<PickClaimResponse>>(reclaimed)
            .await
            .unwrap()
            .task_id,
        task_ids[0]
    );
}

#[tokio::test]
async fn confirmation_requires_exact_scans_and_atomically_transfers_reserved_inventory() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("pick-confirm@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "pick-confirm-orders",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Pick Confirmation Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Pick Confirmation Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let destination_id = staging_location(
        &fixture,
        access.tenant_id,
        facility_id,
        "PICK-CONFIRM-STAGE",
    )
    .await;
    let destination_plate_id = plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        destination_id,
        "PICK-CONFIRM-TOTE",
    )
    .await;
    let wrong_destination_id = staging_location(
        &fixture,
        access.tenant_id,
        facility_id,
        "PICK-CONFIRM-WRONG-STAGE",
    )
    .await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        wrong_destination_id,
        "PICK-CONFIRM-WRONG-TOTE",
    )
    .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let order = allocated_order(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        "PICK-CONFIRM",
        &[4],
        &[7],
    )
    .await;
    let released = release(
        &app,
        &token,
        access.tenant_id,
        order.order_id,
        Some("pick-confirm-release-order"),
        release_body(facility_id, destination_id, 2),
    )
    .await;
    let released = expect_status(released, StatusCode::OK, "confirmation test order release").await;
    drop(released);
    let claim = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/picking-claims/next",
        Some("pick-confirm-claim"),
        Some(json!({})),
    )
    .await;
    assert_eq!(claim.status(), StatusCode::OK);
    let claim = response_json::<Option<PickClaimResponse>>(claim)
        .await
        .unwrap();
    assert_eq!(
        claim.pick_policy.source,
        PickDecisionPolicySource::ProductDefault
    );
    assert!(claim.pick_policy.require_source_location_scan);
    assert!(claim.pick_policy.require_item_scan);
    assert!(claim.pick_policy.require_destination_container_scan);
    let confirmation_path = format!(
        "/api/v1/picking-tasks/{}/contents/{}/confirmations",
        claim.task_id, claim.content.content_id
    );
    let valid = json!({
        "source_location_barcode": claim.content.source_location_barcode,
        "item_barcode": claim.content.item_barcodes[0],
        "destination_license_plate_barcode": "PICK-CONFIRM-TOTE"
    });

    let missing_required_scans = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &confirmation_path,
        Some("pick-confirm-missing-required-scans"),
        Some(json!({})),
    )
    .await;
    assert_eq!(missing_required_scans.status(), StatusCode::BAD_REQUEST);

    for (key, body, expected_status) in [
        (
            "pick-confirm-wrong-source",
            json!({
                "source_location_barcode": "NOT-THE-SOURCE",
                "item_barcode": claim.content.item_barcodes[0],
                "destination_license_plate_barcode": "PICK-CONFIRM-TOTE"
            }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "pick-confirm-wrong-item",
            json!({
                "source_location_barcode": claim.content.source_location_barcode,
                "item_barcode": "NOT-THE-ITEM",
                "destination_license_plate_barcode": "PICK-CONFIRM-TOTE"
            }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "pick-confirm-unexpected-source-plate",
            json!({
                "source_location_barcode": claim.content.source_location_barcode,
                "item_barcode": claim.content.item_barcodes[0],
                "source_license_plate_barcode": "UNEXPECTED-LP",
                "destination_license_plate_barcode": "PICK-CONFIRM-TOTE"
            }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "pick-confirm-wrong-destination",
            json!({
                "source_location_barcode": claim.content.source_location_barcode,
                "item_barcode": claim.content.item_barcodes[0],
                "destination_license_plate_barcode": "PICK-CONFIRM-WRONG-TOTE"
            }),
            StatusCode::CONFLICT,
        ),
    ] {
        let rejected = send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &confirmation_path,
            Some(key),
            Some(body),
        )
        .await;
        assert_eq!(rejected.status(), expected_status, "{key}");
    }

    let current_after_bad_scans = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        "/api/v1/picking-claims/current",
        None,
        None,
    )
    .await;
    assert_eq!(current_after_bad_scans.status(), StatusCode::OK);
    assert_eq!(
        response_json::<Option<PickClaimResponse>>(current_after_bad_scans)
            .await
            .unwrap()
            .task_id,
        claim.task_id
    );
    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let rejected_effects: (String, String, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT task.status, content.state, balance.qty_on_hand, balance.qty_reserved,
               (SELECT COUNT(*) FROM pick_confirmations confirmation
                WHERE confirmation.tenant_id = task.tenant_id
                  AND confirmation.task_id = task.id),
               (SELECT COUNT(*) FROM inventory_transactions transaction
                WHERE transaction.tenant_id = task.tenant_id
                  AND transaction.operation = 'picking.confirm_content.v1'
                  AND transaction.reference_id = content.id)
        FROM pick_tasks task
        INNER JOIN pick_task_contents content
          ON content.tenant_id = task.tenant_id AND content.task_id = task.id
        INNER JOIN inventory_balances balance
          ON balance.tenant_id = content.tenant_id
         AND balance.id = content.source_inventory_balance_id
        WHERE task.tenant_id = $1 AND task.id = $2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(claim.task_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        rejected_effects,
        ("in_progress".into(), "pending".into(), 7, 4, 0, 0)
    );

    let first = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &confirmation_path,
        Some("pick-confirm-success"),
        Some(valid.clone()),
    );
    let second = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &confirmation_path,
        Some("pick-confirm-race"),
        Some(valid.clone()),
    );
    let (first, second) = tokio::join!(first, second);
    let (success, conflict, replay_key) = match (first.status(), second.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => (first, second, "pick-confirm-success"),
        (StatusCode::CONFLICT, StatusCode::OK) => (second, first, "pick-confirm-race"),
        statuses => panic!("expected one confirmation and one conflict, got {statuses:?}"),
    };
    assert_eq!(
        response_json::<ErrorResponse>(conflict).await.reason,
        ErrorReason::Conflict
    );
    let confirmed: PickContentConfirmationResponse = response_json(success).await;
    assert_eq!(confirmed.picked_quantity, 4);
    assert_eq!(confirmed.destination_license_plate_id, destination_plate_id);
    assert!(confirmed.source_location_scan_verified);
    assert!(confirmed.item_scan_verified);
    assert!(confirmed.destination_container_scan_verified);
    assert_eq!(confirmed.content_state, PickContentState::Completed);
    assert!(confirmed.task_completed);
    assert!(confirmed.order_ready_to_pack);
    assert_eq!(confirmed.order_status, PickOrderStatus::AwaitingPacking);
    assert_eq!(confirmed.order_revision.get(), 4);

    let replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &confirmation_path,
        Some(replay_key),
        Some(valid),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<PickContentConfirmationResponse>(replay).await,
        confirmed
    );
    let changed_replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &confirmation_path,
        Some(replay_key),
        Some(json!({
            "source_location_barcode": claim.content.source_location_barcode,
            "item_barcode": "CHANGED-ITEM-SCAN",
            "destination_license_plate_barcode": "PICK-CONFIRM-TOTE"
        })),
    )
    .await;
    assert_eq!(changed_replay.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed_replay).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let inventory = sqlx::query(
        r#"
        SELECT source.status AS source_status, source.deleted AS source_deleted,
               destination.status AS destination_status,
               destination.deleted AS destination_deleted,
               destination.reservation_id AS destination_reservation_id,
               source.reservation_id AS source_reservation_id,
               source_balance.qty_on_hand AS source_on_hand,
               source_balance.qty_reserved AS source_reserved,
               destination_balance.qty_on_hand AS destination_on_hand,
               destination_balance.qty_reserved AS destination_reserved,
               destination.location_id AS destination_location_id,
               destination.license_plate_id AS destination_license_plate_id,
               confirmation.inventory_transaction_id,
               (SELECT COALESCE(SUM(entry.quantity_delta), 0)::BIGINT
                FROM inventory_entries entry
                WHERE entry.tenant_id = confirmation.tenant_id
                  AND entry.transaction_id = confirmation.inventory_transaction_id)
                  AS journal_net,
               (SELECT COUNT(*) FROM pick_confirmations pick
                WHERE pick.tenant_id = confirmation.tenant_id
                  AND pick.pick_task_content_id = confirmation.pick_task_content_id)
                  AS confirmation_count
        FROM pick_confirmations confirmation
        INNER JOIN inventory_allocations source
          ON source.tenant_id = confirmation.tenant_id
         AND source.id = confirmation.source_inventory_allocation_id
        INNER JOIN inventory_allocations destination
          ON destination.tenant_id = confirmation.tenant_id
         AND destination.id = confirmation.destination_inventory_allocation_id
        INNER JOIN inventory_balances source_balance
          ON source_balance.tenant_id = confirmation.tenant_id
         AND source_balance.id = confirmation.source_inventory_balance_id
        INNER JOIN inventory_balances destination_balance
          ON destination_balance.tenant_id = confirmation.tenant_id
         AND destination_balance.id = confirmation.destination_inventory_balance_id
        WHERE confirmation.tenant_id = $1 AND confirmation.id = $2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(confirmed.result_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        inventory.try_get::<String, _>("source_status").unwrap(),
        "fulfilled"
    );
    assert!(inventory
        .try_get::<Option<wareboxes_domain::Timestamp>, _>("source_deleted")
        .unwrap()
        .is_some());
    assert_eq!(
        inventory
            .try_get::<String, _>("destination_status")
            .unwrap(),
        "allocated"
    );
    assert!(inventory
        .try_get::<Option<wareboxes_domain::Timestamp>, _>("destination_deleted")
        .unwrap()
        .is_none());
    assert_eq!(
        inventory
            .try_get::<i64, _>("destination_reservation_id")
            .unwrap(),
        inventory
            .try_get::<i64, _>("source_reservation_id")
            .unwrap()
    );
    assert_eq!(inventory.try_get::<i64, _>("source_on_hand").unwrap(), 3);
    assert_eq!(inventory.try_get::<i64, _>("source_reserved").unwrap(), 0);
    assert_eq!(
        inventory.try_get::<i64, _>("destination_on_hand").unwrap(),
        4
    );
    assert_eq!(
        inventory.try_get::<i64, _>("destination_reserved").unwrap(),
        4
    );
    assert_eq!(
        inventory
            .try_get::<i64, _>("destination_location_id")
            .unwrap(),
        destination_id
    );
    assert_eq!(
        inventory
            .try_get::<Option<i64>, _>("destination_license_plate_id")
            .unwrap(),
        Some(destination_plate_id)
    );
    assert_eq!(inventory.try_get::<i64, _>("journal_net").unwrap(), 0);
    assert_eq!(
        inventory.try_get::<i64, _>("confirmation_count").unwrap(),
        1
    );
    let order_state: (String, i64) =
        sqlx::query_as("SELECT status, revision FROM orders WHERE tenant_id = $1 AND id = $2")
            .bind(access.tenant_id.get())
            .bind(order.order_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(order_state, ("awaiting packing".into(), 4));

    let admin = admin_db_for(&fixture.db).await;
    for (operation, statement) in [
        (
            "order release update",
            "UPDATE order_releases SET released_qty = released_qty WHERE tenant_id = $1 AND order_id = $2",
        ),
        (
            "release allocation update",
            "UPDATE order_release_allocations SET planned_qty = planned_qty WHERE tenant_id = $1 AND order_id = $2",
        ),
        (
            "confirmation update",
            "UPDATE pick_confirmations SET picked_qty = picked_qty WHERE tenant_id = $1 AND order_id = $2",
        ),
        (
            "confirmation delete",
            "DELETE FROM pick_confirmations WHERE tenant_id = $1 AND order_id = $2",
        ),
    ] {
        let result = sqlx::query(statement)
            .bind(access.tenant_id.get())
            .bind(order.order_id)
            .execute(&admin)
            .await;
        assert!(result.is_err(), "{operation} must be rejected");
    }
    admin.close().await;

    let outsider = fixture.wms_user("pick-confirm-outsider@test.local").await;
    let outsider_tenant = tenant_for_user(&fixture.db, outsider.id).await;
    let app_db = app_db_for(&fixture.db).await;
    let mut outsider_tx = tenant_tx(&app_db, outsider_tenant).await;
    let concealed: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM order_releases WHERE tenant_id = $1),
               (SELECT COUNT(*) FROM order_release_allocations WHERE tenant_id = $1),
               (SELECT COUNT(*) FROM pick_tasks WHERE tenant_id = $1),
               (SELECT COUNT(*) FROM pick_task_contents WHERE tenant_id = $1),
               (SELECT COUNT(*) FROM pick_confirmations WHERE tenant_id = $1)
        "#,
    )
    .bind(access.tenant_id.get())
    .fetch_one(&mut *outsider_tx)
    .await
    .unwrap();
    outsider_tx.rollback().await.unwrap();
    app_db.close().await;
    assert_eq!(concealed, (0, 0, 0, 0, 0));

    set_scope(
        &fixture.db,
        access.tenant_id,
        operator.id,
        Vec::new(),
        Vec::new(),
    )
    .await;
    let concealed_replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &confirmation_path,
        Some(replay_key),
        Some(json!({
            "source_location_barcode": claim.content.source_location_barcode,
            "item_barcode": claim.content.item_barcodes[0],
            "destination_license_plate_barcode": "PICK-CONFIRM-TOTE"
        })),
    )
    .await;
    assert_eq!(concealed_replay.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn picking_ledgers_are_tenant_isolated_immutable_and_minimally_granted() {
    let fixture = Fixture::new().await;
    let first = fixture.wms_user("pick-ledger-a@test.local").await;
    let second = fixture.wms_user("pick-ledger-b@test.local").await;
    let first_tenant = tenant_for_user(&fixture.db, first.id).await;
    let second_tenant = tenant_for_user(&fixture.db, second.id).await;
    let admin = admin_db_for(&fixture.db).await;

    for (table, can_update, can_delete) in [
        ("order_releases", false, false),
        ("order_release_allocations", false, false),
        ("pick_tasks", true, false),
        ("pick_task_contents", true, false),
        ("pick_confirmations", false, false),
        ("pick_carts", true, false),
        ("pick_cart_slots", false, false),
        ("pick_clusters", true, false),
        ("pick_cluster_orders", false, false),
        ("pick_cluster_members", false, false),
        ("pick_zone_claims", false, false),
    ] {
        let privileges: (bool, bool, bool, bool) = sqlx::query_as(
            r#"
            SELECT has_table_privilege('wareboxes_app', $1, 'SELECT'),
                   has_table_privilege('wareboxes_app', $1, 'INSERT'),
                   has_table_privilege('wareboxes_app', $1, 'UPDATE'),
                   has_table_privilege('wareboxes_app', $1, 'DELETE')
            "#,
        )
        .bind(table)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(privileges, (true, true, can_update, can_delete), "{table}");
        let forced_rls: bool =
            sqlx::query_scalar("SELECT relforcerowsecurity FROM pg_class WHERE oid = $1::regclass")
                .bind(table)
                .fetch_one(&admin)
                .await
                .unwrap();
        assert!(forced_rls, "{table}");
    }

    let second_facility = fixture
        .facility(second_tenant, "Tenant B cluster RLS facility")
        .await;
    let mut second_tx = tenant_tx(&fixture.db, second_tenant).await;
    let second_cart_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO pick_carts(
          tenant_id,facility_id,barcode,name,status,revision,created_by_user_id,created_at)
        VALUES($1,$2,'TENANT-B-RLS-CART','Tenant B RLS cart','active',1,$3,statement_timestamp())
        RETURNING id"#,
    )
    .bind(second_tenant.get())
    .bind(second_facility)
    .bind(second.id)
    .fetch_one(&mut *second_tx)
    .await
    .unwrap();
    for (code, sequence) in [("A", 1_i64), ("B", 2_i64)] {
        sqlx::query(
            r#"INSERT INTO pick_cart_slots(
              tenant_id,facility_id,cart_id,code,sequence,created_at)
            VALUES($1,$2,$3,$4,$5,statement_timestamp())"#,
        )
        .bind(second_tenant.get())
        .bind(second_facility)
        .bind(second_cart_id)
        .bind(code)
        .bind(sequence)
        .execute(&mut *second_tx)
        .await
        .unwrap();
    }
    second_tx.commit().await.unwrap();

    let app_db = app_db_for(&fixture.db).await;
    let mut first_tx = tenant_tx(&app_db, first_tenant).await;
    let cross_tenant_counts: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM order_releases WHERE tenant_id = $1),
               (SELECT COUNT(*) FROM order_release_allocations WHERE tenant_id = $1),
               (SELECT COUNT(*) FROM pick_tasks WHERE tenant_id = $1),
               (SELECT COUNT(*) FROM pick_task_contents WHERE tenant_id = $1),
               (SELECT COUNT(*) FROM pick_confirmations WHERE tenant_id = $1)
        "#,
    )
    .bind(second_tenant.get())
    .fetch_one(&mut *first_tx)
    .await
    .unwrap();
    let hidden_cluster_cart_counts: (i64, i64) = sqlx::query_as(
        r#"SELECT
          (SELECT COUNT(*) FROM pick_carts WHERE id=$1),
          (SELECT COUNT(*) FROM pick_cart_slots WHERE cart_id=$1)"#,
    )
    .bind(second_cart_id)
    .fetch_one(&mut *first_tx)
    .await
    .unwrap();
    first_tx.rollback().await.unwrap();
    assert_eq!(cross_tenant_counts, (0, 0, 0, 0, 0));
    assert_eq!(hidden_cluster_cart_counts, (0, 0));

    app_db.close().await;
    admin.close().await;
}
