use super::*;
use serde_json::json;
use wareboxes_domain::Timestamp;

struct ReleasedOrder {
    fixture: Fixture,
    app: axum::Router,
    token: String,
    tenant_id: TenantId,
    user_id: i64,
    order_id: i64,
    task_id: i64,
}

async fn released_order(key: &str) -> ReleasedOrder {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user(&format!("{key}@test.local")).await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant_orders(&fixture.db, tenant_id, user.id).await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let owner_id = fixture
        .inventory_owner(tenant_id, &format!("{key} Owner"))
        .await;
    let facility_id = fixture.facility(tenant_id, &format!("{key} DC")).await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;
    let item_id = fixture
        .item(tenant_id, &format!("{key} Item"), "each")
        .await;
    repo::items::add_barcode(
        &fixture.db,
        tenant_id,
        item_id,
        &format!("{key}-ITEM"),
        "code128",
        None,
    )
    .await
    .unwrap();
    let order_id = fixture.order_header(tenant_id, key, owner_id).await;
    fixture.order_item(tenant_id, order_id, item_id, 5).await;
    fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id,
                qty: 5,
                key,
            },
        )
        .await;
    let app = routes::app(AppState::new(fixture.db.clone()));
    let allocation = allocate(
        &app,
        &token,
        tenant_id,
        order_id,
        facility_id,
        &format!("{key}-allocate"),
    )
    .await;
    assert_eq!(allocation.revision.get(), 2);

    let destination_location_id = wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some(&format!("{key}-STAGE")),
        Some(&format!("{key} staging")),
        "staging",
        true,
        false,
        false,
    )
    .await
    .unwrap();
    let release = app
        .clone()
        .oneshot(api_request(
            &token,
            tenant_id,
            &format!("/api/v1/orders/{order_id}/releases"),
            Some(&format!("{key}-release")),
            &json!({
                "facility_id": facility_id,
                "destination_location_id": destination_location_id,
                "expected_revision": 2
            }),
        ))
        .await
        .unwrap();
    if release.status() != StatusCode::OK {
        panic!("release failed: {}", response_json::<Value>(release).await);
    }

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let task_id: i64 =
        sqlx::query_scalar("SELECT id FROM pick_tasks WHERE tenant_id = $1 AND order_id = $2")
            .bind(tenant_id.get())
            .bind(order_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.rollback().await.unwrap();

    ReleasedOrder {
        fixture,
        app,
        token,
        tenant_id,
        user_id: user.id,
        order_id,
        task_id,
    }
}

