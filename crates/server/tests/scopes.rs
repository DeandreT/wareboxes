mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_core::models::{Facility, InventoryOwner, Location, TenantAccess};
use wareboxes_domain::{FacilityId, InventoryOwnerId};
use wareboxes_server::auth::TENANT_ID_HEADER;
use wareboxes_server::routes;
use wareboxes_server::state::AppState;

async fn grant_permissions(
    db: &db::Db,
    tenant_id: TenantId,
    user_id: i64,
    role_name: &str,
    permission_ids: &[i64],
) {
    let role = repo::roles::add_role(db, tenant_id, role_name, Some("Scope test role"))
        .await
        .unwrap();
    for permission_id in permission_ids {
        repo::roles::add_role_permission(db, tenant_id, role, *permission_id)
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
    let bytes = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn selected_resource_scopes_are_projected_and_enforced() {
    let db = setup().await;
    let administrator = auth::register_user(&db, "scope-admin@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let operator = auth::register_user(&db, "scope-operator@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let tenant_id = tenant_for_user(&db, administrator.id).await;

    sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
        .bind(tenant_id.get())
        .bind(operator.id)
        .execute(&db)
        .await
        .unwrap();

    let admin_permission =
        repo::permissions::add_permission(&db, tenant_id, "admin", Some("Admin"))
            .await
            .unwrap();
    let wms_permission = repo::permissions::add_permission(&db, tenant_id, "wms", Some("WMS"))
        .await
        .unwrap();
    grant_permissions(
        &db,
        tenant_id,
        administrator.id,
        "scope-administrator",
        &[admin_permission],
    )
    .await;
    grant_permissions(
        &db,
        tenant_id,
        operator.id,
        "scoped-operator",
        &[admin_permission, wms_permission],
    )
    .await;

    let allowed_facility = repo::facilities::add_facility(&db, tenant_id, "Allowed DC")
        .await
        .unwrap();
    let denied_facility = repo::facilities::add_facility(&db, tenant_id, "Denied DC")
        .await
        .unwrap();
    let allowed_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "Allowed Owner",
        "allowed@test.com",
    )
    .await
    .unwrap();
    let denied_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "Denied Owner",
        "denied@test.com",
    )
    .await
    .unwrap();
    for facility_id in [allowed_facility, denied_facility] {
        sqlx::query(
            r#"
            INSERT INTO inventory_owner_facilities
                (tenant_id, created, inventory_owner_id, facility_id)
            VALUES ($1, CURRENT_TIMESTAMP, $2, $3)
            "#,
        )
        .bind(tenant_id.get())
        .bind(allowed_owner)
        .bind(facility_id)
        .execute(&db)
        .await
        .unwrap();
    }
    let allowed_location = repo::locations::add_location(
        &db,
        tenant_id,
        allowed_facility,
        None,
        Some("ALLOWED-BIN"),
        Some("Allowed Bin"),
        "bin",
        true,
        true,
        false,
    )
    .await
    .unwrap();
    let denied_location = repo::locations::add_location(
        &db,
        tenant_id,
        denied_facility,
        None,
        Some("DENIED-BIN"),
        Some("Denied Bin"),
        "bin",
        true,
        true,
        false,
    )
    .await
    .unwrap();

    let administrator_token = auth::create_session(&db, administrator.id).await.unwrap();
    let operator_token = auth::create_session(&db, operator.id).await.unwrap();
    let app = routes::app(AppState::new(db.clone()));

    let response = app
        .clone()
        .oneshot(api_request(
            &administrator_token,
            tenant_id,
            Method::POST,
            "/api/users/access-scope",
            Some(json!({
                "user_id": operator.id,
                "all_facilities": false,
                "facility_ids": [allowed_facility],
                "all_inventory_owners": false,
                "inventory_owner_ids": [allowed_owner]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response_json::<bool>(response).await);

    let response = app
        .clone()
        .oneshot(api_request(
            &operator_token,
            tenant_id,
            Method::GET,
            "/api/auth/context",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let context = response_json::<TenantAccess>(response).await;
    assert!(!context.site_scope.all_facilities);
    assert_eq!(
        context.site_scope.facility_ids,
        vec![FacilityId::new(allowed_facility).unwrap()]
    );
    assert!(!context.owner_scope.all_inventory_owners);
    assert_eq!(
        context.owner_scope.inventory_owner_ids,
        vec![InventoryOwnerId::new(allowed_owner).unwrap()]
    );

    let response = app
        .clone()
        .oneshot(api_request(
            &operator_token,
            tenant_id,
            Method::GET,
            "/api/facilities",
            None,
        ))
        .await
        .unwrap();
    let facilities = response_json::<Vec<Facility>>(response).await;
    assert_eq!(facilities.len(), 1);
    assert_eq!(facilities[0].id, allowed_facility);

    let response = app
        .clone()
        .oneshot(api_request(
            &operator_token,
            tenant_id,
            Method::GET,
            "/api/inventory-owners",
            None,
        ))
        .await
        .unwrap();
    let inventory_owners = response_json::<Vec<InventoryOwner>>(response).await;
    assert_eq!(inventory_owners.len(), 1);
    assert_eq!(inventory_owners[0].id, allowed_owner);
    assert_eq!(inventory_owners[0].inventory_owner_facilities.len(), 1);
    assert_eq!(
        inventory_owners[0].inventory_owner_facilities[0].id,
        allowed_facility
    );

    let response = app
        .clone()
        .oneshot(api_request(
            &operator_token,
            tenant_id,
            Method::GET,
            "/api/locations",
            None,
        ))
        .await
        .unwrap();
    let locations = response_json::<Vec<Location>>(response).await;
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].id, allowed_location);

    let response = app
        .clone()
        .oneshot(api_request(
            &operator_token,
            tenant_id,
            Method::POST,
            "/api/locations/update",
            Some(json!({
                "location_id": denied_location,
                "name": "Unauthorized change"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response_json::<bool>(response).await);

    let response = app
        .clone()
        .oneshot(api_request(
            &operator_token,
            tenant_id,
            Method::POST,
            "/api/inventory-owners/update",
            Some(json!({
                "inventory_owner_id": denied_owner,
                "name": "Unauthorized Owner"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = app
        .oneshot(api_request(
            &operator_token,
            tenant_id,
            Method::POST,
            "/api/inventory-owners/add",
            Some(json!({
                "name": "Unscoped Owner",
                "email": "unscoped@test.com"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn scope_replacement_is_atomic_and_delegation_cannot_escalate() {
    let db = setup().await;
    let administrator = auth::register_user(
        &db,
        "scope-atomic-admin@test.com",
        "supersecret",
        None,
        None,
    )
    .await
    .unwrap();
    let operator = auth::register_user(
        &db,
        "scope-atomic-operator@test.com",
        "supersecret",
        None,
        None,
    )
    .await
    .unwrap();
    let outsider = auth::register_user(
        &db,
        "scope-atomic-outsider@test.com",
        "supersecret",
        None,
        None,
    )
    .await
    .unwrap();
    let tenant_id = tenant_for_user(&db, administrator.id).await;
    let outsider_tenant_id = tenant_for_user(&db, outsider.id).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
        .bind(tenant_id.get())
        .bind(operator.id)
        .execute(&db)
        .await
        .unwrap();

    let admin_permission =
        repo::permissions::add_permission(&db, tenant_id, "admin", Some("Admin"))
            .await
            .unwrap();
    grant_permissions(
        &db,
        tenant_id,
        administrator.id,
        "atomic-administrator",
        &[admin_permission],
    )
    .await;
    grant_permissions(
        &db,
        tenant_id,
        operator.id,
        "atomic-operator",
        &[admin_permission],
    )
    .await;

    let facility = repo::facilities::add_facility(&db, tenant_id, "Selected DC")
        .await
        .unwrap();
    let owner = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "Selected Owner",
        "selected@test.com",
    )
    .await
    .unwrap();
    let foreign_facility = repo::facilities::add_facility(&db, outsider_tenant_id, "Foreign DC")
        .await
        .unwrap();

    let administrator_token = auth::create_session(&db, administrator.id).await.unwrap();
    let operator_token = auth::create_session(&db, operator.id).await.unwrap();
    let app = routes::app(AppState::new(db.clone()));
    let selected_scope = json!({
        "user_id": operator.id,
        "all_facilities": false,
        "facility_ids": [facility],
        "all_inventory_owners": false,
        "inventory_owner_ids": [owner]
    });

    let response = app
        .clone()
        .oneshot(api_request(
            &administrator_token,
            tenant_id,
            Method::POST,
            "/api/users/access-scope",
            Some(selected_scope),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response_json::<bool>(response).await);

    let response = app
        .clone()
        .oneshot(api_request(
            &administrator_token,
            tenant_id,
            Method::POST,
            "/api/users/access-scope",
            Some(json!({
                "user_id": operator.id,
                "all_facilities": false,
                "facility_ids": [foreign_facility],
                "all_inventory_owners": false,
                "inventory_owner_ids": [owner]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let access = repo::tenants::access_for_user(&db, operator.id, tenant_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        access.site_scope.facility_ids,
        vec![FacilityId::new(facility).unwrap()]
    );
    assert_eq!(
        access.owner_scope.inventory_owner_ids,
        vec![InventoryOwnerId::new(owner).unwrap()]
    );

    let response = app
        .clone()
        .oneshot(api_request(
            &administrator_token,
            tenant_id,
            Method::POST,
            "/api/users/access-scope",
            Some(json!({
                "user_id": operator.id,
                "all_facilities": false,
                "facility_ids": [facility, facility],
                "all_inventory_owners": false,
                "inventory_owner_ids": [owner]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .oneshot(api_request(
            &operator_token,
            tenant_id,
            Method::POST,
            "/api/users/access-scope",
            Some(json!({
                "user_id": operator.id,
                "all_facilities": true,
                "facility_ids": [],
                "all_inventory_owners": false,
                "inventory_owner_ids": [owner]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let access = repo::tenants::access_for_user(&db, operator.id, tenant_id)
        .await
        .unwrap()
        .unwrap();
    assert!(!access.site_scope.all_facilities);
    assert_eq!(
        access.site_scope.facility_ids,
        vec![FacilityId::new(facility).unwrap()]
    );
}
