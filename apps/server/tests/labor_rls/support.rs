use std::collections::BTreeMap;
use std::time::Duration;

use super::common::*;
use super::{DirectLaborRefs, LaborRefs, StartableLaborRefs, TablePrivileges, SEQUENCES, TABLES};
use sqlx::Row;
use wareboxes_application::{workforce_identity::LinkEmployeeIdentityCommand, CommandContext};
use wareboxes_domain::{EmployeeId, EmployeeIdentityReason, UserId};

pub(super) async fn seed_labor(fixture: &Fixture, email: &str, key: &str) -> LaborRefs {
    seed_labor_with_identity(fixture, email, key, false).await
}

pub(super) async fn seed_labor_with_identity(
    fixture: &Fixture,
    email: &str,
    key: &str,
    link_identity: bool,
) -> LaborRefs {
    let user = fixture.user(email).await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant_admin_permission(fixture, tenant_id, user.id, key).await;
    let access = repo::tenants::access_for_user(&fixture.db, user.id, tenant_id)
        .await
        .unwrap()
        .unwrap();
    let facility_id = fixture
        .facility(tenant_id, &format!("Labor RLS facility {key}"))
        .await;
    let now = db::now_iso();
    let employee_id = repo::employees::add_employee(
        &fixture.db,
        tenant_id,
        &access.site_scope,
        &repo::employees::NewEmployee {
            first_name: "Labor",
            last_name: key,
            title: "Warehouse associate",
            employee_type: "hourly",
            email: None,
            phone: None,
            hired: now - Duration::from_secs(86_400),
            facility_ids: &[facility_id],
        },
    )
    .await
    .unwrap();

    if link_identity {
        link_employee_identity(fixture, tenant_id, user.id, employee_id, user.id, key).await;
    }

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let skill_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO labor_skills
          (tenant_id,code,name,certification_required,active,configured_by_user_id,configured_at)
          VALUES($1,$2,$3,true,true,$4,$5) RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(format!("FORKLIFT-{key}"))
    .bind(format!("Forklift skill {key}"))
    .bind(user.id)
    .bind(now)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let certification_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO employee_certifications
          (tenant_id,employee_id,skill_id,facility_id,certification_number,issued_at,
           expires_at,note,certified_by_user_id,certified_at)
          VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(employee_id)
    .bind(skill_id)
    .bind(facility_id)
    .bind(format!("CERT-{key}"))
    .bind(now - Duration::from_secs(3_600))
    .bind(now + Duration::from_secs(86_400))
    .bind("initial certification")
    .bind(user.id)
    .bind(now)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let equipment_class_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO equipment_classes
          (tenant_id,code,name,required_skill_id,active,configured_by_user_id,configured_at)
          VALUES($1,$2,$3,$4,true,$5,$6) RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(format!("REACH-{key}"))
    .bind(format!("Reach truck class {key}"))
    .bind(skill_id)
    .bind(user.id)
    .bind(now)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let equipment_asset_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO equipment_assets
          (tenant_id,facility_id,equipment_class_id,equipment_number,name,status,
           configured_by_user_id,configured_at)
          VALUES($1,$2,$3,$4,$5,'available',$6,$7) RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(facility_id)
    .bind(equipment_class_id)
    .bind(format!("RT-{key}"))
    .bind(format!("Reach truck {key}"))
    .bind(user.id)
    .bind(now)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let standard_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO labor_standards
          (tenant_id,facility_id,inventory_owner_id,code,name,activity_kind,quantity_basis,
           setup_seconds,seconds_per_unit,required_skill_id,required_equipment_class_id,
           effective_from,configured_by_user_id,configured_at)
          VALUES($1,$2,NULL,$3,$4,'cycle_count','task',10,5,$5,$6,$7,$8,$9)
          RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(facility_id)
    .bind(format!("COUNT-{key}"))
    .bind(format!("Cycle count standard {key}"))
    .bind(skill_id)
    .bind(equipment_class_id)
    .bind(now - Duration::from_secs(3_600))
    .bind(user.id)
    .bind(now)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let attendance_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO attendance_intervals
          (tenant_id,employee_id,facility_id,status,clocked_in_at,clocked_in_by_user_id)
          VALUES($1,$2,$3,'open',$4,$5) RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(employee_id)
    .bind(facility_id)
    .bind(now - Duration::from_secs(120))
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let activity_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO labor_activities
          (tenant_id,attendance_interval_id,employee_id,facility_id,activity_kind,status,
           started_at,started_by_user_id,note)
          VALUES($1,$2,$3,$4,'break','active',$5,$6,$7) RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(attendance_id)
    .bind(employee_id)
    .bind(facility_id)
    .bind(now - Duration::from_secs(60))
    .bind(user.id)
    .bind(format!("active labor fixture {key}"))
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let mut terminal_tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        r#"UPDATE labor_activities SET status='completed',revision=revision+1,
           completed_at=$2,actual_seconds=30,exception_seconds=0,completed_by_user_id=$3
           WHERE id=$1"#,
    )
    .bind(activity_id)
    .bind(now - Duration::from_secs(30))
    .bind(user.id)
    .execute(&mut *terminal_tx)
    .await
    .unwrap();
    sqlx::query(
        r#"UPDATE attendance_intervals SET status='closed',revision=revision+1,
           clocked_out_at=$2,paid_seconds=110,clock_out_note='historical shift closed',
           clocked_out_by_user_id=$3 WHERE id=$1"#,
    )
    .bind(attendance_id)
    .bind(now - Duration::from_secs(10))
    .bind(user.id)
    .execute(&mut *terminal_tx)
    .await
    .unwrap();
    terminal_tx.commit().await.unwrap();

    let adjusted_at = db::now_iso();
    let mut adjustment_tx = tenant_tx(&fixture.db, tenant_id).await;
    let attendance_adjustment_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO attendance_adjustments
          (tenant_id,attendance_interval_id,expected_revision,resulting_revision,
           before_clocked_in_at,before_clocked_out_at,before_paid_seconds,
           corrected_clocked_in_at,corrected_clocked_out_at,corrected_paid_seconds,
           correction_reason,correction_note,adjusted_by_user_id,adjusted_at)
          VALUES($1,$2,2,3,$3,$4,110,$5,$4,120,'timekeeping_error',
            'correct historical clock-in',$6,$7) RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(attendance_id)
    .bind(now - Duration::from_secs(120))
    .bind(now - Duration::from_secs(10))
    .bind(now - Duration::from_secs(130))
    .bind(user.id)
    .bind(adjusted_at)
    .fetch_one(&mut *adjustment_tx)
    .await
    .unwrap();
    let activity_adjustment_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO labor_activity_adjustments
          (tenant_id,labor_activity_id,expected_revision,resulting_revision,
           before_started_at,corrected_started_at,before_completed_at,corrected_completed_at,
           before_actual_seconds,corrected_actual_seconds,before_exception_seconds,
           corrected_exception_seconds,correction_reason,correction_note,
           adjusted_by_user_id,adjusted_at)
          VALUES($1,$2,2,3,$3,$4,$5,$5,30,40,0,0,'timekeeping_error',
            'correct historical activity start',$6,$7) RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(activity_id)
    .bind(now - Duration::from_secs(60))
    .bind(now - Duration::from_secs(70))
    .bind(now - Duration::from_secs(30))
    .bind(user.id)
    .bind(adjusted_at)
    .fetch_one(&mut *adjustment_tx)
    .await
    .unwrap();
    adjustment_tx.commit().await.unwrap();

    let historical_attendance_id = attendance_id;
    let historical_activity_id = activity_id;
    let mut current_tx = tenant_tx(&fixture.db, tenant_id).await;
    let attendance_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO attendance_intervals
          (tenant_id,employee_id,facility_id,status,clocked_in_at,clocked_in_by_user_id)
          VALUES($1,$2,$3,'open',$4,$5) RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(employee_id)
    .bind(facility_id)
    .bind(now - Duration::from_secs(5))
    .bind(user.id)
    .fetch_one(&mut *current_tx)
    .await
    .unwrap();
    let activity_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO labor_activities
          (tenant_id,attendance_interval_id,employee_id,facility_id,activity_kind,status,
           started_at,started_by_user_id,note)
          VALUES($1,$2,$3,$4,'break','active',$5,$6,$7) RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(attendance_id)
    .bind(employee_id)
    .bind(facility_id)
    .bind(now - Duration::from_secs(2))
    .bind(user.id)
    .bind(format!("current active labor fixture {key}"))
    .fetch_one(&mut *current_tx)
    .await
    .unwrap();
    current_tx.commit().await.unwrap();

    LaborRefs {
        tenant_id,
        actor_id: user.id,
        facility_id,
        employee_id,
        skill_id,
        certification_id,
        equipment_class_id,
        equipment_asset_id,
        standard_id,
        historical_attendance_id,
        historical_activity_id,
        attendance_id,
        activity_id,
        attendance_adjustment_id,
        activity_adjustment_id,
    }
}

