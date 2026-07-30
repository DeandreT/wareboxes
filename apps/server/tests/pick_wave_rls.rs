mod common;

use common::*;

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct TablePrivileges {
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
    can_use: bool,
    can_select: bool,
    can_update: bool,
}

#[tokio::test]
async fn pick_waves_remain_inaccessible_until_a_typed_workflow_exists() {
    let fixture = Fixture::new().await;
    let table_privileges: TablePrivileges = sqlx::query_as(
        r#"
        SELECT has_table_privilege(current_user, 'public.pick_waves', 'SELECT')
                   AS can_select,
               has_table_privilege(current_user, 'public.pick_waves', 'INSERT')
                   AS can_insert,
               has_table_privilege(current_user, 'public.pick_waves', 'UPDATE')
                   AS can_update,
               has_table_privilege(current_user, 'public.pick_waves', 'DELETE')
                   AS can_delete,
               has_table_privilege(current_user, 'public.pick_waves', 'TRUNCATE')
                   AS can_truncate,
               has_table_privilege(current_user, 'public.pick_waves', 'REFERENCES')
                   AS can_reference,
               has_table_privilege(current_user, 'public.pick_waves', 'TRIGGER')
                   AS can_trigger
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(
        table_privileges,
        TablePrivileges {
            can_select: false,
            can_insert: false,
            can_update: false,
            can_delete: false,
            can_truncate: false,
            can_reference: false,
            can_trigger: false,
        }
    );

    let sequence_privileges: SequencePrivileges = sqlx::query_as(
        r#"
        SELECT has_sequence_privilege(current_user, 'public.pick_waves_id_seq', 'USAGE')
                   AS can_use,
               has_sequence_privilege(current_user, 'public.pick_waves_id_seq', 'SELECT')
                   AS can_select,
               has_sequence_privilege(current_user, 'public.pick_waves_id_seq', 'UPDATE')
                   AS can_update
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(
        sequence_privileges,
        SequencePrivileges {
            can_use: false,
            can_select: false,
            can_update: false,
        }
    );

    let user_a = fixture.user("pick-wave-rls-a@test.com").await;
    let user_b = fixture.user("pick-wave-rls-b@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, user_a.id).await;
    let tenant_b = tenant_for_user(&fixture.db, user_b.id).await;
    let admin_db = admin_db_for(&fixture.db).await;
    for (tenant_id, name) in [(tenant_a, "PICK-RLS-A"), (tenant_b, "PICK-RLS-B")] {
        sqlx::query("INSERT INTO pick_waves (tenant_id, created, name) VALUES ($1, $2, $3)")
            .bind(tenant_id.get())
            .bind(db::now_iso())
            .bind(name)
            .execute(&admin_db)
            .await
            .unwrap();
    }
    admin_db.close().await;

    for context in [None, Some(tenant_a), Some(tenant_b)] {
        let mut tx = fixture.db.begin().await.unwrap();
        if let Some(tenant_id) = context {
            db::bind_tenant_context(&mut tx, tenant_id).await.unwrap();
        }
        let error = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pick_waves")
            .fetch_one(&mut *tx)
            .await
            .unwrap_err();
        assert_sqlstate(error, "42501");
        tx.rollback().await.unwrap();
    }

    let mut tx = tenant_tx(&fixture.db, tenant_a).await;
    for statement in [
        "INSERT INTO pick_waves (tenant_id, created, name) VALUES ($1, CURRENT_TIMESTAMP, 'FORGED')",
        "UPDATE pick_waves SET name = name WHERE tenant_id = $1",
        "DELETE FROM pick_waves WHERE tenant_id = $1",
    ] {
        let error = sqlx::query(statement)
            .bind(tenant_a.get())
            .execute(&mut *tx)
            .await
            .unwrap_err();
        assert_sqlstate(error, "42501");
        tx.rollback().await.unwrap();
        tx = tenant_tx(&fixture.db, tenant_a).await;
    }
    tx.rollback().await.unwrap();
}

fn assert_sqlstate(error: sqlx::Error, expected: &str) {
    let code = error.as_database_error().and_then(|error| error.code());
    assert_eq!(code.as_deref(), Some(expected));
}
