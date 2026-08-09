mod common;

#[allow(dead_code)]
#[path = "api_v1_pick_shortages/support.rs"]
mod shortage_support;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{json, Value};
use shortage_support::{expect_status, response_json, PickShortageFixture};
use sqlx::Row;
use wareboxes_api_contract::v1::{
    CloseCartonResponse, ConfirmShipmentDepartureResponse, CreateCartonResponse,
    CreateShipmentResponse, ErrorReason, ErrorResponse, GeneratePackingSlipResponse,
    ItemSubstitutionPolicyResponse, OpenPackSessionResponse, PackPickedAllocationResponse,
    PickClaimResponse, PickShortageResolution, PickShortageResponse, PickShortageStatus,
    RecordManualManifestResponse, ReportPickShortageResponse, ShipmentOrderStatus,
    SubstitutePickShortageResponse,
};

#[tokio::test]
async fn approved_substitute_creates_normal_pick_work_and_resolves_the_source_shortage() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("wareboxes_api=debug")
        .with_test_writer()
        .try_init();
    let fixture = PickShortageFixture::new("item-substitution-success", 4).await;
    fixture.make_destination_a_packing_station().await;
    fixture.grant_supervisor().await;
    let (substitute_item_id, substitute_barcode) = add_substitute_stock(&fixture, 4).await;
    let policy = configure_policy(
        &fixture,
        "item-substitution-policy",
        json!({
            "inventory_owner_id": fixture.inventory_owner_id,
            "facility_id": fixture.facility_id,
            "source_item_id": fixture.item_id,
            "source_uom": "each",
            "substitute_item_id": substitute_item_id,
            "substitute_uom": "each",
            "source_quantity": 1,
            "substitute_quantity": 1
        }),
    )
    .await;
    assert_eq!(policy.revision.get(), 1);
    assert!(policy.active);

    let report = fixture
        .report(
            Some("item-substitution-report"),
            fixture.no_pick_body("inventory_missing", None),
        )
        .await;
    let report: ReportPickShortageResponse =
        response_json(expect_status(report, StatusCode::OK, "report source shortage").await).await;

    let body = substitution_body(
        policy.policy_id,
        policy.revision.get(),
        report.shortage_revision.get(),
        report.order_revision.get(),
    );
    let response = fixture
        .request(
            Method::POST,
            &format!(
                "/api/v1/pick-shortages/{}/substitutions",
                report.shortage_id
            ),
            Some("item-substitution-execute"),
            Some(body.clone()),
        )
        .await;
    let substituted: SubstitutePickShortageResponse =
        response_json(expect_status(response, StatusCode::OK, "substitute source shortage").await)
            .await;
    assert_eq!(substituted.shortage_id, report.shortage_id);
    assert_eq!(substituted.shortage_revision.get(), 2);
    assert_eq!(substituted.accepted_source_quantity, 4);
    assert_eq!(substituted.substitute_quantity, 4);
    assert_eq!(substituted.substitute_item_id, substitute_item_id);
    assert_eq!(
        substituted.order_revision.get(),
        report.order_revision.get() + 1
    );
    assert_eq!(substituted.work.len(), 1);

    let replay = fixture
        .request(
            Method::POST,
            &format!(
                "/api/v1/pick-shortages/{}/substitutions",
                report.shortage_id
            ),
            Some("item-substitution-execute"),
            Some(body),
        )
        .await;
    assert_eq!(
        response_json::<SubstitutePickShortageResponse>(
            expect_status(replay, StatusCode::OK, "replay substitution").await,
        )
        .await,
        substituted
    );

    let detail = fixture
        .request(
            Method::GET,
            &format!("/api/v1/pick-shortages/{}", report.shortage_id),
            None,
            None,
        )
        .await;
    let detail: PickShortageResponse =
        response_json(expect_status(detail, StatusCode::OK, "read substituted shortage").await)
            .await;
    assert_eq!(detail.status, PickShortageStatus::Resolved);
    assert_eq!(detail.resolution, Some(PickShortageResolution::Substituted));
    assert_eq!(detail.accepted_short_quantity, 0);
    assert_eq!(detail.accepted_substitute_quantity, 4);
    assert_eq!(detail.hold.held_quantity, 4);

    assert_substitution_evidence(&fixture, &substituted).await;
    assert_substitution_governance(&fixture, &policy, &substituted).await;

    let claim = fixture
        .request(
            Method::POST,
            "/api/v1/picking-claims/next",
            Some("item-substitution-claim"),
            Some(json!({})),
        )
        .await;
    let claim: PickClaimResponse = response_json::<Option<PickClaimResponse>>(
        expect_status(claim, StatusCode::OK, "claim substitute pick").await,
    )
    .await
    .expect("substitution created executable pick work");
    assert_eq!(claim.order_id, fixture.order_id);
    assert_eq!(claim.content.item_id, substitute_item_id);
    assert_eq!(claim.content.planned_quantity, 4);

    let confirmation = fixture
        .request(
            Method::POST,
            &format!(
                "/api/v1/picking-tasks/{}/contents/{}/confirmations",
                claim.task_id, claim.content.content_id
            ),
            Some("item-substitution-confirm"),
            Some(json!({
                "source_location_barcode": claim.content.source_location_barcode,
                "item_barcode": substitute_barcode,
                "source_license_plate_barcode": claim.content.source_license_plate_barcode,
                "destination_license_plate_barcode": fixture.destination_plate_barcode
            })),
        )
        .await;
    let confirmation: Value =
        response_json(expect_status(confirmation, StatusCode::OK, "confirm substitute pick").await)
            .await;
    assert_eq!(confirmation["order_status"], "awaiting_packing");
    assert_eq!(confirmation["picked_quantity"], 4);
    let departed = pack_manifest_and_depart_substitution(
        &fixture,
        &substituted,
        confirmation["order_revision"]
            .as_i64()
            .expect("confirmation returns the order revision"),
    )
    .await;
    assert_eq!(departed.order_status, ShipmentOrderStatus::Shipped);
    assert_eq!(departed.demand.ordered_quantity, 8);
    assert_eq!(departed.demand.shipped_quantity, 4);
    assert_eq!(departed.demand.accepted_short_quantity, 0);
    assert_eq!(departed.demand.accepted_substitute_quantity, 4);
}