pub(super) async fn assert_force_rls_and_exact_runtime_privileges(db: &db::Db) {
    let rls: Vec<(String, bool, bool)> = sqlx::query_as(
        r#"SELECT class.relname,class.relrowsecurity,class.relforcerowsecurity
           FROM unnest($1::TEXT[]) WITH ORDINALITY requested(table_name,ordinal)
           JOIN pg_catalog.pg_class class ON class.relname=requested.table_name
           JOIN pg_catalog.pg_namespace namespace ON namespace.oid=class.relnamespace
           WHERE namespace.nspname='public' ORDER BY requested.ordinal"#,
    )
    .bind(TABLES.as_slice())
    .fetch_all(db)
    .await
    .unwrap();
    assert_eq!(
        rls,
        TABLES
            .iter()
            .map(|table| ((*table).to_owned(), true, true))
            .collect::<Vec<_>>()
    );

    let table_privileges: Vec<TablePrivileges> = sqlx::query_as(
        r#"SELECT table_name,
               has_table_privilege(current_user,'public.'||table_name,'SELECT') can_select,
               has_table_privilege(current_user,'public.'||table_name,'INSERT') can_insert,
               has_table_privilege(current_user,'public.'||table_name,'UPDATE') can_update,
               has_table_privilege(current_user,'public.'||table_name,'DELETE') can_delete,
               has_table_privilege(current_user,'public.'||table_name,'TRUNCATE') can_truncate,
               has_table_privilege(current_user,'public.'||table_name,'REFERENCES') can_reference,
               has_table_privilege(current_user,'public.'||table_name,'TRIGGER') can_trigger
           FROM unnest($1::TEXT[]) WITH ORDINALITY requested(table_name,ordinal)
           ORDER BY ordinal"#,
    )
    .bind(TABLES.as_slice())
    .fetch_all(db)
    .await
    .unwrap();
    assert_eq!(
        table_privileges,
        TABLES
            .iter()
            .map(|table| TablePrivileges {
                table_name: (*table).to_owned(),
                can_select: true,
                can_insert: true,
                can_update: false,
                can_delete: false,
                can_truncate: false,
                can_reference: false,
                can_trigger: false,
            })
            .collect::<Vec<_>>()
    );

    let expected_update_columns = BTreeMap::from([
        (
            "labor_skills",
            vec![
                "name",
                "certification_required",
                "active",
                "revision",
                "configured_by_user_id",
                "configured_at",
            ],
        ),
        (
            "employee_certifications",
            vec![
                "revision",
                "revoked_by_user_id",
                "revoked_at",
                "revocation_note",
            ],
        ),
        (
            "equipment_classes",
            vec![
                "name",
                "required_skill_id",
                "active",
                "revision",
                "configured_by_user_id",
                "configured_at",
            ],
        ),
        (
            "equipment_assets",
            vec![
                "status",
                "assigned_employee_id",
                "revision",
                "status_note",
                "status_changed_by_user_id",
                "status_changed_at",
            ],
        ),
        (
            "labor_standards",
            vec!["effective_until", "retired_by_user_id", "retired_at"],
        ),
        (
            "attendance_intervals",
            vec![
                "status",
                "revision",
                "clocked_out_at",
                "paid_seconds",
                "clock_out_note",
                "clocked_out_by_user_id",
            ],
        ),
        (
            "labor_activities",
            vec![
                "status",
                "revision",
                "completed_at",
                "actual_seconds",
                "exception_seconds",
                "exception_reason",
                "exception_note",
                "exception_approved_by_user_id",
                "completed_quantity",
                "expected_seconds",
                "efficiency_basis_points",
                "completed_by_user_id",
                "cancelled_by_user_id",
                "note",
            ],
        ),
        ("attendance_adjustments", vec![]),
        ("labor_activity_adjustments", vec![]),
    ]);
    for table in TABLES {
        let actual: Vec<String> = sqlx::query_scalar(
            r#"SELECT column_name FROM information_schema.columns
               WHERE table_schema='public' AND table_name=$1
                 AND has_column_privilege(current_user,
                   format('public.%I',table_name),column_name,'UPDATE')
               ORDER BY ordinal_position"#,
        )
        .bind(table)
        .fetch_all(db)
        .await
        .unwrap();
        assert_eq!(
            actual,
            expected_update_columns[table]
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            "unexpected UPDATE grant for {table}"
        );
    }

    let sequence_privileges: Vec<(String, bool, bool, bool)> = sqlx::query_as(
        r#"SELECT sequence_name,
               has_sequence_privilege(current_user,'public.'||sequence_name,'USAGE'),
               has_sequence_privilege(current_user,'public.'||sequence_name,'SELECT'),
               has_sequence_privilege(current_user,'public.'||sequence_name,'UPDATE')
           FROM unnest($1::TEXT[]) WITH ORDINALITY requested(sequence_name,ordinal)
           ORDER BY ordinal"#,
    )
    .bind(SEQUENCES.as_slice())
    .fetch_all(db)
    .await
    .unwrap();
    assert_eq!(
        sequence_privileges,
        SEQUENCES
            .iter()
            .map(|sequence| ((*sequence).to_owned(), true, false, false))
            .collect::<Vec<_>>()
    );
}

