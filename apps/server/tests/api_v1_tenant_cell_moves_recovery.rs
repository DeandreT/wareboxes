mod common;

#[path = "api_v1_data_cells/support.rs"]
mod support;

use axum::http::{Method, StatusCode};
use chrono::Timelike;
use serde::Serialize;
use tower::ServiceExt;
use wareboxes_api::{auth, routes, state::AppState};
use wareboxes_api_contract::v1::{
    CancelTenantCellMoveRequest, ChangeTenantStatusRequest, CheckpointTenantCellMoveRequest,
    CreateServiceAccountRequest, CreateTenantRequest, CutoverTenantCellMoveRequest, DataCellMode,
    DataCellResponse, FreezeTenantCellMoveRequest, IssueServiceAccountCredentialRequest,
    IssuedServiceAccountCredentialResponse, PlanTenantCellMoveRequest, Revision,
    RollbackTenantCellMoveRequest, ServiceAccountAccessRequest, ServiceAccountResponse,
    StartTenantCellMoveCopyRequest, TenantCellMoveCheckpointEvidence,
    TenantCellMoveCutoverVerificationEvidence, TenantCellMoveResponse,
    TenantCellMoveRollbackVerificationEvidence, TenantCellMoveStatus,
    TenantCellMoveValidationEvidence, TenantLifecycleResponse, TenantStatus,
    ValidateTenantCellMoveRequest, VerifyTenantCellMoveCutoverRequest,
};

use common::*;
use support::{
    grant_platform_administrator, register_and_activate, request, response, ActiveDataCell,
};

struct MoveFixture {
    fixture: Fixture,
    platform_admin_id: i64,
    platform_admin_email: String,
    home: TenantId,
    token: String,
    app: axum::Router,
    source: DataCellResponse,
    target: DataCellResponse,
}

