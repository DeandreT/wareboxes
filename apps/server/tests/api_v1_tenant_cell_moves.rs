mod common;

#[path = "api_v1_data_cells/support.rs"]
mod support;

use axum::http::{Method, StatusCode};
use sqlx::Row;
use std::time::{Duration, Instant};
use tower::ServiceExt;
use wareboxes_api::{auth, routes, state::AppState};
use wareboxes_api_contract::v1::{
    CheckpointTenantCellMoveRequest, CompleteTenantCellMoveRequest, CreateTenantRequest,
    CutoverTenantCellMoveRequest, DataCellMode, FreezeTenantCellMoveRequest,
    PlanTenantCellMoveRequest, Revision, StartTenantCellMoveCopyRequest, TenantCellMoveAction,
    TenantCellMoveBlocker, TenantCellMoveCheckpointEvidence,
    TenantCellMoveCutoverVerificationEvidence, TenantCellMoveEventAction, TenantCellMoveEventPage,
    TenantCellMoveResponse, TenantCellMoveStatus, TenantCellMoveValidationEvidence,
    ValidateTenantCellMoveRequest, VerifyTenantCellMoveCutoverRequest,
};

use common::*;
use support::{
    grant_platform_administrator, register_and_activate, request, response, ActiveDataCell,
};

const CONTROL_PLANE_BURST_SIZE: usize = 16;
const CONTROL_PLANE_BURST_BUDGET: Duration = Duration::from_secs(5);

fn checksum(value: char) -> String {
    value.to_string().repeat(64)
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("wareboxes_api=debug")
        .with_test_writer()
        .try_init();
}

fn checkpoint(
    revision: Revision,
    source_lsn: &str,
    target_lsn: &str,
) -> CheckpointTenantCellMoveRequest {
    CheckpointTenantCellMoveRequest {
        expected_revision: revision,
        checkpoint: TenantCellMoveCheckpointEvidence {
            source_lsn: source_lsn.into(),
            target_replay_lsn: target_lsn.into(),
            copied_row_count: 42,
            copied_bytes: 4096,
        },
    }
}

fn validation(revision: Revision) -> ValidateTenantCellMoveRequest {
    ValidateTenantCellMoveRequest {
        expected_revision: revision,
        validation: TenantCellMoveValidationEvidence {
            tool_version: "cell-validator/1.0.0".into(),
            source_lsn: "0/20".into(),
            target_replay_lsn: "0/20".into(),
            source_row_count: 42,
            target_row_count: 42,
            source_data_checksum: checksum('a'),
            target_data_checksum: checksum('a'),
            source_schema_checksum: checksum('b'),
            target_schema_checksum: checksum('b'),
            source_object_manifest_checksum: checksum('c'),
            target_object_manifest_checksum: checksum('c'),
            inventory_reconciled: true,
            idempotency_verified: true,
            outbox_verified: true,
        },
    }
}

