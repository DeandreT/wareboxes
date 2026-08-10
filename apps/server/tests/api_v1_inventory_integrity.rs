mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use common::*;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::{repo, routes, state::AppState};
use wareboxes_api_contract::v1::{
    ErrorReason, ErrorResponse, InventoryIntegrityIssueKind, InventoryIntegrityPage,
    InventoryJournalPage,
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

async fn response<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "could not decode {status} response as {}: {error}; body={}",
            std::any::type_name::<T>(),
            String::from_utf8_lossy(&bytes)
        )
    })
}

#[allow(
    clippy::too_many_arguments,
    reason = "the fixture helper keeps each inventory dimension explicit at call sites"
)]
async fn receive(
    fixture: &Fixture,
    tenant_id: TenantId,
    actor_id: i64,
    owner_id: i64,
    location_id: i64,
    item_id: i64,
    lot: &str,
    quantity: i64,
    key: &str,
) -> (i64, i64) {
    let batch_id = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        owner_id,
        item_id,
        None,
        Some(lot),
        None,
        None,
    )
    .await
    .unwrap();
    let transaction_id = repo::inventory::receive_inventory(
        &fixture.db,
        tenant_id,
        actor_id,
        batch_id,
        location_id,
        quantity,
        None,
        Some("integrity test receipt"),
        Some("inventory-integrity-test"),
        Some(batch_id),
        key,
    )
    .await
    .unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let balance_id = sqlx::query_scalar(
        "SELECT id FROM inventory_balances WHERE tenant_id=$1 AND item_batch_id=$2 AND location_id=$3 AND deleted IS NULL",
    )
    .bind(tenant_id.get())
    .bind(batch_id)
    .bind(location_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    (transaction_id, balance_id)
}

#[tokio::test]
async fn inventory_journal_is_scope_safe_filter_bound_and_sorted_across_pages() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("inventory-integrity@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let allowed_owner = fixture.inventory_owner(tenant_id, "Visible Client").await;
    let denied_owner = fixture.inventory_owner(tenant_id, "Hidden Client").await;
    let allowed_facility = fixture.facility(tenant_id, "Visible Facility").await;
    let denied_facility = fixture.facility(tenant_id, "Hidden Facility").await;
    fixture
        .assign_owner_to_facility(tenant_id, allowed_owner, allowed_facility)
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, denied_owner, denied_facility)
        .await;
    let allowed_location = fixture
        .location(tenant_id, allowed_facility, "TRACE-VISIBLE")
        .await;
    let denied_location = fixture
        .location(tenant_id, denied_facility, "TRACE-HIDDEN")
        .await;
    let first_item = fixture.item(tenant_id, "Trace First", "each").await;
    let second_item = fixture.item(tenant_id, "Trace Second", "each").await;
    let denied_item = fixture.item(tenant_id, "Trace Hidden", "each").await;
    let (first_transaction, _) = receive(
        &fixture,
        tenant_id,
        operator.id,
        allowed_owner,
        allowed_location,
        first_item,
        "TRACE-LOT-FIRST",
        3,
        "trace-first",
    )
    .await;
    let (second_transaction, _) = receive(
        &fixture,
        tenant_id,
        operator.id,
        allowed_owner,
        allowed_location,
        second_item,
        "TRACE-LOT-SECOND",
        9,
        "trace-second",
    )
    .await;
    let _ = receive(
        &fixture,
        tenant_id,
        operator.id,
        denied_owner,
        denied_location,
        denied_item,
        "TRACE-LOT-HIDDEN",
        99,
        "trace-hidden",
    )
    .await;
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
    let token = wareboxes_api::auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let first = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            "/api/v1/inventory/journal?sort=net_quantity&direction=ascending&limit=1",
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: InventoryJournalPage = response(first).await;
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].id, first_transaction);
    assert_eq!(first.items[0].net_quantity, 3);
    assert_eq!(first.items[0].inventory_owner_name, "Visible Client");
    assert_eq!(first.items[0].entries[0].facility_name, "Visible Facility");
    assert_eq!(
        first.items[0].entries[0].location_barcode.as_deref(),
        Some("TRACE-VISIBLE")
    );
    assert_eq!(
        first.items[0].entries[0].lot.as_deref(),
        Some("TRACE-LOT-FIRST")
    );
    let cursor = first.next_cursor.unwrap();

    let second = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            &format!(
                "/api/v1/inventory/journal?sort=net_quantity&direction=ascending&limit=1&cursor={cursor}"
            ),
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second: InventoryJournalPage = response(second).await;
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].id, second_transaction);
    assert!(second.next_cursor.is_none());

    let changed_filter = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            &format!(
                "/api/v1/inventory/journal?sort=net_quantity&direction=descending&limit=1&cursor={cursor}"
            ),
        ))
        .await
        .unwrap();
    assert_eq!(changed_filter.status(), StatusCode::BAD_REQUEST);
    let error: ErrorResponse = response(changed_filter).await;
    assert_eq!(error.reason, ErrorReason::InvalidCursor);

    let trace = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            "/api/v1/inventory/journal?query=TRACE-LOT-SECOND",
        ))
        .await
        .unwrap();
    assert_eq!(trace.status(), StatusCode::OK);
    let trace: InventoryJournalPage = response(trace).await;
    assert_eq!(trace.items.len(), 1);
    assert_eq!(trace.items[0].id, second_transaction);
    assert!(trace
        .items
        .iter()
        .all(|item| item.inventory_owner_id == allowed_owner));
}

