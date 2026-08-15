mod common;
#[path = "labor_rls/support.rs"]
mod support;

use support::*;

use std::time::Duration;

use common::*;

const TABLES: [&str; 9] = [
    "labor_skills",
    "employee_certifications",
    "equipment_classes",
    "equipment_assets",
    "labor_standards",
    "attendance_intervals",
    "labor_activities",
    "attendance_adjustments",
    "labor_activity_adjustments",
];

const SEQUENCES: [&str; 9] = [
    "labor_skills_id_seq",
    "employee_certifications_id_seq",
    "equipment_classes_id_seq",
    "equipment_assets_id_seq",
    "labor_standards_id_seq",
    "attendance_intervals_id_seq",
    "labor_activities_id_seq",
    "attendance_adjustments_id_seq",
    "labor_activity_adjustments_id_seq",
];

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct TablePrivileges {
    table_name: String,
    can_select: bool,
    can_insert: bool,
    can_update: bool,
    can_delete: bool,
    can_truncate: bool,
    can_reference: bool,
    can_trigger: bool,
}

#[derive(Clone, Copy)]
struct LaborRefs {
    tenant_id: TenantId,
    actor_id: i64,
    facility_id: i64,
    employee_id: i64,
    skill_id: i64,
    certification_id: i64,
    equipment_class_id: i64,
    equipment_asset_id: i64,
    standard_id: i64,
    historical_attendance_id: i64,
    historical_activity_id: i64,
    attendance_id: i64,
    activity_id: i64,
    attendance_adjustment_id: i64,
    activity_adjustment_id: i64,
}

impl LaborRefs {
    fn ids(self) -> [i64; 9] {
        [
            self.skill_id,
            self.certification_id,
            self.equipment_class_id,
            self.equipment_asset_id,
            self.standard_id,
            self.attendance_id,
            self.activity_id,
            self.attendance_adjustment_id,
            self.activity_adjustment_id,
        ]
    }

    fn visible_ids(self) -> [Vec<i64>; 9] {
        [
            vec![self.skill_id],
            vec![self.certification_id],
            vec![self.equipment_class_id],
            vec![self.equipment_asset_id],
            vec![self.standard_id],
            vec![self.historical_attendance_id, self.attendance_id],
            vec![self.historical_activity_id, self.activity_id],
            vec![self.attendance_adjustment_id],
            vec![self.activity_adjustment_id],
        ]
    }
}

#[derive(Clone, Copy)]
struct DirectLaborRefs {
    core: LaborRefs,
    task_id: i64,
    activity_id: i64,
    started_at: wareboxes_domain::Timestamp,
}

#[derive(Clone, Copy)]
struct StartableLaborRefs {
    core: LaborRefs,
    task_id: i64,
}

#[tokio::test]
async fn labor_core_is_force_rls_fail_closed_and_has_exact_runtime_privileges() {
    let fixture = Fixture::new().await;
    let refs_a = seed_labor(&fixture, "labor-rls-a@test.local", "A").await;
    let refs_b = seed_labor(&fixture, "labor-rls-b@test.local", "B").await;

    assert_force_rls_and_exact_runtime_privileges(&fixture.db).await;

    let unbound_counts = visible_counts(&fixture.db).await;
    assert_eq!(
        unbound_counts, [0; 9],
        "unbound runtime sessions must fail closed"
    );
    assert_eq!(self_update_counts(&fixture.db, refs_a).await, [0; 7]);

    let mut tenant_a = tenant_tx(&fixture.db, refs_a.tenant_id).await;
    assert_eq!(visible_ids(&mut tenant_a).await, refs_a.visible_ids());
    tenant_a.rollback().await.unwrap();

    let mut tenant_b = tenant_tx(&fixture.db, refs_b.tenant_id).await;
    assert_eq!(visible_ids(&mut tenant_b).await, refs_b.visible_ids());
    assert_eq!(self_update_counts_on(&mut tenant_b, refs_a).await, [0; 7]);
    tenant_b.rollback().await.unwrap();

    assert_delete_is_denied_for_every_table(&fixture.db, refs_a).await;
    assert_forged_inserts_fail(&fixture.db, None, refs_a, "UNBOUND").await;
    assert_forged_inserts_fail(&fixture.db, Some(refs_b.tenant_id), refs_a, "CROSS-TENANT").await;
}

