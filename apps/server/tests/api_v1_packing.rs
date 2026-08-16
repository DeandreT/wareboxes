mod common;
mod api_v1_packing {
    mod decision_policy;
    mod removal;
    mod reopening;
    mod scale;
}

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use sqlx::Row;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::{IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    CloseCartonResponse, CreateCartonResponse, ErrorReason, ErrorResponse, OpenPackSessionResponse,
    PackCartonLifecycleResponse, PackDecisionPolicySource, PackPickedAllocationResponse,
    PackSessionResponse, PackSessionStatus, PackingOrderStatus, PackingQueuePage,
    PickClaimResponse, PickContentConfirmationResponse, PickOrderStatus, VoidCartonResponse,
    PRODUCT_DEFAULT_PACK_DECISION_POLICY_HASH,
};
use wareboxes_core::dto::UpdateUserAccessScope;

fn request(
    token: &str,
    tenant_id: TenantId,
    method: Method,
    path: &str,
    idempotency_key: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    if let Some(key) = idempotency_key {
        request = request
            .header(IDEMPOTENCY_KEY_HEADER, key)
            .header(REQUEST_ID_HEADER, format!("request-{key}"));
    }
    let body = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    request.body(body).unwrap()
}

async fn send(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    method: Method,
    path: &str,
    idempotency_key: Option<&str>,
    body: Option<Value>,
) -> axum::response::Response {
    app.clone()
        .oneshot(request(
            token,
            tenant_id,
            method,
            path,
            idempotency_key,
            body,
        ))
        .await
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn expect_status(
    response: axum::response::Response,
    expected: StatusCode,
    operation: &str,
) -> axum::response::Response {
    if response.status() != expected {
        let actual = response.status();
        let body = response_json::<Value>(response).await;
        panic!("{operation}: expected {expected}, got {actual}: {body}");
    }
    response
}

async fn grant_orders(db: &db::Db, tenant_id: TenantId, user_id: i64, role_name: &str) {
    let permission =
        match wareboxes_persistence_postgres::permissions::find_by_name(db, tenant_id, "orders")
            .await
            .unwrap()
        {
            Some(permission) => permission.id,
            None => wareboxes_persistence_postgres::permissions::add_permission(
                db,
                tenant_id,
                "orders",
                Some("Fulfillment orders"),
            )
            .await
            .unwrap(),
        };
    let role = wareboxes_persistence_postgres::roles::add_role(
        db,
        tenant_id,
        role_name,
        Some("Fulfillment execution"),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        db, tenant_id, role, permission
    )
    .await
    .unwrap());
    assert!(
        wareboxes_persistence_postgres::roles::add_role_to_user(db, tenant_id, user_id, role)
            .await
            .unwrap()
    );
}

async fn add_wms_operator(
    fixture: &Fixture,
    tenant_id: TenantId,
    email: &str,
    role_name: &str,
) -> wareboxes_core::models::User {
    let user = fixture.user(email).await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
        .bind(tenant_id.get())
        .bind(user.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let permission =
        wareboxes_persistence_postgres::permissions::find_by_name(&fixture.db, tenant_id, "wms")
            .await
            .unwrap()
            .expect("tenant has a WMS permission");
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        role_name,
        Some("Packing operator"),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        permission.id,
    )
    .await
    .unwrap());
    assert!(wareboxes_persistence_postgres::roles::add_role_to_user(
        &fixture.db,
        tenant_id,
        user.id,
        role,
    )
    .await
    .unwrap());
    user
}

async fn set_scope(
    db: &db::Db,
    tenant_id: TenantId,
    user_id: i64,
    facility_ids: Vec<i64>,
    inventory_owner_ids: Vec<i64>,
) {
    assert!(repo::tenants::update_user_access_scope(
        db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id,
            all_facilities: false,
            facility_ids,
            all_inventory_owners: false,
            inventory_owner_ids,
        },
    )
    .await
    .unwrap());
}

async fn execution_location(
    fixture: &Fixture,
    tenant_id: TenantId,
    facility_id: i64,
    barcode: &str,
    location_type: &str,
) -> i64 {
    wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some(barcode),
        Some(barcode),
        location_type,
        true,
        false,
        false,
    )
    .await
    .unwrap()
}