pub(super) async fn visible_counts(db: &db::Db) -> [i64; 9] {
    let row = sqlx::query(
        r#"SELECT
          (SELECT COUNT(*) FROM labor_skills),
          (SELECT COUNT(*) FROM employee_certifications),
          (SELECT COUNT(*) FROM equipment_classes),
          (SELECT COUNT(*) FROM equipment_assets),
          (SELECT COUNT(*) FROM labor_standards),
          (SELECT COUNT(*) FROM attendance_intervals),
          (SELECT COUNT(*) FROM labor_activities),
          (SELECT COUNT(*) FROM attendance_adjustments),
          (SELECT COUNT(*) FROM labor_activity_adjustments)"#,
    )
    .fetch_one(db)
    .await
    .unwrap();
    std::array::from_fn(|index| row.try_get(index).unwrap())
}

pub(super) async fn visible_ids(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> [Vec<i64>; 9] {
    let row = sqlx::query(
        r#"SELECT
          ARRAY(SELECT id FROM labor_skills ORDER BY id),
          ARRAY(SELECT id FROM employee_certifications ORDER BY id),
          ARRAY(SELECT id FROM equipment_classes ORDER BY id),
          ARRAY(SELECT id FROM equipment_assets ORDER BY id),
          ARRAY(SELECT id FROM labor_standards ORDER BY id),
          ARRAY(SELECT id FROM attendance_intervals ORDER BY id),
          ARRAY(SELECT id FROM labor_activities ORDER BY id),
          ARRAY(SELECT id FROM attendance_adjustments ORDER BY id),
          ARRAY(SELECT id FROM labor_activity_adjustments ORDER BY id)"#,
    )
    .fetch_one(&mut **tx)
    .await
    .unwrap();
    std::array::from_fn(|index| row.try_get(index).unwrap())
}

pub(super) async fn self_update_counts(db: &db::Db, refs: LaborRefs) -> [u64; 7] {
    let mut connection = db.acquire().await.unwrap();
    self_update_counts_on(&mut connection, refs).await
}

pub(super) async fn self_update_counts_on(
    connection: &mut sqlx::PgConnection,
    refs: LaborRefs,
) -> [u64; 7] {
    let statements = [
        (
            "UPDATE labor_skills SET name=name WHERE id=$1",
            refs.skill_id,
        ),
        (
            "UPDATE employee_certifications SET revision=revision WHERE id=$1",
            refs.certification_id,
        ),
        (
            "UPDATE equipment_classes SET name=name WHERE id=$1",
            refs.equipment_class_id,
        ),
        (
            "UPDATE equipment_assets SET status=status WHERE id=$1",
            refs.equipment_asset_id,
        ),
        (
            "UPDATE labor_standards SET effective_until=effective_until WHERE id=$1",
            refs.standard_id,
        ),
        (
            "UPDATE attendance_intervals SET status=status WHERE id=$1",
            refs.attendance_id,
        ),
        (
            "UPDATE labor_activities SET status=status WHERE id=$1",
            refs.activity_id,
        ),
    ];
    let mut counts = [0; 7];
    for (index, (statement, id)) in statements.into_iter().enumerate() {
        counts[index] = sqlx::query(statement)
            .bind(id)
            .execute(&mut *connection)
            .await
            .unwrap()
            .rows_affected();
    }
    counts
}

pub(super) async fn assert_delete_is_denied_for_every_table(db: &db::Db, refs: LaborRefs) {
    for (table, id) in TABLES.into_iter().zip(refs.ids()) {
        let mut tx = tenant_tx(db, refs.tenant_id).await;
        assert!(
            sqlx::query(&format!("DELETE FROM {table} WHERE id=$1"))
                .bind(id)
                .execute(&mut *tx)
                .await
                .is_err(),
            "runtime role unexpectedly deleted {table}"
        );
        tx.rollback().await.unwrap();
    }
}

pub(super) async fn assert_forged_inserts_fail(
    db: &db::Db,
    context: Option<TenantId>,
    refs: LaborRefs,
    key: &str,
) {
    let now = db::now_iso();
    let attempts = [
        (
            "labor_skills",
            sqlx::query(
                r#"INSERT INTO labor_skills
                  (tenant_id,code,name,certification_required,active,configured_by_user_id,configured_at)
                  VALUES($1,$2,$2,false,true,$3,$4)"#,
            )
            .bind(refs.tenant_id.get())
            .bind(format!("FORGED-SKILL-{key}"))
            .bind(refs.actor_id)
            .bind(now),
        ),
        (
            "employee_certifications",
            sqlx::query(
                r#"INSERT INTO employee_certifications
                  (tenant_id,employee_id,skill_id,facility_id,certification_number,issued_at,
                   expires_at,certified_by_user_id,certified_at)
                  VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)"#,
            )
            .bind(refs.tenant_id.get())
            .bind(refs.employee_id)
            .bind(refs.skill_id)
            .bind(refs.facility_id)
            .bind(format!("FORGED-CERT-{key}"))
            .bind(now)
            .bind(now + Duration::from_secs(3_600))
            .bind(refs.actor_id)
            .bind(now),
        ),
        (
            "equipment_classes",
            sqlx::query(
                r#"INSERT INTO equipment_classes
                  (tenant_id,code,name,active,configured_by_user_id,configured_at)
                  VALUES($1,$2,$2,true,$3,$4)"#,
            )
            .bind(refs.tenant_id.get())
            .bind(format!("FORGED-CLASS-{key}"))
            .bind(refs.actor_id)
            .bind(now),
        ),
        (
            "equipment_assets",
            sqlx::query(
                r#"INSERT INTO equipment_assets
                  (tenant_id,facility_id,equipment_class_id,equipment_number,name,status,
                   configured_by_user_id,configured_at)
                  VALUES($1,$2,$3,$4,$4,'available',$5,$6)"#,
            )
            .bind(refs.tenant_id.get())
            .bind(refs.facility_id)
            .bind(refs.equipment_class_id)
            .bind(format!("FORGED-ASSET-{key}"))
            .bind(refs.actor_id)
            .bind(now),
        ),
        (
            "labor_standards",
            sqlx::query(
                r#"INSERT INTO labor_standards
                  (tenant_id,facility_id,code,name,activity_kind,quantity_basis,setup_seconds,
                   seconds_per_unit,effective_from,configured_by_user_id,configured_at)
                  VALUES($1,$2,$3,$3,'cycle_count','task',1,1,$4,$5,$4)"#,
            )
            .bind(refs.tenant_id.get())
            .bind(refs.facility_id)
            .bind(format!("FORGED-STANDARD-{key}"))
            .bind(now)
            .bind(refs.actor_id),
        ),
        (
            "attendance_intervals",
            sqlx::query(
                r#"INSERT INTO attendance_intervals
                  (tenant_id,employee_id,facility_id,status,clocked_in_at,clocked_in_by_user_id)
                  VALUES($1,$2,$3,'open',$4,$5)"#,
            )
            .bind(refs.tenant_id.get())
            .bind(refs.employee_id)
            .bind(refs.facility_id)
            .bind(now)
            .bind(refs.actor_id),
        ),
        (
            "labor_activities",
            sqlx::query(
                r#"INSERT INTO labor_activities
                  (tenant_id,attendance_interval_id,employee_id,facility_id,activity_kind,status,
                   started_at,started_by_user_id)
                  VALUES($1,$2,$3,$4,'break','active',$5,$6)"#,
            )
            .bind(refs.tenant_id.get())
            .bind(refs.attendance_id)
            .bind(refs.employee_id)
            .bind(refs.facility_id)
            .bind(now)
            .bind(refs.actor_id),
        ),
        (
            "attendance_adjustments",
            sqlx::query(
                r#"INSERT INTO attendance_adjustments
                  (tenant_id,attendance_interval_id,expected_revision,resulting_revision,
                   before_clocked_in_at,before_clocked_out_at,before_paid_seconds,
                   corrected_clocked_in_at,corrected_clocked_out_at,corrected_paid_seconds,
                   correction_reason,correction_note,adjusted_by_user_id,adjusted_at)
                  VALUES($1,$2,1,2,$3,$4,10,$5,$4,11,'other',$6,$7,$8)"#,
            )
            .bind(refs.tenant_id.get())
            .bind(refs.attendance_id)
            .bind(now - Duration::from_secs(20))
            .bind(now - Duration::from_secs(10))
            .bind(now - Duration::from_secs(21))
            .bind(format!("forged attendance adjustment {key}"))
            .bind(refs.actor_id)
            .bind(now),
        ),
        (
            "labor_activity_adjustments",
            sqlx::query(
                r#"INSERT INTO labor_activity_adjustments
                  (tenant_id,labor_activity_id,expected_revision,resulting_revision,
                   before_started_at,corrected_started_at,before_completed_at,
                   corrected_completed_at,before_actual_seconds,corrected_actual_seconds,
                   before_exception_seconds,corrected_exception_seconds,correction_reason,
                   correction_note,adjusted_by_user_id,adjusted_at)
                  VALUES($1,$2,1,2,$3,$4,$5,$5,10,11,0,0,'other',$6,$7,$8)"#,
            )
            .bind(refs.tenant_id.get())
            .bind(refs.activity_id)
            .bind(now - Duration::from_secs(20))
            .bind(now - Duration::from_secs(21))
            .bind(now - Duration::from_secs(10))
            .bind(format!("forged labor adjustment {key}"))
            .bind(refs.actor_id)
            .bind(now),
        ),
    ];

    for (table, query) in attempts {
        let mut tx = db.begin().await.unwrap();
        if let Some(tenant_id) = context {
            db::bind_tenant_context(&mut tx, tenant_id).await.unwrap();
        }
        assert!(
            query.execute(&mut *tx).await.is_err(),
            "forged {table} insert unexpectedly succeeded"
        );
        tx.rollback().await.unwrap();
    }
}

