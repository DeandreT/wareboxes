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
    ErrorReason, ErrorResponse, ReplaceFulfillmentOrderLineRequest,
    ReplaceFulfillmentOrderLinesRequest, ReplaceFulfillmentOrderLinesResponse, Revision,
};
use wareboxes_application::order_line_amendment::REPLACE_FULFILLMENT_ORDER_LINES_OPERATION;
use wareboxes_core::dto::UpdateUserAccessScope;

fn request<T: Serialize>(
    token: &str,
    tenant_id: TenantId,
    order_id: i64,
    key: Option<&str>,
    body: &T,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/orders/{order_id}/line-amendments"))
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
    body: &ReplaceFulfillmentOrderLinesRequest,
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

fn replacement(
    revision: i64,
    lines: impl IntoIterator<Item = (&'static str, i64, i64, &'static str)>,
) -> ReplaceFulfillmentOrderLinesRequest {
    ReplaceFulfillmentOrderLinesRequest {
        expected_revision: Revision::new(revision).unwrap(),
        lines: lines
            .into_iter()
            .map(|(line_key, item_id, quantity, requested_uom)| {
                ReplaceFulfillmentOrderLineRequest {
                    line_key: line_key.into(),
                    item_id,
                    quantity,
                    requested_uom: requested_uom.into(),
                }
            })
            .collect(),
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
        Some("Replace pre-execution demand lines"),
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
async fn exact_replacement_releases_commitments_and_is_replay_safe() {
    let fixture = Fixture::new().await;
    let user = fixture.user("order-lines-replace@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant_orders(&fixture.db, tenant_id, user.id, "order-lines-replace").await;
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let access = repo::tenants::access_for_user(&fixture.db, user.id, tenant_id)
        .await
        .unwrap()
        .unwrap();
    let owner_id = fixture
        .inventory_owner(tenant_id, "Line Replace Client")
        .await;
    let facility_id = fixture.facility(tenant_id, "Line Replace DC").await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;
    let first_item_id = fixture.item(tenant_id, "Original Item", "case").await;
    let second_item_id = fixture.item(tenant_id, "Replacement Item", "each").await;
    let order_id = fixture
        .order_header(tenant_id, "LINE-REPLACE-001", owner_id)
        .await;
    let first_line_id = fixture
        .order_item(tenant_id, order_id, first_item_id, 4)
        .await;
    let second_line_id = fixture
        .order_item(tenant_id, order_id, second_item_id, 2)
        .await;
    let received = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id: first_item_id,
                qty: 4,
                key: "LINE-REPLACE-STOCK",
            },
        )
        .await;
    let commitment = fixture
        .allocated_reservation(
            tenant_id,
            user.id,
            order_id,
            received.balance_id,
            4,
            "line-replace-commitment",
        )
        .await;
    let app = routes::app(AppState::new(fixture.db.clone()));
    let command = replacement(
        1,
        [
            ("replacement-A", first_item_id, 1, "case"),
            ("replacement-B", second_item_id, 6, "each"),
        ],
    );

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
        Some("replace-lines"),
        &command,
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first: ReplaceFulfillmentOrderLinesResponse = json(first).await;
    assert_eq!(first.order_id, order_id);
    assert_eq!(first.inventory_owner_id, owner_id);
    assert_eq!(first.previous_revision.get(), 1);
    assert_eq!(first.revision.get(), 2);
    assert_eq!(first.previous_line_count, 2);
    assert_eq!(first.previous_quantity, 6);
    assert_eq!(first.resulting_quantity, 7);
    assert_eq!(first.released_reservation_count, 1);
    assert_eq!(first.released_allocation_count, 1);
    assert_eq!(first.released_quantity, 4);
    assert_eq!(first.lines.len(), 2);
    assert_eq!(first.lines[0].line_key, "replacement-A");
    assert_eq!(first.lines[1].line_key, "replacement-B");

    let replay = send(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("replace-lines"),
        &command,
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        json::<ReplaceFulfillmentOrderLinesResponse>(replay).await,
        first
    );

    let reused = send(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("replace-lines"),
        &replacement(1, [("changed", first_item_id, 2, "case")]),
    )
    .await;
    assert_eq!(reused.status(), StatusCode::CONFLICT);
    assert_eq!(
        json::<ErrorResponse>(reused).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let effects = sqlx::query(
        r#"SELECT orders.revision,
                  (SELECT COUNT(*) FROM order_items item
                   WHERE item.tenant_id=orders.tenant_id AND item.order_id=orders.id
                     AND item.deleted IS NULL) active_line_count,
                  (SELECT COUNT(*) FROM order_items item
                   WHERE item.tenant_id=orders.tenant_id AND item.order_id=orders.id
                     AND item.deleted IS NOT NULL) retired_line_count,
                  (SELECT COUNT(*) FROM order_line_amendments amendment
                   WHERE amendment.tenant_id=orders.tenant_id
                     AND amendment.order_id=orders.id) amendment_count,
                  (SELECT COUNT(*) FROM order_line_amendment_lines evidence
                   WHERE evidence.tenant_id=orders.tenant_id
                     AND evidence.order_line_amendment_id=$3) evidence_count,
                  (SELECT COUNT(*) FROM order_activity activity
                   WHERE activity.tenant_id=orders.tenant_id
                     AND activity.order_id=orders.id
                     AND action='replaced fulfillment order demand lines') activity_count,
                  (SELECT COUNT(*) FROM outbox_events event
                   WHERE event.tenant_id=orders.tenant_id
                     AND event.aggregate_type='order'
                     AND event.aggregate_id=orders.id::text
                     AND event.event_type='outbound.order.lines_replaced') event_count,
                  (SELECT COUNT(*) FROM command_idempotency_records command
                   WHERE command.tenant_id=orders.tenant_id AND command.operation=$4
                     AND (command.result_json->>'order_id')::bigint=orders.id) command_count,
                  reservation.status reservation_status,
                  allocation.status allocation_status,
                  balance.qty_reserved
           FROM orders
           INNER JOIN inventory_reservations reservation
             ON reservation.tenant_id=orders.tenant_id AND reservation.id=$5
           INNER JOIN inventory_allocations allocation
             ON allocation.tenant_id=orders.tenant_id AND allocation.id=$6
           INNER JOIN inventory_balances balance
             ON balance.tenant_id=orders.tenant_id AND balance.id=$7
           WHERE orders.tenant_id=$1 AND orders.id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .bind(first.amendment_id)
    .bind(REPLACE_FULFILLMENT_ORDER_LINES_OPERATION)
    .bind(commitment.reservation_id)
    .bind(commitment.allocation_id)
    .bind(received.balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(effects.try_get::<i64, _>("revision").unwrap(), 2);
    assert_eq!(effects.try_get::<i64, _>("active_line_count").unwrap(), 2);
    assert_eq!(effects.try_get::<i64, _>("retired_line_count").unwrap(), 2);
    assert_eq!(effects.try_get::<i64, _>("amendment_count").unwrap(), 1);
    assert_eq!(effects.try_get::<i64, _>("evidence_count").unwrap(), 4);
    assert_eq!(effects.try_get::<i64, _>("activity_count").unwrap(), 1);
    assert_eq!(effects.try_get::<i64, _>("event_count").unwrap(), 1);
    assert_eq!(effects.try_get::<i64, _>("command_count").unwrap(), 1);
    assert_eq!(
        effects.try_get::<String, _>("reservation_status").unwrap(),
        "cancelled"
    );
    assert_eq!(
        effects.try_get::<String, _>("allocation_status").unwrap(),
        "released"
    );
    assert_eq!(effects.try_get::<i64, _>("qty_reserved").unwrap(), 0);
    let retired_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM order_items WHERE tenant_id=$1 AND order_id=$2 AND deleted IS NOT NULL ORDER BY id",
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(retired_ids, vec![first_line_id, second_line_id]);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn invalid_sets_execution_states_and_revision_races_have_zero_or_one_effect() {
    let fixture = Fixture::new().await;
    let user = fixture.user("order-lines-race@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant_orders(&fixture.db, tenant_id, user.id, "order-lines-race").await;
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let owner_id = fixture.inventory_owner(tenant_id, "Line Race Client").await;
    let item_id = fixture.item(tenant_id, "Line Race Item", "case").await;
    let order_id = fixture
        .order_header(tenant_id, "LINE-REPLACE-RACE", owner_id)
        .await;
    fixture.order_item(tenant_id, order_id, item_id, 4).await;
    let app = routes::app(AppState::new(fixture.db.clone()));

    let empty = send(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("replace-empty"),
        &replacement(1, []),
    )
    .await;
    assert_eq!(empty.status(), StatusCode::BAD_REQUEST);
    let duplicate = send(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("replace-duplicate"),
        &replacement(
            1,
            [("same", item_id, 2, "case"), ("same", item_id, 2, "case")],
        ),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::BAD_REQUEST);
    let noop = send(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("replace-noop"),
        &replacement(1, [("fixture-1", item_id, 4, "case")]),
    )
    .await;
    assert_eq!(noop.status(), StatusCode::BAD_REQUEST);
    let bad_uom = send(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("replace-uom"),
        &replacement(1, [("changed", item_id, 4, "each")]),
    )
    .await;
    assert_eq!(bad_uom.status(), StatusCode::CONFLICT);

    let left_command = replacement(1, [("left", item_id, 5, "case")]);
    let right_command = replacement(1, [("right", item_id, 6, "case")]);
    let left = send(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("replace-left"),
        &left_command,
    );
    let right = send(
        &app,
        &token,
        tenant_id,
        order_id,
        Some("replace-right"),
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

    let processing_order = fixture
        .order_header(tenant_id, "LINE-REPLACE-PROCESSING", owner_id)
        .await;
    fixture
        .order_item(tenant_id, processing_order, item_id, 2)
        .await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("UPDATE orders SET status='processing' WHERE tenant_id=$1 AND id=$2")
        .bind(tenant_id.get())
        .bind(processing_order)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let processing = send(
        &app,
        &token,
        tenant_id,
        processing_order,
        Some("replace-processing"),
        &replacement(1, [("changed", item_id, 2, "case")]),
    )
    .await;
    assert_eq!(processing.status(), StatusCode::CONFLICT);

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let effects: (i64, i64, i64) = sqlx::query_as(
        r#"SELECT orders.revision,
                  (SELECT COUNT(*) FROM order_line_amendments amendment
                   WHERE amendment.tenant_id=orders.tenant_id
                     AND amendment.order_id=orders.id),
                  (SELECT COUNT(*) FROM order_items item
                   WHERE item.tenant_id=orders.tenant_id AND item.order_id=orders.id
                     AND item.deleted IS NULL)
           FROM orders WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(effects, (2, 1, 1));
    let processing_effects: (i64, i64) = sqlx::query_as(
        r#"SELECT revision,
                  (SELECT COUNT(*) FROM order_line_amendments amendment
                   WHERE amendment.tenant_id=orders.tenant_id
                     AND amendment.order_id=orders.id)
           FROM orders WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(processing_order)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(processing_effects, (1, 0));
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn scope_replay_rls_grants_and_evidence_fail_closed() {
    let fixture = Fixture::new().await;
    let operator = fixture.user("order-lines-scope@test.local").await;
    let no_permission = fixture.user("order-lines-no-permission@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let admin = admin_db_for(&fixture.db).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id,user_id) VALUES ($1,$2)")
        .bind(tenant_id.get())
        .bind(no_permission.id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    grant_orders(&fixture.db, tenant_id, operator.id, "order-lines-scope").await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let no_permission_token = auth::create_session(&fixture.db, no_permission.id)
        .await
        .unwrap();
    let allowed_owner = fixture
        .inventory_owner(tenant_id, "Allowed Line Client")
        .await;
    let denied_owner = fixture
        .inventory_owner(tenant_id, "Denied Line Client")
        .await;
    let item_id = fixture.item(tenant_id, "Scoped Line Item", "each").await;
    let allowed_order = fixture
        .order_header(tenant_id, "LINE-ALLOWED", allowed_owner)
        .await;
    fixture
        .order_item(tenant_id, allowed_order, item_id, 2)
        .await;
    let denied_order = fixture
        .order_header(tenant_id, "LINE-DENIED", denied_owner)
        .await;
    fixture
        .order_item(tenant_id, denied_order, item_id, 2)
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
    let command = replacement(1, [("new", item_id, 3, "each")]);

    let denied = send(
        &app,
        &token,
        tenant_id,
        denied_order,
        Some("replace-denied"),
        &command,
    )
    .await;
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    let forbidden = send(
        &app,
        &no_permission_token,
        tenant_id,
        allowed_order,
        Some("replace-forbidden"),
        &command,
    )
    .await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let success = send(
        &app,
        &token,
        tenant_id,
        allowed_order,
        Some("replace-visible"),
        &command,
    )
    .await;
    assert_eq!(success.status(), StatusCode::OK);
    let success: ReplaceFulfillmentOrderLinesResponse = json(success).await;

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
        Some("replace-visible"),
        &command,
    )
    .await;
    assert_eq!(concealed_replay.status(), StatusCode::NOT_FOUND);
    let concealed_changed_replay = send(
        &app,
        &token,
        tenant_id,
        allowed_order,
        Some("replace-visible"),
        &replacement(1, [("changed", item_id, 4, "each")]),
    )
    .await;
    assert_eq!(concealed_changed_replay.status(), StatusCode::NOT_FOUND);

    let app_db = app_db_for(&fixture.db).await;
    let other_user = fixture.user("order-lines-other-tenant@test.local").await;
    let other_tenant = tenant_for_user(&fixture.db, other_user.id).await;
    let mut other_tx = app_db.begin().await.unwrap();
    db::bind_tenant_context(&mut other_tx, other_tenant)
        .await
        .unwrap();
    let hidden: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM order_line_amendments), (SELECT COUNT(*) FROM order_line_amendment_lines)",
    )
    .fetch_one(&mut *other_tx)
    .await
    .unwrap();
    assert_eq!(hidden, (0, 0));
    other_tx.rollback().await.unwrap();
    app_db.close().await;

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    assert!(
        sqlx::query("UPDATE order_line_amendments SET amended_at=amended_at WHERE id=$1")
            .bind(success.amendment_id)
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    assert!(sqlx::query(
        "UPDATE order_line_amendment_lines SET qty=qty WHERE order_line_amendment_id=$1",
    )
    .bind(success.amendment_id)
    .execute(&mut *tx)
    .await
    .is_err());
    tx.rollback().await.unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    assert!(
        sqlx::query("UPDATE order_items SET qty=qty+1 WHERE tenant_id=$1 AND order_id=$2")
            .bind(tenant_id.get())
            .bind(allowed_order)
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let amended_at: wareboxes_domain::Timestamp = sqlx::query_scalar(
        "SELECT amended_at FROM order_line_amendments WHERE tenant_id=$1 AND id=$2",
    )
    .bind(tenant_id.get())
    .bind(success.amendment_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query("SELECT set_config('wareboxes.order_line_amendment_id',$1,true)")
        .bind(success.amendment_id.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO order_items
           (tenant_id,inventory_owner_id,created,line_key,line_number,qty,item_id,order_id,uom)
           VALUES ($1,$2,$3,'forged',99,1,$4,$5,'each')"#,
    )
    .bind(tenant_id.get())
    .bind(allowed_owner)
    .bind(amended_at)
    .bind(item_id)
    .bind(allowed_order)
    .execute(&mut *tx)
    .await
    .unwrap();
    assert!(tx.commit().await.is_err());

    let admin = admin_db_for(&fixture.db).await;
    let grants = sqlx::query(
        r#"SELECT
             has_table_privilege('wareboxes_app','order_line_amendments','SELECT') header_select,
             has_table_privilege('wareboxes_app','order_line_amendments','INSERT') header_insert,
             has_table_privilege('wareboxes_app','order_line_amendments','UPDATE') header_update,
             has_table_privilege('wareboxes_app','order_line_amendments','DELETE') header_delete,
             has_table_privilege('wareboxes_app','order_line_amendment_lines','SELECT') line_select,
             has_table_privilege('wareboxes_app','order_line_amendment_lines','INSERT') line_insert,
             has_table_privilege('wareboxes_app','order_line_amendment_lines','UPDATE') line_update,
             has_table_privilege('wareboxes_app','order_line_amendment_lines','DELETE') line_delete,
             has_sequence_privilege('wareboxes_app','order_line_amendments_id_seq','USAGE') header_sequence,
             has_sequence_privilege('wareboxes_app','order_line_amendment_lines_id_seq','USAGE') line_sequence,
             has_table_privilege('wareboxes_app','order_items','INSERT') item_insert,
             has_table_privilege('wareboxes_app','order_items','UPDATE') item_update,
             has_table_privilege('wareboxes_app','order_items','DELETE') item_delete,
             has_column_privilege('wareboxes_app','order_items','deleted','UPDATE') item_retire,
             has_column_privilege('wareboxes_app','order_items','qty','UPDATE') item_qty_update"#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    for column in [
        "header_select",
        "header_insert",
        "line_select",
        "line_insert",
        "header_sequence",
        "line_sequence",
        "item_insert",
        "item_retire",
    ] {
        assert!(grants.try_get::<bool, _>(column).unwrap(), "{column}");
    }
    for column in [
        "header_update",
        "header_delete",
        "line_update",
        "line_delete",
        "item_update",
        "item_delete",
        "item_qty_update",
    ] {
        assert!(!grants.try_get::<bool, _>(column).unwrap(), "{column}");
    }
    admin.close().await;
}