async fn plate_at(
    fixture: &Fixture,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
    location_id: i64,
    barcode: &str,
) -> i64 {
    let plate_id = repo::license_plates::add_license_plate(
        &fixture.db,
        tenant_id,
        inventory_owner_id,
        facility_id,
        Some(barcode),
    )
    .await
    .unwrap();
    let admin = admin_db_for(&fixture.db).await;
    sqlx::query("UPDATE license_plates SET location_id = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(location_id)
        .bind(tenant_id.get())
        .bind(plate_id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    plate_id
}

#[derive(Debug)]
struct PreparedOrder {
    order_id: i64,
}

#[allow(clippy::too_many_arguments)]
async fn prepare_order(
    fixture: &Fixture,
    app: &axum::Router,
    token: &str,
    access: &wareboxes_core::models::TenantAccess,
    inventory_owner_id: i64,
    facility_id: i64,
    key: &str,
    quantities: &[i64],
) -> PreparedOrder {
    let order_id = fixture
        .order_header(access.tenant_id, key, inventory_owner_id)
        .await;
    for (index, &quantity) in quantities.iter().enumerate() {
        let item_id = fixture
            .item(access.tenant_id, &format!("{key} item {index}"), "each")
            .await;
        repo::items::add_barcode(
            &fixture.db,
            access.tenant_id,
            item_id,
            &format!("{key}-ITEM-{index}"),
            "code128",
            None,
        )
        .await
        .unwrap();
        fixture
            .order_item(access.tenant_id, order_id, item_id, quantity)
            .await;
        fixture
            .received_balance(
                access,
                ReceivedBalanceSetup {
                    inventory_owner_id,
                    facility_id,
                    item_id,
                    qty: quantity + 2,
                    key: &format!("{key}-SOURCE-{index}"),
                },
            )
            .await;
    }
    let allocated = send(
        app,
        token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{order_id}/allocation-runs"),
        Some(&format!("{key}-allocate")),
        Some(json!({
            "facility_id": facility_id,
            "expected_revision": 1,
                "expected_policy": {"source": "product_default", "policy_hash": "6090a99a06ea2e049d7321d5cf2b8f462c6d6e6e2ca527ae87657a7a5fd9d156"}
        })),
    )
    .await;
    expect_status(allocated, StatusCode::OK, "allocate packing order").await;
    PreparedOrder { order_id }
}

async fn release_order(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    order_id: i64,
    facility_id: i64,
    destination_location_id: i64,
    key: &str,
) {
    let response = send(
        app,
        token,
        tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{order_id}/releases"),
        Some(key),
        Some(json!({
            "facility_id": facility_id,
            "destination_location_id": destination_location_id,
            "expected_revision": 2
        })),
    )
    .await;
    expect_status(response, StatusCode::OK, "release packing order").await;
}

async fn pick_order(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    tote_barcode: &str,
    content_count: usize,
    key: &str,
) -> Vec<PickContentConfirmationResponse> {
    let mut confirmations = Vec::with_capacity(content_count);
    for index in 0..content_count {
        let claim = send(
            app,
            token,
            tenant_id,
            Method::POST,
            "/api/v1/picking-claims/next",
            Some(&format!("{key}-claim-{index}")),
            Some(json!({})),
        )
        .await;
        let claim = expect_status(claim, StatusCode::OK, "claim packing pick").await;
        let claim = response_json::<Option<PickClaimResponse>>(claim)
            .await
            .expect("released order has pick work");
        let confirmation = send(
            app,
            token,
            tenant_id,
            Method::POST,
            &format!(
                "/api/v1/picking-tasks/{}/contents/{}/confirmations",
                claim.task_id, claim.content.content_id
            ),
            Some(&format!("{key}-confirm-{index}")),
            Some(json!({
                "source_location_barcode": claim.content.source_location_barcode,
                "item_barcode": claim.content.item_barcodes[0],
                "destination_license_plate_barcode": tote_barcode
            })),
        )
        .await;
        let confirmation =
            expect_status(confirmation, StatusCode::OK, "confirm packing pick").await;
        confirmations.push(response_json(confirmation).await);
    }
    confirmations
}

async fn open_session(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    order_id: i64,
    facility_id: i64,
    packing_location_id: i64,
    key: &str,
) -> OpenPackSessionResponse {
    let response = send(
        app,
        token,
        tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{order_id}/packing-sessions"),
        Some(key),
        Some(json!({
            "facility_id": facility_id,
            "station_location_id": packing_location_id,
            "expected_revision": 4
        })),
    )
    .await;
    let response = expect_status(response, StatusCode::OK, "open packing session").await;
    response_json(response).await
}

async fn create_carton(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    session_id: i64,
    barcode: &str,
    revision: i64,
    key: &str,
) -> CreateCartonResponse {
    let response = send(
        app,
        token,
        tenant_id,
        Method::POST,
        &format!("/api/v1/packing-sessions/{session_id}/cartons"),
        Some(key),
        Some(json!({
            "carton_barcode": barcode,
            "expected_revision": revision
        })),
    )
    .await;
    let response = expect_status(response, StatusCode::OK, "create packing carton").await;
    response_json(response).await
}

fn pack_body(
    allocation_id: i64,
    item: &str,
    lot: &str,
    tote: &str,
    carton: &str,
    revision: i64,
) -> Value {
    json!({
        "inventory_allocation_id": allocation_id,
        "item_barcode": item,
        "lot_scan": lot,
        "source_license_plate_barcode": tote,
        "carton_barcode": carton,
        "expected_revision": revision
    })
}

fn controlled_pack_body(
    allocation_id: i64,
    item: &str,
    lot_scan: Option<&str>,
    serial_scan: Option<&str>,
    tote: &str,
    carton: &str,
    revision: i64,
) -> Value {
    let mut body = json!({
        "inventory_allocation_id": allocation_id,
        "item_barcode": item,
        "source_license_plate_barcode": tote,
        "carton_barcode": carton,
        "expected_revision": revision
    });
    if let Some(lot_scan) = lot_scan {
        body["lot_scan"] = json!(lot_scan);
    }
    if let Some(serial_scan) = serial_scan {
        body["serial_scan"] = json!(serial_scan);
    }
    body
}

#[allow(clippy::too_many_arguments)]
async fn close_carton(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    session_id: i64,
    carton_id: i64,
    barcode: &str,
    revision: i64,
    key: &str,
) -> CloseCartonResponse {
    let response = send(
        app,
        token,
        tenant_id,
        Method::POST,
        &format!("/api/v1/packing-sessions/{session_id}/cartons/{carton_id}/closures"),
        Some(key),
        Some(json!({
            "carton_barcode": barcode,
            "measurements": {
                "weight_grams": 1250,
                "dimensions": {"length_mm": 300, "width_mm": 200, "height_mm": 150}
            },
            "expected_revision": revision
        })),
    )
    .await;
    let response = expect_status(response, StatusCode::OK, "close packing carton").await;
    response_json(response).await
}

#[tokio::test]
async fn packing_is_exact_revisioned_replay_safe_and_conserves_reserved_inventory() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("packing-flow@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "packing-flow-orders",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Packing Flow Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Packing Flow Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let packing_id = execution_location(
        &fixture,
        access.tenant_id,
        facility_id,
        "PACK-FLOW-STATION",
        "packing",
    )
    .await;
    let tote_id = plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        packing_id,
        "PACK-FLOW-TOTE",
    )
    .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let order = prepare_order(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        "PACK-FLOW",
        &[3, 2],
    )
    .await;
    release_order(
        &app,
        &token,
        access.tenant_id,
        order.order_id,
        facility_id,
        packing_id,
        "pack-flow-release",
    )
    .await;
    let picks = pick_order(
        &app,
        &token,
        access.tenant_id,
        "PACK-FLOW-TOTE",
        2,
        "pack-flow",
    )
    .await;
    assert_eq!(picks[0].order_status, PickOrderStatus::Processing);
    assert_eq!(picks[0].order_revision.get(), 3);
    assert_eq!(picks[1].order_status, PickOrderStatus::AwaitingPacking);
    assert_eq!(picks[1].order_revision.get(), 4);
    assert!(picks[1].order_ready_to_pack);

    let second = prepare_order(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        "PACK-TOTE-REUSE",
        &[1],
    )
    .await;
    release_order(
        &app,
        &token,
        access.tenant_id,
        second.order_id,
        facility_id,
        packing_id,
        "pack-tote-reuse-release",
    )
    .await;
    let second_claim = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/picking-claims/next",
        Some("pack-tote-reuse-claim"),
        Some(json!({})),
    )
    .await;
    let second_claim = response_json::<Option<PickClaimResponse>>(
        expect_status(second_claim, StatusCode::OK, "claim second order").await,
    )
    .await
    .unwrap();
    let reused_tote = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/picking-tasks/{}/contents/{}/confirmations",
            second_claim.task_id, second_claim.content.content_id
        ),
        Some("pack-tote-reuse-confirm"),
        Some(json!({
            "source_location_barcode": second_claim.content.source_location_barcode,
            "item_barcode": second_claim.content.item_barcodes[0],
            "destination_license_plate_barcode": "PACK-FLOW-TOTE"
        })),
    )
    .await;
    assert_eq!(reused_tote.status(), StatusCode::CONFLICT);
    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let tote_bindings: (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), MIN(order_id) FROM outbound_order_containers WHERE tenant_id = $1 AND license_plate_id = $2",
    )
    .bind(access.tenant_id.get())
    .bind(tote_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(tote_bindings, (1, order.order_id));

    let opened = open_session(
        &app,
        &token,
        access.tenant_id,
        order.order_id,
        facility_id,
        packing_id,
        "pack-flow-open",
    )
    .await;
    assert_eq!(opened.session.revision.get(), 5);
    assert_eq!(opened.session.progress.status, PackSessionStatus::Open);
    assert_eq!(opened.session.progress.expected_allocation_count, 2);
    assert_eq!(opened.session.progress.expected_quantity, 5);
    assert_eq!(
        opened.session.pack_policy.source,
        PackDecisionPolicySource::ProductDefault
    );
    assert_eq!(
        opened.session.pack_policy.policy_hash,
        PRODUCT_DEFAULT_PACK_DECISION_POLICY_HASH
    );
    assert!(!opened.session.station_scan_verified);
    assert_eq!(opened.session.allocations.len(), 2);
    assert!(opened
        .session
        .allocations
        .iter()
        .all(|allocation| allocation.license_plate_id == tote_id));

    let open_replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{}/packing-sessions", order.order_id),
        Some("pack-flow-open"),
        Some(json!({
            "facility_id": facility_id,
            "station_location_id": packing_id,
            "expected_revision": 4
        })),
    )
    .await;
    assert_eq!(open_replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<OpenPackSessionResponse>(open_replay).await,
        opened
    );

    let session_id = opened.session.session_id;
    let carton_one = create_carton(
        &app,
        &token,
        access.tenant_id,
        session_id,
        "PACK-FLOW-CARTON-1",
        5,
        "pack-flow-carton-1",
    )
    .await;
    assert_eq!(carton_one.revision.get(), 6);
    let first_allocation = &opened.session.allocations[0];
    let first_pack_path = format!(
        "/api/v1/packing-sessions/{session_id}/cartons/{}/contents",
        carton_one.carton.carton_id
    );
    for (key, body) in [
        (
            "pack-flow-bad-item",
            pack_body(
                first_allocation.inventory_allocation_id,
                "NOT-THE-ITEM",
                first_allocation.lot.as_deref().unwrap(),
                "PACK-FLOW-TOTE",
                "PACK-FLOW-CARTON-1",
                6,
            ),
        ),
        (
            "pack-flow-bad-tote",
            pack_body(
                first_allocation.inventory_allocation_id,
                &first_allocation.item_barcodes[0],
                first_allocation.lot.as_deref().unwrap(),
                "NOT-THE-TOTE",
                "PACK-FLOW-CARTON-1",
                6,
            ),
        ),
        (
            "pack-flow-bad-carton",
            pack_body(
                first_allocation.inventory_allocation_id,
                &first_allocation.item_barcodes[0],
                first_allocation.lot.as_deref().unwrap(),
                "PACK-FLOW-TOTE",
                "NOT-THE-CARTON",
                6,
            ),
        ),
    ] {
        let rejected = send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &first_pack_path,
            Some(key),
            Some(body),
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST, "{key}");
    }
    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let rejected_effects: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM carton_contents WHERE tenant_id = $1 AND packing_session_id = $2),
               (SELECT COUNT(*) FROM inventory_transactions WHERE tenant_id = $1 AND operation = 'packing.content.confirm.v1'),
               (SELECT revision FROM packing_sessions WHERE tenant_id = $1 AND id = $2),
               (SELECT revision FROM orders WHERE tenant_id = $1 AND id = $3)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(session_id)
    .bind(order.order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(rejected_effects, (0, 0, 6, 6));

    let first_pack_body = pack_body(
        first_allocation.inventory_allocation_id,
        &first_allocation.item_barcodes[0],
        first_allocation.lot.as_deref().unwrap(),
        "PACK-FLOW-TOTE",
        "PACK-FLOW-CARTON-1",
        6,
    );
    let packed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &first_pack_path,
        Some("pack-flow-pack-1"),
        Some(first_pack_body),
    )
    .await;
    let packed: PackPickedAllocationResponse =
        response_json(expect_status(packed, StatusCode::OK, "pack first allocation").await).await;
    assert_eq!(packed.revision.get(), 7);
    assert_eq!(packed.quantity, first_allocation.quantity);
    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let moved = sqlx::query(
        r#"
        SELECT source.status AS source_status, source.deleted AS source_deleted,
               destination.status AS destination_status,
               destination.deleted AS destination_deleted,
               source.reservation_id AS source_reservation_id,
               destination.reservation_id AS destination_reservation_id,
               source_balance.qty_reserved AS source_reserved,
               destination_balance.qty_reserved AS destination_reserved,
               (SELECT COALESCE(SUM(entry.quantity_delta), 0)::BIGINT
                  FROM inventory_entries entry
                 WHERE entry.tenant_id = $1 AND entry.transaction_id = $2) AS journal_net,
               (SELECT COUNT(*) FROM inventory_entries entry
                 WHERE entry.tenant_id = $1 AND entry.transaction_id = $2) AS journal_entries
        FROM inventory_allocations source
        INNER JOIN inventory_allocations destination
          ON destination.tenant_id = source.tenant_id AND destination.id = $3
        INNER JOIN inventory_balances source_balance
          ON source_balance.tenant_id = source.tenant_id AND source_balance.id = $4
        INNER JOIN inventory_balances destination_balance
          ON destination_balance.tenant_id = source.tenant_id AND destination_balance.id = $5
        WHERE source.tenant_id = $1 AND source.id = $6
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(packed.inventory_transaction_id)
    .bind(packed.destination_inventory_allocation_id)
    .bind(packed.source_inventory_balance_id)
    .bind(packed.destination_inventory_balance_id)
    .bind(packed.source_inventory_allocation_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        moved.try_get::<String, _>("source_status").unwrap(),
        "fulfilled"
    );
    assert!(moved
        .try_get::<Option<wareboxes_domain::Timestamp>, _>("source_deleted")
        .unwrap()
        .is_some());
    assert_eq!(
        moved.try_get::<String, _>("destination_status").unwrap(),
        "allocated"
    );
    assert!(moved
        .try_get::<Option<wareboxes_domain::Timestamp>, _>("destination_deleted")
        .unwrap()
        .is_none());
    assert_eq!(
        moved.try_get::<i64, _>("source_reservation_id").unwrap(),
        moved
            .try_get::<i64, _>("destination_reservation_id")
            .unwrap()
    );
    assert_eq!(moved.try_get::<i64, _>("source_reserved").unwrap(), 0);
    assert_eq!(
        moved.try_get::<i64, _>("destination_reserved").unwrap(),
        packed.quantity
    );
    assert_eq!(moved.try_get::<i64, _>("journal_net").unwrap(), 0);
    assert_eq!(moved.try_get::<i64, _>("journal_entries").unwrap(), 2);

    let first_closed = close_carton(
        &app,
        &token,
        access.tenant_id,
        session_id,
        carton_one.carton.carton_id,
        "PACK-FLOW-CARTON-1",
        7,
        "pack-flow-close-1",
    )
    .await;
    assert_eq!(first_closed.order_status, PackingOrderStatus::Packing);
    assert!(!first_closed.ready_to_manifest);
    assert_eq!(first_closed.revision.get(), 8);
    let carton_two = create_carton(
        &app,
        &token,
        access.tenant_id,
        session_id,
        "PACK-FLOW-CARTON-2",
        8,
        "pack-flow-carton-2",
    )
    .await;
    assert_eq!(carton_two.revision.get(), 9);
    let second_allocation = &opened.session.allocations[1];
    let second_pack_path = format!(
        "/api/v1/packing-sessions/{session_id}/cartons/{}/contents",
        carton_two.carton.carton_id
    );
    let second_pack_body = pack_body(
        second_allocation.inventory_allocation_id,
        &second_allocation.item_barcodes[0],
        second_allocation.lot.as_deref().unwrap(),
        "PACK-FLOW-TOTE",
        "PACK-FLOW-CARTON-2",
        9,
    );
    let stale = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &second_pack_path,
        Some("pack-flow-stale"),
        Some(pack_body(
            second_allocation.inventory_allocation_id,
            &second_allocation.item_barcodes[0],
            second_allocation.lot.as_deref().unwrap(),
            "PACK-FLOW-TOTE",
            "PACK-FLOW-CARTON-2",
            8,
        )),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let first = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &second_pack_path,
        Some("pack-flow-race-a"),
        Some(second_pack_body.clone()),
    );
    let second = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &second_pack_path,
        Some("pack-flow-race-b"),
        Some(second_pack_body.clone()),
    );
    let (first, second) = tokio::join!(first, second);
    let (success, conflict, replay_key) = match (first.status(), second.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => (first, second, "pack-flow-race-a"),
        (StatusCode::CONFLICT, StatusCode::OK) => (second, first, "pack-flow-race-b"),
        statuses => panic!("expected one pack winner and one conflict, got {statuses:?}"),
    };
    drop(conflict);
    let packed_two: PackPickedAllocationResponse = response_json(success).await;
    assert_eq!(packed_two.revision.get(), 10);
    let replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &second_pack_path,
        Some(replay_key),
        Some(second_pack_body.clone()),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<PackPickedAllocationResponse>(replay).await,
        packed_two
    );
    let changed_payload = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &second_pack_path,
        Some(replay_key),
        Some(pack_body(
            second_allocation.inventory_allocation_id,
            "CHANGED-ITEM",
            second_allocation.lot.as_deref().unwrap(),
            "PACK-FLOW-TOTE",
            "PACK-FLOW-CARTON-2",
            9,
        )),
    )
    .await;
    assert_eq!(changed_payload.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed_payload).await.reason,
        ErrorReason::IdempotencyKeyReused
    );
    let changed_key = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &second_pack_path,
        Some("pack-flow-after-success"),
        Some(second_pack_body),
    )
    .await;
    assert_eq!(changed_key.status(), StatusCode::CONFLICT);

    let final_close = close_carton(
        &app,
        &token,
        access.tenant_id,
        session_id,
        carton_two.carton.carton_id,
        "PACK-FLOW-CARTON-2",
        10,
        "pack-flow-close-2",
    )
    .await;
    assert_eq!(
        final_close.order_status,
        PackingOrderStatus::AwaitingShipment
    );
    assert!(final_close.ready_to_manifest);
    assert_eq!(final_close.pack_policy, opened.session.pack_policy);
    assert_eq!(final_close.revision.get(), 11);
    assert_eq!(
        final_close.progress.status,
        PackSessionStatus::ReadyToManifest
    );
    let current = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &format!("/api/v1/packing-sessions/{session_id}"),
        None,
        None,
    )
    .await;
    let current: PackSessionResponse =
        response_json(expect_status(current, StatusCode::OK, "read final packing session").await)
            .await;
    assert_eq!(current.revision.get(), 11);
    assert_eq!(current.progress.packed_allocation_count, 2);
    assert_eq!(current.progress.packed_quantity, 5);
    assert_eq!(current.progress.closed_carton_count, 2);
    assert_eq!(current.progress.status, PackSessionStatus::ReadyToManifest);

    let admin = admin_db_for(&fixture.db).await;
    for (operation, statement) in [
        (
            "outbound container update",
            "UPDATE outbound_order_containers SET order_id = order_id WHERE tenant_id = $1 AND order_id = $2",
        ),
        (
            "packing allocation update",
            "UPDATE packing_session_allocations SET planned_qty = planned_qty WHERE tenant_id = $1 AND order_id = $2",
        ),
        (
            "carton content update",
            "UPDATE carton_contents SET packed_qty = packed_qty WHERE tenant_id = $1 AND order_id = $2",
        ),
        (
            "packing session plan update",
            "UPDATE packing_sessions SET expected_qty = expected_qty + 1 WHERE tenant_id = $1 AND order_id = $2",
        ),
        (
            "closed carton second transition",
            "UPDATE cartons SET state = 'open' WHERE tenant_id = $1 AND order_id = $2",
        ),
        (
            "carton content delete",
            "DELETE FROM carton_contents WHERE tenant_id = $1 AND order_id = $2",
        ),
        (
            "packing session delete",
            "DELETE FROM packing_sessions WHERE tenant_id = $1 AND order_id = $2",
        ),
    ] {
        let result = sqlx::query(statement)
            .bind(access.tenant_id.get())
            .bind(order.order_id)
            .execute(&admin)
            .await;
        assert!(result.is_err(), "{operation} must be rejected");
    }
    admin.close().await;
}

