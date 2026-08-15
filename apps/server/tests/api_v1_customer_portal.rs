mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::CustomerPortalWorkspaceResponse;
use wareboxes_core::dto::UpdateUserAccessScope;

async fn grant_permission(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    permission_name: &str,
) {
    let permission = match wareboxes_persistence_postgres::permissions::find_by_name(
        &fixture.db,
        tenant_id,
        permission_name,
    )
    .await
    .unwrap()
    {
        Some(permission) => permission.id,
        None => wareboxes_persistence_postgres::permissions::add_permission(
            &fixture.db,
            tenant_id,
            permission_name,
            Some("Customer portal test permission"),
        )
        .await
        .unwrap(),
    };
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("{permission_name}-customer-portal-test"),
        None,
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        permission,
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_to_user(&fixture.db, tenant_id, user_id, role)
        .await
        .unwrap();
}

fn request(token: &str, tenant_id: TenantId, path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .body(Body::empty())
        .unwrap()
}

async fn body(response: axum::response::Response) -> (StatusCode, Vec<u8>) {
    let status = response.status();
    let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, body)
}

#[tokio::test]
async fn customer_portal_is_permission_and_scope_safe_and_omits_internal_positions() {
    let fixture = Fixture::new().await;
    let user = fixture.user("customer-portal-reader@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant_permission(&fixture, tenant_id, user.id, "wms").await;

    let allowed_owner = fixture.inventory_owner(tenant_id, "Allowed Client").await;
    let hidden_owner = fixture.inventory_owner(tenant_id, "Hidden Client").await;
    let allowed_facility = fixture.facility(tenant_id, "Allowed Facility").await;
    let hidden_facility = fixture.facility(tenant_id, "Hidden Facility").await;
    fixture
        .assign_owner_to_facility(tenant_id, allowed_owner, allowed_facility)
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, hidden_owner, hidden_facility)
        .await;
    let allowed_item = fixture.item(tenant_id, "Allowed Widget", "each").await;
    let hidden_item = fixture.item(tenant_id, "Hidden Widget", "each").await;
    let unrestricted = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let allowed_balance = fixture
        .received_balance(
            &unrestricted,
            ReceivedBalanceSetup {
                inventory_owner_id: allowed_owner,
                facility_id: allowed_facility,
                item_id: allowed_item,
                qty: 17,
                key: "PORTAL-ALLOWED-BALANCE",
            },
        )
        .await;
    let hidden_balance = fixture
        .received_balance(
            &unrestricted,
            ReceivedBalanceSetup {
                inventory_owner_id: hidden_owner,
                facility_id: hidden_facility,
                item_id: hidden_item,
                qty: 29,
                key: "PORTAL-HIDDEN-BALANCE",
            },
        )
        .await;

    let allowed_order = fixture
        .order_header(tenant_id, "PORTAL-ORDER-ALLOWED", allowed_owner)
        .await;
    fixture
        .order_item(tenant_id, allowed_order, allowed_item, 4)
        .await;
    fixture
        .allocated_reservation(
            tenant_id,
            user.id,
            allowed_order,
            allowed_balance.balance_id,
            4,
            "portal-allowed-reservation",
        )
        .await;
    let hidden_order = fixture
        .order_header(tenant_id, "PORTAL-ORDER-HIDDEN", hidden_owner)
        .await;
    fixture
        .order_item(tenant_id, hidden_order, hidden_item, 5)
        .await;
    fixture
        .allocated_reservation(
            tenant_id,
            user.id,
            hidden_order,
            hidden_balance.balance_id,
            5,
            "portal-hidden-reservation",
        )
        .await;

    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: user.id,
            all_facilities: false,
            facility_ids: vec![allowed_facility],
            all_inventory_owners: false,
            inventory_owner_ids: vec![allowed_owner],
        },
    )
    .await
    .unwrap());

    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let forbidden = app
        .clone()
        .oneshot(request(&token, tenant_id, "/api/v1/portal/workspace"))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    grant_permission(&fixture, tenant_id, user.id, "customer_portal").await;
    let (status, workspace_body) = body(
        app.clone()
            .oneshot(request(
                &token,
                tenant_id,
                "/api/v1/portal/workspace?include_history=true",
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&workspace_body)
    );
    let workspace: CustomerPortalWorkspaceResponse =
        serde_json::from_slice(&workspace_body).unwrap();
    assert_eq!(workspace.inventory.len(), 1);
    assert_eq!(workspace.inventory[0].inventory_owner_id, allowed_owner);
    assert_eq!(workspace.inventory[0].facility_id, allowed_facility);
    assert_eq!(workspace.inventory[0].on_hand, 17);
    assert_eq!(workspace.inventory[0].reserved, 4);
    assert_eq!(workspace.inventory[0].available, 13);
    assert_eq!(workspace.orders.len(), 1);
    assert_eq!(workspace.orders[0].order_key, "PORTAL-ORDER-ALLOWED");
    let wire = String::from_utf8(workspace_body).unwrap();
    assert!(!wire.contains("PORTAL-ORDER-HIDDEN"));
    assert!(!wire.contains("Hidden Widget"));
    assert!(!wire.contains("location_id"));
    assert!(!wire.contains("license_plate"));
    assert!(!wire.contains("tenant_id"));

    let out_of_scope = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            &format!("/api/v1/portal/workspace?inventory_owner_id={hidden_owner}"),
        ))
        .await
        .unwrap();
    assert_eq!(out_of_scope.status(), StatusCode::FORBIDDEN);

    let (status, report) = body(
        app.clone()
            .oneshot(request(
                &token,
                tenant_id,
                "/api/v1/portal/reports/inventory.csv",
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let report = String::from_utf8(report).unwrap();
    assert!(report.contains("Allowed Widget"));
    assert!(!report.contains("Hidden Widget"));
    assert!(!report.contains("location"));

    let missing_document = app
        .oneshot(request(
            &token,
            tenant_id,
            "/api/v1/portal/documents/999999/content",
        ))
        .await
        .unwrap();
    assert_eq!(missing_document.status(), StatusCode::NOT_FOUND);
}