pub(super) async fn assert_forged_direct_snapshots_are_rejected(
    fixture: &Fixture,
    refs: LaborRefs,
) -> i64 {
    let worker = fixture.user("labor-forged-worker@test.local").await;
    let mut membership_tx = tenant_tx(&fixture.db, refs.tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships(tenant_id,user_id) VALUES($1,$2)")
        .bind(refs.tenant_id.get())
        .bind(worker.id)
        .execute(&mut *membership_tx)
        .await
        .unwrap();
    membership_tx.commit().await.unwrap();

    let access = repo::tenants::access_for_user(&fixture.db, refs.actor_id, refs.tenant_id)
        .await
        .unwrap()
        .unwrap();
    let now = db::now_iso();
    let employee_id = repo::employees::add_employee(
        &fixture.db,
        refs.tenant_id,
        &access.site_scope,
        &repo::employees::NewEmployee {
            first_name: "Forged",
            last_name: "Worker",
            title: "Adversarial test worker",
            employee_type: "hourly",
            email: None,
            phone: None,
            hired: now - Duration::from_secs(86_400),
            facility_ids: &[refs.facility_id],
        },
    )
    .await
    .unwrap();
    link_employee_identity(
        fixture,
        refs.tenant_id,
        refs.actor_id,
        employee_id,
        worker.id,
        "forged-worker",
    )
    .await;

    let mut setup_tx = tenant_tx(&fixture.db, refs.tenant_id).await;
    let attendance_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO attendance_intervals
          (tenant_id,employee_id,facility_id,status,clocked_in_at,clocked_in_by_user_id)
          VALUES($1,$2,$3,'open',$4,$5) RETURNING id"#,
    )
    .bind(refs.tenant_id.get())
    .bind(employee_id)
    .bind(refs.facility_id)
    .bind(now - Duration::from_secs(60))
    .bind(refs.actor_id)
    .fetch_one(&mut *setup_tx)
    .await
    .unwrap();
    let task_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO work_tasks
          (tenant_id,created,task_type,status,required_permission,priority,title,
           assigned_user_id,created_by,started_at,lease_expires_at,task_timeout_seconds,
           facility_id)
          VALUES($1,$2,'cycle_count_location','in_progress','wms',0,
            'Forged direct snapshot source',$3,$4,$2,$5,1800,$6) RETURNING id"#,
    )
    .bind(refs.tenant_id.get())
    .bind(now)
    .bind(worker.id)
    .bind(refs.actor_id)
    .bind(now + Duration::from_secs(3_600))
    .bind(refs.facility_id)
    .fetch_one(&mut *setup_tx)
    .await
    .unwrap();
    setup_tx.commit().await.unwrap();

    let attempts = [
        (
            "quantity basis does not reconcile",
            r#"INSERT INTO labor_activities
              (tenant_id,attendance_interval_id,employee_id,facility_id,activity_kind,
               quantity_basis,status,reference_type,reference_id,reference_quantity,
               started_at,started_by_user_id)
              VALUES($1,$2,$3,$4,'cycle_count','task','active','work_task',$5,2,$6,$7)"#,
        ),
        (
            "standard snapshot",
            r#"INSERT INTO labor_activities
              (tenant_id,attendance_interval_id,employee_id,facility_id,activity_kind,
               quantity_basis,status,labor_standard_id,reference_type,reference_id,
               reference_quantity,standard_setup_seconds,standard_seconds_per_unit,
               required_skill_id,required_skill_certification_id,
               required_equipment_class_id,started_at,started_by_user_id)
              VALUES($1,$2,$3,$4,'cycle_count','task','active',$8,'work_task',$5,
                1,999,5,$9,$10,$11,$6,$7)"#,
        ),
        (
            "certification",
            r#"INSERT INTO labor_activities
              (tenant_id,attendance_interval_id,employee_id,facility_id,activity_kind,
               quantity_basis,status,labor_standard_id,reference_type,reference_id,
               reference_quantity,standard_setup_seconds,standard_seconds_per_unit,
               required_skill_id,required_skill_certification_id,
               required_equipment_class_id,started_at,started_by_user_id)
              VALUES($1,$2,$3,$4,'cycle_count','task','active',$8,'work_task',$5,
                1,10,5,$9,$10,$11,$6,$7)"#,
        ),
        (
            "equipment assignment",
            r#"INSERT INTO labor_activities
              (tenant_id,attendance_interval_id,employee_id,facility_id,activity_kind,
               quantity_basis,status,equipment_asset_id,reference_type,reference_id,
               reference_quantity,started_at,started_by_user_id)
              VALUES($1,$2,$3,$4,'cycle_count','task','active',$8,'work_task',$5,
                1,$6,$7)"#,
        ),
    ];
    for (index, (message, statement)) in attempts.into_iter().enumerate() {
        let mut tx = tenant_tx(&fixture.db, refs.tenant_id).await;
        let query = sqlx::query(statement)
            .bind(refs.tenant_id.get())
            .bind(attendance_id)
            .bind(employee_id)
            .bind(refs.facility_id)
            .bind(task_id)
            .bind(now)
            .bind(refs.actor_id);
        let result = match index {
            0 => query.execute(&mut *tx).await,
            1 | 2 => {
                query
                    .bind(refs.standard_id)
                    .bind(refs.skill_id)
                    .bind(refs.certification_id)
                    .bind(refs.equipment_class_id)
                    .execute(&mut *tx)
                    .await
            }
            3 => query.bind(refs.equipment_asset_id).execute(&mut *tx).await,
            _ => unreachable!(),
        };
        expect_statement_or_commit_failure(tx, result, message).await;
    }
    worker.id
}