async fn create_tenant(
    app: &axum::Router,
    token: &str,
    home: TenantId,
    slug: &str,
    administrator_email: &str,
    data_cell_id: i64,
) -> wareboxes_api_contract::v1::TenantLifecycleResponse {
    response(
        app.clone()
            .oneshot(request(
                token,
                home,
                Method::POST,
                "/api/v1/platform/tenants",
                Some(&format!("create-{slug}")),
                &CreateTenantRequest {
                    slug: slug.into(),
                    name: format!("Tenant {slug}"),
                    administrator_email: administrator_email.into(),
                    data_cell_id,
                    residency_requirement: "US".into(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await
}

async fn tenant_membership_write_result(
    db: &wareboxes_api::db::Db,
    tenant_id: TenantId,
) -> Result<(), String> {
    let mut transaction = wareboxes_api::db::begin_tenant_transaction(db, tenant_id)
        .await
        .unwrap();
    let result =
        sqlx::query("UPDATE tenant_memberships SET is_default=is_default WHERE tenant_id=$1")
            .bind(tenant_id.get())
            .execute(&mut *transaction)
            .await;
    match result {
        Ok(_) => {
            transaction.commit().await.unwrap();
            Ok(())
        }
        Err(error) => {
            let code = error
                .as_database_error()
                .and_then(|database_error| database_error.code())
                .map(|code| code.into_owned())
                .unwrap_or_else(|| format!("unexpected SQL error: {error}"));
            Err(code)
        }
    }
}

async fn tenant_membership_write(db: &wareboxes_api::db::Db, tenant_id: TenantId) -> bool {
    tenant_membership_write_result(db, tenant_id).await.is_ok()
}

async fn force_data_cell_viability(
    admin_db: &wareboxes_api::db::Db,
    data_cell_id: i64,
    status: &str,
    residency_code: &str,
    max_tenants: i64,
) {
    let mut tx = admin_db.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        r#"UPDATE data_cells
        SET status=$2,residency_code=$3,max_tenants=$4
        WHERE id=$1"#,
    )
    .bind(data_cell_id)
    .bind(status)
    .bind(residency_code)
    .bind(max_tenants)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn governed_move_fences_validates_cuts_over_and_completes_atomically() {
    init_tracing();
    let fixture = Fixture::new().await;
    let platform_admin = fixture.user("move-platform-admin@test.local").await;
    let home = tenant_for_user(&fixture.db, platform_admin.id).await;
    grant_platform_administrator(&fixture.db, platform_admin.id).await;
    let token = auth::create_session(&fixture.db, platform_admin.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let source = register_and_activate(
        &app,
        &token,
        home,
        ActiveDataCell {
            key: "move-source-us",
            region: "us-west-2",
            residency: "US",
            mode: DataCellMode::Shared,
            capacity: 1,
        },
    )
    .await;
    let target = register_and_activate(
        &app,
        &token,
        home,
        ActiveDataCell {
            key: "move-target-us",
            region: "us-east-1",
            residency: "US",
            mode: DataCellMode::Shared,
            capacity: 2,
        },
    )
    .await;
    let tenant = create_tenant(
        &app,
        &token,
        home,
        "move-acme",
        &platform_admin.email,
        source.data_cell_id,
    )
    .await;
    let moved_tenant_id = TenantId::new(tenant.tenant_id).unwrap();
    assert!(tenant_membership_write(&fixture.db, moved_tenant_id).await);

    let plan_request = PlanTenantCellMoveRequest {
        target_data_cell_id: target.data_cell_id,
        expected_placement_revision: tenant.placement_revision,
        reason: "evacuate source under INC-42".into(),
    };
    let planned: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!("/api/v1/platform/tenants/{}/cell-moves", tenant.tenant_id),
                Some("plan-move-acme"),
                &plan_request,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(planned.status, TenantCellMoveStatus::Planned);
    assert_eq!(planned.target_cell.reserved_inbound_move_count, 1);
    assert_eq!(planned.target_cell.available_tenant_slots, 1);

    let replay: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!("/api/v1/platform/tenants/{}/cell-moves", tenant.tenant_id),
                Some("plan-move-acme"),
                &plan_request,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay, planned);

    let mismatched_replay = app
        .clone()
        .oneshot(request(
            &token,
            home,
            Method::POST,
            &format!("/api/v1/platform/tenants/{}/cell-moves", tenant.tenant_id),
            Some("plan-move-acme"),
            &PlanTenantCellMoveRequest {
                reason: "attempt to reuse the key with another request".into(),
                ..plan_request.clone()
            },
        ))
        .await
        .unwrap();
    assert_eq!(mismatched_replay.status(), StatusCode::CONFLICT);

    let wrong_context = app
        .clone()
        .oneshot(request(
            &token,
            moved_tenant_id,
            Method::POST,
            &format!(
                "/api/v1/platform/tenant-cell-moves/{}/copy-starts",
                planned.tenant_cell_move_id
            ),
            Some("reject-moved-context"),
            &StartTenantCellMoveCopyRequest {
                expected_revision: planned.revision,
                copy_reference: "copy/INC-42".into(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(wrong_context.status(), StatusCode::BAD_REQUEST);

    let copying: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}/copy-starts",
                    planned.tenant_cell_move_id
                ),
                Some("start-copy-acme"),
                &StartTenantCellMoveCopyRequest {
                    expected_revision: planned.revision,
                    copy_reference: "copy/INC-42".into(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let stale_revision = app
        .clone()
        .oneshot(request(
            &token,
            home,
            Method::POST,
            &format!(
                "/api/v1/platform/tenant-cell-moves/{}/checkpoints",
                planned.tenant_cell_move_id
            ),
            Some("reject-stale-move-revision"),
            &checkpoint(planned.revision, "0/20", "0/10"),
        ))
        .await
        .unwrap();
    assert_eq!(stale_revision.status(), StatusCode::CONFLICT);
    let checkpointed: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}/checkpoints",
                    planned.tenant_cell_move_id
                ),
                Some("checkpoint-copy-acme"),
                &checkpoint(copying.revision, "0/20", "0/10"),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let frozen: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}/write-freezes",
                    planned.tenant_cell_move_id
                ),
                Some("freeze-move-acme"),
                &FreezeTenantCellMoveRequest {
                    expected_revision: checkpointed.revision,
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert!(frozen.write_frozen);
    assert!(!tenant_membership_write(&fixture.db, moved_tenant_id).await);

    let frozen_checkpoint: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}/checkpoints",
                    planned.tenant_cell_move_id
                ),
                Some("checkpoint-frozen-acme"),
                &checkpoint(frozen.revision, "0/20", "0/20"),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;

    let admin_db = admin_db_for(&fixture.db).await;
    let mut attack = admin_db.begin().await.unwrap();
    sqlx::query("SELECT set_config('wareboxes.platform_actor_user_id',$1,TRUE)")
        .bind(platform_admin.id.to_string())
        .execute(&mut *attack)
        .await
        .unwrap();
    let erase_copy = sqlx::query(
        r#"UPDATE tenant_cell_moves SET copy_reference=NULL,status='validated',
        revision=revision+1,last_action='validated',changed_at=CURRENT_TIMESTAMP,
        changed_by_user_id=$2,validated_at=CURRENT_TIMESTAMP,validated_by_user_id=$2
        WHERE id=$1"#,
    )
    .bind(planned.tenant_cell_move_id)
    .bind(platform_admin.id)
    .execute(&mut *attack)
    .await;
    assert!(erase_copy.is_err());
    attack.rollback().await.unwrap();

    let validated: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}/validations",
                    planned.tenant_cell_move_id
                ),
                Some("validate-move-acme"),
                &validation(frozen_checkpoint.revision),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let cutover_request = CutoverTenantCellMoveRequest {
        expected_revision: validated.revision,
        expected_placement_revision: tenant.placement_revision,
    };
    let cut_over: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}/cutovers",
                    planned.tenant_cell_move_id
                ),
                Some("cutover-move-acme"),
                &cutover_request,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(cut_over.status, TenantCellMoveStatus::CutOver);
    assert!(cut_over.write_frozen);
    assert_eq!(cut_over.source_cell.reserved_rollback_move_count, 1);
    let rollback_slot_conflict = app
        .clone()
        .oneshot(request(
            &token,
            home,
            Method::POST,
            "/api/v1/platform/tenants",
            Some("preserve-source-rollback-slot"),
            &CreateTenantRequest {
                slug: "move-rollback-slot-intruder".into(),
                name: "Move rollback slot intruder".into(),
                administrator_email: platform_admin.email.clone(),
                data_cell_id: source.data_cell_id,
                residency_requirement: "US".into(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(rollback_slot_conflict.status(), StatusCode::CONFLICT);
    assert!(cut_over
        .action_eligibility
        .iter()
        .find(|eligibility| eligibility.action == TenantCellMoveAction::Complete)
        .unwrap()
        .blockers
        .contains(&TenantCellMoveBlocker::PostCutoverVerificationMissing));

    let placement_revision = cut_over.cutover_placement_revision.unwrap();
    let verified: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}/cutover-verifications",
                    planned.tenant_cell_move_id
                ),
                Some("verify-cutover-acme"),
                &VerifyTenantCellMoveCutoverRequest {
                    expected_revision: cut_over.revision,
                    verification: TenantCellMoveCutoverVerificationEvidence {
                        tool_version: "cell-validator/1.0.0".into(),
                        routing_reference: "route/INC-42".into(),
                        observed_data_cell_id: target.data_cell_id,
                        observed_placement_revision: placement_revision,
                        routing_verified: true,
                        target_read_verified: true,
                        write_fence_verified: true,
                        inventory_reconciled: true,
                        idempotency_verified: true,
                        outbox_verified: true,
                    },
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert!(verified.cutover_verification.is_some());
    let completion_request = CompleteTenantCellMoveRequest {
        expected_revision: verified.revision,
        reason: "routing and target health verified under INC-42".into(),
    };
    let completed: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}/completions",
                    planned.tenant_cell_move_id
                ),
                Some("complete-move-acme"),
                &completion_request,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(completed.status, TenantCellMoveStatus::Completed);
    assert!(!completed.write_frozen);
    assert_eq!(completed.source_cell.reserved_rollback_move_count, 0);
    assert!(tenant_membership_write(&fixture.db, moved_tenant_id).await);

    let replayed_cutover: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}/cutovers",
                    planned.tenant_cell_move_id
                ),
                Some("cutover-move-acme"),
                &cutover_request,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(replayed_cutover, cut_over);
    let replayed_completion: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}/completions",
                    planned.tenant_cell_move_id
                ),
                Some("complete-move-acme"),
                &completion_request,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(replayed_completion, completed);

    let events: TenantCellMoveEventPage = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::GET,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}/events?limit=20",
                    planned.tenant_cell_move_id
                ),
                None,
                &(),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(events.items.len(), 9);
    assert_eq!(events.items[0].action, TenantCellMoveEventAction::Completed);
    assert_eq!(events.items[0].request_id, "complete-move-acme");
    assert_eq!(
        events.items[1].action,
        TenantCellMoveEventAction::PostCutoverVerified
    );

    let evidence = sqlx::query(
        r#"SELECT
        (SELECT data_cell_id FROM tenant_cell_placements WHERE tenant_id=$1) placement_cell,
        (SELECT revision FROM tenant_cell_placements WHERE tenant_id=$1) placement_revision,
        (SELECT COUNT(*) FROM tenant_write_fences WHERE tenant_id=$1) fences,
        (SELECT COUNT(*) FROM tenant_cell_move_events WHERE tenant_cell_move_id=$2) events,
        (SELECT COUNT(*) FROM outbox_events WHERE tenant_id=$1
          AND aggregate_type='tenant_cell_move' AND aggregate_id=$2::TEXT) outbox_events"#,
    )
    .bind(tenant.tenant_id)
    .bind(planned.tenant_cell_move_id)
    .fetch_one(&admin_db)
    .await
    .unwrap();
    assert_eq!(
        evidence.get::<i64, _>("placement_cell"),
        target.data_cell_id
    );
    assert_eq!(evidence.get::<i64, _>("placement_revision"), 2);
    assert_eq!(evidence.get::<i64, _>("fences"), 0);
    assert_eq!(evidence.get::<i64, _>("events"), 9);
    assert_eq!(evidence.get::<i64, _>("outbox_events"), 9);
    admin_db.close().await;
}

