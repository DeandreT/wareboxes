use super::reversal_support::{completed_pick, reversal_body, send};
use super::*;
use serde_json::json;
use wareboxes_api_contract::v1::{
    PickClaimResponse, PickContentConfirmationResponse, ReleaseOrderResponse,
    ReversePickConfirmationResponse,
};

#[tokio::test]
async fn fully_reversed_pick_can_be_cancelled_and_its_empty_tote_reused() {
    let fixture = Fixture::new().await;
    let supervisor = fixture
        .wms_user("reversed-cancellation-supervisor@test.local")
        .await;
    let access = default_tenant_for_user(&fixture.db, supervisor.id)
        .await
        .unwrap();
    grant_orders(&fixture.db, access.tenant_id, supervisor.id).await;
    super::reversal_support::grant_permission(
        &fixture,
        access.tenant_id,
        supervisor.id,
        "wms_supervisor",
        "reversed-cancellation-supervisor",
    )
    .await;
    let token = auth::create_session(&fixture.db, supervisor.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let picked = completed_pick(&fixture, &app, &token, &access, "CANCEL-REVERSED").await;

    let before_reversal = cancel(
        &app,
        &token,
        access.tenant_id,
        picked.order_id,
        Some("cancel-active-pick"),
        &cancellation_request(4),
    )
    .await;
    assert_eq!(before_reversal.status(), StatusCode::CONFLICT);

    let reversal_path = format!(
        "/api/v1/pick-confirmations/{}/reversals",
        picked.confirmation.result_id
    );
    let reversal = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &reversal_path,
        Some("cancel-after-reversal-reverse"),
        Some(reversal_body(&picked, 4)),
    )
    .await;
    assert_eq!(reversal.status(), StatusCode::OK);
    let reversal: ReversePickConfirmationResponse = response_json(reversal).await;
    assert_eq!(reversal.order_revision.get(), 5);

    let repick_claim = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/picking-claims/next",
        Some("cancel-after-reversal-repick-claim"),
        Some(json!({})),
    )
    .await;
    let repick_claim: PickClaimResponse = response_json::<Option<PickClaimResponse>>(repick_claim)
        .await
        .unwrap();
    let repick = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/picking-tasks/{}/contents/{}/confirmations",
            repick_claim.task_id, repick_claim.content.content_id
        ),
        Some("cancel-after-reversal-repick"),
        Some(json!({
            "source_location_barcode": repick_claim.content.source_location_barcode,
            "item_barcode": repick_claim.content.item_barcodes[0],
            "destination_license_plate_barcode": picked.tote_barcode
        })),
    )
    .await;
    assert_eq!(repick.status(), StatusCode::OK);
    let repick: PickContentConfirmationResponse = response_json(repick).await;
    assert_eq!(repick.order_revision.get(), 6);

    let active_repick_cancellation = cancel(
        &app,
        &token,
        access.tenant_id,
        picked.order_id,
        Some("cancel-active-repick"),
        &cancellation_request(6),
    )
    .await;
    assert_eq!(active_repick_cancellation.status(), StatusCode::CONFLICT);

    let second_reversal = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/pick-confirmations/{}/reversals", repick.result_id),
        Some("cancel-after-reversal-reverse-again"),
        Some(reversal_body(&picked, 6)),
    )
    .await;
    assert_eq!(second_reversal.status(), StatusCode::OK);
    let second_reversal: ReversePickConfirmationResponse = response_json(second_reversal).await;
    assert_eq!(second_reversal.order_revision.get(), 7);

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let journal_count_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM inventory_transactions WHERE tenant_id = $1")
            .bind(access.tenant_id.get())
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.rollback().await.unwrap();

    let request = CancelOrderRequest {
        expected_revision: Revision::new(7).unwrap(),
        reason: OrderCancellationReason::ClientRequest,
        note: Some("Client cancelled after the pick was physically returned".into()),
    };
    let response = cancel(
        &app,
        &token,
        access.tenant_id,
        picked.order_id,
        Some("cancel-after-reversal"),
        &request,
    )
    .await;
    if response.status() != StatusCode::OK {
        panic!(
            "reversed cancellation failed: {}",
            response_json::<Value>(response).await
        );
    }
    let cancelled: CancelOrderResponse = response_json(response).await;
    assert_eq!(cancelled.revision.get(), 8);
    assert_eq!(cancelled.reversed_pick_confirmation_count, 2);
    assert_eq!(cancelled.released_outbound_container_count, 1);
    assert_eq!(cancelled.cancelled_pick_task_count, 1);
    assert_eq!(cancelled.cancelled_pick_content_count, 1);
    assert_eq!(cancelled.released_reservation_count, 1);
    assert_eq!(cancelled.released_allocation_count, 1);
    assert_eq!(cancelled.released_quantity, 4);

    let replay = cancel(
        &app,
        &token,
        access.tenant_id,
        picked.order_id,
        Some("cancel-after-reversal"),
        &request,
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<CancelOrderResponse>(replay).await,
        cancelled
    );

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let state = sqlx::query(
        r#"
        SELECT order_header.status, order_header.revision,
               task.status AS task_status, content.state AS content_state,
               source.status AS source_status, source.deleted AS source_deleted,
               reservation.status AS reservation_status,
               container.released_at, container.released_by_user_id,
               container.release_order_cancellation_id,
               (SELECT COUNT(*) FROM inventory_transactions transaction
                WHERE transaction.tenant_id = order_header.tenant_id) AS journal_count,
               (SELECT COUNT(*) FROM outbox_events event
                WHERE event.tenant_id = order_header.tenant_id
                  AND event.event_type = 'order.cancelled'
                  AND event.aggregate_id = order_header.id::TEXT) AS event_count
        FROM orders order_header
        INNER JOIN pick_tasks task
          ON task.tenant_id = order_header.tenant_id AND task.order_id = order_header.id
        INNER JOIN pick_task_contents content
          ON content.tenant_id = task.tenant_id AND content.task_id = task.id
        INNER JOIN inventory_reservations reservation
          ON reservation.tenant_id = order_header.tenant_id
         AND reservation.order_id = order_header.id
        INNER JOIN inventory_allocations source
          ON source.tenant_id = reservation.tenant_id
         AND source.reservation_id = reservation.id
         AND source.execution_stage = 'pick_source'
        INNER JOIN outbound_order_containers container
          ON container.tenant_id = order_header.tenant_id
         AND container.order_id = order_header.id
        WHERE order_header.tenant_id = $1 AND order_header.id = $2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(picked.order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(state.try_get::<String, _>("status").unwrap(), "cancelled");
    assert_eq!(state.try_get::<i64, _>("revision").unwrap(), 8);
    assert_eq!(
        state.try_get::<String, _>("task_status").unwrap(),
        "cancelled"
    );
    assert_eq!(
        state.try_get::<String, _>("content_state").unwrap(),
        "cancelled"
    );
    assert_eq!(
        state.try_get::<String, _>("source_status").unwrap(),
        "released"
    );
    assert!(state
        .try_get::<Option<wareboxes_domain::Timestamp>, _>("source_deleted")
        .unwrap()
        .is_some());
    assert_eq!(
        state.try_get::<String, _>("reservation_status").unwrap(),
        "cancelled"
    );
    assert!(state
        .try_get::<Option<wareboxes_domain::Timestamp>, _>("released_at")
        .unwrap()
        .is_some());
    assert_eq!(
        state
            .try_get::<Option<i64>, _>("released_by_user_id")
            .unwrap(),
        Some(supervisor.id)
    );
    assert_eq!(
        state
            .try_get::<Option<i64>, _>("release_order_cancellation_id")
            .unwrap(),
        Some(cancelled.cancellation_id)
    );
    assert_eq!(
        state.try_get::<i64, _>("journal_count").unwrap(),
        journal_count_before
    );
    assert_eq!(state.try_get::<i64, _>("event_count").unwrap(), 1);
    tx.rollback().await.unwrap();

    let admin = admin_db_for(&fixture.db).await;
    let released_container_id: i64 = sqlx::query_scalar(
        "SELECT id FROM outbound_order_containers WHERE tenant_id = $1 AND order_id = $2",
    )
    .bind(access.tenant_id.get())
    .bind(picked.order_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    let mutation =
        sqlx::query("UPDATE outbound_order_containers SET released_at = released_at WHERE id = $1")
            .bind(released_container_id)
            .execute(&admin)
            .await
            .unwrap_err();
    assert_eq!(
        mutation.as_database_error().unwrap().code().as_deref(),
        Some("55000")
    );
    admin.close().await;

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let (owner_id, item_id): (i64, i64) = sqlx::query_as(
        r#"
        SELECT order_header.inventory_owner_id, item.item_id
        FROM orders order_header
        INNER JOIN order_items item
          ON item.tenant_id = order_header.tenant_id
         AND item.inventory_owner_id = order_header.inventory_owner_id
         AND item.order_id = order_header.id
        WHERE order_header.tenant_id = $1 AND order_header.id = $2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(picked.order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    let next_order_id = fixture
        .order_header(access.tenant_id, "CANCEL-REVERSED-REUSE", owner_id)
        .await;
    fixture
        .order_item(access.tenant_id, next_order_id, item_id, 3)
        .await;
    let allocation = allocate(
        &app,
        &token,
        access.tenant_id,
        next_order_id,
        picked.facility_id,
        "cancel-reversed-reuse-allocate",
    )
    .await;
    assert_eq!(allocation.revision.get(), 2);
    let release = app
        .clone()
        .oneshot(api_request(
            &token,
            access.tenant_id,
            &format!("/api/v1/orders/{next_order_id}/releases"),
            Some("cancel-reversed-reuse-release"),
            &json!({
                "facility_id": picked.facility_id,
                "destination_location_id": picked.execution_location_id,
                "expected_revision": 2
            }),
        ))
        .await
        .unwrap();
    let _: ReleaseOrderResponse = response_json(release).await;
    let claim = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/picking-claims/next",
        Some("cancel-reversed-reuse-claim"),
        Some(json!({})),
    )
    .await;
    let claim: PickClaimResponse = response_json::<Option<PickClaimResponse>>(claim)
        .await
        .unwrap();
    let confirmation = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/picking-tasks/{}/contents/{}/confirmations",
            claim.task_id, claim.content.content_id
        ),
        Some("cancel-reversed-reuse-confirm"),
        Some(json!({
            "source_location_barcode": claim.content.source_location_barcode,
            "item_barcode": claim.content.item_barcodes[0],
            "destination_license_plate_barcode": picked.tote_barcode
        })),
    )
    .await;
    assert_eq!(confirmation.status(), StatusCode::OK);
    let confirmation: PickContentConfirmationResponse = response_json(confirmation).await;
    assert_eq!(confirmation.picked_quantity, 3);

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let assignments: (i64, i64) = sqlx::query_as(
        r#"
        SELECT COUNT(*), COUNT(*) FILTER (WHERE released_at IS NULL)
        FROM outbound_order_containers container
        INNER JOIN license_plates plate
          ON plate.tenant_id = container.tenant_id
         AND plate.inventory_owner_id = container.inventory_owner_id
         AND plate.facility_id = container.facility_id
         AND plate.id = container.license_plate_id
        WHERE container.tenant_id = $1 AND plate.barcode = $2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(&picked.tote_barcode)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(assignments, (2, 1));
}
