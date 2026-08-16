mod common;

#[path = "api_v1_replenishment/decision_policy.rs"]
mod decision_policy;
#[path = "api_v1_replenishment/support.rs"]
mod support;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{json, Value};
use sqlx::Row;
use support::*;
use wareboxes_api::repo::inventory::{AllocateInventoryCommand, CreateInventoryReservationCommand};
use wareboxes_api::{auth, routes, state::AppState};
use wareboxes_api_contract::v1::{
    ConfigureReplenishmentPolicyResponse, ErrorReason, ErrorResponse, PlanReplenishmentResponse,
    ReplenishmentClaimHeartbeatResponse, ReplenishmentClaimReleaseResponse,
    ReplenishmentClaimResponse, ReplenishmentConfirmationResponse, ReplenishmentPlanningOutcome,
    ReplenishmentPolicyPage, ReplenishmentPolicyStatus, ReplenishmentQueuePage,
    ReplenishmentWorkStatus, RetireReplenishmentPolicyResponse,
};

#[tokio::test]
async fn policies_are_typed_versioned_replay_safe_paginated_and_retirable() {
    init_test_tracing();
    let rig = ReplenishmentFixture::new("replenishment-policy").await;
    let (first_source, _) = rig.reserve_source("POLICY-A").await;
    let (second_source, _) = rig.reserve_source("POLICY-B").await;

    let missing_key = rig
        .request(
            Method::POST,
            "/api/v1/replenishment-policies",
            None,
            Some(json!({
                "inventory_owner_id": rig.inventory_owner_id,
                "facility_id": rig.facility_id,
                "item_id": rig.item_id,
                "uom": "each",
                "pick_face_location_id": rig.pick_face_location_id,
                "minimum_quantity": 2,
                "target_quantity": 8,
                "reserve_source_location_ids": [first_source]
            })),
        )
        .await;
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);

    for (label, body) in [
        (
            "pick face is a source",
            json!({
                "inventory_owner_id": rig.inventory_owner_id,
                "facility_id": rig.facility_id,
                "item_id": rig.item_id,
                "uom": "each",
                "pick_face_location_id": rig.pick_face_location_id,
                "minimum_quantity": 2,
                "target_quantity": 8,
                "reserve_source_location_ids": [rig.pick_face_location_id]
            }),
        ),
        (
            "invalid thresholds",
            json!({
                "inventory_owner_id": rig.inventory_owner_id,
                "facility_id": rig.facility_id,
                "item_id": rig.item_id,
                "uom": "each",
                "pick_face_location_id": rig.pick_face_location_id,
                "minimum_quantity": 8,
                "target_quantity": 8,
                "reserve_source_location_ids": [first_source]
            }),
        ),
        (
            "invalid uom",
            json!({
                "inventory_owner_id": rig.inventory_owner_id,
                "facility_id": rig.facility_id,
                "item_id": rig.item_id,
                "uom": " each ",
                "pick_face_location_id": rig.pick_face_location_id,
                "minimum_quantity": 2,
                "target_quantity": 8,
                "reserve_source_location_ids": [first_source]
            }),
        ),
    ] {
        let key = format!("policy-invalid-{}", label.replace(' ', "-"));
        let response = rig
            .request(
                Method::POST,
                "/api/v1/replenishment-policies",
                Some(&key),
                Some(body),
            )
            .await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{label}"
        );
    }
    let pickable_source = rig
        .fixture
        .location(
            rig.access.tenant_id,
            rig.facility_id,
            "POLICY-PICKABLE-SOURCE",
        )
        .await;
    let non_executable_source = rig
        .configure(
            "policy-non-executable-source",
            &[pickable_source],
            2,
            8,
            None,
        )
        .await;
    assert_error_reason(
        non_executable_source,
        StatusCode::CONFLICT,
        ErrorReason::Conflict,
        "pickable location cannot be a reserve source",
    )
    .await;
    let invalid_pick_face = rig
        .request(
            Method::POST,
            "/api/v1/replenishment-policies",
            Some("policy-non-executable-pick-face"),
            Some(json!({
                "inventory_owner_id": rig.inventory_owner_id,
                "facility_id": rig.facility_id,
                "item_id": rig.item_id,
                "uom": "each",
                "pick_face_location_id": first_source,
                "minimum_quantity": 2,
                "target_quantity": 8,
                "reserve_source_location_ids": [second_source]
            })),
        )
        .await;
    assert_error_reason(
        invalid_pick_face,
        StatusCode::CONFLICT,
        ErrorReason::Conflict,
        "reserve location cannot be a pick face",
    )
    .await;
    assert_eq!(rig.effect_counts().await.policies, 0);

    let configured = rig
        .configure(
            "policy-configure",
            &[second_source, first_source, second_source],
            2,
            8,
            None,
        )
        .await;
    let configured: ConfigureReplenishmentPolicyResponse = response_json(
        expect_status(configured, StatusCode::OK, "configure replenishment policy").await,
    )
    .await;
    assert_eq!(configured.revision.get(), 1);
    assert_eq!(configured.status, ReplenishmentPolicyStatus::Active);
    assert_eq!(
        configured.reserve_source_location_ids.as_slice(),
        &[
            first_source.min(second_source),
            first_source.max(second_source)
        ]
    );

    let replay = rig
        .configure(
            "policy-configure",
            &[second_source, first_source],
            2,
            8,
            None,
        )
        .await;
    assert_eq!(
        response_json::<ConfigureReplenishmentPolicyResponse>(
            expect_status(replay, StatusCode::OK, "replay policy configuration").await,
        )
        .await,
        configured
    );
    let changed_hash = rig
        .configure("policy-configure", &[first_source], 2, 9, Some(1))
        .await;
    assert_error_reason(
        changed_hash,
        StatusCode::CONFLICT,
        ErrorReason::IdempotencyKeyReused,
        "policy configuration key reuse",
    )
    .await;
    let stale = rig
        .configure(
            "policy-stale-revision",
            &[first_source, second_source],
            3,
            9,
            Some(2),
        )
        .await;
    assert_error_reason(
        stale,
        StatusCode::CONFLICT,
        ErrorReason::Conflict,
        "stale policy revision",
    )
    .await;

    let replacement = rig
        .configure("policy-reconfigure", &[second_source], 3, 9, Some(1))
        .await;
    let replacement: ConfigureReplenishmentPolicyResponse =
        response_json(expect_status(replacement, StatusCode::OK, "replace policy").await).await;
    assert_ne!(replacement.policy_id, configured.policy_id);
    assert_eq!(replacement.previous_revision.unwrap().get(), 1);
    assert_eq!(replacement.revision.get(), 2);

    let second = rig.policy_dimensions("POLICY-SECOND").await;
    let second_policy = rig
        .configure_for(
            second.item_id,
            second.pick_face_location_id,
            "policy-second-configure",
            &[first_source],
            1,
            4,
            None,
        )
        .await;
    let second_policy: ConfigureReplenishmentPolicyResponse = response_json(
        expect_status(second_policy, StatusCode::OK, "configure second policy").await,
    )
    .await;
    let first_page = rig
        .request(
            Method::GET,
            "/api/v1/replenishment-policies?limit=1&sort=target_gap&direction=descending",
            None,
            None,
        )
        .await;
    let first_page: ReplenishmentPolicyPage =
        response_json(expect_status(first_page, StatusCode::OK, "first policy page").await).await;
    assert_eq!(first_page.items.len(), 1);
    let cursor = first_page.next_cursor.expect("another active policy");
    let second_page = rig
        .request(
            Method::GET,
            &format!(
                "/api/v1/replenishment-policies?limit=1&sort=target_gap&direction=descending&cursor={}",
                cursor.as_str()
            ),
            None,
            None,
        )
        .await;
    let second_page_result: ReplenishmentPolicyPage =
        response_json(expect_status(second_page, StatusCode::OK, "second policy page").await).await;
    assert_eq!(second_page_result.items.len(), 1);
    assert_ne!(
        second_page_result.items[0].policy_id,
        first_page.items[0].policy_id
    );
    assert!(
        first_page.items[0].target_gap >= second_page_result.items[0].target_gap,
        "target gap sort must be applied before pagination"
    );
    let mismatch = rig
        .request(
            Method::GET,
            &format!(
                "/api/v1/replenishment-policies?limit=1&sort=target_gap&direction=ascending&cursor={}",
                cursor.as_str()
            ),
            None,
            None,
        )
        .await;
    assert_error_reason(
        mismatch,
        StatusCode::BAD_REQUEST,
        ErrorReason::InvalidCursor,
        "policy cursor filter mismatch",
    )
    .await;

    let retired = rig
        .request(
            Method::POST,
            &format!(
                "/api/v1/replenishment-policies/{}/retirements",
                second_policy.policy_id
            ),
            Some("policy-retire"),
            Some(json!({"expected_revision": 1})),
        )
        .await;
    let retired: RetireReplenishmentPolicyResponse =
        response_json(expect_status(retired, StatusCode::OK, "retire idle policy").await).await;
    assert_eq!(retired.status, ReplenishmentPolicyStatus::Retired);
    let retired_replay = rig
        .request(
            Method::POST,
            &format!(
                "/api/v1/replenishment-policies/{}/retirements",
                second_policy.policy_id
            ),
            Some("policy-retire"),
            Some(json!({"expected_revision": 1})),
        )
        .await;
    assert_eq!(
        response_json::<RetireReplenishmentPolicyResponse>(
            expect_status(retired_replay, StatusCode::OK, "replay policy retirement").await,
        )
        .await,
        retired
    );
    assert_eq!(replacement.status, ReplenishmentPolicyStatus::Active);
}