#[tokio::test]
async fn labor_sql_tampering_status_jumps_and_overlapping_timelines_are_rejected() {
    let fixture = Fixture::new().await;
    let refs = seed_labor(&fixture, "labor-invariants@test.local", "INVARIANTS").await;
    let now = db::now_iso();

    let mut tx = tenant_tx(&fixture.db, refs.tenant_id).await;
    let result = sqlx::query(
        r#"UPDATE attendance_intervals SET status='closed',revision=revision+1,
           clocked_out_at=$2,paid_seconds=NULL,clocked_out_by_user_id=$3 WHERE id=$1"#,
    )
    .bind(refs.attendance_id)
    .bind(now)
    .bind(refs.actor_id)
    .execute(&mut *tx)
    .await;
    expect_statement_or_commit_failure(tx, result, "attendance").await;

    let mut tx = tenant_tx(&fixture.db, refs.tenant_id).await;
    let result = sqlx::query(
        r#"UPDATE labor_activities SET status='completed',revision=revision+1,
           completed_at=$2,actual_seconds=NULL,exception_seconds=0,
           completed_by_user_id=$3 WHERE id=$1"#,
    )
    .bind(refs.activity_id)
    .bind(now)
    .bind(refs.actor_id)
    .execute(&mut *tx)
    .await;
    expect_statement_or_commit_failure(tx, result, "labor").await;

    for (expected_message, id, statement) in [
        (
            "labor skill",
            refs.skill_id,
            "UPDATE labor_skills SET revision=revision+2,name=name,configured_by_user_id=$2,configured_at=$3 WHERE id=$1",
        ),
        (
            "certification",
            refs.certification_id,
            "UPDATE employee_certifications SET revision=revision+2,revoked_by_user_id=$2,revoked_at=$3,revocation_note='invalid jump' WHERE id=$1",
        ),
        (
            "equipment class",
            refs.equipment_class_id,
            "UPDATE equipment_classes SET revision=revision+2,name=name,configured_by_user_id=$2,configured_at=$3 WHERE id=$1",
        ),
        (
            "equipment",
            refs.equipment_asset_id,
            "UPDATE equipment_assets SET status='out_of_service',revision=revision+2,status_note='invalid jump',status_changed_by_user_id=$2,status_changed_at=$3 WHERE id=$1",
        ),
    ] {
        let mut tx = tenant_tx(&fixture.db, refs.tenant_id).await;
        let result = sqlx::query(statement)
            .bind(id)
            .bind(refs.actor_id)
            .bind(now)
            .execute(&mut *tx)
            .await;
        expect_statement_or_commit_failure(tx, result, expected_message).await;
    }

    let mut tx = tenant_tx(&fixture.db, refs.tenant_id).await;
    let result = sqlx::query(
        r#"INSERT INTO attendance_intervals
          (tenant_id,employee_id,facility_id,status,clocked_in_at,clocked_in_by_user_id)
          VALUES($1,$2,$3,'open',$4,$5)"#,
    )
    .bind(refs.tenant_id.get())
    .bind(refs.employee_id)
    .bind(refs.facility_id)
    .bind(now - Duration::from_secs(30))
    .bind(refs.actor_id)
    .execute(&mut *tx)
    .await;
    expect_statement_or_commit_failure(tx, result, "attendance").await;

    let mut tx = tenant_tx(&fixture.db, refs.tenant_id).await;
    let result = sqlx::query(
        r#"INSERT INTO labor_activities
          (tenant_id,attendance_interval_id,employee_id,facility_id,activity_kind,status,
           started_at,started_by_user_id)
          VALUES($1,$2,$3,$4,'meeting','active',$5,$6)"#,
    )
    .bind(refs.tenant_id.get())
    .bind(refs.attendance_id)
    .bind(refs.employee_id)
    .bind(refs.facility_id)
    .bind(now - Duration::from_secs(30))
    .bind(refs.actor_id)
    .execute(&mut *tx)
    .await;
    expect_statement_or_commit_failure(tx, result, "labor").await;

    let mut tx = tenant_tx(&fixture.db, refs.tenant_id).await;
    let result = sqlx::query(
        r#"INSERT INTO labor_standards
          (tenant_id,facility_id,code,name,activity_kind,quantity_basis,setup_seconds,
           seconds_per_unit,effective_from,configured_by_user_id,configured_at)
          SELECT tenant_id,facility_id,code,'Overlapping standard',activity_kind,quantity_basis,
            setup_seconds,seconds_per_unit,effective_from+INTERVAL '1 second',
            configured_by_user_id,$2 FROM labor_standards WHERE id=$1"#,
    )
    .bind(refs.standard_id)
    .bind(now)
    .execute(&mut *tx)
    .await;
    expect_statement_or_commit_failure(tx, result, "overlap").await;

    let mut tx = tenant_tx(&fixture.db, refs.tenant_id).await;
    let result = sqlx::query(
        r#"INSERT INTO employee_certifications
          (tenant_id,employee_id,skill_id,facility_id,certification_number,issued_at,
           expires_at,note,certified_by_user_id,certified_at)
          SELECT tenant_id,employee_id,skill_id,facility_id,'OVERLAP',$2,$3,
            'overlapping certification',certified_by_user_id,$4
          FROM employee_certifications WHERE id=$1"#,
    )
    .bind(refs.certification_id)
    .bind(now - Duration::from_secs(1_800))
    .bind(now + Duration::from_secs(1_800))
    .bind(now)
    .execute(&mut *tx)
    .await;
    expect_statement_or_commit_failure(tx, result, "overlap").await;

    let mut tx = tenant_tx(&fixture.db, refs.tenant_id).await;
    let result = sqlx::query(
        r#"UPDATE employee_certifications SET revision=revision+1,
           revoked_by_user_id=$2,revoked_at=certified_at-INTERVAL '1 second',
           revocation_note='historical revocation' WHERE id=$1"#,
    )
    .bind(refs.certification_id)
    .bind(refs.actor_id)
    .execute(&mut *tx)
    .await;
    expect_statement_or_commit_failure(tx, result, "revocation").await;

    let admin = admin_db_for(&fixture.db).await;
    for (field, statement) in [
        (
            "standard snapshot",
            "UPDATE labor_activities SET standard_setup_seconds=123 WHERE id=$1 AND $2::bigint IS NOT NULL",
        ),
        (
            "equipment snapshot",
            "UPDATE labor_activities SET equipment_asset_id=$2 WHERE id=$1",
        ),
        (
            "certification snapshot",
            "UPDATE labor_activities SET required_skill_certification_id=$2 WHERE id=$1",
        ),
        (
            "canonical reference quantity",
            "UPDATE labor_activities SET reference_quantity=999 WHERE id=$1 AND $2::bigint IS NOT NULL",
        ),
    ] {
        let mut tx = admin.begin().await.unwrap();
        let result = sqlx::query(statement)
            .bind(refs.activity_id)
            .bind(refs.certification_id)
            .execute(&mut *tx)
            .await;
        expect_statement_or_commit_failure(tx, result, "terminal transition").await;
        let _ = field;
    }

    let outsider_id = assert_forged_direct_snapshots_are_rejected(&fixture, refs).await;

    let mut tx = tenant_tx(&fixture.db, refs.tenant_id).await;
    let result = sqlx::query(
        r#"UPDATE labor_activities SET status='completed',revision=revision+1,
           completed_at=started_at+INTERVAL '10 seconds',actual_seconds=10,
           exception_seconds=1,exception_reason='system',exception_note='forged approval',
           exception_approved_by_user_id=$2,completed_by_user_id=$3 WHERE id=$1"#,
    )
    .bind(refs.activity_id)
    .bind(outsider_id)
    .bind(refs.actor_id)
    .execute(&mut *tx)
    .await;
    expect_statement_or_commit_failure(tx, result, "exception").await;
    admin.close().await;
}

