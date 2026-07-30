mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use common::*;
use tower::ServiceExt;
use wareboxes_api_contract::v1::{
    ErrorReason, ErrorResponse, InventoryBalancePage, InventoryBalanceStatus,
};
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_core::models::InventoryStatus;
use wareboxes_server::auth::TENANT_ID_HEADER;
use wareboxes_server::{routes, state::AppState};

fn request(token: &str, tenant_id: TenantId, uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .body(Body::empty())
        .unwrap()
}

async fn body_bytes(response: axum::response::Response) -> Vec<u8> {
    to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap()
        .to_vec()
}

async fn receive_balance(
    fixture: &Fixture,
    tenant_id: TenantId,
    actor_id: i64,
    inventory_owner_id: i64,
    location_id: i64,
    item_key: &str,
    status: InventoryStatus,
) -> i64 {
    let item_id = fixture.item(tenant_id, item_key, "each").await;
    let item_batch_id = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        inventory_owner_id,
        item_id,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    repo::inventory::receive_inventory(
        &fixture.db,
        tenant_id,
        actor_id,
        item_batch_id,
        location_id,
        10,
        Some(status),
        None,
        Some("v1-contract-test"),
        None,
        &format!("receive-{item_key}"),
    )
    .await
    .unwrap();
    item_id
}