pub(super) async fn prepare_startable_labor(fixture: &Fixture, key: &str) -> StartableLaborRefs {
    let core = seed_labor_with_identity(
        fixture,
        &format!("labor-direct-{}@test.local", key.to_ascii_lowercase()),
        key,
        true,
    )
    .await;

    let mut close_break = tenant_tx(&fixture.db, core.tenant_id).await;
    sqlx::query(
        r#"UPDATE labor_activities SET status='cancelled',revision=revision+1,
           completed_at=statement_timestamp(),
           actual_seconds=trunc(EXTRACT(EPOCH FROM statement_timestamp()-started_at))::bigint,
           cancelled_by_user_id=$2,note='prepare direct labor fixture' WHERE id=$1"#,
    )
    .bind(core.activity_id)
    .bind(core.actor_id)
    .execute(&mut *close_break)
    .await
    .unwrap();
    close_break.commit().await.unwrap();

    let now = db::now_iso();
    let mut task_tx = tenant_tx(&fixture.db, core.tenant_id).await;
    let task_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO work_tasks
          (tenant_id,created,task_type,status,required_permission,priority,title,
           assigned_user_id,created_by,started_at,lease_expires_at,task_timeout_seconds,
           facility_id)
          VALUES($1,$2,'cycle_count_location','in_progress','wms',0,$3,
            $4,$4,$2,$5,1800,$6) RETURNING id"#,
    )
    .bind(core.tenant_id.get())
    .bind(now)
    .bind(format!("Labor source task {key}"))
    .bind(core.actor_id)
    .bind(now + Duration::from_secs(3_600))
    .bind(core.facility_id)
    .fetch_one(&mut *task_tx)
    .await
    .unwrap();
    task_tx.commit().await.unwrap();

    StartableLaborRefs { core, task_id }
}

