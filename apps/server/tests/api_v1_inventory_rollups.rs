mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use common::*;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    ErrorReason, ErrorResponse, InventoryFacilityRollupPage, InventoryItemRollupPage,
    InventoryLocationRollupPage,
};
use wareboxes_core::dto::UpdateUserAccessScope;

fn request(token: &str, tenant_id: TenantId, uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .body(Body::empty())
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let bytes = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn receive(
    fixture: &Fixture,
    tenant_id: TenantId,
    actor_id: i64,
    item_batch_id: i64,
    location_id: i64,
    quantity: i64,
    key: &str,
) -> i64 {
    repo::inventory::receive_inventory(
        &fixture.db,
        tenant_id,
        actor_id,
        item_batch_id,
        location_id,
        quantity,
        None,
        None,
        Some("inventory-rollup-test"),
        None,
        key,
    )
    .await
    .unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let balance_id = sqlx::query_scalar(
        r#"
        SELECT id
        FROM inventory_balances
        WHERE tenant_id = $1
          AND item_batch_id = $2
          AND location_id = $3
          AND status = 'available'
          AND deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(item_batch_id)
    .bind(location_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    balance_id
}

async fn place_hold(
    fixture: &Fixture,
    tenant_id: TenantId,
    actor_id: i64,
    balance_id: i64,
    quantity: i64,
) {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let balance = sqlx::query(
        r#"
        SELECT inventory_owner_id, facility_id, location_id, license_plate_id,
               item_batch_id, item_id, uom, status
        FROM inventory_balances
        WHERE tenant_id = $1 AND id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    use sqlx::Row as _;
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
            $13, 'quality_inspection', 'rollup projection', 'active'
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(balance.try_get::<i64, _>("inventory_owner_id").unwrap())
    .bind(db::now_iso())
    .bind(actor_id)
    .bind(balance_id)
    .bind(balance.try_get::<i64, _>("facility_id").unwrap())
    .bind(balance.try_get::<i64, _>("location_id").unwrap())
    .bind(
        balance
            .try_get::<Option<i64>, _>("license_plate_id")
            .unwrap(),
    )
    .bind(balance.try_get::<i64, _>("item_batch_id").unwrap())
    .bind(balance.try_get::<i64, _>("item_id").unwrap())
    .bind(balance.try_get::<String, _>("uom").unwrap())
    .bind(balance.try_get::<String, _>("status").unwrap())
    .bind(quantity)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn inventory_rollups_are_typed_scoped_searchable_and_paginated() {
    let fixture = Fixture::new().await;
    let administrator = fixture.user("rollups-admin@test.com").await;
    let operator = fixture.user("rollups-operator@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, administrator.id).await;

    let mut membership_tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
        .bind(tenant_id.get())
        .bind(operator.id)
        .execute(&mut *membership_tx)
        .await
        .unwrap();
    membership_tx.commit().await.unwrap();
    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        &fixture.db,
        tenant_id,
        "wms",
        Some("WMS"),
    )
    .await
    .unwrap();
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        "inventory-rollup-reader",
        Some("Inventory rollup reader"),
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
    wareboxes_persistence_postgres::roles::add_role_to_user(
        &fixture.db,
        tenant_id,
        operator.id,
        role,
    )
    .await
    .unwrap();

    let first_facility = fixture.facility(tenant_id, "Rollup North").await;
    let second_facility = fixture.facility(tenant_id, "Rollup South").await;
    let denied_facility = fixture.facility(tenant_id, "Rollup Hidden").await;
    let allowed_owner = fixture.inventory_owner(tenant_id, "Rollup Client").await;
    let denied_owner = fixture.inventory_owner(tenant_id, "Hidden Client").await;
    fixture
        .assign_owner_to_facility(tenant_id, allowed_owner, first_facility)
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, allowed_owner, second_facility)
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, denied_owner, denied_facility)
        .await;
    let first_location = fixture
        .location(tenant_id, first_facility, "ROLLUP-N-01")
        .await;
    let second_location = fixture
        .location(tenant_id, first_facility, "ROLLUP-N-02")
        .await;
    let fourth_location = fixture
        .location(tenant_id, first_facility, "ROLLUP-N-03")
        .await;
    let third_location = fixture
        .location(tenant_id, second_facility, "ROLLUP-S-01")
        .await;
    let denied_location = fixture
        .location(tenant_id, denied_facility, "ROLLUP-HIDDEN")
        .await;

    let item_id = fixture.item(tenant_id, "Rollup Widget", "each").await;
    let first_batch = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        allowed_owner,
        item_id,
        None,
        Some("ROLLUP-LOT-1"),
        None,
        None,
    )
    .await
    .unwrap();
    let second_batch = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        allowed_owner,
        item_id,
        None,
        Some("ROLLUP-LOT-2"),
        None,
        None,
    )
    .await
    .unwrap();
    let denied_batch = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        denied_owner,
        item_id,
        None,
        Some("ROLLUP-HIDDEN"),
        None,
        None,
    )
    .await
    .unwrap();
    let committed_balance = receive(
        &fixture,
        tenant_id,
        administrator.id,
        first_batch,
        first_location,
        10,
        "rollup-first-batch",
    )
    .await;
    receive(
        &fixture,
        tenant_id,
        administrator.id,
        second_batch,
        fourth_location,
        5,
        "rollup-second-batch",
    )
    .await;
    receive(
        &fixture,
        tenant_id,
        administrator.id,
        first_batch,
        second_location,
        7,
        "rollup-second-location",
    )
    .await;
    receive(
        &fixture,
        tenant_id,
        administrator.id,
        first_batch,
        third_location,
        4,
        "rollup-second-facility",
    )
    .await;
    receive(
        &fixture,
        tenant_id,
        administrator.id,
        denied_batch,
        denied_location,
        99,
        "rollup-hidden",
    )
    .await;

    let order_id = fixture
        .order_header(tenant_id, "ROLLUP-COMMITMENTS", allowed_owner)
        .await;
    fixture
        .allocated_reservation(
            tenant_id,
            administrator.id,
            order_id,
            committed_balance,
            2,
            "rollup-allocation",
        )
        .await;
    place_hold(&fixture, tenant_id, administrator.id, committed_balance, 3).await;

    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: operator.id,
            all_facilities: false,
            facility_ids: vec![first_facility, second_facility],
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

    let location_response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            "/api/v1/inventory/rollups/by-location?limit=100",
        ))
        .await
        .unwrap();
    assert_eq!(location_response.status(), StatusCode::OK);
    let location_page: InventoryLocationRollupPage = response_json(location_response).await;
    assert_eq!(location_page.items.len(), 4);
    assert!(location_page
        .items
        .iter()
        .all(|row| row.inventory_owner_id == allowed_owner));
    let first_location_row = location_page
        .items
        .iter()
        .find(|row| row.location_id == first_location)
        .unwrap();
    assert_eq!(first_location_row.item_id, item_id);
    assert_eq!(first_location_row.inventory_owner_name, "Rollup Client");
    assert_eq!(
        first_location_row.location_barcode.as_deref(),
        Some("ROLLUP-N-01")
    );
    assert_eq!(first_location_row.balance_count, 1);
    assert_eq!(first_location_row.batch_count, 1);
    assert_eq!(first_location_row.quantities.len(), 1);
    assert_eq!(first_location_row.quantities[0].uom, "each");
    assert_eq!(first_location_row.quantities[0].quantity.on_hand, 10);
    assert_eq!(first_location_row.quantities[0].quantity.reserved, 2);
    assert_eq!(first_location_row.quantities[0].quantity.held, 3);
    assert_eq!(first_location_row.quantities[0].quantity.available, 5);

    let sorted_response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            "/api/v1/inventory/rollups/by-location?limit=1&sort=scope&direction=descending",
        ))
        .await
        .unwrap();
    assert_eq!(sorted_response.status(), StatusCode::OK);
    let sorted_page: InventoryLocationRollupPage = response_json(sorted_response).await;
    assert_eq!(sorted_page.items.len(), 1);
    assert_eq!(
        sorted_page.items[0].location_barcode.as_deref(),
        Some("ROLLUP-S-01")
    );
    let sorted_cursor = sorted_page.next_cursor.unwrap();
    let sorted_second_response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            &format!(
                "/api/v1/inventory/rollups/by-location?limit=1&sort=scope&direction=descending&cursor={}",
                sorted_cursor.as_str()
            ),
        ))
        .await
        .unwrap();
    assert_eq!(sorted_second_response.status(), StatusCode::OK);
    let sorted_second: InventoryLocationRollupPage = response_json(sorted_second_response).await;
    assert_eq!(
        sorted_second.items[0].location_barcode.as_deref(),
        Some("ROLLUP-N-03")
    );

    let searched_response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            "/api/v1/inventory/rollups/by-location?query=ROLLUP-N-02",
        ))
        .await
        .unwrap();
    assert_eq!(searched_response.status(), StatusCode::OK);
    let searched_page: InventoryLocationRollupPage = response_json(searched_response).await;
    assert_eq!(searched_page.items.len(), 1);
    assert_eq!(
        searched_page.items[0].location_barcode.as_deref(),
        Some("ROLLUP-N-02")
    );

    let mismatched_sort_cursor = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            &format!(
                "/api/v1/inventory/rollups/by-location?limit=1&sort=client&direction=descending&cursor={}",
                sorted_cursor.as_str()
            ),
        ))
        .await
        .unwrap();
    assert_eq!(mismatched_sort_cursor.status(), StatusCode::BAD_REQUEST);

    let facility_response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            "/api/v1/inventory/rollups/by-facility",
        ))
        .await
        .unwrap();
    assert_eq!(facility_response.status(), StatusCode::OK);
    let facility_page: InventoryFacilityRollupPage = response_json(facility_response).await;
    assert_eq!(facility_page.items.len(), 2);
    let first_facility_row = facility_page
        .items
        .iter()
        .find(|row| row.facility_id == first_facility)
        .unwrap();
    assert_eq!(first_facility_row.balance_count, 3);
    assert_eq!(first_facility_row.batch_count, 2);
    assert_eq!(first_facility_row.location_count, 3);
    assert_eq!(first_facility_row.quantities[0].quantity.on_hand, 22);
    assert_eq!(first_facility_row.quantities[0].quantity.available, 17);

    let item_response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            "/api/v1/inventory/rollups/by-item",
        ))
        .await
        .unwrap();
    assert_eq!(item_response.status(), StatusCode::OK);
    let item_page: InventoryItemRollupPage = response_json(item_response).await;
    assert_eq!(item_page.items.len(), 1);
    let item_row = &item_page.items[0];
    assert_eq!(item_row.balance_count, 4);
    assert_eq!(item_row.batch_count, 2);
    assert_eq!(item_row.location_count, 4);
    assert_eq!(item_row.facility_count, 2);
    assert_eq!(item_row.quantities[0].quantity.on_hand, 26);
    assert_eq!(item_row.quantities[0].quantity.reserved, 2);
    assert_eq!(item_row.quantities[0].quantity.held, 3);
    assert_eq!(item_row.quantities[0].quantity.available, 21);

    let first_page_response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            "/api/v1/inventory/rollups/by-location?limit=1",
        ))
        .await
        .unwrap();
    let first_page: InventoryLocationRollupPage = response_json(first_page_response).await;
    assert_eq!(first_page.items.len(), 1);
    let cursor = first_page.next_cursor.unwrap();
    let second_page_response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            &format!(
                "/api/v1/inventory/rollups/by-location?limit=100&cursor={}",
                cursor.as_str()
            ),
        ))
        .await
        .unwrap();
    let second_page: InventoryLocationRollupPage = response_json(second_page_response).await;
    assert_eq!(second_page.items.len(), 3);
    assert!(second_page
        .items
        .iter()
        .all(|row| row.location_id != first_page.items[0].location_id));

    let wrong_dimension_cursor = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            &format!(
                "/api/v1/inventory/rollups/by-facility?cursor={}",
                cursor.as_str()
            ),
        ))
        .await
        .unwrap();
    assert_eq!(wrong_dimension_cursor.status(), StatusCode::BAD_REQUEST);
    let error: ErrorResponse = response_json(wrong_dimension_cursor).await;
    assert_eq!(error.reason, ErrorReason::InvalidCursor);

    let excessive_limit = app
        .oneshot(request(
            &token,
            tenant_id,
            "/api/v1/inventory/rollups/by-item?limit=1001",
        ))
        .await
        .unwrap();
    assert_eq!(excessive_limit.status(), StatusCode::BAD_REQUEST);
}
