use super::*;
use wareboxes_api_contract::v1::{CancelPurchaseOrderResponse, PurchaseOrderCancellationReason};

fn cancellation_body(expected_revision: i64, reason: &str, note: Option<&str>) -> Value {
    let mut body = json!({
        "expected_revision": expected_revision,
        "reason": reason,
    });
    if let Some(note) = note {
        body["note"] = json!(note);
    }
    body
}

async fn cancel_order(
    context: &PurchaseOrderFixture,
    purchase_order_id: i64,
    key: &str,
    body: &Value,
) -> axum::response::Response {
    routes::app(AppState::new(context.fixture.db.clone()))
        .oneshot(command_request(
            context,
            &format!("purchase-orders/{purchase_order_id}/cancellations"),
            key,
            body,
        ))
        .await
        .unwrap()
}

#[tokio::test]
async fn draft_cancellation_is_replayable_audited_and_inventory_neutral() {
    let context = fixture("purchase-order-cancel-draft@test.local").await;
    let order = create_order(&context, "PO-CANCEL-DRAFT").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));

    let before = app
        .clone()
        .oneshot(get_request(
            &context,
            &format!("purchase-orders/{}", order.purchase_order_id),
        ))
        .await
        .unwrap();
    let before = json_body::<PurchaseOrderDetailResponse>(before).await;
    assert!(before.summary.cancellation_ready);
    assert_eq!(before.summary.status, PurchaseOrderStatus::Draft);

    let missing_note = cancel_order(
        &context,
        order.purchase_order_id,
        "cancel-draft-missing-note",
        &cancellation_body(1, "other", None),
    )
    .await;
    assert_eq!(missing_note.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let body = cancellation_body(
        1,
        "demand_cancelled",
        Some("Customer withdrew the inbound demand"),
    );
    let cancelled = cancel_order(&context, order.purchase_order_id, "cancel-draft", &body).await;
    assert_eq!(cancelled.status(), StatusCode::OK);
    let cancelled = json_body::<CancelPurchaseOrderResponse>(cancelled).await;
    assert_eq!(cancelled.purchase_order_id, order.purchase_order_id);
    assert_eq!(cancelled.previous_status, PurchaseOrderStatus::Draft);
    assert_eq!(cancelled.status, PurchaseOrderStatus::Cancelled);
    assert_eq!(cancelled.revision.get(), 2);
    assert_eq!(
        cancelled.reason,
        PurchaseOrderCancellationReason::DemandCancelled
    );

    let replay = cancel_order(&context, order.purchase_order_id, "cancel-draft", &body).await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        json_body::<CancelPurchaseOrderResponse>(replay).await,
        cancelled
    );
    let changed = cancel_order(
        &context,
        order.purchase_order_id,
        "cancel-draft",
        &cancellation_body(1, "duplicate_order", None),
    )
    .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let detail = app
        .oneshot(get_request(
            &context,
            &format!("purchase-orders/{}", order.purchase_order_id),
        ))
        .await
        .unwrap();
    let detail = json_body::<PurchaseOrderDetailResponse>(detail).await;
    assert_eq!(detail.summary.status, PurchaseOrderStatus::Cancelled);
    assert_eq!(detail.summary.revision.get(), 2);
    assert!(!detail.summary.cancellation_ready);
    assert_eq!(
        detail.summary.cancellation_id,
        Some(cancelled.cancellation_id)
    );
    assert_eq!(detail.summary.total_active_inbound_quantity, 0);
    assert_eq!(detail.summary.total_available_to_notify_quantity, 0);
    assert_eq!(detail.summary.total_open_receipt_quantity, 0);
    assert!(detail.lines.iter().all(|line| {
        line.active_inbound_quantity == 0
            && line.available_to_notify_quantity == 0
            && line.open_receipt_quantity == 0
    }));

    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let effects: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM purchase_order_cancellations WHERE purchase_order_id=$1),
          (SELECT COUNT(*) FROM purchase_order_releases WHERE purchase_order_id=$1),
          (SELECT COUNT(*) FROM purchase_order_asn_sources WHERE purchase_order_id=$1),
          (SELECT COUNT(*) FROM outbox_events
             WHERE aggregate_type='purchase_order' AND aggregate_id=$1::TEXT
               AND aggregate_sequence=2
               AND event_type='inbound.purchase_order.cancelled'),
          (SELECT COUNT(*) FROM command_idempotency_records
             WHERE operation='inbound.purchase_order.cancel.v1'
               AND (result_json->>'purchase_order_id')::BIGINT=$1)
        "#,
    )
    .bind(order.purchase_order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(effects, (1, 0, 0, 1, 1));
    let payload: Value = sqlx::query_scalar(
        "SELECT payload FROM outbox_events WHERE aggregate_type='purchase_order' AND aggregate_id=$1::TEXT AND event_type='inbound.purchase_order.cancelled'",
    )
    .bind(order.purchase_order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(payload["status"], "cancelled");
    assert_eq!(payload["revision"], 2);
    assert_eq!(payload["reason"], "demand_cancelled");
    assert_eq!(payload["note"], "Customer withdrew the inbound demand");
    let immutable = sqlx::query(
        "UPDATE purchase_order_cancellations SET reason_code='duplicate_order' WHERE purchase_order_id=$1",
    )
    .bind(order.purchase_order_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(!immutable.to_string().is_empty());
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn released_cancellation_requires_every_source_notice_to_be_cancelled() {
    let context = fixture("purchase-order-cancel-released@test.local").await;
    let order = create_order(&context, "PO-CANCEL-RELEASED").await;
    let release = release_order(&context, &order, "release-po-cancel-released").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let asn = app
        .clone()
        .oneshot(command_request(
            &context,
            &format!("purchase-orders/{}/asns", order.purchase_order_id),
            "source-po-cancel-released",
            &asn_body(&order, "ASN-PO-CANCEL-RELEASED", 12, 8),
        ))
        .await
        .unwrap();
    assert_eq!(asn.status(), StatusCode::OK);
    let asn = json_body::<CreatePurchaseOrderAsnResponse>(asn).await;

    let body = cancellation_body(
        release.revision.get(),
        "supplier_cancelled",
        Some("Supplier cancelled all remaining dispatches"),
    );
    let blocked = cancel_order(
        &context,
        order.purchase_order_id,
        "cancel-released-blocked",
        &body,
    )
    .await;
    assert_eq!(blocked.status(), StatusCode::CONFLICT);
    let blocked_detail = app
        .clone()
        .oneshot(get_request(
            &context,
            &format!("purchase-orders/{}", order.purchase_order_id),
        ))
        .await
        .unwrap();
    let blocked_detail = json_body::<PurchaseOrderDetailResponse>(blocked_detail).await;
    assert!(!blocked_detail.summary.cancellation_ready);
    assert_eq!(blocked_detail.summary.status, PurchaseOrderStatus::Released);
    assert_eq!(blocked_detail.summary.revision.get(), 2);

    let asn_cancel = app
        .clone()
        .oneshot(command_request(
            &context,
            &format!("inbound-asns/{}/cancellations", asn.asn_id),
            "cancel-source-before-order",
            &json!({
                "expected_revision": asn.revision.get(),
                "reason": "supplier_cancelled",
                "note": "Supplier cancelled this notice"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(asn_cancel.status(), StatusCode::OK);

    let ready_detail = app
        .clone()
        .oneshot(get_request(
            &context,
            &format!("purchase-orders/{}", order.purchase_order_id),
        ))
        .await
        .unwrap();
    assert!(
        json_body::<PurchaseOrderDetailResponse>(ready_detail)
            .await
            .summary
            .cancellation_ready
    );
    let cancelled = cancel_order(&context, order.purchase_order_id, "cancel-released", &body).await;
    assert_eq!(cancelled.status(), StatusCode::OK);
    let cancelled = json_body::<CancelPurchaseOrderResponse>(cancelled).await;
    assert_eq!(cancelled.previous_status, PurchaseOrderStatus::Released);
    assert_eq!(cancelled.status, PurchaseOrderStatus::Cancelled);
    assert_eq!(cancelled.revision.get(), 3);

    let replacement = app
        .clone()
        .oneshot(command_request(
            &context,
            &format!("purchase-orders/{}/asns", order.purchase_order_id),
            "source-after-po-cancel",
            &asn_body(&order, "ASN-AFTER-PO-CANCEL", 12, 8),
        ))
        .await
        .unwrap();
    assert_eq!(replacement.status(), StatusCode::CONFLICT);

    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let evidence: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM purchase_order_releases WHERE purchase_order_id=$1),
          (SELECT COUNT(*) FROM purchase_order_cancellations WHERE purchase_order_id=$1),
          (SELECT COUNT(*) FROM purchase_order_asn_sources WHERE purchase_order_id=$1),
          (SELECT COUNT(*) FROM inbound_asn_cancellations WHERE asn_id=$2)
        "#,
    )
    .bind(order.purchase_order_id)
    .bind(asn.asn_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(evidence, (1, 1, 1, 1));
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn cancellation_and_source_notice_creation_are_serialized() {
    let context = fixture("purchase-order-cancel-race@test.local").await;
    let order = create_order(&context, "PO-CANCEL-RACE").await;
    release_order(&context, &order, "release-po-cancel-race").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let cancel = app.clone().oneshot(command_request(
        &context,
        &format!("purchase-orders/{}/cancellations", order.purchase_order_id),
        "cancel-po-race",
        &cancellation_body(2, "duplicate_order", None),
    ));
    let source = app.clone().oneshot(command_request(
        &context,
        &format!("purchase-orders/{}/asns", order.purchase_order_id),
        "source-po-race",
        &asn_body(&order, "ASN-PO-CANCEL-RACE", 12, 8),
    ));
    let (cancel, source) = tokio::join!(cancel, source);
    let cancel = cancel.unwrap();
    let source = source.unwrap();
    assert_eq!(
        [cancel.status(), source.status()]
            .into_iter()
            .filter(|status| *status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        [cancel.status(), source.status()]
            .into_iter()
            .filter(|status| *status == StatusCode::CONFLICT)
            .count(),
        1
    );

    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let state: (String, i64, i64) = sqlx::query_as(
        r#"
        SELECT purchase.status,
               (SELECT COUNT(*) FROM purchase_order_cancellations evidence
                WHERE evidence.purchase_order_id=purchase.id),
               (SELECT COUNT(*) FROM purchase_order_asn_sources source
                WHERE source.purchase_order_id=purchase.id)
        FROM purchase_orders purchase WHERE purchase.id=$1
        "#,
    )
    .bind(order.purchase_order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    if state.0 == "cancelled" {
        assert_eq!(state, ("cancelled".to_owned(), 1, 0));
    } else {
        assert_eq!(state, ("released".to_owned(), 0, 1));
    }
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn cancellation_replays_and_ledger_are_scope_bound() {
    let context = fixture("purchase-order-cancel-scope@test.local").await;
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
    let order = create_order(&context, "PO-CANCEL-SCOPE").await;
    let body = cancellation_body(1, "duplicate_order", None);
    let cancelled = cancel_order(&context, order.purchase_order_id, "cancel-po-scope", &body).await;
    assert_eq!(cancelled.status(), StatusCode::OK);

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
    for hidden_body in [body, cancellation_body(1, "supplier_cancelled", None)] {
        let hidden = cancel_order(
            &context,
            order.purchase_order_id,
            "cancel-po-scope",
            &hidden_body,
        )
        .await;
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    }

    let admin = admin_db_for(&context.fixture.db).await;
    let controls: (bool, bool, bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT relforcerowsecurity,
               (SELECT COUNT(*)=1 FROM pg_policies
                WHERE schemaname='public'
                  AND tablename='purchase_order_cancellations'),
               has_table_privilege('wareboxes_app','purchase_order_cancellations','SELECT'),
               has_table_privilege('wareboxes_app','purchase_order_cancellations','INSERT'),
               has_table_privilege('wareboxes_app','purchase_order_cancellations','UPDATE'),
               has_table_privilege('wareboxes_app','purchase_order_cancellations','DELETE')
        FROM pg_class WHERE oid='purchase_order_cancellations'::regclass
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(controls, (true, true, true, true, false, false));
    admin.close().await;
}
