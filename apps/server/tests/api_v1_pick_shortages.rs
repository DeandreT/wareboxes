mod common;

#[path = "api_v1_pick_shortages/support.rs"]
mod support;

use axum::http::{Method, StatusCode};
use serde_json::json;
use support::*;
use wareboxes_api_contract::v1::{
    ErrorReason, ErrorResponse, OrderAllocationOutcome, PickContentState, PickOrderStatus,
    PickShortagePage, PickShortageReason, PickShortageResponse, PickShortageStatus,
    ReallocatePickShortageResponse, ReportPickShortageResponse,
};

#[tokio::test]
async fn short_pick_zero_and_partial_outcomes_are_strict_replay_safe_and_conserved() {
    let zero = PickShortageFixture::new("short-zero", 4).await;
    let no_pick = zero.no_pick_body("inventory_missing", None);

    let released_allocation = zero
        .request(
            Method::POST,
            &format!("/api/v1/orders/{}/allocation-runs", zero.order_id),
            Some("short-zero-released-allocation"),
            Some(json!({
                "facility_id": zero.facility_id,
                "expected_revision": zero.claim.order_revision,
                "strategy": "fefo"
            })),
        )
        .await;
    assert_eq!(released_allocation.status(), StatusCode::CONFLICT);
    zero.assert_untouched_pick(4).await;

    let other_operator = zero
        .operator_only_token("short-zero-other-operator@test.local")
        .await;
    let not_owned = send(
        &zero.app,
        &other_operator,
        zero.access.tenant_id,
        Method::POST,
        &zero.short_pick_path(),
        Some("short-zero-not-owned"),
        Some(no_pick.clone()),
    )
    .await;
    assert_eq!(not_owned.status(), StatusCode::CONFLICT);
    zero.assert_untouched_pick(4).await;

    let missing_key = zero.report(None, no_pick.clone()).await;
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);
    assert_eq!(zero.shortage_count().await, 0);

    let wrong_plate = zero
        .wrong_destination_plate("SHORT-ZERO-WRONG-LOCATION-TOTE")
        .await;
    let mut wrong_destination = zero.partial_body(1, "insufficient_quantity", None);
    wrong_destination["outcome"]["destination_license_plate_barcode"] = json!(wrong_plate);
    let wrong_destination = zero
        .report(Some("short-zero-wrong-destination"), wrong_destination)
        .await;
    assert_eq!(wrong_destination.status(), StatusCode::CONFLICT);

    for (key, body) in zero.invalid_report_bodies() {
        let rejected = zero.report(Some(key), body).await;
        let expected = if key == "short-no-pick-with-destination" {
            StatusCode::UNPROCESSABLE_ENTITY
        } else {
            StatusCode::BAD_REQUEST
        };
        assert_eq!(rejected.status(), expected, "{key}");
    }
    assert_eq!(zero.shortage_count().await, 0);
    zero.assert_untouched_pick(4).await;

    let created = zero
        .report(Some("short-zero-report"), no_pick.clone())
        .await;
    let created = expect_status(created, StatusCode::OK, "zero short report").await;
    let created: ReportPickShortageResponse = response_json(created).await;
    assert_report_contract(&created, &zero, 0, 4, PickShortageStatus::AwaitingInventory);
    assert!(created.movement.is_none());
    assert_eq!(created.details.reason, PickShortageReason::InventoryMissing);
    assert!(created.observed_item_barcode.is_none());

    let replay = zero
        .report(Some("short-zero-report"), no_pick.clone())
        .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<ReportPickShortageResponse>(replay).await,
        created
    );

    let changed = zero
        .report(
            Some("short-zero-report"),
            zero.no_pick_body(
                "inventory_missing",
                Some("The same key cannot change its note"),
            ),
        )
        .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );
    zero.assert_reported_state(&created, 0, 4).await;

    let partial = PickShortageFixture::new("short-partial", 6).await;
    let body = partial.partial_body(2, "insufficient_quantity", None);
    let reported = partial
        .report(Some("short-partial-report"), body.clone())
        .await;
    let reported = expect_status(reported, StatusCode::OK, "partial short report").await;
    let reported: ReportPickShortageResponse = response_json(reported).await;
    assert_report_contract(
        &reported,
        &partial,
        2,
        4,
        PickShortageStatus::AwaitingInventory,
    );
    assert_eq!(
        reported.details.reason,
        PickShortageReason::InsufficientQuantity
    );
    assert_eq!(
        reported.observed_item_barcode.as_deref(),
        Some(partial.item_barcode.as_str())
    );
    assert_partial_movement_contract(&reported, &partial, 2);
    partial.assert_reported_state(&reported, 2, 4).await;
    partial.assert_one_conserved_move(&reported, 2).await;
}