#[tokio::test]
async fn packing_reads_and_replays_fail_closed_across_owner_facility_and_tenant_scopes() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("packing-scope@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "packing-scope-orders",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Packing Scope Owner")
        .await;
    let other_owner_id = fixture
        .inventory_owner(access.tenant_id, "Packing Other Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Packing Scope Facility")
        .await;
    let other_facility_id = fixture
        .facility(access.tenant_id, "Packing Other Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let packing_id = execution_location(
        &fixture,
        access.tenant_id,
        facility_id,
        "PACK-SCOPE-STATION",
        "packing",
    )
    .await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        packing_id,
        "PACK-SCOPE-TOTE",
    )
    .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let order = prepare_order(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        "PACK-SCOPE",
        &[2],
    )
    .await;
    release_order(
        &app,
        &token,
        access.tenant_id,
        order.order_id,
        facility_id,
        packing_id,
        "pack-scope-release",
    )
    .await;
    pick_order(
        &app,
        &token,
        access.tenant_id,
        "PACK-SCOPE-TOTE",
        1,
        "pack-scope",
    )
    .await;
    let opened = open_session(
        &app,
        &token,
        access.tenant_id,
        order.order_id,
        facility_id,
        packing_id,
        "pack-scope-open",
    )
    .await;
    let session_path = format!("/api/v1/packing-sessions/{}", opened.session.session_id);
    let queue_path = format!("/api/v1/packing-queue?facility_id={facility_id}&limit=1");
    let queue = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &queue_path,
        None,
        None,
    )
    .await;
    let queue: PackingQueuePage =
        response_json(expect_status(queue, StatusCode::OK, "read scoped packing queue").await)
            .await;
    assert_eq!(queue.items.len(), 1);
    assert_eq!(queue.items[0].order_id, order.order_id);
    assert_eq!(
        queue.items[0]
            .session
            .as_ref()
            .map(|session| session.session_id),
        Some(opened.session.session_id)
    );

    let mismatched_cursor = format!(
        "/api/v1/packing-queue?facility_id={other_facility_id}&limit=1&cursor=pq1.{facility_id:016x}.0.n.{:016x}",
        order.order_id
    );
    let mismatched_cursor = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &mismatched_cursor,
        None,
        None,
    )
    .await;
    assert_eq!(mismatched_cursor.status(), StatusCode::BAD_REQUEST);

    let queue_operator = add_wms_operator(
        &fixture,
        access.tenant_id,
        "packing-queue-only@test.local",
        "packing-queue-only",
    )
    .await;
    set_scope(
        &fixture.db,
        access.tenant_id,
        queue_operator.id,
        vec![facility_id],
        vec![owner_id],
    )
    .await;
    let queue_operator_token = auth::create_session(&fixture.db, queue_operator.id)
        .await
        .unwrap();
    let wms_only_queue = send(
        &app,
        &queue_operator_token,
        access.tenant_id,
        Method::GET,
        &queue_path,
        None,
        None,
    )
    .await;
    let wms_only_queue: PackingQueuePage = response_json(
        expect_status(wms_only_queue, StatusCode::OK, "WMS-only packing queue").await,
    )
    .await;
    assert_eq!(wms_only_queue.items.len(), 1);
    assert_eq!(wms_only_queue.items[0].order_id, order.order_id);

    let owner_denied = add_wms_operator(
        &fixture,
        access.tenant_id,
        "packing-owner-denied@test.local",
        "packing-owner-denied",
    )
    .await;
    set_scope(
        &fixture.db,
        access.tenant_id,
        owner_denied.id,
        vec![facility_id],
        vec![other_owner_id],
    )
    .await;
    let owner_denied_token = auth::create_session(&fixture.db, owner_denied.id)
        .await
        .unwrap();
    let concealed = send(
        &app,
        &owner_denied_token,
        access.tenant_id,
        Method::GET,
        &session_path,
        None,
        None,
    )
    .await;
    assert_eq!(concealed.status(), StatusCode::NOT_FOUND);
    let concealed_queue = send(
        &app,
        &owner_denied_token,
        access.tenant_id,
        Method::GET,
        &queue_path,
        None,
        None,
    )
    .await;
    let concealed_queue: PackingQueuePage = response_json(
        expect_status(
            concealed_queue,
            StatusCode::OK,
            "owner-scoped packing queue",
        )
        .await,
    )
    .await;
    assert!(concealed_queue.items.is_empty());

    let facility_denied = add_wms_operator(
        &fixture,
        access.tenant_id,
        "packing-facility-denied@test.local",
        "packing-facility-denied",
    )
    .await;
    set_scope(
        &fixture.db,
        access.tenant_id,
        facility_denied.id,
        vec![other_facility_id],
        vec![owner_id],
    )
    .await;
    let facility_denied_token = auth::create_session(&fixture.db, facility_denied.id)
        .await
        .unwrap();
    let concealed = send(
        &app,
        &facility_denied_token,
        access.tenant_id,
        Method::GET,
        &session_path,
        None,
        None,
    )
    .await;
    assert_eq!(concealed.status(), StatusCode::NOT_FOUND);
    let concealed_queue = send(
        &app,
        &facility_denied_token,
        access.tenant_id,
        Method::GET,
        &queue_path,
        None,
        None,
    )
    .await;
    let concealed_queue: PackingQueuePage = response_json(
        expect_status(
            concealed_queue,
            StatusCode::OK,
            "facility-scoped packing queue",
        )
        .await,
    )
    .await;
    assert!(concealed_queue.items.is_empty());

    let outsider = fixture.wms_user("packing-outsider@test.local").await;
    let outsider_tenant = tenant_for_user(&fixture.db, outsider.id).await;
    let outsider_token = auth::create_session(&fixture.db, outsider.id)
        .await
        .unwrap();
    let concealed = send(
        &app,
        &outsider_token,
        outsider_tenant,
        Method::GET,
        &session_path,
        None,
        None,
    )
    .await;
    assert_eq!(concealed.status(), StatusCode::NOT_FOUND);
    let app_db = app_db_for(&fixture.db).await;
    let mut outsider_tx = tenant_tx(&app_db, outsider_tenant).await;
    let cross_tenant: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM outbound_order_containers WHERE tenant_id = $1),
               (SELECT COUNT(*) FROM packing_sessions WHERE tenant_id = $1),
               (SELECT COUNT(*) FROM packing_session_allocations WHERE tenant_id = $1),
               (SELECT COUNT(*) FROM cartons WHERE tenant_id = $1),
               (SELECT COUNT(*) FROM carton_contents WHERE tenant_id = $1)
        "#,
    )
    .bind(access.tenant_id.get())
    .fetch_one(&mut *outsider_tx)
    .await
    .unwrap();
    outsider_tx.rollback().await.unwrap();
    app_db.close().await;
    assert_eq!(cross_tenant, (0, 0, 0, 0, 0));

    set_scope(
        &fixture.db,
        access.tenant_id,
        operator.id,
        Vec::new(),
        Vec::new(),
    )
    .await;
    let concealed_replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{}/packing-sessions", order.order_id),
        Some("pack-scope-open"),
        Some(json!({
            "facility_id": facility_id,
            "station_location_id": packing_id,
            "expected_revision": 4
        })),
    )
    .await;
    assert_eq!(concealed_replay.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn controlled_stock_requires_exact_lot_serial_and_packing_station_scans() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("packing-controlled@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "packing-controlled-orders",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Packing Controlled Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Packing Controlled Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    set_scope(
        &fixture.db,
        access.tenant_id,
        operator.id,
        vec![facility_id],
        vec![owner_id],
    )
    .await;
    let packing_id = execution_location(
        &fixture,
        access.tenant_id,
        facility_id,
        "PACK-CONTROLLED-STATION",
        "packing",
    )
    .await;
    let wrong_packing_id = execution_location(
        &fixture,
        access.tenant_id,
        facility_id,
        "PACK-CONTROLLED-WRONG-STATION",
        "packing",
    )
    .await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        packing_id,
        "PACK-CONTROLLED-TOTE",
    )
    .await;

    let item_id = fixture
        .item(access.tenant_id, "Packing Controlled Item", "each")
        .await;
    repo::items::add_barcode(
        &fixture.db,
        access.tenant_id,
        item_id,
        "PACK-CONTROLLED-ITEM",
        "code128",
        None,
    )
    .await
    .unwrap();
    let order_id = fixture
        .order_header(access.tenant_id, "PACK-CONTROLLED", owner_id)
        .await;
    fixture
        .order_item(access.tenant_id, order_id, item_id, 2)
        .await;
    let source_a = fixture
        .location(access.tenant_id, facility_id, "PACK-CONTROLLED-SOURCE-A")
        .await;
    let source_b = fixture
        .location(access.tenant_id, facility_id, "PACK-CONTROLLED-SOURCE-B")
        .await;
    for (source_location_id, lot, serial, key) in [
        (
            source_a,
            "PACK-LOT-A",
            "PACK-SERIAL-A",
            "pack-controlled-receive-a",
        ),
        (
            source_b,
            "PACK-LOT-B",
            "PACK-SERIAL-B",
            "pack-controlled-receive-b",
        ),
    ] {
        let batch_id = repo::inventory::add_item_batch(
            &fixture.db,
            access.tenant_id,
            owner_id,
            item_id,
            None,
            Some(lot),
            Some(serial),
            None,
        )
        .await
        .unwrap();
        repo::inventory::receive_inventory(
            &fixture.db,
            access.tenant_id,
            operator.id,
            batch_id,
            source_location_id,
            1,
            None,
            Some("controlled packing stock"),
            None,
            None,
            key,
        )
        .await
        .unwrap();
    }

    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let allocated = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{order_id}/allocation-runs"),
        Some("pack-controlled-allocate"),
        Some(json!({
            "facility_id": facility_id,
            "expected_revision": 1,
            "expected_policy": {"source": "product_default", "policy_hash": "6090a99a06ea2e049d7321d5cf2b8f462c6d6e6e2ca527ae87657a7a5fd9d156"}
        })),
    )
    .await;
    expect_status(allocated, StatusCode::OK, "allocate controlled order").await;
    release_order(
        &app,
        &token,
        access.tenant_id,
        order_id,
        facility_id,
        packing_id,
        "pack-controlled-release",
    )
    .await;
    let picks = pick_order(
        &app,
        &token,
        access.tenant_id,
        "PACK-CONTROLLED-TOTE",
        2,
        "pack-controlled",
    )
    .await;
    assert_eq!(picks[1].order_status, PickOrderStatus::AwaitingPacking);
    assert_eq!(picks[1].order_revision.get(), 4);

    let wrong_station = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{order_id}/packing-sessions"),
        Some("pack-controlled-wrong-station"),
        Some(json!({
            "facility_id": facility_id,
            "station_location_id": wrong_packing_id,
            "expected_revision": 4
        })),
    )
    .await;
    assert_eq!(wrong_station.status(), StatusCode::CONFLICT);
    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let rejected_open: (i64, String, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM packing_sessions WHERE tenant_id = $1 AND order_id = $2),
               status, revision
        FROM orders
        WHERE tenant_id = $1 AND id = $2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(rejected_open, (0, "awaiting packing".into(), 4));

    let opened = open_session(
        &app,
        &token,
        access.tenant_id,
        order_id,
        facility_id,
        packing_id,
        "pack-controlled-open",
    )
    .await;
    assert_eq!(opened.session.revision.get(), 5);
    assert_eq!(opened.session.allocations.len(), 2);
    assert!(opened
        .session
        .allocations
        .iter()
        .all(|allocation| allocation.item_id == item_id));
    let allocation_a = opened
        .session
        .allocations
        .iter()
        .find(|allocation| allocation.lot.as_deref() == Some("PACK-LOT-A"))
        .cloned()
        .unwrap();
    let allocation_b = opened
        .session
        .allocations
        .iter()
        .find(|allocation| allocation.lot.as_deref() == Some("PACK-LOT-B"))
        .cloned()
        .unwrap();
    assert_eq!(allocation_a.serial.as_deref(), Some("PACK-SERIAL-A"));
    assert_eq!(allocation_b.serial.as_deref(), Some("PACK-SERIAL-B"));

    let empty_carton = create_carton(
        &app,
        &token,
        access.tenant_id,
        opened.session.session_id,
        "PACK-CONTROLLED-EMPTY",
        5,
        "pack-controlled-empty",
    )
    .await;
    assert_eq!(empty_carton.revision.get(), 6);
    let void_path = format!(
        "/api/v1/packing-sessions/{}/cartons/{}/voids",
        opened.session.session_id, empty_carton.carton.carton_id
    );
    let void_body = json!({
        "carton_barcode": "PACK-CONTROLLED-EMPTY",
        "expected_revision": 6
    });
    let voided = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &void_path,
        Some("pack-controlled-void-empty"),
        Some(void_body.clone()),
    )
    .await;
    let voided: VoidCartonResponse =
        response_json(expect_status(voided, StatusCode::OK, "void empty controlled carton").await)
            .await;
    assert_eq!(voided.revision.get(), 7);
    assert_eq!(voided.progress.open_carton_count, 0);
    assert!(matches!(
        &voided.lifecycle,
        PackCartonLifecycleResponse::Voided { .. }
    ));
    let void_replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &void_path,
        Some("pack-controlled-void-empty"),
        Some(void_body),
    )
    .await;
    assert_eq!(void_replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<VoidCartonResponse>(void_replay).await,
        voided
    );
    let void_changed_payload = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &void_path,
        Some("pack-controlled-void-empty"),
        Some(json!({
            "carton_barcode": "PACK-CONTROLLED-CHANGED",
            "expected_revision": 6
        })),
    )
    .await;
    assert_eq!(void_changed_payload.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(void_changed_payload)
            .await
            .reason,
        ErrorReason::IdempotencyKeyReused
    );

    let carton = create_carton(
        &app,
        &token,
        access.tenant_id,
        opened.session.session_id,
        "PACK-CONTROLLED-CARTON",
        7,
        "pack-controlled-carton",
    )
    .await;
    assert_eq!(carton.revision.get(), 8);
    let pack_path = format!(
        "/api/v1/packing-sessions/{}/cartons/{}/contents",
        opened.session.session_id, carton.carton.carton_id
    );
    let item_barcode = &allocation_a.item_barcodes[0];
    for (key, body) in [
        (
            "pack-controlled-missing-lot",
            controlled_pack_body(
                allocation_a.inventory_allocation_id,
                item_barcode,
                None,
                Some("PACK-SERIAL-A"),
                "PACK-CONTROLLED-TOTE",
                "PACK-CONTROLLED-CARTON",
                8,
            ),
        ),
        (
            "pack-controlled-wrong-lot",
            controlled_pack_body(
                allocation_a.inventory_allocation_id,
                item_barcode,
                Some("PACK-LOT-WRONG"),
                Some("PACK-SERIAL-A"),
                "PACK-CONTROLLED-TOTE",
                "PACK-CONTROLLED-CARTON",
                8,
            ),
        ),
        (
            "pack-controlled-missing-serial",
            controlled_pack_body(
                allocation_a.inventory_allocation_id,
                item_barcode,
                Some("PACK-LOT-A"),
                None,
                "PACK-CONTROLLED-TOTE",
                "PACK-CONTROLLED-CARTON",
                8,
            ),
        ),
        (
            "pack-controlled-wrong-serial",
            controlled_pack_body(
                allocation_a.inventory_allocation_id,
                item_barcode,
                Some("PACK-LOT-A"),
                Some("PACK-SERIAL-WRONG"),
                "PACK-CONTROLLED-TOTE",
                "PACK-CONTROLLED-CARTON",
                8,
            ),
        ),
        (
            "pack-controlled-other-allocation-identity",
            controlled_pack_body(
                allocation_a.inventory_allocation_id,
                item_barcode,
                allocation_b.lot.as_deref(),
                allocation_b.serial.as_deref(),
                "PACK-CONTROLLED-TOTE",
                "PACK-CONTROLLED-CARTON",
                8,
            ),
        ),
    ] {
        let rejected = send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &pack_path,
            Some(key),
            Some(body),
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST, "{key}");
    }
    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let rejected_scans: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM carton_contents WHERE tenant_id = $1 AND packing_session_id = $2),
               (SELECT COUNT(*) FROM inventory_transactions WHERE tenant_id = $1 AND operation = 'packing.content.confirm.v1'),
               (SELECT revision FROM packing_sessions WHERE tenant_id = $1 AND id = $2),
               (SELECT revision FROM orders WHERE tenant_id = $1 AND id = $3)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(opened.session.session_id)
    .bind(order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(rejected_scans, (0, 0, 8, 8));

    let packed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &pack_path,
        Some("pack-controlled-exact"),
        Some(controlled_pack_body(
            allocation_a.inventory_allocation_id,
            item_barcode,
            allocation_a.lot.as_deref(),
            allocation_a.serial.as_deref(),
            "PACK-CONTROLLED-TOTE",
            "PACK-CONTROLLED-CARTON",
            8,
        )),
    )
    .await;
    let packed: PackPickedAllocationResponse = response_json(
        expect_status(packed, StatusCode::OK, "pack exact controlled allocation").await,
    )
    .await;
    assert_eq!(
        packed.inventory_allocation_id,
        allocation_a.inventory_allocation_id
    );
    assert_eq!(packed.item_batch_id, allocation_a.item_batch_id);
    assert_eq!(packed.revision.get(), 9);

    let nonempty_void = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/packing-sessions/{}/cartons/{}/voids",
            opened.session.session_id, carton.carton.carton_id
        ),
        Some("pack-controlled-void-nonempty"),
        Some(json!({
            "carton_barcode": "PACK-CONTROLLED-CARTON",
            "expected_revision": 9
        })),
    )
    .await;
    assert_eq!(nonempty_void.status(), StatusCode::CONFLICT);
    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let rejected_nonempty_void: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM carton_contents WHERE tenant_id = $1 AND packing_session_id = $2),
               (SELECT COUNT(*) FROM inventory_transactions WHERE tenant_id = $1 AND operation = 'packing.content.confirm.v1'),
               (SELECT revision FROM packing_sessions WHERE tenant_id = $1 AND id = $2),
               (SELECT revision FROM orders WHERE tenant_id = $1 AND id = $3)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(opened.session.session_id)
    .bind(order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(rejected_nonempty_void, (1, 1, 9, 9));

    let admin = admin_db_for(&fixture.db).await;
    for (operation, statement) in [
        (
            "voided carton lifecycle update",
            "UPDATE cartons SET state = 'open' WHERE tenant_id = $1 AND id = $2",
        ),
        (
            "voided carton delete",
            "DELETE FROM cartons WHERE tenant_id = $1 AND id = $2",
        ),
    ] {
        let result = sqlx::query(statement)
            .bind(access.tenant_id.get())
            .bind(empty_carton.carton.carton_id)
            .execute(&admin)
            .await;
        assert!(result.is_err(), "{operation} must be rejected");
    }
    admin.close().await;
}

