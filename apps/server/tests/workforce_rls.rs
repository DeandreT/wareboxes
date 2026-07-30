mod common;

use common::*;

const TABLES: [&str; 2] = ["employees", "employee_facilities"];
const SEQUENCES: [&str; 2] = ["employees_id_seq", "employee_facilities_id_seq"];

#[derive(Clone, Copy)]
struct WorkforceRefs {
    tenant_id: TenantId,
    employee_id: i64,
    employee_facility_id: i64,
    facility_id: i64,
}

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

#[tokio::test]
async fn workforce_requires_tenant_context_and_exact_runtime_privileges() {
    let fixture = Fixture::new().await;
    assert_exact_runtime_privileges(&fixture.db).await;

    let refs_a = workforce_refs(&fixture, "workforce-rls-a@test.com", "WORKFORCE-A").await;
    let refs_b = workforce_refs(&fixture, "workforce-rls-b@test.com", "WORKFORCE-B").await;
    assert_repository_workforce(&fixture.db, refs_a).await;
    assert_repository_workforce(&fixture.db, refs_b).await;

    let snapshot_a = snapshot(&fixture.db, refs_a.tenant_id).await;
    let snapshot_b = snapshot(&fixture.db, refs_b.tenant_id).await;

    let unbound_counts: (i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM employees),
               (SELECT COUNT(*) FROM employee_facilities)
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(unbound_counts, (0, 0));

    let mut unbound = fixture.db.acquire().await.unwrap();
    assert_eq!(allowed_update_counts(&mut unbound, refs_a).await, [0, 0]);
    drop(unbound);

    let mut tenant_b_tx = tenant_tx(&fixture.db, refs_b.tenant_id).await;
    assert_eq!(
        visible_ids(&mut tenant_b_tx).await,
        (vec![refs_b.employee_id], vec![refs_b.employee_facility_id])
    );
    assert_eq!(
        allowed_update_counts(&mut tenant_b_tx, refs_a).await,
        [0, 0]
    );
    tenant_b_tx.rollback().await.unwrap();

    assert_deletes_fail(&fixture.db, refs_a).await;
    assert_forged_inserts_fail(&fixture.db, None, refs_a, "UNBOUND").await;
    assert_forged_inserts_fail(&fixture.db, Some(refs_b.tenant_id), refs_a, "CROSS").await;

    let mut invariant_tx = tenant_tx(&fixture.db, refs_a.tenant_id).await;
    sqlx::query("UPDATE employee_facilities SET deleted = clock_timestamp() WHERE id = $1")
        .bind(refs_a.employee_facility_id)
        .execute(&mut *invariant_tx)
        .await
        .unwrap();
    assert!(invariant_tx.commit().await.is_err());

    assert_repository_workforce(&fixture.db, refs_a).await;
    assert_repository_workforce(&fixture.db, refs_b).await;
    assert_eq!(snapshot(&fixture.db, refs_a.tenant_id).await, snapshot_a);
    assert_eq!(snapshot(&fixture.db, refs_b.tenant_id).await, snapshot_b);
}

