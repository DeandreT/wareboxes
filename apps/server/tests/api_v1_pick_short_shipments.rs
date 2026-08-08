mod common;

#[allow(dead_code)]
#[path = "api_v1_pick_shortages/support.rs"]
mod shortage_support;

#[path = "api_v1_pick_short_shipments/support.rs"]
mod support;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{json, Value};
use shortage_support::*;
use sqlx::Row;
use support::*;
use wareboxes_api::{auth, routes, state::AppState};
use wareboxes_api_contract::v1::{
    AcceptPickShortageAsShortShipResponse, CloseCartonResponse, ConfirmShipmentDepartureResponse,
    CreateCartonResponse, CreateShipmentResponse, ErrorReason, ErrorResponse,
    OpenPackSessionResponse, PackPickedAllocationResponse, PickClaimResponse, PickOrderStatus,
    PickShortShipReason, PickShortageResolution, PickShortageResponse, PickShortageStatus,
    RecordManualManifestResponse, ReportPickShortageResponse,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::TenantId;

#[tokio::test]
async fn short_ship_acceptance_is_inventory_neutral_replay_safe_and_advances_effective_demand() {
    init_test_tracing();
    let short = PickShortageFixture::new("short-ship-success", 5).await;
    short.make_destination_a_packing_station().await;
    let report = short
        .report(
            Some("short-ship-success-report"),
            short.partial_body(2, "insufficient_quantity", None),
        )
        .await;
    let report: ReportPickShortageResponse =
        response_json(expect_status(report, StatusCode::OK, "report partial shortage").await).await;
    short.grant_supervisor().await;
    let body = short_ship_body(report.shortage_revision.get(), report.order_revision.get());
    let before = inventory_snapshot(&short).await;

    let missing_key = accept_short_ship(&short, report.shortage_id, None, body.clone()).await;
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);
    let stale_shortage = accept_short_ship(
        &short,
        report.shortage_id,
        Some("short-ship-stale-shortage"),
        short_ship_body(
            report.shortage_revision.get() + 1,
            report.order_revision.get(),
        ),
    )
    .await;
    assert_eq!(stale_shortage.status(), StatusCode::CONFLICT);
    let stale_order = accept_short_ship(
        &short,
        report.shortage_id,
        Some("short-ship-stale-order"),
        short_ship_body(
            report.shortage_revision.get(),
            report.order_revision.get() + 1,
        ),
    )
    .await;
    assert_eq!(stale_order.status(), StatusCode::CONFLICT);
    assert_eq!(disposition_count(&short).await, 0);
    before.assert_unchanged(&inventory_snapshot(&short).await);

    let accepted = accept_short_ship(
        &short,
        report.shortage_id,
        Some("short-ship-success-accept"),
        body.clone(),
    )
    .await;
    let accepted: AcceptPickShortageAsShortShipResponse =
        response_json(expect_status(accepted, StatusCode::OK, "accept short shipment").await).await;
    assert_eq!(accepted.shortage_id, report.shortage_id);
    assert_eq!(
        accepted.previous_shortage_status,
        PickShortageStatus::AwaitingInventory
    );
    assert_eq!(accepted.shortage_status, PickShortageStatus::Resolved);
    assert_eq!(
        accepted.shortage_resolution,
        PickShortageResolution::ShortShip
    );
    assert_eq!(accepted.shortage_revision.get(), 2);
    assert_eq!(accepted.previous_order_status, PickOrderStatus::Processing);
    assert_eq!(accepted.order_status, PickOrderStatus::AwaitingPacking);
    assert_eq!(
        accepted.order_revision.get(),
        report.order_revision.get() + 1
    );
    assert!(accepted.order_ready_to_pack);
    assert_eq!(accepted.shortage_quantities, report.quantities);
    assert_eq!(accepted.accepted_short_quantity, 3);
    assert_eq!(accepted.reallocated_quantity, 0);
    assert_eq!(accepted.recovery_terminal_quantity, 0);
    assert_eq!(accepted.line_demand.ordered, 5);
    assert_eq!(accepted.line_demand.accepted_short, 3);
    assert_eq!(accepted.line_demand.effective, 2);
    assert_eq!(accepted.order_demand, accepted.line_demand);
    assert_eq!(accepted.inventory_hold_id, report.hold.hold_id);
    assert_eq!(accepted.reason, PickShortShipReason::InventoryUnavailable);
    assert_eq!(
        accepted.note.as_deref(),
        Some("Inventory unavailable before the shipping commitment")
    );
    before.assert_unchanged(&inventory_snapshot(&short).await);

    let replay = accept_short_ship(
        &short,
        report.shortage_id,
        Some("short-ship-success-accept"),
        body.clone(),
    )
    .await;
    assert_eq!(
        response_json::<AcceptPickShortageAsShortShipResponse>(
            expect_status(replay, StatusCode::OK, "replay short shipment").await,
        )
        .await,
        accepted
    );
    let mut changed = body;
    changed["reason"] = json!("client_authorized");
    let changed = accept_short_ship(
        &short,
        report.shortage_id,
        Some("short-ship-success-accept"),
        changed,
    )
    .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );
    assert_eq!(disposition_count(&short).await, 1);
    before.assert_unchanged(&inventory_snapshot(&short).await);

    let mut tx = tenant_tx(&short.fixture.db, short.access.tenant_id).await;
    let row = sqlx::query(
        r#"
        SELECT shortage.status, shortage.resolution, shortage.accepted_short_qty,
               shortage.remaining_to_allocate_qty, shortage.reallocated_qty,
               shortage.recovery_terminal_qty, orders.status AS order_status,
               orders.revision AS order_revision, hold.status AS hold_status,
               hold.qty AS hold_qty, balance.qty_on_hand, balance.qty_reserved,
               balance.qty_held, reservation.status AS reservation_status,
               reservation.qty AS reservation_qty
        FROM pick_shortages shortage
        INNER JOIN orders
          ON orders.tenant_id = shortage.tenant_id AND orders.id = shortage.order_id
        INNER JOIN inventory_holds hold
          ON hold.tenant_id = shortage.tenant_id AND hold.id = shortage.inventory_hold_id
        INNER JOIN inventory_balances balance
          ON balance.tenant_id = shortage.tenant_id
         AND balance.id = shortage.source_inventory_balance_id
        INNER JOIN inventory_reservations reservation
          ON reservation.tenant_id = shortage.tenant_id
         AND reservation.id = shortage.reservation_id
        WHERE shortage.tenant_id = $1 AND shortage.id = $2
        "#,
    )
    .bind(short.access.tenant_id.get())
    .bind(report.shortage_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(row.get::<String, _>("status"), "resolved");
    assert_eq!(
        row.get::<Option<String>, _>("resolution").as_deref(),
        Some("short_ship")
    );
    assert_eq!(row.get::<i64, _>("accepted_short_qty"), 3);
    assert_eq!(row.get::<i64, _>("remaining_to_allocate_qty"), 3);
    assert_eq!(row.get::<i64, _>("reallocated_qty"), 0);
    assert_eq!(row.get::<i64, _>("recovery_terminal_qty"), 0);
    assert_eq!(row.get::<String, _>("order_status"), "awaiting packing");
    assert_eq!(
        row.get::<i64, _>("order_revision"),
        accepted.order_revision.get()
    );
    assert_eq!(row.get::<String, _>("hold_status"), "active");
    assert_eq!(row.get::<i64, _>("hold_qty"), 3);
    assert_eq!(row.get::<i64, _>("qty_on_hand"), 3);
    assert_eq!(row.get::<i64, _>("qty_reserved"), 0);
    assert_eq!(row.get::<i64, _>("qty_held"), 3);
    assert_eq!(row.get::<String, _>("reservation_status"), "active");
    assert_eq!(row.get::<i64, _>("reservation_qty"), 5);
}

