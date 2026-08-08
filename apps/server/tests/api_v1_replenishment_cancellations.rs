mod common;

#[path = "api_v1_replenishment/support.rs"]
#[allow(dead_code)]
mod support;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{json, Value};
use sqlx::Row;
use support::*;
use wareboxes_api::{auth, routes, state::AppState};
use wareboxes_api_contract::v1::{
    ConfigureReplenishmentPolicyResponse, ErrorReason, ErrorResponse, PlanReplenishmentResponse,
    ReplenishmentClaimReleaseResponse, ReplenishmentClaimResponse,
    ReplenishmentConfirmationResponse, ReplenishmentQueuePage,
    ReplenishmentWorkCancellationResponse, ReplenishmentWorkStatus,
};

async fn planned_work(
    rig: &ReplenishmentFixture,
    key: &str,
) -> (ConfigureReplenishmentPolicyResponse, i64) {
    let (source_location_id, source_barcode) = rig.reserve_source(key).await;
    rig.seed_stock(
        source_location_id,
        &source_barcode,
        20,
        &format!("{key}-LOT"),
        None,
        &format!("{key}-stock"),
    )
    .await;
    let policy = rig
        .configure(
            &format!("{key}-configure"),
            &[source_location_id],
            2,
            5,
            None,
        )
        .await;
    let policy: ConfigureReplenishmentPolicyResponse =
        response_json(expect_status(policy, StatusCode::OK, "configure cancellation policy").await)
            .await;
    let plan = rig
        .plan(
            policy.policy_id,
            policy.revision.get(),
            &format!("{key}-plan"),
        )
        .await;
    let plan: PlanReplenishmentResponse =
        response_json(expect_status(plan, StatusCode::OK, "plan cancellable work").await).await;
    assert_eq!(plan.work.len(), 1);
    (policy, plan.work[0].work_id)
}

async fn plan_again(rig: &ReplenishmentFixture, policy_id: i64, key: &str) -> i64 {
    let plan = rig.plan(policy_id, 1, key).await;
    let plan: PlanReplenishmentResponse =
        response_json(expect_status(plan, StatusCode::OK, "plan replacement work").await).await;
    assert_eq!(plan.work.len(), 1);
    plan.work[0].work_id
}

async fn cancel(
    rig: &ReplenishmentFixture,
    work_id: i64,
    key: &str,
    reason: &str,
    note: Option<&str>,
) -> axum::response::Response {
    let mut body = json!({"reason": reason});
    if let Some(note) = note {
        body["note"] = json!(note);
    }
    rig.request(
        Method::POST,
        &format!("/api/v1/replenishment-tasks/{work_id}/cancellations"),
        Some(key),
        Some(body),
    )
    .await
}