async fn move_fixture(case: &str) -> MoveFixture {
    let fixture = Fixture::new().await;
    let platform_admin_email = format!("{case}-platform-admin@test.local");
    let platform_admin = fixture.user(&platform_admin_email).await;
    let home = tenant_for_user(&fixture.db, platform_admin.id).await;
    grant_platform_administrator(&fixture.db, platform_admin.id).await;
    let token = auth::create_session(&fixture.db, platform_admin.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let source_key = format!("{case}-source");
    let target_key = format!("{case}-target");
    let source = register_and_activate(
        &app,
        &token,
        home,
        ActiveDataCell {
            key: &source_key,
            region: "us-west-2",
            residency: "US",
            mode: DataCellMode::Shared,
            capacity: 8,
        },
    )
    .await;
    let target = register_and_activate(
        &app,
        &token,
        home,
        ActiveDataCell {
            key: &target_key,
            region: "us-east-1",
            residency: "US",
            mode: DataCellMode::Shared,
            capacity: 8,
        },
    )
    .await;
    MoveFixture {
        fixture,
        platform_admin_id: platform_admin.id,
        platform_admin_email,
        home,
        token,
        app,
        source,
        target,
    }
}

async fn create_moved_tenant(rig: &MoveFixture, slug: &str) -> TenantLifecycleResponse {
    response(
        rig.app
            .clone()
            .oneshot(request(
                &rig.token,
                rig.home,
                Method::POST,
                "/api/v1/platform/tenants",
                Some(&format!("create-{slug}")),
                &CreateTenantRequest {
                    slug: slug.into(),
                    name: format!("Tenant {slug}"),
                    administrator_email: rig.platform_admin_email.clone(),
                    data_cell_id: rig.source.data_cell_id,
                    residency_requirement: "US".into(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await
}

async fn plan_move(
    rig: &MoveFixture,
    tenant: &TenantLifecycleResponse,
    key: &str,
) -> TenantCellMoveResponse {
    response(
        rig.app
            .clone()
            .oneshot(request(
                &rig.token,
                rig.home,
                Method::POST,
                &format!("/api/v1/platform/tenants/{}/cell-moves", tenant.tenant_id),
                Some(key),
                &PlanTenantCellMoveRequest {
                    target_data_cell_id: rig.target.data_cell_id,
                    expected_placement_revision: tenant.placement_revision,
                    reason: format!("recovery acceptance move for {key}"),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await
}

async fn move_action<T: Serialize>(
    rig: &MoveFixture,
    move_id: i64,
    action: &str,
    key: &str,
    body: &T,
) -> TenantCellMoveResponse {
    response(
        rig.app
            .clone()
            .oneshot(request(
                &rig.token,
                rig.home,
                Method::POST,
                &format!("/api/v1/platform/tenant-cell-moves/{move_id}/{action}"),
                Some(key),
                body,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await
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

fn checksum(value: char) -> String {
    value.to_string().repeat(64)
}

fn validation(revision: Revision) -> ValidateTenantCellMoveRequest {
    ValidateTenantCellMoveRequest {
        expected_revision: revision,
        validation: TenantCellMoveValidationEvidence {
            tool_version: "cell-recovery-validator/1.0.0".into(),
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

async fn move_to_frozen(
    rig: &MoveFixture,
    planned: TenantCellMoveResponse,
    key: &str,
) -> TenantCellMoveResponse {
    let copying = move_action(
        rig,
        planned.tenant_cell_move_id,
        "copy-starts",
        &format!("{key}-start"),
        &StartTenantCellMoveCopyRequest {
            expected_revision: planned.revision,
            copy_reference: format!("copy/{key}"),
        },
    )
    .await;
    let checkpointed = move_action(
        rig,
        copying.tenant_cell_move_id,
        "checkpoints",
        &format!("{key}-checkpoint"),
        &checkpoint(copying.revision, "0/20", "0/10"),
    )
    .await;
    move_action(
        rig,
        checkpointed.tenant_cell_move_id,
        "write-freezes",
        &format!("{key}-freeze"),
        &FreezeTenantCellMoveRequest {
            expected_revision: checkpointed.revision,
        },
    )
    .await
}

async fn move_to_cut_over(
    rig: &MoveFixture,
    frozen: TenantCellMoveResponse,
    key: &str,
) -> TenantCellMoveResponse {
    let final_checkpoint = move_action(
        rig,
        frozen.tenant_cell_move_id,
        "checkpoints",
        &format!("{key}-final-checkpoint"),
        &checkpoint(frozen.revision, "0/20", "0/20"),
    )
    .await;
    let validated = move_action(
        rig,
        final_checkpoint.tenant_cell_move_id,
        "validations",
        &format!("{key}-validate"),
        &validation(final_checkpoint.revision),
    )
    .await;
    move_action(
        rig,
        validated.tenant_cell_move_id,
        "cutovers",
        &format!("{key}-cutover"),
        &CutoverTenantCellMoveRequest {
            expected_revision: validated.revision,
            expected_placement_revision: validated.source_placement_revision,
        },
    )
    .await
}

async fn tenant_membership_write(db: &db::Db, tenant_id: TenantId) -> bool {
    let mut transaction = db::begin_tenant_transaction(db, tenant_id).await.unwrap();
    let result =
        sqlx::query("UPDATE tenant_memberships SET is_default=is_default WHERE tenant_id=$1")
            .bind(tenant_id.get())
            .execute(&mut *transaction)
            .await;
    if result.is_ok() {
        transaction.commit().await.unwrap();
        true
    } else {
        transaction.rollback().await.unwrap();
        false
    }
}

async fn placement_state(db: &db::Db, tenant_id: TenantId) -> (i64, i64) {
    sqlx::query_as("SELECT data_cell_id,revision FROM tenant_cell_placements WHERE tenant_id=$1")
        .bind(tenant_id.get())
        .fetch_one(db)
        .await
        .unwrap()
}

async fn fence_count(db: &db::Db, tenant_id: TenantId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM tenant_write_fences WHERE tenant_id=$1")
        .bind(tenant_id.get())
        .fetch_one(db)
        .await
        .unwrap()
}

async fn assert_database_rejects_rollback_verification(
    rig: &MoveFixture,
    move_read: &TenantCellMoveResponse,
    observed_data_cell_id: i64,
    expected_rollback_placement_revision: i64,
) {
    let admin_db = admin_db_for(&rig.fixture.db).await;
    let mut transaction = admin_db.begin().await.unwrap();
    sqlx::query("SELECT set_config('wareboxes.platform_actor_user_id',$1,TRUE)")
        .bind(rig.platform_admin_id.to_string())
        .execute(&mut *transaction)
        .await
        .unwrap();
    let move_revision = move_read.revision.get() + 1;
    sqlx::query(
        r#"WITH command_clock AS (SELECT clock_timestamp() AS occurred_at)
        UPDATE tenant_cell_moves AS move SET status='rolled_back',revision=$2,
          last_action='rolled_back',changed_at=command_clock.occurred_at,
          changed_by_user_id=$3,change_reason='direct rollback proof probe',
          rolled_back_at=command_clock.occurred_at,rolled_back_by_user_id=$3,
          rollback_placement_revision=$4
        FROM command_clock WHERE move.id=$1"#,
    )
    .bind(move_read.tenant_cell_move_id)
    .bind(move_revision)
    .bind(rig.platform_admin_id)
    .bind(move_read.cutover_placement_revision.unwrap().get() + 1)
    .execute(&mut *transaction)
    .await
    .unwrap();
    let error = sqlx::query(
        r#"INSERT INTO tenant_cell_move_rollback_verifications(
          tenant_id,tenant_cell_move_id,move_revision,tool_version,routing_reference,
          observed_data_cell_id,expected_rollback_placement_revision,routing_verified,
          source_read_verified,write_fence_verified,inventory_reconciled,
          idempotency_verified,outbox_verified,verified_at,verified_by_user_id)
        SELECT tenant_id,id,revision,'cell-validator/1.0','route/direct-probe',$2,$3,
          TRUE,TRUE,TRUE,TRUE,TRUE,TRUE,rolled_back_at,$4
        FROM tenant_cell_moves WHERE id=$1"#,
    )
    .bind(move_read.tenant_cell_move_id)
    .bind(observed_data_cell_id)
    .bind(expected_rollback_placement_revision)
    .bind(rig.platform_admin_id)
    .execute(&mut *transaction)
    .await
    .unwrap_err();
    assert_eq!(database_code(&error).as_deref(), Some("23514"));
    transaction.rollback().await.unwrap();
    admin_db.close().await;
}

async fn assert_direct_transition_requires_command_and_outbox(
    rig: &MoveFixture,
    planned: &TenantCellMoveResponse,
) {
    let admin_db = admin_db_for(&rig.fixture.db).await;
    let mut transaction = admin_db.begin().await.unwrap();
    sqlx::query("SELECT set_config('wareboxes.platform_actor_user_id',$1,TRUE)")
        .bind(rig.platform_admin_id.to_string())
        .execute(&mut *transaction)
        .await
        .unwrap();
    sqlx::query(
        r#"UPDATE tenant_cell_moves SET status='copying',revision=2,
        last_action='copy_started',changed_at=CURRENT_TIMESTAMP,
        changed_by_user_id=$2,copy_reference='direct/sql-copy',
        copy_started_at=CURRENT_TIMESTAMP,copy_started_by_user_id=$2
        WHERE id=$1 AND revision=1"#,
    )
    .bind(planned.tenant_cell_move_id)
    .bind(rig.platform_admin_id)
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO tenant_cell_move_events(
          tenant_id,tenant_cell_move_id,action,move_revision,previous_status,
          resulting_status,actor_user_id,occurred_at,reason,request_id,evidence)
        SELECT tenant_id,id,'copy_started',revision,'planned',status,$2,changed_at,
          NULL,'direct-sql-transition',jsonb_build_object(
            'tenant_cell_move_id',id,'tenant_id',tenant_id,
            'action','copy_started','move_revision',revision,
            'previous_status','planned','resulting_status',status,
            'source_data_cell_id',source_data_cell_id,
            'target_data_cell_id',target_data_cell_id,
            'source_placement_revision',source_placement_revision,
            'resulting_placement_revision',NULL,
            'copy_reference',copy_reference,'actor_user_id',$2,
            'occurred_at',changed_at)
        FROM tenant_cell_moves WHERE id=$1"#,
    )
    .bind(planned.tenant_cell_move_id)
    .bind(rig.platform_admin_id)
    .execute(&mut *transaction)
    .await
    .unwrap();
    let error = sqlx::query("SET CONSTRAINTS tenant_cell_moves_require_evidence IMMEDIATE")
        .execute(&mut *transaction)
        .await
        .unwrap_err();
    assert_eq!(database_code(&error).as_deref(), Some("23514"));
    transaction.rollback().await.unwrap();

    let persisted: (String, i64) =
        sqlx::query_as("SELECT status,revision FROM tenant_cell_moves WHERE id=$1")
            .bind(planned.tenant_cell_move_id)
            .fetch_one(&admin_db)
            .await
            .unwrap();
    assert_eq!(persisted, ("planned".into(), 1));
    admin_db.close().await;
}

fn database_code(error: &sqlx::Error) -> Option<String> {
    error
        .as_database_error()
        .and_then(|database_error| database_error.code())
        .map(|code| code.into_owned())
}

async fn assert_frozen_state_rejects_direct_tampering(
    rig: &MoveFixture,
    tenant_id: TenantId,
    frozen: &TenantCellMoveResponse,
) {
    let admin_db = admin_db_for(&rig.fixture.db).await;

    let mut fence_tx = admin_db.begin().await.unwrap();
    sqlx::query("SELECT set_config('wareboxes.platform_actor_user_id',$1,TRUE)")
        .bind(rig.platform_admin_id.to_string())
        .execute(&mut *fence_tx)
        .await
        .unwrap();
    let fence_error = sqlx::query(
        "DELETE FROM tenant_write_fences WHERE tenant_id=$1 AND tenant_cell_move_id=$2",
    )
    .bind(tenant_id.get())
    .bind(frozen.tenant_cell_move_id)
    .execute(&mut *fence_tx)
    .await
    .unwrap_err();
    assert_eq!(database_code(&fence_error).as_deref(), Some("23514"));
    fence_tx.rollback().await.unwrap();

    let mut placement_tx = admin_db.begin().await.unwrap();
    sqlx::query("SELECT set_config('wareboxes.platform_actor_user_id',$1,TRUE)")
        .bind(rig.platform_admin_id.to_string())
        .execute(&mut *placement_tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('wareboxes.tenant_cell_placement_move_id',$1,TRUE)")
        .bind(frozen.tenant_cell_move_id.to_string())
        .execute(&mut *placement_tx)
        .await
        .unwrap();
    let placement_error = sqlx::query(
        "UPDATE tenant_cell_placements SET data_cell_id=$2,revision=revision+1 WHERE tenant_id=$1",
    )
    .bind(tenant_id.get())
    .bind(rig.target.data_cell_id)
    .execute(&mut *placement_tx)
    .await
    .unwrap_err();
    assert_eq!(database_code(&placement_error).as_deref(), Some("55000"));
    placement_tx.rollback().await.unwrap();

    assert_eq!(fence_count(&admin_db, tenant_id).await, 1);
    assert_eq!(
        placement_state(&admin_db, tenant_id).await,
        (
            rig.source.data_cell_id,
            frozen.source_placement_revision.get()
        )
    );
    admin_db.close().await;
}

async fn grant_permission(fixture: &Fixture, tenant_id: TenantId, user_id: i64, name: &str) {
    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        &fixture.db,
        tenant_id,
        name,
        Some(name),
    )
    .await
    .unwrap();
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("cell-move-recovery-{name}-{user_id}"),
        Some("Tenant cell move recovery acceptance role"),
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        permission,
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_to_user(&fixture.db, tenant_id, user_id, role)
        .await
        .unwrap();
}

#[tokio::test]
async fn pre_cutover_cancellation_preserves_placement_and_releases_a_frozen_tenant() {
    let rig = move_fixture("cancel-recovery").await;
    let admin_db = admin_db_for(&rig.fixture.db).await;

    let planned_tenant = create_moved_tenant(&rig, "cancel-planned").await;
    let planned_tenant_id = TenantId::new(planned_tenant.tenant_id).unwrap();
    let planned = plan_move(&rig, &planned_tenant, "plan-cancel-planned").await;
    assert_direct_transition_requires_command_and_outbox(&rig, &planned).await;
    let cancelled_planned = move_action(
        &rig,
        planned.tenant_cell_move_id,
        "cancellations",
        "cancel-planned",
        &CancelTenantCellMoveRequest {
            expected_revision: planned.revision,
            reason: "copy was never started".into(),
        },
    )
    .await;
    assert_eq!(cancelled_planned.status, TenantCellMoveStatus::Cancelled);
    assert!(!cancelled_planned.write_frozen);
    assert_eq!(cancelled_planned.target_cell.reserved_inbound_move_count, 0);
    assert_eq!(
        placement_state(&admin_db, planned_tenant_id).await,
        (
            rig.source.data_cell_id,
            planned_tenant.placement_revision.get()
        )
    );
    assert_eq!(fence_count(&admin_db, planned_tenant_id).await, 0);

    let frozen_tenant = create_moved_tenant(&rig, "cancel-frozen").await;
    let frozen_tenant_id = TenantId::new(frozen_tenant.tenant_id).unwrap();
    let planned = plan_move(&rig, &frozen_tenant, "plan-cancel-frozen").await;
    let frozen = move_to_frozen(&rig, planned, "cancel-frozen").await;
    assert_eq!(frozen.status, TenantCellMoveStatus::Frozen);
    assert!(frozen.write_frozen);
    assert_eq!(fence_count(&admin_db, frozen_tenant_id).await, 1);
    assert!(!tenant_membership_write(&rig.fixture.db, frozen_tenant_id).await);
    let reconciliation_minute = chrono::Utc::now()
        .with_second(0)
        .and_then(|timestamp| timestamp.with_nanosecond(0))
        .unwrap();
    let reconciliation = wareboxes_persistence_postgres::inventory_reconciliation::execute(
        &rig.fixture.db,
        frozen_tenant_id,
        "tenant-cell-move-recovery-test",
        reconciliation_minute,
        60,
    )
    .await
    .unwrap();
    assert_eq!(reconciliation.tenant_id, frozen_tenant_id);
    assert_frozen_state_rejects_direct_tampering(&rig, frozen_tenant_id, &frozen).await;

    let cancelled_frozen = move_action(
        &rig,
        frozen.tenant_cell_move_id,
        "cancellations",
        "cancel-frozen",
        &CancelTenantCellMoveRequest {
            expected_revision: frozen.revision,
            reason: "source remains authoritative after failed rehearsal".into(),
        },
    )
    .await;
    assert_eq!(cancelled_frozen.status, TenantCellMoveStatus::Cancelled);
    assert_eq!(cancelled_frozen.revision.get(), frozen.revision.get() + 1);
    assert!(!cancelled_frozen.write_frozen);
    assert_eq!(cancelled_frozen.cutover_placement_revision, None);
    assert_eq!(cancelled_frozen.rollback_placement_revision, None);
    assert_eq!(cancelled_frozen.target_cell.reserved_inbound_move_count, 0);
    assert_eq!(fence_count(&admin_db, frozen_tenant_id).await, 0);
    assert_eq!(
        placement_state(&admin_db, frozen_tenant_id).await,
        (
            rig.source.data_cell_id,
            frozen_tenant.placement_revision.get()
        )
    );
    let placement_event_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tenant_cell_placement_events WHERE tenant_id=$1")
            .bind(frozen_tenant_id.get())
            .fetch_one(&admin_db)
            .await
            .unwrap();
    assert_eq!(placement_event_count, 1);
    assert!(tenant_membership_write(&rig.fixture.db, frozen_tenant_id).await);
    admin_db.close().await;
}

#[tokio::test]
async fn post_cutover_rollback_restores_source_with_a_new_revision_and_releases_fence() {
    let rig = move_fixture("rollback-recovery").await;
    let tenant = create_moved_tenant(&rig, "rollback-cutover").await;
    let tenant_id = TenantId::new(tenant.tenant_id).unwrap();
    let planned = plan_move(&rig, &tenant, "plan-rollback").await;
    let frozen = move_to_frozen(&rig, planned, "rollback").await;
    let cut_over = move_to_cut_over(&rig, frozen, "rollback").await;
    let cutover_placement_revision = cut_over.cutover_placement_revision.unwrap();
    assert_eq!(cut_over.status, TenantCellMoveStatus::CutOver);
    assert_eq!(
        cutover_placement_revision.get(),
        tenant.placement_revision.get() + 1
    );
    assert_eq!(cut_over.source_cell.reserved_rollback_move_count, 1);
    assert!(cut_over.write_frozen);

    let verified = move_action(
        &rig,
        cut_over.tenant_cell_move_id,
        "cutover-verifications",
        "verify-before-rollback",
        &VerifyTenantCellMoveCutoverRequest {
            expected_revision: cut_over.revision,
            verification: TenantCellMoveCutoverVerificationEvidence {
                tool_version: "cell-recovery-validator/1.0.0".into(),
                routing_reference: "route/recovery-rollback".into(),
                observed_data_cell_id: rig.target.data_cell_id,
                observed_placement_revision: cutover_placement_revision,
                routing_verified: true,
                target_read_verified: true,
                write_fence_verified: true,
                inventory_reconciled: true,
                idempotency_verified: true,
                outbox_verified: true,
            },
        },
    )
    .await;
    assert_eq!(verified.status, TenantCellMoveStatus::CutOver);
    assert!(verified.cutover_verification.is_some());

    let admin_db = admin_db_for(&rig.fixture.db).await;
    assert_eq!(
        placement_state(&admin_db, tenant_id).await,
        (rig.target.data_cell_id, cutover_placement_revision.get())
    );
    assert_eq!(fence_count(&admin_db, tenant_id).await, 1);
    assert!(!tenant_membership_write(&rig.fixture.db, tenant_id).await);

    let rollback_placement_revision = Revision::new(cutover_placement_revision.get() + 1).unwrap();
    assert_database_rejects_rollback_verification(
        &rig,
        &verified,
        rig.target.data_cell_id,
        rollback_placement_revision.get(),
    )
    .await;
    assert_database_rejects_rollback_verification(
        &rig,
        &verified,
        rig.source.data_cell_id,
        rollback_placement_revision.get() + 1,
    )
    .await;

    let rollback_path = format!(
        "/api/v1/platform/tenant-cell-moves/{}/rollbacks",
        verified.tenant_cell_move_id
    );
    let missing_proof = rig
        .app
        .clone()
        .oneshot(request(
            &rig.token,
            rig.home,
            Method::POST,
            &rollback_path,
            Some("rollback-missing-proof"),
            &serde_json::json!({
                "expected_revision": verified.revision,
                "reason": "missing rollback proof"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(missing_proof.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let verification = TenantCellMoveRollbackVerificationEvidence {
        tool_version: "cell-recovery-validator/1.0.0".into(),
        routing_reference: "route/recovery-source-rollback".into(),
        observed_data_cell_id: rig.source.data_cell_id,
        expected_rollback_placement_revision: rollback_placement_revision,
        routing_verified: true,
        source_read_verified: true,
        write_fence_verified: true,
        inventory_reconciled: true,
        idempotency_verified: true,
        outbox_verified: true,
    };
    let mut malformed_verification = verification.clone();
    malformed_verification.source_read_verified = false;
    let malformed_proof = rig
        .app
        .clone()
        .oneshot(request(
            &rig.token,
            rig.home,
            Method::POST,
            &rollback_path,
            Some("rollback-malformed-proof"),
            &RollbackTenantCellMoveRequest {
                expected_revision: verified.revision,
                verification: malformed_verification,
                reason: "malformed rollback proof".into(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(malformed_proof.status(), StatusCode::BAD_REQUEST);

    let mut wrong_source_verification = verification.clone();
    wrong_source_verification.observed_data_cell_id = rig.target.data_cell_id;
    let wrong_source = rig
        .app
        .clone()
        .oneshot(request(
            &rig.token,
            rig.home,
            Method::POST,
            &rollback_path,
            Some("rollback-wrong-source"),
            &RollbackTenantCellMoveRequest {
                expected_revision: verified.revision,
                verification: wrong_source_verification,
                reason: "wrong source rollback proof".into(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(wrong_source.status(), StatusCode::CONFLICT);

    let mut wrong_revision_verification = verification.clone();
    wrong_revision_verification.expected_rollback_placement_revision =
        Revision::new(rollback_placement_revision.get() + 1).unwrap();
    let wrong_revision = rig
        .app
        .clone()
        .oneshot(request(
            &rig.token,
            rig.home,
            Method::POST,
            &rollback_path,
            Some("rollback-wrong-revision"),
            &RollbackTenantCellMoveRequest {
                expected_revision: verified.revision,
                verification: wrong_revision_verification,
                reason: "wrong revision rollback proof".into(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(wrong_revision.status(), StatusCode::CONFLICT);
    assert_eq!(
        placement_state(&admin_db, tenant_id).await,
        (rig.target.data_cell_id, cutover_placement_revision.get())
    );
    assert_eq!(fence_count(&admin_db, tenant_id).await, 1);

    let rollback_request = RollbackTenantCellMoveRequest {
        expected_revision: verified.revision,
        verification: verification.clone(),
        reason: "target routing health regressed".into(),
    };
    let rolled_back = move_action(
        &rig,
        verified.tenant_cell_move_id,
        "rollbacks",
        "rollback-after-cutover",
        &rollback_request,
    )
    .await;
    let replayed = move_action(
        &rig,
        verified.tenant_cell_move_id,
        "rollbacks",
        "rollback-after-cutover",
        &rollback_request,
    )
    .await;
    assert_eq!(replayed, rolled_back);
    let rollback_placement_revision = rolled_back.rollback_placement_revision.unwrap();
    assert_eq!(rolled_back.status, TenantCellMoveStatus::RolledBack);
    assert_eq!(rolled_back.revision.get(), verified.revision.get() + 1);
    assert_eq!(
        rollback_placement_revision.get(),
        cutover_placement_revision.get() + 1
    );
    assert_eq!(
        rolled_back.cutover_placement_revision,
        Some(cutover_placement_revision)
    );
    let persisted_verification = rolled_back.rollback_verification.as_ref().unwrap();
    assert_eq!(persisted_verification.move_revision, rolled_back.revision);
    assert_eq!(persisted_verification.verification, verification);
    assert_eq!(rolled_back.source_cell.reserved_rollback_move_count, 0);
    assert_eq!(rolled_back.target_cell.reserved_inbound_move_count, 0);
    assert!(!rolled_back.write_frozen);
    assert_eq!(fence_count(&admin_db, tenant_id).await, 0);
    assert_eq!(
        placement_state(&admin_db, tenant_id).await,
        (rig.source.data_cell_id, rollback_placement_revision.get())
    );
    let rollback_evidence: serde_json::Value = sqlx::query_scalar(
        "SELECT evidence->'rollback_verification' FROM tenant_cell_move_events WHERE tenant_cell_move_id=$1 AND action='rolled_back'",
    )
    .bind(rolled_back.tenant_cell_move_id)
    .fetch_one(&admin_db)
    .await
    .unwrap();
    assert_eq!(
        rollback_evidence,
        serde_json::to_value(&verification).unwrap()
    );
    let rollback_verification_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM tenant_cell_move_rollback_verifications WHERE tenant_cell_move_id=$1",
    )
    .bind(rolled_back.tenant_cell_move_id)
    .fetch_one(&admin_db)
    .await
    .unwrap();
    assert_eq!(rollback_verification_count, 1);
    let mut immutable_proof_tx = admin_db.begin().await.unwrap();
    sqlx::query("SELECT set_config('wareboxes.platform_actor_user_id',$1,TRUE)")
        .bind(rig.platform_admin_id.to_string())
        .execute(&mut *immutable_proof_tx)
        .await
        .unwrap();
    let immutable_error = sqlx::query(
        "UPDATE tenant_cell_move_rollback_verifications SET routing_reference=routing_reference WHERE tenant_cell_move_id=$1",
    )
    .bind(rolled_back.tenant_cell_move_id)
    .execute(&mut *immutable_proof_tx)
    .await
    .unwrap_err();
    assert_eq!(database_code(&immutable_error).as_deref(), Some("55000"));
    immutable_proof_tx.rollback().await.unwrap();
    let placement_events: Vec<(String, i64, Option<i64>, i64)> = sqlx::query_as(
        r#"SELECT action,placement_revision,previous_data_cell_id,resulting_data_cell_id
        FROM tenant_cell_placement_events WHERE tenant_id=$1 ORDER BY placement_revision"#,
    )
    .bind(tenant_id.get())
    .fetch_all(&admin_db)
    .await
    .unwrap();
    assert_eq!(
        placement_events,
        vec![
            ("placed".into(), 1, None, rig.source.data_cell_id),
            (
                "moved".into(),
                cutover_placement_revision.get(),
                Some(rig.source.data_cell_id),
                rig.target.data_cell_id,
            ),
            (
                "rolled_back".into(),
                rollback_placement_revision.get(),
                Some(rig.target.data_cell_id),
                rig.source.data_cell_id,
            ),
        ]
    );
    assert!(tenant_membership_write(&rig.fixture.db, tenant_id).await);
    admin_db.close().await;
}

#[tokio::test]
async fn frozen_tenant_authentication_reconciliation_and_suspension_control_remain_available() {
    let rig = move_fixture("frozen-control").await;
    let tenant = create_moved_tenant(&rig, "frozen-control-tenant").await;
    let tenant_id = TenantId::new(tenant.tenant_id).unwrap();
    grant_permission(&rig.fixture, tenant_id, rig.platform_admin_id, "orders").await;
    let service_account: ServiceAccountResponse = response(
        rig.app
            .clone()
            .oneshot(request(
                &rig.token,
                tenant_id,
                Method::POST,
                "/api/v1/service-accounts",
                Some("create-frozen-control-service"),
                &CreateServiceAccountRequest {
                    name: "Frozen tenant control probe".into(),
                    description: None,
                    access: ServiceAccountAccessRequest {
                        all_facilities: true,
                        facility_ids: vec![],
                        all_inventory_owners: true,
                        inventory_owner_ids: vec![],
                        permission_names: vec!["orders".into()],
                    },
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let service_token = format!("wbs_sa_{}", "S".repeat(48));
    let _: IssuedServiceAccountCredentialResponse = response(
        rig.app
            .clone()
            .oneshot(request(
                &rig.token,
                tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/service-accounts/{}/credentials",
                    service_account.service_account_id
                ),
                Some("issue-frozen-control-service"),
                &IssueServiceAccountCredentialRequest {
                    expected_revision: service_account.revision,
                    label: "frozen control probe".into(),
                    expires_at: None,
                    bearer_token: service_token.clone(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;

    let planned = plan_move(&rig, &tenant, "plan-frozen-control").await;
    let frozen = move_to_frozen(&rig, planned, "frozen-control").await;
    assert!(frozen.write_frozen);
    let service_context = rig
        .app
        .clone()
        .oneshot(request(
            &service_token,
            tenant_id,
            Method::GET,
            "/api/auth/context",
            None,
            &(),
        ))
        .await
        .unwrap();
    assert_eq!(service_context.status(), StatusCode::OK);

    let suspended: TenantLifecycleResponse = response(
        rig.app
            .clone()
            .oneshot(request(
                &rig.token,
                rig.home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenants/{}/status-changes",
                    tenant.tenant_id
                ),
                Some("suspend-frozen-control-tenant"),
                &ChangeTenantStatusRequest {
                    expected_revision: tenant.revision,
                    status: TenantStatus::Suspended,
                    reason: "exercise emergency control during a governed move".into(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(suspended.status, TenantStatus::Suspended);
    let admin_db = admin_db_for(&rig.fixture.db).await;
    let revoked_event_count: i64 = sqlx::query_scalar(
        r#"SELECT count(*) FROM service_account_events
        WHERE tenant_id=$1 AND action='credential_revoked'"#,
    )
    .bind(tenant_id.get())
    .fetch_one(&admin_db)
    .await
    .unwrap();
    assert_eq!(revoked_event_count, 1);
    assert_eq!(fence_count(&admin_db, tenant_id).await, 1);
    admin_db.close().await;
}

#[tokio::test]
async fn tenant_cell_move_routes_deny_ordinary_users_and_service_accounts() {
    let fixture = Fixture::new().await;
    let platform_admin = fixture
        .user("recovery-auth-platform-admin@test.local")
        .await;
    let home = tenant_for_user(&fixture.db, platform_admin.id).await;
    grant_platform_administrator(&fixture.db, platform_admin.id).await;
    grant_permission(&fixture, home, platform_admin.id, "admin").await;
    grant_permission(&fixture, home, platform_admin.id, "orders").await;
    let platform_token = auth::create_session(&fixture.db, platform_admin.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let ordinary = fixture.user("recovery-auth-ordinary@test.local").await;
    let ordinary_home = tenant_for_user(&fixture.db, ordinary.id).await;
    let ordinary_token = auth::create_session(&fixture.db, ordinary.id)
        .await
        .unwrap();
    for denied in [
        app.clone()
            .oneshot(request(
                &ordinary_token,
                ordinary_home,
                Method::GET,
                "/api/v1/platform/tenant-cell-moves?limit=20",
                None,
                &(),
            ))
            .await
            .unwrap(),
        app.clone()
            .oneshot(request(
                &ordinary_token,
                ordinary_home,
                Method::POST,
                "/api/v1/platform/tenant-cell-moves/999/cancellations",
                Some("ordinary-cannot-cancel"),
                &CancelTenantCellMoveRequest {
                    expected_revision: Revision::new(1).unwrap(),
                    reason: "unauthorized".into(),
                },
            ))
            .await
            .unwrap(),
    ] {
        let _: serde_json::Value = response(denied, StatusCode::FORBIDDEN).await;
    }

    let service_account: ServiceAccountResponse = response(
        app.clone()
            .oneshot(request(
                &platform_token,
                home,
                Method::POST,
                "/api/v1/service-accounts",
                Some("create-cell-move-recovery-service"),
                &CreateServiceAccountRequest {
                    name: "Cell move recovery probe".into(),
                    description: None,
                    access: ServiceAccountAccessRequest {
                        all_facilities: true,
                        facility_ids: vec![],
                        all_inventory_owners: true,
                        inventory_owner_ids: vec![],
                        permission_names: vec!["orders".into()],
                    },
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let service_token = format!("wbs_sa_{}", "R".repeat(48));
    let _: IssuedServiceAccountCredentialResponse = response(
        app.clone()
            .oneshot(request(
                &platform_token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/service-accounts/{}/credentials",
                    service_account.service_account_id
                ),
                Some("issue-cell-move-recovery-service"),
                &IssueServiceAccountCredentialRequest {
                    expected_revision: service_account.revision,
                    label: "recovery probe".into(),
                    expires_at: None,
                    bearer_token: service_token.clone(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;

    for denied in [
        app.clone()
            .oneshot(request(
                &service_token,
                home,
                Method::GET,
                "/api/v1/platform/tenant-cell-moves?limit=20",
                None,
                &(),
            ))
            .await
            .unwrap(),
        app.clone()
            .oneshot(request(
                &service_token,
                home,
                Method::POST,
                "/api/v1/platform/tenant-cell-moves/999/cancellations",
                Some("service-cannot-cancel"),
                &CancelTenantCellMoveRequest {
                    expected_revision: Revision::new(1).unwrap(),
                    reason: "unauthorized".into(),
                },
            ))
            .await
            .unwrap(),
    ] {
        let _: serde_json::Value = response(denied, StatusCode::FORBIDDEN).await;
    }

    let platform_list = app
        .oneshot(request(
            &platform_token,
            home,
            Method::GET,
            "/api/v1/platform/tenant-cell-moves?limit=20",
            None,
            &(),
        ))
        .await
        .unwrap();
    assert_eq!(platform_list.status(), StatusCode::OK);
}