#[tokio::test]
async fn move_phases_recheck_target_viability_and_post_freeze_checkpoint_revision() {
    init_tracing();
    let fixture = Fixture::new().await;
    let platform_admin = fixture.user("move-recheck-platform-admin@test.local").await;
    let home = tenant_for_user(&fixture.db, platform_admin.id).await;
    grant_platform_administrator(&fixture.db, platform_admin.id).await;
    let token = auth::create_session(&fixture.db, platform_admin.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let source = register_and_activate(
        &app,
        &token,
        home,
        ActiveDataCell {
            key: "move-recheck-source",
            region: "us-west-2",
            residency: "US",
            mode: DataCellMode::Shared,
            capacity: 4,
        },
    )
    .await;
    let target = register_and_activate(
        &app,
        &token,
        home,
        ActiveDataCell {
            key: "move-recheck-target",
            region: "us-east-2",
            residency: "US",
            mode: DataCellMode::Shared,
            capacity: 2,
        },
    )
    .await;
    let first_tenant = create_tenant(
        &app,
        &token,
        home,
        "move-recheck-first",
        &platform_admin.email,
        source.data_cell_id,
    )
    .await;
    let second_tenant = create_tenant(
        &app,
        &token,
        home,
        "move-recheck-second",
        &platform_admin.email,
        source.data_cell_id,
    )
    .await;
    let first: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenants/{}/cell-moves",
                    first_tenant.tenant_id
                ),
                Some("plan-rechecked-move"),
                &PlanTenantCellMoveRequest {
                    target_data_cell_id: target.data_cell_id,
                    expected_placement_revision: first_tenant.placement_revision,
                    reason: "prove target viability is checked at every guarded phase".into(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let _: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenants/{}/cell-moves",
                    second_tenant.tenant_id
                ),
                Some("plan-capacity-reservation-peer"),
                &PlanTenantCellMoveRequest {
                    target_data_cell_id: target.data_cell_id,
                    expected_placement_revision: second_tenant.placement_revision,
                    reason: "reserve the remaining target slot".into(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;

    let admin_db = admin_db_for(&fixture.db).await;
    force_data_cell_viability(&admin_db, target.data_cell_id, "draining", "US", 2).await;
    let blocked_start: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::GET,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}",
                    first.tenant_cell_move_id
                ),
                None,
                &(),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert!(blocked_start
        .action_eligibility
        .iter()
        .find(|eligibility| eligibility.action == TenantCellMoveAction::StartCopy)
        .unwrap()
        .blockers
        .contains(&TenantCellMoveBlocker::TargetNotActive));
    let start_conflict = app
        .clone()
        .oneshot(request(
            &token,
            home,
            Method::POST,
            &format!(
                "/api/v1/platform/tenant-cell-moves/{}/copy-starts",
                first.tenant_cell_move_id
            ),
            Some("reject-copy-to-draining-target"),
            &StartTenantCellMoveCopyRequest {
                expected_revision: first.revision,
                copy_reference: "copy/rechecked-move".into(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(start_conflict.status(), StatusCode::CONFLICT);

    force_data_cell_viability(&admin_db, target.data_cell_id, "active", "US", 2).await;
    let copying: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}/copy-starts",
                    first.tenant_cell_move_id
                ),
                Some("start-rechecked-copy"),
                &StartTenantCellMoveCopyRequest {
                    expected_revision: first.revision,
                    copy_reference: "copy/rechecked-move".into(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let checkpointed: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}/checkpoints",
                    first.tenant_cell_move_id
                ),
                Some("checkpoint-before-rechecked-freeze"),
                &checkpoint(copying.revision, "0/20", "0/10"),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;

    force_data_cell_viability(&admin_db, target.data_cell_id, "active", "CA", 2).await;
    let blocked_freeze: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::GET,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}",
                    first.tenant_cell_move_id
                ),
                None,
                &(),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert!(blocked_freeze
        .action_eligibility
        .iter()
        .find(|eligibility| eligibility.action == TenantCellMoveAction::Freeze)
        .unwrap()
        .blockers
        .contains(&TenantCellMoveBlocker::ResidencyMismatch));
    let freeze_conflict = app
        .clone()
        .oneshot(request(
            &token,
            home,
            Method::POST,
            &format!(
                "/api/v1/platform/tenant-cell-moves/{}/write-freezes",
                first.tenant_cell_move_id
            ),
            Some("reject-freeze-for-residency-mismatch"),
            &FreezeTenantCellMoveRequest {
                expected_revision: checkpointed.revision,
            },
        ))
        .await
        .unwrap();
    assert_eq!(freeze_conflict.status(), StatusCode::CONFLICT);

    force_data_cell_viability(&admin_db, target.data_cell_id, "active", "US", 2).await;
    let frozen: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}/write-freezes",
                    first.tenant_cell_move_id
                ),
                Some("freeze-rechecked-move"),
                &FreezeTenantCellMoveRequest {
                    expected_revision: checkpointed.revision,
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;

    let mut timestamp_attack = admin_db.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *timestamp_attack)
        .await
        .unwrap();
    sqlx::query("UPDATE tenant_cell_moves SET checkpointed_at=frozen_at WHERE id=$1")
        .bind(first.tenant_cell_move_id)
        .execute(&mut *timestamp_attack)
        .await
        .unwrap();
    timestamp_attack.commit().await.unwrap();
    let timestamp_only_checkpoint: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::GET,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}",
                    first.tenant_cell_move_id
                ),
                None,
                &(),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert!(timestamp_only_checkpoint
        .action_eligibility
        .iter()
        .find(|eligibility| eligibility.action == TenantCellMoveAction::Validate)
        .unwrap()
        .blockers
        .contains(&TenantCellMoveBlocker::ValidationStale));
    let validation_without_new_checkpoint = app
        .clone()
        .oneshot(request(
            &token,
            home,
            Method::POST,
            &format!(
                "/api/v1/platform/tenant-cell-moves/{}/validations",
                first.tenant_cell_move_id
            ),
            Some("reject-timestamp-only-final-checkpoint"),
            &validation(frozen.revision),
        ))
        .await
        .unwrap();
    assert_eq!(
        validation_without_new_checkpoint.status(),
        StatusCode::CONFLICT
    );

    let final_checkpoint: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}/checkpoints",
                    first.tenant_cell_move_id
                ),
                Some("checkpoint-after-rechecked-freeze"),
                &checkpoint(frozen.revision, "0/20", "0/20"),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;

    force_data_cell_viability(&admin_db, target.data_cell_id, "active", "US", 1).await;
    let blocked_validation: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::GET,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}",
                    first.tenant_cell_move_id
                ),
                None,
                &(),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert!(blocked_validation
        .action_eligibility
        .iter()
        .find(|eligibility| eligibility.action == TenantCellMoveAction::Validate)
        .unwrap()
        .blockers
        .contains(&TenantCellMoveBlocker::TargetCapacityUnavailable));
    let validation_conflict = app
        .clone()
        .oneshot(request(
            &token,
            home,
            Method::POST,
            &format!(
                "/api/v1/platform/tenant-cell-moves/{}/validations",
                first.tenant_cell_move_id
            ),
            Some("reject-validation-without-capacity"),
            &validation(final_checkpoint.revision),
        ))
        .await
        .unwrap();
    assert_eq!(validation_conflict.status(), StatusCode::CONFLICT);

    force_data_cell_viability(&admin_db, target.data_cell_id, "active", "US", 2).await;
    let validated: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}/validations",
                    first.tenant_cell_move_id
                ),
                Some("validate-after-target-recovery"),
                &validation(final_checkpoint.revision),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(validated.status, TenantCellMoveStatus::Validated);
    admin_db.close().await;
}