#[tokio::test]
async fn zero_effective_demand_and_nonawaiting_shortages_are_rejected_without_effects() {
    init_test_tracing();
    let zero = PickShortageFixture::new("short-ship-zero", 4).await;
    let zero_report = zero
        .report(
            Some("short-ship-zero-report"),
            zero.no_pick_body("inventory_missing", None),
        )
        .await;
    let zero_report: ReportPickShortageResponse =
        response_json(expect_status(zero_report, StatusCode::OK, "report total shortage").await)
            .await;
    zero.grant_supervisor().await;
    let before = inventory_snapshot(&zero).await;
    let rejected = accept_short_ship(
        &zero,
        zero_report.shortage_id,
        Some("short-ship-zero-accept"),
        short_ship_body(
            zero_report.shortage_revision.get(),
            zero_report.order_revision.get(),
        ),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    assert_eq!(short_ship_effect_counts(&zero).await, (0, 0, 0, 0));
    before.assert_unchanged(&inventory_snapshot(&zero).await);
    let detail = zero
        .request(
            Method::GET,
            &format!("/api/v1/pick-shortages/{}", zero_report.shortage_id),
            None,
            None,
        )
        .await;
    let detail: PickShortageResponse =
        response_json(expect_status(detail, StatusCode::OK, "read rejected total shortage").await)
            .await;
    assert_eq!(detail.status, PickShortageStatus::AwaitingInventory);
    assert_eq!(detail.shortage_revision, zero_report.shortage_revision);
    assert_eq!(detail.order_revision, zero_report.order_revision);

    let recovery = PickShortageFixture::new("short-ship-recovery-active", 5).await;
    let report = recovery
        .report(
            Some("short-ship-recovery-active-report"),
            recovery.partial_body(1, "insufficient_quantity", None),
        )
        .await;
    let report: ReportPickShortageResponse =
        response_json(expect_status(report, StatusCode::OK, "report recoverable shortage").await)
            .await;
    recovery.grant_supervisor().await;
    recovery
        .add_recovery_balance(4, "short-ship-recovery-active-stock")
        .await;
    let reallocated = recovery
        .reallocate(
            report.shortage_id,
            Some("short-ship-recovery-active-reallocate"),
            reallocation_body(report.shortage_revision.get(), report.order_revision.get()),
        )
        .await;
    let reallocated: wareboxes_api_contract::v1::ReallocatePickShortageResponse =
        response_json(expect_status(reallocated, StatusCode::OK, "start shortage recovery").await)
            .await;
    assert_eq!(
        reallocated.shortage_status,
        PickShortageStatus::RecoveryInProgress
    );
    let before = inventory_snapshot(&recovery).await;
    let rejected = accept_short_ship(
        &recovery,
        report.shortage_id,
        Some("short-ship-recovery-active-accept"),
        short_ship_body(
            reallocated.shortage_revision.get(),
            reallocated.order_revision.get(),
        ),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    assert_eq!(disposition_count(&recovery).await, 0);
    before.assert_unchanged(&inventory_snapshot(&recovery).await);

    let stale = PickShortageFixture::new("short-ship-stale-revisions", 4).await;
    let stale_report = stale
        .report(
            Some("short-ship-stale-revisions-report"),
            stale.partial_body(1, "insufficient_quantity", None),
        )
        .await;
    let stale_report: ReportPickShortageResponse = response_json(
        expect_status(stale_report, StatusCode::OK, "report revision shortage").await,
    )
    .await;
    stale.grant_supervisor().await;
    let current = stale
        .reallocate(
            stale_report.shortage_id,
            Some("short-ship-stale-revisions-reallocate"),
            reallocation_body(
                stale_report.shortage_revision.get(),
                stale_report.order_revision.get(),
            ),
        )
        .await;
    let current: wareboxes_api_contract::v1::ReallocatePickShortageResponse = response_json(
        expect_status(current, StatusCode::OK, "advance disposition revisions").await,
    )
    .await;
    let stale_shortage = accept_short_ship(
        &stale,
        stale_report.shortage_id,
        Some("short-ship-independent-stale-shortage"),
        short_ship_body(
            stale_report.shortage_revision.get(),
            current.order_revision.get(),
        ),
    )
    .await;
    assert_eq!(stale_shortage.status(), StatusCode::CONFLICT);
    let stale_order = accept_short_ship(
        &stale,
        stale_report.shortage_id,
        Some("short-ship-independent-stale-order"),
        short_ship_body(
            current.shortage_revision.get(),
            stale_report.order_revision.get(),
        ),
    )
    .await;
    assert_eq!(stale_order.status(), StatusCode::CONFLICT);
    assert_eq!(disposition_count(&stale).await, 0);
}

#[tokio::test]
async fn partial_recovery_nested_and_multiple_shortages_use_cumulative_effective_demand() {
    init_test_tracing();
    let partial = PickShortageFixture::new("short-ship-partial-recovery", 5).await;
    let report = partial
        .report(
            Some("short-ship-partial-recovery-report"),
            partial.no_pick_body("inventory_missing", None),
        )
        .await;
    let report: ReportPickShortageResponse = response_json(
        expect_status(report, StatusCode::OK, "report partial recovery shortage").await,
    )
    .await;
    partial.grant_supervisor().await;
    partial
        .add_recovery_balance(2, "short-ship-partial-recovery-stock")
        .await;
    let reallocation = partial
        .reallocate(
            report.shortage_id,
            Some("short-ship-partial-recovery-reallocate"),
            reallocation_body(report.shortage_revision.get(), report.order_revision.get()),
        )
        .await;
    let reallocation: wareboxes_api_contract::v1::ReallocatePickShortageResponse = response_json(
        expect_status(
            reallocation,
            StatusCode::OK,
            "partially reallocate shortage",
        )
        .await,
    )
    .await;
    assert_eq!(reallocation.newly_allocated_quantity, 2);
    let confirmation = partial
        .confirm_next(report.shortage_id, "short-ship-partial-recovery-confirm")
        .await;
    assert_eq!(
        confirmation.shortage_status,
        PickShortageStatus::AwaitingInventory
    );
    let detail = partial
        .request(
            Method::GET,
            &format!("/api/v1/pick-shortages/{}", report.shortage_id),
            None,
            None,
        )
        .await;
    let detail: PickShortageResponse = response_json(
        expect_status(detail, StatusCode::OK, "read terminal partial recovery").await,
    )
    .await;
    let accepted = accept_short_ship(
        &partial,
        report.shortage_id,
        Some("short-ship-partial-recovery-accept"),
        short_ship_body(detail.shortage_revision.get(), detail.order_revision.get()),
    )
    .await;
    let accepted: AcceptPickShortageAsShortShipResponse = response_json(
        expect_status(accepted, StatusCode::OK, "accept terminal unmet recovery").await,
    )
    .await;
    assert_eq!(accepted.reallocated_quantity, 2);
    assert_eq!(accepted.recovery_terminal_quantity, 2);
    assert_eq!(accepted.accepted_short_quantity, 3);
    assert_eq!(accepted.order_demand.effective, 2);
    assert!(accepted.order_ready_to_pack);

    let nested = PickShortageFixture::new("short-ship-nested", 6).await;
    let parent = nested
        .report(
            Some("short-ship-nested-parent-report"),
            nested.partial_body(2, "insufficient_quantity", None),
        )
        .await;
    let parent: ReportPickShortageResponse =
        response_json(expect_status(parent, StatusCode::OK, "report nested parent shortage").await)
            .await;
    nested.grant_supervisor().await;
    nested
        .add_recovery_balance(4, "short-ship-nested-recovery-stock")
        .await;
    let reallocated = nested
        .reallocate(
            parent.shortage_id,
            Some("short-ship-nested-reallocate"),
            reallocation_body(parent.shortage_revision.get(), parent.order_revision.get()),
        )
        .await;
    let reallocated: wareboxes_api_contract::v1::ReallocatePickShortageResponse =
        response_json(expect_status(reallocated, StatusCode::OK, "reallocate nested parent").await)
            .await;
    let recovery_claim = nested.claim_next("short-ship-nested-recovery-claim").await;
    let child = nested
        .report_claim(
            &recovery_claim,
            "short-ship-nested-child-report",
            PickShortageFixture::no_pick_body_for_claim(&recovery_claim),
        )
        .await;
    let child: ReportPickShortageResponse =
        response_json(expect_status(child, StatusCode::OK, "report nested child shortage").await)
            .await;
    assert_ne!(child.shortage_id, parent.shortage_id);
    assert_eq!(reallocated.newly_allocated_quantity, child.quantities.short);
    let accepted = accept_short_ship(
        &nested,
        child.shortage_id,
        Some("short-ship-nested-child-accept"),
        short_ship_body(child.shortage_revision.get(), child.order_revision.get()),
    )
    .await;
    let accepted: AcceptPickShortageAsShortShipResponse = response_json(
        expect_status(accepted, StatusCode::OK, "accept nested child shortage").await,
    )
    .await;
    assert_eq!(accepted.accepted_short_quantity, 4);
    assert_eq!(accepted.order_demand.ordered, 6);
    assert_eq!(accepted.order_demand.accepted_short, 4);
    assert_eq!(accepted.order_demand.effective, 2);
    assert!(accepted.order_ready_to_pack);

    let multiple = MultiShortageFixture::new("short-ship-multiple", &[(3, 1), (4, 2)]).await;
    let first_report = &multiple.reports[0];
    let second_report = &multiple.reports[1];
    let first = multiple
        .accept(
            first_report.shortage_id,
            "short-ship-multiple-first",
            short_ship_body(
                first_report.shortage_revision.get(),
                second_report.order_revision.get(),
            ),
        )
        .await;
    let first: AcceptPickShortageAsShortShipResponse =
        response_json(expect_status(first, StatusCode::OK, "accept first sibling shortage").await)
            .await;
    assert_eq!(first.order_status, PickOrderStatus::Processing);
    assert!(!first.order_ready_to_pack);
    assert_eq!(first.order_demand.ordered, 7);
    assert_eq!(first.order_demand.accepted_short, 2);
    assert_eq!(first.order_demand.effective, 5);
    let second = multiple
        .accept(
            second_report.shortage_id,
            "short-ship-multiple-second",
            short_ship_body(
                second_report.shortage_revision.get(),
                first.order_revision.get(),
            ),
        )
        .await;
    let second: AcceptPickShortageAsShortShipResponse = response_json(
        expect_status(second, StatusCode::OK, "accept second sibling shortage").await,
    )
    .await;
    assert_eq!(second.order_status, PickOrderStatus::AwaitingPacking);
    assert!(second.order_ready_to_pack);
    assert_eq!(second.order_demand.ordered, 7);
    assert_eq!(second.order_demand.accepted_short, 4);
    assert_eq!(second.order_demand.effective, 3);
}

#[tokio::test]
async fn disposition_and_reallocation_race_has_one_winner() {
    init_test_tracing();
    let race = PickShortageFixture::new("short-ship-race", 4).await;
    let report = race
        .report(
            Some("short-ship-race-report"),
            race.partial_body(1, "insufficient_quantity", None),
        )
        .await;
    let report: ReportPickShortageResponse =
        response_json(expect_status(report, StatusCode::OK, "report race shortage").await).await;
    race.grant_supervisor().await;
    race.add_recovery_balance(3, "short-ship-race-stock").await;
    let accept = accept_short_ship(
        &race,
        report.shortage_id,
        Some("short-ship-race-accept"),
        short_ship_body(report.shortage_revision.get(), report.order_revision.get()),
    );
    let reallocate = race.reallocate(
        report.shortage_id,
        Some("short-ship-race-reallocate"),
        reallocation_body(report.shortage_revision.get(), report.order_revision.get()),
    );
    let (accept, reallocate) = tokio::join!(accept, reallocate);
    match (accept.status(), reallocate.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => {
            let accepted: AcceptPickShortageAsShortShipResponse = response_json(accept).await;
            assert_eq!(accepted.order_status, PickOrderStatus::AwaitingPacking);
            assert_eq!(disposition_count(&race).await, 1);
            race.assert_reallocation_count(report.shortage_id, 0).await;
        }
        (StatusCode::CONFLICT, StatusCode::OK) => {
            let reallocated: wareboxes_api_contract::v1::ReallocatePickShortageResponse =
                response_json(reallocate).await;
            assert_eq!(
                reallocated.shortage_status,
                PickShortageStatus::RecoveryInProgress
            );
            assert_eq!(disposition_count(&race).await, 0);
            race.assert_reallocation_count(report.shortage_id, 1).await;
        }
        statuses => panic!("expected one disposition-or-reallocation winner, got {statuses:?}"),
    }
}

#[tokio::test]
async fn dispositions_are_scoped_rls_governed_immutable_audited_and_replay_concealed() {
    init_test_tracing();
    let short = PickShortageFixture::new("short-ship-governance", 5).await;
    let report = short
        .report(
            Some("short-ship-governance-report"),
            short.partial_body(2, "insufficient_quantity", None),
        )
        .await;
    let report: ReportPickShortageResponse =
        response_json(expect_status(report, StatusCode::OK, "report governed shortage").await)
            .await;
    short.grant_supervisor().await;
    let body = short_ship_body(report.shortage_revision.get(), report.order_revision.get());

    let operator = short
        .operator_only_token("short-ship-governance-operator@test.local")
        .await;
    let forbidden = send(
        &short.app,
        &operator,
        short.access.tenant_id,
        Method::POST,
        &short_ship_path(report.shortage_id),
        Some("short-ship-governance-forbidden"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    assert_eq!(disposition_count(&short).await, 0);

    let outsider_user = short
        .fixture
        .wms_user("short-ship-governance-outsider@test.local")
        .await;
    let outsider_tenant = tenant_for_user(&short.fixture.db, outsider_user.id).await;
    grant_permission(
        &short.fixture.db,
        outsider_tenant,
        outsider_user.id,
        "short-ship-governance-outsider-supervisor",
        "wms_supervisor",
    )
    .await;
    let outsider_token = auth::create_session(&short.fixture.db, outsider_user.id)
        .await
        .unwrap();
    let guessed = send(
        &short.app,
        &outsider_token,
        outsider_tenant,
        Method::POST,
        &short_ship_path(report.shortage_id),
        Some("short-ship-governance-guessed"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(guessed.status(), StatusCode::NOT_FOUND);
    assert_eq!(disposition_count(&short).await, 0);

    let accepted = accept_short_ship(
        &short,
        report.shortage_id,
        Some("short-ship-governance-accept"),
        body.clone(),
    )
    .await;
    let accepted: AcceptPickShortageAsShortShipResponse = response_json(
        expect_status(accepted, StatusCode::OK, "accept governed short shipment").await,
    )
    .await;
    assert_disposition_evidence(&short, &report, &accepted).await;

    let admin = admin_db_for(&short.fixture.db).await;
    let privileges: (bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT has_table_privilege('wareboxes_app', 'pick_short_ship_dispositions', 'SELECT'),
               has_table_privilege('wareboxes_app', 'pick_short_ship_dispositions', 'INSERT'),
               has_table_privilege('wareboxes_app', 'pick_short_ship_dispositions', 'UPDATE'),
               has_table_privilege('wareboxes_app', 'pick_short_ship_dispositions', 'DELETE')
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(privileges, (true, true, false, false));
    let sequence_privileges: (bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT has_sequence_privilege('wareboxes_app', 'pick_short_ship_dispositions_id_seq', 'USAGE'),
               has_sequence_privilege('wareboxes_app', 'pick_short_ship_dispositions_id_seq', 'SELECT'),
               has_sequence_privilege('wareboxes_app', 'pick_short_ship_dispositions_id_seq', 'UPDATE')
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(sequence_privileges, (true, false, false));
    let rls: (bool, bool) = sqlx::query_as(
        "SELECT relrowsecurity, relforcerowsecurity FROM pg_class WHERE oid = 'pick_short_ship_dispositions'::regclass",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(rls, (true, true));
    for (operation, statement, id) in [
        (
            "disposition update",
            "UPDATE pick_short_ship_dispositions SET reason_code = reason_code WHERE tenant_id = $1 AND id = $2",
            accepted.disposition_id,
        ),
        (
            "disposition delete",
            "DELETE FROM pick_short_ship_dispositions WHERE tenant_id = $1 AND id = $2",
            accepted.disposition_id,
        ),
        (
            "shortage resolution rewrite",
            "UPDATE pick_shortages SET accepted_short_qty = accepted_short_qty + 1 WHERE tenant_id = $1 AND id = $2",
            report.shortage_id,
        ),
    ] {
        let result = sqlx::query(statement)
            .bind(short.access.tenant_id.get())
            .bind(id)
            .execute(&admin)
            .await;
        assert!(result.is_err(), "{operation} must be rejected");
    }
    admin.close().await;

    let app_db = app_db_for(&short.fixture.db).await;
    let missing_context: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pick_short_ship_dispositions WHERE tenant_id = $1",
    )
    .bind(short.access.tenant_id.get())
    .fetch_one(&app_db)
    .await
    .unwrap();
    assert_eq!(missing_context, 0);
    let mut outsider_tx = tenant_tx(&app_db, outsider_tenant).await;
    let concealed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pick_short_ship_dispositions WHERE tenant_id = $1 AND id = $2",
    )
    .bind(short.access.tenant_id.get())
    .bind(accepted.disposition_id)
    .fetch_one(&mut *outsider_tx)
    .await
    .unwrap();
    outsider_tx.rollback().await.unwrap();
    app_db.close().await;
    assert_eq!(concealed, 0);

    short.set_scope(vec![short.facility_id], Vec::new()).await;
    let owner_revoked = accept_short_ship(
        &short,
        report.shortage_id,
        Some("short-ship-governance-accept"),
        body.clone(),
    )
    .await;
    assert_eq!(owner_revoked.status(), StatusCode::NOT_FOUND);
    let owner_revoked_fresh = accept_short_ship(
        &short,
        report.shortage_id,
        Some("short-ship-governance-owner-revoked-fresh"),
        short_ship_body(
            accepted.shortage_revision.get(),
            accepted.order_revision.get(),
        ),
    )
    .await;
    assert_eq!(owner_revoked_fresh.status(), StatusCode::NOT_FOUND);
    let mut changed = body.clone();
    changed["reason"] = json!("client_authorized");
    let owner_revoked_changed = accept_short_ship(
        &short,
        report.shortage_id,
        Some("short-ship-governance-accept"),
        changed,
    )
    .await;
    assert_eq!(owner_revoked_changed.status(), StatusCode::NOT_FOUND);
    short
        .set_scope(Vec::new(), vec![short.inventory_owner_id])
        .await;
    let facility_revoked = accept_short_ship(
        &short,
        report.shortage_id,
        Some("short-ship-governance-accept"),
        body,
    )
    .await;
    assert_eq!(facility_revoked.status(), StatusCode::NOT_FOUND);
    let facility_revoked_fresh = accept_short_ship(
        &short,
        report.shortage_id,
        Some("short-ship-governance-facility-revoked-fresh"),
        short_ship_body(
            accepted.shortage_revision.get(),
            accepted.order_revision.get(),
        ),
    )
    .await;
    assert_eq!(facility_revoked_fresh.status(), StatusCode::NOT_FOUND);
    assert_eq!(disposition_count(&short).await, 1);
}

#[tokio::test]
async fn reduced_order_packs_manifests_and_departs_with_demand_and_inventory_conservation() {
    init_test_tracing();
    let short = PickShortageFixture::new("short-ship-fulfillment", 5).await;
    short.make_destination_a_packing_station().await;
    let report = short
        .report(
            Some("short-ship-fulfillment-report"),
            short.partial_body(2, "insufficient_quantity", None),
        )
        .await;
    let report: ReportPickShortageResponse =
        response_json(expect_status(report, StatusCode::OK, "report fulfillment shortage").await)
            .await;
    short.grant_supervisor().await;
    let accepted = accept_short_ship(
        &short,
        report.shortage_id,
        Some("short-ship-fulfillment-accept"),
        short_ship_body(report.shortage_revision.get(), report.order_revision.get()),
    )
    .await;
    let accepted: AcceptPickShortageAsShortShipResponse =
        response_json(expect_status(accepted, StatusCode::OK, "accept fulfillment shortage").await)
            .await;
    let departed = pack_manifest_and_depart(&short, &accepted, "short-ship-fulfillment").await;
    assert_eq!(
        departed.order_status,
        wareboxes_api_contract::v1::ShipmentOrderStatus::Shipped
    );
    assert_eq!(departed.demand.ordered_quantity, 5);
    assert_eq!(departed.demand.shipped_quantity, 2);
    assert_eq!(departed.demand.accepted_short_quantity, 3);

    let mut tx = tenant_tx(&short.fixture.db, short.access.tenant_id).await;
    let row = sqlx::query(
        r#"
        SELECT order_item.qty AS ordered_qty,
               disposition.accepted_short_qty,
               shipment.shipped_qty,
               orders.status AS order_status,
               shipment.state AS shipment_status,
               reservation.status AS reservation_status,
               reservation.qty AS reservation_qty,
               hold.status AS hold_status, hold.qty AS hold_qty,
               (SELECT COALESCE(SUM(entry.quantity_delta), 0)::BIGINT
                FROM inventory_entries entry
                WHERE entry.tenant_id = shipment.tenant_id
                  AND entry.transaction_id = confirmation.inventory_transaction_id)
                   AS departure_delta,
               (SELECT COUNT(*) FROM inventory_transactions transaction
                WHERE transaction.tenant_id = shipment.tenant_id
                  AND transaction.operation = 'picking.shortage.accept_short_ship.v1')
                   AS disposition_transaction_count
        FROM pick_short_ship_dispositions disposition
        INNER JOIN pick_shortages shortage
          ON shortage.tenant_id = disposition.tenant_id
         AND shortage.id = disposition.pick_shortage_id
        INNER JOIN order_items order_item
          ON order_item.tenant_id = disposition.tenant_id
         AND order_item.id = disposition.order_item_id
        INNER JOIN orders
          ON orders.tenant_id = disposition.tenant_id
         AND orders.id = disposition.order_id
        INNER JOIN inventory_reservations reservation
          ON reservation.tenant_id = disposition.tenant_id
         AND reservation.id = disposition.reservation_id
        INNER JOIN inventory_holds hold
          ON hold.tenant_id = shortage.tenant_id
         AND hold.id = shortage.inventory_hold_id
        INNER JOIN shipments shipment
          ON shipment.tenant_id = disposition.tenant_id
         AND shipment.order_id = disposition.order_id
        INNER JOIN shipment_confirmations confirmation
          ON confirmation.tenant_id = shipment.tenant_id
         AND confirmation.shipment_id = shipment.id
        WHERE disposition.tenant_id = $1 AND disposition.id = $2
        "#,
    )
    .bind(short.access.tenant_id.get())
    .bind(accepted.disposition_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    let ordered = row.get::<i64, _>("ordered_qty");
    let accepted_short = row.get::<i64, _>("accepted_short_qty");
    let shipped = row.get::<i64, _>("shipped_qty");
    assert_eq!(ordered, 5);
    assert_eq!(accepted_short, 3);
    assert_eq!(shipped, 2);
    assert_eq!(ordered, shipped + accepted_short);
    assert_eq!(row.get::<i64, _>("departure_delta"), -shipped);
    assert_eq!(row.get::<String, _>("order_status"), "shipped");
    assert_eq!(row.get::<String, _>("shipment_status"), "departed");
    assert_eq!(row.get::<String, _>("reservation_status"), "fulfilled");
    assert_eq!(row.get::<i64, _>("reservation_qty"), ordered);
    assert_eq!(row.get::<String, _>("hold_status"), "active");
    assert_eq!(row.get::<i64, _>("hold_qty"), accepted_short);
    assert_eq!(row.get::<i64, _>("disposition_transaction_count"), 0);
}
