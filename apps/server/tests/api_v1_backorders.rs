mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::{IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use wareboxes_api::{auth, routes, state::AppState};
use wareboxes_api_contract::v1::{
    BackorderPolicyMode, BackorderPolicyResponse, ErrorReason, ErrorResponse,
    OrderAllocationOutcome, OrderAllocationReadinessResponse, OrderAllocationReadinessStatus,
    PlanOrderAllocationResponse, ReleaseOrderResponse, SplitOrderBackorderResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;

fn init_test_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("wareboxes_api=debug")
        .with_test_writer()
        .try_init();
}

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
    let body = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
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

async fn grant_permissions(db: &db::Db, tenant_id: TenantId, user_id: i64, role_name: &str) {
    let role = wareboxes_persistence_postgres::roles::add_role(
        db,
        tenant_id,
        role_name,
        Some("Backorder supervisor"),
    )
    .await
    .unwrap();
    for name in ["orders", "wms_supervisor"] {
        let permission =
            wareboxes_persistence_postgres::permissions::find_by_name(db, tenant_id, name)
                .await
                .unwrap()
                .map(|permission| permission.id)
                .unwrap_or(
                    wareboxes_persistence_postgres::permissions::add_permission(
                        db,
                        tenant_id,
                        name,
                        Some(name),
                    )
                    .await
                    .unwrap(),
                );
        wareboxes_persistence_postgres::roles::add_role_permission(db, tenant_id, role, permission)
            .await
            .unwrap();
    }
    wareboxes_persistence_postgres::roles::add_role_to_user(db, tenant_id, user_id, role)
        .await
        .unwrap();
}

struct BackorderFixture {
    fixture: Fixture,
    app: axum::Router,
    token: String,
    user_id: i64,
    tenant_id: TenantId,
    owner_id: i64,
    facility_id: i64,
    destination_location_id: i64,
    order_id: i64,
}

impl BackorderFixture {
    async fn new(key: &str, demand: i64, available: i64) -> Self {
        let fixture = Fixture::new().await;
        let user = fixture.user(&format!("{key}@test.local")).await;
        let tenant_id = tenant_for_user(&fixture.db, user.id).await;
        grant_permissions(&fixture.db, tenant_id, user.id, &format!("{key}-role")).await;
        let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
        let owner_id = fixture
            .inventory_owner(tenant_id, &format!("{key} owner"))
            .await;
        let facility_id = fixture.facility(tenant_id, &format!("{key} DC")).await;
        fixture
            .assign_owner_to_facility(tenant_id, owner_id, facility_id)
            .await;
        let destination_location_id = fixture
            .location(tenant_id, facility_id, &format!("{key}-PACK"))
            .await;
        let admin = admin_db_for(&fixture.db).await;
        sqlx::query(
            "UPDATE locations SET type='staging', pickable=false, receivable=false WHERE tenant_id=$1 AND id=$2",
        )
        .bind(tenant_id.get())
        .bind(destination_location_id)
        .execute(&admin)
        .await
        .unwrap();
        admin.close().await;
        let item_id = fixture
            .item(tenant_id, &format!("{key} item"), "each")
            .await;
        wareboxes_api::repo::items::add_barcode(
            &fixture.db,
            tenant_id,
            item_id,
            &format!("{key}-ITEM"),
            "code128",
            None,
        )
        .await
        .unwrap();
        let order_id = fixture
            .order_header(tenant_id, &format!("{key}-ORDER"), owner_id)
            .await;
        fixture
            .order_item(tenant_id, order_id, item_id, demand)
            .await;
        fixture
            .received_balance(
                &access,
                ReceivedBalanceSetup {
                    inventory_owner_id: owner_id,
                    facility_id,
                    item_id,
                    qty: available,
                    key: &format!("{key}-SOURCE"),
                },
            )
            .await;
        let token = auth::create_session(&fixture.db, user.id).await.unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));
        Self {
            fixture,
            app,
            token,
            user_id: user.id,
            tenant_id,
            owner_id,
            facility_id,
            destination_location_id,
            order_id,
        }
    }

    async fn configure(&self, key: &str, mode: &str) -> axum::response::Response {
        send(
            &self.app,
            &self.token,
            self.tenant_id,
            Method::POST,
            "/api/v1/backorder-policies",
            Some(key),
            Some(json!({
                "inventory_owner_id": self.owner_id,
                "facility_id": self.facility_id,
                "mode": mode
            })),
        )
        .await
    }

    async fn allocate(&self, key: &str) -> axum::response::Response {
        send(
            &self.app,
            &self.token,
            self.tenant_id,
            Method::POST,
            &format!("/api/v1/orders/{}/allocation-runs", self.order_id),
            Some(key),
            Some(json!({
                "facility_id": self.facility_id,
                "expected_revision": 1,
                "expected_policy": {"source": "product_default", "policy_hash": "6090a99a06ea2e049d7321d5cf2b8f462c6d6e6e2ca527ae87657a7a5fd9d156"}
            })),
        )
        .await
    }

    async fn readiness(&self) -> axum::response::Response {
        send(
            &self.app,
            &self.token,
            self.tenant_id,
            Method::GET,
            &format!(
                "/api/v1/orders/{}/allocation-readiness?facility_id={}",
                self.order_id, self.facility_id
            ),
            None,
            None,
        )
        .await
    }

    async fn split(&self, key: Option<&str>, order_revision: i64) -> axum::response::Response {
        send(
            &self.app,
            &self.token,
            self.tenant_id,
            Method::POST,
            &format!("/api/v1/orders/{}/backorder-splits", self.order_id),
            key,
            Some(json!({
                "facility_id": self.facility_id,
                "expected_order_revision": order_revision,
                "expected_policy_revision": 1,
                "reason": "inventory_unavailable"
            })),
        )
        .await
    }
}