#[tokio::test]
async fn packing_ledgers_are_forced_rls_and_minimally_granted() {
    let fixture = Fixture::new().await;
    let admin = admin_db_for(&fixture.db).await;
    for (table, can_insert, can_update) in [
        ("outbound_order_containers", true, false),
        ("packing_sessions", true, true),
        ("packing_session_allocations", true, false),
        ("cartons", true, true),
        ("carton_contents", true, false),
        ("packing_allocation_positions", false, false),
        ("carton_content_removals", true, false),
        ("carton_weight_evidence", true, false),
    ] {
        let privileges: (bool, bool, bool, bool) = sqlx::query_as(
            r#"
            SELECT has_table_privilege('wareboxes_app', $1, 'SELECT'),
                   has_table_privilege('wareboxes_app', $1, 'INSERT'),
                   has_table_privilege('wareboxes_app', $1, 'UPDATE'),
                   has_table_privilege('wareboxes_app', $1, 'DELETE')
            "#,
        )
        .bind(table)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(privileges, (true, can_insert, can_update, false), "{table}");
        let forced_rls: bool =
            sqlx::query_scalar("SELECT relforcerowsecurity FROM pg_class WHERE oid = $1::regclass")
                .bind(table)
                .fetch_one(&admin)
                .await
                .unwrap();
        assert!(forced_rls, "{table}");
    }
    for column in [
        "state",
        "current_carton_content_id",
        "current_inventory_allocation_id",
        "current_inventory_balance_id",
        "current_location_id",
        "current_license_plate_id",
        "revision",
        "positioned_at",
    ] {
        let can_update: bool = sqlx::query_scalar(
            "SELECT has_column_privilege('wareboxes_app', 'packing_allocation_positions', $1, 'UPDATE')",
        )
        .bind(column)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(can_update, "packing_allocation_positions.{column}");
    }
    let cannot_rewrite_identity: bool = sqlx::query_scalar(
        "SELECT has_column_privilege('wareboxes_app', 'packing_allocation_positions', 'packing_session_allocation_id', 'UPDATE')",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(!cannot_rewrite_identity);
    for sequence in [
        "outbound_order_containers_id_seq",
        "packing_sessions_id_seq",
        "packing_session_allocations_id_seq",
        "cartons_id_seq",
        "carton_contents_id_seq",
        "carton_content_removals_id_seq",
        "carton_weight_evidence_id_seq",
    ] {
        let usage: bool =
            sqlx::query_scalar("SELECT has_sequence_privilege('wareboxes_app', $1, 'USAGE')")
                .bind(sequence)
                .fetch_one(&admin)
                .await
                .unwrap();
        assert!(usage, "{sequence}");
    }
    admin.close().await;
}
