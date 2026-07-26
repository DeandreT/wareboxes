//! Database access layer.
//!
//! We currently target PostgreSQL in production and test.
//!
//! The repository layer uses PostgreSQL connections and row types directly.

use anyhow::Context;
use sqlx::postgres::{PgConnection, PgPoolOptions};
use sqlx::{PgPool, Postgres, Transaction};
use wareboxes_core::models::Timestamp;
use wareboxes_domain::TenantId;

use crate::error::{AppError, AppResult};

pub type Db = PgPool;

#[derive(sqlx::FromRow)]
struct RuntimeRole {
    name: String,
    session_name: String,
    can_login: bool,
    is_superuser: bool,
    inherits_roles: bool,
    can_create_roles: bool,
    can_create_databases: bool,
    can_replicate: bool,
    bypasses_rls: bool,
    owns_database: bool,
    owns_non_system_objects: bool,
    has_database_create: bool,
    has_database_temporary: bool,
    has_non_system_schema_create: bool,
    has_role_memberships: bool,
    tenant_policy_contract_valid: bool,
    reconciliation_view_contract_valid: bool,
    preset_tenant_id: Option<String>,
    search_path: String,
    in_recovery: bool,
    transaction_read_only: bool,
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct DatabaseIdentity {
    database_name: String,
    database_oid: i64,
    system_identifier: String,
}

pub fn now_iso() -> Timestamp {
    chrono::Utc::now()
}

static PG_MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/postgres");

pub async fn connect(database_url: &str) -> anyhow::Result<Db> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .after_connect(|connection, _metadata| {
            Box::pin(async move { configure_connection(connection, "public, pg_catalog").await })
        })
        .connect(database_url)
        .await
        .context("connecting to PostgreSQL")?;
    Ok(pool)
}

pub async fn connect_runtime(database_url: &str) -> anyhow::Result<Db> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .after_connect(|connection, _metadata| {
            Box::pin(async move {
                configure_connection(connection, "pg_catalog, public").await?;
                validate_runtime_connection(connection)
                    .await
                    .map_err(|error| {
                        sqlx::Error::Configuration(std::io::Error::other(error.to_string()).into())
                    })
            })
        })
        .connect(database_url)
        .await
        .context("connecting to PostgreSQL with the restricted runtime role")?;
    Ok(pool)
}

async fn configure_connection(
    connection: &mut PgConnection,
    search_path: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('search_path', $1, false)")
        .bind(search_path)
        .execute(connection)
        .await?;
    Ok(())
}

pub async fn run_migrations(pool: &Db) -> anyhow::Result<()> {
    PG_MIGRATIONS.run(pool).await?;
    Ok(())
}

pub async fn bind_tenant_context(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
) -> AppResult<()> {
    let tenant_id = tenant_id.to_string();
    let current: Option<String> =
        sqlx::query_scalar("SELECT NULLIF(current_setting('wareboxes.tenant_id', true), '')")
            .fetch_one(&mut **tx)
            .await?;
    match current.as_deref() {
        None => {
            sqlx::query_scalar::<_, String>("SELECT set_config('wareboxes.tenant_id', $1, true)")
                .bind(&tenant_id)
                .fetch_one(&mut **tx)
                .await?;
            Ok(())
        }
        Some(current) if current == tenant_id => Ok(()),
        Some(_) => Err(AppError::forbidden()),
    }
}

pub async fn begin_tenant_transaction(
    db: &Db,
    tenant_id: TenantId,
) -> AppResult<Transaction<'_, Postgres>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    Ok(tx)
}

pub async fn validate_runtime_role(pool: &Db) -> anyhow::Result<()> {
    let mut connection = pool.acquire().await?;
    validate_runtime_connection(&mut connection).await
}