#[tokio::test]
async fn active_labor_blocks_dependency_retirement_and_source_task_detachment() {
    let fixture = Fixture::new().await;
    let direct = prepare_direct_labor(&fixture, "DEPENDENCIES").await;
    let refs = direct.core;
    let admin = admin_db_for(&fixture.db).await;
    let now = db::now_iso();

    let mut identity_tx = admin.begin().await.unwrap();
    let result = sqlx::query(
        r#"UPDATE employees SET user_id=NULL,identity_revision=identity_revision+1,
           identity_changed_by_user_id=$2,identity_changed_at=$3
           WHERE tenant_id=$4 AND id=$1"#,
    )
    .bind(refs.employee_id)
    .bind(refs.actor_id)
    .bind(now)
    .bind(refs.tenant_id.get())
    .execute(&mut *identity_tx)
    .await;
    expect_statement_or_commit_failure(identity_tx, result, "identity cannot change").await;

    for (dependency, expected_message, statement) in [
        (
            "employee",
            "identity must be unlinked",
            "UPDATE employees SET deleted=$2 WHERE tenant_id=$3 AND id=$1",
        ),
        (
            "employee facility",
            "labor remains open",
            "UPDATE employee_facilities SET deleted=$2 WHERE tenant_id=$3 AND employee_id=$1",
        ),
        (
            "facility",
            "labor remains open",
            "UPDATE facilities SET deleted=$2 WHERE tenant_id=$3 AND id=$1",
        ),
    ] {
        let target_id = if dependency == "facility" {
            refs.facility_id
        } else {
            refs.employee_id
        };
        let mut tx = admin.begin().await.unwrap();
        let result = sqlx::query(statement)
            .bind(target_id)
            .bind(now)
            .bind(refs.tenant_id.get())
            .execute(&mut *tx)
            .await;
        expect_statement_or_commit_failure(tx, result, expected_message).await;
    }

    for (transition, statement) in [
        (
            "completion",
            r#"UPDATE work_tasks SET status='completed',lease_expires_at=NULL,
               completed_by=$2,completed_at=$3 WHERE tenant_id=$4 AND id=$1"#,
        ),
        (
            "reassignment",
            r#"UPDATE work_tasks SET assigned_user_id=NULL,modified=$3
               WHERE tenant_id=$4 AND id=$1 AND $2::bigint IS NOT NULL"#,
        ),
        (
            "release",
            r#"UPDATE work_tasks SET status='open',assigned_user_id=NULL,started_at=NULL,
               lease_expires_at=NULL,last_released_at=$3,release_count=release_count+1
               WHERE tenant_id=$4 AND id=$1 AND $2::bigint IS NOT NULL"#,
        ),
        (
            "lease expiry",
            r#"UPDATE work_tasks SET lease_expires_at=$3-INTERVAL '1 second'
               WHERE tenant_id=$4 AND id=$1 AND $2::bigint IS NOT NULL"#,
        ),
    ] {
        let mut tx = admin.begin().await.unwrap();
        let result = sqlx::query(statement)
            .bind(direct.task_id)
            .bind(refs.actor_id)
            .bind(now)
            .bind(refs.tenant_id.get())
            .execute(&mut *tx)
            .await;
        expect_statement_or_commit_failure(tx, result, "cannot detach active labor").await;
        let _ = transition;
    }

    let mut tx = admin.begin().await.unwrap();
    let result = sqlx::query(
        r#"UPDATE labor_standards SET effective_until=$2,retired_by_user_id=$3,retired_at=$4
           WHERE tenant_id=$5 AND id=$1"#,
    )
    .bind(refs.standard_id)
    .bind(direct.started_at)
    .bind(refs.actor_id)
    .bind(now)
    .bind(refs.tenant_id.get())
    .execute(&mut *tx)
    .await;
    expect_statement_or_commit_failure(tx, result, "recorded labor").await;

    for (snapshot, statement) in [
        (
            "standard snapshot",
            "UPDATE labor_activities SET standard_seconds_per_unit=999 WHERE id=$1",
        ),
        (
            "equipment snapshot",
            "UPDATE labor_activities SET required_equipment_class_id=NULL WHERE id=$1",
        ),
        (
            "certification snapshot",
            "UPDATE labor_activities SET equipment_skill_certification_id=NULL WHERE id=$1",
        ),
        (
            "canonical reference quantity",
            "UPDATE labor_activities SET reference_quantity=2 WHERE id=$1",
        ),
    ] {
        let mut tx = admin.begin().await.unwrap();
        let result = sqlx::query(statement)
            .bind(direct.activity_id)
            .execute(&mut *tx)
            .await;
        expect_statement_or_commit_failure(tx, result, "terminal transition").await;
        let _ = snapshot;
    }
    admin.close().await;
}

