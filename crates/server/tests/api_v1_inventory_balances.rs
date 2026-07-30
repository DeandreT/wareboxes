mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use common::*;
use tower::ServiceExt;
use wareboxes_api_contract::v1::{
    ErrorReason, ErrorResponse, InventoryBalancePage, InventoryBalanceStatus,
    MAX_INVENTORY_BALANCE_QUERY_LENGTH,
};
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_core::models::{InboundReceiptExceptionReason, InventoryStatus};
use wareboxes_domain::CommandContext;
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

async fn receive_searchable_plate_balance(
    fixture: &Fixture,
    tenant_id: TenantId,
    actor_id: i64,
    inventory_owner_id: i64,
    facility_id: i64,
    location_id: i64,
) -> (i64, i64) {
    let item_id = fixture.item(tenant_id, "V1 First Item", "each").await;
    let load_id = repo::loads::add_load(
        &fixture.db,
        tenant_id,
        actor_id,
        facility_id,
        inventory_owner_id,
        LoadType::Inbound,
        Some("V1-SEARCH-RECEIPT"),
        None,
        None,
        None,
        None,
        Some(location_id),
        None,
        None,
    )
    .await
    .unwrap();
    let load_line_id = repo::loads::add_line(
        &fixture.db,
        tenant_id,
        actor_id,
        load_id,
        item_id,
        None,
        10,
        Some("LOT-SEARCH-ALPHA"),
        Some("SERIAL-SEARCH-ALPHA"),
        None,
    )
    .await
    .unwrap();
    assert!(repo::loads::update_load(
        &fixture.db,
        tenant_id,
        actor_id,
        load_id,
        Some(LoadStatus::Arrived),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap());

    let access = default_tenant_for_user(&fixture.db, actor_id)
        .await
        .unwrap();
    let context = CommandContext {
        tenant_id,
        actor_id: access.user_id,
        request_id: "request-v1-search-receipt".to_owned(),
        idempotency_key: Some("v1-search-receipt".to_owned()),
    };
    let result = repo::inbound_receipt::receive_expected_inventory(
        &fixture.db,
        &access,
        &context,
        load_line_id,
        &repo::inbound_receipt::ReceiveExpectedInventoryCommand {
            receiving_location_id: Some(location_id),
            received_qty: 10,
            rejected_qty: 0,
            missing_qty: 0,
            license_plate_id: None,
            license_plate_barcode: Some("LP-SEARCH-ALPHA"),
            lot: Some("LOT-SEARCH-ALPHA"),
            serial: Some("SERIAL-SEARCH-ALPHA"),
            expiration: None,
            exception_reason: None::<InboundReceiptExceptionReason>,
            exception_note: None,
        },
    )
    .await
    .unwrap();

    (
        item_id,
        result
            .license_plate_id
            .expect("container receipt creates a license plate"),
    )
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
    fixture
        .assign_owner_to_facility(tenant_id, allowed_owner, denied_facility)
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, denied_owner, allowed_facility)
        .await;
    let allowed_location = repo::locations::add_location(
        &fixture.db,
        tenant_id,
        allowed_facility,
        None,
        Some("V1-ALLOWED"),
        Some("Mezzanine Search Zone"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let denied_location = fixture
        .location(tenant_id, denied_facility, "V1-DENIED")
        .await;
    let second_allowed_location = fixture
        .location(tenant_id, allowed_facility, "V1-SECOND")
        .await;
    let site_denied_location = fixture
        .location(tenant_id, denied_facility, "V1-SITE-DENIED")
        .await;
    let owner_denied_location = fixture
        .location(tenant_id, allowed_facility, "V1-OWNER-DENIED")
        .await;

    let (first_item, first_plate) = receive_searchable_plate_balance(
        &fixture,
        tenant_id,
        administrator.id,
        allowed_owner,
        allowed_facility,
        allowed_location,
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
        second_allowed_location,
        "V1 Second Item",
        InventoryStatus::Damaged,
    )
    .await;
    receive_balance(
        &fixture,
        tenant_id,
        administrator.id,
        allowed_owner,
        site_denied_location,
        "V1 Site Denied Item",
        InventoryStatus::Available,
    )
    .await;
    receive_balance(
        &fixture,
        tenant_id,
        administrator.id,
        denied_owner,
        owner_denied_location,
        "V1 Owner Denied Item",
        InventoryStatus::Available,
    )
    .await;

    repo::items::add_sku(&fixture.db, tenant_id, first_item, "SKU-SEARCH-ALPHA", None)
        .await
        .unwrap();
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
    assert_eq!(first_page.items[0].license_plate_id, Some(first_plate));
    assert_eq!(
        first_page.items[0].license_plate_barcode.as_deref(),
        Some("LP-SEARCH-ALPHA")
    );
    assert_eq!(
        first_page.items[0].item_description.as_deref(),
        Some("V1 First Item")
    );
    assert_eq!(
        first_page.items[0].primary_sku.as_deref(),
        Some("SKU-SEARCH-ALPHA")
    );
    assert_eq!(first_page.items[0].lot.as_deref(), Some("LOT-SEARCH-ALPHA"));
    assert_eq!(
        first_page.items[0].serial.as_deref(),
        Some("SERIAL-SEARCH-ALPHA")
    );
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

    for query in [
        "mezzanine",
        "v1-allowed",
        "lp-search-alpha",
        "sku-search-alpha",
        "first",
        "lot-search-alpha",
        "serial-search-alpha",
    ] {
        let response = app
            .clone()
            .oneshot(request(
                &token,
                tenant_id,
                &format!("/api/v1/inventory/balances?query={query}"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "query: {query}");
        let page: InventoryBalancePage =
            serde_json::from_slice(&body_bytes(response).await).unwrap();
        assert_eq!(
            page.items
                .iter()
                .map(|balance| balance.item_id)
                .collect::<Vec<_>>(),
            vec![first_item],
            "query: {query}"
        );
    }

    let id_response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            &format!("/api/v1/inventory/balances?query={}", first_balance.id),
        ))
        .await
        .unwrap();
    assert_eq!(id_response.status(), StatusCode::OK);
    let id_page: InventoryBalancePage =
        serde_json::from_slice(&body_bytes(id_response).await).unwrap();
    assert!(id_page
        .items
        .iter()
        .any(|balance| balance.id == first_balance.id));

    for (label, query) in [("denied", "denied"), ("literal percent", "%25")] {
        let response = app
            .clone()
            .oneshot(request(
                &token,
                tenant_id,
                &format!("/api/v1/inventory/balances?query={query}"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "query: {label}");
        let page: InventoryBalancePage =
            serde_json::from_slice(&body_bytes(response).await).unwrap();
        assert!(page.items.is_empty(), "query: {label}");
    }

    let filtered_first_response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            "/api/v1/inventory/balances?query=v1&limit=1",
        ))
        .await
        .unwrap();
    assert_eq!(filtered_first_response.status(), StatusCode::OK);
    let filtered_first_page: InventoryBalancePage =
        serde_json::from_slice(&body_bytes(filtered_first_response).await).unwrap();
    assert_eq!(filtered_first_page.items.len(), 1);
    let filtered_cursor = filtered_first_page.next_cursor.unwrap();
    let filtered_second_response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            &format!(
                "/api/v1/inventory/balances?query=v1&limit=1&cursor={}",
                filtered_cursor.as_str()
            ),
        ))
        .await
        .unwrap();
    assert_eq!(filtered_second_response.status(), StatusCode::OK);
    let filtered_second_page: InventoryBalancePage =
        serde_json::from_slice(&body_bytes(filtered_second_response).await).unwrap();
    assert_eq!(
        filtered_second_page
            .items
            .iter()
            .map(|balance| balance.item_id)
            .collect::<Vec<_>>(),
        vec![second_item]
    );
    assert!(filtered_second_page.next_cursor.is_none());

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

    let excessive_query = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            &format!(
                "/api/v1/inventory/balances?query={}",
                "x".repeat(MAX_INVENTORY_BALANCE_QUERY_LENGTH + 1)
            ),
        ))
        .await
        .unwrap();
    assert_eq!(excessive_query.status(), StatusCode::BAD_REQUEST);
    let error: ErrorResponse = serde_json::from_slice(&body_bytes(excessive_query).await).unwrap();
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
