mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_core::dto::{ErrorCode, ErrorResponse, UpdateUserAccessScope};
use wareboxes_server::auth::TENANT_ID_HEADER;
use wareboxes_server::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_server::{routes, state::AppState};

fn request(
    token: &str,
    tenant_id: TenantId,
    idempotency_key: Option<&str>,
    body: Value,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri("/api/orders/cancel")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(idempotency_key) = idempotency_key {
        request = request.header(IDEMPOTENCY_KEY_HEADER, idempotency_key);
    }
    request.body(Body::from(body.to_string())).unwrap()
}

async fn send(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    idempotency_key: Option<&str>,
    body: Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(request(token, tenant_id, idempotency_key, body))
        .await
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn grant_orders(db: &db::Db, tenant_id: TenantId, user_id: i64, suffix: &str) -> i64 {
    let permission = match repo::permissions::find_by_name(db, tenant_id, "orders")
        .await
        .unwrap()
    {
        Some(permission) => permission.id,
        None => repo::permissions::add_permission(db, tenant_id, "orders", Some("Orders"))
            .await
            .unwrap(),
    };
    let role = repo::roles::add_role(db, tenant_id, &format!("orders-{suffix}"), None)
        .await
        .unwrap();
    repo::roles::add_role_permission(db, tenant_id, role, permission)
        .await
        .unwrap();
    repo::roles::add_role_to_user(db, tenant_id, user_id, role)
        .await
        .unwrap();
    role
}

async fn assign_owner_to_facility(
    db: &db::Db,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
) {
    sqlx::query(
        "INSERT INTO inventory_owner_facilities (tenant_id, created, inventory_owner_id, facility_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(tenant_id.get())
    .bind(db::now_iso())
    .bind(inventory_owner_id)
    .bind(facility_id)
    .execute(db)
    .await
    .unwrap();
}

#[tokio::test]
async fn order_cancellation_commands_are_replay_safe() {
    let fixture = Fixture::new().await;
    let worker = fixture.wms_user("order-cancel-idempotency@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, worker.id).await;
    let orders_role = grant_orders(&fixture.db, tenant_id, worker.id, "primary").await;
    let token = auth::create_session(&fixture.db, worker.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let facility = fixture.facility(tenant_id, "Cancel Replay DC").await;
    let other_facility = fixture.facility(tenant_id, "Cancel Replay Other DC").await;
    let owner = fixture
        .inventory_owner(tenant_id, "Cancel Replay Owner")
        .await;
    assign_owner_to_facility(&fixture.db, tenant_id, owner, facility).await;
    let item = fixture.item(tenant_id, "Cancel Replay Item", "each").await;
    let order = fixture.order(tenant_id, "CANCEL-REPLAY-1", owner).await;
    fixture.order_item(order, item, 4).await;
    let body = json!({"order_id": order, "facility_id": facility});

    let missing_key = send(&app, &token, tenant_id, None, body.clone()).await;
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json::<ErrorResponse>(missing_key).await.code,
        ErrorCode::IdempotencyKeyRequired
    );
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let untouched: (String, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT orders.status,
               (SELECT COUNT(*) FROM order_activity
                WHERE tenant_id = orders.tenant_id
                  AND order_id = orders.id
                  AND action = 'cancelled order'),
               (SELECT COUNT(*) FROM unpack_cancelled_order_tasks
                WHERE tenant_id = orders.tenant_id AND order_id = orders.id),
               (SELECT COUNT(*) FROM command_idempotency_records
                WHERE tenant_id = orders.tenant_id AND operation = 'order.cancel.v1')
        FROM orders
        WHERE orders.tenant_id = $1 AND orders.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(order)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(untouched, ("open".to_owned(), 0, 0, 0));

    let first = send(
        &app,
        &token,
        tenant_id,
        Some("cancel-order-1"),
        body.clone(),
    );
    let replay = send(
        &app,
        &token,
        tenant_id,
        Some("cancel-order-1"),
        body.clone(),
    );
    let (first, replay) = tokio::join!(first, replay);
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(replay.status(), StatusCode::OK);
    let task_id = response_json::<i64>(first).await;
    assert_eq!(response_json::<i64>(replay).await, task_id);

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let state: (String, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT orders.status,
               (SELECT COUNT(*) FROM order_activity activity
                WHERE activity.tenant_id = orders.tenant_id
                  AND activity.order_id = orders.id
                  AND activity.action = 'cancelled order'),
               (SELECT COUNT(*) FROM work_tasks task
                INNER JOIN unpack_cancelled_order_tasks detail
                    ON detail.tenant_id = task.tenant_id AND detail.task_id = task.id
                WHERE task.tenant_id = orders.tenant_id AND detail.order_id = orders.id),
               (SELECT COUNT(*) FROM unpack_cancelled_order_task_lines line
                WHERE line.tenant_id = orders.tenant_id AND line.task_id = $3),
               (SELECT COUNT(*) FROM command_idempotency_records command
                WHERE command.tenant_id = orders.tenant_id
                  AND command.operation = 'order.cancel.v1'
                  AND command.idempotency_key = 'cancel-order-1')
        FROM orders
        WHERE orders.tenant_id = $1 AND orders.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(order)
    .bind(task_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(state, ("cancelled".to_owned(), 1, 1, 1, 1));

    let changed_payload = send(
        &app,
        &token,
        tenant_id,
        Some("cancel-order-1"),
        json!({"order_id": order, "facility_id": other_facility}),
    )
    .await;
    assert_eq!(changed_payload.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed_payload).await.code,
        ErrorCode::IdempotencyKeyReused
    );

    let other_actor = fixture.user("order-cancel-replay-actor@test.com").await;
    sqlx::query(
        "INSERT INTO tenant_memberships (tenant_id, user_id, is_default) VALUES ($1, $2, FALSE)",
    )
    .bind(tenant_id.get())
    .bind(other_actor.id)
    .execute(&fixture.db)
    .await
    .unwrap();
    repo::roles::add_role_to_user(&fixture.db, tenant_id, other_actor.id, orders_role)
        .await
        .unwrap();
    let other_token = auth::create_session(&fixture.db, other_actor.id)
        .await
        .unwrap();
    let changed_actor = send(
        &app,
        &other_token,
        tenant_id,
        Some("cancel-order-1"),
        body.clone(),
    )
    .await;
    assert_eq!(changed_actor.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed_actor).await.code,
        ErrorCode::IdempotencyKeyReused
    );

    let second_key = send(
        &app,
        &token,
        tenant_id,
        Some("cancel-order-2"),
        body.clone(),
    );
    let third_key = send(
        &app,
        &token,
        tenant_id,
        Some("cancel-order-3"),
        body.clone(),
    );
    let (second_key, third_key) = tokio::join!(second_key, third_key);
    for response in [second_key, third_key] {
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json::<i64>(response).await, task_id);
    }
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let effects: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT COUNT(*) FROM order_activity
             WHERE tenant_id = $1 AND order_id = $2 AND action = 'cancelled order'),
            (SELECT COUNT(*) FROM unpack_cancelled_order_tasks
             WHERE tenant_id = $1 AND order_id = $2),
            (SELECT COUNT(*) FROM command_idempotency_records
             WHERE tenant_id = $1 AND operation = 'order.cancel.v1')
        "#,
    )
    .bind(tenant_id.get())
    .bind(order)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(effects, (1, 1, 3));

    let direct_task = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/tasks/unpack-cancelled-orders/add")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, tenant_id.to_string())
                .header(header::CONTENT_TYPE, "application/json")
                .header(IDEMPOTENCY_KEY_HEADER, "cancel-order-1")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(direct_task.status(), StatusCode::OK);
    assert_eq!(response_json::<i64>(direct_task).await, task_id);
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let direct_task_command: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM command_idempotency_records WHERE tenant_id = $1 AND operation = 'task.create_unpack_cancelled_order.v1' AND idempotency_key = 'cancel-order-1'",
    )
    .bind(tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(direct_task_command, 1);

    repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: worker.id,
            all_facilities: true,
            facility_ids: Vec::new(),
            all_inventory_owners: false,
            inventory_owner_ids: Vec::new(),
        },
    )
    .await
    .unwrap();
    let revoked_replay = send(
        &app,
        &token,
        tenant_id,
        Some("cancel-order-1"),
        body.clone(),
    )
    .await;
    assert_eq!(revoked_replay.status(), StatusCode::NOT_FOUND);

    repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: worker.id,
            all_facilities: true,
            facility_ids: Vec::new(),
            all_inventory_owners: true,
            inventory_owner_ids: Vec::new(),
        },
    )
    .await
    .unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("UPDATE work_tasks SET deleted = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(db::now_iso())
        .bind(tenant_id.get())
        .bind(task_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let deleted_task_replay = send(&app, &token, tenant_id, Some("cancel-order-1"), body).await;
    assert_eq!(deleted_task_replay.status(), StatusCode::OK);
    assert_eq!(response_json::<i64>(deleted_task_replay).await, task_id);
}

#[tokio::test]
async fn failed_unpack_work_rolls_back_order_cancellation_command() {
    let fixture = Fixture::new().await;
    let worker = fixture.wms_user("order-cancel-rollback@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, worker.id).await;
    grant_orders(&fixture.db, tenant_id, worker.id, "rollback").await;
    let token = auth::create_session(&fixture.db, worker.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let facility = fixture.facility(tenant_id, "Cancel Rollback DC").await;
    let owner = fixture
        .inventory_owner(tenant_id, "Cancel Rollback Owner")
        .await;
    assign_owner_to_facility(&fixture.db, tenant_id, owner, facility).await;
    let item = fixture
        .item(tenant_id, "Cancel Rollback Item", "each")
        .await;
    let order = fixture.order(tenant_id, "CANCEL-ROLLBACK-1", owner).await;
    fixture.order_item(order, item, 2).await;
    sqlx::query("UPDATE items SET deleted = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(db::now_iso())
        .bind(tenant_id.get())
        .bind(item)
        .execute(&fixture.db)
        .await
        .unwrap();
    let body = json!({"order_id": order, "facility_id": facility});

    let failed = send(
        &app,
        &token,
        tenant_id,
        Some("cancel-rollback-1"),
        body.clone(),
    )
    .await;
    assert_eq!(failed.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(failed).await.code,
        ErrorCode::Conflict
    );
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let rolled_back: (String, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT orders.status,
               (SELECT COUNT(*) FROM order_activity
                WHERE tenant_id = orders.tenant_id
                  AND order_id = orders.id
                  AND action = 'cancelled order'),
               (SELECT COUNT(*) FROM unpack_cancelled_order_tasks
                WHERE tenant_id = orders.tenant_id AND order_id = orders.id),
               (SELECT COUNT(*) FROM command_idempotency_records
                WHERE tenant_id = orders.tenant_id
                  AND operation = 'order.cancel.v1'
                  AND idempotency_key = 'cancel-rollback-1')
        FROM orders
        WHERE orders.tenant_id = $1 AND orders.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(order)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(rolled_back, ("open".to_owned(), 0, 0, 0));

    sqlx::query("UPDATE items SET deleted = NULL WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id.get())
        .bind(item)
        .execute(&fixture.db)
        .await
        .unwrap();
    let retry = send(&app, &token, tenant_id, Some("cancel-rollback-1"), body).await;
    assert_eq!(retry.status(), StatusCode::OK);
    response_json::<i64>(retry).await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let committed: (String, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT orders.status,
               (SELECT COUNT(*) FROM order_activity
                WHERE tenant_id = orders.tenant_id
                  AND order_id = orders.id
                  AND action = 'cancelled order'),
               (SELECT COUNT(*) FROM unpack_cancelled_order_tasks
                WHERE tenant_id = orders.tenant_id AND order_id = orders.id),
               (SELECT COUNT(*) FROM command_idempotency_records
                WHERE tenant_id = orders.tenant_id
                  AND operation = 'order.cancel.v1'
                  AND idempotency_key = 'cancel-rollback-1')
        FROM orders
        WHERE orders.tenant_id = $1 AND orders.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(order)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(committed, ("cancelled".to_owned(), 1, 1, 1));
}