#[tokio::test]
async fn inventory_balance_v1_contract_is_scoped_keyset_paginated_and_stable() {
    let fixture = Fixture::new().await;
    let administrator = fixture.user("v1-balances-admin@test.com").await;
    let operator = fixture.user("v1-balances-operator@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, administrator.id).await;

    let mut membership_tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
        .bind(tenant_id.get())
        .bind(operator.id)
        .execute(&mut *membership_tx)
        .await
        .unwrap();
    membership_tx.commit().await.unwrap();

    let permission = repo::permissions::add_permission(&fixture.db, tenant_id, "wms", Some("WMS"))
        .await
        .unwrap();
    let role = repo::roles::add_role(
        &fixture.db,
        tenant_id,
        "v1-balance-reader",
        Some("V1 balance reader"),
    )
    .await
    .unwrap();
    repo::roles::add_role_permission(&fixture.db, tenant_id, role, permission)
        .await
        .unwrap();
    repo::roles::add_role_to_user(&fixture.db, tenant_id, operator.id, role)
        .await
        .unwrap();

    let allowed_facility = fixture.facility(tenant_id, "V1 Allowed DC").await;
    let denied_facility = fixture.facility(tenant_id, "V1 Denied DC").await;
    let allowed_owner = fixture.inventory_owner(tenant_id, "V1 Allowed Owner").await;
    let denied_owner = fixture.inventory_owner(tenant_id, "V1 Denied Owner").await;
    fixture
        .assign_owner_to_facility(tenant_id, allowed_owner, allowed_facility)
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, denied_owner, denied_facility)
        .await;
    let allowed_location = fixture
        .location(tenant_id, allowed_facility, "V1-ALLOWED")
        .await;
    let denied_location = fixture
        .location(tenant_id, denied_facility, "V1-DENIED")
        .await;

    let first_item = receive_balance(
        &fixture,
        tenant_id,
        administrator.id,
        allowed_owner,
        allowed_location,
        "V1 First Item",
        InventoryStatus::Available,
    )
    .await;
    let denied_item = receive_balance(
        &fixture,
        tenant_id,
        administrator.id,
        denied_owner,
        denied_location,
        "V1 Denied Item",
        InventoryStatus::Available,
    )
    .await;
    let second_item = receive_balance(
        &fixture,
        tenant_id,
        administrator.id,
        allowed_owner,
        allowed_location,
        "V1 Second Item",
        InventoryStatus::Damaged,
    )
    .await;

    let first_balance = repo::inventory::get_balances(&fixture.db, tenant_id, false)
        .await
        .unwrap()
        .into_iter()
        .find(|balance| balance.item_id == first_item)
        .unwrap();
    let second_balance = repo::inventory::get_balances(&fixture.db, tenant_id, false)
        .await
        .unwrap()
        .into_iter()
        .find(|balance| balance.item_id == second_item)
        .unwrap();
    let order_id = fixture
        .order(tenant_id, "V1-BALANCE-COMMITMENTS", allowed_owner)
        .await;
    fixture
        .allocated_reservation(
            tenant_id,
            administrator.id,
            order_id,
            first_balance.id,
            2,
            "v1-balance-allocation",
        )
        .await;
    let mut hold_tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        r#"
        INSERT INTO inventory_holds (
            tenant_id, inventory_owner_id, created, modified, created_by,
            inventory_balance_id, facility_id, location_id, license_plate_id,
            item_batch_id, item_id, uom, inventory_status, qty, reason_code,
            note, status
        )
        VALUES (
            $1, $2, $3, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
            3, 'quality_inspection', 'v1 quantity projection', 'active'
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(first_balance.inventory_owner_id.get())
    .bind(db::now_iso())
    .bind(administrator.id)
    .bind(first_balance.id)
    .bind(first_balance.facility_id)
    .bind(first_balance.location_id)
    .bind(first_balance.license_plate_id)
    .bind(first_balance.item_batch_id)
    .bind(first_balance.item_id)
    .bind(&first_balance.uom)
    .bind(first_balance.status.as_str())
    .execute(&mut *hold_tx)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO inventory_holds (
            tenant_id, inventory_owner_id, created, modified, created_by,
            inventory_balance_id, facility_id, location_id, license_plate_id,
            item_batch_id, item_id, uom, inventory_status, qty, reason_code,
            note, status
        )
        VALUES (
            $1, $2, $3, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
            3, 'damage_suspected', 'non-available quantity projection', 'active'
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(second_balance.inventory_owner_id.get())
    .bind(db::now_iso())
    .bind(administrator.id)
    .bind(second_balance.id)
    .bind(second_balance.facility_id)
    .bind(second_balance.location_id)
    .bind(second_balance.license_plate_id)
    .bind(second_balance.item_batch_id)
    .bind(second_balance.item_id)
    .bind(&second_balance.uom)
    .bind(second_balance.status.as_str())
    .execute(&mut *hold_tx)
    .await
    .unwrap();
    hold_tx.commit().await.unwrap();

    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
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
    .unwrap());

    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let first_response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            "/api/v1/inventory/balances?limit=1",
        ))
        .await
        .unwrap();
    assert_eq!(first_response.status(), StatusCode::OK);
    let first_body = body_bytes(first_response).await;
    let first_json: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
    let first_page: InventoryBalancePage = serde_json::from_slice(&first_body).unwrap();
    assert_eq!(first_page.items.len(), 1);
    assert_eq!(first_page.items[0].item_id, first_item);
    assert_eq!(first_page.items[0].facility_id, allowed_facility);
    assert_eq!(first_page.items[0].inventory_owner_name, "V1 Allowed Owner");
    assert_eq!(
        first_page.items[0].location_barcode.as_deref(),
        Some("V1-ALLOWED")
    );
    assert_eq!(
        first_page.items[0].item_description.as_deref(),
        Some("V1 First Item")
    );
    assert!(first_page.items[0].primary_sku.is_none());
    assert_eq!(first_page.items[0].quantity.on_hand, 10);
    assert_eq!(first_page.items[0].quantity.reserved, 2);
    assert_eq!(first_page.items[0].quantity.held, 3);
    assert_eq!(first_page.items[0].quantity.available, 5);
    assert!(first_page.next_cursor.is_some());
    let item_json = &first_json["items"][0];
    for persistence_field in ["tenant_id", "deleted", "created", "modified"] {
        assert!(item_json.get(persistence_field).is_none());
    }

    let cursor = first_page.next_cursor.unwrap();
    let second_response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            &format!(
                "/api/v1/inventory/balances?limit=1&cursor={}",
                cursor.as_str()
            ),
        ))
        .await
        .unwrap();
    assert_eq!(second_response.status(), StatusCode::OK);
    let second_page: InventoryBalancePage =
        serde_json::from_slice(&body_bytes(second_response).await).unwrap();
    assert_eq!(
        second_page
            .items
            .iter()
            .map(|balance| balance.item_id)
            .collect::<Vec<_>>(),
        vec![second_item]
    );
    assert_eq!(second_page.items[0].status, InventoryBalanceStatus::Damaged);
    assert_eq!(second_page.items[0].quantity.on_hand, 10);
    assert_eq!(second_page.items[0].quantity.reserved, 0);
    assert_eq!(second_page.items[0].quantity.held, 3);
    assert_eq!(second_page.items[0].quantity.available, 0);
    assert!(second_page.next_cursor.is_none());
    assert!(!second_page
        .items
        .iter()
        .any(|balance| balance.item_id == denied_item));

    let invalid_cursor = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            "/api/v1/inventory/balances?cursor=not-a-v1-cursor",
        ))
        .await
        .unwrap();
    assert_eq!(invalid_cursor.status(), StatusCode::BAD_REQUEST);
    let error: ErrorResponse = serde_json::from_slice(&body_bytes(invalid_cursor).await).unwrap();
    assert_eq!(error.reason, ErrorReason::InvalidCursor);

    let excessive_limit = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            "/api/v1/inventory/balances?limit=1001",
        ))
        .await
        .unwrap();
    assert_eq!(excessive_limit.status(), StatusCode::BAD_REQUEST);
    let error: ErrorResponse = serde_json::from_slice(&body_bytes(excessive_limit).await).unwrap();
    assert_eq!(error.reason, ErrorReason::InvalidRequest);

    let unauthenticated = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/inventory/balances")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    let request_id = unauthenticated
        .headers()
        .get("x-request-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let error: ErrorResponse = serde_json::from_slice(&body_bytes(unauthenticated).await).unwrap();
    assert_eq!(error.reason, ErrorReason::Unauthorized);
    assert_eq!(error.request_id, request_id);
}
