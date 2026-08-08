mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde::Serialize;
use sqlx::Row;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::{IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    AmendFulfillmentOrderRequest, AmendFulfillmentOrderResponse, ErrorReason, ErrorResponse,
    FulfillmentOrderDestination, Revision,
};
use wareboxes_application::order_amendment::AMEND_FULFILLMENT_ORDER_OPERATION;
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_domain::Timestamp;

fn request<T: Serialize>(
    token: &str,
    tenant_id: TenantId,
    order_id: i64,
    key: Option<&str>,
    body: &T,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/orders/{order_id}/amendments"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = key {
        request = request
            .header(IDEMPOTENCY_KEY_HEADER, key)
            .header(REQUEST_ID_HEADER, format!("request-{key}"));
    }
    request
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

async fn send(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    order_id: i64,
    key: Option<&str>,
    body: &AmendFulfillmentOrderRequest,
) -> axum::response::Response {
    app.clone()
        .oneshot(request(token, tenant_id, order_id, key, body))
        .await
        .unwrap()
}

async fn json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn amendment(revision: i64, rush: bool, line1: &str) -> AmendFulfillmentOrderRequest {
    AmendFulfillmentOrderRequest {
        expected_revision: Revision::new(revision).unwrap(),
        rush,
        ship_by: if rush {
            Some("2027-08-12T17:00:00Z".into())
        } else {
            None
        },
        destination: FulfillmentOrderDestination {
            recipient_name: "Receiving Team".into(),
            company: Some("Northstar Retail".into()),
            phone: Some("+1 775 555 0100".into()),
            email: Some("receiving@example.com".into()),
            line1: line1.into(),
            line2: Some("Dock 4".into()),
            city: "Reno".into(),
            region: "NV".into(),
            postal_code: "89502".into(),
            country: "US".into(),
        },
    }
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
        Some("Amend fulfillment orders"),
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_permission(db, tenant_id, role, permission)
        .await
        .unwrap();
    wareboxes_persistence_postgres::roles::add_role_to_user(db, tenant_id, user_id, role)
        .await
        .unwrap();
}

