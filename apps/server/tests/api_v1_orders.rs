mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde::Serialize;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::{IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    CreateFulfillmentOrderLineRequest, CreateFulfillmentOrderRequest,
    CreateFulfillmentOrderResponse, CreatedFulfillmentOrderStatus, ErrorReason, ErrorResponse,
    FulfillmentOrderDestination, OrderEntryItemResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;

#[derive(Debug, sqlx::FromRow)]
struct PersistedOrderHeader {
    id: i64,
    inventory_owner_id: i64,
    order_key: String,
    rush: bool,
    status: String,
    revision: i64,
    ship_by: wareboxes_domain::Timestamp,
    line1: String,
    line2: Option<String>,
    city: String,
    state: String,
    postal_code: Option<String>,
    country: String,
}

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
    if let Some(idempotency_key) = idempotency_key {
        request = request
            .header(IDEMPOTENCY_KEY_HEADER, idempotency_key)
            .header(REQUEST_ID_HEADER, format!("request-{idempotency_key}"));
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

fn malformed_order_request(
    token: &str,
    tenant_id: TenantId,
    idempotency_key: &str,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/v1/orders")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(header::CONTENT_TYPE, "application/json")
        .header(IDEMPOTENCY_KEY_HEADER, idempotency_key)
        .header(REQUEST_ID_HEADER, format!("request-{idempotency_key}"))
        .body(Body::from(r#"{"inventory_owner_id": "#))
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn assert_error(response: axum::response::Response, status: StatusCode, reason: ErrorReason) {
    assert_eq!(response.status(), status);
    assert_eq!(
        response_json::<ErrorResponse>(response).await.reason,
        reason
    );
}

async fn grant_permission(db: &db::Db, tenant_id: TenantId, user_id: i64, permission_name: &str) {
    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        db,
        tenant_id,
        permission_name,
        None,
    )
    .await
    .unwrap();
    let role_name = format!("{permission_name}-order-entry-operator");
    let role = wareboxes_persistence_postgres::roles::add_role(
        db,
        tenant_id,
        &role_name,
        Some("Create fulfillment orders"),
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

async fn grant_orders(db: &db::Db, tenant_id: TenantId, user_id: i64) {
    grant_permission(db, tenant_id, user_id, "orders").await;
}

async fn link_item(
    db: &db::Db,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    item_id: i64,
    active: bool,
) {
    let mut tx = tenant_tx(db, tenant_id).await;
    sqlx::query(
        r#"
        INSERT INTO inventory_owner_items
            (tenant_id, created, deleted, inventory_owner_id, item_id)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(tenant_id.get())
    .bind(db::now_iso())
    .bind((!active).then(db::now_iso))
    .bind(inventory_owner_id)
    .bind(item_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

fn order_request(
    inventory_owner_id: i64,
    first_item_id: i64,
    second_item_id: i64,
    order_key: &str,
) -> CreateFulfillmentOrderRequest {
    CreateFulfillmentOrderRequest {
        inventory_owner_id,
        order_key: order_key.into(),
        rush: true,
        ship_by: Some("2027-08-12T10:00:00-07:00".into()),
        destination: FulfillmentOrderDestination {
            line1: "125 Shipping Lane".into(),
            line2: Some("Dock 4".into()),
            city: "Reno".into(),
            region: "NV".into(),
            postal_code: "89502".into(),
            country: "US".into(),
        },
        lines: vec![
            CreateFulfillmentOrderLineRequest {
                line_key: "client-line-20".into(),
                item_id: second_item_id,
                quantity: 4,
                requested_uom: "each".into(),
            },
            CreateFulfillmentOrderLineRequest {
                line_key: "client-line-10".into(),
                item_id: first_item_id,
                quantity: 3,
                requested_uom: "case".into(),
            },
        ],
    }
}

async fn effect_counts(db: &db::Db, tenant_id: TenantId) -> (i64, i64, i64, i64, i64, i64) {
    let mut tx = tenant_tx(db, tenant_id).await;
    let counts = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM orders WHERE tenant_id = $1),
               (SELECT COUNT(*) FROM addresses WHERE tenant_id = $1),
               (SELECT COUNT(*) FROM order_items WHERE tenant_id = $1),
               (SELECT COUNT(*) FROM order_activity WHERE tenant_id = $1),
               (SELECT COUNT(*) FROM outbox_events
                WHERE tenant_id = $1 AND aggregate_type = 'order'),
               (SELECT COUNT(*) FROM command_idempotency_records
                WHERE tenant_id = $1 AND operation = 'order.create.v1')
        "#,
    )
    .bind(tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    counts
}

#[tokio::test]
async fn order_entry_and_creation_are_scoped_atomic_and_replay_safe() {
    let fixture = Fixture::new().await;
    let user = fixture.user("v1-order-entry@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant_permission(&fixture.db, tenant_id, user.id, "admin").await;
    let owner_id = fixture
        .inventory_owner(tenant_id, "Order Entry Client")
        .await;
    let other_owner_id = fixture
        .inventory_owner(tenant_id, "Other Order Entry Client")
        .await;
    let case_item_id = fixture.item(tenant_id, "Case Item", "case").await;
    let each_item_id = fixture.item(tenant_id, "Each Item", "each").await;
    let other_item_id = fixture.item(tenant_id, "Other Client Item", "each").await;
    link_item(&fixture.db, tenant_id, owner_id, case_item_id, true).await;
    link_item(&fixture.db, tenant_id, owner_id, each_item_id, true).await;
    link_item(&fixture.db, tenant_id, owner_id, other_item_id, false).await;
    link_item(&fixture.db, tenant_id, other_owner_id, other_item_id, true).await;

    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let entry_items = app
        .clone()
        .oneshot(api_request::<Value>(
            &token,
            tenant_id,
            Method::GET,
            &format!("/api/v1/inventory-owners/{owner_id}/order-entry-items"),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(entry_items.status(), StatusCode::OK);
    assert_eq!(
        response_json::<Vec<OrderEntryItemResponse>>(entry_items).await,
        vec![
            OrderEntryItemResponse {
                item_id: case_item_id,
                description: Some("Case Item".into()),
                requested_uom: "case".into(),
            },
            OrderEntryItemResponse {
                item_id: each_item_id,
                description: Some("Each Item".into()),
                requested_uom: "each".into(),
            },
        ]
    );
    let other_entry_items = app
        .clone()
        .oneshot(api_request::<Value>(
            &token,
            tenant_id,
            Method::GET,
            &format!("/api/v1/inventory-owners/{other_owner_id}/order-entry-items"),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(other_entry_items.status(), StatusCode::OK);
    assert_eq!(
        response_json::<Vec<OrderEntryItemResponse>>(other_entry_items).await,
        vec![OrderEntryItemResponse {
            item_id: other_item_id,
            description: Some("Other Client Item".into()),
            requested_uom: "each".into(),
        }]
    );

    let request = order_request(owner_id, case_item_id, each_item_id, "WMS-ORDER-1001");
    assert_error(
        app.clone()
            .oneshot(api_request(
                &token,
                tenant_id,
                Method::POST,
                "/api/v1/orders",
                None,
                Some(&request),
            ))
            .await
            .unwrap(),
        StatusCode::BAD_REQUEST,
        ErrorReason::IdempotencyKeyRequired,
    )
    .await;
    assert_error(
        app.clone()
            .oneshot(malformed_order_request(
                &token,
                tenant_id,
                "order-create-malformed",
            ))
            .await
            .unwrap(),
        StatusCode::BAD_REQUEST,
        ErrorReason::InvalidRequest,
    )
    .await;

    let mut missing_lines = request.clone();
    missing_lines.order_key = "WMS-ORDER-NO-LINES".into();
    missing_lines.lines.clear();
    assert_error(
        app.clone()
            .oneshot(api_request(
                &token,
                tenant_id,
                Method::POST,
                "/api/v1/orders",
                Some("order-create-no-lines"),
                Some(&missing_lines),
            ))
            .await
            .unwrap(),
        StatusCode::BAD_REQUEST,
        ErrorReason::InvalidRequest,
    )
    .await;

    let mut invalid_quantity = request.clone();
    invalid_quantity.order_key = "WMS-ORDER-BAD-QUANTITY".into();
    invalid_quantity.lines[0].quantity = 0;
    assert_error(
        app.clone()
            .oneshot(api_request(
                &token,
                tenant_id,
                Method::POST,
                "/api/v1/orders",
                Some("order-create-bad-quantity"),
                Some(&invalid_quantity),
            ))
            .await
            .unwrap(),
        StatusCode::BAD_REQUEST,
        ErrorReason::InvalidRequest,
    )
    .await;

    let mut bad_uom = request.clone();
    bad_uom.order_key = "WMS-ORDER-BAD-UOM".into();
    bad_uom.lines[1].requested_uom = "each".into();
    assert_error(
        app.clone()
            .oneshot(api_request(
                &token,
                tenant_id,
                Method::POST,
                "/api/v1/orders",
                Some("order-create-bad-uom"),
                Some(&bad_uom),
            ))
            .await
            .unwrap(),
        StatusCode::CONFLICT,
        ErrorReason::Conflict,
    )
    .await;

    let mut unlinked_item = request.clone();
    unlinked_item.order_key = "WMS-ORDER-UNLINKED".into();
    unlinked_item.lines[0].item_id = other_item_id;
    unlinked_item.lines[0].requested_uom = "each".into();
    assert_error(
        app.clone()
            .oneshot(api_request(
                &token,
                tenant_id,
                Method::POST,
                "/api/v1/orders",
                Some("order-create-unlinked"),
                Some(&unlinked_item),
            ))
            .await
            .unwrap(),
        StatusCode::CONFLICT,
        ErrorReason::Conflict,
    )
    .await;
    assert_eq!(
        effect_counts(&fixture.db, tenant_id).await,
        (0, 0, 0, 0, 0, 0)
    );

    let first = app.clone().oneshot(api_request(
        &token,
        tenant_id,
        Method::POST,
        "/api/v1/orders",
        Some("order-create-1001"),
        Some(&request),
    ));
    let retry = app.clone().oneshot(api_request(
        &token,
        tenant_id,
        Method::POST,
        "/api/v1/orders",
        Some("order-create-1001"),
        Some(&request),
    ));
    let (first, retry) = tokio::join!(first, retry);
    let first = first.unwrap();
    let retry = retry.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(retry.status(), StatusCode::OK);
    let created: CreateFulfillmentOrderResponse = response_json(first).await;
    assert_eq!(
        response_json::<CreateFulfillmentOrderResponse>(retry).await,
        created
    );
    assert!(created.order_id > 0);
    assert_eq!(created.order_key, "WMS-ORDER-1001");
    assert_eq!(created.status, CreatedFulfillmentOrderStatus::Open);
    assert_eq!(created.revision.get(), 1);
    assert_eq!(created.lines.len(), 2);
    assert_eq!(created.lines[0].line_key, "client-line-20");
    assert_eq!(created.lines[1].line_key, "client-line-10");
    assert!(created.lines[0].order_line_id > 0);
    assert!(created.lines[0].order_line_id < created.lines[1].order_line_id);

    let exact_replay = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            Method::POST,
            "/api/v1/orders",
            Some("order-create-1001"),
            Some(&request),
        ))
        .await
        .unwrap();
    assert_eq!(exact_replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<CreateFulfillmentOrderResponse>(exact_replay).await,
        created
    );

    let mut changed_payload = request.clone();
    changed_payload.rush = false;
    assert_error(
        app.clone()
            .oneshot(api_request(
                &token,
                tenant_id,
                Method::POST,
                "/api/v1/orders",
                Some("order-create-1001"),
                Some(&changed_payload),
            ))
            .await
            .unwrap(),
        StatusCode::CONFLICT,
        ErrorReason::IdempotencyKeyReused,
    )
    .await;
    assert_error(
        app.clone()
            .oneshot(api_request(
                &token,
                tenant_id,
                Method::POST,
                "/api/v1/orders",
                Some("order-create-natural-key-conflict"),
                Some(&request),
            ))
            .await
            .unwrap(),
        StatusCode::CONFLICT,
        ErrorReason::Conflict,
    )
    .await;

    assert_eq!(
        effect_counts(&fixture.db, tenant_id).await,
        (1, 1, 2, 1, 1, 1)
    );
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let header: PersistedOrderHeader = sqlx::query_as(
        r#"
        SELECT orders.id, orders.inventory_owner_id, orders.order_key, orders.rush,
               orders.status, orders.revision, orders.ship_by,
               addresses.line1, addresses.line2, addresses.city, addresses.state,
               addresses.postal_code, addresses.country
        FROM orders
        INNER JOIN addresses
          ON addresses.tenant_id = orders.tenant_id
         AND addresses.id = orders.address_id
        WHERE orders.tenant_id = $1 AND orders.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(created.order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(header.id, created.order_id);
    assert_eq!(header.inventory_owner_id, owner_id);
    assert_eq!(header.order_key, "WMS-ORDER-1001");
    assert!(header.rush);
    assert_eq!(header.status, "open");
    assert_eq!(header.revision, 1);
    assert_eq!(header.ship_by.to_rfc3339(), "2027-08-12T17:00:00+00:00");
    assert_eq!(
        (
            &header.line1,
            header.line2.as_deref(),
            &header.city,
            &header.state,
            header.postal_code.as_deref(),
            &header.country
        ),
        (
            &"125 Shipping Lane".to_owned(),
            Some("Dock 4"),
            &"Reno".to_owned(),
            &"NV".to_owned(),
            Some("89502"),
            &"US".to_owned(),
        )
    );

    let lines: Vec<(i64, String, i64, i64, i64, String)> = sqlx::query_as(
        r#"
        SELECT id, line_key, line_number, item_id, qty, uom
        FROM order_items
        WHERE tenant_id = $1 AND order_id = $2
        ORDER BY line_number
        "#,
    )
    .bind(tenant_id.get())
    .bind(created.order_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        lines,
        vec![
            (
                created.lines[0].order_line_id,
                "client-line-20".into(),
                1,
                each_item_id,
                4,
                "each".into(),
            ),
            (
                created.lines[1].order_line_id,
                "client-line-10".into(),
                2,
                case_item_id,
                3,
                "case".into(),
            ),
        ]
    );

    let activity: (i64, String) = sqlx::query_as(
        r#"
        SELECT actor_user_id, action
        FROM order_activity
        WHERE tenant_id = $1 AND order_id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(created.order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(activity, (user.id, "created fulfillment order".into()));

    let event: (i64, i64, String, String, String, i64, String, i32, Value) = sqlx::query_as(
        r#"
        SELECT inventory_owner_id, actor_user_id, event_key, aggregate_id,
               ordering_key, aggregate_sequence, event_type, schema_version, payload
        FROM outbox_events
        WHERE tenant_id = $1 AND aggregate_type = 'order'
        "#,
    )
    .bind(tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(event.0, owner_id);
    assert_eq!(event.1, user.id);
    assert_eq!(event.2, format!("order:{}:created", created.order_id));
    assert_eq!(event.3, created.order_id.to_string());
    assert_eq!(event.4, format!("order:{}", created.order_id));
    assert_eq!(event.5, 1);
    assert_eq!(event.6, "order.created");
    assert_eq!(event.7, 1);
    assert_eq!(
        event.8,
        json!({
            "order_id": created.order_id,
            "order_key": "WMS-ORDER-1001",
            "inventory_owner_id": owner_id,
            "status": "open",
            "revision": 1,
            "line_count": 2,
            "ordered_quantity": 7,
            "ship_by": "2027-08-12T17:00:00Z",
            "destination": {
                "line1": "125 Shipping Lane",
                "line2": "Dock 4",
                "city": "Reno",
                "region": "NV",
                "postal_code": "89502",
                "country": "US"
            },
            "lines": [
                {
                    "line_key": "client-line-20",
                    "line_number": 1,
                    "item_id": each_item_id,
                    "quantity": 4,
                    "uom": "each"
                },
                {
                    "line_key": "client-line-10",
                    "line_number": 2,
                    "item_id": case_item_id,
                    "quantity": 3,
                    "uom": "case"
                }
            ]
        })
    );

    let command: (i64, String, String, String, Value) = sqlx::query_as(
        r#"
        SELECT actor_user_id, operation, idempotency_key, request_id, result_json
        FROM command_idempotency_records
        WHERE tenant_id = $1 AND operation = 'order.create.v1'
        "#,
    )
    .bind(tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(command.0, user.id);
    assert_eq!(command.1, "order.create.v1");
    assert_eq!(command.2, "order-create-1001");
    assert_eq!(command.3, "request-order-create-1001");
    assert_eq!(command.4, serde_json::to_value(&created).unwrap());
    tx.rollback().await.unwrap();

    repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: user.id,
            all_facilities: true,
            facility_ids: Vec::new(),
            all_inventory_owners: false,
            inventory_owner_ids: Vec::new(),
        },
    )
    .await
    .unwrap();
    let revoked_replay = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            Method::POST,
            "/api/v1/orders",
            Some("order-create-1001"),
            Some(&request),
        ))
        .await
        .unwrap();
    assert!(matches!(
        revoked_replay.status(),
        StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
    ));
    let revoked_reason = response_json::<ErrorResponse>(revoked_replay).await.reason;
    assert!(matches!(
        revoked_reason,
        ErrorReason::Forbidden | ErrorReason::NotFound
    ));
    assert_eq!(
        effect_counts(&fixture.db, tenant_id).await,
        (1, 1, 2, 1, 1, 1)
    );
}

#[tokio::test]
async fn concurrent_natural_key_creates_commit_one_complete_aggregate() {
    let fixture = Fixture::new().await;
    let user = fixture.user("v1-order-natural-key@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant_orders(&fixture.db, tenant_id, user.id).await;
    let owner_id = fixture
        .inventory_owner(tenant_id, "Natural Key Client")
        .await;
    let case_item_id = fixture.item(tenant_id, "Natural Case", "case").await;
    let each_item_id = fixture.item(tenant_id, "Natural Each", "each").await;
    link_item(&fixture.db, tenant_id, owner_id, case_item_id, true).await;
    link_item(&fixture.db, tenant_id, owner_id, each_item_id, true).await;

    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let request = order_request(
        owner_id,
        case_item_id,
        each_item_id,
        "WMS-CONCURRENT-NATURAL-KEY",
    );
    let first = app.clone().oneshot(api_request(
        &token,
        tenant_id,
        Method::POST,
        "/api/v1/orders",
        Some("natural-key-create-a"),
        Some(&request),
    ));
    let second = app.clone().oneshot(api_request(
        &token,
        tenant_id,
        Method::POST,
        "/api/v1/orders",
        Some("natural-key-create-b"),
        Some(&request),
    ));

    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap();
    let second = second.unwrap();
    let (accepted, rejected, accepted_key) = match (first.status(), second.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => (first, second, "natural-key-create-a"),
        (StatusCode::CONFLICT, StatusCode::OK) => (second, first, "natural-key-create-b"),
        statuses => panic!("expected one accepted create and one conflict, got {statuses:?}"),
    };

    let created = response_json::<CreateFulfillmentOrderResponse>(accepted).await;
    let conflict = response_json::<ErrorResponse>(rejected).await;
    assert_eq!(conflict.reason, ErrorReason::Conflict);
    assert_eq!(conflict.message, "order key already exists for client");
    assert_eq!(
        effect_counts(&fixture.db, tenant_id).await,
        (1, 1, 2, 1, 1, 1)
    );

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let command_keys: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT idempotency_key
        FROM command_idempotency_records
        WHERE tenant_id = $1 AND operation = 'order.create.v1'
        "#,
    )
    .bind(tenant_id.get())
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(command_keys, vec![accepted_key.to_owned()]);
    let aggregate: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT orders.id,
               (SELECT COUNT(*) FROM order_items
                WHERE tenant_id = $1 AND order_id = orders.id),
               (SELECT COUNT(*) FROM outbox_events
                WHERE tenant_id = $1
                  AND aggregate_type = 'order'
                  AND aggregate_id = orders.id::TEXT)
        FROM orders
        WHERE orders.tenant_id = $1
          AND orders.inventory_owner_id = $2
          AND orders.order_key = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id)
    .bind("WMS-CONCURRENT-NATURAL-KEY")
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(aggregate, (created.order_id, 2, 1));
}

#[tokio::test]
async fn order_entry_uses_fresh_narrow_owner_scope_and_hides_foreign_owners() {
    let fixture = Fixture::new().await;
    let user = fixture.user("v1-order-narrow-scope@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant_orders(&fixture.db, tenant_id, user.id).await;
    let allowed_owner_id = fixture.inventory_owner(tenant_id, "Allowed Client").await;
    let denied_owner_id = fixture.inventory_owner(tenant_id, "Denied Client").await;
    let allowed_case_id = fixture.item(tenant_id, "Allowed Case", "case").await;
    let allowed_each_id = fixture.item(tenant_id, "Allowed Each", "each").await;
    let denied_case_id = fixture.item(tenant_id, "Denied Case", "case").await;
    let denied_each_id = fixture.item(tenant_id, "Denied Each", "each").await;
    for (owner_id, item_id) in [
        (allowed_owner_id, allowed_case_id),
        (allowed_owner_id, allowed_each_id),
        (denied_owner_id, denied_case_id),
        (denied_owner_id, denied_each_id),
    ] {
        link_item(&fixture.db, tenant_id, owner_id, item_id, true).await;
    }

    let foreign_user = fixture.user("v1-order-foreign-scope@test.local").await;
    let foreign_tenant_id = tenant_for_user(&fixture.db, foreign_user.id).await;
    let foreign_owner_id = fixture
        .inventory_owner(foreign_tenant_id, "Foreign Client")
        .await;
    let foreign_case_id = fixture
        .item(foreign_tenant_id, "Foreign Case", "case")
        .await;
    let foreign_each_id = fixture
        .item(foreign_tenant_id, "Foreign Each", "each")
        .await;
    link_item(
        &fixture.db,
        foreign_tenant_id,
        foreign_owner_id,
        foreign_case_id,
        true,
    )
    .await;
    link_item(
        &fixture.db,
        foreign_tenant_id,
        foreign_owner_id,
        foreign_each_id,
        true,
    )
    .await;

    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let initially_visible = app
        .clone()
        .oneshot(api_request::<Value>(
            &token,
            tenant_id,
            Method::GET,
            &format!("/api/v1/inventory-owners/{denied_owner_id}/order-entry-items"),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(initially_visible.status(), StatusCode::OK);

    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: user.id,
            all_facilities: true,
            facility_ids: Vec::new(),
            all_inventory_owners: false,
            inventory_owner_ids: vec![allowed_owner_id],
        },
    )
    .await
    .unwrap());

    let allowed_catalog = app
        .clone()
        .oneshot(api_request::<Value>(
            &token,
            tenant_id,
            Method::GET,
            &format!("/api/v1/inventory-owners/{allowed_owner_id}/order-entry-items"),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(allowed_catalog.status(), StatusCode::OK);
    assert_eq!(
        response_json::<Vec<OrderEntryItemResponse>>(allowed_catalog)
            .await
            .len(),
        2
    );
    for hidden_owner_id in [denied_owner_id, foreign_owner_id] {
        assert_error(
            app.clone()
                .oneshot(api_request::<Value>(
                    &token,
                    tenant_id,
                    Method::GET,
                    &format!("/api/v1/inventory-owners/{hidden_owner_id}/order-entry-items"),
                    None,
                    None,
                ))
                .await
                .unwrap(),
            StatusCode::NOT_FOUND,
            ErrorReason::NotFound,
        )
        .await;
    }

    let allowed_request = order_request(
        allowed_owner_id,
        allowed_case_id,
        allowed_each_id,
        "WMS-ALLOWED-SCOPE",
    );
    let allowed_create = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            Method::POST,
            "/api/v1/orders",
            Some("order-create-allowed-scope"),
            Some(&allowed_request),
        ))
        .await
        .unwrap();
    assert_eq!(allowed_create.status(), StatusCode::OK);

    for (owner_id, case_item_id, each_item_id, key, idempotency_key) in [
        (
            denied_owner_id,
            denied_case_id,
            denied_each_id,
            "WMS-DENIED-SCOPE",
            "order-create-denied-scope",
        ),
        (
            foreign_owner_id,
            foreign_case_id,
            foreign_each_id,
            "WMS-FOREIGN-SCOPE",
            "order-create-foreign-scope",
        ),
    ] {
        assert_error(
            app.clone()
                .oneshot(api_request(
                    &token,
                    tenant_id,
                    Method::POST,
                    "/api/v1/orders",
                    Some(idempotency_key),
                    Some(&order_request(owner_id, case_item_id, each_item_id, key)),
                ))
                .await
                .unwrap(),
            StatusCode::FORBIDDEN,
            ErrorReason::Forbidden,
        )
        .await;
    }

    assert_eq!(
        effect_counts(&fixture.db, tenant_id).await,
        (1, 1, 2, 1, 1, 1)
    );
    assert_eq!(
        effect_counts(&fixture.db, foreign_tenant_id).await,
        (0, 0, 0, 0, 0, 0)
    );
}

#[tokio::test]
async fn concurrent_order_creation_and_owner_deletion_preserve_owner_lifecycle() {
    let fixture = Fixture::new().await;
    let user = fixture.user("v1-order-owner-delete@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant_orders(&fixture.db, tenant_id, user.id).await;
    let owner_id = fixture.inventory_owner(tenant_id, "Lifecycle Client").await;
    let case_item_id = fixture.item(tenant_id, "Lifecycle Case", "case").await;
    let each_item_id = fixture.item(tenant_id, "Lifecycle Each", "each").await;
    link_item(&fixture.db, tenant_id, owner_id, case_item_id, true).await;
    link_item(&fixture.db, tenant_id, owner_id, each_item_id, true).await;

    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let request = order_request(owner_id, case_item_id, each_item_id, "WMS-OWNER-LIFECYCLE");
    let create = app.oneshot(api_request(
        &token,
        tenant_id,
        Method::POST,
        "/api/v1/orders",
        Some("order-create-owner-lifecycle"),
        Some(&request),
    ));
    let delete = repo::inventory_owners::delete_inventory_owner(&fixture.db, tenant_id, owner_id);
    let (create, delete) = tokio::join!(create, delete);
    let create = create.unwrap();

    let creation_committed = match (create.status(), delete) {
        (StatusCode::OK, Err(AppError::Application(ApplicationError::Conflict(message)))) => {
            assert_eq!(
                message,
                "Inventory owner has orders that are not shipped or cancelled"
            );
            true
        }
        (StatusCode::NOT_FOUND, Ok(true)) => {
            assert_eq!(
                response_json::<ErrorResponse>(create).await.reason,
                ErrorReason::NotFound
            );
            false
        }
        (status, result) => {
            panic!("unexpected create/delete race result: create={status}, delete={result:?}")
        }
    };

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let state: (bool, i64) = sqlx::query_as(
        r#"
        SELECT deleted IS NOT NULL,
               (SELECT COUNT(*)
                FROM orders
                WHERE tenant_id = $1
                  AND inventory_owner_id = $2
                  AND deleted IS NULL
                  AND status NOT IN ('shipped', 'cancelled'))
        FROM inventory_owners
        WHERE tenant_id = $1 AND id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert!(
        !state.0 || state.1 == 0,
        "deleted owner retained an active order"
    );

    if creation_committed {
        assert_eq!(state, (false, 1));
        assert_eq!(
            effect_counts(&fixture.db, tenant_id).await,
            (1, 1, 2, 1, 1, 1)
        );
    } else {
        assert_eq!(state, (true, 0));
        assert_eq!(
            effect_counts(&fixture.db, tenant_id).await,
            (0, 0, 0, 0, 0, 0)
        );
    }
}