pub(super) async fn prepare_direct_labor(fixture: &Fixture, key: &str) -> DirectLaborRefs {
    let startable = prepare_startable_labor(fixture, key).await;
    let mut activity_tx = tenant_tx(&fixture.db, startable.core.tenant_id).await;
    let (activity_id, started_at) =
        stage_valid_direct_labor(&mut activity_tx, startable, key, false).await;
    activity_tx.commit().await.unwrap();

    DirectLaborRefs {
        core: startable.core,
        task_id: startable.task_id,
        activity_id,
        started_at,
    }
}

pub(super) async fn stage_valid_direct_labor(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    startable: StartableLaborRefs,
    key: &str,
    lock_certification: bool,
) -> (i64, wareboxes_domain::Timestamp) {
    let core = startable.core;
    if lock_certification {
        sqlx::query("SELECT id FROM employee_certifications WHERE id=$1 FOR SHARE")
            .bind(core.certification_id)
            .fetch_one(&mut **tx)
            .await
            .unwrap();
    }
    let started_at = db::now_iso();
    sqlx::query(
        r#"UPDATE equipment_assets SET status='assigned',assigned_employee_id=$2,
           revision=revision+1,status_note='assigned to direct labor',
           status_changed_by_user_id=$3,status_changed_at=$4 WHERE id=$1"#,
    )
    .bind(core.equipment_asset_id)
    .bind(core.employee_id)
    .bind(core.actor_id)
    .bind(started_at)
    .execute(&mut **tx)
    .await
    .unwrap();
    let activity_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO labor_activities
          (tenant_id,attendance_interval_id,employee_id,facility_id,inventory_owner_id,
           activity_kind,quantity_basis,status,labor_standard_id,equipment_asset_id,
           reference_type,reference_id,reference_quantity,standard_setup_seconds,
           standard_seconds_per_unit,required_skill_id,required_skill_certification_id,
           required_equipment_class_id,equipment_required_skill_id,
           equipment_skill_certification_id,started_at,started_by_user_id,note)
          VALUES($1,$2,$3,$4,NULL,'cycle_count','task','active',$5,$6,
            'work_task',$7,1,10,5,$8,$9,$10,$8,$9,$11,$12,$13) RETURNING id"#,
    )
    .bind(core.tenant_id.get())
    .bind(core.attendance_id)
    .bind(core.employee_id)
    .bind(core.facility_id)
    .bind(core.standard_id)
    .bind(core.equipment_asset_id)
    .bind(startable.task_id)
    .bind(core.skill_id)
    .bind(core.certification_id)
    .bind(core.equipment_class_id)
    .bind(started_at)
    .bind(core.actor_id)
    .bind(format!("active direct labor fixture {key}"))
    .fetch_one(&mut **tx)
    .await
    .unwrap();
    (activity_id, started_at)
}