#[tokio::test]
async fn amendment_is_replay_safe_audited_and_preserves_allocated_demand() {
    let fixture = Fixture::new().await;
    let user = fixture.user("order-amendment@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant_orders(&fixture.db, tenant_id, user.id, "order-amendment").await;
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let access = repo::tenants::access_for_user(&fixture.db, user.id, tenant_id)
        .await
        .unwrap()
        .unwrap();
    let owner_id = fixture.inventory_owner(tenant_id, "Amendment Client").await;
    let facility_id = fixture.facility(tenant_id, "Amendment DC").await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;
    let item_id = fixture.item(tenant_id, "Amendment Item", "case").await;
    let order_id = fixture.order_header(tenant_id, "AMEND-001", owner_id).await;
    fixture.order_item(tenant_id, order_id, item_id, 4).await;
    let received = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id,
                qty: 4,
                key: "AMEND-STOCK",
            },
        )
        .await;
    let allocation = fixture
        .allocated_reservation(
            tenant_id,
            user.id,
            order_id,
            received.balance_id,
            4,
            "amend-allocation",
        )
        .await;
    let app = routes::app(AppState::new(fixture.db.clone()));
    let command = amendment(1, true, "200 Replay Street");

    let missing_key = send(&app, &token, tenant_id, order_id, None, &command).await;
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json::<ErrorResponse>(missing_key).await.reason,
        ErrorReason::IdempotencyKeyRequired
    );

    let first = send(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("amend-order"),
        &command,
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first: AmendFulfillmentOrderResponse = json(first).await;
    assert_eq!(first.order_id, order_id);
    assert_eq!(first.inventory_owner_id, owner_id);
    assert_eq!(first.revision.get(), 2);
    assert!(first.rush);
    assert_eq!(first.destination.line1, "200 Replay Street");

    let replay = send(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("amend-order"),
        &command,
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(json::<AmendFulfillmentOrderResponse>(replay).await, first);

    let reused = send(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("amend-order"),
        &amendment(1, false, "201 Changed Street"),
    )
    .await;
    assert_eq!(reused.status(), StatusCode::CONFLICT);
    assert_eq!(
        json::<ErrorResponse>(reused).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let stale = send(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("stale-amendment"),
        &command,
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let noop = send(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("noop-amendment"),
        &amendment(2, true, "200 Replay Street"),
    )
    .await;
    assert_eq!(noop.status(), StatusCode::BAD_REQUEST);

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let row = sqlx::query(
        r#"
        SELECT orders.order_key, orders.rush, orders.revision, orders.ship_by,
               address.name, address.company, address.line1,
               (SELECT COUNT(*) FROM order_amendments
                WHERE tenant_id=$1 AND order_id=$2) amendment_count,
               (SELECT COUNT(*) FROM order_activity
                WHERE tenant_id=$1 AND order_id=$2
                  AND action='amended fulfillment order header') activity_count,
               (SELECT COUNT(*) FROM outbox_events
                WHERE tenant_id=$1 AND aggregate_type='order'
                  AND aggregate_id=$2::TEXT
                  AND event_type='outbound.order.amended') event_count,
               (SELECT COUNT(*) FROM command_idempotency_records
                WHERE tenant_id=$1 AND operation=$3
                  AND (result_json->>'order_id')::BIGINT=$2) command_count,
               allocation.status allocation_status,
               reservation.status reservation_status,
               balance.qty_reserved
        FROM orders
        INNER JOIN addresses address
          ON address.tenant_id=orders.tenant_id AND address.id=orders.address_id
        INNER JOIN inventory_allocations allocation
          ON allocation.tenant_id=orders.tenant_id AND allocation.id=$4
        INNER JOIN inventory_reservations reservation
          ON reservation.tenant_id=allocation.tenant_id
         AND reservation.id=allocation.reservation_id
        INNER JOIN inventory_balances balance
          ON balance.tenant_id=allocation.tenant_id
         AND balance.id=allocation.inventory_balance_id
        WHERE orders.tenant_id=$1 AND orders.id=$2
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .bind(AMEND_FULFILLMENT_ORDER_OPERATION)
    .bind(allocation.allocation_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(row.try_get::<String, _>("order_key").unwrap(), "AMEND-001");
    assert!(row.try_get::<bool, _>("rush").unwrap());
    assert_eq!(row.try_get::<i64, _>("revision").unwrap(), 2);
    assert!(row
        .try_get::<Option<Timestamp>, _>("ship_by")
        .unwrap()
        .is_some());
    assert_eq!(
        row.try_get::<Option<String>, _>("name").unwrap().as_deref(),
        Some("Receiving Team")
    );
    assert_eq!(
        row.try_get::<Option<String>, _>("company")
            .unwrap()
            .as_deref(),
        Some("Northstar Retail")
    );
    assert_eq!(
        row.try_get::<String, _>("line1").unwrap(),
        "200 Replay Street"
    );
    assert_eq!(row.try_get::<i64, _>("amendment_count").unwrap(), 1);
    assert_eq!(row.try_get::<i64, _>("activity_count").unwrap(), 1);
    assert_eq!(row.try_get::<i64, _>("event_count").unwrap(), 1);
    assert_eq!(row.try_get::<i64, _>("command_count").unwrap(), 1);
    assert_eq!(
        row.try_get::<String, _>("allocation_status").unwrap(),
        "allocated"
    );
    assert_eq!(
        row.try_get::<String, _>("reservation_status").unwrap(),
        "active"
    );
    assert_eq!(row.try_get::<i64, _>("qty_reserved").unwrap(), 4);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn amendment_revision_race_has_one_winner_and_execution_states_are_closed() {
    let fixture = Fixture::new().await;
    let user = fixture.user("order-amendment-race@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant_orders(&fixture.db, tenant_id, user.id, "order-amendment-race").await;
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let owner_id = fixture.inventory_owner(tenant_id, "Race Client").await;
    let order_id = fixture
        .order_header(tenant_id, "AMEND-RACE", owner_id)
        .await;
    let app = routes::app(AppState::new(fixture.db.clone()));

    let left_command = amendment(1, true, "300 Left Street");
    let right_command = amendment(1, true, "400 Right Street");
    let left = send(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("amend-left"),
        &left_command,
    );
    let right = send(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("amend-right"),
        &right_command,
    );
    let (left, right) = tokio::join!(left, right);
    let statuses = [left.status(), right.status()];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CONFLICT)
            .count(),
        1
    );

    let held_id = fixture
        .order_header(tenant_id, "AMEND-HELD", owner_id)
        .await;
    let processing_id = fixture
        .order_header(tenant_id, "AMEND-PROCESSING", owner_id)
        .await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("UPDATE orders SET status='held' WHERE tenant_id=$1 AND id=$2")
        .bind(tenant_id.get())
        .bind(held_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE orders SET status='processing' WHERE tenant_id=$1 AND id=$2")
        .bind(tenant_id.get())
        .bind(processing_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let held = send(
        &app,
        &token,
        tenant_id,
        held_id,
        Some("amend-held"),
        &amendment(1, false, "500 Held Street"),
    )
    .await;
    assert_eq!(held.status(), StatusCode::OK);
    let processing = send(
        &app,
        &token,
        tenant_id,
        processing_id,
        Some("amend-processing"),
        &amendment(1, true, "600 Processing Street"),
    )
    .await;
    assert_eq!(processing.status(), StatusCode::CONFLICT);

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COUNT(DISTINCT order_id) FROM order_amendments WHERE tenant_id=$1",
    )
    .bind(tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(counts, (2, 2));
}

#[tokio::test]
async fn amendment_scope_replay_rls_grants_and_immutable_evidence_fail_closed() {
    let fixture = Fixture::new().await;
    let operator = fixture.user("order-amendment-scope@test.local").await;
    let no_permission = fixture
        .user("order-amendment-no-permission@test.local")
        .await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let admin = admin_db_for(&fixture.db).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id,user_id) VALUES ($1,$2)")
        .bind(tenant_id.get())
        .bind(no_permission.id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    grant_orders(&fixture.db, tenant_id, operator.id, "order-amendment-scope").await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let no_permission_token = auth::create_session(&fixture.db, no_permission.id)
        .await
        .unwrap();
    let allowed_owner = fixture.inventory_owner(tenant_id, "Allowed Client").await;
    let denied_owner = fixture.inventory_owner(tenant_id, "Denied Client").await;
    let allowed_order = fixture
        .order_header(tenant_id, "AMEND-ALLOWED", allowed_owner)
        .await;
    let denied_order = fixture
        .order_header(tenant_id, "AMEND-DENIED", denied_owner)
        .await;
    repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: operator.id,
            all_facilities: true,
            facility_ids: Vec::new(),
            all_inventory_owners: false,
            inventory_owner_ids: vec![allowed_owner],
        },
    )
    .await
    .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let denied = send(
        &app,
        &token,
        tenant_id,
        denied_order,
        Some("amend-denied"),
        &amendment(1, true, "700 Denied Street"),
    )
    .await;
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    let forbidden = send(
        &app,
        &no_permission_token,
        tenant_id,
        allowed_order,
        Some("amend-forbidden"),
        &amendment(1, true, "800 Forbidden Street"),
    )
    .await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let command = amendment(1, true, "900 Allowed Street");
    let success = send(
        &app,
        &token,
        tenant_id,
        allowed_order,
        Some("amend-visible"),
        &command,
    )
    .await;
    assert_eq!(success.status(), StatusCode::OK);
    let success: AmendFulfillmentOrderResponse = json(success).await;

    repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: operator.id,
            all_facilities: true,
            facility_ids: Vec::new(),
            all_inventory_owners: false,
            inventory_owner_ids: Vec::new(),
        },
    )
    .await
    .unwrap();
    let concealed_replay = send(
        &app,
        &token,
        tenant_id,
        allowed_order,
        Some("amend-visible"),
        &command,
    )
    .await;
    assert_eq!(concealed_replay.status(), StatusCode::NOT_FOUND);

    let app_db = app_db_for(&fixture.db).await;
    let other_user = fixture
        .user("order-amendment-other-tenant@test.local")
        .await;
    let other_tenant = tenant_for_user(&fixture.db, other_user.id).await;
    let mut other_tx = app_db.begin().await.unwrap();
    db::bind_tenant_context(&mut other_tx, other_tenant)
        .await
        .unwrap();
    let hidden_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM order_amendments")
        .fetch_one(&mut *other_tx)
        .await
        .unwrap();
    assert_eq!(hidden_count, 0);
    other_tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let resulting_address_id: i64 =
        sqlx::query_scalar("SELECT resulting_address_id FROM order_amendments WHERE id=$1")
            .bind(success.amendment_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(
        sqlx::query("UPDATE order_amendments SET amended_at=amended_at WHERE id=$1")
            .bind(success.amendment_id)
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    assert!(
        sqlx::query("UPDATE addresses SET line1='forged' WHERE id=$1")
            .bind(resulting_address_id)
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("UPDATE orders SET rush=false,revision=revision+1 WHERE tenant_id=$1 AND id=$2")
        .bind(tenant_id.get())
        .bind(allowed_order)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(tx.commit().await.is_err());

    let admin = admin_db_for(&fixture.db).await;
    assert!(
        sqlx::query("UPDATE orders SET order_key='FORGED' WHERE tenant_id=$1 AND id=$2")
            .bind(tenant_id.get())
            .bind(allowed_order)
            .execute(&admin)
            .await
            .is_err()
    );
    let grants = sqlx::query(
        r#"
        SELECT has_table_privilege('wareboxes_app','order_amendments','SELECT') can_select,
               has_table_privilege('wareboxes_app','order_amendments','INSERT') can_insert,
               has_table_privilege('wareboxes_app','order_amendments','UPDATE') can_update,
               has_table_privilege('wareboxes_app','order_amendments','DELETE') can_delete,
               has_sequence_privilege(
                   'wareboxes_app','order_amendments_id_seq','USAGE'
               ) can_use_sequence,
               has_column_privilege('wareboxes_app','orders','order_key','UPDATE') can_update_key,
               has_column_privilege('wareboxes_app','orders','rush','UPDATE') can_update_rush,
               has_column_privilege('wareboxes_app','orders','address_id','UPDATE') can_update_address,
               has_column_privilege('wareboxes_app','orders','revision','UPDATE') can_update_revision
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(grants.try_get::<bool, _>("can_select").unwrap());
    assert!(grants.try_get::<bool, _>("can_insert").unwrap());
    assert!(!grants.try_get::<bool, _>("can_update").unwrap());
    assert!(!grants.try_get::<bool, _>("can_delete").unwrap());
    assert!(grants.try_get::<bool, _>("can_use_sequence").unwrap());
    assert!(!grants.try_get::<bool, _>("can_update_key").unwrap());
    assert!(grants.try_get::<bool, _>("can_update_rush").unwrap());
    assert!(grants.try_get::<bool, _>("can_update_address").unwrap());
    assert!(grants.try_get::<bool, _>("can_update_revision").unwrap());
    admin.close().await;
}
