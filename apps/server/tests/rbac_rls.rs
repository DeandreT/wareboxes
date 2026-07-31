mod common;

use common::*;

const TABLES: [&str; 4] = ["roles", "permissions", "user_roles", "role_permissions"];
const SEQUENCES: [&str; 4] = [
    "roles_id_seq",
    "permissions_id_seq",
    "user_roles_id_seq",
    "role_permissions_id_seq",
];

#[derive(Clone, Copy)]
struct RbacRefs {
    tenant_id: TenantId,
    user_id: i64,
    role_id: i64,
    permission_id: i64,
    unassigned_role_id: i64,
    unassigned_permission_id: i64,
    user_role_id: i64,
    role_permission_id: i64,
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

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct SequencePrivileges {
    sequence_name: String,
    can_use: bool,
    can_select: bool,
    can_update: bool,
}

#[tokio::test]
async fn rbac_requires_tenant_context_and_exact_runtime_privileges() {
    let fixture = Fixture::new().await;
    assert_exact_runtime_privileges(&fixture.db).await;

    let refs_a = rbac_refs(&fixture, "rbac-rls-a@test.com", "RBAC-RLS-A").await;
    let refs_b = rbac_refs(&fixture, "rbac-rls-b@test.com", "RBAC-RLS-B").await;
    assert_repository_rbac(&fixture.db, refs_a, "RBAC-RLS-A").await;
    assert_repository_rbac(&fixture.db, refs_b, "RBAC-RLS-B").await;

    let snapshot_a = snapshot(&fixture.db, refs_a.tenant_id).await;
    let snapshot_b = snapshot(&fixture.db, refs_b.tenant_id).await;
    let visible_b = tenant_visible_ids(&fixture.db, refs_b.tenant_id).await;
    assert!(visible_b.0.contains(&refs_b.role_id));
    assert!(visible_b.1.contains(&refs_b.permission_id));
    assert!(visible_b.2.contains(&refs_b.user_role_id));
    assert!(visible_b.3.contains(&refs_b.role_permission_id));

    let unbound_counts: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM roles),
               (SELECT COUNT(*) FROM permissions),
               (SELECT COUNT(*) FROM user_roles),
               (SELECT COUNT(*) FROM role_permissions)
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(unbound_counts, (0, 0, 0, 0));

    let mut unbound = fixture.db.acquire().await.unwrap();
    assert_eq!(allowed_update_counts(&mut unbound, refs_a).await, [0; 4]);
    drop(unbound);

    let mut tenant_b_tx = tenant_tx(&fixture.db, refs_b.tenant_id).await;
    assert_eq!(visible_ids(&mut tenant_b_tx).await, visible_b);
    assert_eq!(
        allowed_update_counts(&mut tenant_b_tx, refs_a).await,
        [0; 4]
    );
    tenant_b_tx.rollback().await.unwrap();

    assert_deletes_fail(&fixture.db, refs_a).await;
    assert_forged_inserts_fail(&fixture.db, None, refs_a, "UNBOUND").await;
    assert_forged_inserts_fail(&fixture.db, Some(refs_b.tenant_id), refs_a, "CROSS").await;
    assert_cross_dimensional_assignments_fail(&fixture.db, refs_a, refs_b).await;
    assert_role_invariants_are_database_enforced(&fixture.db, refs_a).await;

    assert_repository_rbac(&fixture.db, refs_a, "RBAC-RLS-A").await;
    assert_repository_rbac(&fixture.db, refs_b, "RBAC-RLS-B").await;
    assert_eq!(snapshot(&fixture.db, refs_a.tenant_id).await, snapshot_a);
    assert_eq!(snapshot(&fixture.db, refs_b.tenant_id).await, snapshot_b);
}