#[tokio::test]
async fn certification_revocation_and_labor_start_are_serialized_in_both_orders() {
    let fixture = Fixture::new().await;

    let revoke_wins = prepare_startable_labor(&fixture, "REVOKE-WINS").await;
    let mut revoke_tx = tenant_tx(&fixture.db, revoke_wins.core.tenant_id).await;
    let revoked_at = db::now_iso();
    sqlx::query(
        r#"UPDATE employee_certifications SET revision=revision+1,
           revoked_by_user_id=$2,revoked_at=$3,revocation_note='concurrent revocation wins'
           WHERE id=$1"#,
    )
    .bind(revoke_wins.core.certification_id)
    .bind(revoke_wins.core.actor_id)
    .bind(revoked_at)
    .execute(&mut *revoke_tx)
    .await
    .unwrap();

    let start_db = fixture.db.clone();
    let blocked_start = tokio::spawn(async move {
        let mut tx = tenant_tx(&start_db, revoke_wins.core.tenant_id).await;
        stage_valid_direct_labor(&mut tx, revoke_wins, "REVOKE-WINS-RACE", true).await;
        tx.commit().await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !blocked_start.is_finished(),
        "labor start should wait for the in-flight certification revocation"
    );
    revoke_tx.commit().await.unwrap();
    let error = blocked_start
        .await
        .unwrap()
        .expect_err("labor start unexpectedly committed after certification revocation");
    assert_error_contains(&error, "certification");

    let start_wins = prepare_startable_labor(&fixture, "START-WINS").await;
    let mut start_tx = tenant_tx(&fixture.db, start_wins.core.tenant_id).await;
    stage_valid_direct_labor(&mut start_tx, start_wins, "START-WINS-RACE", true).await;

    let revoke_db = fixture.db.clone();
    let blocked_revoke = tokio::spawn(async move {
        let mut tx = tenant_tx(&revoke_db, start_wins.core.tenant_id).await;
        let result = sqlx::query(
            r#"UPDATE employee_certifications SET revision=revision+1,
               revoked_by_user_id=$2,revoked_at=$3,
               revocation_note='concurrent labor start wins' WHERE id=$1"#,
        )
        .bind(start_wins.core.certification_id)
        .bind(start_wins.core.actor_id)
        .bind(db::now_iso())
        .execute(&mut *tx)
        .await;
        match result {
            Ok(_) => tx.commit().await,
            Err(error) => {
                tx.rollback().await.unwrap();
                Err(error)
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !blocked_revoke.is_finished(),
        "certification revocation should wait for the in-flight labor start"
    );
    start_tx.commit().await.unwrap();
    let error = blocked_revoke
        .await
        .unwrap()
        .expect_err("certification revocation unexpectedly detached active labor");
    assert_error_contains(&error, "active labor");
}

#[tokio::test]
async fn concurrent_certification_timeline_and_standard_writes_preserve_invariants() {
    let fixture = Fixture::new().await;
    let refs = seed_labor(&fixture, "labor-concurrency@test.local", "CONCURRENCY").await;
    let now = db::now_iso();

    let issued_at = now + Duration::from_secs(172_800);
    let expires_at = now + Duration::from_secs(259_200);
    let mut first_cert = tenant_tx(&fixture.db, refs.tenant_id).await;
    sqlx::query(
        r#"INSERT INTO employee_certifications
          (tenant_id,employee_id,skill_id,facility_id,certification_number,issued_at,
           expires_at,note,certified_by_user_id,certified_at)
          VALUES($1,$2,$3,$4,'CONCURRENT-CERT-A',$5,$6,'future renewal',$7,$5)"#,
    )
    .bind(refs.tenant_id.get())
    .bind(refs.employee_id)
    .bind(refs.skill_id)
    .bind(refs.facility_id)
    .bind(issued_at)
    .bind(expires_at)
    .bind(refs.actor_id)
    .execute(&mut *first_cert)
    .await
    .unwrap();
    let cert_db = fixture.db.clone();
    let second_cert = tokio::spawn(async move {
        let mut tx = tenant_tx(&cert_db, refs.tenant_id).await;
        let result = sqlx::query(
            r#"INSERT INTO employee_certifications
              (tenant_id,employee_id,skill_id,facility_id,certification_number,issued_at,
               expires_at,note,certified_by_user_id,certified_at)
              VALUES($1,$2,$3,$4,'CONCURRENT-CERT-B',$5,$6,'racing renewal',$7,$5)"#,
        )
        .bind(refs.tenant_id.get())
        .bind(refs.employee_id)
        .bind(refs.skill_id)
        .bind(refs.facility_id)
        .bind(issued_at)
        .bind(expires_at)
        .bind(refs.actor_id)
        .execute(&mut *tx)
        .await;
        finish_transaction(tx, result).await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!second_cert.is_finished());
    first_cert.commit().await.unwrap();
    let error = second_cert.await.unwrap().unwrap_err();
    assert_error_contains(&error, "overlap");

    let access = repo::tenants::access_for_user(&fixture.db, refs.actor_id, refs.tenant_id)
        .await
        .unwrap()
        .unwrap();
    let employee_id = repo::employees::add_employee(
        &fixture.db,
        refs.tenant_id,
        &access.site_scope,
        &repo::employees::NewEmployee {
            first_name: "Concurrent",
            last_name: "Attendance",
            title: "Warehouse associate",
            employee_type: "hourly",
            email: None,
            phone: None,
            hired: now - Duration::from_secs(86_400),
            facility_ids: &[refs.facility_id],
        },
    )
    .await
    .unwrap();
    let mut first_attendance = tenant_tx(&fixture.db, refs.tenant_id).await;
    sqlx::query(
        r#"INSERT INTO attendance_intervals
          (tenant_id,employee_id,facility_id,status,clocked_in_at,clocked_in_by_user_id)
          VALUES($1,$2,$3,'open',$4,$5)"#,
    )
    .bind(refs.tenant_id.get())
    .bind(employee_id)
    .bind(refs.facility_id)
    .bind(now)
    .bind(refs.actor_id)
    .execute(&mut *first_attendance)
    .await
    .unwrap();
    let attendance_db = fixture.db.clone();
    let second_attendance = tokio::spawn(async move {
        let mut tx = tenant_tx(&attendance_db, refs.tenant_id).await;
        let result = sqlx::query(
            r#"INSERT INTO attendance_intervals
              (tenant_id,employee_id,facility_id,status,clocked_in_at,clocked_in_by_user_id)
              VALUES($1,$2,$3,'open',$4,$5)"#,
        )
        .bind(refs.tenant_id.get())
        .bind(employee_id)
        .bind(refs.facility_id)
        .bind(now + Duration::from_secs(1))
        .bind(refs.actor_id)
        .execute(&mut *tx)
        .await;
        finish_transaction(tx, result).await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!second_attendance.is_finished());
    first_attendance.commit().await.unwrap();
    let error = second_attendance.await.unwrap().unwrap_err();
    assert_sqlstate(&error, "23505");

    let mut first_standard = tenant_tx(&fixture.db, refs.tenant_id).await;
    sqlx::query(
        r#"INSERT INTO labor_standards
          (tenant_id,facility_id,code,name,activity_kind,quantity_basis,setup_seconds,
           seconds_per_unit,effective_from,configured_by_user_id,configured_at)
          VALUES($1,$2,'CONCURRENT-STANDARD','Concurrent A','cycle_count','task',1,1,
            $3,$4,$5)"#,
    )
    .bind(refs.tenant_id.get())
    .bind(refs.facility_id)
    .bind(now)
    .bind(refs.actor_id)
    .bind(now)
    .execute(&mut *first_standard)
    .await
    .unwrap();
    let standard_db = fixture.db.clone();
    let second_standard = tokio::spawn(async move {
        let mut tx = tenant_tx(&standard_db, refs.tenant_id).await;
        let result = sqlx::query(
            r#"INSERT INTO labor_standards
              (tenant_id,facility_id,code,name,activity_kind,quantity_basis,setup_seconds,
               seconds_per_unit,effective_from,configured_by_user_id,configured_at)
              VALUES($1,$2,'CONCURRENT-STANDARD','Concurrent B','cycle_count','task',2,2,
                $3,$4,$5)"#,
        )
        .bind(refs.tenant_id.get())
        .bind(refs.facility_id)
        .bind(now + Duration::from_secs(1))
        .bind(refs.actor_id)
        .bind(now)
        .execute(&mut *tx)
        .await;
        finish_transaction(tx, result).await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!second_standard.is_finished());
    first_standard.commit().await.unwrap();
    let error = second_standard.await.unwrap().unwrap_err();
    assert_error_contains(&error, "overlap");
}