#[tokio::test]
async fn pending_cancellation_is_replay_safe_audited_and_releases_all_work_claims() {
    init_test_tracing();
    let rig = ReplenishmentFixture::new("replenishment-cancel-success").await;
    let (policy, work_id) = planned_work(&rig, "CANCEL-SUCCESS").await;
    let before = rig.effect_counts().await;

    let response = cancel(
        &rig,
        work_id,
        "cancel-success",
        "demand_removed",
        Some("outbound demand was withdrawn"),
    )
    .await;
    let cancelled: ReplenishmentWorkCancellationResponse = response_json(
        expect_status(
            response,
            StatusCode::OK,
            "cancel pending replenishment work",
        )
        .await,
    )
    .await;
    assert_eq!(cancelled.work_id, work_id);
    assert_eq!(cancelled.previous_status, ReplenishmentWorkStatus::Pending);
    assert!(cancelled.previous_assigned_user_id.is_none());
    assert_eq!(cancelled.status, ReplenishmentWorkStatus::Cancelled);
    assert_eq!(cancelled.quantity, 5);
    assert_eq!(cancelled.cancelled_by, rig.access.user_id.get());

    let replay = cancel(
        &rig,
        work_id,
        "cancel-success",
        "demand_removed",
        Some("outbound demand was withdrawn"),
    )
    .await;
    assert_eq!(
        response_json::<ReplenishmentWorkCancellationResponse>(
            expect_status(replay, StatusCode::OK, "replay replenishment cancellation").await,
        )
        .await,
        cancelled
    );
    assert_error_reason(
        cancel(
            &rig,
            work_id,
            "cancel-success",
            "planning_error",
            Some("changed request"),
        )
        .await,
        StatusCode::CONFLICT,
        ErrorReason::IdempotencyKeyReused,
        "changed cancellation replay",
    )
    .await;

    let after = rig.effect_counts().await;
    assert_eq!(after.transactions, before.transactions);
    assert_eq!(after.entries, before.entries);
    assert_eq!(after.allocations, before.allocations);
    assert_eq!(after.confirmations, before.confirmations);
    let mut tx = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    let row = sqlx::query(
        r#"
        SELECT work.status,work.assigned_user_id,work.started_at,work.lease_expires_at,
          work.completed_by,work.completed_at,detail.closed_at,claim.released_at,
          evidence.previous_work_status,evidence.previous_assigned_user_id,
          evidence.reason_code,evidence.note,evidence.cancelled_at,
          (SELECT count(*) FROM replenishment_cancellations item
             WHERE item.tenant_id=work.tenant_id AND item.task_id=work.id) evidence_count,
          (SELECT count(*) FROM work_task_progress progress
             WHERE progress.tenant_id=work.tenant_id AND progress.task_id=work.id
               AND progress.action='replenishment_cancelled') progress_count,
          (SELECT count(*) FROM outbox_events event
             WHERE event.tenant_id=work.tenant_id
               AND event.aggregate_type='replenishment_task'
               AND event.aggregate_id=work.id::text
               AND event.event_type='inventory.replenishment.cancelled') event_count
        FROM work_tasks work
        JOIN replenishment_tasks detail ON detail.tenant_id=work.tenant_id AND detail.task_id=work.id
        JOIN loose_inventory_movement_claims claim ON claim.tenant_id=work.tenant_id
          AND claim.work_kind='replenishment' AND claim.work_task_id=work.id
        JOIN replenishment_cancellations evidence ON evidence.tenant_id=work.tenant_id
          AND evidence.task_id=work.id
        WHERE work.tenant_id=$1 AND work.id=$2
        "#,
    )
    .bind(rig.access.tenant_id.get())
    .bind(work_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("status"), "cancelled");
    assert!(row.get::<Option<i64>, _>("assigned_user_id").is_none());
    assert!(row
        .get::<Option<wareboxes_domain::Timestamp>, _>("started_at")
        .is_none());
    assert!(row
        .get::<Option<wareboxes_domain::Timestamp>, _>("lease_expires_at")
        .is_none());
    assert_eq!(row.get::<i64, _>("completed_by"), rig.access.user_id.get());
    let completed_at = row.get::<wareboxes_domain::Timestamp, _>("completed_at");
    assert_eq!(
        row.get::<wareboxes_domain::Timestamp, _>("closed_at"),
        completed_at
    );
    assert_eq!(
        row.get::<wareboxes_domain::Timestamp, _>("released_at"),
        completed_at
    );
    assert_eq!(
        row.get::<wareboxes_domain::Timestamp, _>("cancelled_at"),
        completed_at
    );
    assert_eq!(row.get::<String, _>("previous_work_status"), "open");
    assert!(row
        .get::<Option<i64>, _>("previous_assigned_user_id")
        .is_none());
    assert_eq!(row.get::<String, _>("reason_code"), "demand_removed");
    assert_eq!(row.get::<i64, _>("evidence_count"), 1);
    assert_eq!(row.get::<i64, _>("progress_count"), 1);
    assert_eq!(row.get::<i64, _>("event_count"), 1);
    tx.rollback().await.unwrap();

    let default_queue: ReplenishmentQueuePage = response_json(
        expect_status(
            rig.request(Method::GET, "/api/v1/replenishment-queue", None, None)
                .await,
            StatusCode::OK,
            "default queue after cancellation",
        )
        .await,
    )
    .await;
    assert!(default_queue
        .items
        .iter()
        .all(|item| item.work_id != work_id));
    let cancelled_queue: ReplenishmentQueuePage = response_json(
        expect_status(
            rig.request(
                Method::GET,
                "/api/v1/replenishment-queue?status=cancelled",
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "cancelled queue",
        )
        .await,
    )
    .await;
    assert!(cancelled_queue
        .items
        .iter()
        .any(|item| item.work_id == work_id));

    let replacement = rig
        .configure(
            "cancel-success-reconfigure",
            &[policy.reserve_source_location_ids.as_slice()[0]],
            1,
            6,
            Some(1),
        )
        .await;
    assert_eq!(replacement.status(), StatusCode::OK);
}

#[tokio::test]
async fn cancellation_accepts_assigned_but_rejects_active_completed_and_invalid_requests() {
    init_test_tracing();
    let rig = ReplenishmentFixture::new("replenishment-cancel-states").await;
    let (policy, first_work) = planned_work(&rig, "CANCEL-STATES").await;
    let mut tx = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    let forged = sqlx::query(
        r#"
        INSERT INTO replenishment_cancellations (
          tenant_id,task_id,plan_run_id,policy_id,policy_revision,inventory_owner_id,
          facility_id,source_inventory_balance_id,item_batch_id,item_id,uom,planned_qty,
          previous_work_status,previous_assigned_user_id,reason_code,note,
          cancelled_by_user_id,cancelled_at
        )
        SELECT detail.tenant_id,detail.task_id,detail.plan_run_id,detail.policy_id,
          detail.policy_revision,detail.inventory_owner_id,detail.facility_id,
          detail.source_inventory_balance_id,detail.item_batch_id,detail.item_id,
          detail.uom,detail.planned_qty,'assigned',$3,'planning_error',NULL,$3,
          statement_timestamp()
        FROM replenishment_tasks detail
        WHERE detail.tenant_id=$1 AND detail.task_id=$2
        "#,
    )
    .bind(rig.access.tenant_id.get())
    .bind(first_work)
    .bind(rig.access.user_id.get())
    .execute(&mut *tx)
    .await
    .expect_err("forged prior assignment evidence must be rejected");
    assert_eq!(
        forged.as_database_error().unwrap().code().as_deref(),
        Some("55000")
    );
    tx.rollback().await.unwrap();
    for (key, reason, note) in [
        ("cancel-other-no-note", "other", None),
        ("cancel-blank-note", "planning_error", Some(" ")),
    ] {
        assert_eq!(
            cancel(&rig, first_work, key, reason, note).await.status(),
            StatusCode::BAD_REQUEST
        );
    }
    let long_note = "x".repeat(501);
    assert_eq!(
        cancel(
            &rig,
            first_work,
            "cancel-long-note",
            "planning_error",
            Some(&long_note),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    let _claim: ReplenishmentClaimResponse = response_json(
        expect_status(
            rig.claim_by_id(first_work, "cancel-states-claim").await,
            StatusCode::OK,
            "claim work before cancellation",
        )
        .await,
    )
    .await;
    assert_error_reason(
        cancel(
            &rig,
            first_work,
            "cancel-active",
            "source_unavailable",
            None,
        )
        .await,
        StatusCode::CONFLICT,
        ErrorReason::Conflict,
        "active claim requires release",
    )
    .await;
    let released: ReplenishmentClaimReleaseResponse = response_json(
        expect_status(
            rig.request(
                Method::POST,
                &format!("/api/v1/replenishment-claims/{first_work}/releases"),
                Some("cancel-states-release"),
                Some(json!({"reason":"work_interrupted"})),
            )
            .await,
            StatusCode::OK,
            "release active work",
        )
        .await,
    )
    .await;
    assert_eq!(released.status, ReplenishmentWorkStatus::Pending);
    assert_eq!(
        cancel(
            &rig,
            first_work,
            "cancel-after-release",
            "source_unavailable",
            None,
        )
        .await
        .status(),
        StatusCode::OK
    );

    let assigned_work = plan_again(&rig, policy.policy_id, "cancel-assigned-plan").await;
    let mut tx = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    sqlx::query(
        "UPDATE work_tasks SET status='assigned',assigned_user_id=$1,modified=statement_timestamp() WHERE tenant_id=$2 AND id=$3 AND status='open'",
    )
    .bind(rig.access.user_id.get())
    .bind(rig.access.tenant_id.get())
    .bind(assigned_work)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let assigned: ReplenishmentWorkCancellationResponse = response_json(
        expect_status(
            cancel(
                &rig,
                assigned_work,
                "cancel-assigned",
                "destination_unavailable",
                Some("pick face is blocked"),
            )
            .await,
            StatusCode::OK,
            "cancel assigned work",
        )
        .await,
    )
    .await;
    assert_eq!(assigned.previous_status, ReplenishmentWorkStatus::Pending);
    assert_eq!(
        assigned.previous_assigned_user_id,
        Some(rig.access.user_id.get())
    );

    let completed_work = plan_again(&rig, policy.policy_id, "cancel-completed-plan").await;
    let claim: ReplenishmentClaimResponse = response_json(
        expect_status(
            rig.claim_by_id(completed_work, "cancel-completed-claim")
                .await,
            StatusCode::OK,
            "claim work to complete",
        )
        .await,
    )
    .await;
    let confirmation: ReplenishmentConfirmationResponse = response_json(
        expect_status(
            rig.confirm(&claim, "cancel-completed-confirm", rig.exact_scans(&claim))
                .await,
            StatusCode::OK,
            "complete replenishment before cancellation",
        )
        .await,
    )
    .await;
    assert_eq!(confirmation.work_id, completed_work);
    for (work_id, key) in [
        (first_work, "cancel-terminal-cancelled"),
        (completed_work, "cancel-terminal-completed"),
    ] {
        assert_error_reason(
            cancel(&rig, work_id, key, "planning_error", None).await,
            StatusCode::CONFLICT,
            ErrorReason::Conflict,
            "terminal replenishment cannot be cancelled",
        )
        .await;
    }
}

#[tokio::test]
async fn cancellation_conceals_tenant_and_scope_and_evidence_is_rls_protected_immutable() {
    init_test_tracing();
    let rig = ReplenishmentFixture::new("replenishment-cancel-scope").await;
    let (_, work_id) = planned_work(&rig, "CANCEL-SCOPE").await;
    let cancelled: ReplenishmentWorkCancellationResponse = response_json(
        expect_status(
            cancel(
                &rig,
                work_id,
                "cancel-scope-success",
                "planning_error",
                Some("source selection was invalid"),
            )
            .await,
            StatusCode::OK,
            "cancel before scope revocation",
        )
        .await,
    )
    .await;

    let foreign = rig
        .fixture
        .wms_user("replenishment-cancel-foreign@test.local")
        .await;
    let foreign_access = default_tenant_for_user(&rig.fixture.db, foreign.id)
        .await
        .unwrap();
    let foreign_token = auth::create_session(&rig.fixture.db, foreign.id)
        .await
        .unwrap();
    assert_error_reason(
        send(
            &rig.app,
            &foreign_token,
            foreign_access.tenant_id,
            Method::POST,
            &format!("/api/v1/replenishment-tasks/{work_id}/cancellations"),
            Some("cancel-missing-supervisor"),
            Some(json!({"reason":"planning_error"})),
        )
        .await,
        StatusCode::FORBIDDEN,
        ErrorReason::Forbidden,
        "cancellation requires supervisor permission",
    )
    .await;
    grant_permission(
        &rig.fixture.db,
        foreign_access.tenant_id,
        foreign.id,
        "replenishment-cancel-foreign-supervisor",
        "wms_supervisor",
    )
    .await;
    assert_error_reason(
        send(
            &rig.app,
            &foreign_token,
            foreign_access.tenant_id,
            Method::POST,
            &format!("/api/v1/replenishment-tasks/{work_id}/cancellations"),
            Some("cancel-cross-tenant"),
            Some(json!({"reason":"planning_error"})),
        )
        .await,
        StatusCode::NOT_FOUND,
        ErrorReason::NotFound,
        "cross-tenant work is concealed",
    )
    .await;

    rig.set_scope(Vec::new(), vec![rig.inventory_owner_id])
        .await;
    for (key, reason) in [
        ("cancel-scope-success", "planning_error"),
        ("cancel-scope-success", "source_unavailable"),
        ("cancel-scope-new-key", "planning_error"),
    ] {
        assert_error_reason(
            cancel(
                &rig,
                work_id,
                key,
                reason,
                Some("source selection was invalid"),
            )
            .await,
            StatusCode::NOT_FOUND,
            ErrorReason::NotFound,
            "facility-revoked cancellation is concealed",
        )
        .await;
    }
    rig.set_scope(vec![rig.facility_id], Vec::new()).await;
    assert_error_reason(
        cancel(
            &rig,
            work_id,
            "cancel-owner-revoked",
            "planning_error",
            None,
        )
        .await,
        StatusCode::NOT_FOUND,
        ErrorReason::NotFound,
        "owner-revoked cancellation is concealed",
    )
    .await;

    let app_db = app_db_for(&rig.fixture.db).await;
    let unbound_count: i64 = sqlx::query_scalar("SELECT count(*) FROM replenishment_cancellations")
        .fetch_one(&app_db)
        .await
        .unwrap();
    assert_eq!(unbound_count, 0);
    let privileges: (bool, bool, bool, bool) = sqlx::query_as(
        r#"SELECT
          has_table_privilege(current_user,'replenishment_cancellations','SELECT'),
          has_table_privilege(current_user,'replenishment_cancellations','INSERT'),
          has_table_privilege(current_user,'replenishment_cancellations','UPDATE'),
          has_table_privilege(current_user,'replenishment_cancellations','DELETE')"#,
    )
    .fetch_one(&app_db)
    .await
    .unwrap();
    assert_eq!(privileges, (true, true, false, false));
    app_db.close().await;

    let admin = admin_db_for(&rig.fixture.db).await;
    for statement in [
        "UPDATE replenishment_cancellations SET note='tampered' WHERE id=$1",
        "DELETE FROM replenishment_cancellations WHERE id=$1",
    ] {
        let error = sqlx::query(statement)
            .bind(cancelled.cancellation_id)
            .execute(&admin)
            .await
            .expect_err("cancellation evidence is immutable");
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("55000")
        );
    }
    admin.close().await;
}

#[tokio::test]
async fn cancellation_and_operator_claim_race_has_one_winner_and_no_stranded_source_claim() {
    init_test_tracing();
    let rig = ReplenishmentFixture::new("replenishment-cancel-race").await;
    let (_, work_id) = planned_work(&rig, "CANCEL-RACE").await;
    let claim_future = rig.claim_by_id(work_id, "cancel-race-claim");
    let cancel_future = cancel(&rig, work_id, "cancel-race-cancel", "demand_removed", None);
    let (claim, cancellation) = tokio::join!(claim_future, cancel_future);
    let statuses = (claim.status(), cancellation.status());
    assert!(matches!(
        statuses,
        (StatusCode::OK, StatusCode::CONFLICT) | (StatusCode::CONFLICT, StatusCode::OK)
    ));

    let mut tx = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    let row = sqlx::query(
        r#"SELECT work.status,claim.released_at,
          (SELECT count(*) FROM replenishment_cancellations evidence
             WHERE evidence.tenant_id=work.tenant_id AND evidence.task_id=work.id) evidence_count
        FROM work_tasks work
        JOIN loose_inventory_movement_claims claim ON claim.tenant_id=work.tenant_id
          AND claim.work_kind='replenishment' AND claim.work_task_id=work.id
        WHERE work.tenant_id=$1 AND work.id=$2"#,
    )
    .bind(rig.access.tenant_id.get())
    .bind(work_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    if cancellation.status() == StatusCode::OK {
        assert_eq!(row.get::<String, _>("status"), "cancelled");
        assert!(row
            .get::<Option<wareboxes_domain::Timestamp>, _>("released_at")
            .is_some());
        assert_eq!(row.get::<i64, _>("evidence_count"), 1);
    } else {
        assert_eq!(row.get::<String, _>("status"), "in_progress");
        assert!(row
            .get::<Option<wareboxes_domain::Timestamp>, _>("released_at")
            .is_none());
        assert_eq!(row.get::<i64, _>("evidence_count"), 0);
    }
    tx.rollback().await.unwrap();
}
