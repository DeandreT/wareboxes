mod common;

#[path = "api_v1_pick_reversals/support.rs"]
mod support;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::json;
use sqlx::Row;
use support::*;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    ErrorReason, ErrorResponse, OpenPackSessionResponse, PickClaimResponse,
    PickConfirmationHistoryPage, PickContentConfirmationResponse, PickContentState,
    PickOrderStatus, ReversePickConfirmationResponse,
};

#[tokio::test]
async fn reversal_is_scan_verified_replay_safe_inventory_neutral_and_repickable() {
    let fixture = Fixture::new().await;
    let supervisor = fixture
        .wms_user("pick-reversal-supervisor@test.local")
        .await;
    let access = default_tenant_for_user(&fixture.db, supervisor.id)
        .await
        .unwrap();
    grant_permission(
        &fixture,
        access.tenant_id,
        supervisor.id,
        "orders",
        "pick-reversal-orders",
    )
    .await;
    grant_permission(
        &fixture,
        access.tenant_id,
        supervisor.id,
        "wms_supervisor",
        "pick-reversal-supervisor",
    )
    .await;
    let token = auth::create_session(&fixture.db, supervisor.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let picked = completed_pick(&fixture, &app, &token, &access, "PICK-REVERSAL").await;
    let path = format!(
        "/api/v1/pick-confirmations/{}/reversals",
        picked.confirmation.result_id
    );
    let valid = reversal_body(&picked, 4);

    for (key, changed_field, changed_value) in [
        (
            "pick-reversal-wrong-stage",
            "staged_location_barcode",
            "WRONG-STAGE",
        ),
        (
            "pick-reversal-wrong-tote",
            "staged_license_plate_barcode",
            "WRONG-TOTE",
        ),
        ("pick-reversal-wrong-item", "item_barcode", "WRONG-ITEM"),
        (
            "pick-reversal-wrong-return",
            "return_location_barcode",
            "WRONG-RETURN",
        ),
    ] {
        let mut body = valid.clone();
        body[changed_field] = json!(changed_value);
        let response = send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &path,
            Some(key),
            Some(body),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{key}");
    }
    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let zero_effects: (i64, String, String, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM pick_reversals WHERE tenant_id = $1),
               task.status, content.state, order_header.revision
        FROM pick_tasks task
        INNER JOIN pick_task_contents content
          ON content.tenant_id = task.tenant_id AND content.task_id = task.id
        INNER JOIN orders order_header
          ON order_header.tenant_id = task.tenant_id AND order_header.id = task.order_id
        WHERE task.tenant_id = $1 AND task.id = $2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(picked.task_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(zero_effects, (0, "completed".into(), "completed".into(), 4));

    let first = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &path,
        Some("pick-reversal-first"),
        Some(valid.clone()),
    );
    let second = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &path,
        Some("pick-reversal-race"),
        Some(valid.clone()),
    );
    let (first, second) = tokio::join!(first, second);
    let (success, conflict, replay_key) = match (first.status(), second.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => (first, second, "pick-reversal-first"),
        (StatusCode::CONFLICT, StatusCode::OK) => (second, first, "pick-reversal-race"),
        statuses => panic!("expected one reversal and one conflict, got {statuses:?}"),
    };
    assert_eq!(
        response_json::<ErrorResponse>(conflict).await.reason,
        ErrorReason::Conflict
    );
    let reversed: ReversePickConfirmationResponse = response_json(success).await;
    assert_eq!(reversed.confirmation_id, picked.confirmation.result_id);
    assert_eq!(reversed.reversed_quantity, 4);
    assert_eq!(reversed.content_state, PickContentState::Pending);
    assert_eq!(reversed.order_status, PickOrderStatus::Processing);
    assert_eq!(reversed.order_revision.get(), 5);

    let replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &path,
        Some(replay_key),
        Some(valid.clone()),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<ReversePickConfirmationResponse>(replay).await,
        reversed
    );
    let mut changed = valid.clone();
    changed["reason"] = json!("wrong_quantity");
    let changed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &path,
        Some(replay_key),
        Some(changed),
    )
    .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let durable = sqlx::query(
        r#"
        SELECT task.status, task.assigned_user_id, content.state,
               order_header.status AS order_status, order_header.revision,
               source.status AS source_status, source.deleted AS source_deleted,
               staged.status AS staged_status, staged.deleted AS staged_deleted,
               source_balance.qty_on_hand AS source_on_hand,
               source_balance.qty_reserved AS source_reserved,
               staged_balance.qty_on_hand AS staged_on_hand,
               staged_balance.qty_reserved AS staged_reserved,
               (SELECT COUNT(*) FROM inventory_entries entry
                WHERE entry.tenant_id = reversal.tenant_id
                  AND entry.transaction_id = reversal.inventory_transaction_id) AS entry_count,
               (SELECT COALESCE(SUM(entry.quantity_delta), 0)::BIGINT
                FROM inventory_entries entry
                WHERE entry.tenant_id = reversal.tenant_id
                  AND entry.transaction_id = reversal.inventory_transaction_id) AS entry_net,
               (SELECT COUNT(*) FROM outbox_events event
                WHERE event.tenant_id = reversal.tenant_id
                  AND event.event_type = 'outbound.pick.reversed'
                  AND (event.payload->>'pick_reversal_id')::BIGINT = reversal.id) AS event_count
        FROM pick_reversals reversal
        INNER JOIN pick_tasks task
          ON task.tenant_id = reversal.tenant_id AND task.id = reversal.task_id
        INNER JOIN pick_task_contents content
          ON content.tenant_id = reversal.tenant_id AND content.id = reversal.pick_task_content_id
        INNER JOIN orders order_header
          ON order_header.tenant_id = reversal.tenant_id AND order_header.id = reversal.order_id
        INNER JOIN inventory_allocations source
          ON source.tenant_id = reversal.tenant_id
         AND source.inventory_owner_id = reversal.inventory_owner_id
         AND source.id = reversal.source_inventory_allocation_id
        INNER JOIN inventory_allocations staged
          ON staged.tenant_id = reversal.tenant_id
         AND staged.inventory_owner_id = reversal.inventory_owner_id
         AND staged.id = reversal.staged_inventory_allocation_id
        INNER JOIN inventory_balances source_balance
          ON source_balance.tenant_id = reversal.tenant_id
         AND source_balance.inventory_owner_id = reversal.inventory_owner_id
         AND source_balance.id = reversal.source_inventory_balance_id
        INNER JOIN inventory_balances staged_balance
          ON staged_balance.tenant_id = reversal.tenant_id
         AND staged_balance.inventory_owner_id = reversal.inventory_owner_id
         AND staged_balance.id = reversal.staged_inventory_balance_id
        WHERE reversal.tenant_id = $1 AND reversal.id = $2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(reversed.reversal_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(durable.try_get::<String, _>("status").unwrap(), "open");
    assert!(durable
        .try_get::<Option<i64>, _>("assigned_user_id")
        .unwrap()
        .is_none());
    assert_eq!(durable.try_get::<String, _>("state").unwrap(), "pending");
    assert_eq!(
        durable.try_get::<String, _>("order_status").unwrap(),
        "processing"
    );
    assert_eq!(durable.try_get::<i64, _>("revision").unwrap(), 5);
    assert_eq!(
        durable.try_get::<String, _>("source_status").unwrap(),
        "allocated"
    );
    assert!(durable
        .try_get::<Option<wareboxes_domain::Timestamp>, _>("source_deleted")
        .unwrap()
        .is_none());
    assert_eq!(
        durable.try_get::<String, _>("staged_status").unwrap(),
        "released"
    );
    assert!(durable
        .try_get::<Option<wareboxes_domain::Timestamp>, _>("staged_deleted")
        .unwrap()
        .is_some());
    assert_eq!(durable.try_get::<i64, _>("source_on_hand").unwrap(), 7);
    assert_eq!(durable.try_get::<i64, _>("source_reserved").unwrap(), 4);
    assert_eq!(durable.try_get::<i64, _>("staged_on_hand").unwrap(), 0);
    assert_eq!(durable.try_get::<i64, _>("staged_reserved").unwrap(), 0);
    assert_eq!(durable.try_get::<i64, _>("entry_count").unwrap(), 2);
    assert_eq!(durable.try_get::<i64, _>("entry_net").unwrap(), 0);
    assert_eq!(durable.try_get::<i64, _>("event_count").unwrap(), 1);

    let claim = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/picking-claims/{}", picked.task_id),
        Some("pick-reversal-reclaim"),
        Some(json!({})),
    )
    .await;
    let claim: PickClaimResponse =
        response_json(expect_status(claim, StatusCode::OK, "reclaim reversed pick").await).await;
    let repick = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/picking-tasks/{}/contents/{}/confirmations",
            claim.task_id, claim.content.content_id
        ),
        Some("pick-reversal-repick"),
        Some(json!({
            "source_location_barcode": claim.content.source_location_barcode,
            "item_barcode": claim.content.item_barcodes[0],
            "destination_license_plate_barcode": picked.tote_barcode
        })),
    )
    .await;
    let repick: PickContentConfirmationResponse =
        response_json(expect_status(repick, StatusCode::OK, "confirm repick after reversal").await)
            .await;
    assert_ne!(repick.result_id, picked.confirmation.result_id);
    assert_eq!(repick.order_revision.get(), 6);
    assert_eq!(repick.order_status, PickOrderStatus::AwaitingPacking);

    let history = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &format!(
            "/api/v1/orders/{}/pick-confirmations?limit=1",
            picked.order_id
        ),
        None,
        None,
    )
    .await;
    let first_page: PickConfirmationHistoryPage = response_json(
        expect_status(history, StatusCode::OK, "first confirmation history page").await,
    )
    .await;
    assert_eq!(first_page.items.len(), 1);
    assert_eq!(first_page.items[0].confirmation_id, repick.result_id);
    assert!(first_page.items[0].reversal.is_none());
    let cursor = first_page.next_cursor.unwrap();
    let history = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &format!(
            "/api/v1/orders/{}/pick-confirmations?limit=1&cursor={cursor}",
            picked.order_id
        ),
        None,
        None,
    )
    .await;
    let second_page: PickConfirmationHistoryPage = response_json(
        expect_status(history, StatusCode::OK, "second confirmation history page").await,
    )
    .await;
    assert_eq!(second_page.items.len(), 1);
    assert_eq!(
        second_page.items[0].confirmation_id,
        picked.confirmation.result_id
    );
    assert_eq!(
        second_page.items[0].reversal.as_ref().unwrap().reversal_id,
        reversed.reversal_id
    );
    assert!(second_page.next_cursor.is_none());
    let mismatched_cursor = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &format!(
            "/api/v1/orders/{}/pick-confirmations?limit=1&cursor={cursor}",
            picked.order_id + 1
        ),
        None,
        None,
    )
    .await;
    assert_eq!(mismatched_cursor.status(), StatusCode::BAD_REQUEST);

    let session = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{}/packing-sessions", picked.order_id),
        Some("pick-reversal-open-packing"),
        Some(json!({
            "facility_id": picked.facility_id,
            "station_location_id": picked.execution_location_id,
            "expected_revision": 6
        })),
    )
    .await;
    let _: OpenPackSessionResponse =
        response_json(expect_status(session, StatusCode::OK, "open packing after repick").await)
            .await;
    let repick_path = format!("/api/v1/pick-confirmations/{}/reversals", repick.result_id);
    let after_packing = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &repick_path,
        Some("pick-reversal-after-packing"),
        Some(reversal_body(&picked, 7)),
    )
    .await;
    assert_eq!(after_packing.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn reversal_requires_supervisor_scope_and_immutable_tenant_evidence() {
    let fixture = Fixture::new().await;
    let supervisor = fixture.wms_user("pick-reversal-scope@test.local").await;
    let access = default_tenant_for_user(&fixture.db, supervisor.id)
        .await
        .unwrap();
    grant_permission(
        &fixture,
        access.tenant_id,
        supervisor.id,
        "orders",
        "pick-reversal-scope-orders",
    )
    .await;
    grant_permission(
        &fixture,
        access.tenant_id,
        supervisor.id,
        "wms_supervisor",
        "pick-reversal-scope-supervisor",
    )
    .await;
    let token = auth::create_session(&fixture.db, supervisor.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let picked = completed_pick(&fixture, &app, &token, &access, "PICK-REV-SCOPE").await;
    let path = format!(
        "/api/v1/pick-confirmations/{}/reversals",
        picked.confirmation.result_id
    );
    let body = reversal_body(&picked, 4);

    let admin = admin_db_for(&fixture.db).await;
    for statement in [
        format!(
            "UPDATE pick_tasks SET status = 'open', assigned_user_id = NULL, claimed_at = NULL, lease_expires_at = NULL, completed_at = NULL WHERE tenant_id = {} AND id = {}",
            access.tenant_id.get(), picked.task_id
        ),
        format!(
            "UPDATE pick_task_contents SET state = 'pending', completed_at = NULL WHERE tenant_id = {} AND task_id = {}",
            access.tenant_id.get(), picked.task_id
        ),
        format!(
            "UPDATE inventory_allocations SET status = 'allocated', modified = NULL, deleted = NULL WHERE tenant_id = {} AND id = {}",
            access.tenant_id.get(), picked.confirmation.source_inventory_allocation_id
        ),
    ] {
        let error = sqlx::query(&statement).execute(&admin).await.unwrap_err();
        assert_eq!(
            error
                .as_database_error()
                .and_then(|database| database.code())
                .as_deref(),
            Some("55000")
        );
    }
    admin.close().await;

    let operator = fixture
        .wms_user("pick-reversal-no-supervisor@test.local")
        .await;
    let operator_access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    let denied = send(
        &app,
        &auth::create_session(&fixture.db, operator.id)
            .await
            .unwrap(),
        operator_access.tenant_id,
        Method::POST,
        &path,
        Some("pick-reversal-no-supervisor"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let success = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &path,
        Some("pick-reversal-scope-success"),
        Some(body.clone()),
    )
    .await;
    let reversed: ReversePickConfirmationResponse =
        response_json(expect_status(success, StatusCode::OK, "scoped reversal").await).await;

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let privileges: (bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT has_table_privilege(current_user, 'pick_reversals', 'SELECT'),
               has_table_privilege(current_user, 'pick_reversals', 'INSERT'),
               has_table_privilege(current_user, 'pick_reversals', 'UPDATE'),
               has_table_privilege(current_user, 'pick_reversals', 'DELETE')
        "#,
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(privileges, (true, true, false, false));
    let forced_rls: (bool, bool) = sqlx::query_as(
        "SELECT relrowsecurity, relforcerowsecurity FROM pg_class WHERE oid = 'pick_reversals'::regclass",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(forced_rls, (true, true));
    let update_error =
        sqlx::query("UPDATE pick_reversals SET reason = reason WHERE tenant_id = $1 AND id = $2")
            .bind(access.tenant_id.get())
            .bind(reversed.reversal_id)
            .execute(&mut *tx)
            .await
            .unwrap_err();
    assert_eq!(
        update_error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("42501")
    );
    tx.rollback().await.unwrap();

    set_scope(
        &fixture,
        access.tenant_id,
        supervisor.id,
        Vec::new(),
        Vec::new(),
    )
    .await;
    for request_body in [body.clone(), {
        let mut changed = body.clone();
        changed["reason"] = json!("wrong_quantity");
        changed
    }] {
        let concealed = send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &path,
            Some("pick-reversal-scope-success"),
            Some(request_body),
        )
        .await;
        assert_eq!(concealed.status(), StatusCode::NOT_FOUND);
    }

    let app_pool = privileged_session_as_app(&fixture.db).await;
    let mut unbound = app_pool.begin().await.unwrap();
    let unbound_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pick_reversals")
        .fetch_one(&mut *unbound)
        .await
        .unwrap();
    assert_eq!(unbound_count, 0);
    unbound.rollback().await.unwrap();
    app_pool.close().await;
}