pub(super) async fn grant_admin_permission(
    fixture: &Fixture,
    tenant_id: TenantId,
    actor_id: i64,
    key: &str,
) {
    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        &fixture.db,
        tenant_id,
        "admin",
        Some("Labor RLS fixture administrator"),
    )
    .await
    .unwrap();
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("labor-rls-admin-{key}"),
        Some("Labor RLS fixture administrator"),
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
    wareboxes_persistence_postgres::roles::add_role_to_user(&fixture.db, tenant_id, actor_id, role)
        .await
        .unwrap();
}

pub(super) async fn link_employee_identity(
    fixture: &Fixture,
    tenant_id: TenantId,
    actor_id: i64,
    employee_id: i64,
    linked_user_id: i64,
    key: &str,
) {
    let access = repo::tenants::access_for_user(&fixture.db, actor_id, tenant_id)
        .await
        .unwrap()
        .unwrap();
    repo::employees::link_employee_identity(
        &fixture.db,
        &access,
        &CommandContext {
            tenant_id,
            actor_id: UserId::new(actor_id).unwrap(),
            request_id: format!("labor-rls-link-{key}"),
            idempotency_key: Some(format!("labor-rls-link-{key}")),
        },
        &LinkEmployeeIdentityCommand {
            employee_id: EmployeeId::new(employee_id).unwrap(),
            user_id: UserId::new(linked_user_id).unwrap(),
            expected_user_id: None,
            reason: EmployeeIdentityReason::new("link employee for labor database acceptance test")
                .unwrap(),
        },
    )
    .await
    .unwrap();
}