#[tokio::test]
async fn partial_allocation_splits_to_a_releaseable_parent_and_open_child() {
    init_test_tracing();
    let setup = BackorderFixture::new("backorder-flow", 10, 6).await;
    let policy: BackorderPolicyResponse = response_json(
        expect_status(
            setup
                .configure("backorder-flow-policy", "split_shortage")
                .await,
            StatusCode::OK,
            "configure backorder policy",
        )
        .await,
    )
    .await;
    assert_eq!(policy.mode, BackorderPolicyMode::SplitShortage);
    assert_eq!(policy.revision.get(), 1);

    let allocation: PlanOrderAllocationResponse = response_json(
        expect_status(
            setup.allocate("backorder-flow-allocate").await,
            StatusCode::OK,
            "partially allocate parent",
        )
        .await,
    )
    .await;
    assert_eq!(
        allocation.outcome,
        OrderAllocationOutcome::PartiallyAllocated
    );
    assert_eq!(allocation.original_demand_quantity, 10);
    assert_eq!(allocation.backordered_quantity, 0);
    assert_eq!(allocation.demand_quantity, 10);
    assert_eq!(allocation.allocated_quantity, 6);
    assert_eq!(allocation.shortage_quantity, 4);

    let split_body = setup.split(Some("backorder-flow-split"), 2).await;
    let split: SplitOrderBackorderResponse =
        response_json(expect_status(split_body, StatusCode::OK, "split allocation shortage").await)
            .await;
    assert_eq!(split.parent_order_id, setup.order_id);
    assert_eq!(split.parent_revision.get(), 3);
    assert_eq!(split.child_revision.get(), 1);
    assert_eq!(split.original_quantity, 10);
    assert_eq!(split.allocated_quantity, 6);
    assert_eq!(split.newly_backordered_quantity, 4);
    assert_eq!(split.parent_effective_quantity, 6);
    assert_eq!(split.lines.len(), 1);
    assert_eq!(split.lines[0].newly_backordered_quantity, 4);

    let replay: SplitOrderBackorderResponse = response_json(
        expect_status(
            setup.split(Some("backorder-flow-split"), 2).await,
            StatusCode::OK,
            "replay backorder split",
        )
        .await,
    )
    .await;
    assert_eq!(replay, split);

    let changed = send(
        &setup.app,
        &setup.token,
        setup.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{}/backorder-splits", setup.order_id),
        Some("backorder-flow-split"),
        Some(json!({
            "facility_id": setup.facility_id,
            "expected_order_revision": 2,
            "expected_policy_revision": 1,
            "reason": "client_requested"
        })),
    )
    .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let readiness: OrderAllocationReadinessResponse = response_json(
        expect_status(
            setup.readiness().await,
            StatusCode::OK,
            "read effective allocation readiness",
        )
        .await,
    )
    .await;
    assert_eq!(
        readiness.status,
        OrderAllocationReadinessStatus::AlreadyFullyAllocated
    );
    assert_eq!(readiness.revision.get(), 3);
    assert_eq!(readiness.original_demand_quantity, 10);
    assert_eq!(readiness.backordered_quantity, 4);
    assert_eq!(readiness.demand_quantity, 6);
    assert_eq!(readiness.reserved_quantity, 6);
    assert_eq!(readiness.allocated_quantity, 6);
    assert_eq!(readiness.shortage_quantity, 0);
    assert_eq!(readiness.lines[0].original_demand_quantity, 10);
    assert_eq!(readiness.lines[0].backordered_quantity, 4);
    assert_eq!(readiness.lines[0].demand_quantity, 6);

    let release: ReleaseOrderResponse = response_json(
        expect_status(
            send(
                &setup.app,
                &setup.token,
                setup.tenant_id,
                Method::POST,
                &format!("/api/v1/orders/{}/releases", setup.order_id),
                Some("backorder-flow-release"),
                Some(json!({
                    "facility_id": setup.facility_id,
                    "destination_location_id": setup.destination_location_id,
                    "expected_revision": 3
                })),
            )
            .await,
            StatusCode::OK,
            "release effective parent demand",
        )
        .await,
    )
    .await;
    assert_eq!(release.released_quantity, 6);
    assert_eq!(release.revision.get(), 4);

    let mut tx = tenant_tx(&setup.fixture.db, setup.tenant_id).await;
    let evidence: (i64, i64, i64, i64, String, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM order_backorder_splits WHERE tenant_id=$1 AND parent_order_id=$2),
               (SELECT COUNT(*) FROM order_backorder_split_lines WHERE tenant_id=$1 AND parent_order_id=$2),
               (SELECT COUNT(*) FROM command_idempotency_records WHERE tenant_id=$1
                 AND operation='outbound.backorder.split.v1' AND idempotency_key='backorder-flow-split'),
               (SELECT qty FROM order_items WHERE tenant_id=$1 AND order_id=$3),
               (SELECT status FROM orders WHERE tenant_id=$1 AND id=$3),
               (SELECT COUNT(*) FROM inventory_reservations WHERE tenant_id=$1 AND order_id=$3),
               (SELECT COUNT(*) FROM inventory_allocations allocation
                 JOIN inventory_reservations reservation
                   ON reservation.tenant_id=allocation.tenant_id
                  AND reservation.id=allocation.reservation_id
                 WHERE allocation.tenant_id=$1 AND reservation.order_id=$3),
               (SELECT COUNT(*) FROM order_activity activity
                 WHERE activity.tenant_id=$1
                   AND activity.order_id IN ($2,$3)
                   AND (activity.action LIKE 'split % units to backorder %'
                     OR activity.action LIKE 'created from backorder split of %')),
               (SELECT COUNT(*) FROM outbox_events event
                 WHERE event.tenant_id=$1
                   AND event.aggregate_type='order'
                   AND event.aggregate_id IN ($2::text,$3::text)
                   AND event.event_type IN (
                     'outbound.order.backorder_split',
                     'outbound.order.created_from_backorder'))
        "#,
    )
    .bind(setup.tenant_id.get())
    .bind(setup.order_id)
    .bind(split.child_order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(evidence, (1, 1, 1, 4, "open".into(), 0, 0, 2, 2));

    let admin = admin_db_for(&setup.fixture.db).await;
    assert!(
        sqlx::query("UPDATE order_backorder_splits SET reason_code=reason_code WHERE id=$1")
            .bind(split.split_id)
            .execute(&admin)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM order_backorder_split_lines WHERE backorder_split_id=$1")
            .bind(split.split_id)
            .execute(&admin)
            .await
            .is_err()
    );
    admin.close().await;

    assert!(repo::tenants::update_user_access_scope(
        &setup.fixture.db,
        setup.tenant_id,
        &UpdateUserAccessScope {
            user_id: setup.user_id,
            all_facilities: false,
            facility_ids: Vec::new(),
            all_inventory_owners: false,
            inventory_owner_ids: Vec::new(),
        },
    )
    .await
    .unwrap());
    let concealed = setup.split(Some("backorder-flow-split"), 2).await;
    assert_eq!(concealed.status(), StatusCode::NOT_FOUND);
    let concealed_changed = send(
        &setup.app,
        &setup.token,
        setup.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{}/backorder-splits", setup.order_id),
        Some("backorder-flow-split"),
        Some(json!({
            "facility_id": setup.facility_id,
            "expected_order_revision": 2,
            "expected_policy_revision": 1,
            "reason": "client_requested"
        })),
    )
    .await;
    assert_eq!(concealed_changed.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn policy_and_revision_guards_leave_the_shortage_unchanged() {
    init_test_tracing();
    let setup = BackorderFixture::new("backorder-guards", 7, 3).await;
    expect_status(
        setup.allocate("backorder-guards-allocate").await,
        StatusCode::OK,
        "partially allocate guarded order",
    )
    .await;

    let no_policy = setup.split(Some("backorder-guards-no-policy"), 2).await;
    assert_eq!(no_policy.status(), StatusCode::CONFLICT);
    expect_status(
        setup.configure("backorder-guards-block", "block").await,
        StatusCode::OK,
        "configure blocking policy",
    )
    .await;
    let blocked = setup.split(Some("backorder-guards-blocked"), 2).await;
    assert_eq!(blocked.status(), StatusCode::CONFLICT);

    let replaced: BackorderPolicyResponse = response_json(
        expect_status(
            send(
                &setup.app,
                &setup.token,
                setup.tenant_id,
                Method::POST,
                "/api/v1/backorder-policies",
                Some("backorder-guards-enable"),
                Some(json!({
                    "inventory_owner_id": setup.owner_id,
                    "facility_id": setup.facility_id,
                    "mode": "split_shortage",
                    "expected_revision": 1
                })),
            )
            .await,
            StatusCode::OK,
            "replace blocking policy",
        )
        .await,
    )
    .await;
    assert_eq!(replaced.revision.get(), 2);

    let stale = setup.split(Some("backorder-guards-stale"), 3).await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let stale_error: ErrorResponse = response_json(stale).await;
    assert_eq!(stale_error.reason, ErrorReason::Conflict);

    let mut tx = tenant_tx(&setup.fixture.db, setup.tenant_id).await;
    let effects: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT orders.revision,
               (SELECT COUNT(*) FROM order_backorder_splits split
                 WHERE split.tenant_id=orders.tenant_id AND split.parent_order_id=orders.id),
               (SELECT COUNT(*) FROM order_backorder_splits split
                 WHERE split.tenant_id=orders.tenant_id AND split.parent_order_id=orders.id),
               (SELECT COALESCE(SUM(backordered_qty),0)::bigint FROM outbound_effective_demand demand
                 WHERE demand.tenant_id=orders.tenant_id AND demand.order_id=orders.id)
        FROM orders WHERE orders.tenant_id=$1 AND orders.id=$2
        "#,
    )
    .bind(setup.tenant_id.get())
    .bind(setup.order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(effects, (2, 0, 0, 0));
}

#[tokio::test]
async fn concurrent_shortage_splits_create_exactly_one_child() {
    init_test_tracing();
    let setup = BackorderFixture::new("backorder-race", 9, 5).await;
    expect_status(
        setup
            .configure("backorder-race-policy", "split_shortage")
            .await,
        StatusCode::OK,
        "configure race policy",
    )
    .await;
    expect_status(
        setup.allocate("backorder-race-allocate").await,
        StatusCode::OK,
        "allocate race order",
    )
    .await;

    let first = setup.split(Some("backorder-race-first"), 2);
    let second = setup.split(Some("backorder-race-second"), 2);
    let (first, second) = tokio::join!(first, second);
    let statuses = [first.status(), second.status()];
    assert!(statuses.contains(&StatusCode::OK));
    assert!(statuses.contains(&StatusCode::CONFLICT));

    let mut tx = tenant_tx(&setup.fixture.db, setup.tenant_id).await;
    let effects: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT orders.revision,
               (SELECT COUNT(*) FROM order_backorder_splits split
                 WHERE split.tenant_id=orders.tenant_id AND split.parent_order_id=orders.id),
               (SELECT COUNT(*) FROM order_backorder_split_lines line
                 WHERE line.tenant_id=orders.tenant_id AND line.parent_order_id=orders.id),
               (SELECT COALESCE(SUM(backordered_qty),0)::bigint FROM outbound_effective_demand demand
                 WHERE demand.tenant_id=orders.tenant_id AND demand.order_id=orders.id),
               (SELECT COUNT(*) FROM command_idempotency_records command
                 WHERE command.tenant_id=orders.tenant_id
                   AND command.operation='outbound.backorder.split.v1'
                   AND command.idempotency_key IN ('backorder-race-first','backorder-race-second'))
        FROM orders WHERE orders.tenant_id=$1 AND orders.id=$2
        "#,
    )
    .bind(setup.tenant_id.get())
    .bind(setup.order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(effects, (3, 1, 1, 4, 1));
}