#[tokio::test]
async fn target_capacity_reservation_is_serialized_between_move_plans() {
    init_tracing();
    let fixture = Fixture::new().await;
    let platform_admin = fixture.user("move-race-platform-admin@test.local").await;
    let home = tenant_for_user(&fixture.db, platform_admin.id).await;
    grant_platform_administrator(&fixture.db, platform_admin.id).await;
    let token = auth::create_session(&fixture.db, platform_admin.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let source = register_and_activate(
        &app,
        &token,
        home,
        ActiveDataCell {
            key: "move-race-source",
            region: "us-west-2",
            residency: "US",
            mode: DataCellMode::Shared,
            capacity: 4,
        },
    )
    .await;
    let target = register_and_activate(
        &app,
        &token,
        home,
        ActiveDataCell {
            key: "move-race-target",
            region: "us-east-1",
            residency: "US",
            mode: DataCellMode::Dedicated,
            capacity: 1,
        },
    )
    .await;
    let first = create_tenant(
        &app,
        &token,
        home,
        "move-race-first",
        &platform_admin.email,
        source.data_cell_id,
    )
    .await;
    let second = create_tenant(
        &app,
        &token,
        home,
        "move-race-second",
        &platform_admin.email,
        source.data_cell_id,
    )
    .await;

    let first_plan = app.clone().oneshot(request(
        &token,
        home,
        Method::POST,
        &format!("/api/v1/platform/tenants/{}/cell-moves", first.tenant_id),
        Some("race-first-move"),
        &PlanTenantCellMoveRequest {
            target_data_cell_id: target.data_cell_id,
            expected_placement_revision: first.placement_revision,
            reason: "dedicated target race first".into(),
        },
    ));
    let second_plan = app.clone().oneshot(request(
        &token,
        home,
        Method::POST,
        &format!("/api/v1/platform/tenants/{}/cell-moves", second.tenant_id),
        Some("race-second-move"),
        &PlanTenantCellMoveRequest {
            target_data_cell_id: target.data_cell_id,
            expected_placement_revision: second.placement_revision,
            reason: "dedicated target race second".into(),
        },
    ));
    let (first_response, second_response) = tokio::join!(first_plan, second_plan);
    let statuses = [
        first_response.unwrap().status(),
        second_response.unwrap().status(),
    ];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CONFLICT)
            .count(),
        1
    );

    let admin_db = admin_db_for(&fixture.db).await;
    let reservations: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM tenant_cell_moves
        WHERE target_data_cell_id=$1 AND status='planned'"#,
    )
    .bind(target.data_cell_id)
    .fetch_one(&admin_db)
    .await
    .unwrap();
    assert_eq!(reservations, 1);
    admin_db.close().await;
}