#[tokio::test]
async fn substitution_policy_versions_retire_and_reenable_with_exact_replay() {
    let fixture = PickShortageFixture::new("item-substitution-policy", 2).await;
    fixture.grant_supervisor().await;
    let (substitute_item_id, _) = add_substitute_stock(&fixture, 2).await;
    let initial_body = policy_body(&fixture, substitute_item_id, 1, 1, None);
    let initial = configure_policy(
        &fixture,
        "item-substitution-policy-initial",
        initial_body.clone(),
    )
    .await;
    let replay = configure_policy(
        &fixture,
        "item-substitution-policy-initial",
        initial_body.clone(),
    )
    .await;
    assert_eq!(replay, initial);
    let mut changed = initial_body;
    changed["substitute_quantity"] = json!(2);
    let changed = fixture
        .request(
            Method::POST,
            "/api/v1/item-substitution-policies",
            Some("item-substitution-policy-initial"),
            Some(changed),
        )
        .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let replacement = configure_policy(
        &fixture,
        "item-substitution-policy-reconfigure",
        policy_body(&fixture, substitute_item_id, 1, 2, Some(1)),
    )
    .await;
    assert_eq!(replacement.revision.get(), 2);
    assert!(replacement.active);

    let history = fixture
        .request(
            Method::GET,
            &format!(
                "/api/v1/item-substitution-policies?inventory_owner_id={}&facility_id={}&active_only=false",
                fixture.inventory_owner_id, fixture.facility_id
            ),
            None,
            None,
        )
        .await;
    let history: Vec<ItemSubstitutionPolicyResponse> = response_json(
        expect_status(history, StatusCode::OK, "list substitution policy history").await,
    )
    .await;
    assert_eq!(history.len(), 2);
    assert_eq!(history.iter().filter(|policy| policy.active).count(), 1);

    let retired = fixture
        .request(
            Method::POST,
            &format!(
                "/api/v1/item-substitution-policies/{}/retirements",
                replacement.policy_id
            ),
            Some("item-substitution-policy-retire"),
            Some(json!({"expected_revision": replacement.revision})),
        )
        .await;
    let retired: ItemSubstitutionPolicyResponse =
        response_json(expect_status(retired, StatusCode::OK, "retire substitution policy").await)
            .await;
    assert!(!retired.active);
    assert!(retired.retired_by.is_some());
    assert!(retired.retired_at.is_some());

    let reenabled = configure_policy(
        &fixture,
        "item-substitution-policy-reenable",
        policy_body(&fixture, substitute_item_id, 1, 1, Some(2)),
    )
    .await;
    assert_eq!(reenabled.revision.get(), 3);
    assert!(reenabled.active);
}