#[tokio::test]
async fn planning_uses_live_demand_inbound_projection_and_deterministic_fefo() {
    init_test_tracing();
    let rig = ReplenishmentFixture::new("replenishment-plan").await;
    let (early_location, early_barcode) = rig.reserve_source("PLAN-EARLY").await;
    let (late_location, late_barcode) = rig.reserve_source("PLAN-LATE").await;
    let order_id = rig
        .fixture
        .order_header(
            rig.access.tenant_id,
            "REPLENISHMENT-DEMAND",
            rig.inventory_owner_id,
        )
        .await;
    let order_item_id = rig
        .fixture
        .order_item(rig.access.tenant_id, order_id, rig.item_id, 9)
        .await;
    let reservation = repo::inventory::create_inventory_reservation(
        &rig.fixture.db,
        &rig.access,
        &CreateInventoryReservationCommand {
            order_id,
            order_item_id,
            facility_id: rig.facility_id,
            qty: 9,
            idempotency_key: "replenishment-demand-reserve",
        },
    )
    .await
    .unwrap();
    let late = rig
        .seed_stock(
            late_location,
            &late_barcode,
            6,
            "LOT-LATE",
            Some("2027-02-01T00:00:00Z"),
            "plan-late-stock",
        )
        .await;
    let early = rig
        .seed_stock(
            early_location,
            &early_barcode,
            3,
            "LOT-EARLY",
            Some("2027-01-01T00:00:00Z"),
            "plan-early-stock",
        )
        .await;
    let configured = rig
        .configure(
            "plan-configure",
            &[late_location, early_location],
            2,
            7,
            None,
        )
        .await;
    let configured: ConfigureReplenishmentPolicyResponse =
        response_json(expect_status(configured, StatusCode::OK, "configure planning policy").await)
            .await;

    let plan = rig.plan(configured.policy_id, 1, "plan-full-demand").await;
    let plan: PlanReplenishmentResponse =
        response_json(expect_status(plan, StatusCode::OK, "plan full demand replenishment").await)
            .await;
    assert_eq!(plan.snapshot.pick_face_free, 0);
    assert_eq!(plan.snapshot.active_inbound, 0);
    assert_eq!(plan.snapshot.projected_free, 0);
    assert_eq!(plan.snapshot.unallocated_demand, 9);
    assert_eq!(plan.snapshot.reserve_free, 9);
    assert_eq!(plan.required_level, 9);
    assert_eq!(plan.target_gap, 9);
    assert_eq!(plan.planned_quantity, 9);
    assert_eq!(plan.remaining_quantity, 0);
    assert_eq!(plan.outcome, ReplenishmentPlanningOutcome::FullyPlanned);
    assert_eq!(plan.work.len(), 2);
    assert_eq!(plan.work[0].source_inventory_balance_id, early.balance_id);
    assert_eq!(plan.work[0].quantity, 3);
    assert_eq!(plan.work[1].source_inventory_balance_id, late.balance_id);
    assert_eq!(plan.work[1].quantity, 6);
    assert_eq!(plan.work[0].sequence, 1);
    assert_eq!(plan.work[1].sequence, 2);

    let replay = rig.plan(configured.policy_id, 1, "plan-full-demand").await;
    assert_eq!(
        response_json::<PlanReplenishmentResponse>(
            expect_status(replay, StatusCode::OK, "replay replenishment plan").await,
        )
        .await,
        plan
    );
    let changed_hash = rig.plan(configured.policy_id, 2, "plan-full-demand").await;
    assert_error_reason(
        changed_hash,
        StatusCode::CONFLICT,
        ErrorReason::IdempotencyKeyReused,
        "plan key reuse",
    )
    .await;
    let second_plan = rig.plan(configured.policy_id, 1, "plan-zero").await;
    let second_plan: PlanReplenishmentResponse =
        response_json(expect_status(second_plan, StatusCode::OK, "plan with active inbound").await)
            .await;
    assert_eq!(second_plan.snapshot.active_inbound, 9);
    assert_eq!(second_plan.snapshot.projected_free, 9);
    assert_eq!(second_plan.snapshot.unallocated_demand, 9);
    assert_eq!(second_plan.target_gap, 0);
    assert_eq!(second_plan.planned_quantity, 0);
    assert_eq!(second_plan.outcome, ReplenishmentPlanningOutcome::NotNeeded);
    assert!(second_plan.work.is_empty());

    let partial = rig.policy_dimensions("PLAN-PARTIAL").await;
    let (partial_source, partial_source_barcode) = rig.reserve_source("PLAN-PARTIAL").await;
    rig.seed_item_stock(
        partial.item_id,
        partial_source,
        &partial_source_barcode,
        4,
        "LOT-PARTIAL",
        None,
        "plan-partial-stock",
    )
    .await;
    let partial_policy = rig
        .configure_for(
            partial.item_id,
            partial.pick_face_location_id,
            "plan-partial-configure",
            &[partial_source],
            5,
            10,
            None,
        )
        .await;
    let partial_policy: ConfigureReplenishmentPolicyResponse = response_json(
        expect_status(partial_policy, StatusCode::OK, "configure partial policy").await,
    )
    .await;
    let partial_plan = rig.plan(partial_policy.policy_id, 1, "plan-partial").await;
    let partial_plan: PlanReplenishmentResponse = response_json(
        expect_status(partial_plan, StatusCode::OK, "plan partial replenishment").await,
    )
    .await;
    assert_eq!(
        partial_plan.outcome,
        ReplenishmentPlanningOutcome::PartiallyPlanned
    );
    assert_eq!(partial_plan.planned_quantity, 4);
    assert_eq!(partial_plan.remaining_quantity, 6);

    let empty = rig.policy_dimensions("PLAN-EMPTY").await;
    let (empty_source, _) = rig.reserve_source("PLAN-EMPTY").await;
    let empty_policy = rig
        .configure_for(
            empty.item_id,
            empty.pick_face_location_id,
            "plan-empty-configure",
            &[empty_source],
            1,
            5,
            None,
        )
        .await;
    let empty_policy: ConfigureReplenishmentPolicyResponse =
        response_json(expect_status(empty_policy, StatusCode::OK, "configure empty policy").await)
            .await;
    let empty_plan = rig.plan(empty_policy.policy_id, 1, "plan-empty").await;
    let empty_plan: PlanReplenishmentResponse =
        response_json(expect_status(empty_plan, StatusCode::OK, "plan without reserve").await)
            .await;
    assert_eq!(
        empty_plan.outcome,
        ReplenishmentPlanningOutcome::InsufficientReserve
    );
    assert_eq!(empty_plan.planned_quantity, 0);
    assert_eq!(empty_plan.remaining_quantity, 5);

    let policies = rig
        .request(
            Method::GET,
            &format!("/api/v1/replenishment-policies?item_id={}", rig.item_id),
            None,
            None,
        )
        .await;
    let policies: ReplenishmentPolicyPage =
        response_json(expect_status(policies, StatusCode::OK, "live policy readiness").await).await;
    assert_eq!(policies.items.len(), 1);
    assert_eq!(policies.items[0].snapshot.active_inbound, 9);
    assert_eq!(policies.items[0].active_work_count, 2);
    assert_eq!(policies.items[0].active_work_quantity, 9);
    assert_eq!(
        policies.items[0].latest_plan.as_ref().unwrap().outcome,
        ReplenishmentPlanningOutcome::NotNeeded
    );

    let first_queue = rig
        .request(
            Method::GET,
            "/api/v1/replenishment-queue?limit=1",
            None,
            None,
        )
        .await;
    let first_queue: ReplenishmentQueuePage = response_json(
        expect_status(
            first_queue,
            StatusCode::OK,
            "first replenishment queue page",
        )
        .await,
    )
    .await;
    assert_eq!(first_queue.items.len(), 1);
    let cursor = first_queue.next_cursor.expect("queue has another task");
    let second_queue = rig
        .request(
            Method::GET,
            &format!(
                "/api/v1/replenishment-queue?limit=1&cursor={}",
                cursor.as_str()
            ),
            None,
            None,
        )
        .await;
    let second_queue: ReplenishmentQueuePage = response_json(
        expect_status(
            second_queue,
            StatusCode::OK,
            "second replenishment queue page",
        )
        .await,
    )
    .await;
    assert_eq!(second_queue.items.len(), 1);
    assert_ne!(second_queue.items[0].work_id, first_queue.items[0].work_id);

    let quantity_queue = rig
        .request(
            Method::GET,
            "/api/v1/replenishment-queue?limit=1&sort=quantity&direction=descending",
            None,
            None,
        )
        .await;
    let quantity_queue: ReplenishmentQueuePage = response_json(
        expect_status(
            quantity_queue,
            StatusCode::OK,
            "quantity-sorted replenishment queue",
        )
        .await,
    )
    .await;
    assert_eq!(quantity_queue.items[0].quantity, 6);
    let quantity_cursor = quantity_queue.next_cursor.as_ref().unwrap();
    let sort_mismatch = rig
        .request(
            Method::GET,
            &format!(
                "/api/v1/replenishment-queue?limit=1&sort=quantity&direction=ascending&cursor={quantity_cursor}"
            ),
            None,
            None,
        )
        .await;
    assert_error_reason(
        sort_mismatch,
        StatusCode::BAD_REQUEST,
        ErrorReason::InvalidCursor,
        "queue cursor sort mismatch",
    )
    .await;
    let mismatch = rig
        .request(
            Method::GET,
            &format!(
                "/api/v1/replenishment-queue?limit=1&status=claimed&cursor={}",
                cursor.as_str()
            ),
            None,
            None,
        )
        .await;
    assert_error_reason(
        mismatch,
        StatusCode::BAD_REQUEST,
        ErrorReason::InvalidCursor,
        "queue cursor filter mismatch",
    )
    .await;
    assert!(reservation.reservation_id > 0);
}