#[tokio::test]
async fn concurrent_plan_burst_reserves_every_shared_target_slot_within_budget() {
    init_tracing();
    let fixture = Fixture::new().await;
    let platform_admin = fixture.user("move-burst-platform-admin@test.local").await;
    let home = tenant_for_user(&fixture.db, platform_admin.id).await;
    grant_platform_administrator(&fixture.db, platform_admin.id).await;
    let token = auth::create_session(&fixture.db, platform_admin.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let source = register_and_activate(
        &app,
        &token,
        home,
        ActiveDataCell {
            key: "move-burst-source",
            region: "us-west-2",
            residency: "US",
            mode: DataCellMode::Shared,
            capacity: 32,
        },
    )
    .await;
    let target = register_and_activate(
        &app,
        &token,
        home,
        ActiveDataCell {
            key: "move-burst-target",
            region: "us-east-1",
            residency: "US",
            mode: DataCellMode::Shared,
            capacity: 32,
        },
    )
    .await;

    let mut tenants = Vec::with_capacity(CONTROL_PLANE_BURST_SIZE);
    for index in 0..CONTROL_PLANE_BURST_SIZE {
        let slug = format!("move-burst-{index:02}");
        tenants.push(
            create_tenant(
                &app,
                &token,
                home,
                &slug,
                &platform_admin.email,
                source.data_cell_id,
            )
            .await,
        );
    }

    // Build every request before starting the measured concurrent window.
    let operations = tenants
        .into_iter()
        .enumerate()
        .map(|(index, tenant)| {
            app.clone().oneshot(request(
                &token,
                home,
                Method::POST,
                &format!("/api/v1/platform/tenants/{}/cell-moves", tenant.tenant_id),
                Some(&format!("plan-shared-target-burst-{index:02}")),
                &PlanTenantCellMoveRequest {
                    target_data_cell_id: target.data_cell_id,
                    expected_placement_revision: tenant.placement_revision,
                    reason: "measured shared-target reservation burst".into(),
                },
            ))
        })
        .collect::<Vec<_>>();

    let started_at = Instant::now();
    let responses = tokio::time::timeout(CONTROL_PLANE_BURST_BUDGET, async move {
        let mut tasks = tokio::task::JoinSet::new();
        for operation in operations {
            tasks.spawn(operation);
        }
        let mut responses = Vec::with_capacity(CONTROL_PLANE_BURST_SIZE);
        while let Some(result) = tasks.join_next().await {
            responses.push(
                result
                    .expect("concurrent plan task completes")
                    .expect("concurrent plan service remains available"),
            );
        }
        responses
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "{CONTROL_PLANE_BURST_SIZE} concurrent plan commands exceeded the {CONTROL_PLANE_BURST_BUDGET:?} budget"
        )
    });
    let elapsed = started_at.elapsed();

    assert_eq!(responses.len(), CONTROL_PLANE_BURST_SIZE);
    assert!(
        responses
            .iter()
            .all(|response| response.status() == StatusCode::OK),
        "all concurrent plans must succeed without HTTP errors: {:?}",
        responses
            .iter()
            .map(|response| response.status())
            .collect::<Vec<_>>()
    );
    assert!(
        elapsed <= CONTROL_PLANE_BURST_BUDGET,
        "concurrent plan burst took {elapsed:?}"
    );

    let admin_db = admin_db_for(&fixture.db).await;
    let reservations: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM tenant_cell_moves
        WHERE target_data_cell_id=$1
          AND status IN ('planned','copying','frozen','validated')"#,
    )
    .bind(target.data_cell_id)
    .fetch_one(&admin_db)
    .await
    .unwrap();
    assert_eq!(reservations, CONTROL_PLANE_BURST_SIZE as i64);
    admin_db.close().await;
}

