mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_application::CommandContext;
use wareboxes_core::dto::{
    CancelInventoryReservationResult, ReleaseInventoryHoldResult, UpdateUserAccessScope,
};
use wareboxes_core::models::{
    InventoryBalance, InventoryHold, InventoryHoldReason, InventoryHoldReconciliationIssue,
    InventoryReconciliationIssue, InventoryReservation, InventoryTransaction, ItemBatch,
};

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
            if let Some(idempotency_key) = body.get("idempotency_key").and_then(Value::as_str) {
                request = request.header(IDEMPOTENCY_KEY_HEADER, idempotency_key);
            }
            Body::from(serde_json::to_vec(&body).unwrap())
        }
        None => Body::empty(),
    };
    request.body(body).unwrap()
}

async fn send_api(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> axum::response::Response {
    app.clone()
        .oneshot(api_request(token, tenant_id, method, uri, body))
        .await
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn inventory_routes_enforce_owner_and_facility_scopes() {
    let db = setup().await;
    let fixture = Fixture { db: db.clone() };
    let administrator = auth::register_user(
        &db,
        "inventory-scope-admin@test.com",
        "supersecret",
        None,
        None,
    )
    .await
    .unwrap();
    let operator = auth::register_user(
        &db,
        "inventory-scope-operator@test.com",
        "supersecret",
        None,
        None,
    )
    .await
    .unwrap();
    let tenant_id = tenant_for_user(&db, administrator.id).await;
    let mut membership_tx = tenant_tx(&db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
        .bind(tenant_id.get())
        .bind(operator.id)
        .execute(&mut *membership_tx)
        .await
        .unwrap();
    membership_tx.commit().await.unwrap();
    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        &db,
        tenant_id,
        "wms",
        Some("WMS"),
    )
    .await
    .unwrap();
    let role = wareboxes_persistence_postgres::roles::add_role(
        &db,
        tenant_id,
        "inventory-scope-operator",
        Some("Scoped inventory operator"),
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_permission(&db, tenant_id, role, permission)
        .await
        .unwrap();
    wareboxes_persistence_postgres::roles::add_role_to_user(&db, tenant_id, operator.id, role)
        .await
        .unwrap();

    let allowed_facility = wareboxes_persistence_postgres::facilities::add_facility(
        &db,
        tenant_id,
        "Allowed Inventory DC",
    )
    .await
    .unwrap();
    let denied_facility = wareboxes_persistence_postgres::facilities::add_facility(
        &db,
        tenant_id,
        "Denied Inventory DC",
    )
    .await
    .unwrap();
    let allowed_source = wareboxes_persistence_postgres::locations::add_location(
        &db,
        tenant_id,
        allowed_facility,
        None,
        Some("INV-SCOPE-ALLOWED-SOURCE"),
        Some("Allowed Source"),
        "bin",
        true,
        true,
        false,
    )
    .await
    .unwrap();
    let allowed_destination = wareboxes_persistence_postgres::locations::add_location(
        &db,
        tenant_id,
        allowed_facility,
        None,
        Some("INV-SCOPE-ALLOWED-DESTINATION"),
        Some("Allowed Destination"),
        "bin",
        true,
        true,
        false,
    )
    .await
    .unwrap();
    let denied_location = wareboxes_persistence_postgres::locations::add_location(
        &db,
        tenant_id,
        denied_facility,
        None,
        Some("INV-SCOPE-DENIED"),
        Some("Denied Location"),
        "bin",
        true,
        true,
        false,
    )
    .await
    .unwrap();
    let allowed_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "Allowed Inventory Owner",
        "allowed-inventory-owner@test.com",
    )
    .await
    .unwrap();
    let denied_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "Denied Inventory Owner",
        "denied-inventory-owner@test.com",
    )
    .await
    .unwrap();
    assert!(repo::inventory_owners::replace_inventory_owner_facilities(
        &db,
        tenant_id,
        allowed_owner,
        &[allowed_facility],
    )
    .await
    .unwrap());
    assert!(repo::inventory_owners::replace_inventory_owner_facilities(
        &db,
        tenant_id,
        denied_owner,
        &[denied_facility],
    )
    .await
    .unwrap());
    let item = repo::items::add_item(
        &db,
        tenant_id,
        "Scoped Inventory Item",
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
    let allowed_batch = repo::inventory::add_item_batch(
        &db,
        tenant_id,
        allowed_owner,
        item,
        None,
        Some("INV-SCOPE-ALLOWED"),
        None,
        None,
    )
    .await
    .unwrap();
    let denied_batch = repo::inventory::add_item_batch(
        &db,
        tenant_id,
        denied_owner,
        item,
        None,
        Some("INV-SCOPE-DENIED"),
        None,
        None,
    )
    .await
    .unwrap();

    repo::inventory::receive_inventory(
        &db,
        tenant_id,
        administrator.id,
        allowed_batch,
        allowed_source,
        20,
        None,
        None,
        None,
        None,
        "inventory-scope-allowed-receipt",
    )
    .await
    .unwrap();
    repo::inventory::receive_inventory(
        &db,
        tenant_id,
        administrator.id,
        denied_batch,
        denied_location,
        15,
        None,
        None,
        None,
        None,
        "inventory-scope-denied-receipt",
    )
    .await
    .unwrap();
    let balances = repo::inventory::get_balances(&db, tenant_id, false)
        .await
        .unwrap();
    let allowed_balance = balances
        .iter()
        .find(|balance| balance.item_batch_id == allowed_batch)
        .unwrap();
    let denied_balance = balances
        .iter()
        .find(|balance| balance.item_batch_id == denied_batch)
        .unwrap();
    let allowed_balance_id = allowed_balance.id;
    let denied_balance_id = denied_balance.id;
    repo::orders::add_order(
        &db,
        tenant_id,
        &new_order("INV-SCOPE-ALLOWED-ORDER", allowed_owner),
    )
    .await
    .unwrap();
    repo::orders::add_order(
        &db,
        tenant_id,
        &new_order("INV-SCOPE-DENIED-ORDER", denied_owner),
    )
    .await
    .unwrap();
    let orders = repo::orders::get_orders(&db, tenant_id).await.unwrap();
    let allowed_order = orders
        .iter()
        .find(|order| order.order_key == "INV-SCOPE-ALLOWED-ORDER")
        .unwrap()
        .id;
    let denied_order = orders
        .iter()
        .find(|order| order.order_key == "INV-SCOPE-DENIED-ORDER")
        .unwrap()
        .id;
    let allowed_order_item = fixture.order_item(tenant_id, allowed_order, item, 3).await;
    let denied_order_item = fixture.order_item(tenant_id, denied_order, item, 3).await;
    let administrator_access = default_tenant_for_user(&db, administrator.id)
        .await
        .unwrap();
    let allowed_reservation = repo::inventory::create_inventory_reservation(
        &db,
        &administrator_access,
        &repo::inventory::CreateInventoryReservationCommand {
            order_id: allowed_order,
            order_item_id: allowed_order_item,
            facility_id: allowed_facility,
            qty: 3,
            idempotency_key: "inventory-scope-allowed-reservation-setup",
        },
    )
    .await
    .unwrap()
    .reservation_id;
    repo::inventory::allocate_inventory(
        &db,
        &administrator_access,
        &repo::inventory::AllocateInventoryCommand {
            reservation_id: allowed_reservation,
            inventory_balance_id: allowed_balance_id,
            qty: 3,
            idempotency_key: "inventory-scope-allowed-allocation-setup",
        },
    )
    .await
    .unwrap();
    let allowed_hold = repo::inventory::place_inventory_hold(
        &db,
        &administrator_access,
        &CommandContext {
            tenant_id,
            actor_id: administrator_access.user_id,
            request_id: "request-inventory-scope-allowed-hold-setup".into(),
            idempotency_key: Some("inventory-scope-allowed-hold-setup".into()),
        },
        &repo::inventory::PlaceInventoryHoldCommand {
            inventory_balance_id: allowed_balance_id,
            qty: 2,
            reason: InventoryHoldReason::QualityInspection,
            note: None,
            reference_type: None,
            reference_id: None,
        },
    )
    .await
    .unwrap();
    let denied_hold = repo::inventory::place_inventory_hold(
        &db,
        &administrator_access,
        &CommandContext {
            tenant_id,
            actor_id: administrator_access.user_id,
            request_id: "request-inventory-scope-denied-hold-setup".into(),
            idempotency_key: Some("inventory-scope-denied-hold-setup".into()),
        },
        &repo::inventory::PlaceInventoryHoldCommand {
            inventory_balance_id: denied_balance_id,
            qty: 2,
            reason: InventoryHoldReason::Regulatory,
            note: None,
            reference_type: None,
            reference_id: None,
        },
    )
    .await
    .unwrap();
    let denied_reservation = repo::inventory::create_inventory_reservation(
        &db,
        &administrator_access,
        &repo::inventory::CreateInventoryReservationCommand {
            order_id: denied_order,
            order_item_id: denied_order_item,
            facility_id: denied_facility,
            qty: 3,
            idempotency_key: "inventory-scope-denied-reservation-setup",
        },
    )
    .await
    .unwrap()
    .reservation_id;
    repo::inventory::allocate_inventory(
        &db,
        &administrator_access,
        &repo::inventory::AllocateInventoryCommand {
            reservation_id: denied_reservation,
            inventory_balance_id: denied_balance_id,
            qty: 3,
            idempotency_key: "inventory-scope-denied-allocation-setup",
        },
    )
    .await
    .unwrap();

    let transactions_before = repo::inventory::get_transactions(&db, tenant_id)
        .await
        .unwrap();
    let quantity_before = repo::inventory::get_balances(&db, tenant_id, false)
        .await
        .unwrap()
        .iter()
        .map(|balance| balance.qty_on_hand)
        .sum::<i64>();
    let cross_facility = repo::inventory::move_inventory(
        &db,
        tenant_id,
        administrator.id,
        allowed_batch,
        allowed_source,
        denied_location,
        1,
        None,
        None,
        None,
        None,
        "inventory-scope-cross-facility",
    )
    .await
    .unwrap_err();
    assert!(matches!(
        cross_facility,
        AppError::Core(CoreError::Conflict(_))
    ));
    assert_eq!(
        repo::inventory::get_transactions(&db, tenant_id)
            .await
            .unwrap()
            .len(),
        transactions_before.len()
    );
    assert_eq!(
        repo::inventory::get_balances(&db, tenant_id, false)
            .await
            .unwrap()
            .iter()
            .map(|balance| balance.qty_on_hand)
            .sum::<i64>(),
        quantity_before
    );

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

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::GET,
        "/api/inventory/batches",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let batches = response_json::<Vec<ItemBatch>>(response).await;
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].id, allowed_batch);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::GET,
        "/api/inventory/balances",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let scoped_balances = response_json::<Vec<InventoryBalance>>(response).await;
    assert_eq!(scoped_balances.len(), 1);
    assert_eq!(scoped_balances[0].id, allowed_balance_id);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::GET,
        "/api/inventory/transactions",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let transactions = response_json::<Vec<InventoryTransaction>>(response).await;
    assert_eq!(transactions.len(), 1);
    assert_eq!(transactions[0].inventory_owner_id.get(), allowed_owner);
    assert!(transactions[0]
        .entries
        .iter()
        .all(|entry| entry.facility_id == allowed_facility));

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::GET,
        "/api/inventory/reservations",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let reservations = response_json::<Vec<InventoryReservation>>(response).await;
    assert_eq!(reservations.len(), 1);
    assert_eq!(reservations[0].id, allowed_reservation);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::GET,
        "/api/inventory/holds",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let holds = response_json::<Vec<InventoryHold>>(response).await;
    assert_eq!(holds.len(), 1);
    assert_eq!(holds[0].id, allowed_hold.hold_id);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::GET,
        "/api/inventory/holds/reconciliation",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response_json::<Vec<InventoryHoldReconciliationIssue>>(response)
            .await
            .is_empty()
    );

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/inventory/batches/add",
        Some(json!({
            "inventory_owner_id": denied_owner,
            "item_id": item,
            "lot": "HIDDEN-OWNER-LOT"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/inventory/holds/place",
        Some(json!({
            "inventory_balance_id": denied_balance_id,
            "qty": 1,
            "reason": "customer_request",
            "note": null,
            "reference_type": null,
            "reference_id": null,
            "idempotency_key": "inventory-scope-hidden-hold-place"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/inventory/holds/release",
        Some(json!({
            "hold_id": denied_hold.hold_id,
            "idempotency_key": "inventory-scope-hidden-hold-release"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/inventory/holds/release",
        Some(json!({
            "hold_id": allowed_hold.hold_id,
            "idempotency_key": "inventory-scope-allowed-hold-release"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json::<ReleaseInventoryHoldResult>(response)
            .await
            .released_qty,
        2
    );

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/inventory/batches/delete",
        Some(json!({"item_batch_id": denied_batch})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response_json::<bool>(response).await);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/inventory/moves",
        Some(json!({
            "item_batch_id": denied_batch,
            "from_location_id": denied_location,
            "to_location_id": allowed_destination,
            "qty": 1,
            "idempotency_key": "inventory-scope-hidden-source-move"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/inventory/moves",
        Some(json!({
            "item_batch_id": allowed_batch,
            "from_location_id": allowed_source,
            "to_location_id": denied_location,
            "qty": 1,
            "idempotency_key": "inventory-scope-hidden-destination-move"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/inventory/moves/split",
        Some(json!({
            "from_inventory_balance_id": denied_balance_id,
            "destinations": [{"to_location_id": allowed_destination, "qty": 1}],
            "idempotency_key": "inventory-scope-hidden-split"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/inventory/reservations/create",
        Some(json!({
            "order_id": denied_order,
            "order_item_id": denied_order_item,
            "facility_id": denied_facility,
            "qty": 1,
            "idempotency_key": "inventory-scope-hidden-order-reservation"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/inventory/allocations/create",
        Some(json!({
            "reservation_id": allowed_reservation,
            "inventory_balance_id": denied_balance_id,
            "qty": 1,
            "idempotency_key": "inventory-scope-hidden-balance-reservation"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/inventory/reservations/cancel",
        Some(json!({
            "reservation_id": denied_reservation,
            "idempotency_key": "inventory-scope-hidden-reservation-cancel"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/inventory/moves",
        Some(json!({
            "item_batch_id": allowed_batch,
            "from_location_id": allowed_source,
            "to_location_id": allowed_destination,
            "qty": 2,
            "idempotency_key": "inventory-scope-allowed-move"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response_json::<i64>(response).await > 0);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/inventory/reservations/cancel",
        Some(json!({
            "reservation_id": allowed_reservation,
            "idempotency_key": "inventory-scope-allowed-reservation-cancel"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json::<CancelInventoryReservationResult>(response)
            .await
            .released_qty,
        3
    );

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/inventory/reservations/cancel",
        Some(json!({
            "reservation_id": allowed_reservation,
            "idempotency_key": "inventory-scope-allowed-reservation-cancel"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json::<CancelInventoryReservationResult>(response)
            .await
            .released_qty,
        3
    );

    let balances = repo::inventory::get_balances(&db, tenant_id, false)
        .await
        .unwrap();
    assert_eq!(
        balances
            .iter()
            .filter(|balance| balance.item_batch_id == allowed_batch)
            .map(|balance| balance.qty_on_hand)
            .sum::<i64>(),
        20
    );
    let denied_balance = balances
        .iter()
        .find(|balance| balance.id == denied_balance_id)
        .unwrap();
    assert_eq!(denied_balance.qty_on_hand, 15);
    assert_eq!(denied_balance.qty_reserved, 3);
    let denied_reservation_record = repo::inventory::get_reservations(&db, tenant_id, false)
        .await
        .unwrap()
        .into_iter()
        .find(|reservation| reservation.id == denied_reservation)
        .unwrap();
    assert!(denied_reservation_record.deleted.is_none());

    let admin_db = admin_db_for(&db).await;
    sqlx::query(
        "ALTER TABLE inventory_balances DISABLE TRIGGER inventory_balances_capture_projection_change",
    )
    .execute(&admin_db)
    .await
    .unwrap();
    sqlx::query("UPDATE inventory_balances SET qty_on_hand = qty_on_hand + 1 WHERE id = ANY($1)")
        .bind(vec![allowed_balance_id, denied_balance_id])
        .execute(&admin_db)
        .await
        .unwrap();
    sqlx::query(
        "ALTER TABLE inventory_balances ENABLE TRIGGER inventory_balances_capture_projection_change",
    )
    .execute(&admin_db)
    .await
    .unwrap();
    let unscoped_issues = repo::inventory::get_reconciliation_issues(&db, tenant_id)
        .await
        .unwrap();
    assert_eq!(unscoped_issues.len(), 2);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::GET,
        "/api/inventory/reconciliation",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let issues = response_json::<Vec<InventoryReconciliationIssue>>(response).await;
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].facility_id, allowed_facility);
    assert_eq!(issues[0].inventory_owner_id.get(), allowed_owner);

    sqlx::query(
        "ALTER TABLE inventory_balances DISABLE TRIGGER inventory_balances_capture_projection_change",
    )
    .execute(&admin_db)
    .await
    .unwrap();
    sqlx::query("UPDATE inventory_balances SET qty_on_hand = qty_on_hand - 1 WHERE id = ANY($1)")
        .bind(vec![allowed_balance_id, denied_balance_id])
        .execute(&admin_db)
        .await
        .unwrap();
    sqlx::query(
        "ALTER TABLE inventory_balances ENABLE TRIGGER inventory_balances_capture_projection_change",
    )
    .execute(&admin_db)
    .await
    .unwrap();
    admin_db.close().await;
    assert!(repo::inventory::get_reconciliation_issues(&db, tenant_id)
        .await
        .unwrap()
        .is_empty());
}
