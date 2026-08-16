mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde::Serialize;
use serde_json::Value;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::{IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    ErrorReason, ErrorResponse, OrderAllocationOutcome, OrderAllocationReadinessResponse,
    OrderAllocationReadinessStatus, PlanOrderAllocationRequest, PlanOrderAllocationResponse,
    Revision,
};
use wareboxes_application::order_cancellation::CancelOrderCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_domain::{OrderCancellationReason, OrderHoldReason, OrderId, OrderRevision};

fn api_request<T: Serialize>(
    token: &str,
    tenant_id: TenantId,
    method: Method,
    path: &str,
    idempotency_key: Option<&str>,
    body: Option<&T>,
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
            Body::from(serde_json::to_vec(body).unwrap())
        }
        None => Body::empty(),
    };
    request.body(body).unwrap()
}

#[tokio::test]
async fn allocation_planning_serializes_revisions_and_shared_stock_and_rolls_back_on_hold() {
    let fixture = Fixture::new().await;
    let user = fixture.user("allocation-concurrency@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant_orders(&fixture.db, tenant_id, user.id).await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let owner_id = fixture
        .inventory_owner(tenant_id, "Concurrency Client")
        .await;
    let facility_id = fixture.facility(tenant_id, "Concurrency DC").await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;

    let revision_item_id = fixture.item(tenant_id, "Revision Item", "each").await;
    let revision_order_id = fixture
        .order_header(tenant_id, "ALLOCATE-REVISION-RACE", owner_id)
        .await;
    fixture
        .order_item(tenant_id, revision_order_id, revision_item_id, 3)
        .await;
    let revision_balance = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id: revision_item_id,
                qty: 8,
                key: "REVISION-RACE-BALANCE",
            },
        )
        .await;
    let revision_request = plan_request(facility_id, 1);
    let first = plan(
        &app,
        &token,
        tenant_id,
        revision_order_id,
        Some("revision-race-first"),
        &revision_request,
    );
    let second = plan(
        &app,
        &token,
        tenant_id,
        revision_order_id,
        Some("revision-race-second"),
        &revision_request,
    );
    let (first, second) = tokio::join!(first, second);
    let (success, conflict) = match (first.status(), second.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => (first, second),
        (StatusCode::CONFLICT, StatusCode::OK) => (second, first),
        statuses => panic!("expected one success and one revision conflict, got {statuses:?}"),
    };
    let success: PlanOrderAllocationResponse = response_json(success).await;
    assert_eq!(success.outcome, OrderAllocationOutcome::FullyAllocated);
    assert_eq!(success.revision.get(), 2);
    let conflict: ErrorResponse = response_json(conflict).await;
    assert_eq!(conflict.reason, ErrorReason::Conflict);
    assert_eq!(
        conflict.message,
        "order revision does not match expected revision"
    );

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let revision_effects: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT orders.revision,
               (SELECT COUNT(*) FROM order_allocation_runs run
                WHERE run.tenant_id = orders.tenant_id AND run.order_id = orders.id),
               (SELECT COUNT(*) FROM inventory_reservations reservation
                WHERE reservation.tenant_id = orders.tenant_id
                  AND reservation.order_id = orders.id),
               (SELECT qty_reserved FROM inventory_balances
                WHERE tenant_id = orders.tenant_id AND id = $3),
               (SELECT COUNT(*) FROM command_idempotency_records command
                WHERE command.tenant_id = orders.tenant_id
                  AND command.operation = 'order.allocate.v1'
                  AND command.idempotency_key IN ('revision-race-first', 'revision-race-second'))
        FROM orders
        WHERE orders.tenant_id = $1 AND orders.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(revision_order_id)
    .bind(revision_balance.balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(revision_effects, (2, 1, 1, 3, 1));

    let capacity_item_id = fixture
        .item(tenant_id, "Shared Capacity Item", "each")
        .await;
    let capacity_balance = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id: capacity_item_id,
                qty: 5,
                key: "SHARED-CAPACITY-BALANCE",
            },
        )
        .await;
    let first_order_id = fixture
        .order_header(tenant_id, "ALLOCATE-CAPACITY-A", owner_id)
        .await;
    let second_order_id = fixture
        .order_header(tenant_id, "ALLOCATE-CAPACITY-B", owner_id)
        .await;
    fixture
        .order_item(tenant_id, first_order_id, capacity_item_id, 4)
        .await;
    fixture
        .order_item(tenant_id, second_order_id, capacity_item_id, 4)
        .await;
    let capacity_request = plan_request(facility_id, 1);
    let first = plan(
        &app,
        &token,
        tenant_id,
        first_order_id,
        Some("capacity-race-first"),
        &capacity_request,
    );
    let second = plan(
        &app,
        &token,
        tenant_id,
        second_order_id,
        Some("capacity-race-second"),
        &capacity_request,
    );
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    let first: PlanOrderAllocationResponse = response_json(first).await;
    let second: PlanOrderAllocationResponse = response_json(second).await;
    let mut allocated = vec![
        first.newly_allocated_quantity,
        second.newly_allocated_quantity,
    ];
    allocated.sort_unstable();
    assert_eq!(allocated, vec![1, 4]);
    assert_eq!(
        first.newly_allocated_quantity + second.newly_allocated_quantity,
        5
    );
    assert!(matches!(
        (first.outcome, second.outcome),
        (
            OrderAllocationOutcome::FullyAllocated,
            OrderAllocationOutcome::PartiallyAllocated
        ) | (
            OrderAllocationOutcome::PartiallyAllocated,
            OrderAllocationOutcome::FullyAllocated
        )
    ));

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let capacity_effects: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT balance.qty_on_hand,
               balance.qty_reserved,
               (SELECT COALESCE(SUM(allocation.qty), 0)::BIGINT
                FROM inventory_allocations allocation
                INNER JOIN inventory_reservations reservation
                  ON reservation.tenant_id = allocation.tenant_id
                 AND reservation.id = allocation.reservation_id
                WHERE allocation.tenant_id = balance.tenant_id
                  AND reservation.order_id IN ($3, $4)
                  AND allocation.status = 'allocated'),
               (SELECT COUNT(*) FROM order_allocation_runs run
                WHERE run.tenant_id = balance.tenant_id
                  AND run.order_id IN ($3, $4))
        FROM inventory_balances balance
        WHERE balance.tenant_id = $1 AND balance.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(capacity_balance.balance_id)
    .bind(first_order_id)
    .bind(second_order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(capacity_effects, (5, 5, 5, 2));

    let held_order_id = fixture
        .order_header(tenant_id, "ALLOCATE-HELD", owner_id)
        .await;
    fixture
        .order_item(tenant_id, held_order_id, capacity_item_id, 2)
        .await;
    let hold = repo::orders::place_order_hold(
        &fixture.db,
        &access,
        &command_context(&access, "hold-before-allocation"),
        held_order_id,
        OrderHoldReason::ComplianceReview,
        Some("allocation must wait for review"),
    )
    .await
    .unwrap();
    assert_eq!(hold.active_hold_count, 1);
    let held = plan(
        &app,
        &token,
        tenant_id,
        held_order_id,
        Some("allocate-held-order"),
        &plan_request(facility_id, 2),
    )
    .await;
    assert_eq!(held.status(), StatusCode::CONFLICT);
    let held: ErrorResponse = response_json(held).await;
    assert_eq!(held.reason, ErrorReason::Conflict);
    assert_eq!(held.message, "order has an active hold");

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let held_effects: (String, i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT orders.status,
               orders.revision,
               (SELECT COUNT(*) FROM order_allocation_runs run
                WHERE run.tenant_id = orders.tenant_id AND run.order_id = orders.id),
               (SELECT COUNT(*) FROM inventory_reservations reservation
                WHERE reservation.tenant_id = orders.tenant_id
                  AND reservation.order_id = orders.id),
               (SELECT COUNT(*) FROM inventory_allocations allocation
                INNER JOIN inventory_reservations reservation
                  ON reservation.tenant_id = allocation.tenant_id
                 AND reservation.id = allocation.reservation_id
                WHERE allocation.tenant_id = orders.tenant_id
                  AND reservation.order_id = orders.id),
               (SELECT COUNT(*) FROM command_idempotency_records command
                WHERE command.tenant_id = orders.tenant_id
                  AND command.operation = 'order.allocate.v1'
                  AND command.idempotency_key = 'allocate-held-order')
        FROM orders
        WHERE orders.tenant_id = $1 AND orders.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(held_order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(held_effects, ("held".into(), 2, 0, 0, 0, 0));
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn grant_orders(db: &db::Db, tenant_id: TenantId, user_id: i64) {
    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        db,
        tenant_id,
        "orders",
        Some("Fulfillment orders"),
    )
    .await
    .unwrap();
    let role = wareboxes_persistence_postgres::roles::add_role(
        db,
        tenant_id,
        "allocation-planner",
        Some("Plan outbound inventory"),
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_permission(db, tenant_id, role, permission)
        .await
        .unwrap();
    wareboxes_persistence_postgres::roles::add_role_to_user(db, tenant_id, user_id, role)
        .await
        .unwrap();
}

fn plan_request(facility_id: i64, revision: i64) -> PlanOrderAllocationRequest {
    PlanOrderAllocationRequest {
        facility_id,
        expected_revision: Revision::new(revision).unwrap(),
        expected_policy: wareboxes_api_contract::v1::AllocationPolicyReference::product_default(),
    }
}

async fn readiness(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    order_id: i64,
    facility_id: i64,
) -> axum::response::Response {
    app.clone()
        .oneshot(api_request::<Value>(
            token,
            tenant_id,
            Method::GET,
            &format!("/api/v1/orders/{order_id}/allocation-readiness?facility_id={facility_id}"),
            None,
            None,
        ))
        .await
        .unwrap()
}

async fn plan(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    order_id: i64,
    key: Option<&str>,
    request: &PlanOrderAllocationRequest,
) -> axum::response::Response {
    app.clone()
        .oneshot(api_request(
            token,
            tenant_id,
            Method::POST,
            &format!("/api/v1/orders/{order_id}/allocation-runs"),
            key,
            Some(request),
        ))
        .await
        .unwrap()
}

fn command_context(access: &wareboxes_core::models::TenantAccess, key: &str) -> CommandContext {
    CommandContext {
        tenant_id: access.tenant_id,
        actor_id: access.user_id,
        request_id: format!("request-{key}"),
        idempotency_key: Some(key.to_owned()),
    }
}

#[tokio::test]
async fn order_allocation_is_fefo_replay_safe_replenishable_and_cancel_safe() {
    let fixture = Fixture::new().await;
    let user = fixture.user("v1-order-allocation@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant_orders(&fixture.db, tenant_id, user.id).await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let owner_id = fixture
        .inventory_owner(tenant_id, "Allocation Client")
        .await;
    let facility_id = fixture.facility(tenant_id, "Allocation DC").await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;
    let item_id = fixture.item(tenant_id, "FEFO Item", "each").await;

    let order_id = fixture
        .order_header(tenant_id, "ALLOCATE-FEFO-1", owner_id)
        .await;
    fixture.order_item(tenant_id, order_id, item_id, 7).await;
    let later = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id,
                qty: 10,
                key: "FEFO-LATER",
            },
        )
        .await;
    let earlier = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id,
                qty: 3,
                key: "FEFO-EARLIER",
            },
        )
        .await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        "UPDATE item_batches SET expiration = CURRENT_TIMESTAMP + INTERVAL '20 days' WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id.get())
    .bind(later.item_batch_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE item_batches SET expiration = CURRENT_TIMESTAMP + INTERVAL '10 days' WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id.get())
    .bind(earlier.item_batch_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let before = readiness(&app, &token, tenant_id, order_id, facility_id).await;
    assert_eq!(before.status(), StatusCode::OK);
    let before: OrderAllocationReadinessResponse = response_json(before).await;
    assert_eq!(before.status, OrderAllocationReadinessStatus::Ready);
    assert_eq!(before.revision.get(), 1);
    assert_eq!(before.demand_quantity, 7);
    assert_eq!(before.allocated_quantity, 0);
    assert_eq!(before.eligible_facilities.len(), 1);

    let request = plan_request(facility_id, 1);
    let missing_key = plan(&app, &token, tenant_id, order_id, None, &request).await;
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json::<ErrorResponse>(missing_key).await.reason,
        ErrorReason::IdempotencyKeyRequired
    );

    let first = plan(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("allocate-fefo"),
        &request,
    );
    let retry = plan(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("allocate-fefo"),
        &request,
    );
    let (first, retry) = tokio::join!(first, retry);
    if first.status() != StatusCode::OK {
        panic!(
            "first allocation failed: {}",
            response_json::<Value>(first).await
        );
    }
    if retry.status() != StatusCode::OK {
        panic!(
            "allocation retry failed: {}",
            response_json::<Value>(retry).await
        );
    }
    let result: PlanOrderAllocationResponse = response_json(first).await;
    assert_eq!(
        response_json::<PlanOrderAllocationResponse>(retry).await,
        result
    );
    assert_eq!(result.outcome, OrderAllocationOutcome::FullyAllocated);
    assert_eq!(result.revision.get(), 2);
    assert_eq!(result.newly_allocated_quantity, 7);
    assert_eq!(result.lines.len(), 1);
    assert_eq!(result.lines[0].allocations.len(), 2);
    assert_eq!(
        result.lines[0].allocations[0].inventory_balance_id,
        earlier.balance_id
    );
    assert_eq!(result.lines[0].allocations[0].quantity, 3);
    assert_eq!(
        result.lines[0].allocations[1].inventory_balance_id,
        later.balance_id
    );
    assert_eq!(result.lines[0].allocations[1].quantity, 4);
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let provenance_mutation = sqlx::query(
        "UPDATE inventory_allocations SET allocation_run_id = NULL WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id.get())
    .bind(result.lines[0].allocations[0].allocation_id)
    .execute(&mut *tx)
    .await;
    assert!(provenance_mutation.is_err());
    tx.rollback().await.unwrap();

    let stale = plan(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("allocate-fefo-stale"),
        &request,
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let shortage_item_id = fixture.item(tenant_id, "Replenishment Item", "each").await;
    let shortage_order_id = fixture
        .order_header(tenant_id, "ALLOCATE-SHORT-1", owner_id)
        .await;
    fixture
        .order_item(tenant_id, shortage_order_id, shortage_item_id, 8)
        .await;
    let shortage_balance = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id: shortage_item_id,
                qty: 3,
                key: "SHORTAGE-BALANCE",
            },
        )
        .await;
    let shortage_first = plan(
        &app,
        &token,
        tenant_id,
        shortage_order_id,
        Some("allocate-shortage-first"),
        &plan_request(facility_id, 1),
    )
    .await;
    assert_eq!(shortage_first.status(), StatusCode::OK);
    let shortage_first: PlanOrderAllocationResponse = response_json(shortage_first).await;
    assert_eq!(
        shortage_first.outcome,
        OrderAllocationOutcome::PartiallyAllocated
    );
    assert_eq!(shortage_first.allocated_quantity, 3);
    assert_eq!(shortage_first.shortage_quantity, 5);
    let reservation_id = shortage_first.lines[0].reservation_id.unwrap();

    repo::inventory::receive_inventory(
        &fixture.db,
        tenant_id,
        user.id,
        shortage_balance.item_batch_id,
        shortage_balance.location_id,
        5,
        None,
        Some("shortage replenishment"),
        None,
        None,
        "shortage-replenishment",
    )
    .await
    .unwrap();
    let shortage_second = plan(
        &app,
        &token,
        tenant_id,
        shortage_order_id,
        Some("allocate-shortage-second"),
        &plan_request(facility_id, 2),
    )
    .await;
    assert_eq!(shortage_second.status(), StatusCode::OK);
    let shortage_second: PlanOrderAllocationResponse = response_json(shortage_second).await;
    assert_eq!(
        shortage_second.outcome,
        OrderAllocationOutcome::FullyAllocated
    );
    assert_eq!(shortage_second.newly_allocated_quantity, 5);
    assert_eq!(shortage_second.revision.get(), 3);
    assert_eq!(
        shortage_second.lines[0].reservation_id,
        Some(reservation_id)
    );
    assert_eq!(shortage_second.lines[0].allocations.len(), 2);
    assert!(shortage_second.lines[0]
        .allocations
        .iter()
        .all(|allocation| allocation.inventory_balance_id == shortage_balance.balance_id));

    let fully_allocated = readiness(&app, &token, tenant_id, shortage_order_id, facility_id).await;
    assert_eq!(fully_allocated.status(), StatusCode::OK);
    assert_eq!(
        response_json::<OrderAllocationReadinessResponse>(fully_allocated)
            .await
            .status,
        OrderAllocationReadinessStatus::AlreadyFullyAllocated
    );

    let cancellation_command = CancelOrderCommand::new(
        OrderId::new(shortage_order_id).unwrap(),
        OrderRevision::new(shortage_second.revision.get()).unwrap(),
        OrderCancellationReason::ClientRequest,
        None,
    )
    .unwrap();
    let cancellation = repo::order_cancellation::cancel_order(
        &fixture.db,
        &access,
        &command_context(&access, "cancel-allocated-order"),
        &cancellation_command,
    )
    .await
    .unwrap();
    assert_eq!(cancellation.released_allocation_count, 2);
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let cancellation_state: (String, i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT orders.status,
               orders.revision,
               (SELECT COUNT(*) FROM inventory_reservations reservation
                WHERE reservation.tenant_id = orders.tenant_id
                  AND reservation.order_id = orders.id
                  AND reservation.status = 'cancelled'),
               (SELECT COUNT(*) FROM inventory_allocations allocation
                INNER JOIN inventory_reservations reservation
                  ON reservation.tenant_id = allocation.tenant_id
                 AND reservation.id = allocation.reservation_id
                WHERE allocation.tenant_id = orders.tenant_id
                  AND reservation.order_id = orders.id
                  AND allocation.status = 'released'),
               (SELECT COALESCE(SUM(balance.qty_reserved), 0)::BIGINT
                FROM inventory_balances balance
                WHERE balance.tenant_id = orders.tenant_id
                  AND balance.id = $3),
               (SELECT COUNT(*) FROM outbox_events event
                WHERE event.tenant_id = orders.tenant_id
                  AND event.aggregate_type = 'order'
                  AND event.aggregate_id = orders.id::TEXT
                  AND event.event_type = 'order.cancelled')
        FROM orders
        WHERE orders.tenant_id = $1 AND orders.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(shortage_order_id)
    .bind(shortage_balance.balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(cancellation_state, ("cancelled".into(), 4, 1, 2, 0, 1));
    tx.rollback().await.unwrap();

    repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: user.id,
            all_facilities: false,
            facility_ids: Vec::new(),
            all_inventory_owners: false,
            inventory_owner_ids: Vec::new(),
        },
    )
    .await
    .unwrap();
    let revoked = readiness(&app, &token, tenant_id, order_id, facility_id).await;
    assert!(matches!(
        revoked.status(),
        StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
    ));
    let revoked_replay = plan(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("allocate-fefo"),
        &request,
    )
    .await;
    assert_eq!(revoked_replay.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn allocation_runs_enforce_scope_tenant_isolation_and_immutability() {
    let fixture = Fixture::new().await;
    let user = fixture.user("allocation-isolation-a@test.local").await;
    let other_user = fixture.user("allocation-isolation-b@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let other_tenant_id = tenant_for_user(&fixture.db, other_user.id).await;
    grant_orders(&fixture.db, tenant_id, user.id).await;
    grant_orders(&fixture.db, other_tenant_id, other_user.id).await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let other_token = auth::create_session(&fixture.db, other_user.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let owner_id = fixture.inventory_owner(tenant_id, "Visible Client").await;
    let other_owner_id = fixture.inventory_owner(tenant_id, "Other Client").await;
    let facility_id = fixture.facility(tenant_id, "Visible DC").await;
    let other_facility_id = fixture.facility(tenant_id, "Other DC").await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;
    let item_id = fixture.item(tenant_id, "Isolated Item", "each").await;
    let order_id = fixture
        .order_header(tenant_id, "ALLOCATE-ISOLATED", owner_id)
        .await;
    fixture.order_item(tenant_id, order_id, item_id, 2).await;
    fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id,
                qty: 2,
                key: "ISOLATED-BALANCE",
            },
        )
        .await;
    let request = plan_request(facility_id, 1);
    let created = plan(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("allocation-isolation-created"),
        &request,
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    let created: PlanOrderAllocationResponse = response_json(created).await;

    let guessed = plan(
        &app,
        &other_token,
        other_tenant_id,
        order_id,
        Some("allocation-cross-tenant-guess"),
        &request,
    )
    .await;
    assert_eq!(guessed.status(), StatusCode::NOT_FOUND);

    repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: user.id,
            all_facilities: true,
            facility_ids: Vec::new(),
            all_inventory_owners: false,
            inventory_owner_ids: vec![other_owner_id],
        },
    )
    .await
    .unwrap();
    let owner_denied = plan(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("allocation-owner-denied"),
        &plan_request(facility_id, 2),
    )
    .await;
    assert_eq!(owner_denied.status(), StatusCode::NOT_FOUND);
    let replay_denied = plan(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("allocation-isolation-created"),
        &request,
    )
    .await;
    assert_eq!(replay_denied.status(), StatusCode::NOT_FOUND);

    repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: user.id,
            all_facilities: false,
            facility_ids: vec![other_facility_id],
            all_inventory_owners: true,
            inventory_owner_ids: Vec::new(),
        },
    )
    .await
    .unwrap();
    let facility_denied = plan(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("allocation-facility-denied"),
        &plan_request(facility_id, 2),
    )
    .await;
    assert_eq!(facility_denied.status(), StatusCode::FORBIDDEN);

    repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: user.id,
            all_facilities: true,
            facility_ids: Vec::new(),
            all_inventory_owners: true,
            inventory_owner_ids: Vec::new(),
        },
    )
    .await
    .unwrap();
    let owner_facility_denied = plan(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("allocation-owner-facility-denied"),
        &plan_request(other_facility_id, 2),
    )
    .await;
    assert_eq!(owner_facility_denied.status(), StatusCode::CONFLICT);

    let unbound_visibility: (i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM order_allocation_runs WHERE id = $1),
               (SELECT COUNT(*) FROM order_allocation_run_lines WHERE allocation_run_id = $1)
        "#,
    )
    .bind(created.allocation_run_id)
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(unbound_visibility, (0, 0));

    let mut other_tenant_tx = tenant_tx(&fixture.db, other_tenant_id).await;
    let guessed_visibility: (i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM order_allocation_runs
                WHERE id = $1 OR order_id = $2),
               (SELECT COUNT(*) FROM order_allocation_run_lines
                WHERE allocation_run_id = $1 OR order_id = $2)
        "#,
    )
    .bind(created.allocation_run_id)
    .bind(order_id)
    .fetch_one(&mut *other_tenant_tx)
    .await
    .unwrap();
    assert_eq!(guessed_visibility, (0, 0));
    other_tenant_tx.rollback().await.unwrap();

    let privileges: (bool, bool, bool, bool, bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT has_table_privilege(current_user, 'order_allocation_runs', 'SELECT'),
               has_table_privilege(current_user, 'order_allocation_runs', 'INSERT'),
               has_table_privilege(current_user, 'order_allocation_runs', 'UPDATE'),
               has_table_privilege(current_user, 'order_allocation_runs', 'DELETE'),
               has_table_privilege(current_user, 'order_allocation_run_lines', 'SELECT'),
               has_table_privilege(current_user, 'order_allocation_run_lines', 'INSERT'),
               has_table_privilege(current_user, 'order_allocation_run_lines', 'UPDATE'),
               has_table_privilege(current_user, 'order_allocation_run_lines', 'DELETE')
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(
        privileges,
        (true, true, false, false, true, true, false, false)
    );

    let admin_db = admin_db_for(&fixture.db).await;
    assert!(
        sqlx::query("UPDATE order_allocation_runs SET outcome = outcome WHERE id = $1")
            .bind(created.allocation_run_id)
            .execute(&admin_db)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM order_allocation_runs WHERE id = $1")
            .bind(created.allocation_run_id)
            .execute(&admin_db)
            .await
            .is_err()
    );
    assert!(sqlx::query(
        "UPDATE order_allocation_run_lines SET short_qty = short_qty WHERE allocation_run_id = $1",
    )
    .bind(created.allocation_run_id)
    .execute(&admin_db)
    .await
    .is_err());
    assert!(
        sqlx::query("DELETE FROM order_allocation_run_lines WHERE allocation_run_id = $1")
            .bind(created.allocation_run_id)
            .execute(&admin_db)
            .await
            .is_err()
    );
    admin_db.close().await;

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let immutable_rows: (i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM order_allocation_runs WHERE id = $1),
               (SELECT COUNT(*) FROM order_allocation_run_lines WHERE allocation_run_id = $1)
        "#,
    )
    .bind(created.allocation_run_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(immutable_rows, (1, 1));
}