#[tokio::test]
async fn write_freeze_waits_for_an_in_flight_tenant_command_before_fencing() {
    init_tracing();
    let fixture = Fixture::new().await;
    let platform_admin = fixture.user("move-fence-platform-admin@test.local").await;
    let home = tenant_for_user(&fixture.db, platform_admin.id).await;
    grant_platform_administrator(&fixture.db, platform_admin.id).await;
    let token = auth::create_session(&fixture.db, platform_admin.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let source = register_and_activate(
        &app,
        &token,
        home,
        ActiveDataCell {
            key: "move-fence-source",
            region: "us-west-2",
            residency: "US",
            mode: DataCellMode::Shared,
            capacity: 4,
        },
    )
    .await;
    let target = register_and_activate(
        &app,
        &token,
        home,
        ActiveDataCell {
            key: "move-fence-target",
            region: "us-east-1",
            residency: "US",
            mode: DataCellMode::Shared,
            capacity: 4,
        },
    )
    .await;
    let tenant = create_tenant(
        &app,
        &token,
        home,
        "move-fence-tenant",
        &platform_admin.email,
        source.data_cell_id,
    )
    .await;
    let tenant_id = TenantId::new(tenant.tenant_id).unwrap();
    let planned: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!("/api/v1/platform/tenants/{}/cell-moves", tenant.tenant_id),
                Some("plan-fence-serialization"),
                &PlanTenantCellMoveRequest {
                    target_data_cell_id: target.data_cell_id,
                    expected_placement_revision: tenant.placement_revision,
                    reason: "prove in-flight command serialization".into(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let copying: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}/copy-starts",
                    planned.tenant_cell_move_id
                ),
                Some("start-fence-serialization-copy"),
                &StartTenantCellMoveCopyRequest {
                    expected_revision: planned.revision,
                    copy_reference: "copy/fence-serialization".into(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let checkpointed: TenantCellMoveResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenant-cell-moves/{}/checkpoints",
                    planned.tenant_cell_move_id
                ),
                Some("checkpoint-fence-serialization"),
                &checkpoint(copying.revision, "0/20", "0/10"),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;

    let mut in_flight = wareboxes_api::db::begin_tenant_transaction(&fixture.db, tenant_id)
        .await
        .unwrap();
    sqlx::query("UPDATE tenant_memberships SET is_default=is_default WHERE tenant_id=$1")
        .bind(tenant_id.get())
        .execute(&mut *in_flight)
        .await
        .unwrap();

    let freeze = app.clone().oneshot(request(
        &token,
        home,
        Method::POST,
        &format!(
            "/api/v1/platform/tenant-cell-moves/{}/write-freezes",
            planned.tenant_cell_move_id
        ),
        Some("freeze-after-in-flight-command"),
        &FreezeTenantCellMoveRequest {
            expected_revision: checkpointed.revision,
        },
    ));
    tokio::pin!(freeze);
    assert!(
        tokio::time::timeout(Duration::from_millis(250), &mut freeze)
            .await
            .is_err(),
        "write freeze must wait for the in-flight tenant transaction"
    );

    in_flight.commit().await.unwrap();
    let frozen: TenantCellMoveResponse = response(
        tokio::time::timeout(Duration::from_secs(5), freeze)
            .await
            .expect("freeze completes after the in-flight command")
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(frozen.status, TenantCellMoveStatus::Frozen);
    assert!(frozen.write_frozen);
    assert!(!tenant_membership_write(&fixture.db, tenant_id).await);

    // Prepare each mutation future before starting the measured fenced window.
    let mutation_attempts = (0..CONTROL_PLANE_BURST_SIZE)
        .map(|_| {
            let db = fixture.db.clone();
            async move { tenant_membership_write_result(&db, tenant_id).await }
        })
        .collect::<Vec<_>>();
    let started_at = Instant::now();
    let outcomes = tokio::time::timeout(CONTROL_PLANE_BURST_BUDGET, async move {
        let mut tasks = tokio::task::JoinSet::new();
        for attempt in mutation_attempts {
            tasks.spawn(attempt);
        }
        let mut outcomes = Vec::with_capacity(CONTROL_PLANE_BURST_SIZE);
        while let Some(result) = tasks.join_next().await {
            outcomes.push(result.expect("fenced mutation task completes"));
        }
        outcomes
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "{CONTROL_PLANE_BURST_SIZE} fenced tenant mutations exceeded the {CONTROL_PLANE_BURST_BUDGET:?} budget"
        )
    });
    let elapsed = started_at.elapsed();

    assert_eq!(outcomes.len(), CONTROL_PLANE_BURST_SIZE);
    assert!(
        outcomes
            .iter()
            .all(|outcome| outcome.as_ref().is_err_and(|code| code == "55000")),
        "every tenant mutation must fail closed with the write-fence SQLSTATE: {outcomes:?}"
    );
    assert!(
        elapsed <= CONTROL_PLANE_BURST_BUDGET,
        "fenced mutation burst took {elapsed:?}"
    );
}