pub(super) async fn expect_statement_or_commit_failure(
    tx: sqlx::Transaction<'_, sqlx::Postgres>,
    statement_result: Result<sqlx::postgres::PgQueryResult, sqlx::Error>,
    expected_message: &str,
) {
    let error = match statement_result {
        Ok(_) => tx
            .commit()
            .await
            .expect_err("adversarial labor mutation unexpectedly committed"),
        Err(error) => {
            tx.rollback().await.unwrap();
            error
        }
    };
    let message = error.to_string().to_ascii_lowercase();
    assert!(
        message.contains(&expected_message.to_ascii_lowercase()),
        "expected SQL error containing {expected_message:?}, got {error}"
    );
}

pub(super) async fn finish_transaction(
    tx: sqlx::Transaction<'_, sqlx::Postgres>,
    statement_result: Result<sqlx::postgres::PgQueryResult, sqlx::Error>,
) -> Result<(), sqlx::Error> {
    match statement_result {
        Ok(_) => tx.commit().await,
        Err(error) => {
            tx.rollback().await.unwrap();
            Err(error)
        }
    }
}

pub(super) fn assert_error_contains(error: &sqlx::Error, expected_message: &str) {
    assert!(
        error
            .to_string()
            .to_ascii_lowercase()
            .contains(&expected_message.to_ascii_lowercase()),
        "expected SQL error containing {expected_message:?}, got {error}"
    );
}

pub(super) fn assert_sqlstate(error: &sqlx::Error, expected: &str) {
    assert_eq!(
        error.as_database_error().and_then(|error| error.code()),
        Some(std::borrow::Cow::Borrowed(expected)),
        "unexpected database error: {error}"
    );
}
