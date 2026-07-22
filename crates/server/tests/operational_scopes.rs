mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_core::dto::{OrderPage, UpdateUserAccessScope};
use wareboxes_core::models::{Load, LoadFileCategory, LoadType, Order};
use wareboxes_server::auth::TENANT_ID_HEADER;
use wareboxes_server::routes;
use wareboxes_server::state::AppState;

async fn grant_permissions(
    db: &db::Db,
    tenant_id: TenantId,
    user_id: i64,
    role_name: &str,
    permission_names: &[&str],
) {
    let role = repo::roles::add_role(db, tenant_id, role_name, Some("Operational scope role"))
        .await
        .unwrap();
    for permission_name in permission_names {
        let permission = repo::permissions::add_permission(
            db,
            tenant_id,
            permission_name,
            Some(permission_name),
        )
        .await
        .unwrap();
        repo::roles::add_role_permission(db, tenant_id, role, permission)
            .await
            .unwrap();
    }
    repo::roles::add_role_to_user(db, tenant_id, user_id, role)
        .await
        .unwrap();
}

fn api_request(
    token: &str,
    tenant_id: TenantId,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    let body = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&body).unwrap())
        }
        None => Body::empty(),
    };
    request.body(body).unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn order_and_load_workflows_enforce_owner_and_facility_scopes() {
    let db = setup().await;
    let administrator =
        auth::register_user(&db, "operations-admin@test.com", "supersecret", None, None)
            .await
            .unwrap();
    let operator = auth::register_user(
        &db,
        "operations-operator@test.com",
        "supersecret",
        None,
        None,
    )
    .await
    .unwrap();
    let tenant_id = tenant_for_user(&db, administrator.id).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
        .bind(tenant_id.get())
        .bind(operator.id)
        .execute(&db)
        .await
        .unwrap();
    grant_permissions(
        &db,
        tenant_id,
        operator.id,
        "scoped-operations",
        &["orders", "wms"],
    )
    .await;

    let allowed_facility = repo::facilities::add_facility(&db, tenant_id, "Allowed Operations DC")
        .await
        .unwrap();
    let denied_facility = repo::facilities::add_facility(&db, tenant_id, "Denied Operations DC")
        .await
        .unwrap();
    let allowed_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "Allowed Operations Owner",
        "allowed.operations@test.com",
    )
    .await
    .unwrap();
    let denied_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "Denied Operations Owner",
        "denied.operations@test.com",
    )
    .await
    .unwrap();
    let allowed_location = repo::locations::add_location(
        &db,
        tenant_id,
        allowed_facility,
        None,
        Some("OPS-ALLOWED"),
        Some("Allowed Receiving"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let denied_location = repo::locations::add_location(
        &db,
        tenant_id,
        denied_facility,
        None,
        Some("OPS-DENIED"),
        Some("Denied Receiving"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let item = repo::items::add_item(
        &db,
        tenant_id,
        "Scoped Operations Item",
        None,
        "each",
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    repo::orders::add_order(&db, tenant_id, &new_order("OPS-ALLOWED", allowed_owner))
        .await
        .unwrap();
    repo::orders::add_order(&db, tenant_id, &new_order("OPS-DENIED", denied_owner))
        .await
        .unwrap();
    let orders = repo::orders::get_orders(&db, tenant_id).await.unwrap();
    let allowed_order = orders
        .iter()
        .find(|order| order.order_key == "OPS-ALLOWED")
        .unwrap()
        .id;
    let denied_order = orders
        .iter()
        .find(|order| order.order_key == "OPS-DENIED")
        .unwrap()
        .id;

    let allowed_load = repo::loads::add_load(
        &db,
        tenant_id,
        administrator.id,
        allowed_facility,
        allowed_owner,
        LoadType::Inbound,
        Some("OPS-ALLOWED-LOAD"),
        None,
        None,
        None,
        None,
        Some(allowed_location),
        None,
        None,
    )
    .await
    .unwrap();
    let denied_load = repo::loads::add_load(
        &db,
        tenant_id,
        administrator.id,
        denied_facility,
        denied_owner,
        LoadType::Inbound,
        Some("OPS-DENIED-LOAD"),
        None,
        None,
        None,
        None,
        Some(denied_location),
        None,
        None,
    )
    .await
    .unwrap();
    let allowed_line = repo::loads::add_line(
        &db,
        tenant_id,
        administrator.id,
        allowed_load,
        item,
        None,
        5,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let denied_line = repo::loads::add_line(
        &db,
        tenant_id,
        administrator.id,
        denied_load,
        item,
        None,
        5,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let denied_note = repo::loads::add_note(
        &db,
        tenant_id,
        administrator.id,
        denied_load,
        "Protected note",
    )
    .await
    .unwrap();
    let denied_file = repo::loads::add_file(
        &db,
        tenant_id,
        administrator.id,
        denied_load,
        "protected.txt",
        "protected-storage.txt",
        "protected/protected-storage.txt",
        Some("text/plain"),
        LoadFileCategory::General,
    )
    .await
    .unwrap();

    repo::tenants::update_user_access_scope(
        &db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: operator.id,
            all_facilities: false,
            facility_ids: vec![allowed_facility],
            all_inventory_owners: false,
            inventory_owner_ids: vec![allowed_owner],
        },
    )
    .await
    .unwrap();
    let token = auth::create_session(&db, operator.id).await.unwrap();
    let app = routes::app(AppState::new(db.clone()));

    let response = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            Method::GET,
            "/api/orders",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let page = response_json::<OrderPage>(response).await;
    assert_eq!(page.page.total, 1);
    assert_eq!(page.page.items.len(), 1);
    assert_eq!(page.page.items[0].id, allowed_order);
    assert_eq!(
        page.summaries
            .iter()
            .map(|summary| summary.count)
            .sum::<i64>(),
        1
    );

    let response = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            Method::GET,
            &format!("/api/orders/{denied_order}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response_json::<Option<Order>>(response).await.is_none());

    let response = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            Method::POST,
            "/api/orders/update",
            Some(json!({"order_id": denied_order, "rush": true})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response_json::<bool>(response).await);

    let response = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            Method::POST,
            "/api/orders/add",
            Some(serde_json::to_value(new_order("OPS-FORBIDDEN", denied_owner)).unwrap()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            Method::GET,
            "/api/loads",
            None,
        ))
        .await
        .unwrap();
    let loads = response_json::<Vec<Load>>(response).await;
    assert_eq!(loads.len(), 1);
    assert_eq!(loads[0].id, allowed_load);

    let response = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            Method::GET,
            &format!("/api/loads/{denied_load}"),
            None,
        ))
        .await
        .unwrap();
    assert!(response_json::<Option<Load>>(response).await.is_none());

    let response = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            Method::POST,
            "/api/loads/update",
            Some(json!({"load_id": denied_load, "carrier": "Unauthorized"})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response_json::<bool>(response).await);

    let response = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            Method::POST,
            "/api/loads/notes/add",
            Some(json!({"load_id": denied_load, "note": "Unauthorized"})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            Method::POST,
            "/api/loads/notes/delete",
            Some(json!({"load_note_id": denied_note})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response_json::<bool>(response).await);

    let response = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            Method::POST,
            "/api/loads/lines/add",
            Some(json!({
                "load_id": denied_load,
                "item_id": item,
                "expected_qty": 1
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            Method::POST,
            "/api/loads/files/add",
            Some(json!({
                "load_id": denied_load,
                "original_name": "unauthorized.txt",
                "name": "unauthorized-storage.txt",
                "path": "unauthorized/unauthorized-storage.txt"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            Method::POST,
            "/api/loads/files/delete",
            Some(json!({"file_id": denied_file})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response_json::<bool>(response).await);

    let response = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            Method::POST,
            "/api/loads/lines/receive",
            Some(json!({
                "load_line_id": denied_line,
                "to_location_id": denied_location,
                "received_qty": 1,
                "rejected_qty": 0,
                "idempotency_key": "denied-line-receipt"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .oneshot(api_request(
            &token,
            tenant_id,
            Method::POST,
            "/api/loads/lines/receive",
            Some(json!({
                "load_line_id": allowed_line,
                "to_location_id": denied_location,
                "received_qty": 1,
                "rejected_qty": 0,
                "idempotency_key": "cross-facility-receipt"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let denied_order = repo::orders::get_order(&db, tenant_id, denied_order)
        .await
        .unwrap()
        .unwrap();
    assert!(!denied_order.rush);
    let denied_load = repo::loads::get_load(&db, tenant_id, denied_load, true)
        .await
        .unwrap()
        .unwrap();
    assert!(denied_load.carrier.is_none());
    assert_eq!(denied_load.notes.len(), 1);
    assert_eq!(denied_load.files.len(), 1);
    assert_eq!(denied_load.lines.len(), 1);
}