#[tokio::test]
async fn invalid_and_racing_substitutions_have_one_exact_effect_and_fail_closed() {
    let fixture = PickShortageFixture::new("item-substitution-race", 4).await;
    fixture.grant_supervisor().await;
    let (substitute_item_id, _) = add_substitute_stock(&fixture, 3).await;
    let inexact_policy = configure_policy(
        &fixture,
        "item-substitution-race-policy-inexact",
        policy_body(&fixture, substitute_item_id, 3, 2, None),
    )
    .await;
    let report = fixture
        .report(
            Some("item-substitution-race-report"),
            fixture.no_pick_body("inventory_missing", None),
        )
        .await;
    let report: ReportPickShortageResponse =
        response_json(expect_status(report, StatusCode::OK, "report racing shortage").await).await;
    let inexact = fixture
        .request(
            Method::POST,
            &substitution_path(report.shortage_id),
            Some("item-substitution-inexact"),
            Some(substitution_body(
                inexact_policy.policy_id,
                inexact_policy.revision.get(),
                report.shortage_revision.get(),
                report.order_revision.get(),
            )),
        )
        .await;
    assert_eq!(inexact.status(), StatusCode::CONFLICT);
    assert_eq!(substitution_effect_count(&fixture).await, 0);

    let exact_policy = configure_policy(
        &fixture,
        "item-substitution-race-policy-exact",
        policy_body(&fixture, substitute_item_id, 1, 1, Some(1)),
    )
    .await;
    let unavailable = fixture
        .request(
            Method::POST,
            &substitution_path(report.shortage_id),
            Some("item-substitution-insufficient"),
            Some(substitution_body(
                exact_policy.policy_id,
                exact_policy.revision.get(),
                report.shortage_revision.get(),
                report.order_revision.get(),
            )),
        )
        .await;
    assert_eq!(unavailable.status(), StatusCode::CONFLICT);
    assert_eq!(substitution_effect_count(&fixture).await, 0);
    fixture
        .fixture
        .received_balance(
            &fixture.access,
            ReceivedBalanceSetup {
                inventory_owner_id: fixture.inventory_owner_id,
                facility_id: fixture.facility_id,
                item_id: substitute_item_id,
                qty: 1,
                key: "item-substitution-race-extra-stock",
            },
        )
        .await;

    let body = substitution_body(
        exact_policy.policy_id,
        exact_policy.revision.get(),
        report.shortage_revision.get(),
        report.order_revision.get(),
    );
    let path = substitution_path(report.shortage_id);
    let (left, right) = tokio::join!(
        fixture.request(
            Method::POST,
            &path,
            Some("item-substitution-race-left"),
            Some(body.clone())
        ),
        fixture.request(
            Method::POST,
            &path,
            Some("item-substitution-race-right"),
            Some(body.clone())
        )
    );
    let (winner_key, winner, loser) = if left.status() == StatusCode::OK {
        ("item-substitution-race-left", left, right)
    } else {
        ("item-substitution-race-right", right, left)
    };
    assert_eq!(winner.status(), StatusCode::OK);
    assert_eq!(loser.status(), StatusCode::CONFLICT);
    let winner: SubstitutePickShortageResponse = response_json(winner).await;
    assert_eq!(substitution_effect_count(&fixture).await, 1);

    let replay = fixture
        .request(Method::POST, &path, Some(winner_key), Some(body.clone()))
        .await;
    assert_eq!(
        response_json::<SubstitutePickShortageResponse>(
            expect_status(replay, StatusCode::OK, "replay racing winner").await,
        )
        .await,
        winner
    );
    let mut changed = body.clone();
    changed["reason"] = json!("client_authorized");
    let changed = fixture
        .request(Method::POST, &path, Some(winner_key), Some(changed))
        .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    fixture.revoke_scope().await;
    let concealed = fixture
        .request(Method::POST, &path, Some(winner_key), Some(body))
        .await;
    assert_eq!(concealed.status(), StatusCode::NOT_FOUND);
    assert_eq!(substitution_effect_count(&fixture).await, 1);
}