#[tokio::test]
async fn integrity_issues_expose_scoped_operator_context_without_tenant_metadata() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("inventory-reconcile@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let allowed_owner = fixture.inventory_owner(tenant_id, "Reconcile Client").await;
    let denied_owner = fixture.inventory_owner(tenant_id, "Other Client").await;
    let allowed_facility = fixture.facility(tenant_id, "Reconcile Facility").await;
    let denied_facility = fixture.facility(tenant_id, "Other Facility").await;
    fixture
        .assign_owner_to_facility(tenant_id, allowed_owner, allowed_facility)
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, denied_owner, denied_facility)
        .await;
    let allowed_location = fixture
        .location(tenant_id, allowed_facility, "RECON-VISIBLE")
        .await;
    let denied_location = fixture
        .location(tenant_id, denied_facility, "RECON-HIDDEN")
        .await;
    let allowed_item = fixture.item(tenant_id, "Reconcile Item", "each").await;
    let denied_item = fixture.item(tenant_id, "Other Item", "each").await;
    let (_, allowed_balance) = receive(
        &fixture,
        tenant_id,
        operator.id,
        allowed_owner,
        allowed_location,
        allowed_item,
        "RECON-LOT",
        5,
        "recon-visible",
    )
    .await;
    let (_, denied_balance) = receive(
        &fixture,
        tenant_id,
        operator.id,
        denied_owner,
        denied_location,
        denied_item,
        "RECON-HIDDEN",
        7,
        "recon-hidden",
    )
    .await;
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

    let admin = admin_db_for(&fixture.db).await;
    sqlx::query(
        "ALTER TABLE inventory_balances DISABLE TRIGGER inventory_balances_capture_projection_change",
    )
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query("UPDATE inventory_balances SET qty_on_hand=qty_on_hand+1 WHERE id=ANY($1)")
        .bind(vec![allowed_balance, denied_balance])
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "ALTER TABLE inventory_balances ENABLE TRIGGER inventory_balances_capture_projection_change",
    )
    .execute(&admin)
    .await
    .unwrap();

    let token = wareboxes_api::auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let result = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            "/api/v1/inventory/integrity-issues?sort=severity&direction=descending",
        ))
        .await
        .unwrap();
    assert_eq!(result.status(), StatusCode::OK);
    let bytes = to_bytes(result.into_body(), 256 * 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let page: InventoryIntegrityPage = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(page.items.len(), 1);
    let issue = &page.items[0];
    assert_eq!(issue.kind, InventoryIntegrityIssueKind::JournalProjection);
    assert_eq!(issue.inventory_owner_id, allowed_owner);
    assert_eq!(issue.facility_id, allowed_facility);
    assert_eq!(issue.location_barcode.as_deref(), Some("RECON-VISIBLE"));
    assert_eq!(issue.lot.as_deref(), Some("RECON-LOT"));
    assert_eq!(issue.journal_quantity, Some(5));
    assert_eq!(issue.projected_quantity, Some(6));
    assert_eq!(issue.variance_quantity, Some(1));
    assert_eq!(issue.severity_quantity, 1);
    assert!(json["items"][0].get("tenant_id").is_none());

    sqlx::query(
        "ALTER TABLE inventory_balances DISABLE TRIGGER inventory_balances_capture_projection_change",
    )
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query("UPDATE inventory_balances SET qty_on_hand=qty_on_hand-1 WHERE id=ANY($1)")
        .bind(vec![allowed_balance, denied_balance])
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "ALTER TABLE inventory_balances ENABLE TRIGGER inventory_balances_capture_projection_change",
    )
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;

    let clean = app
        .oneshot(request(
            &token,
            tenant_id,
            "/api/v1/inventory/integrity-issues",
        ))
        .await
        .unwrap();
    assert_eq!(clean.status(), StatusCode::OK);
    let clean: InventoryIntegrityPage = response(clean).await;
    assert!(clean.items.is_empty());
}
