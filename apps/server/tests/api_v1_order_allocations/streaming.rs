use super::*;

async fn staging_location(
    fixture: &Fixture,
    tenant_id: TenantId,
    facility_id: i64,
    barcode: &str,
) -> i64 {
    wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some(barcode),
        Some(barcode),
        "staging",
        true,
        false,
        false,
    )
    .await
    .unwrap()
}

fn stream_request(
    facility_id: i64,
    destination_location_id: i64,
    revision: i64,
) -> StreamOrderRequest {
    StreamOrderRequest {
        facility_id,
        destination_location_id,
        expected_revision: Revision::new(revision).unwrap(),
        expected_allocation_policy:
            wareboxes_api_contract::v1::AllocationPolicyReference::product_default(),
    }
}

async fn stream(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    order_id: i64,
    key: &str,
    request: &StreamOrderRequest,
) -> axum::response::Response {
    app.clone()
        .oneshot(api_request(
            token,
            tenant_id,
            Method::POST,
            &format!("/api/v1/orders/{order_id}/streams"),
            Some(key),
            Some(request),
        ))
        .await
        .unwrap()
}

#[tokio::test]
async fn stream_allocates_and_releases_once_with_exact_concurrent_replay() {
    let fixture = Fixture::new().await;
    let user = fixture.user("order-stream@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant_orders(&fixture.db, tenant_id, user.id).await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let owner_id = fixture.inventory_owner(tenant_id, "Stream Client").await;
    let other_owner_id = fixture
        .inventory_owner(tenant_id, "Other Stream Client")
        .await;
    let facility_id = fixture.facility(tenant_id, "Stream DC").await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;
    let destination_id = staging_location(&fixture, tenant_id, facility_id, "STREAM-STAGE").await;
    let other_destination_id =
        staging_location(&fixture, tenant_id, facility_id, "STREAM-STAGE-2").await;
    let item_id = fixture.item(tenant_id, "Stream Item", "each").await;
    repo::items::add_barcode(
        &fixture.db,
        tenant_id,
        item_id,
        "STREAM-ITEM",
        "code128",
        None,
    )
    .await
    .unwrap();
    let order_id = fixture
        .order_header(tenant_id, "STREAM-ORDER-1", owner_id)
        .await;
    fixture.order_item(tenant_id, order_id, item_id, 5).await;
    fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id,
                qty: 5,
                key: "STREAM-STOCK",
            },
        )
        .await;

    let request = stream_request(facility_id, destination_id, 1);
    let first = stream(
        &app,
        &token,
        tenant_id,
        order_id,
        "stream-order-once",
        &request,
    );
    let retry = stream(
        &app,
        &token,
        tenant_id,
        order_id,
        "stream-order-once",
        &request,
    );
    let (first, retry) = tokio::join!(first, retry);
    if first.status() != StatusCode::OK {
        panic!(
            "first stream failed: {}",
            response_json::<Value>(first).await
        );
    }
    if retry.status() != StatusCode::OK {
        panic!(
            "stream retry failed: {}",
            response_json::<Value>(retry).await
        );
    }
    let result: StreamOrderResponse = response_json(first).await;
    assert_eq!(response_json::<StreamOrderResponse>(retry).await, result);
    assert_eq!(
        result.allocation.outcome,
        OrderAllocationOutcome::FullyAllocated
    );
    assert_eq!(result.allocation.revision.get(), 2);
    assert_eq!(result.release.revision.get(), 3);
    assert_eq!(result.release.released_quantity, 5);
    assert_eq!(result.release.pick_task_count, 1);

    let changed = stream(
        &app,
        &token,
        tenant_id,
        order_id,
        "stream-order-once",
        &stream_request(facility_id, other_destination_id, 1),
    )
    .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let effects: (String, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"SELECT orders.status,orders.revision,
          (SELECT count(*) FROM order_allocation_runs run
           WHERE run.tenant_id=orders.tenant_id AND run.order_id=orders.id),
          (SELECT count(*) FROM order_releases release
           WHERE release.tenant_id=orders.tenant_id AND release.order_id=orders.id),
          (SELECT count(*) FROM pick_tasks task
           WHERE task.tenant_id=orders.tenant_id AND task.order_id=orders.id),
          (SELECT count(*) FROM command_idempotency_records command
           WHERE command.tenant_id=orders.tenant_id AND command.operation='order.stream.v1'
             AND command.idempotency_key='stream-order-once'),
          (SELECT count(*) FROM outbox_events event
           WHERE event.tenant_id=orders.tenant_id AND event.aggregate_id=orders.id::text
             AND event.event_type IN ('order.allocation.planned','order.released'))
        FROM orders WHERE orders.tenant_id=$1 AND orders.id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(effects, ("processing".into(), 3, 1, 1, 1, 1, 2));

    repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: user.id,
            all_facilities: true,
            facility_ids: Vec::new(),
            all_inventory_owners: false,
            inventory_owner_ids: vec![other_owner_id],
        },
    )
    .await
    .unwrap();
    let concealed_replay = stream(
        &app,
        &token,
        tenant_id,
        order_id,
        "stream-order-once",
        &request,
    )
    .await;
    assert_eq!(concealed_replay.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn stream_shortage_and_concurrent_commands_roll_back_or_choose_one_winner() {
    let fixture = Fixture::new().await;
    let user = fixture.user("order-stream-race@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant_orders(&fixture.db, tenant_id, user.id).await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let owner_id = fixture
        .inventory_owner(tenant_id, "Stream Race Client")
        .await;
    let facility_id = fixture.facility(tenant_id, "Stream Race DC").await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;
    let destination_id =
        staging_location(&fixture, tenant_id, facility_id, "STREAM-RACE-STAGE").await;
    let item_id = fixture.item(tenant_id, "Stream Race Item", "each").await;
    repo::items::add_barcode(
        &fixture.db,
        tenant_id,
        item_id,
        "STREAM-RACE-ITEM",
        "code128",
        None,
    )
    .await
    .unwrap();

    let short_order_id = fixture
        .order_header(tenant_id, "STREAM-SHORT", owner_id)
        .await;
    fixture
        .order_item(tenant_id, short_order_id, item_id, 8)
        .await;
    fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id,
                qty: 3,
                key: "STREAM-SHORT-STOCK",
            },
        )
        .await;
    let shortage = stream(
        &app,
        &token,
        tenant_id,
        short_order_id,
        "stream-shortage",
        &stream_request(facility_id, destination_id, 1),
    )
    .await;
    assert_eq!(shortage.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(shortage).await.message,
        "order streaming requires every demand line to allocate; no allocation was committed"
    );

    let race_order_id = fixture
        .order_header(tenant_id, "STREAM-RACE", owner_id)
        .await;
    fixture
        .order_item(tenant_id, race_order_id, item_id, 4)
        .await;
    fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id,
                qty: 4,
                key: "STREAM-RACE-STOCK",
            },
        )
        .await;
    let request = stream_request(facility_id, destination_id, 1);
    let first = stream(
        &app,
        &token,
        tenant_id,
        race_order_id,
        "stream-race-a",
        &request,
    );
    let second = stream(
        &app,
        &token,
        tenant_id,
        race_order_id,
        "stream-race-b",
        &request,
    );
    let (first, second) = tokio::join!(first, second);
    let statuses = (first.status(), second.status());
    if !matches!(
        statuses,
        (StatusCode::OK, StatusCode::CONFLICT) | (StatusCode::CONFLICT, StatusCode::OK)
    ) {
        panic!(
            "unexpected race statuses {statuses:?}: first={}, second={}",
            response_json::<Value>(first).await,
            response_json::<Value>(second).await
        );
    }

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let effects: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"SELECT
          (SELECT revision FROM orders WHERE tenant_id=$1 AND id=$2),
          (SELECT count(*) FROM order_allocation_runs WHERE tenant_id=$1 AND order_id=$2),
          (SELECT count(*) FROM order_releases WHERE tenant_id=$1 AND order_id=$2),
          (SELECT count(*) FROM order_allocation_runs WHERE tenant_id=$1 AND order_id=$3),
          (SELECT count(*) FROM inventory_reservations WHERE tenant_id=$1 AND order_id=$3),
          (SELECT count(*) FROM command_idempotency_records
           WHERE tenant_id=$1 AND operation='order.stream.v1'
             AND idempotency_key='stream-shortage')"#,
    )
    .bind(tenant_id.get())
    .bind(race_order_id)
    .bind(short_order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(effects, (3, 1, 1, 0, 0, 0));
}