#[tokio::test]
async fn derived_shortage_cannot_substitute_back_into_its_ancestor_item() {
    let fixture = PickShortageFixture::new("item-substitution-cycle", 2).await;
    fixture.grant_supervisor().await;
    let (substitute_item_id, _) = add_substitute_stock(&fixture, 2).await;
    let forward = configure_policy(
        &fixture,
        "item-substitution-cycle-forward",
        policy_body(&fixture, substitute_item_id, 1, 1, None),
    )
    .await;
    let reverse = configure_policy(
        &fixture,
        "item-substitution-cycle-reverse",
        json!({
            "inventory_owner_id": fixture.inventory_owner_id,
            "facility_id": fixture.facility_id,
            "source_item_id": substitute_item_id,
            "source_uom": "each",
            "substitute_item_id": fixture.item_id,
            "substitute_uom": "each",
            "source_quantity": 1,
            "substitute_quantity": 1
        }),
    )
    .await;
    let first_report = fixture
        .report(
            Some("item-substitution-cycle-first-report"),
            fixture.no_pick_body("inventory_missing", None),
        )
        .await;
    let first_report: ReportPickShortageResponse = response_json(
        expect_status(first_report, StatusCode::OK, "report ancestor shortage").await,
    )
    .await;
    let first = fixture
        .request(
            Method::POST,
            &substitution_path(first_report.shortage_id),
            Some("item-substitution-cycle-first"),
            Some(substitution_body(
                forward.policy_id,
                forward.revision.get(),
                first_report.shortage_revision.get(),
                first_report.order_revision.get(),
            )),
        )
        .await;
    expect_status(first, StatusCode::OK, "create derived substitute demand").await;

    let claim = fixture
        .claim_next("item-substitution-cycle-child-claim")
        .await;
    assert_eq!(claim.content.item_id, substitute_item_id);
    let child_report = fixture
        .report_claim(
            &claim,
            "item-substitution-cycle-child-report",
            PickShortageFixture::no_pick_body_for_claim(&claim),
        )
        .await;
    let child_report: ReportPickShortageResponse =
        response_json(expect_status(child_report, StatusCode::OK, "report derived shortage").await)
            .await;
    let cycle = fixture
        .request(
            Method::POST,
            &substitution_path(child_report.shortage_id),
            Some("item-substitution-cycle-reject"),
            Some(substitution_body(
                reverse.policy_id,
                reverse.revision.get(),
                child_report.shortage_revision.get(),
                child_report.order_revision.get(),
            )),
        )
        .await;
    assert_eq!(cycle.status(), StatusCode::CONFLICT);
    assert_eq!(substitution_effect_count(&fixture).await, 1);
    let detail = fixture
        .request(
            Method::GET,
            &format!("/api/v1/pick-shortages/{}", child_report.shortage_id),
            None,
            None,
        )
        .await;
    let detail: PickShortageResponse =
        response_json(expect_status(detail, StatusCode::OK, "read cycle-rejected shortage").await)
            .await;
    assert_eq!(detail.status, PickShortageStatus::AwaitingInventory);
    assert_eq!(detail.resolution, None);
}