async fn rbac_refs(fixture: &Fixture, email: &str, key: &str) -> RbacRefs {
    let user = fixture.user(email).await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let permission_id = wareboxes_persistence_postgres::permissions::add_permission(
        &fixture.db,
        tenant_id,
        key,
        Some("RLS permission"),
    )
    .await
    .unwrap();
    let role_id = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        key,
        Some("RLS role"),
    )
    .await
    .unwrap();
    let unassigned_permission_id = wareboxes_persistence_postgres::permissions::add_permission(
        &fixture.db,
        tenant_id,
        &format!("{key}-UNASSIGNED"),
        Some("Unassigned RLS permission"),
    )
    .await
    .unwrap();
    let unassigned_role_id = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("{key}-UNASSIGNED"),
        Some("Unassigned RLS role"),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role_id,
        permission_id
    )
    .await
    .unwrap());
    assert!(wareboxes_persistence_postgres::roles::add_role_to_user(
        &fixture.db,
        tenant_id,
        user.id,
        role_id
    )
    .await
    .unwrap());

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let (user_role_id, role_permission_id): (i64, i64) = sqlx::query_as(
        r#"
        SELECT (
                   SELECT id
                   FROM user_roles
                   WHERE tenant_id = $1 AND user_id = $2 AND role_id = $3
               ),
               (
                   SELECT id
                   FROM role_permissions
                   WHERE tenant_id = $1 AND role_id = $3 AND permission_id = $4
               )
        "#,
    )
    .bind(tenant_id.get())
    .bind(user.id)
    .bind(role_id)
    .bind(permission_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    RbacRefs {
        tenant_id,
        user_id: user.id,
        role_id,
        permission_id,
        unassigned_role_id,
        unassigned_permission_id,
        user_role_id,
        role_permission_id,
    }
}