#[tokio::test]
async fn shortage_reallocation_progresses_none_partial_full_then_packing_ready() {
    let shortage = PickShortageFixture::new("short-recovery", 6).await;
    let report = shortage
        .report(
            Some("short-recovery-report"),
            shortage.partial_body(2, "insufficient_quantity", None),
        )
        .await;
    let report = expect_status(report, StatusCode::OK, "recovery short report").await;
    let report: ReportPickShortageResponse = response_json(report).await;
    let shortage_id = report.shortage_id;
    shortage.grant_supervisor().await;

    let queue = shortage
        .request(
            Method::GET,
            &format!(
                "/api/v1/pick-shortages?facility_id={}&inventory_owner_id={}&order_key=short-recovery-ORDER&status=awaiting_inventory&limit=20",
                shortage.facility_id, shortage.inventory_owner_id
            ),
            None,
            None,
        )
        .await;
    let queue = expect_status(queue, StatusCode::OK, "supervisor shortage queue").await;
    let queue: PickShortagePage = response_json(queue).await;
    assert_eq!(queue.items.len(), 1);
    assert_eq!(queue.items[0].shortage_id, shortage_id);
    assert_eq!(queue.items[0].status, PickShortageStatus::AwaitingInventory);
    assert_eq!(queue.items[0].quantities, report.quantities);
    assert!(queue.next_cursor.is_none());
    let detail = shortage
        .request(
            Method::GET,
            &format!("/api/v1/pick-shortages/{shortage_id}"),
            None,
            None,
        )
        .await;
    let detail = expect_status(detail, StatusCode::OK, "supervisor shortage detail").await;
    let detail: PickShortageResponse = response_json(detail).await;
    assert_eq!(detail.shortage_id, shortage_id);
    assert_eq!(detail.order_revision, report.order_revision);
    assert_eq!(detail.hold, report.hold);

    let operator_token = shortage
        .operator_only_token("short-recovery-operator@test.local")
        .await;
    let concealed_queue = send(
        &shortage.app,
        &operator_token,
        shortage.access.tenant_id,
        Method::GET,
        "/api/v1/pick-shortages",
        None,
        None,
    )
    .await;
    assert_eq!(concealed_queue.status(), StatusCode::FORBIDDEN);
    let forbidden_reallocation = send(
        &shortage.app,
        &operator_token,
        shortage.access.tenant_id,
        Method::POST,
        &format!("/api/v1/pick-shortages/{shortage_id}/reallocations"),
        Some("short-recovery-operator-forbidden"),
        Some(reallocation_body(
            report.shortage_revision.get(),
            report.order_revision.get(),
        )),
    )
    .await;
    assert_eq!(forbidden_reallocation.status(), StatusCode::FORBIDDEN);
    shortage.assert_reallocation_count(shortage_id, 0).await;

    let missing_reallocation_key = shortage
        .reallocate(
            shortage_id,
            None,
            reallocation_body(report.shortage_revision.get(), report.order_revision.get()),
        )
        .await;
    assert_eq!(missing_reallocation_key.status(), StatusCode::BAD_REQUEST);
    let invalid_reallocation = shortage
        .reallocate(
            shortage_id,
            Some("short-recovery-invalid-revision"),
            json!({
                "expected_shortage_revision": 0,
                "expected_order_revision": report.order_revision,
                "strategy": "fefo"
            }),
        )
        .await;
    assert_eq!(
        invalid_reallocation.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    shortage.assert_reallocation_count(shortage_id, 0).await;

    let generic = shortage
        .request(
            Method::POST,
            &format!("/api/v1/orders/{}/allocation-runs", shortage.order_id),
            Some("short-generic-allocation"),
            Some(json!({
                "facility_id": shortage.facility_id,
                "expected_revision": report.order_revision,
                "strategy": "fefo"
            })),
        )
        .await;
    assert_eq!(generic.status(), StatusCode::CONFLICT);

    let none = shortage
        .reallocate(
            shortage_id,
            Some("short-recovery-none"),
            reallocation_body(report.shortage_revision.get(), report.order_revision.get()),
        )
        .await;
    let none = expect_status(none, StatusCode::OK, "no-stock reallocation").await;
    let none: ReallocatePickShortageResponse = response_json(none).await;
    assert_reallocation_contract(
        &none,
        shortage_id,
        OrderAllocationOutcome::NotAllocated,
        AllocationProgress {
            newly_allocated: 0,
            recovery_allocated: 0,
            recovery_picked: 0,
            remaining: 4,
            task_count: 0,
            status: PickShortageStatus::AwaitingInventory,
        },
    );
    assert_eq!(none.shortage_revision.get(), 2);
    assert_eq!(none.order_revision.get(), 5);

    let stale = shortage
        .reallocate(
            shortage_id,
            Some("short-recovery-stale"),
            reallocation_body(report.shortage_revision.get(), report.order_revision.get()),
        )
        .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    shortage.assert_reallocation_count(shortage_id, 1).await;

    shortage
        .add_recovery_balance(2, "short-recovery-alt-a")
        .await;
    let partial = shortage
        .reallocate(
            shortage_id,
            Some("short-recovery-partial"),
            reallocation_body(none.shortage_revision.get(), none.order_revision.get()),
        )
        .await;
    let partial = expect_status(partial, StatusCode::OK, "partial reallocation").await;
    let partial: ReallocatePickShortageResponse = response_json(partial).await;
    assert_reallocation_contract(
        &partial,
        shortage_id,
        OrderAllocationOutcome::PartiallyAllocated,
        AllocationProgress {
            newly_allocated: 2,
            recovery_allocated: 2,
            recovery_picked: 0,
            remaining: 2,
            task_count: 1,
            status: PickShortageStatus::RecoveryInProgress,
        },
    );
    assert_eq!(partial.shortage_revision.get(), 3);
    assert_eq!(partial.order_revision.get(), 6);

    shortage
        .add_recovery_balance(2, "short-recovery-alt-b")
        .await;
    let full = shortage
        .reallocate(
            shortage_id,
            Some("short-recovery-full"),
            reallocation_body(
                partial.shortage_revision.get(),
                partial.order_revision.get(),
            ),
        )
        .await;
    let full = expect_status(full, StatusCode::OK, "full reallocation").await;
    let full: ReallocatePickShortageResponse = response_json(full).await;
    assert_reallocation_contract(
        &full,
        shortage_id,
        OrderAllocationOutcome::FullyAllocated,
        AllocationProgress {
            newly_allocated: 2,
            recovery_allocated: 4,
            recovery_picked: 0,
            remaining: 0,
            task_count: 1,
            status: PickShortageStatus::RecoveryInProgress,
        },
    );
    assert_eq!(full.shortage_revision.get(), 4);
    assert_eq!(full.order_revision.get(), 7);
    shortage
        .assert_reallocation_ledger(shortage_id, &none, &partial, &full)
        .await;

    let first = shortage
        .confirm_next(shortage_id, "short-recovery-confirm-a")
        .await;
    assert_eq!(first.order_status, PickOrderStatus::Processing);
    assert_eq!(
        first.shortage_status,
        PickShortageStatus::RecoveryInProgress
    );
    let second = shortage
        .confirm_next(shortage_id, "short-recovery-confirm-b")
        .await;
    assert_eq!(second.order_status, PickOrderStatus::AwaitingPacking);
    assert_eq!(second.shortage_status, PickShortageStatus::Resolved);
    shortage
        .assert_fully_recovered_and_packing_ready(shortage_id, 6)
        .await;
}

#[tokio::test]
async fn recovery_short_creates_child_that_blocks_packing_until_its_own_recovery_completes() {
    let shortage = PickShortageFixture::new("nested-short-recovery", 4).await;
    shortage.make_destination_a_packing_station().await;
    let parent_report = shortage
        .report(
            Some("nested-short-parent-report"),
            shortage.no_pick_body("inventory_missing", None),
        )
        .await;
    let parent_report =
        expect_status(parent_report, StatusCode::OK, "report parent shortage").await;
    let parent_report: ReportPickShortageResponse = response_json(parent_report).await;
    shortage.grant_supervisor().await;
    shortage
        .add_recovery_balance(4, "nested-short-parent-source")
        .await;
    let parent_reallocation = shortage
        .reallocate(
            parent_report.shortage_id,
            Some("nested-short-parent-reallocate"),
            reallocation_body(
                parent_report.shortage_revision.get(),
                parent_report.order_revision.get(),
            ),
        )
        .await;
    let parent_reallocation = expect_status(
        parent_reallocation,
        StatusCode::OK,
        "fully reallocate parent shortage",
    )
    .await;
    let parent_reallocation: ReallocatePickShortageResponse =
        response_json(parent_reallocation).await;
    assert_eq!(
        parent_reallocation.outcome,
        OrderAllocationOutcome::FullyAllocated
    );
    assert_eq!(parent_reallocation.shortage_revision.get(), 2);
    assert_eq!(parent_reallocation.order_revision.get(), 5);
    assert_eq!(parent_reallocation.new_tasks.len(), 1);

    let parent_recovery_claim = shortage
        .claim_next("nested-short-parent-recovery-claim")
        .await;
    assert_eq!(parent_recovery_claim.order_id, shortage.order_id);
    assert_eq!(
        parent_recovery_claim.content.inventory_allocation_id,
        parent_reallocation.new_allocations[0].allocation_id
    );
    let child_body = PickShortageFixture::no_pick_body_for_claim(&parent_recovery_claim);
    let child_report = shortage
        .report_claim(
            &parent_recovery_claim,
            "nested-short-child-report",
            child_body.clone(),
        )
        .await;
    let child_report =
        expect_status(child_report, StatusCode::OK, "short parent recovery task").await;
    let child_report: ReportPickShortageResponse = response_json(child_report).await;
    assert_ne!(child_report.shortage_id, parent_report.shortage_id);
    assert_eq!(child_report.quantities.planned, 4);
    assert_eq!(child_report.quantities.picked, 0);
    assert_eq!(child_report.quantities.short, 4);
    assert_eq!(child_report.order_revision.get(), 6);

    let child_replay = shortage
        .report_claim(
            &parent_recovery_claim,
            "nested-short-child-report",
            child_body,
        )
        .await;
    let child_replay = expect_status(child_replay, StatusCode::OK, "replay child shortage").await;
    assert_eq!(
        response_json::<ReportPickShortageResponse>(child_replay).await,
        child_report
    );

    let parent_detail = shortage
        .request(
            Method::GET,
            &format!("/api/v1/pick-shortages/{}", parent_report.shortage_id),
            None,
            None,
        )
        .await;
    let parent_detail = expect_status(
        parent_detail,
        StatusCode::OK,
        "read resolved parent shortage",
    )
    .await;
    let parent_detail: PickShortageResponse = response_json(parent_detail).await;
    assert_eq!(parent_detail.status, PickShortageStatus::Resolved);
    assert_eq!(parent_detail.shortage_revision.get(), 3);
    assert_eq!(parent_detail.reallocated_quantity, 4);
    assert_eq!(parent_detail.recovery_terminal_quantity, 4);
    assert_eq!(parent_detail.remaining_to_allocate_quantity, 0);
    assert!(parent_detail.resolved_at.is_some());

    let child_detail = shortage
        .request(
            Method::GET,
            &format!("/api/v1/pick-shortages/{}", child_report.shortage_id),
            None,
            None,
        )
        .await;
    let child_detail = expect_status(
        child_detail,
        StatusCode::OK,
        "read unresolved child shortage",
    )
    .await;
    let child_detail: PickShortageResponse = response_json(child_detail).await;
    assert_eq!(child_detail.status, PickShortageStatus::AwaitingInventory);
    assert_eq!(child_detail.shortage_revision.get(), 1);
    assert_eq!(child_detail.reallocated_quantity, 0);
    assert_eq!(child_detail.recovery_terminal_quantity, 0);
    assert_eq!(child_detail.remaining_to_allocate_quantity, 4);
    assert!(child_detail.resolved_at.is_none());

    shortage
        .assert_recovery_progress_event(RecoveryProgressExpectation {
            shortage_id: parent_report.shortage_id,
            shortage_revision: 3,
            shortage_status: PickShortageStatus::Resolved,
            order_revision: child_report.order_revision.get(),
            trigger_task_id: parent_recovery_claim.task_id,
            trigger_source_allocation_id: parent_recovery_claim.content.inventory_allocation_id,
            terminal_quantity: 4,
            preceding_event_type: "outbound.pick.shortage_reported",
        })
        .await;

    let blocked_packing = shortage
        .open_packing_session(
            child_report.order_revision.get(),
            "nested-short-packing-blocked",
        )
        .await;
    assert_eq!(blocked_packing.status(), StatusCode::CONFLICT);

    shortage
        .add_recovery_balance(4, "nested-short-child-source")
        .await;
    let child_reallocation = shortage
        .reallocate(
            child_report.shortage_id,
            Some("nested-short-child-reallocate"),
            reallocation_body(
                child_report.shortage_revision.get(),
                child_report.order_revision.get(),
            ),
        )
        .await;
    let child_reallocation = expect_status(
        child_reallocation,
        StatusCode::OK,
        "fully reallocate child shortage",
    )
    .await;
    let child_reallocation: ReallocatePickShortageResponse =
        response_json(child_reallocation).await;
    assert_eq!(
        child_reallocation.outcome,
        OrderAllocationOutcome::FullyAllocated
    );
    assert_eq!(child_reallocation.shortage_revision.get(), 2);
    assert_eq!(child_reallocation.order_revision.get(), 7);
    assert_eq!(child_reallocation.new_tasks.len(), 1);

    let child_recovery_claim = shortage
        .claim_next("nested-short-child-recovery-claim")
        .await;
    assert_eq!(
        child_recovery_claim.content.inventory_allocation_id,
        child_reallocation.new_allocations[0].allocation_id
    );
    let confirmation_body = shortage.confirmation_body_for_claim(&child_recovery_claim);
    let confirmation = shortage
        .confirm_claim(
            &child_recovery_claim,
            "nested-short-child-confirm",
            confirmation_body.clone(),
        )
        .await;
    let confirmation =
        PickShortageFixture::parse_confirmation(confirmation, "confirm child recovery task").await;
    assert_eq!(confirmation.content_state, PickContentState::Completed);
    assert!(confirmation.task_completed);
    assert!(confirmation.order_ready_to_pack);
    assert_eq!(confirmation.order_status, PickOrderStatus::AwaitingPacking);
    assert_eq!(confirmation.order_revision.get(), 8);

    let confirmation_replay = shortage
        .confirm_claim(
            &child_recovery_claim,
            "nested-short-child-confirm",
            confirmation_body,
        )
        .await;
    let confirmation_replay = PickShortageFixture::parse_confirmation(
        confirmation_replay,
        "replay child recovery confirmation",
    )
    .await;
    assert_eq!(confirmation_replay, confirmation);

    let final_parent = shortage
        .request(
            Method::GET,
            &format!("/api/v1/pick-shortages/{}", parent_report.shortage_id),
            None,
            None,
        )
        .await;
    let final_parent = expect_status(final_parent, StatusCode::OK, "read final parent").await;
    let final_parent: PickShortageResponse = response_json(final_parent).await;
    assert_eq!(final_parent.status, PickShortageStatus::Resolved);
    assert_eq!(final_parent.shortage_revision.get(), 3);
    let final_child = shortage
        .request(
            Method::GET,
            &format!("/api/v1/pick-shortages/{}", child_report.shortage_id),
            None,
            None,
        )
        .await;
    let final_child = expect_status(final_child, StatusCode::OK, "read final child").await;
    let final_child: PickShortageResponse = response_json(final_child).await;
    assert_eq!(final_child.status, PickShortageStatus::Resolved);
    assert_eq!(final_child.shortage_revision.get(), 3);
    assert_eq!(final_child.recovery_terminal_quantity, 4);
    assert!(final_child.resolved_at.is_some());

    shortage
        .assert_recovery_progress_event(RecoveryProgressExpectation {
            shortage_id: parent_report.shortage_id,
            shortage_revision: 3,
            shortage_status: PickShortageStatus::Resolved,
            order_revision: child_report.order_revision.get(),
            trigger_task_id: parent_recovery_claim.task_id,
            trigger_source_allocation_id: parent_recovery_claim.content.inventory_allocation_id,
            terminal_quantity: 4,
            preceding_event_type: "outbound.pick.shortage_reported",
        })
        .await;
    shortage
        .assert_recovery_progress_event(RecoveryProgressExpectation {
            shortage_id: child_report.shortage_id,
            shortage_revision: 3,
            shortage_status: PickShortageStatus::Resolved,
            order_revision: confirmation.order_revision.get(),
            trigger_task_id: child_recovery_claim.task_id,
            trigger_source_allocation_id: child_recovery_claim.content.inventory_allocation_id,
            terminal_quantity: 4,
            preceding_event_type: "outbound.pick.confirmed",
        })
        .await;

    let default_queue = shortage
        .request(Method::GET, "/api/v1/pick-shortages?limit=20", None, None)
        .await;
    let default_queue = expect_status(
        default_queue,
        StatusCode::OK,
        "default shortage queue excludes resolved rows",
    )
    .await;
    let default_queue: PickShortagePage = response_json(default_queue).await;
    assert!(default_queue.items.is_empty());
    let resolved_queue = shortage
        .request(
            Method::GET,
            "/api/v1/pick-shortages?status=resolved&limit=20",
            None,
            None,
        )
        .await;
    let resolved_queue = expect_status(
        resolved_queue,
        StatusCode::OK,
        "explicit resolved shortage queue",
    )
    .await;
    let resolved_queue: PickShortagePage = response_json(resolved_queue).await;
    let mut resolved_ids = resolved_queue
        .items
        .iter()
        .map(|item| item.shortage_id)
        .collect::<Vec<_>>();
    resolved_ids.sort_unstable();
    let mut expected_ids = vec![parent_report.shortage_id, child_report.shortage_id];
    expected_ids.sort_unstable();
    assert_eq!(resolved_ids, expected_ids);

    let packing = shortage
        .open_packing_session(
            confirmation.order_revision.get(),
            "nested-short-packing-ready",
        )
        .await;
    expect_status(packing, StatusCode::OK, "open recovered packing session").await;
}

#[tokio::test]
async fn shortage_queue_cursor_pages_without_duplicates_and_rejects_filter_reuse() {
    let shortage = PickShortageFixture::new("shortage-cursor", 2).await;
    let first = shortage
        .report(
            Some("shortage-cursor-first-report"),
            shortage.no_pick_body("inventory_missing", None),
        )
        .await;
    let first = expect_status(first, StatusCode::OK, "report first paged shortage").await;
    let first: ReportPickShortageResponse = response_json(first).await;
    shortage.grant_supervisor().await;
    let second = shortage
        .create_additional_shortage("shortage-cursor-second", 3)
        .await;

    let filters = format!(
        "facility_id={}&inventory_owner_id={}&status=awaiting_inventory&limit=1",
        shortage.facility_id, shortage.inventory_owner_id
    );
    let first_page = shortage
        .request(
            Method::GET,
            &format!("/api/v1/pick-shortages?{filters}"),
            None,
            None,
        )
        .await;
    let first_page = expect_status(first_page, StatusCode::OK, "first shortage page").await;
    let first_page: PickShortagePage = response_json(first_page).await;
    assert_eq!(first_page.items.len(), 1);
    assert_eq!(first_page.items[0].shortage_id, second.shortage_id);
    let cursor = first_page
        .next_cursor
        .as_ref()
        .expect("two shortages produce a second page");
    let second_page = shortage
        .request(
            Method::GET,
            &format!("/api/v1/pick-shortages?{filters}&cursor={cursor}"),
            None,
            None,
        )
        .await;
    let second_page = expect_status(second_page, StatusCode::OK, "second shortage page").await;
    let second_page: PickShortagePage = response_json(second_page).await;
    assert_eq!(second_page.items.len(), 1);
    assert!(second_page.next_cursor.is_none());
    let mut actual_ids = vec![
        first_page.items[0].shortage_id,
        second_page.items[0].shortage_id,
    ];
    actual_ids.sort_unstable();
    let mut expected_ids = vec![first.shortage_id, second.shortage_id];
    expected_ids.sort_unstable();
    assert_eq!(actual_ids, expected_ids);

    let quantity_sorted = shortage
        .request(
            Method::GET,
            &format!("/api/v1/pick-shortages?{filters}&sort=short_quantity&direction=descending"),
            None,
            None,
        )
        .await;
    let quantity_sorted = expect_status(
        quantity_sorted,
        StatusCode::OK,
        "globally quantity-sorted shortage page",
    )
    .await;
    let quantity_sorted: PickShortagePage = response_json(quantity_sorted).await;
    assert_eq!(quantity_sorted.items[0].shortage_id, second.shortage_id);
    let quantity_cursor = quantity_sorted.next_cursor.as_ref().unwrap();
    let sort_mismatch = shortage
        .request(
            Method::GET,
            &format!(
                "/api/v1/pick-shortages?{filters}&sort=short_quantity&direction=ascending&cursor={quantity_cursor}"
            ),
            None,
            None,
        )
        .await;
    let sort_mismatch = expect_status(
        sort_mismatch,
        StatusCode::BAD_REQUEST,
        "shortage cursor sort mismatch",
    )
    .await;
    assert_eq!(
        response_json::<ErrorResponse>(sort_mismatch).await.reason,
        ErrorReason::InvalidCursor
    );

    for path in [
        format!(
            "/api/v1/pick-shortages?facility_id={}&inventory_owner_id={}&limit=1&cursor={cursor}",
            shortage.facility_id, shortage.inventory_owner_id
        ),
        format!("/api/v1/pick-shortages?{filters}&order_key=cursor-mismatch&cursor={cursor}"),
    ] {
        let mismatch = shortage.request(Method::GET, &path, None, None).await;
        let mismatch =
            expect_status(mismatch, StatusCode::BAD_REQUEST, "cursor filter mismatch").await;
        assert_eq!(
            response_json::<ErrorResponse>(mismatch).await.reason,
            ErrorReason::InvalidCursor
        );
    }
}

#[tokio::test]
async fn reallocation_rejects_stale_shortage_and_order_revisions_independently() {
    let shortage = PickShortageFixture::new("shortage-independent-revisions", 3).await;
    let report = shortage
        .report(
            Some("shortage-independent-revisions-report"),
            shortage.no_pick_body("inventory_missing", None),
        )
        .await;
    let report = expect_status(report, StatusCode::OK, "report revision shortage").await;
    let report: ReportPickShortageResponse = response_json(report).await;
    shortage.grant_supervisor().await;
    let current = shortage
        .reallocate(
            report.shortage_id,
            Some("shortage-independent-revisions-current"),
            reallocation_body(report.shortage_revision.get(), report.order_revision.get()),
        )
        .await;
    let current = expect_status(current, StatusCode::OK, "advance both revisions").await;
    let current: ReallocatePickShortageResponse = response_json(current).await;
    assert_eq!(current.shortage_revision.get(), 2);
    assert_eq!(current.order_revision.get(), 5);

    let stale_shortage = shortage
        .reallocate(
            report.shortage_id,
            Some("shortage-independent-stale-shortage"),
            reallocation_body(report.shortage_revision.get(), current.order_revision.get()),
        )
        .await;
    let stale_shortage = expect_status(
        stale_shortage,
        StatusCode::CONFLICT,
        "stale shortage revision",
    )
    .await;
    let stale_shortage: ErrorResponse = response_json(stale_shortage).await;
    assert_eq!(stale_shortage.reason, ErrorReason::Conflict);
    assert_eq!(
        stale_shortage.message,
        "pick shortage revision does not match expected revision"
    );

    let stale_order = shortage
        .reallocate(
            report.shortage_id,
            Some("shortage-independent-stale-order"),
            reallocation_body(current.shortage_revision.get(), report.order_revision.get()),
        )
        .await;
    let stale_order =
        expect_status(stale_order, StatusCode::CONFLICT, "stale order revision").await;
    let stale_order: ErrorResponse = response_json(stale_order).await;
    assert_eq!(stale_order.reason, ErrorReason::Conflict);
    assert_eq!(
        stale_order.message,
        "order revision does not match expected revision"
    );
    shortage
        .assert_reallocation_count(report.shortage_id, 1)
        .await;
}

#[tokio::test]
async fn short_pick_and_reallocation_races_have_one_winner_and_fail_closed_after_revocation() {
    let short = PickShortageFixture::new("short-race", 4).await;
    let short_path = short.short_pick_path();
    let short_body = short.partial_body(1, "insufficient_quantity", None);
    let confirm_body = short.confirmation_body();
    let report = short.request(
        Method::POST,
        &short_path,
        Some("short-race-report"),
        Some(short_body),
    );
    let confirmation_path = short.confirmation_path();
    let confirm = short.request(
        Method::POST,
        &confirmation_path,
        Some("short-race-confirm"),
        Some(confirm_body),
    );
    let (report, confirm) = tokio::join!(report, confirm);
    match (report.status(), confirm.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => {
            let report: ReportPickShortageResponse = response_json(report).await;
            short.assert_reported_state(&report, 1, 3).await;
            short.assert_confirmation_quantities(&[1]).await;
        }
        (StatusCode::CONFLICT, StatusCode::OK) => {
            short.assert_shortage_count(0).await;
            short.assert_confirmation_quantities(&[4]).await;
        }
        statuses => panic!("expected one confirm-or-short winner, got {statuses:?}"),
    }

    let race = PickShortageFixture::new("reallocation-race", 4).await;
    let report = race
        .report(
            Some("reallocation-race-report"),
            race.no_pick_body("inventory_missing", None),
        )
        .await;
    let report = expect_status(report, StatusCode::OK, "race short report").await;
    let report: ReportPickShortageResponse = response_json(report).await;
    let shortage_id = report.shortage_id;
    race.grant_supervisor().await;
    race.add_recovery_balance(4, "reallocation-race-alt").await;
    let body = reallocation_body(report.shortage_revision.get(), report.order_revision.get());
    let first = race.reallocate(shortage_id, Some("reallocation-race-a"), body.clone());
    let second = race.reallocate(shortage_id, Some("reallocation-race-b"), body.clone());
    let (first, second) = tokio::join!(first, second);
    let success = match (first.status(), second.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => first,
        (StatusCode::CONFLICT, StatusCode::OK) => second,
        statuses => panic!("expected one reallocation winner, got {statuses:?}"),
    };
    let success: ReallocatePickShortageResponse = response_json(success).await;
    race.assert_one_reallocation(shortage_id, 4).await;

    let replay_key = race.successful_reallocation_key(shortage_id).await;
    let replay = race
        .reallocate(shortage_id, Some(&replay_key), body.clone())
        .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<ReallocatePickShortageResponse>(replay).await,
        success
    );
    let changed = race
        .reallocate(
            shortage_id,
            Some(&replay_key),
            reallocation_body(
                report.shortage_revision.get(),
                report.order_revision.get() + 1,
            ),
        )
        .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    race.revoke_scope().await;
    let concealed_report = race
        .report(
            Some("reallocation-race-report"),
            race.no_pick_body("inventory_missing", None),
        )
        .await;
    assert_eq!(concealed_report.status(), StatusCode::NOT_FOUND);
    let concealed_reallocation = race.reallocate(shortage_id, Some(&replay_key), body).await;
    assert_eq!(concealed_reallocation.status(), StatusCode::NOT_FOUND);
    let concealed_changed_reallocation = race
        .reallocate(
            shortage_id,
            Some(&replay_key),
            reallocation_body(
                success.shortage_revision.get(),
                success.order_revision.get(),
            ),
        )
        .await;
    assert_eq!(
        concealed_changed_reallocation.status(),
        StatusCode::NOT_FOUND
    );
    let concealed_detail = race
        .request(
            Method::GET,
            &format!("/api/v1/pick-shortages/{shortage_id}"),
            None,
            None,
        )
        .await;
    assert_eq!(concealed_detail.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn shortages_enforce_scope_cancellation_immutability_rls_and_minimal_grants() {
    let short = PickShortageFixture::new("short-governance", 3).await;
    let report = short
        .report(
            Some("short-governance-report"),
            short.no_pick_body("inventory_missing", None),
        )
        .await;
    let report = expect_status(report, StatusCode::OK, "governance short report").await;
    let report: ReportPickShortageResponse = response_json(report).await;
    let shortage_id = report.shortage_id;

    let cancellation = short
        .request(
            Method::POST,
            &format!("/api/v1/orders/{}/cancellations", short.order_id),
            Some("short-governance-cancel"),
            Some(json!({
                "expected_revision": report.order_revision,
                "reason": "inventory_unavailable",
                "note": "Disposition is not part of shortage recovery"
            })),
        )
        .await;
    assert_eq!(cancellation.status(), StatusCode::CONFLICT);
    short.assert_cancellation_has_zero_effects().await;
    short.grant_supervisor().await;

    let no_stock = short
        .reallocate(
            shortage_id,
            Some("short-governance-reallocate"),
            reallocation_body(report.shortage_revision.get(), report.order_revision.get()),
        )
        .await;
    let no_stock = expect_status(no_stock, StatusCode::OK, "governance reallocation").await;
    let no_stock: ReallocatePickShortageResponse = response_json(no_stock).await;
    assert_eq!(no_stock.outcome, OrderAllocationOutcome::NotAllocated);

    short.assert_shortage_tables_governed().await;
    short
        .assert_shortage_rows_immutable(shortage_id, no_stock.reallocation_run_id)
        .await;
    short.assert_cross_tenant_rls(shortage_id).await;

    let outsider = short
        .cross_tenant_user("short-governance-outsider@test.local")
        .await;
    let guessed = send(
        &short.app,
        &outsider.token,
        outsider.tenant_id,
        Method::POST,
        &short.short_pick_path(),
        Some("short-cross-tenant-guess"),
        Some(short.no_pick_body("inventory_missing", None)),
    )
    .await;
    assert_eq!(guessed.status(), StatusCode::NOT_FOUND);

    short.set_scope(vec![short.facility_id], Vec::new()).await;
    let owner_revoked_replay = short
        .report(
            Some("short-governance-report"),
            short.no_pick_body("inventory_missing", None),
        )
        .await;
    assert_eq!(owner_revoked_replay.status(), StatusCode::NOT_FOUND);
    let owner_revoked_changed_replay = short
        .report(
            Some("short-governance-report"),
            short.no_pick_body("inventory_missing", Some("changed after scope revocation")),
        )
        .await;
    assert_eq!(owner_revoked_changed_replay.status(), StatusCode::NOT_FOUND);
    short
        .set_scope(Vec::new(), vec![short.inventory_owner_id])
        .await;
    let facility_revoked_replay = short
        .report(
            Some("short-governance-report"),
            short.no_pick_body("inventory_missing", None),
        )
        .await;
    assert_eq!(facility_revoked_replay.status(), StatusCode::NOT_FOUND);
}