async fn configure_policy(
    fixture: &PickShortageFixture,
    key: &str,
    body: Value,
) -> ItemSubstitutionPolicyResponse {
    let response = fixture
        .request(
            Method::POST,
            "/api/v1/item-substitution-policies",
            Some(key),
            Some(body),
        )
        .await;
    response_json(expect_status(response, StatusCode::OK, "configure substitution policy").await)
        .await
}

fn policy_body(
    fixture: &PickShortageFixture,
    substitute_item_id: i64,
    source_quantity: i64,
    substitute_quantity: i64,
    expected_revision: Option<i64>,
) -> Value {
    json!({
        "inventory_owner_id": fixture.inventory_owner_id,
        "facility_id": fixture.facility_id,
        "source_item_id": fixture.item_id,
        "source_uom": "each",
        "substitute_item_id": substitute_item_id,
        "substitute_uom": "each",
        "source_quantity": source_quantity,
        "substitute_quantity": substitute_quantity,
        "expected_revision": expected_revision
    })
}

fn substitution_path(shortage_id: i64) -> String {
    format!("/api/v1/pick-shortages/{shortage_id}/substitutions")
}

fn substitution_body(
    policy_id: i64,
    policy_revision: i64,
    shortage_revision: i64,
    order_revision: i64,
) -> Value {
    json!({
        "policy_id": policy_id,
        "expected_policy_revision": policy_revision,
        "expected_shortage_revision": shortage_revision,
        "expected_order_revision": order_revision,
        "reason": "inventory_unavailable",
        "note": "Approved like-for-like replacement"
    })
}