#[tokio::test]
async fn released_unclaimed_order_cancels_pick_work_and_commitments_without_inventory_movement() {
    let setup = released_order("CANCEL-RELEASED").await;
    let mut tx = tenant_tx(&setup.fixture.db, setup.tenant_id).await;
    let transaction_count_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM inventory_transactions WHERE tenant_id = $1")
            .bind(setup.tenant_id.get())
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.rollback().await.unwrap();

    let request = CancelOrderRequest {
        expected_revision: Revision::new(3).unwrap(),
        reason: OrderCancellationReason::ClientRequest,
        note: Some("Cancelled after release but before an operator started picking".into()),
    };
    let response = cancel(
        &setup.app,
        &setup.token,
        setup.tenant_id,
        setup.order_id,
        Some("cancel-released"),
        &request,
    )
    .await;
    if response.status() != StatusCode::OK {
        panic!(
            "released cancellation failed: {}",
            response_json::<Value>(response).await
        );
    }
    let response: CancelOrderResponse = response_json(response).await;
    assert_eq!(
        response.previous_status,
        OrderCancellationStatus::Processing
    );
    assert_eq!(response.status, OrderCancellationStatus::Cancelled);
    assert_eq!(response.revision.get(), 4);
    assert_eq!(response.cancelled_pick_task_count, 1);
    assert_eq!(response.cancelled_pick_content_count, 1);
    assert_eq!(response.released_reservation_count, 1);
    assert_eq!(response.released_allocation_count, 1);
    assert_eq!(response.released_quantity, 5);

    let replay = cancel(
        &setup.app,
        &setup.token,
        setup.tenant_id,
        setup.order_id,
        Some("cancel-released"),
        &request,
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(response_json::<CancelOrderResponse>(replay).await, response);

    let mut tx = tenant_tx(&setup.fixture.db, setup.tenant_id).await;
    let state = sqlx::query(
        r#"
        SELECT order_header.status, order_header.revision,
               cancellation.occurred_at,
               task.status AS task_status, task.completed_at AS task_completed_at,
               content.state AS content_state, content.completed_at AS content_completed_at,
               reservation.status AS reservation_status,
               allocation.status AS allocation_status,
               balance.qty_reserved,
               cancellation.cancelled_pick_task_count,
               cancellation.cancelled_pick_content_count,
               (SELECT COUNT(*) FROM inventory_transactions transaction
                WHERE transaction.tenant_id = order_header.tenant_id) AS transaction_count,
               (SELECT COUNT(*) FROM unpack_cancelled_order_tasks unpack
                WHERE unpack.tenant_id = order_header.tenant_id
                  AND unpack.order_id = order_header.id) AS unpack_count
        FROM orders order_header
        INNER JOIN order_cancellations cancellation
          ON cancellation.tenant_id = order_header.tenant_id
         AND cancellation.order_id = order_header.id
        INNER JOIN pick_tasks task
          ON task.tenant_id = order_header.tenant_id
         AND task.order_id = order_header.id
        INNER JOIN pick_task_contents content
          ON content.tenant_id = task.tenant_id AND content.task_id = task.id
        INNER JOIN inventory_reservations reservation
          ON reservation.tenant_id = order_header.tenant_id
         AND reservation.order_id = order_header.id
        INNER JOIN inventory_allocations allocation
          ON allocation.tenant_id = reservation.tenant_id
         AND allocation.reservation_id = reservation.id
        INNER JOIN inventory_balances balance
          ON balance.tenant_id = allocation.tenant_id
         AND balance.id = allocation.inventory_balance_id
        WHERE order_header.tenant_id = $1 AND order_header.id = $2
        "#,
    )
    .bind(setup.tenant_id.get())
    .bind(setup.order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(state.try_get::<String, _>("status").unwrap(), "cancelled");
    assert_eq!(state.try_get::<i64, _>("revision").unwrap(), 4);
    assert_eq!(
        state.try_get::<String, _>("task_status").unwrap(),
        "cancelled"
    );
    assert_eq!(
        state.try_get::<String, _>("content_state").unwrap(),
        "cancelled"
    );
    assert_eq!(
        state.try_get::<Timestamp, _>("task_completed_at").unwrap(),
        state.try_get::<Timestamp, _>("occurred_at").unwrap()
    );
    assert_eq!(
        state
            .try_get::<Timestamp, _>("content_completed_at")
            .unwrap(),
        state.try_get::<Timestamp, _>("occurred_at").unwrap()
    );
    assert_eq!(
        state.try_get::<String, _>("reservation_status").unwrap(),
        "cancelled"
    );
    assert_eq!(
        state.try_get::<String, _>("allocation_status").unwrap(),
        "released"
    );
    assert_eq!(state.try_get::<i64, _>("qty_reserved").unwrap(), 0);
    assert_eq!(
        state.try_get::<i64, _>("transaction_count").unwrap(),
        transaction_count_before
    );
    assert_eq!(state.try_get::<i64, _>("unpack_count").unwrap(), 0);

    let event: Value = sqlx::query_scalar(
        r#"
        SELECT payload FROM outbox_events
        WHERE tenant_id = $1 AND aggregate_type = 'order'
          AND aggregate_id = $2::TEXT AND event_type = 'order.cancelled'
        "#,
    )
    .bind(setup.tenant_id.get())
    .bind(setup.order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(event["cancelled_pick_task_count"], 1);
    assert_eq!(event["cancelled_pick_content_count"], 1);
    assert_eq!(event["released_quantity"], 5);
}

#[tokio::test]
async fn claim_and_cancellation_race_has_exactly_one_winner() {
    let setup = released_order("CANCEL-CLAIM-RACE").await;
    let claim_path = format!("/api/v1/picking-claims/{}", setup.task_id);
    let claim = setup.app.clone().oneshot(api_request(
        &setup.token,
        setup.tenant_id,
        &claim_path,
        Some("claim-race"),
        &json!({}),
    ));
    let cancellation_request = cancellation_request(3);
    let cancellation = cancel(
        &setup.app,
        &setup.token,
        setup.tenant_id,
        setup.order_id,
        Some("cancel-claim-race"),
        &cancellation_request,
    );
    let (claim, cancellation) = tokio::join!(claim, cancellation);
    let claim = claim.unwrap();
    match (claim.status(), cancellation.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) | (StatusCode::CONFLICT, StatusCode::OK) => {}
        statuses => panic!("expected exactly one race winner, got {statuses:?}"),
    }

    let mut tx = tenant_tx(&setup.fixture.db, setup.tenant_id).await;
    let state = sqlx::query(
        r#"
        SELECT order_header.status, order_header.revision,
               task.status AS task_status, task.assigned_user_id,
               (SELECT COUNT(*) FROM order_cancellations cancellation
                WHERE cancellation.tenant_id = order_header.tenant_id
                  AND cancellation.order_id = order_header.id) AS cancellation_count
        FROM orders order_header
        INNER JOIN pick_tasks task
          ON task.tenant_id = order_header.tenant_id
         AND task.order_id = order_header.id
        WHERE order_header.tenant_id = $1 AND order_header.id = $2
        "#,
    )
    .bind(setup.tenant_id.get())
    .bind(setup.order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    let cancellation_count = state.try_get::<i64, _>("cancellation_count").unwrap();
    if cancellation.status() == StatusCode::OK {
        assert_eq!(state.try_get::<String, _>("status").unwrap(), "cancelled");
        assert_eq!(
            state.try_get::<String, _>("task_status").unwrap(),
            "cancelled"
        );
        assert!(state
            .try_get::<Option<i64>, _>("assigned_user_id")
            .unwrap()
            .is_none());
        assert_eq!(cancellation_count, 1);
    } else {
        assert_eq!(state.try_get::<String, _>("status").unwrap(), "processing");
        assert_eq!(
            state.try_get::<String, _>("task_status").unwrap(),
            "in_progress"
        );
        assert_eq!(
            state.try_get::<Option<i64>, _>("assigned_user_id").unwrap(),
            Some(setup.user_id)
        );
        assert_eq!(cancellation_count, 0);
    }
}

#[tokio::test]
async fn claimed_work_rejects_cancellation_and_direct_terminal_forgery() {
    let setup = released_order("CANCEL-CLAIMED").await;

    let admin_db = admin_db_for(&setup.fixture.db).await;
    let mut forged = admin_db.begin().await.unwrap();
    let forged_at = db::now_iso();
    sqlx::query(
        r#"
        UPDATE pick_task_contents
        SET state = 'cancelled', completed_at = $1
        WHERE tenant_id = $2 AND order_id = $3
        "#,
    )
    .bind(forged_at)
    .bind(setup.tenant_id.get())
    .bind(setup.order_id)
    .execute(&mut *forged)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE pick_tasks
        SET status = 'cancelled', completed_at = $1
        WHERE tenant_id = $2 AND id = $3
        "#,
    )
    .bind(forged_at)
    .bind(setup.tenant_id.get())
    .bind(setup.task_id)
    .execute(&mut *forged)
    .await
    .unwrap();
    assert!(forged.commit().await.is_err());
    admin_db.close().await;

    let claimed = setup
        .app
        .clone()
        .oneshot(api_request(
            &setup.token,
            setup.tenant_id,
            &format!("/api/v1/picking-claims/{}", setup.task_id),
            Some("claim-before-cancel"),
            &json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(claimed.status(), StatusCode::OK);

    let cancellation = cancel(
        &setup.app,
        &setup.token,
        setup.tenant_id,
        setup.order_id,
        Some("cancel-after-claim"),
        &cancellation_request(3),
    )
    .await;
    assert_eq!(cancellation.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(cancellation).await.reason,
        ErrorReason::Conflict
    );
    assert_no_cancellation_effects(&setup.fixture.db, setup.tenant_id, &[setup.order_id]).await;
}