#[tokio::test]
async fn claims_require_exact_scans_and_confirmation_is_a_conserved_unallocated_move() {
    init_test_tracing();
    let rig = ReplenishmentFixture::new("replenishment-confirm").await;
    let (source_location, source_barcode) = rig.reserve_source("CONFIRM").await;
    let source = rig
        .seed_item_stock_with_serial(
            rig.item_id,
            source_location,
            &source_barcode,
            8,
            "LOT-CONFIRM",
            None,
            None,
            "confirm-source-stock",
        )
        .await;
    let policy = rig
        .configure("confirm-configure", &[source_location], 2, 5, None)
        .await;
    let policy: ConfigureReplenishmentPolicyResponse =
        response_json(expect_status(policy, StatusCode::OK, "configure confirmation policy").await)
            .await;
    let plan = rig.plan(policy.policy_id, 1, "confirm-plan").await;
    let plan: PlanReplenishmentResponse =
        response_json(expect_status(plan, StatusCode::OK, "plan confirmation work").await).await;
    assert_eq!(plan.work.len(), 1);
    let work_id = plan.work[0].work_id;

    let claim = rig
        .request(
            Method::POST,
            "/api/v1/replenishment-claims/next",
            Some("confirm-claim-next"),
            Some(json!({})),
        )
        .await;
    let claim: ReplenishmentClaimResponse = response_json::<Option<_>>(
        expect_status(claim, StatusCode::OK, "claim next replenishment").await,
    )
    .await
    .expect("planned replenishment is claimable");
    assert_eq!(claim.work_id, work_id);
    assert_eq!(claim.quantity, 5);
    assert_eq!(claim.source_location.barcode, source.location_barcode);
    assert_eq!(claim.destination_pick_face.barcode, rig.pick_face_barcode);
    assert_eq!(claim.item_barcodes, vec![rig.item_barcode.clone()]);

    let current = rig
        .request(
            Method::GET,
            "/api/v1/replenishment-claims/current",
            None,
            None,
        )
        .await;
    assert_eq!(
        response_json::<Option<ReplenishmentClaimResponse>>(
            expect_status(current, StatusCode::OK, "current replenishment claim").await,
        )
        .await,
        Some(claim.clone())
    );
    let heartbeat = rig
        .request(
            Method::POST,
            &format!("/api/v1/replenishment-claims/{work_id}/heartbeats"),
            Some("confirm-heartbeat"),
            Some(json!({})),
        )
        .await;
    let heartbeat: ReplenishmentClaimHeartbeatResponse = response_json(
        expect_status(heartbeat, StatusCode::OK, "heartbeat replenishment claim").await,
    )
    .await;
    assert_eq!(heartbeat.work_id, work_id);

    let released = rig
        .request(
            Method::POST,
            &format!("/api/v1/replenishment-claims/{work_id}/releases"),
            Some("confirm-release"),
            Some(json!({
                "reason": "inventory_mismatch",
                "note": "operator recounted reserve stock"
            })),
        )
        .await;
    let released: ReplenishmentClaimReleaseResponse =
        response_json(expect_status(released, StatusCode::OK, "release replenishment claim").await)
            .await;
    assert_eq!(released.status, ReplenishmentWorkStatus::Pending);
    assert_eq!(released.release_count, 1);
    let current = rig
        .request(
            Method::GET,
            "/api/v1/replenishment-claims/current",
            None,
            None,
        )
        .await;
    assert!(response_json::<Option<ReplenishmentClaimResponse>>(
        expect_status(current, StatusCode::OK, "no current claim after release").await,
    )
    .await
    .is_none());
    let claim = rig.claim_by_id(work_id, "confirm-claim-by-id").await;
    let claim: ReplenishmentClaimResponse =
        response_json(expect_status(claim, StatusCode::OK, "claim replenishment by ID").await)
            .await;

    let before_invalid = rig.effect_counts().await;
    for (label, field, value) in [
        (
            "wrong source",
            "source_location_barcode",
            json!("WRONG-SOURCE"),
        ),
        ("wrong item", "item_barcode", json!("WRONG-ITEM")),
        ("wrong lot", "lot_scan", json!("WRONG-LOT")),
        ("wrong serial", "serial_scan", json!("WRONG-SERIAL")),
        (
            "wrong destination",
            "destination_pick_face_barcode",
            json!("WRONG-PICK-FACE"),
        ),
    ] {
        let mut body = rig.exact_scans(&claim);
        body[field] = value;
        let response = rig
            .confirm(&claim, &format!("confirm-invalid-{field}"), body)
            .await;
        assert_error_reason(
            response,
            StatusCode::BAD_REQUEST,
            ErrorReason::InvalidRequest,
            label,
        )
        .await;
        before_invalid.assert_unchanged(&rig.effect_counts().await, label);
    }

    let exact_scans = rig.exact_scans(&claim);
    let confirmed = rig
        .confirm(&claim, "confirm-exact", exact_scans.clone())
        .await;
    let confirmed: ReplenishmentConfirmationResponse =
        response_json(expect_status(confirmed, StatusCode::OK, "confirm replenishment").await)
            .await;
    assert_eq!(confirmed.work_id, work_id);
    assert_eq!(confirmed.source_inventory_balance_id, source.balance_id);
    assert_eq!(confirmed.source_location_id, source.location_id);
    assert_eq!(
        confirmed.destination_pick_face_location_id,
        rig.pick_face_location_id
    );
    assert_eq!(confirmed.quantity, 5);
    assert_eq!(confirmed.work_status, ReplenishmentWorkStatus::Completed);
    let replay = rig
        .confirm(&claim, "confirm-exact", exact_scans.clone())
        .await;
    assert_eq!(
        response_json::<ReplenishmentConfirmationResponse>(
            expect_status(replay, StatusCode::OK, "replay replenishment confirmation").await,
        )
        .await,
        confirmed
    );
    let mut changed = exact_scans;
    changed["item_barcode"] = json!("DIFFERENT");
    let changed = rig.confirm(&claim, "confirm-exact", changed).await;
    assert_error_reason(
        changed,
        StatusCode::CONFLICT,
        ErrorReason::IdempotencyKeyReused,
        "confirmation key reuse",
    )
    .await;

    let mut tx = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    let movement = sqlx::query(
        r#"
        SELECT
          (SELECT transaction_type FROM inventory_transactions
           WHERE tenant_id = $1 AND id = $2) transaction_type,
          (SELECT operation FROM inventory_transactions
           WHERE tenant_id = $1 AND id = $2) operation,
          (SELECT reference_type FROM inventory_transactions
           WHERE tenant_id = $1 AND id = $2) reference_type,
          (SELECT reference_id FROM inventory_transactions
           WHERE tenant_id = $1 AND id = $2) reference_id,
          (SELECT COUNT(*) FROM inventory_entries
           WHERE tenant_id = $1 AND transaction_id = $2) entry_count,
          (SELECT SUM(quantity_delta)::BIGINT FROM inventory_entries
           WHERE tenant_id = $1 AND transaction_id = $2) net_quantity,
          (SELECT qty_on_hand FROM inventory_balances
           WHERE tenant_id = $1 AND id = $3) source_on_hand,
          (SELECT qty_on_hand FROM inventory_balances
           WHERE tenant_id = $1 AND id = $4) destination_on_hand,
          (SELECT COUNT(*) FROM inventory_allocations WHERE tenant_id = $1) allocation_count,
          (SELECT COUNT(*) FROM work_task_progress
           WHERE tenant_id = $1 AND task_id = $5 AND action = 'replenishment_confirmed') progress_count,
          (SELECT COUNT(*) FROM outbox_events
           WHERE tenant_id = $1 AND aggregate_id = $5::TEXT
             AND event_type = 'inventory.replenishment.confirmed') event_count
        "#,
    )
    .bind(rig.access.tenant_id.get())
    .bind(confirmed.inventory_transaction_id)
    .bind(source.balance_id)
    .bind(confirmed.destination_inventory_balance_id)
    .bind(work_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(movement.get::<String, _>("transaction_type"), "move");
    assert_eq!(
        movement.get::<String, _>("operation"),
        "task.confirm_replenishment.v1"
    );
    assert_eq!(
        movement
            .get::<Option<String>, _>("reference_type")
            .as_deref(),
        Some("replenishment_task")
    );
    assert_eq!(
        movement.get::<Option<i64>, _>("reference_id"),
        Some(work_id)
    );
    assert_eq!(movement.get::<i64, _>("entry_count"), 2);
    assert_eq!(movement.get::<i64, _>("net_quantity"), 0);
    assert_eq!(movement.get::<i64, _>("source_on_hand"), 3);
    assert_eq!(movement.get::<i64, _>("destination_on_hand"), 5);
    assert_eq!(movement.get::<i64, _>("allocation_count"), 0);
    assert_eq!(movement.get::<i64, _>("progress_count"), 1);
    assert_eq!(movement.get::<i64, _>("event_count"), 1);
    tx.rollback().await.unwrap();

    let order_id = rig
        .fixture
        .order_header(
            rig.access.tenant_id,
            "REPLENISHMENT-LATER-ALLOCATION",
            rig.inventory_owner_id,
        )
        .await;
    let order_item_id = rig
        .fixture
        .order_item(rig.access.tenant_id, order_id, rig.item_id, 2)
        .await;
    let reservation = repo::inventory::create_inventory_reservation(
        &rig.fixture.db,
        &rig.access,
        &CreateInventoryReservationCommand {
            order_id,
            order_item_id,
            facility_id: rig.facility_id,
            qty: 2,
            idempotency_key: "replenishment-later-reservation",
        },
    )
    .await
    .unwrap();
    let allocated = repo::inventory::allocate_inventory(
        &rig.fixture.db,
        &rig.access,
        &AllocateInventoryCommand {
            reservation_id: reservation.reservation_id,
            inventory_balance_id: confirmed.destination_inventory_balance_id,
            qty: 2,
            idempotency_key: "replenishment-later-allocation",
        },
    )
    .await
    .unwrap();
    assert!(allocated.allocation_id > 0);
    assert_eq!(rig.effect_counts().await.allocations, 1);
}

#[tokio::test]
async fn replenishment_enforces_scope_rls_immutability_and_cross_work_exclusion() {
    init_test_tracing();
    let rig = ReplenishmentFixture::new("replenishment-boundaries").await;
    let (block_source, block_barcode) = rig.reserve_source("BOUNDARY-BLOCK").await;
    rig.seed_stock(
        block_source,
        &block_barcode,
        6,
        "LOT-BLOCK",
        None,
        "boundary-block-stock",
    )
    .await;
    let blocked_policy = rig
        .configure("boundary-block-configure", &[block_source], 2, 5, None)
        .await;
    let blocked_policy: ConfigureReplenishmentPolicyResponse = response_json(
        expect_status(blocked_policy, StatusCode::OK, "configure blocked policy").await,
    )
    .await;
    let blocked_plan = rig
        .plan(blocked_policy.policy_id, 1, "boundary-block-plan")
        .await;
    let blocked_plan: PlanReplenishmentResponse = response_json(
        expect_status(blocked_plan, StatusCode::OK, "plan active inbound work").await,
    )
    .await;
    assert_eq!(blocked_plan.work.len(), 1);
    for (label, path, body) in [
        (
            "reconfigure",
            "/api/v1/replenishment-policies".to_owned(),
            json!({
                "inventory_owner_id": rig.inventory_owner_id,
                "facility_id": rig.facility_id,
                "item_id": rig.item_id,
                "uom": "each",
                "pick_face_location_id": rig.pick_face_location_id,
                "minimum_quantity": 1,
                "target_quantity": 4,
                "reserve_source_location_ids": [block_source],
                "expected_revision": 1
            }),
        ),
        (
            "retire",
            format!(
                "/api/v1/replenishment-policies/{}/retirements",
                blocked_policy.policy_id
            ),
            json!({"expected_revision": 1}),
        ),
    ] {
        let response = rig
            .request(
                Method::POST,
                &path,
                Some(&format!("boundary-block-{label}")),
                Some(body),
            )
            .await;
        assert_error_reason(
            response,
            StatusCode::CONFLICT,
            ErrorReason::Conflict,
            &format!("active work blocks policy {label}"),
        )
        .await;
    }

    let race_dimensions = rig.policy_dimensions("BOUNDARY-RACE").await;
    let (race_source, race_barcode) = rig.reserve_source("BOUNDARY-RACE").await;
    let race_stock = rig
        .seed_item_stock(
            race_dimensions.item_id,
            race_source,
            &race_barcode,
            7,
            "LOT-RACE",
            None,
            "boundary-race-stock",
        )
        .await;
    let race_policy = rig
        .configure_for(
            race_dimensions.item_id,
            race_dimensions.pick_face_location_id,
            "boundary-race-configure",
            &[race_source],
            2,
            5,
            None,
        )
        .await;
    let race_policy: ConfigureReplenishmentPolicyResponse =
        response_json(expect_status(race_policy, StatusCode::OK, "configure race policy").await)
            .await;
    let plan = rig.plan(race_policy.policy_id, 1, "boundary-race-plan");
    let relocation = rig.request(
        Method::POST,
        "/api/v1/inventory-relocation-tasks",
        Some("boundary-race-relocation"),
        Some(json!({
            "work": {
                "workflow": "loose_balance",
                "source_inventory_balance_id": race_stock.balance_id,
                "quantity": 5
            },
            "destination_location_id": race_dimensions.pick_face_location_id
        })),
    );
    let (plan, relocation) = tokio::join!(plan, relocation);
    let plan: PlanReplenishmentResponse =
        response_json(expect_status(plan, StatusCode::OK, "concurrent replenishment plan").await)
            .await;
    if plan.work.is_empty() {
        assert_eq!(
            relocation.status(),
            StatusCode::OK,
            "relocation won, so planning records a zero-work snapshot"
        );
        assert_eq!(
            plan.outcome,
            ReplenishmentPlanningOutcome::InsufficientReserve
        );
        assert_eq!(plan.planned_quantity, 0);
    } else {
        assert_eq!(plan.work.len(), 1);
        assert_eq!(
            plan.work[0].source_inventory_balance_id,
            race_stock.balance_id
        );
        assert_eq!(
            relocation.status(),
            StatusCode::CONFLICT,
            "replenishment won, so relocation cannot claim the same source"
        );
    }
    let mut tx = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    let active_claims: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM loose_inventory_movement_claims
        WHERE tenant_id = $1
          AND source_inventory_balance_id = $2
          AND released_at IS NULL
        "#,
    )
    .bind(rig.access.tenant_id.get())
    .bind(race_stock.balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(active_claims, 1);

    let foreign_operator = rig
        .fixture
        .wms_user("replenishment-boundaries-foreign@test.local")
        .await;
    let foreign_access = default_tenant_for_user(&rig.fixture.db, foreign_operator.id)
        .await
        .unwrap();
    grant_permission(
        &rig.fixture.db,
        foreign_access.tenant_id,
        foreign_operator.id,
        "replenishment-boundaries-foreign-supervisor",
        "wms_supervisor",
    )
    .await;
    let foreign_token = auth::create_session(&rig.fixture.db, foreign_operator.id)
        .await
        .unwrap();
    let cross_tenant = send(
        &rig.app,
        &foreign_token,
        foreign_access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/replenishment-policies/{}/plan-runs",
            blocked_policy.policy_id
        ),
        Some("boundary-cross-tenant-plan"),
        Some(json!({"expected_policy_revision": 1})),
    )
    .await;
    assert_error_reason(
        cross_tenant,
        StatusCode::NOT_FOUND,
        ErrorReason::NotFound,
        "cross-tenant policy ID is concealed",
    )
    .await;

    rig.set_scope(Vec::new(), vec![rig.inventory_owner_id])
        .await;
    let denied_plan = rig
        .plan(blocked_policy.policy_id, 1, "boundary-facility-denied-plan")
        .await;
    assert_error_reason(
        denied_plan,
        StatusCode::NOT_FOUND,
        ErrorReason::NotFound,
        "facility-scoped plan is concealed",
    )
    .await;
    for revision in [1, 2] {
        let denied_replay = rig
            .plan(blocked_policy.policy_id, revision, "boundary-block-plan")
            .await;
        assert_error_reason(
            denied_replay,
            StatusCode::NOT_FOUND,
            ErrorReason::NotFound,
            "scope-revoked exact and changed plan replays are concealed",
        )
        .await;
    }
    let denied_claim = rig
        .claim_by_id(
            blocked_plan.work[0].work_id,
            "boundary-facility-denied-claim",
        )
        .await;
    assert_error_reason(
        denied_claim,
        StatusCode::NOT_FOUND,
        ErrorReason::NotFound,
        "facility-scoped work is concealed",
    )
    .await;
    let denied_queue = rig
        .request(
            Method::GET,
            &format!(
                "/api/v1/replenishment-queue?facility_id={}",
                rig.facility_id
            ),
            None,
            None,
        )
        .await;
    assert_eq!(denied_queue.status(), StatusCode::FORBIDDEN);

    rig.set_scope(vec![rig.facility_id], Vec::new()).await;
    let owner_denied_plan = rig
        .plan(blocked_policy.policy_id, 1, "boundary-owner-denied-plan")
        .await;
    assert_error_reason(
        owner_denied_plan,
        StatusCode::NOT_FOUND,
        ErrorReason::NotFound,
        "owner-scoped plan is concealed",
    )
    .await;

    let app_db = app_db_for(&rig.fixture.db).await;
    let unbound_counts: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM replenishment_policies),
          (SELECT COUNT(*) FROM replenishment_policy_sources),
          (SELECT COUNT(*) FROM replenishment_plan_runs),
          (SELECT COUNT(*) FROM replenishment_tasks),
          (SELECT COUNT(*) FROM replenishment_confirmations)
        "#,
    )
    .fetch_one(&app_db)
    .await
    .unwrap();
    assert_eq!(unbound_counts, (0, 0, 0, 0, 0));
    let grants: (bool, bool, bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT
          has_table_privilege(current_user, 'replenishment_policies', 'SELECT'),
          has_table_privilege(current_user, 'replenishment_policies', 'INSERT'),
          has_column_privilege(current_user, 'replenishment_policies', 'effective_to', 'UPDATE'),
          has_column_privilege(current_user, 'replenishment_policies', 'minimum_qty', 'UPDATE'),
          has_table_privilege(current_user, 'replenishment_plan_runs', 'UPDATE'),
          has_table_privilege(current_user, 'replenishment_confirmations', 'DELETE')
        "#,
    )
    .fetch_one(&app_db)
    .await
    .unwrap();
    assert_eq!(grants, (true, true, true, false, false, false));

    let mutation = sqlx::query(
        "UPDATE replenishment_plan_runs SET planned_qty = planned_qty + 1 WHERE tenant_id = $1",
    )
    .bind(rig.access.tenant_id.get())
    .execute(&app_db)
    .await
    .expect_err("application role cannot mutate immutable plan evidence");
    assert_eq!(
        mutation.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    let deletion = sqlx::query("DELETE FROM replenishment_confirmations WHERE tenant_id = $1")
        .bind(rig.access.tenant_id.get())
        .execute(&app_db)
        .await
        .expect_err("application role cannot delete confirmation evidence");
    assert_eq!(
        deletion.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    app_db.close().await;

    let admin = admin_db_for(&rig.fixture.db).await;
    let immutable = sqlx::query(
        "UPDATE replenishment_policies SET minimum_qty = minimum_qty + 1 WHERE id = $1",
    )
    .bind(blocked_policy.policy_id)
    .execute(&admin)
    .await
    .expect_err("policy business facts remain immutable even to a privileged writer");
    assert_eq!(
        immutable.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    admin.close().await;
}