async fn assert_repository_rbac(db: &db::Db, refs: RbacRefs, key: &str) {
    let role = wareboxes_persistence_postgres::roles::get_role(db, refs.tenant_id, refs.role_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(role.id, refs.role_id);
    assert!(role
        .role_permissions
        .iter()
        .any(|permission| permission.id == refs.permission_id));

    let permissions =
        wareboxes_persistence_postgres::permissions::get_permissions(db, refs.tenant_id, false)
            .await
            .unwrap();
    assert!(permissions
        .iter()
        .any(|permission| permission.id == refs.permission_id));

    let user_permissions = permissions::get_user_permissions(db, refs.tenant_id, refs.user_id)
        .await
        .unwrap();
    assert!(user_permissions
        .iter()
        .any(|permission| permission.name == key));
}

async fn tenant_visible_ids(
    db: &db::Db,
    tenant_id: TenantId,
) -> (Vec<i64>, Vec<i64>, Vec<i64>, Vec<i64>) {
    let mut tx = tenant_tx(db, tenant_id).await;
    let ids = visible_ids(&mut tx).await;
    tx.rollback().await.unwrap();
    ids
}

async fn visible_ids(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> (Vec<i64>, Vec<i64>, Vec<i64>, Vec<i64>) {
    sqlx::query_as(
        r#"
        SELECT ARRAY(SELECT id FROM roles ORDER BY id),
               ARRAY(SELECT id FROM permissions ORDER BY id),
               ARRAY(SELECT id FROM user_roles ORDER BY id),
               ARRAY(SELECT id FROM role_permissions ORDER BY id)
        "#,
    )
    .fetch_one(&mut **tx)
    .await
    .unwrap()
}

async fn allowed_update_counts(connection: &mut sqlx::PgConnection, refs: RbacRefs) -> [u64; 4] {
    let role = sqlx::query("UPDATE roles SET description = description WHERE id = $1")
        .bind(refs.role_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    let permission = sqlx::query("UPDATE permissions SET description = description WHERE id = $1")
        .bind(refs.permission_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    let user_role = sqlx::query("UPDATE user_roles SET deleted = deleted WHERE id = $1")
        .bind(refs.user_role_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    let role_permission =
        sqlx::query("UPDATE role_permissions SET deleted = deleted WHERE id = $1")
            .bind(refs.role_permission_id)
            .execute(&mut *connection)
            .await
            .unwrap()
            .rows_affected();
    [role, permission, user_role, role_permission]
}

async fn assert_deletes_fail(db: &db::Db, refs: RbacRefs) {
    for (table, id) in [
        ("roles", refs.role_id),
        ("permissions", refs.permission_id),
        ("user_roles", refs.user_role_id),
        ("role_permissions", refs.role_permission_id),
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
    refs: RbacRefs,
    key: &str,
) {
    for table in TABLES {
        let mut tx = db.begin().await.unwrap();
        if let Some(tenant_id) = context {
            db::bind_tenant_context(&mut tx, tenant_id).await.unwrap();
        }
        let result = match table {
            "roles" => {
                sqlx::query("INSERT INTO roles (tenant_id, created, name) VALUES ($1, $2, $3)")
                    .bind(refs.tenant_id.get())
                    .bind(db::now_iso())
                    .bind(format!("{key}-ROLE"))
                    .execute(&mut *tx)
                    .await
            }
            "permissions" => {
                sqlx::query(
                    "INSERT INTO permissions (tenant_id, created, name) VALUES ($1, $2, $3)",
                )
                .bind(refs.tenant_id.get())
                .bind(db::now_iso())
                .bind(format!("{key}-PERMISSION"))
                .execute(&mut *tx)
                .await
            }
            "user_roles" => {
                sqlx::query(
                    r#"
                    INSERT INTO user_roles (tenant_id, created, user_id, role_id)
                    VALUES ($1, $2, $3, $4)
                    "#,
                )
                .bind(refs.tenant_id.get())
                .bind(db::now_iso())
                .bind(refs.user_id)
                .bind(refs.unassigned_role_id)
                .execute(&mut *tx)
                .await
            }
            "role_permissions" => {
                sqlx::query(
                    r#"
                    INSERT INTO role_permissions
                        (tenant_id, created, role_id, permission_id)
                    VALUES ($1, $2, $3, $4)
                    "#,
                )
                .bind(refs.tenant_id.get())
                .bind(db::now_iso())
                .bind(refs.role_id)
                .bind(refs.unassigned_permission_id)
                .execute(&mut *tx)
                .await
            }
            _ => unreachable!(),
        };
        assert!(
            result.is_err(),
            "{table} accepted an insert outside its tenant context"
        );
        tx.rollback().await.unwrap();
    }
}

async fn assert_cross_dimensional_assignments_fail(
    db: &db::Db,
    refs_a: RbacRefs,
    refs_b: RbacRefs,
) {
    let mut user_role_tx = tenant_tx(db, refs_a.tenant_id).await;
    assert!(
        sqlx::query(
            r#"
            INSERT INTO user_roles (tenant_id, created, user_id, role_id)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(refs_a.tenant_id.get())
        .bind(db::now_iso())
        .bind(refs_b.user_id)
        .bind(refs_a.role_id)
        .execute(&mut *user_role_tx)
        .await
        .is_err(),
        "a role accepted a user from another tenant"
    );
    user_role_tx.rollback().await.unwrap();

    let mut role_permission_tx = tenant_tx(db, refs_a.tenant_id).await;
    assert!(
        sqlx::query(
            r#"
            INSERT INTO role_permissions (tenant_id, created, role_id, permission_id)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(refs_a.tenant_id.get())
        .bind(db::now_iso())
        .bind(refs_a.role_id)
        .bind(refs_b.permission_id)
        .execute(&mut *role_permission_tx)
        .await
        .is_err(),
        "a role accepted a permission from another tenant"
    );
    role_permission_tx.rollback().await.unwrap();
}

async fn assert_role_invariants_are_database_enforced(db: &db::Db, refs: RbacRefs) {
    let mut self_role_tx = tenant_tx(db, refs.tenant_id).await;
    let (self_role_id, self_user_role_id): (i64, i64) = sqlx::query_as(
        r#"
        SELECT role.id, user_role.id
        FROM roles role
        INNER JOIN user_roles user_role
            ON user_role.tenant_id = role.tenant_id
           AND user_role.role_id = role.id
           AND user_role.user_id = role.self_user_id
        WHERE role.tenant_id = $1
          AND role.self_user_id = $2
        "#,
    )
    .bind(refs.tenant_id.get())
    .bind(refs.user_id)
    .fetch_one(&mut *self_role_tx)
    .await
    .unwrap();
    assert!(
        sqlx::query("UPDATE roles SET deleted = CURRENT_TIMESTAMP WHERE id = $1")
            .bind(self_role_id)
            .execute(&mut *self_role_tx)
            .await
            .is_err(),
        "self roles must not be retired through direct SQL"
    );
    self_role_tx.rollback().await.unwrap();

    let mut self_assignment_tx = tenant_tx(db, refs.tenant_id).await;
    assert!(
        sqlx::query("UPDATE user_roles SET deleted = CURRENT_TIMESTAMP WHERE id = $1")
            .bind(self_user_role_id)
            .execute(&mut *self_assignment_tx)
            .await
            .is_err(),
        "self role assignments must not be retired through direct SQL"
    );
    self_assignment_tx.rollback().await.unwrap();

    let mut hierarchy_tx = tenant_tx(db, refs.tenant_id).await;
    sqlx::query("UPDATE roles SET parent_id = $1 WHERE id = $2")
        .bind(refs.role_id)
        .bind(refs.unassigned_role_id)
        .execute(&mut *hierarchy_tx)
        .await
        .unwrap();
    assert!(
        sqlx::query("UPDATE roles SET parent_id = $1 WHERE id = $2")
            .bind(refs.unassigned_role_id)
            .bind(refs.role_id)
            .execute(&mut *hierarchy_tx)
            .await
            .is_err(),
        "role hierarchy cycles must be rejected by the database"
    );
    hierarchy_tx.rollback().await.unwrap();
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

    let sequence_privileges: Vec<SequencePrivileges> = sqlx::query_as(
        r#"
        SELECT sequence_name,
               has_sequence_privilege(current_user, 'public.' || sequence_name, 'USAGE')
                   AS can_use,
               has_sequence_privilege(current_user, 'public.' || sequence_name, 'SELECT')
                   AS can_select,
               has_sequence_privilege(current_user, 'public.' || sequence_name, 'UPDATE')
                   AS can_update
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
            .map(|sequence| SequencePrivileges {
                sequence_name: (*sequence).to_owned(),
                can_use: true,
                can_select: false,
                can_update: false,
            })
            .collect::<Vec<_>>()
    );

    for function in [
        "guard_role_hierarchy()",
        "guard_self_role()",
        "guard_self_user_role()",
    ] {
        let can_execute: bool =
            sqlx::query_scalar("SELECT has_function_privilege(current_user, $1, 'EXECUTE')")
                .bind(function)
                .fetch_one(db)
                .await
                .unwrap();
        assert!(!can_execute, "{function} must not be runtime-callable");
    }
}

async fn snapshot(db: &db::Db, tenant_id: TenantId) -> String {
    let mut tx = tenant_tx(db, tenant_id).await;
    let snapshot = sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
            'roles',
                COALESCE(
                    (SELECT jsonb_agg(to_jsonb(role) ORDER BY id) FROM roles role),
                    '[]'::jsonb
                ),
            'permissions',
                COALESCE(
                    (SELECT jsonb_agg(to_jsonb(permission) ORDER BY id)
                     FROM permissions permission),
                    '[]'::jsonb
                ),
            'user_roles',
                COALESCE(
                    (SELECT jsonb_agg(to_jsonb(user_role) ORDER BY id)
                     FROM user_roles user_role),
                    '[]'::jsonb
                ),
            'role_permissions',
                COALESCE(
                    (SELECT jsonb_agg(to_jsonb(role_permission) ORDER BY id)
                     FROM role_permissions role_permission),
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