async fn validate_runtime_connection(connection: &mut PgConnection) -> anyhow::Result<()> {
    let role: RuntimeRole = sqlx::query_as(
        r#"
        WITH tenant_table AS (
            SELECT tenant_class.oid,
                   tenant_class.relname AS table_name,
                   tenant_class.relrowsecurity,
                   tenant_class.relforcerowsecurity
            FROM pg_class tenant_class
            JOIN pg_namespace tenant_namespace
              ON tenant_namespace.oid = tenant_class.relnamespace
            JOIN pg_attribute tenant_column
              ON tenant_column.attrelid = tenant_class.oid
             AND tenant_column.attname = 'tenant_id'
             AND tenant_column.attnum > 0
             AND NOT tenant_column.attisdropped
            WHERE tenant_namespace.nspname = 'public'
              AND tenant_class.relkind IN ('r', 'p')
        ),
        tenant_policy_debt(table_name) AS (
            VALUES
                ('audit_location_counts'),
                ('audit_wave_assignments'),
                ('audit_wave_inventory_owners'),
                ('audit_wave_items'),
                ('audit_wave_locations'),
                ('audit_waves'),
                ('employee_facilities'),
                ('employees'),
                ('load_files'),
                ('load_lines'),
                ('load_notes'),
                ('load_orders'),
                ('loads'),
                ('permissions'),
                ('pick_waves'),
                ('role_permissions'),
                ('roles'),
                ('tenant_memberships'),
                ('user_facilities'),
                ('user_inventory_owners'),
                ('user_roles')
        ),
        expected_policy(table_name, policy_name) AS (
            VALUES
                (
                    'facilities',
                    'facilities_tenant_isolation'
                ),
                (
                    'locations',
                    'locations_tenant_isolation'
                ),
                (
                    'inventory_owners',
                    'inventory_owners_tenant_isolation'
                ),
                (
                    'inventory_owner_facilities',
                    'inventory_owner_facilities_tenant_isolation'
                ),
                (
                    'dims',
                    'dims_tenant_isolation'
                ),
                (
                    'items',
                    'items_tenant_isolation'
                ),
                (
                    'skus',
                    'skus_tenant_isolation'
                ),
                (
                    'barcodes',
                    'barcodes_tenant_isolation'
                ),
                (
                    'item_pack_links',
                    'item_pack_links_tenant_isolation'
                ),
                (
                    'inventory_owner_items',
                    'inventory_owner_items_tenant_isolation'
                ),
                (
                    'item_batches',
                    'item_batches_tenant_isolation'
                ),
                (
                    'command_idempotency_records',
                    'command_idempotency_records_tenant_isolation'
                ),
                (
                    'integration_inbox_receipts',
                    'integration_inbox_receipts_tenant_isolation'
                ),
                (
                    'integration_inbox_keys',
                    'integration_inbox_keys_tenant_isolation'
                ),
                (
                    'inventory_transactions',
                    'inventory_transactions_tenant_isolation'
                ),
                (
                    'inventory_entries',
                    'inventory_entries_tenant_isolation'
                ),
                (
                    'license_plates',
                    'license_plates_tenant_isolation'
                ),
                (
                    'order_activity',
                    'order_activity_tenant_isolation'
                ),
                (
                    'load_activity',
                    'load_activity_tenant_isolation'
                ),
                (
                    'work_tasks',
                    'work_tasks_tenant_isolation'
                ),
                (
                    'work_task_progress',
                    'work_task_progress_tenant_isolation'
                ),
                (
                    'cycle_count_item_location_tasks',
                    'cycle_count_item_location_tasks_tenant_isolation'
                ),
                (
                    'cycle_count_item_location_results',
                    'cycle_count_item_location_results_tenant_isolation'
                ),
                (
                    'cycle_count_location_tasks',
                    'cycle_count_location_tasks_tenant_isolation'
                ),
                (
                    'break_master_pack_tasks',
                    'break_master_pack_tasks_tenant_isolation'
                ),
                (
                    'unpack_cancelled_order_tasks',
                    'unpack_cancelled_order_tasks_tenant_isolation'
                ),
                (
                    'unpack_cancelled_order_task_lines',
                    'unpack_cancelled_order_task_lines_tenant_isolation'
                ),
                (
                    'inventory_reservations',
                    'inventory_reservations_tenant_isolation'
                ),
                (
                    'order_tracking_numbers',
                    'order_tracking_numbers_tenant_isolation'
                ),
                (
                    'inventory_balances',
                    'inventory_balances_tenant_isolation'
                ),
                (
                    'addresses',
                    'addresses_tenant_isolation'
                ),
                (
                    'orders',
                    'orders_tenant_isolation'
                ),
                (
                    'order_items',
                    'order_items_tenant_isolation'
                ),
                (
                    'outbox_event_keys',
                    'outbox_event_keys_tenant_isolation'
                ),
                (
                    'outbox_aggregate_sequences',
                    'outbox_aggregate_sequences_tenant_isolation'
                ),
                (
                    'outbox_events',
                    'outbox_events_tenant_isolation'
                ),
                (
                    'outbox_delivery_attempts',
                    'outbox_delivery_attempts_tenant_isolation'
                ),
                (
                    'outbox_delivery_attempt_results',
                    'outbox_delivery_attempt_results_tenant_isolation'
                )
        ),
        tenant_table_classification AS (
            SELECT
                (SELECT COUNT(*) FROM tenant_table) =
                    (SELECT COUNT(*) FROM expected_policy) +
                    (SELECT COUNT(*) FROM tenant_policy_debt)
                AND NOT EXISTS (
                    SELECT 1
                    FROM expected_policy expected
                    JOIN tenant_policy_debt debt
                      ON debt.table_name = expected.table_name
                )
                AND NOT EXISTS (
                    SELECT 1
                    FROM expected_policy expected
                    LEFT JOIN tenant_table tenant
                      ON tenant.table_name = expected.table_name
                    WHERE tenant.oid IS NULL
                )
                AND NOT EXISTS (
                    SELECT 1
                    FROM tenant_policy_debt debt
                    LEFT JOIN tenant_table tenant
                      ON tenant.table_name = debt.table_name
                    WHERE tenant.oid IS NULL
                       OR tenant.relrowsecurity
                       OR tenant.relforcerowsecurity
                       OR EXISTS (
                           SELECT 1
                           FROM pg_policy policy
                           WHERE policy.polrelid = tenant.oid
                       )
                )
                AND NOT EXISTS (
                    SELECT 1
                    FROM tenant_table tenant
                    WHERE NOT EXISTS (
                              SELECT 1
                              FROM expected_policy expected
                              WHERE expected.table_name = tenant.table_name
                          )
                      AND NOT EXISTS (
                              SELECT 1
                              FROM tenant_policy_debt debt
                              WHERE debt.table_name = tenant.table_name
                          )
                ) AS valid
        )
        SELECT role.rolname AS name,
               session_user::TEXT AS session_name,
               role.rolcanlogin AS can_login,
               role.rolsuper AS is_superuser,
               role.rolinherit AS inherits_roles,
               role.rolcreaterole AS can_create_roles,
               role.rolcreatedb AS can_create_databases,
               role.rolreplication AS can_replicate,
               role.rolbypassrls AS bypasses_rls,
               database.datdba = role.oid AS owns_database,
               EXISTS (
                   SELECT 1
                   FROM pg_namespace owned_namespace
                   WHERE owned_namespace.nspowner = role.oid
                     AND owned_namespace.nspname <> 'information_schema'
                     AND owned_namespace.nspname !~ '^pg_'
               ) OR EXISTS (
                   SELECT 1
                   FROM pg_class owned_class
                   JOIN pg_namespace owned_class_namespace
                     ON owned_class_namespace.oid = owned_class.relnamespace
                   WHERE owned_class.relowner = role.oid
                     AND owned_class_namespace.nspname <> 'information_schema'
                     AND owned_class_namespace.nspname !~ '^pg_'
               ) OR EXISTS (
                   SELECT 1
                   FROM pg_proc owned_function
                   JOIN pg_namespace owned_function_namespace
                     ON owned_function_namespace.oid = owned_function.pronamespace
                   WHERE owned_function.proowner = role.oid
                     AND owned_function_namespace.nspname <> 'information_schema'
                     AND owned_function_namespace.nspname !~ '^pg_'
               ) OR EXISTS (
                   SELECT 1
                   FROM pg_type owned_type
                   JOIN pg_namespace owned_type_namespace
                     ON owned_type_namespace.oid = owned_type.typnamespace
                   WHERE owned_type.typowner = role.oid
                     AND owned_type_namespace.nspname <> 'information_schema'
                     AND owned_type_namespace.nspname !~ '^pg_'
               ) AS owns_non_system_objects,
               has_database_privilege(role.oid, database.oid, 'CREATE')
                   AS has_database_create,
               has_database_privilege(role.oid, database.oid, 'TEMPORARY')
                   AS has_database_temporary,
               EXISTS (
                   SELECT 1
                   FROM pg_namespace creatable_namespace
                   WHERE creatable_namespace.nspname <> 'information_schema'
                     AND creatable_namespace.nspname !~ '^pg_'
                     AND has_schema_privilege(
                         role.oid,
                         creatable_namespace.oid,
                         'CREATE'
                     )
               ) AS has_non_system_schema_create,
               EXISTS (
                   SELECT 1 FROM pg_auth_members membership
                   WHERE membership.member = role.oid
               ) AS has_role_memberships,
               (
                   SELECT COUNT(*) > 0
                      AND COUNT(*) = (SELECT COUNT(*) FROM expected_policy)
                      AND (SELECT valid FROM tenant_table_classification)
                   FROM expected_policy expected
                   JOIN pg_namespace policy_namespace
                     ON policy_namespace.nspname = 'public'
                   JOIN pg_class protected_table
                     ON protected_table.relnamespace = policy_namespace.oid
                    AND protected_table.relname = expected.table_name
                   WHERE protected_table.relrowsecurity
                     AND protected_table.relforcerowsecurity
                     AND (
                         SELECT COUNT(*)
                         FROM pg_policy policy
                         WHERE policy.polrelid = protected_table.oid
                     ) = 1
                     AND EXISTS (
                         SELECT 1
                         FROM pg_policy policy
                         WHERE policy.polrelid = protected_table.oid
                           AND policy.polname = expected.policy_name
                           AND policy.polcmd = '*'
                           AND policy.polpermissive
                           AND policy.polroles = ARRAY[0::OID]
                           AND pg_get_expr(policy.polqual, policy.polrelid) =
                               '(tenant_id = (NULLIF(current_setting(''wareboxes.tenant_id''::text, true), ''''::text))::bigint)'
                           AND pg_get_expr(policy.polwithcheck, policy.polrelid) =
                               '(tenant_id = (NULLIF(current_setting(''wareboxes.tenant_id''::text, true), ''''::text))::bigint)'
                     )
               ) AS tenant_policy_contract_valid,
               EXISTS (
                   SELECT 1
                   FROM pg_class reconciliation_view
                   JOIN pg_namespace reconciliation_namespace
                     ON reconciliation_namespace.oid =
                        reconciliation_view.relnamespace
                   WHERE reconciliation_namespace.nspname = 'public'
                     AND reconciliation_view.relname = 'inventory_reconciliation'
                     AND reconciliation_view.relkind = 'v'
                     AND COALESCE(reconciliation_view.reloptions, ARRAY[]::TEXT[])
                         @> ARRAY['security_invoker=true']
                     AND obj_description(reconciliation_view.oid, 'pg_class') =
                         'wareboxes.tenant_contract.md5=' || md5(
                             pg_get_viewdef(reconciliation_view.oid, true)
                         )
               ) AS reconciliation_view_contract_valid,
               NULLIF(current_setting('wareboxes.tenant_id', true), '')
                   AS preset_tenant_id,
               current_setting('search_path') AS search_path,
               pg_is_in_recovery() AS in_recovery,
               current_setting('transaction_read_only')::BOOLEAN
                   AS transaction_read_only
        FROM pg_roles role
        JOIN pg_database database ON database.datname = current_database()
        WHERE role.rolname = current_user
        "#,
    )
    .fetch_one(&mut *connection)
    .await?;
    if role.session_name != role.name
        || !role.can_login
        || role.is_superuser
        || role.inherits_roles
        || role.can_create_roles
        || role.can_create_databases
        || role.can_replicate
        || role.bypasses_rls
        || role.owns_database
        || role.owns_non_system_objects
        || role.has_database_create
        || role.has_database_temporary
        || role.has_non_system_schema_create
        || role.has_role_memberships
        || !role.tenant_policy_contract_valid
        || !role.reconciliation_view_contract_valid
        || role.preset_tenant_id.is_some()
        || role.search_path != "pg_catalog, public"
        || role.in_recovery
        || role.transaction_read_only
    {
        anyhow::bail!(
            "runtime database role {} or tenant-isolation canary is not safely configured",
            role.name
        );
    }
    Ok(())
}

async fn database_identity(pool: &Db) -> anyhow::Result<DatabaseIdentity> {
    sqlx::query_as(
        r#"
        SELECT database.datname AS database_name,
               database.oid::BIGINT AS database_oid,
               control.system_identifier::TEXT AS system_identifier
        FROM pg_database database
        CROSS JOIN pg_control_system() control
        WHERE database.datname = current_database()
        "#,
    )
    .fetch_one(pool)
    .await
    .context("reading database identity")
}

pub async fn validate_same_database(migration_pool: &Db, runtime_pool: &Db) -> anyhow::Result<()> {
    let migration = database_identity(migration_pool).await?;
    let runtime = database_identity(runtime_pool).await?;
    if migration != runtime {
        anyhow::bail!(
            "migration and runtime database connections resolve to different PostgreSQL databases"
        );
    }
    Ok(())
}
