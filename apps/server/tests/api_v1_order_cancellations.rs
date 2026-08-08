mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde::Serialize;
use serde_json::Value;
use sqlx::Row;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::{IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    CancelOrderRequest, CancelOrderResponse, ErrorReason, ErrorResponse, OrderAllocationStrategy,
    OrderCancellationReason, OrderCancellationStatus, PlanOrderAllocationRequest,
    PlanOrderAllocationResponse, Revision,
};
use wareboxes_application::CommandContext;
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_domain::OrderHoldReason;

fn api_request<T: Serialize>(
    token: &str,
    tenant_id: TenantId,
    path: &str,
    idempotency_key: Option<&str>,
    body: &T,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = idempotency_key {
        request = request
            .header(IDEMPOTENCY_KEY_HEADER, key)
            .header(REQUEST_ID_HEADER, format!("request-{key}"));
    }
    request
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn cancel(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    order_id: i64,
    key: Option<&str>,
    request: &CancelOrderRequest,
) -> axum::response::Response {
    app.clone()
        .oneshot(api_request(
            token,
            tenant_id,
            &format!("/api/v1/orders/{order_id}/cancellations"),
            key,
            request,
        ))
        .await
        .unwrap()
}

async fn allocate(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    order_id: i64,
    facility_id: i64,
    key: &str,
) -> PlanOrderAllocationResponse {
    let response = app
        .clone()
        .oneshot(api_request(
            token,
            tenant_id,
            &format!("/api/v1/orders/{order_id}/allocation-runs"),
            Some(key),
            &PlanOrderAllocationRequest {
                facility_id,
                expected_revision: Revision::new(1).unwrap(),
                strategy: OrderAllocationStrategy::Fefo,
            },
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

fn cancellation_request(revision: i64) -> CancelOrderRequest {
    CancelOrderRequest {
        expected_revision: Revision::new(revision).unwrap(),
        reason: OrderCancellationReason::ClientRequest,
        note: Some("Client cancelled before warehouse execution".into()),
    }
}

fn command_context(access: &wareboxes_core::models::TenantAccess, key: &str) -> CommandContext {
    CommandContext {
        tenant_id: access.tenant_id,
        actor_id: access.user_id,
        request_id: format!("request-{key}"),
        idempotency_key: Some(key.into()),
    }
}

async fn grant_orders(db: &db::Db, tenant_id: TenantId, user_id: i64) {
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
        "order-cancellation-operator",
        Some("Cancel fulfillment orders"),
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

async fn order_with_line(
    fixture: &Fixture,
    tenant_id: TenantId,
    key: &str,
    inventory_owner_id: i64,
    item_id: i64,
) -> i64 {
    let order_id = fixture
        .order_header(tenant_id, key, inventory_owner_id)
        .await;
    fixture.order_item(tenant_id, order_id, item_id, 1).await;
    order_id
}

async fn set_order_state(db: &db::Db, tenant_id: TenantId, order_id: i64, status: &str) {
    let mut tx = tenant_tx(db, tenant_id).await;
    sqlx::query("UPDATE orders SET status = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(status)
        .bind(tenant_id.get())
        .bind(order_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn update_scope(
    db: &db::Db,
    tenant_id: TenantId,
    user_id: i64,
    all_facilities: bool,
    facility_ids: Vec<i64>,
    all_inventory_owners: bool,
    inventory_owner_ids: Vec<i64>,
) {
    repo::tenants::update_user_access_scope(
        db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id,
            all_facilities,
            facility_ids,
            all_inventory_owners,
            inventory_owner_ids,
        },
    )
    .await
    .unwrap();
}

async fn assert_no_cancellation_effects(db: &db::Db, tenant_id: TenantId, order_ids: &[i64]) {
    let mut tx = tenant_tx(db, tenant_id).await;
    let effects: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM order_cancellations
                WHERE tenant_id = $1 AND order_id = ANY($2)),
               (SELECT COUNT(*) FROM order_activity
                WHERE tenant_id = $1 AND order_id = ANY($2)
                  AND action LIKE 'cancelled order (%'),
               (SELECT COUNT(*) FROM outbox_events
                WHERE tenant_id = $1 AND aggregate_type = 'order'
                  AND aggregate_id IN (
                      SELECT value::TEXT FROM UNNEST($2::BIGINT[]) AS value
                  )
                  AND event_type = 'order.cancelled'),
               (SELECT COUNT(*) FROM command_idempotency_records
                WHERE tenant_id = $1 AND operation = 'order.cancel.v1'
                  AND (result_json->>'order_id')::BIGINT = ANY($2))
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_ids)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(effects, (0, 0, 0, 0));
}

#[tokio::test]
async fn open_cancellation_is_strict_replay_safe_audited_and_creates_no_recovery_work() {
    let fixture = Fixture::new().await;
    let user = fixture.user("cancel-open@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant_orders(&fixture.db, tenant_id, user.id).await;
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let owner_id = fixture.inventory_owner(tenant_id, "Open Client").await;
    let item_id = fixture
        .item(tenant_id, "Open Cancellation Item", "each")
        .await;
    let order_id = order_with_line(&fixture, tenant_id, "CANCEL-OPEN-001", owner_id, item_id).await;
    let request = cancellation_request(1);

    let missing_key = cancel(&app, &token, tenant_id, order_id, None, &request).await;
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json::<ErrorResponse>(missing_key).await.reason,
        ErrorReason::IdempotencyKeyRequired
    );

    let first = cancel(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("cancel-open"),
        &request,
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first: CancelOrderResponse = response_json(first).await;
    assert_eq!(first.order_id, order_id);
    assert_eq!(first.inventory_owner_id, owner_id);
    assert_eq!(first.previous_status, OrderCancellationStatus::Open);
    assert_eq!(first.status, OrderCancellationStatus::Cancelled);
    assert_eq!(first.revision.get(), 2);
    assert_eq!(first.reason, OrderCancellationReason::ClientRequest);
    assert_eq!(first.released_hold_count, 0);
    assert_eq!(first.released_reservation_count, 0);
    assert_eq!(first.released_allocation_count, 0);
    assert_eq!(first.released_quantity, 0);

    let replay = cancel(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("cancel-open"),
        &request,
    )
    .await;
    if replay.status() != StatusCode::OK {
        panic!(
            "open replay failed: {}",
            response_json::<Value>(replay).await
        );
    }
    assert_eq!(response_json::<CancelOrderResponse>(replay).await, first);

    let changed_payload = cancel(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("cancel-open"),
        &CancelOrderRequest {
            expected_revision: Revision::new(1).unwrap(),
            reason: OrderCancellationReason::DuplicateOrder,
            note: None,
        },
    )
    .await;
    assert_eq!(changed_payload.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed_payload).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let new_key = cancel(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("cancel-open-again"),
        &request,
    )
    .await;
    assert_eq!(new_key.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(new_key).await.reason,
        ErrorReason::Conflict
    );

    let stale_order_id =
        order_with_line(&fixture, tenant_id, "CANCEL-STALE-001", owner_id, item_id).await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("UPDATE orders SET revision = 2 WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id.get())
        .bind(stale_order_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let stale = cancel(
        &app,
        &token,
        tenant_id,
        stale_order_id,
        Some("cancel-stale"),
        &cancellation_request(1),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_no_cancellation_effects(&fixture.db, tenant_id, &[stale_order_id]).await;

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let state = sqlx::query(
        r#"
        SELECT orders.status, orders.revision,
               cancellation.inventory_owner_id, cancellation.actor_user_id,
               cancellation.reason, cancellation.note,
               cancellation.previous_status, cancellation.expected_revision,
               cancellation.resulting_revision, cancellation.affected_facility_ids,
               (SELECT COUNT(*) FROM order_activity activity
                WHERE activity.tenant_id = orders.tenant_id
                  AND activity.order_id = orders.id
                  AND activity.actor_user_id = $3
                  AND activity.action = 'cancelled order (client_request)') AS activity_count,
               (SELECT COUNT(*) FROM outbox_events event
                WHERE event.tenant_id = orders.tenant_id
                  AND event.aggregate_type = 'order'
                  AND event.aggregate_id = orders.id::TEXT
                  AND event.event_type = 'order.cancelled') AS event_count,
               (SELECT COUNT(*) FROM command_idempotency_records command
                WHERE command.tenant_id = orders.tenant_id
                  AND command.operation = 'order.cancel.v1'
                  AND command.idempotency_key = 'cancel-open') AS command_count,
               (SELECT COUNT(*) FROM work_tasks task
                WHERE task.tenant_id = orders.tenant_id
                  AND task.task_type = 'unpack_cancelled_order') AS unpack_work_count,
               (SELECT COUNT(*) FROM unpack_cancelled_order_tasks task
                WHERE task.tenant_id = orders.tenant_id
                  AND task.order_id = orders.id) AS unpack_order_count
        FROM orders
        INNER JOIN order_cancellations cancellation
          ON cancellation.tenant_id = orders.tenant_id
         AND cancellation.order_id = orders.id
        WHERE orders.tenant_id = $1 AND orders.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(state.try_get::<String, _>("status").unwrap(), "cancelled");
    assert_eq!(state.try_get::<i64, _>("revision").unwrap(), 2);
    assert_eq!(
        state.try_get::<i64, _>("inventory_owner_id").unwrap(),
        owner_id
    );
    assert_eq!(state.try_get::<i64, _>("actor_user_id").unwrap(), user.id);
    assert_eq!(
        state.try_get::<String, _>("reason").unwrap(),
        "client_request"
    );
    assert_eq!(
        state.try_get::<Option<String>, _>("note").unwrap(),
        request.note
    );
    assert_eq!(
        state.try_get::<String, _>("previous_status").unwrap(),
        "open"
    );
    assert_eq!(state.try_get::<i64, _>("expected_revision").unwrap(), 1);
    assert_eq!(state.try_get::<i64, _>("resulting_revision").unwrap(), 2);
    assert!(state
        .try_get::<Vec<i64>, _>("affected_facility_ids")
        .unwrap()
        .is_empty());
    for column in ["activity_count", "event_count", "command_count"] {
        assert_eq!(state.try_get::<i64, _>(column).unwrap(), 1, "{column}");
    }
    assert_eq!(state.try_get::<i64, _>("unpack_work_count").unwrap(), 0);
    assert_eq!(state.try_get::<i64, _>("unpack_order_count").unwrap(), 0);

    let event_payload: Value = sqlx::query_scalar(
        r#"
        SELECT payload FROM outbox_events
        WHERE tenant_id = $1 AND aggregate_type = 'order'
          AND aggregate_id = $2::TEXT AND event_type = 'order.cancelled'
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(event_payload["cancellation_id"], first.cancellation_id);
    assert_eq!(event_payload["order_id"], order_id);
    assert_eq!(event_payload["released_quantity"], 0);
    assert_eq!(
        event_payload["affected_facility_ids"],
        serde_json::json!([])
    );

    let admin_db = admin_db_for(&fixture.db).await;
    assert!(
        sqlx::query("UPDATE order_cancellations SET note = note WHERE id = $1")
            .bind(first.cancellation_id)
            .execute(&admin_db)
            .await
            .is_err()
    );
    assert!(sqlx::query("DELETE FROM order_cancellations WHERE id = $1")
        .bind(first.cancellation_id)
        .execute(&admin_db)
        .await
        .is_err());
    admin_db.close().await;
}

#[tokio::test]
async fn held_cancellation_atomically_releases_holds_allocations_and_reservations() {
    let fixture = Fixture::new().await;
    let user = fixture.user("cancel-held@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant_orders(&fixture.db, tenant_id, user.id).await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let owner_id = fixture.inventory_owner(tenant_id, "Held Client").await;
    let facility_id = fixture.facility(tenant_id, "Held Client DC").await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;
    let item_id = fixture.item(tenant_id, "Cancellation Item", "each").await;
    let order_id = fixture
        .order_header(tenant_id, "CANCEL-HELD-001", owner_id)
        .await;
    fixture.order_item(tenant_id, order_id, item_id, 5).await;
    let balance = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id,
                qty: 5,
                key: "CANCEL-HELD-BALANCE",
            },
        )
        .await;
    let allocation = allocate(
        &app,
        &token,
        tenant_id,
        order_id,
        facility_id,
        "allocate-before-cancel",
    )
    .await;
    assert_eq!(allocation.revision.get(), 2);
    assert_eq!(allocation.newly_allocated_quantity, 5);
    let hold = repo::orders::place_order_hold(
        &fixture.db,
        &access,
        &command_context(&access, "hold-before-cancel"),
        order_id,
        OrderHoldReason::ComplianceReview,
        Some("Review ended with cancellation"),
    )
    .await
    .unwrap();
    assert_eq!(hold.order_status.as_str(), "held");

    let request = CancelOrderRequest {
        expected_revision: Revision::new(3).unwrap(),
        reason: OrderCancellationReason::FulfillmentException,
        note: Some("Stopped before physical picking began".into()),
    };
    let response = cancel(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("cancel-held"),
        &request,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let response: CancelOrderResponse = response_json(response).await;
    assert_eq!(response.previous_status, OrderCancellationStatus::Held);
    assert_eq!(response.status, OrderCancellationStatus::Cancelled);
    assert_eq!(response.revision.get(), 4);
    assert_eq!(response.released_hold_count, 1);
    assert_eq!(response.released_reservation_count, 1);
    assert_eq!(response.released_allocation_count, 1);
    assert_eq!(response.released_quantity, 5);

    let replay = cancel(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("cancel-held"),
        &request,
    )
    .await;
    if replay.status() != StatusCode::OK {
        panic!(
            "held replay failed: {}",
            response_json::<Value>(replay).await
        );
    }
    assert_eq!(response_json::<CancelOrderResponse>(replay).await, response);

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let state = sqlx::query(
        r#"
        SELECT orders.status, orders.revision, balance.qty_reserved,
               cancellation.affected_facility_ids,
               cancellation.released_hold_count,
               cancellation.released_reservation_count,
               cancellation.released_allocation_count,
               cancellation.released_quantity,
               (SELECT COUNT(*) FROM order_holds hold
                WHERE hold.tenant_id = orders.tenant_id AND hold.order_id = orders.id
                  AND hold.released_at IS NOT NULL AND hold.released_by_user_id = $4) AS released_holds,
               (SELECT COUNT(*) FROM inventory_reservations reservation
                WHERE reservation.tenant_id = orders.tenant_id
                  AND reservation.order_id = orders.id
                  AND reservation.status = 'cancelled' AND reservation.deleted IS NOT NULL)
                  AS cancelled_reservations,
               (SELECT COUNT(*) FROM inventory_allocations allocation
                INNER JOIN inventory_reservations reservation
                  ON reservation.tenant_id = allocation.tenant_id
                 AND reservation.id = allocation.reservation_id
                WHERE allocation.tenant_id = orders.tenant_id
                  AND reservation.order_id = orders.id
                  AND allocation.status = 'released' AND allocation.deleted IS NOT NULL)
                  AS released_allocations,
               (SELECT COUNT(*) FROM work_tasks task
                WHERE task.tenant_id = orders.tenant_id
                  AND task.task_type = 'unpack_cancelled_order') AS unpack_work_count,
               (SELECT COUNT(*) FROM unpack_cancelled_order_tasks task
                WHERE task.tenant_id = orders.tenant_id AND task.order_id = orders.id)
                  AS unpack_order_count,
               (SELECT COUNT(*) FROM unpack_cancelled_order_task_lines line
                INNER JOIN unpack_cancelled_order_tasks task
                  ON task.tenant_id = line.tenant_id AND task.task_id = line.task_id
                WHERE task.tenant_id = orders.tenant_id AND task.order_id = orders.id)
                  AS unpack_line_count
        FROM orders
        INNER JOIN order_cancellations cancellation
          ON cancellation.tenant_id = orders.tenant_id
         AND cancellation.order_id = orders.id
        INNER JOIN inventory_balances balance
          ON balance.tenant_id = orders.tenant_id AND balance.id = $3
        WHERE orders.tenant_id = $1 AND orders.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .bind(balance.balance_id)
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(state.try_get::<String, _>("status").unwrap(), "cancelled");
    assert_eq!(state.try_get::<i64, _>("revision").unwrap(), 4);
    assert_eq!(state.try_get::<i64, _>("qty_reserved").unwrap(), 0);
    assert_eq!(
        state
            .try_get::<Vec<i64>, _>("affected_facility_ids")
            .unwrap(),
        vec![facility_id]
    );
    for column in [
        "released_hold_count",
        "released_reservation_count",
        "released_allocation_count",
        "released_holds",
        "cancelled_reservations",
        "released_allocations",
    ] {
        assert_eq!(state.try_get::<i64, _>(column).unwrap(), 1, "{column}");
    }
    assert_eq!(state.try_get::<i64, _>("released_quantity").unwrap(), 5);
    for column in [
        "unpack_work_count",
        "unpack_order_count",
        "unpack_line_count",
    ] {
        assert_eq!(state.try_get::<i64, _>(column).unwrap(), 0, "{column}");
    }

    let cancellation_events: Vec<(String, i64)> = sqlx::query_as(
        r#"
        SELECT event_type, COUNT(*)
        FROM outbox_events
        WHERE tenant_id = $1
          AND event_type IN ('inventory.allocation.released',
                             'inventory.reservation.cancelled',
                             'order.hold.released', 'order.cancelled')
        GROUP BY event_type
        ORDER BY event_type
        "#,
    )
    .bind(tenant_id.get())
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    let cancellation_activity: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM order_activity
        WHERE tenant_id = $1 AND order_id = $2 AND actor_user_id = $3
          AND action = 'cancelled order (fulfillment_exception)'
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        cancellation_events,
        vec![
            ("inventory.allocation.released".into(), 1),
            ("inventory.reservation.cancelled".into(), 1),
            ("order.cancelled".into(), 1),
            ("order.hold.released".into(), 1),
        ]
    );
    assert_eq!(cancellation_activity, 1);

    update_scope(
        &fixture.db,
        tenant_id,
        user.id,
        false,
        Vec::new(),
        true,
        Vec::new(),
    )
    .await;
    let site_revoked_replay = cancel(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("cancel-held"),
        &request,
    )
    .await;
    assert_eq!(site_revoked_replay.status(), StatusCode::NOT_FOUND);

    update_scope(
        &fixture.db,
        tenant_id,
        user.id,
        true,
        Vec::new(),
        false,
        Vec::new(),
    )
    .await;
    let owner_revoked_replay = cancel(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("cancel-held"),
        &request,
    )
    .await;
    assert_eq!(owner_revoked_replay.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn terminal_states_and_concurrent_revisions_fail_closed() {
    let fixture = Fixture::new().await;
    let user = fixture.user("cancel-state@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant_orders(&fixture.db, tenant_id, user.id).await;
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let owner_id = fixture.inventory_owner(tenant_id, "State Client").await;
    let item_id = fixture
        .item(tenant_id, "State Cancellation Item", "each")
        .await;
    let mut terminal_order_ids = Vec::new();

    for (index, status) in ["processing", "awaiting shipment", "shipped", "void"]
        .into_iter()
        .enumerate()
    {
        let order_id = order_with_line(
            &fixture,
            tenant_id,
            &format!("CANCEL-TERMINAL-{index}"),
            owner_id,
            item_id,
        )
        .await;
        set_order_state(&fixture.db, tenant_id, order_id, status).await;
        terminal_order_ids.push(order_id);
        let response = cancel(
            &app,
            &token,
            tenant_id,
            order_id,
            Some(&format!("cancel-terminal-{index}")),
            &cancellation_request(1),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT, "{status}");
        assert_eq!(
            response_json::<ErrorResponse>(response).await.reason,
            ErrorReason::Conflict
        );
    }
    assert_no_cancellation_effects(&fixture.db, tenant_id, &terminal_order_ids).await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let retained_states: Vec<String> = sqlx::query_scalar(
        "SELECT status FROM orders WHERE tenant_id = $1 AND id = ANY($2) ORDER BY id",
    )
    .bind(tenant_id.get())
    .bind(&terminal_order_ids)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        retained_states,
        vec!["processing", "awaiting shipment", "shipped", "void"]
    );

    let concurrent_order_id =
        order_with_line(&fixture, tenant_id, "CANCEL-CONCURRENT", owner_id, item_id).await;
    let request = cancellation_request(1);
    let first = cancel(
        &app,
        &token,
        tenant_id,
        concurrent_order_id,
        Some("cancel-concurrent-a"),
        &request,
    );
    let second = cancel(
        &app,
        &token,
        tenant_id,
        concurrent_order_id,
        Some("cancel-concurrent-b"),
        &request,
    );
    let (first, second) = tokio::join!(first, second);
    let (success, conflict) = match (first.status(), second.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => (first, second),
        (StatusCode::CONFLICT, StatusCode::OK) => (second, first),
        statuses => panic!("expected one success and one conflict, got {statuses:?}"),
    };
    assert_eq!(
        response_json::<CancelOrderResponse>(success)
            .await
            .revision
            .get(),
        2
    );
    assert_eq!(
        response_json::<ErrorResponse>(conflict).await.reason,
        ErrorReason::Conflict
    );

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let concurrent_effects: (String, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT orders.status, orders.revision,
               (SELECT COUNT(*) FROM order_cancellations cancellation
                WHERE cancellation.tenant_id = orders.tenant_id
                  AND cancellation.order_id = orders.id),
               (SELECT COUNT(*) FROM command_idempotency_records command
                WHERE command.tenant_id = orders.tenant_id
                  AND command.operation = 'order.cancel.v1'
                  AND command.idempotency_key IN ('cancel-concurrent-a', 'cancel-concurrent-b')),
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
    .bind(concurrent_order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(concurrent_effects, ("cancelled".into(), 2, 1, 1, 1));
}

#[tokio::test]
async fn cancellation_enforces_owner_site_tenant_and_rls_boundaries() {
    let fixture = Fixture::new().await;
    let user = fixture.user("cancel-isolation-a@test.local").await;
    let other_user = fixture.user("cancel-isolation-b@test.local").await;
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
    let denied_owner_id = fixture.inventory_owner(tenant_id, "Denied Client").await;
    let facility_id = fixture.facility(tenant_id, "Visible DC").await;
    let denied_facility_id = fixture.facility(tenant_id, "Denied DC").await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;

    let visible_item_id = fixture
        .item(tenant_id, "Scoped Cancellation Item", "each")
        .await;
    let visible_order_id = fixture
        .order_header(tenant_id, "CANCEL-ISOLATION-VISIBLE", owner_id)
        .await;
    fixture
        .order_item(tenant_id, visible_order_id, visible_item_id, 2)
        .await;
    fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id: visible_item_id,
                qty: 2,
                key: "CANCEL-ISOLATION-BALANCE",
            },
        )
        .await;
    allocate(
        &app,
        &token,
        tenant_id,
        visible_order_id,
        facility_id,
        "allocate-isolation-visible",
    )
    .await;
    let visible_request = cancellation_request(2);
    let created = cancel(
        &app,
        &token,
        tenant_id,
        visible_order_id,
        Some("cancel-isolation-visible"),
        &visible_request,
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    let created: CancelOrderResponse = response_json(created).await;

    let cross_tenant = cancel(
        &app,
        &other_token,
        other_tenant_id,
        visible_order_id,
        Some("cancel-cross-tenant-guess"),
        &visible_request,
    )
    .await;
    assert_eq!(cross_tenant.status(), StatusCode::NOT_FOUND);

    let owner_denied_order_id = order_with_line(
        &fixture,
        tenant_id,
        "CANCEL-OWNER-DENIED",
        denied_owner_id,
        visible_item_id,
    )
    .await;
    update_scope(
        &fixture.db,
        tenant_id,
        user.id,
        true,
        Vec::new(),
        false,
        vec![owner_id],
    )
    .await;
    let owner_denied = cancel(
        &app,
        &token,
        tenant_id,
        owner_denied_order_id,
        Some("cancel-owner-denied"),
        &cancellation_request(1),
    )
    .await;
    assert_eq!(owner_denied.status(), StatusCode::NOT_FOUND);

    let site_order_id = fixture
        .order_header(tenant_id, "CANCEL-SITE-DENIED", owner_id)
        .await;
    fixture
        .order_item(tenant_id, site_order_id, visible_item_id, 1)
        .await;
    update_scope(
        &fixture.db,
        tenant_id,
        user.id,
        true,
        Vec::new(),
        true,
        Vec::new(),
    )
    .await;
    allocate(
        &app,
        &token,
        tenant_id,
        site_order_id,
        facility_id,
        "allocate-site-denied",
    )
    .await;
    update_scope(
        &fixture.db,
        tenant_id,
        user.id,
        false,
        vec![denied_facility_id],
        true,
        Vec::new(),
    )
    .await;
    let site_denied = cancel(
        &app,
        &token,
        tenant_id,
        site_order_id,
        Some("cancel-site-denied"),
        &cancellation_request(2),
    )
    .await;
    assert_eq!(site_denied.status(), StatusCode::FORBIDDEN);
    assert_no_cancellation_effects(
        &fixture.db,
        tenant_id,
        &[owner_denied_order_id, site_order_id],
    )
    .await;

    let unbound_visibility: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM order_cancellations WHERE id = $1 OR order_id = $2",
    )
    .bind(created.cancellation_id)
    .bind(visible_order_id)
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(unbound_visibility, 0);
    let mut other_tx = tenant_tx(&fixture.db, other_tenant_id).await;
    let other_visibility: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM order_cancellations WHERE id = $1 OR order_id = $2",
    )
    .bind(created.cancellation_id)
    .bind(visible_order_id)
    .fetch_one(&mut *other_tx)
    .await
    .unwrap();
    other_tx.rollback().await.unwrap();
    assert_eq!(other_visibility, 0);

    let privileges: (bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT has_table_privilege(current_user, 'order_cancellations', 'SELECT'),
               has_table_privilege(current_user, 'order_cancellations', 'INSERT'),
               has_table_privilege(current_user, 'order_cancellations', 'UPDATE'),
               has_table_privilege(current_user, 'order_cancellations', 'DELETE')
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(privileges, (true, true, false, false));
}