async fn add_substitute_stock(fixture: &PickShortageFixture, quantity: i64) -> (i64, String) {
    let item_id = fixture
        .fixture
        .item(
            fixture.access.tenant_id,
            "item-substitution substitute item",
            "each",
        )
        .await;
    let barcode = "ITEM-SUBSTITUTION-SUBSTITUTE".to_owned();
    wareboxes_api::repo::items::add_barcode(
        &fixture.fixture.db,
        fixture.access.tenant_id,
        item_id,
        &barcode,
        "code128",
        None,
    )
    .await
    .unwrap();
    let mut tx = tenant_tx(&fixture.fixture.db, fixture.access.tenant_id).await;
    sqlx::query(
        r#"INSERT INTO inventory_owner_items
               (tenant_id,created,inventory_owner_id,item_id)
           VALUES ($1,transaction_timestamp(),$2,$3)"#,
    )
    .bind(fixture.access.tenant_id.get())
    .bind(fixture.inventory_owner_id)
    .bind(item_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    fixture
        .fixture
        .received_balance(
            &fixture.access,
            ReceivedBalanceSetup {
                inventory_owner_id: fixture.inventory_owner_id,
                facility_id: fixture.facility_id,
                item_id,
                qty: quantity,
                key: "item-substitution-alternate-source",
            },
        )
        .await;
    (item_id, barcode)
}

async fn assert_substitution_evidence(
    fixture: &PickShortageFixture,
    result: &SubstitutePickShortageResponse,
) {
    let mut tx = tenant_tx(&fixture.fixture.db, fixture.access.tenant_id).await;
    let row = sqlx::query(
        r#"SELECT substitution.accepted_source_qty,substitution.substitute_qty,
                  substitution.allocation_count,shortage.status,shortage.resolution,
                  shortage.accepted_substitute_qty,orders.status AS order_status,
                  source_reservation.qty AS source_reservation_qty,
                  substitute_reservation.qty AS substitute_reservation_qty,
                  hold.status AS hold_status,hold.qty AS hold_qty
           FROM pick_shortage_substitutions substitution
           JOIN pick_shortages shortage
             ON shortage.tenant_id=substitution.tenant_id
            AND shortage.id=substitution.pick_shortage_id
           JOIN orders ON orders.tenant_id=substitution.tenant_id
                      AND orders.id=substitution.order_id
           JOIN inventory_reservations source_reservation
             ON source_reservation.tenant_id=substitution.tenant_id
            AND source_reservation.id=substitution.source_reservation_id
           JOIN inventory_reservations substitute_reservation
             ON substitute_reservation.tenant_id=substitution.tenant_id
            AND substitute_reservation.id=substitution.substitute_reservation_id
           JOIN inventory_holds hold ON hold.tenant_id=substitution.tenant_id
                                    AND hold.id=shortage.inventory_hold_id
           WHERE substitution.tenant_id=$1 AND substitution.id=$2"#,
    )
    .bind(fixture.access.tenant_id.get())
    .bind(result.substitution_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(row.get::<i64, _>("accepted_source_qty"), 4);
    assert_eq!(row.get::<i64, _>("substitute_qty"), 4);
    assert_eq!(row.get::<i64, _>("allocation_count"), 1);
    assert_eq!(row.get::<String, _>("status"), "resolved");
    assert_eq!(row.get::<String, _>("resolution"), "substituted");
    assert_eq!(row.get::<i64, _>("accepted_substitute_qty"), 4);
    assert_eq!(row.get::<String, _>("order_status"), "processing");
    assert_eq!(row.get::<i64, _>("source_reservation_qty"), 4);
    assert_eq!(row.get::<i64, _>("substitute_reservation_qty"), 4);
    assert_eq!(row.get::<String, _>("hold_status"), "active");
    assert_eq!(row.get::<i64, _>("hold_qty"), 4);
}

async fn substitution_effect_count(fixture: &PickShortageFixture) -> i64 {
    let mut tx = tenant_tx(&fixture.fixture.db, fixture.access.tenant_id).await;
    let count = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pick_shortage_substitutions WHERE tenant_id=$1 AND order_id=$2",
    )
    .bind(fixture.access.tenant_id.get())
    .bind(fixture.order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    count
}

async fn assert_substitution_governance(
    fixture: &PickShortageFixture,
    policy: &ItemSubstitutionPolicyResponse,
    substitution: &SubstitutePickShortageResponse,
) {
    let app_db = app_db_for(&fixture.fixture.db).await;
    let missing_context: (i64, i64) = sqlx::query_as(
        r#"SELECT
               (SELECT COUNT(*) FROM item_substitution_policies WHERE tenant_id=$1),
               (SELECT COUNT(*) FROM pick_shortage_substitutions WHERE tenant_id=$1)"#,
    )
    .bind(fixture.access.tenant_id.get())
    .fetch_one(&app_db)
    .await
    .unwrap();
    assert_eq!(missing_context, (0, 0));
    app_db.close().await;

    let admin = admin_db_for(&fixture.fixture.db).await;
    for table in ["item_substitution_policies", "pick_shortage_substitutions"] {
        let privileges: (bool, bool, bool, bool) = sqlx::query_as(
            r#"SELECT has_table_privilege('wareboxes_app',$1,'SELECT'),
                      has_table_privilege('wareboxes_app',$1,'INSERT'),
                      has_table_privilege('wareboxes_app',$1,'UPDATE'),
                      has_table_privilege('wareboxes_app',$1,'DELETE')"#,
        )
        .bind(table)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(privileges, (true, true, false, false));
        let rls: (bool, bool) = sqlx::query_as(
            "SELECT relrowsecurity,relforcerowsecurity FROM pg_class WHERE oid=$1::regclass",
        )
        .bind(table)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(rls, (true, true));
    }
    let policy_closure_privileges: (bool, bool, bool) = sqlx::query_as(
        r#"SELECT has_column_privilege(
                       'wareboxes_app','item_substitution_policies','effective_to','UPDATE'),
                  has_column_privilege(
                       'wareboxes_app','item_substitution_policies','retired_by_user_id','UPDATE'),
                  has_column_privilege(
                       'wareboxes_app','item_substitution_policies','source_qty','UPDATE')"#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(policy_closure_privileges, (true, true, false));
    assert!(sqlx::query(
        "UPDATE item_substitution_policies SET source_qty=source_qty+1 WHERE tenant_id=$1 AND id=$2",
    )
    .bind(fixture.access.tenant_id.get())
    .bind(policy.policy_id)
    .execute(&admin)
    .await
    .is_err());
    assert!(sqlx::query(
        "UPDATE pick_shortage_substitutions SET substitute_qty=substitute_qty WHERE tenant_id=$1 AND id=$2",
    )
    .bind(fixture.access.tenant_id.get())
    .bind(substitution.substitution_id)
    .execute(&admin)
    .await
    .is_err());
    assert!(
        sqlx::query("DELETE FROM pick_shortage_substitutions WHERE tenant_id=$1 AND id=$2",)
            .bind(fixture.access.tenant_id.get())
            .bind(substitution.substitution_id)
            .execute(&admin)
            .await
            .is_err()
    );
    admin.close().await;
}

async fn pack_manifest_and_depart_substitution(
    fixture: &PickShortageFixture,
    substitution: &SubstitutePickShortageResponse,
    order_revision: i64,
) -> ConfirmShipmentDepartureResponse {
    let opened = fixture
        .open_packing_session(order_revision, "item-substitution-open-pack")
        .await;
    let opened: OpenPackSessionResponse = response_json(
        expect_status(opened, StatusCode::OK, "open substitution packing session").await,
    )
    .await;
    assert_eq!(opened.session.allocations.len(), 1);
    assert_eq!(opened.session.progress.expected_quantity, 4);
    let allocation = &opened.session.allocations[0];
    assert_eq!(allocation.item_id, substitution.substitute_item_id);
    let carton_barcode = "ITEM-SUBSTITUTION-CARTON";
    let created = fixture
        .request(
            Method::POST,
            &format!(
                "/api/v1/packing-sessions/{}/cartons",
                opened.session.session_id
            ),
            Some("item-substitution-carton"),
            Some(json!({
                "carton_barcode": carton_barcode,
                "expected_revision": opened.session.revision
            })),
        )
        .await;
    let created: CreateCartonResponse =
        response_json(expect_status(created, StatusCode::OK, "create substitution carton").await)
            .await;
    let packed = fixture
        .request(
            Method::POST,
            &format!(
                "/api/v1/packing-sessions/{}/cartons/{}/contents",
                opened.session.session_id, created.carton.carton_id
            ),
            Some("item-substitution-pack"),
            Some(json!({
                "inventory_allocation_id": allocation.inventory_allocation_id,
                "item_barcode": allocation.item_barcodes[0],
                "lot_scan": allocation.lot,
                "serial_scan": allocation.serial,
                "source_license_plate_barcode": fixture.destination_plate_barcode,
                "carton_barcode": carton_barcode,
                "expected_revision": created.revision
            })),
        )
        .await;
    let packed: PackPickedAllocationResponse =
        response_json(expect_status(packed, StatusCode::OK, "pack substitute allocation").await)
            .await;
    let closed = fixture
        .request(
            Method::POST,
            &format!(
                "/api/v1/packing-sessions/{}/cartons/{}/closures",
                opened.session.session_id, created.carton.carton_id
            ),
            Some("item-substitution-close"),
            Some(json!({
                "carton_barcode": carton_barcode,
                "measurements": {
                    "weight_grams": 1000,
                    "dimensions": {"length_mm": 300, "width_mm": 200, "height_mm": 150}
                },
                "expected_revision": packed.revision
            })),
        )
        .await;
    let closed: CloseCartonResponse =
        response_json(expect_status(closed, StatusCode::OK, "close substitution carton").await)
            .await;
    assert!(closed.ready_to_manifest);

    configure_shipping_origin(fixture).await;
    let shipment = fixture
        .request(
            Method::POST,
            &format!("/api/v1/orders/{}/shipments", fixture.order_id),
            Some("item-substitution-shipment"),
            Some(json!({
                "packing_session_id": opened.session.session_id,
                "expected_revision": closed.revision
            })),
        )
        .await;
    let shipment: CreateShipmentResponse = response_json(
        expect_status(shipment, StatusCode::OK, "create substitution shipment").await,
    )
    .await;
    assert_eq!(shipment.shipment.demand.ordered_quantity, 8);
    assert_eq!(shipment.shipment.demand.shipped_quantity, 4);
    assert_eq!(shipment.shipment.demand.accepted_short_quantity, 0);
    assert_eq!(shipment.shipment.demand.accepted_substitute_quantity, 4);

    let document = fixture
        .request(
            Method::POST,
            &format!(
                "/api/v1/shipments/{}/documents/packing-slips",
                shipment.shipment.shipment_id
            ),
            Some("item-substitution-packing-slip"),
            Some(json!({"expected_shipment_revision": shipment.shipment.revision})),
        )
        .await;
    let document: GeneratePackingSlipResponse = response_json(
        expect_status(
            document,
            StatusCode::OK,
            "generate substitution packing slip",
        )
        .await,
    )
    .await;
    assert_eq!(document.document.demand.accepted_substitute_quantity, 4);

    let manifest = fixture
        .request(
            Method::POST,
            &format!(
                "/api/v1/shipments/{}/manifests",
                shipment.shipment.shipment_id
            ),
            Some("item-substitution-manifest"),
            Some(json!({
                "carrier_code": "TEST",
                "service_code": "GROUND",
                "manifest_reference": "ITEM-SUBSTITUTION-MANIFEST",
                "carton_tracking_assignments": [{
                    "carton_id": created.carton.carton_id,
                    "tracking_number": "ITEM-SUBSTITUTION-TRACKING"
                }],
                "expected_revision": shipment.shipment.revision
            })),
        )
        .await;
    let manifest: RecordManualManifestResponse = response_json(
        expect_status(manifest, StatusCode::OK, "manifest substitution shipment").await,
    )
    .await;
    let departed = fixture
        .request(
            Method::POST,
            &format!(
                "/api/v1/shipments/{}/departures",
                shipment.shipment.shipment_id
            ),
            Some("item-substitution-depart"),
            Some(json!({
                "scanned_carton_barcodes": [carton_barcode],
                "expected_shipment_revision": manifest.revision,
                "expected_order_revision": shipment.order_revision
            })),
        )
        .await;
    response_json(expect_status(departed, StatusCode::OK, "depart substitution shipment").await)
        .await
}

async fn configure_shipping_origin(fixture: &PickShortageFixture) {
    grant_permission(
        &fixture.fixture.db,
        fixture.access.tenant_id,
        fixture.access.user_id.get(),
        "item-substitution-admin",
        "admin",
    )
    .await;
    let response = fixture
        .request(
            Method::POST,
            &format!(
                "/api/v1/facilities/{}/shipping-origin-configurations",
                fixture.facility_id
            ),
            Some("item-substitution-origin"),
            Some(json!({
                "expected_revision": 1,
                "name": "Outbound office",
                "company": "Wareboxes Test Facility",
                "line1": "100 Distribution Way",
                "line2": "Dock 4",
                "city": "Reno",
                "state": "NV",
                "postal_code": "89502",
                "country": "US",
                "phone": "+1 775 555 0100",
                "email": "shipping@test.local"
            })),
        )
        .await;
    expect_status(
        response,
        StatusCode::OK,
        "configure substitution shipping origin",
    )
    .await;
}

async fn grant_permission(
    db: &wareboxes_persistence_postgres::db::Db,
    tenant_id: wareboxes_domain::TenantId,
    user_id: i64,
    role_name: &str,
    permission_name: &str,
) {
    let role =
        wareboxes_persistence_postgres::roles::add_role(db, tenant_id, role_name, Some(role_name))
            .await
            .unwrap();
    let permission = match wareboxes_persistence_postgres::permissions::find_by_name(
        db,
        tenant_id,
        permission_name,
    )
    .await
    .unwrap()
    {
        Some(permission) => permission.id,
        None => wareboxes_persistence_postgres::permissions::add_permission(
            db,
            tenant_id,
            permission_name,
            Some(permission_name),
        )
        .await
        .unwrap(),
    };
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        db, tenant_id, role, permission,
    )
    .await
    .unwrap());
    assert!(
        wareboxes_persistence_postgres::roles::add_role_to_user(db, tenant_id, user_id, role,)
            .await
            .unwrap()
    );
}