async fn workforce_refs(fixture: &Fixture, email: &str, key: &str) -> WorkforceRefs {
    let user = fixture.user(email).await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let access = repo::tenants::access_for_user(&fixture.db, user.id, tenant_id)
        .await
        .unwrap()
        .unwrap();
    let facility_id = fixture.facility(tenant_id, key).await;
    let employee_id = repo::employees::add_employee(
        &fixture.db,
        tenant_id,
        &access.site_scope,
        &repo::employees::NewEmployee {
            first_name: key,
            last_name: "Operator",
            title: "Picker",
            employee_type: "employee",
            email: Some(email),
            phone: None,
            hired: db::now_iso(),
            facility_ids: &[facility_id],
        },
    )
    .await
    .unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let employee_facility_id: i64 = sqlx::query_scalar(
        r#"
        SELECT id
        FROM employee_facilities
        WHERE tenant_id = $1 AND employee_id = $2 AND facility_id = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(employee_id)
    .bind(facility_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    WorkforceRefs {
        tenant_id,
        employee_id,
        employee_facility_id,
        facility_id,
    }
}

async fn assert_repository_workforce(db: &db::Db, refs: WorkforceRefs) {
    let mut tx = tenant_tx(db, refs.tenant_id).await;
    let user_id: i64 = sqlx::query_scalar(
        "SELECT user_id FROM tenant_memberships WHERE tenant_id = $1 ORDER BY id LIMIT 1",
    )
    .bind(refs.tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let access = repo::tenants::access_for_user(db, user_id, refs.tenant_id)
        .await
        .unwrap()
        .unwrap();
    let employees =
        repo::employees::get_employees_in_scope(db, refs.tenant_id, &access.site_scope, false)
            .await
            .unwrap();
    assert_eq!(employees.len(), 1);
    assert_eq!(employees[0].id, refs.employee_id);
    assert_eq!(employees[0].facility_ids, vec![refs.facility_id]);
}

async fn visible_ids(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> (Vec<i64>, Vec<i64>) {
    sqlx::query_as(
        r#"
        SELECT ARRAY(SELECT id FROM employees ORDER BY id),
               ARRAY(SELECT id FROM employee_facilities ORDER BY id)
        "#,
    )
    .fetch_one(&mut **tx)
    .await
    .unwrap()
}

async fn allowed_update_counts(
    connection: &mut sqlx::PgConnection,
    refs: WorkforceRefs,
) -> [u64; 2] {
    let employee = sqlx::query("UPDATE employees SET title = title WHERE id = $1")
        .bind(refs.employee_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    let assignment = sqlx::query("UPDATE employee_facilities SET deleted = deleted WHERE id = $1")
        .bind(refs.employee_facility_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    [employee, assignment]
}

async fn assert_deletes_fail(db: &db::Db, refs: WorkforceRefs) {
    for (table, id) in [
        ("employees", refs.employee_id),
        ("employee_facilities", refs.employee_facility_id),
    ] {
        let mut tx = tenant_tx(db, refs.tenant_id).await;
        assert!(
            sqlx::query(&format!("DELETE FROM {table} WHERE id = $1"))
                .bind(id)
                .execute(&mut *tx)
                .await
                .is_err(),
            "{table} must not be deleteable by the runtime role"
        );
        tx.rollback().await.unwrap();
    }
}

async fn assert_forged_inserts_fail(
    db: &db::Db,
    context: Option<TenantId>,
    refs: WorkforceRefs,
    key: &str,
) {
    let mut employee_tx = db.begin().await.unwrap();
    if let Some(tenant_id) = context {
        db::bind_tenant_context(&mut employee_tx, tenant_id)
            .await
            .unwrap();
    }
    assert!(sqlx::query(
        r#"
            INSERT INTO employees
                (tenant_id, created, first_name, last_name, title, type, hired)
            VALUES ($1, $2, $3, 'Forged', 'Picker', 'employee', $2)
            "#,
    )
    .bind(refs.tenant_id.get())
    .bind(db::now_iso())
    .bind(key)
    .execute(&mut *employee_tx)
    .await
    .is_err());
    employee_tx.rollback().await.unwrap();

    let mut assignment_tx = db.begin().await.unwrap();
    if let Some(tenant_id) = context {
        db::bind_tenant_context(&mut assignment_tx, tenant_id)
            .await
            .unwrap();
    }
    assert!(sqlx::query(
        r#"
            INSERT INTO employee_facilities
                (tenant_id, created, employee_id, facility_id)
            VALUES ($1, $2, $3, $4)
            "#,
    )
    .bind(refs.tenant_id.get())
    .bind(db::now_iso())
    .bind(refs.employee_id)
    .bind(refs.facility_id)
    .execute(&mut *assignment_tx)
    .await
    .is_err());
    assignment_tx.rollback().await.unwrap();
}

async fn assert_exact_runtime_privileges(db: &db::Db) {
    let table_privileges: Vec<TablePrivileges> = sqlx::query_as(
        r#"
        SELECT table_name,
               has_table_privilege(current_user, 'public.' || table_name, 'SELECT')
                   AS can_select,
               has_table_privilege(current_user, 'public.' || table_name, 'INSERT')
                   AS can_insert,
               has_table_privilege(current_user, 'public.' || table_name, 'UPDATE')
                   AS can_update,
               has_table_privilege(current_user, 'public.' || table_name, 'DELETE')
                   AS can_delete,
               has_table_privilege(current_user, 'public.' || table_name, 'TRUNCATE')
                   AS can_truncate,
               has_table_privilege(current_user, 'public.' || table_name, 'REFERENCES')
                   AS can_reference,
               has_table_privilege(current_user, 'public.' || table_name, 'TRIGGER')
                   AS can_trigger
        FROM unnest($1::TEXT[]) WITH ORDINALITY AS tables(table_name, ordinal)
        ORDER BY ordinal
        "#,
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
                can_update: true,
                can_delete: false,
                can_truncate: false,
                can_reference: false,
                can_trigger: false,
            })
            .collect::<Vec<_>>()
    );

    let sequence_privileges: Vec<(String, bool, bool, bool)> = sqlx::query_as(
        r#"
        SELECT sequence_name,
               has_sequence_privilege(current_user, 'public.' || sequence_name, 'USAGE'),
               has_sequence_privilege(current_user, 'public.' || sequence_name, 'SELECT'),
               has_sequence_privilege(current_user, 'public.' || sequence_name, 'UPDATE')
        FROM unnest($1::TEXT[]) WITH ORDINALITY AS sequences(sequence_name, ordinal)
        ORDER BY ordinal
        "#,
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

    let function_privileges: (bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT has_function_privilege(
                   current_user,
                   'public.assert_employee_active_facility(bigint,bigint)',
                   'EXECUTE'
               ),
               has_function_privilege(
                   current_user,
                   'public.enforce_employee_active_facility()',
                   'EXECUTE'
               ),
               has_function_privilege(
                   current_user,
                   'public.retire_deleted_facility_employee_assignments()',
                   'EXECUTE'
               )
        "#,
    )
    .fetch_one(db)
    .await
    .unwrap();
    assert_eq!(function_privileges, (true, false, false));
}

async fn snapshot(db: &db::Db, tenant_id: TenantId) -> String {
    let mut tx = tenant_tx(db, tenant_id).await;
    let snapshot = sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
            'employees',
                COALESCE(
                    (SELECT jsonb_agg(to_jsonb(employee) ORDER BY id) FROM employees employee),
                    '[]'::jsonb
                ),
            'employee_facilities',
                COALESCE(
                    (SELECT jsonb_agg(to_jsonb(assignment) ORDER BY id)
                     FROM employee_facilities assignment),
                    '[]'::jsonb
                )
        )::TEXT
        "#,
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    snapshot
}
