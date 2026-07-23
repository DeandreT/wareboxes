mod common;

use common::*;
use sqlx::Acquire;

async fn insert_command_record(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    key: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO command_idempotency_records
            (tenant_id, created, operation, idempotency_key, request_hash, result_json)
        VALUES ($1, $2, 'rls.contract.v1', $3, 'request-hash', '{"ok": true}'::JSONB)
        "#,
    )
    .bind(tenant_id.get())
    .bind(db::now_iso())
    .bind(key)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

#[tokio::test]
async fn command_records_require_a_transaction_local_tenant_context() {
    let fixture = Fixture::new().await;
    let user_a = fixture.user("rls-tenant-a@test.com").await;
    let user_b = fixture.user("rls-tenant-b@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, user_a.id).await;
    let tenant_b = tenant_for_user(&fixture.db, user_b.id).await;

    db::validate_runtime_role(&fixture.db).await.unwrap();
    let admin_db = admin_db_for(&fixture.db).await;
    assert!(db::validate_runtime_role(&admin_db).await.is_err());
    db::validate_same_database(&admin_db, &fixture.db)
        .await
        .unwrap();
    let control_db = admin_db_named("postgres").await;
    assert!(db::validate_same_database(&control_db, &fixture.db)
        .await
        .is_err());
    control_db.close().await;
    let runtime_role: (String, bool, bool) = sqlx::query_as(
        r#"
        SELECT current_user, role.rolsuper, role.rolbypassrls
        FROM pg_roles role
        WHERE role.rolname = current_user
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(runtime_role, ("wareboxes_app".to_string(), false, false));

    let rls_configuration: (bool, bool, i64) = sqlx::query_as(
        r#"
        SELECT class.relrowsecurity,
               class.relforcerowsecurity,
               (SELECT COUNT(*)
                FROM pg_policy policy
                WHERE policy.polrelid = class.oid
                  AND pg_get_expr(policy.polqual, policy.polrelid) =
                      '(tenant_id = (NULLIF(current_setting(''wareboxes.tenant_id''::text, true), ''''::text))::bigint)'
                  AND pg_get_expr(policy.polwithcheck, policy.polrelid) =
                      '(tenant_id = (NULLIF(current_setting(''wareboxes.tenant_id''::text, true), ''''::text))::bigint)')
        FROM pg_class class
        WHERE class.oid = 'command_idempotency_records'::REGCLASS
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(rls_configuration, (true, true, 1));

    let runtime_privileges: (bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT has_database_privilege(current_user, current_database(), 'CREATE'),
               has_database_privilege(current_user, current_database(), 'TEMPORARY'),
               has_schema_privilege(current_user, 'public', 'CREATE')
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(runtime_privileges, (false, false, false));
    assert!(sqlx::query("SELECT version FROM _sqlx_migrations")
        .fetch_all(&fixture.db)
        .await
        .is_err());

    let privileged_session = privileged_session_as_app(&fixture.db).await;
    assert!(db::validate_runtime_role(&privileged_session)
        .await
        .is_err());
    privileged_session.close().await;

    sqlx::query("CREATE SCHEMA AUTHORIZATION wareboxes_app")
        .execute(&admin_db)
        .await
        .unwrap();
    assert!(db::validate_runtime_role(&fixture.db).await.is_err());
    sqlx::query("DROP SCHEMA wareboxes_app CASCADE")
        .execute(&admin_db)
        .await
        .unwrap();
    db::validate_runtime_role(&fixture.db).await.unwrap();

    sqlx::query("ALTER TABLE command_idempotency_records DISABLE ROW LEVEL SECURITY")
        .execute(&admin_db)
        .await
        .unwrap();
    assert!(db::validate_runtime_role(&fixture.db).await.is_err());
    sqlx::query("ALTER TABLE command_idempotency_records ENABLE ROW LEVEL SECURITY")
        .execute(&admin_db)
        .await
        .unwrap();
    db::validate_runtime_role(&fixture.db).await.unwrap();

    sqlx::query("ALTER TABLE inventory_entries DISABLE ROW LEVEL SECURITY")
        .execute(&admin_db)
        .await
        .unwrap();
    assert!(db::validate_runtime_role(&fixture.db).await.is_err());
    sqlx::query("ALTER TABLE inventory_entries ENABLE ROW LEVEL SECURITY")
        .execute(&admin_db)
        .await
        .unwrap();
    db::validate_runtime_role(&fixture.db).await.unwrap();

    sqlx::query(
        r#"
        ALTER POLICY license_plates_tenant_isolation
        ON license_plates
        USING (true)
        WITH CHECK (true)
        "#,
    )
    .execute(&admin_db)
    .await
    .unwrap();
    assert!(db::validate_runtime_role(&fixture.db).await.is_err());
    sqlx::query(
        r#"
        ALTER POLICY license_plates_tenant_isolation
        ON license_plates
        USING (
            tenant_id =
                NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
        )
        WITH CHECK (
            tenant_id =
                NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
        )
        "#,
    )
    .execute(&admin_db)
    .await
    .unwrap();
    db::validate_runtime_role(&fixture.db).await.unwrap();

    for (table_name, policy_name) in [
        ("order_activity", "order_activity_tenant_isolation"),
        ("load_activity", "load_activity_tenant_isolation"),
    ] {
        sqlx::query(&format!(
            "ALTER POLICY {policy_name} ON {table_name} \
             USING (true) WITH CHECK (true)"
        ))
        .execute(&admin_db)
        .await
        .unwrap();
        assert!(db::validate_runtime_role(&fixture.db).await.is_err());
        sqlx::query(&format!(
            "ALTER POLICY {policy_name} ON {table_name} \
             USING (tenant_id = NULLIF(\
                 current_setting('wareboxes.tenant_id', true), ''\
             )::BIGINT) \
             WITH CHECK (tenant_id = NULLIF(\
                 current_setting('wareboxes.tenant_id', true), ''\
             )::BIGINT)"
        ))
        .execute(&admin_db)
        .await
        .unwrap();
        db::validate_runtime_role(&fixture.db).await.unwrap();
    }

    let reconciliation_definition: String = sqlx::query_scalar(
        "SELECT pg_get_viewdef('public.inventory_reconciliation'::REGCLASS, true)",
    )
    .fetch_one(&admin_db)
    .await
    .unwrap();
    sqlx::query(
        r#"
        CREATE OR REPLACE VIEW public.inventory_reconciliation
        WITH (security_invoker = true)
        AS
        SELECT tenant_id, inventory_owner_id, facility_id, location_id,
               license_plate_id, item_batch_id, item_id, uom, status,
               0::BIGINT AS journal_qty,
               qty_on_hand::BIGINT AS projected_qty,
               qty_on_hand::BIGINT AS variance
        FROM inventory_balances
        WHERE deleted IS NULL
        "#,
    )
    .execute(&admin_db)
    .await
    .unwrap();
    assert!(db::validate_runtime_role(&fixture.db).await.is_err());
    sqlx::query(&format!(
        "CREATE OR REPLACE VIEW public.inventory_reconciliation \
         WITH (security_invoker = true) AS {reconciliation_definition}"
    ))
    .execute(&admin_db)
    .await
    .unwrap();
    db::validate_runtime_role(&fixture.db).await.unwrap();

    let set_role_default: String = sqlx::query_scalar(
        r#"
        SELECT format(
            'ALTER ROLE wareboxes_app IN DATABASE %I SET wareboxes.tenant_id = %L',
            current_database(),
            $1
        )
        "#,
    )
    .bind(tenant_a.to_string())
    .fetch_one(&admin_db)
    .await
    .unwrap();
    sqlx::query(&set_role_default)
        .execute(&admin_db)
        .await
        .unwrap();
    let preset_app_db = app_db_for(&fixture.db).await;
    sqlx::query("SELECT set_config('search_path', 'pg_catalog, public', false)")
        .execute(&preset_app_db)
        .await
        .unwrap();
    let preset_tenant: Option<String> =
        sqlx::query_scalar("SELECT NULLIF(current_setting('wareboxes.tenant_id', true), '')")
            .fetch_one(&preset_app_db)
            .await
            .unwrap();
    assert_eq!(preset_tenant, Some(tenant_a.to_string()));
    assert!(db::validate_runtime_role(&preset_app_db).await.is_err());
    preset_app_db.close().await;
    let reset_role_default: String = sqlx::query_scalar(
        r#"
        SELECT format(
            'ALTER ROLE wareboxes_app IN DATABASE %I RESET wareboxes.tenant_id',
            current_database()
        )
        "#,
    )
    .fetch_one(&admin_db)
    .await
    .unwrap();
    sqlx::query(&reset_role_default)
        .execute(&admin_db)
        .await
        .unwrap();

    let set_read_only_default: String = sqlx::query_scalar(
        r#"
        SELECT format(
            'ALTER ROLE wareboxes_app IN DATABASE %I SET default_transaction_read_only = on',
            current_database()
        )
        "#,
    )
    .fetch_one(&admin_db)
    .await
    .unwrap();
    sqlx::query(&set_read_only_default)
        .execute(&admin_db)
        .await
        .unwrap();
    let read_only_app_db = app_db_for(&fixture.db).await;
    sqlx::query("SELECT set_config('search_path', 'pg_catalog, public', false)")
        .execute(&read_only_app_db)
        .await
        .unwrap();
    assert!(db::validate_runtime_role(&read_only_app_db).await.is_err());
    read_only_app_db.close().await;
    let reset_read_only_default: String = sqlx::query_scalar(
        r#"
        SELECT format(
            'ALTER ROLE wareboxes_app IN DATABASE %I RESET default_transaction_read_only',
            current_database()
        )
        "#,
    )
    .fetch_one(&admin_db)
    .await
    .unwrap();
    sqlx::query(&reset_read_only_default)
        .execute(&admin_db)
        .await
        .unwrap();
    admin_db.close().await;

    let unbound_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM command_idempotency_records")
        .fetch_one(&fixture.db)
        .await
        .unwrap();
    assert_eq!(unbound_count, 0);

    let mut unbound_tx = fixture.db.begin().await.unwrap();
    assert!(insert_command_record(&mut unbound_tx, tenant_a, "unbound")
        .await
        .is_err());
    unbound_tx.rollback().await.unwrap();

    let mut tenant_a_tx = tenant_tx(&fixture.db, tenant_a).await;
    insert_command_record(&mut tenant_a_tx, tenant_a, "tenant-a")
        .await
        .unwrap();
    assert!(
        insert_command_record(&mut tenant_a_tx, tenant_b, "cross-tenant")
            .await
            .is_err()
    );
    tenant_a_tx.rollback().await.unwrap();

    let mut tenant_a_tx = tenant_tx(&fixture.db, tenant_a).await;
    insert_command_record(&mut tenant_a_tx, tenant_a, "tenant-a")
        .await
        .unwrap();
    tenant_a_tx.commit().await.unwrap();
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    insert_command_record(&mut tenant_b_tx, tenant_b, "tenant-b")
        .await
        .unwrap();
    tenant_b_tx.commit().await.unwrap();

    let mut tenant_a_tx = tenant_tx(&fixture.db, tenant_a).await;
    let tenant_a_keys: Vec<String> = sqlx::query_scalar(
        "SELECT idempotency_key FROM command_idempotency_records ORDER BY idempotency_key",
    )
    .fetch_all(&mut *tenant_a_tx)
    .await
    .unwrap();
    assert_eq!(tenant_a_keys, vec!["tenant-a"]);
    assert!(db::bind_tenant_context(&mut tenant_a_tx, tenant_b)
        .await
        .is_err());
    tenant_a_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    let tenant_b_keys: Vec<String> = sqlx::query_scalar(
        "SELECT idempotency_key FROM command_idempotency_records ORDER BY idempotency_key",
    )
    .fetch_all(&mut *tenant_b_tx)
    .await
    .unwrap();
    assert_eq!(tenant_b_keys, vec!["tenant-b"]);
    let cross_tenant_updates = sqlx::query(
        "UPDATE command_idempotency_records SET request_hash = 'changed' WHERE tenant_id = $1",
    )
    .bind(tenant_a.get())
    .execute(&mut *tenant_b_tx)
    .await
    .unwrap()
    .rows_affected();
    assert_eq!(cross_tenant_updates, 0);
    let cross_tenant_deletes =
        sqlx::query("DELETE FROM command_idempotency_records WHERE tenant_id = $1")
            .bind(tenant_a.get())
            .execute(&mut *tenant_b_tx)
            .await
            .unwrap()
            .rows_affected();
    assert_eq!(cross_tenant_deletes, 0);
    tenant_b_tx.commit().await.unwrap();

    let mut connection = fixture.db.acquire().await.unwrap();
    let mut committed_tx = connection.begin().await.unwrap();
    db::bind_tenant_context(&mut committed_tx, tenant_a)
        .await
        .unwrap();
    committed_tx.commit().await.unwrap();
    let leaked_after_commit: Option<String> =
        sqlx::query_scalar("SELECT NULLIF(current_setting('wareboxes.tenant_id', true), '')")
            .fetch_one(&mut *connection)
            .await
            .unwrap();
    assert_eq!(leaked_after_commit, None);
    let still_unbound: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM command_idempotency_records")
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert_eq!(still_unbound, 0);

    let mut rolled_back_tx = connection.begin().await.unwrap();
    db::bind_tenant_context(&mut rolled_back_tx, tenant_a)
        .await
        .unwrap();
    rolled_back_tx.rollback().await.unwrap();
    let leaked_after_rollback: Option<String> =
        sqlx::query_scalar("SELECT NULLIF(current_setting('wareboxes.tenant_id', true), '')")
            .fetch_one(&mut *connection)
            .await
            .unwrap();
    assert_eq!(leaked_after_rollback, None);

    let mut rebound_tx = connection.begin().await.unwrap();
    db::bind_tenant_context(&mut rebound_tx, tenant_b)
        .await
        .unwrap();
    let rebound_keys: Vec<String> =
        sqlx::query_scalar("SELECT idempotency_key FROM command_idempotency_records")
            .fetch_all(&mut *rebound_tx)
            .await
            .unwrap();
    assert_eq!(rebound_keys, vec!["tenant-b"]);
    rebound_tx.rollback().await.unwrap();
}
