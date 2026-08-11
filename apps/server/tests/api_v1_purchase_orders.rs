mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    CreatePurchaseOrderAsnResponse, CreatePurchaseOrderResponse, ErrorReason, ErrorResponse,
    InboundAsnDetailResponse, PurchaseOrderDetailResponse, PurchaseOrderPage, PurchaseOrderStatus,
    ReleasePurchaseOrderResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;

struct PurchaseOrderFixture {
    fixture: Fixture,
    tenant_id: TenantId,
    actor_id: i64,
    facility_id: i64,
    owner_id: i64,
    item_id: i64,
    second_item_id: i64,
    token: String,
}

async fn fixture(email: &str) -> PurchaseOrderFixture {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user(email).await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let facility_id = fixture
        .facility(tenant_id, "Purchase Order Distribution Center")
        .await;
    let owner_id = fixture
        .inventory_owner(tenant_id, "Purchase Order Client")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;
    let item_id = fixture
        .item(tenant_id, "Purchase Order Beans", "case")
        .await;
    let second_item_id = fixture
        .item(tenant_id, "Purchase Order Towels", "each")
        .await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    for item_id in [item_id, second_item_id] {
        sqlx::query(
            "INSERT INTO inventory_owner_items(tenant_id,created,inventory_owner_id,item_id) VALUES ($1,$2,$3,$4)",
        )
        .bind(tenant_id.get())
        .bind(db::now_iso())
        .bind(owner_id)
        .bind(item_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    PurchaseOrderFixture {
        fixture,
        tenant_id,
        actor_id: operator.id,
        facility_id,
        owner_id,
        item_id,
        second_item_id,
        token,
    }
}

fn command_request(
    context: &PurchaseOrderFixture,
    path: &str,
    key: &str,
    body: &Value,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/{path}"))
        .header(header::AUTHORIZATION, format!("Bearer {}", context.token))
        .header(TENANT_ID_HEADER, context.tenant_id.to_string())
        .header(IDEMPOTENCY_KEY_HEADER, key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn get_request(context: &PurchaseOrderFixture, path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/{path}"))
        .header(header::AUTHORIZATION, format!("Bearer {}", context.token))
        .header(TENANT_ID_HEADER, context.tenant_id.to_string())
        .body(Body::empty())
        .unwrap()
}

fn create_body(context: &PurchaseOrderFixture, number: &str, first_qty: i64) -> Value {
    json!({
        "inventory_owner_id": context.owner_id,
        "facility_id": context.facility_id,
        "number": number,
        "supplier": "Northstar Foods",
        "expected_by": "2027-08-20T17:00:00Z",
        "lines": [
            {"item_id": context.item_id, "ordered_quantity": first_qty},
            {"item_id": context.second_item_id, "ordered_quantity": 8}
        ]
    })
}

async fn json_body<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn create_order(context: &PurchaseOrderFixture, number: &str) -> CreatePurchaseOrderResponse {
    let response = routes::app(AppState::new(context.fixture.db.clone()))
        .oneshot(command_request(
            context,
            "purchase-orders",
            &format!("create-{number}"),
            &create_body(context, number, 12),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    json_body(response).await
}

async fn release_order(
    context: &PurchaseOrderFixture,
    order: &CreatePurchaseOrderResponse,
    key: &str,
) -> ReleasePurchaseOrderResponse {
    let response = routes::app(AppState::new(context.fixture.db.clone()))
        .oneshot(command_request(
            context,
            &format!("purchase-orders/{}/releases", order.purchase_order_id),
            key,
            &json!({"expected_revision": order.revision.get()}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    json_body(response).await
}

fn asn_body(
    order: &CreatePurchaseOrderResponse,
    number: &str,
    first_quantity: i64,
    second_quantity: i64,
) -> Value {
    json!({
        "expected_purchase_order_revision": 2,
        "number": number,
        "expected_at": "2027-08-18T17:00:00Z",
        "lines": [
            {
                "purchase_order_line_id": order.lines[0].line_id,
                "expected_quantity": first_quantity,
                "lot": format!("{number}-LOT"),
                "serial": null,
                "expiration": "2028-08-18T00:00:00Z"
            },
            {
                "purchase_order_line_id": order.lines[1].line_id,
                "expected_quantity": second_quantity,
                "lot": null,
                "serial": null,
                "expiration": null
            }
        ]
    })
}

#[tokio::test]
async fn creation_is_atomic_race_safe_replayable_and_immutable() {
    let context = fixture("purchase-order-create@test.local").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let body = create_body(&context, "PO-RACE-100", 12);
    let first = app.clone().oneshot(command_request(
        &context,
        "purchase-orders",
        "po-race-a",
        &body,
    ));
    let second = app.clone().oneshot(command_request(
        &context,
        "purchase-orders",
        "po-race-b",
        &body,
    ));
    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap();
    let second = second.unwrap();
    let (winner_key, winner_response) = if first.status() == StatusCode::OK {
        assert_eq!(second.status(), StatusCode::CONFLICT);
        ("po-race-a", first)
    } else {
        assert_eq!(first.status(), StatusCode::CONFLICT);
        assert_eq!(second.status(), StatusCode::OK);
        ("po-race-b", second)
    };
    let winner = json_body::<CreatePurchaseOrderResponse>(winner_response).await;
    assert_eq!(winner.status, PurchaseOrderStatus::Draft);
    assert_eq!(winner.revision.get(), 1);
    assert_eq!(winner.lines.len(), 2);
    assert_eq!(winner.total_ordered_quantity, 20);

    let replay = app
        .clone()
        .oneshot(command_request(
            &context,
            "purchase-orders",
            winner_key,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        json_body::<CreatePurchaseOrderResponse>(replay).await,
        winner
    );
    let changed = app
        .oneshot(command_request(
            &context,
            "purchase-orders",
            winner_key,
            &create_body(&context, "PO-RACE-100", 13),
        ))
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let effects: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM purchase_orders WHERE number='PO-RACE-100'),
          (SELECT COUNT(*) FROM purchase_order_lines WHERE purchase_order_id=$1),
          (SELECT COUNT(*) FROM outbox_events WHERE event_type='inbound.purchase_order.created'
             AND aggregate_id=$1::TEXT),
          (SELECT COUNT(*) FROM command_idempotency_records
             WHERE operation='inbound.purchase_order.create.v1'
               AND (result_json->>'purchase_order_id')::BIGINT=$1)
        "#,
    )
    .bind(winner.purchase_order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(effects, (1, 2, 1, 1));
    let immutable = sqlx::query(
        "UPDATE purchase_order_lines SET ordered_quantity=99 WHERE purchase_order_id=$1",
    )
    .bind(winner.purchase_order_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(!immutable.to_string().is_empty());
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn release_is_revision_guarded_race_safe_and_audited() {
    let context = fixture("purchase-order-release@test.local").await;
    let created = create_order(&context, "PO-RELEASE-100").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let path = format!("purchase-orders/{}/releases", created.purchase_order_id);
    let body = json!({"expected_revision": 1});
    let first = app
        .clone()
        .oneshot(command_request(&context, &path, "po-release-a", &body));
    let second = app
        .clone()
        .oneshot(command_request(&context, &path, "po-release-b", &body));
    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap();
    let second = second.unwrap();
    let (winner_key, winner_response) = if first.status() == StatusCode::OK {
        assert_eq!(second.status(), StatusCode::CONFLICT);
        ("po-release-a", first)
    } else {
        assert_eq!(first.status(), StatusCode::CONFLICT);
        assert_eq!(second.status(), StatusCode::OK);
        ("po-release-b", second)
    };
    let winner = json_body::<ReleasePurchaseOrderResponse>(winner_response).await;
    assert_eq!(winner.previous_status, PurchaseOrderStatus::Draft);
    assert_eq!(winner.status, PurchaseOrderStatus::Released);
    assert_eq!(winner.revision.get(), 2);

    let replay = app
        .clone()
        .oneshot(command_request(&context, &path, winner_key, &body))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        json_body::<ReleasePurchaseOrderResponse>(replay).await,
        winner
    );
    let stale = app
        .clone()
        .oneshot(command_request(&context, &path, "po-release-stale", &body))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let detail = app
        .oneshot(get_request(
            &context,
            &format!("purchase-orders/{}", created.purchase_order_id),
        ))
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    let detail = json_body::<PurchaseOrderDetailResponse>(detail).await;
    assert_eq!(detail.summary.status, PurchaseOrderStatus::Released);
    assert_eq!(detail.summary.revision.get(), 2);
    assert_eq!(detail.lines.len(), 2);

    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let evidence: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM purchase_order_releases WHERE purchase_order_id=$1),
          (SELECT COUNT(*) FROM outbox_events WHERE event_type='inbound.purchase_order.released'
             AND aggregate_id=$1::TEXT AND aggregate_sequence=2),
          (SELECT COUNT(*) FROM command_idempotency_records
             WHERE operation='inbound.purchase_order.release.v1'
               AND (result_json->>'purchase_order_id')::BIGINT=$1),
          (SELECT COUNT(*) FROM purchase_orders
             WHERE id=$1 AND status='released' AND revision=2)
        "#,
    )
    .bind(created.purchase_order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(evidence, (1, 1, 1, 1));
    let immutable = sqlx::query("DELETE FROM purchase_order_releases WHERE purchase_order_id=$1")
        .bind(created.purchase_order_id)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(!immutable.to_string().is_empty());
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn released_order_sources_multiple_asns_with_exact_conservation_and_replay() {
    let context = fixture("purchase-order-asn@test.local").await;
    let order = create_order(&context, "PO-ASN-100").await;
    let release = release_order(&context, &order, "release-po-asn-100").await;
    assert_eq!(release.revision.get(), 2);
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let path = format!("purchase-orders/{}/asns", order.purchase_order_id);
    let first_body = asn_body(&order, "ASN-PO-100-A", 5, 3);
    let first = app
        .clone()
        .oneshot(command_request(
            &context,
            &path,
            "po-asn-first",
            &first_body,
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first = json_body::<CreatePurchaseOrderAsnResponse>(first).await;
    assert_eq!(first.purchase_order_id, order.purchase_order_id);
    assert_eq!(first.purchase_order_revision.get(), 2);
    assert_eq!(first.total_expected_quantity, 8);
    assert_eq!(first.lines.len(), 2);

    let replay = app
        .clone()
        .oneshot(command_request(
            &context,
            &path,
            "po-asn-first",
            &first_body,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        json_body::<CreatePurchaseOrderAsnResponse>(replay).await,
        first
    );
    let changed = app
        .clone()
        .oneshot(command_request(
            &context,
            &path,
            "po-asn-first",
            &asn_body(&order, "ASN-PO-100-A", 6, 3),
        ))
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let second = app
        .clone()
        .oneshot(command_request(
            &context,
            &path,
            "po-asn-second",
            &asn_body(&order, "ASN-PO-100-B", 7, 5),
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second = json_body::<CreatePurchaseOrderAsnResponse>(second).await;
    assert_eq!(second.total_expected_quantity, 12);

    let over = app
        .clone()
        .oneshot(command_request(
            &context,
            &path,
            "po-asn-over",
            &asn_body(&order, "ASN-PO-100-C", 1, 1),
        ))
        .await
        .unwrap();
    assert_eq!(over.status(), StatusCode::CONFLICT);

    let detail = app
        .clone()
        .oneshot(get_request(
            &context,
            &format!("purchase-orders/{}", order.purchase_order_id),
        ))
        .await
        .unwrap();
    let detail = json_body::<PurchaseOrderDetailResponse>(detail).await;
    assert_eq!(detail.summary.total_ordered_quantity, 20);
    assert_eq!(detail.summary.total_asn_expected_quantity, 20);
    assert_eq!(detail.summary.total_remaining_quantity, 0);
    assert!(detail.lines.iter().all(|line| line.remaining_quantity == 0));
    let asn_detail = app
        .oneshot(get_request(
            &context,
            &format!("inbound-asns/{}", second.asn_id),
        ))
        .await
        .unwrap();
    let asn_detail = json_body::<InboundAsnDetailResponse>(asn_detail).await;
    assert_eq!(
        asn_detail.summary.purchase_order_id,
        Some(order.purchase_order_id)
    );
    assert_eq!(
        asn_detail.summary.purchase_order_number.as_deref(),
        Some("PO-ASN-100")
    );

    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let effects: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM purchase_order_asn_sources WHERE purchase_order_id=$1),
          (SELECT COUNT(*) FROM purchase_order_asn_source_lines WHERE purchase_order_id=$1),
          (SELECT COUNT(*) FROM inbound_asns WHERE id IN ($2,$3)),
          (SELECT COUNT(*) FROM outbox_events WHERE event_type='inbound.asn.created'
             AND aggregate_id IN ($2::TEXT,$3::TEXT)),
          (SELECT COUNT(*) FROM command_idempotency_records
             WHERE operation='inbound.purchase_order.asn.create.v1'
               AND (result_json->>'purchase_order_id')::BIGINT=$1)
        "#,
    )
    .bind(order.purchase_order_id)
    .bind(first.asn_id)
    .bind(second.asn_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(effects, (2, 4, 2, 2, 2));
    let immutable = sqlx::query(
        "UPDATE purchase_order_asn_source_lines SET expected_quantity=1 WHERE purchase_order_id=$1",
    )
    .bind(order.purchase_order_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(!immutable.to_string().is_empty());
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn concurrent_notices_cannot_overstate_remaining_order_demand() {
    let context = fixture("purchase-order-asn-race@test.local").await;
    let order = create_order(&context, "PO-ASN-RACE").await;
    release_order(&context, &order, "release-po-asn-race").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let path = format!("purchase-orders/{}/asns", order.purchase_order_id);
    let first = app.clone().oneshot(command_request(
        &context,
        &path,
        "po-asn-race-a",
        &asn_body(&order, "ASN-PO-RACE-A", 8, 6),
    ));
    let second = app.clone().oneshot(command_request(
        &context,
        &path,
        "po-asn-race-b",
        &asn_body(&order, "ASN-PO-RACE-B", 8, 6),
    ));
    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(
        [first.status(), second.status()]
            .into_iter()
            .filter(|status| *status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        [first.status(), second.status()]
            .into_iter()
            .filter(|status| *status == StatusCode::CONFLICT)
            .count(),
        1
    );
    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let quantities: Vec<(i64, i64, i64)> = sqlx::query_as(
        r#"
        SELECT line.ordered_quantity,COALESCE(SUM(mapping.expected_quantity),0)::BIGINT,
               line.ordered_quantity-COALESCE(SUM(mapping.expected_quantity),0)::BIGINT
        FROM purchase_order_lines line
        LEFT JOIN purchase_order_asn_source_lines mapping
          ON mapping.tenant_id=line.tenant_id AND mapping.purchase_order_line_id=line.id
        WHERE line.purchase_order_id=$1
        GROUP BY line.id,line.sequence,line.ordered_quantity
        ORDER BY line.sequence
        "#,
    )
    .bind(order.purchase_order_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(quantities, vec![(12, 8, 4), (8, 6, 2)]);
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn pages_replays_and_ledgers_are_scope_bound_with_minimal_grants() {
    let context = fixture("purchase-order-scope@test.local").await;
    assert!(repo::tenants::update_user_access_scope(
        &context.fixture.db,
        context.tenant_id,
        &UpdateUserAccessScope {
            user_id: context.actor_id,
            all_facilities: false,
            facility_ids: vec![context.facility_id],
            all_inventory_owners: false,
            inventory_owner_ids: vec![context.owner_id],
        },
    )
    .await
    .unwrap());
    let first = create_order(&context, "PO-PAGE-100").await;
    let _second = create_order(&context, "PO-PAGE-101").await;
    let sourced = create_order(&context, "PO-SCOPE-ASN").await;
    release_order(&context, &sourced, "release-po-scope-asn").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let sourced_path = format!("purchase-orders/{}/asns", sourced.purchase_order_id);
    let sourced_body = asn_body(&sourced, "ASN-PO-SCOPE", 4, 3);
    let sourced_response = app
        .clone()
        .oneshot(command_request(
            &context,
            &sourced_path,
            "po-scope-asn",
            &sourced_body,
        ))
        .await
        .unwrap();
    assert_eq!(sourced_response.status(), StatusCode::OK);
    let page = app
        .clone()
        .oneshot(get_request(
            &context,
            "purchase-orders?status=draft&limit=1",
        ))
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let page = json_body::<PurchaseOrderPage>(page).await;
    assert_eq!(page.items.len(), 1);
    let cursor = page.next_cursor.unwrap();
    let next = app
        .clone()
        .oneshot(get_request(
            &context,
            &format!("purchase-orders?status=draft&limit=1&cursor={cursor}"),
        ))
        .await
        .unwrap();
    assert_eq!(next.status(), StatusCode::OK);
    assert_eq!(json_body::<PurchaseOrderPage>(next).await.items.len(), 1);
    let mismatched = app
        .clone()
        .oneshot(get_request(
            &context,
            &format!("purchase-orders?status=released&limit=1&cursor={cursor}"),
        ))
        .await
        .unwrap();
    assert_eq!(mismatched.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body::<ErrorResponse>(mismatched).await.reason,
        ErrorReason::InvalidCursor
    );

    assert!(repo::tenants::update_user_access_scope(
        &context.fixture.db,
        context.tenant_id,
        &UpdateUserAccessScope {
            user_id: context.actor_id,
            all_facilities: false,
            facility_ids: vec![],
            all_inventory_owners: false,
            inventory_owner_ids: vec![],
        },
    )
    .await
    .unwrap());
    for body in [
        create_body(&context, "PO-PAGE-100", 12),
        create_body(&context, "PO-PAGE-100", 13),
    ] {
        let hidden = app
            .clone()
            .oneshot(command_request(
                &context,
                "purchase-orders",
                "create-PO-PAGE-100",
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    }
    for body in [sourced_body, asn_body(&sourced, "ASN-PO-SCOPE", 5, 3)] {
        let hidden = app
            .clone()
            .oneshot(command_request(
                &context,
                &sourced_path,
                "po-scope-asn",
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    }
    let hidden_detail = app
        .oneshot(get_request(
            &context,
            &format!("purchase-orders/{}", first.purchase_order_id),
        ))
        .await
        .unwrap();
    assert_eq!(hidden_detail.status(), StatusCode::NOT_FOUND);

    let admin = admin_db_for(&context.fixture.db).await;
    for table in [
        "purchase_orders",
        "purchase_order_lines",
        "purchase_order_releases",
        "purchase_order_asn_sources",
        "purchase_order_asn_source_lines",
    ] {
        let checks: (bool, bool) = sqlx::query_as(
            "SELECT relforcerowsecurity,has_table_privilege('wareboxes_app',$1,'DELETE') FROM pg_class WHERE oid=$1::regclass",
        )
        .bind(format!("public.{table}"))
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(checks.0);
        assert!(!checks.1);
    }
    let privileges: (bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT has_table_privilege('wareboxes_app','purchase_order_lines','UPDATE'),
               has_table_privilege('wareboxes_app','purchase_order_releases','UPDATE'),
               has_column_privilege('wareboxes_app','purchase_orders','number','UPDATE')
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(privileges, (false, false, false));
    admin.close().await;
}
