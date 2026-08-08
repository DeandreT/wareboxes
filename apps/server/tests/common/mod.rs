#![allow(dead_code, unused_imports)]

use std::sync::atomic::{AtomicU64, Ordering};

use sqlx::postgres::PgPoolOptions;
use sqlx::Postgres;
use tokio::sync::OnceCell;
use url::Url;

pub use wareboxes_api::error::AppError;
pub use wareboxes_api::{auth, db, permissions, repo};
pub use wareboxes_application::ApplicationError;
pub use wareboxes_core::models::{
    InventoryTransactionType, LoadLineStatus, LoadStatus, LoadType, OrderStatus,
    WorkTaskProgressAction, WorkTaskStatus, WorkTaskType,
};
pub use wareboxes_domain::TenantId;

const DEFAULT_TEST_DATABASE_URL: &str =
    "postgres://wareboxes_admin:wareboxes_admin@127.0.0.1:5433/wareboxes";
const TEST_APP_ROLE: &str = "wareboxes_app";
const TEST_APP_PASSWORD: &str = "wareboxes_app";
const TEMPLATE_FINGERPRINT_LENGTH: usize = 32;
static NEXT_TEST_DB_ID: AtomicU64 = AtomicU64::new(1);
static TEST_APP_ROLE_READY: OnceCell<()> = OnceCell::const_new();

fn set_db_name(database_url: &str, db_name: &str) -> String {
    let mut parsed = Url::parse(database_url).expect("valid TEST_DATABASE_URL");
    parsed.set_path(&format!("/{db_name}"));
    parsed.to_string()
}

fn set_credentials(database_url: &str, username: &str, password: &str) -> String {
    let mut parsed = Url::parse(database_url).expect("valid TEST_DATABASE_URL");
    parsed
        .set_username(username)
        .expect("database URL accepts a username");
    parsed
        .set_password(Some(password))
        .expect("database URL accepts a password");
    parsed.to_string()
}

async fn ensure_test_app_role(admin_pool: &db::Db) {
    sqlx::query(
        r#"
        DO $$
        BEGIN
            CREATE ROLE wareboxes_app LOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT;
        EXCEPTION WHEN duplicate_object THEN
            NULL;
        END
        $$
        "#,
    )
    .execute(admin_pool)
    .await
    .unwrap();
    sqlx::query(
        "ALTER ROLE wareboxes_app LOGIN PASSWORD 'wareboxes_app' NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS",
    )
    .execute(admin_pool)
    .await
    .unwrap();
}

pub async fn setup() -> db::Db {
    let base_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_string());
    let migration_fingerprint = db::migration_fingerprint();
    let template_name = template_database_name(&migration_fingerprint);
    let process_id = std::process::id();
    let database_id = NEXT_TEST_DB_ID.fetch_add(1, Ordering::Relaxed);
    let database_name = match std::env::var("WAREBOXES_TEST_RUN_ID") {
        Ok(run_id) => {
            assert!(
                !run_id.is_empty()
                    && run_id.len() <= 16
                    && run_id.bytes().all(|byte| byte.is_ascii_digit()),
                "WAREBOXES_TEST_RUN_ID must contain at most 16 ASCII digits"
            );
            format!("wareboxes_test_{run_id}_{process_id}_{database_id}")
        }
        Err(_) => format!("wareboxes_test_{process_id}_{database_id}"),
    };

    let admin_url = set_db_name(&base_url, "postgres");
    let test_admin_url = set_db_name(&base_url, &database_name);

    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .unwrap_or_else(|e| panic!("connect admin db ({admin_url}): {e}"));

    TEST_APP_ROLE_READY
        .get_or_init(|| ensure_test_app_role(&admin_pool))
        .await;
    create_test_database(
        &admin_pool,
        &base_url,
        &migration_fingerprint,
        &template_name,
        &database_name,
    )
    .await;

    let test_app_url = set_credentials(&test_admin_url, TEST_APP_ROLE, TEST_APP_PASSWORD);
    let pool = db::connect_runtime(&test_app_url)
        .await
        .unwrap_or_else(|e| panic!("connect test database as restricted app role: {e}"));
    pool
}

pub async fn tenant_tx<'a>(db: &'a db::Db, tenant_id: TenantId) -> sqlx::Transaction<'a, Postgres> {
    let mut tx = db.begin().await.unwrap();
    db::bind_tenant_context(&mut tx, tenant_id).await.unwrap();
    tx
}

pub async fn admin_db_for(db: &db::Db) -> db::Db {
    let database_name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(db)
        .await
        .unwrap();
    admin_db_named(&database_name).await
}

pub async fn admin_db_named(database_name: &str) -> db::Db {
    let base_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_string());
    db::connect(&set_db_name(&base_url, database_name))
        .await
        .unwrap()
}

pub async fn app_db_for(db: &db::Db) -> db::Db {
    let database_name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(db)
        .await
        .unwrap();
    let base_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_string());
    let admin_url = set_db_name(&base_url, &database_name);
    db::connect(&set_credentials(
        &admin_url,
        TEST_APP_ROLE,
        TEST_APP_PASSWORD,
    ))
    .await
    .unwrap()
}

pub async fn privileged_session_as_app(db: &db::Db) -> db::Db {
    let database_name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(db)
        .await
        .unwrap();
    let base_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_string());
    let mut admin_url = Url::parse(&set_db_name(&base_url, &database_name)).unwrap();
    admin_url
        .query_pairs_mut()
        .append_pair("options", "-c role=wareboxes_app");
    db::connect(admin_url.as_str()).await.unwrap()
}

pub async fn tenant_for_user(db: &db::Db, user_id: i64) -> TenantId {
    default_tenant_for_user(db, user_id)
        .await
        .expect("registered test user has a tenant")
        .tenant_id
}

pub async fn default_tenant_for_user(
    db: &db::Db,
    user_id: i64,
) -> Option<wareboxes_core::models::TenantAccess> {
    let token = auth::create_session(db, user_id).await.unwrap();
    let access = auth::default_tenant_for_session(db, &token).await.unwrap();
    auth::destroy_session(db, &token).await.unwrap();
    access
}

pub async fn tenant_accesses_for_user(
    db: &db::Db,
    user_id: i64,
) -> Vec<wareboxes_core::models::TenantAccess> {
    let token = auth::create_session(db, user_id).await.unwrap();
    let access = auth::tenant_accesses_for_session(db, &token).await.unwrap();
    auth::destroy_session(db, &token).await.unwrap();
    access
}

fn template_database_name(migration_fingerprint: &str) -> String {
    assert!(
        migration_fingerprint.len() >= TEMPLATE_FINGERPRINT_LENGTH
            && migration_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
        "migration fingerprint must be a hexadecimal SHA-256 digest"
    );
    format!(
        "wareboxes_tpl_{}",
        &migration_fingerprint[..TEMPLATE_FINGERPRINT_LENGTH]
    )
}

fn staging_database_name(migration_fingerprint: &str) -> String {
    format!(
        "wareboxes_build_{}_{}",
        &migration_fingerprint[..TEMPLATE_FINGERPRINT_LENGTH],
        std::process::id()
    )
}

async fn create_test_database(
    admin_pool: &db::Db,
    base_url: &str,
    migration_fingerprint: &str,
    template_name: &str,
    database_name: &str,
) {
    let lock_name = format!("wareboxes:test-template:{migration_fingerprint}");
    let mut admin = admin_pool.acquire().await.unwrap();

    loop {
        if template_is_ready(&mut admin, template_name).await {
            lock_template_shared(&mut admin, &lock_name).await;
            if template_is_ready(&mut admin, template_name).await {
                clone_test_database(&mut admin, template_name, database_name).await;
                unlock_template_shared(&mut admin, &lock_name).await;
                return;
            }
            unlock_template_shared(&mut admin, &lock_name).await;
        }

        lock_template_exclusive(&mut admin, &lock_name).await;
        if template_is_ready(&mut admin, template_name).await {
            unlock_template_exclusive(&mut admin, &lock_name).await;
            continue;
        }

        build_template(&mut admin, base_url, migration_fingerprint, template_name).await;
        clone_test_database(&mut admin, template_name, database_name).await;
        unlock_template_exclusive(&mut admin, &lock_name).await;
        return;
    }
}

async fn template_is_ready(
    admin: &mut sqlx::pool::PoolConnection<Postgres>,
    template_name: &str,
) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT datistemplate AND NOT datallowconn FROM pg_database WHERE datname = $1",
    )
    .bind(template_name)
    .fetch_optional(&mut **admin)
    .await
    .unwrap_or_else(|e| panic!("inspect test template ({template_name}): {e}"))
    .unwrap_or(false)
}

async fn build_template(
    admin: &mut sqlx::pool::PoolConnection<Postgres>,
    base_url: &str,
    migration_fingerprint: &str,
    template_name: &str,
) {
    let staging_name = staging_database_name(migration_fingerprint);
    sqlx::query(&format!("DROP DATABASE IF EXISTS \"{staging_name}\""))
        .execute(&mut **admin)
        .await
        .unwrap_or_else(|e| panic!("drop stale staging template ({staging_name}): {e}"));
    sqlx::query(&format!("CREATE DATABASE \"{staging_name}\""))
        .execute(&mut **admin)
        .await
        .unwrap_or_else(|e| panic!("create staging template ({staging_name}): {e}"));

    let staging_url = set_db_name(base_url, &staging_name);
    let template_pool = db::connect(&staging_url)
        .await
        .unwrap_or_else(|e| panic!("connect staging template ({staging_url}): {e}"));
    let migration_result = db::run_migrations(&template_pool).await;
    template_pool.close().await;
    migration_result.unwrap_or_else(|e| panic!("migrate staging template ({staging_name}): {e}"));

    sqlx::query(&format!(
        "ALTER DATABASE \"{staging_name}\" WITH IS_TEMPLATE true ALLOW_CONNECTIONS false"
    ))
    .execute(&mut **admin)
    .await
    .unwrap_or_else(|e| panic!("seal staging template ({staging_name}): {e}"));
    sqlx::query(&format!(
        "ALTER DATABASE \"{staging_name}\" RENAME TO \"{template_name}\""
    ))
    .execute(&mut **admin)
    .await
    .unwrap_or_else(|e| panic!("publish test template ({template_name}): {e}"));
}

async fn clone_test_database(
    admin: &mut sqlx::pool::PoolConnection<Postgres>,
    template_name: &str,
    database_name: &str,
) {
    sqlx::query(&format!("DROP DATABASE IF EXISTS \"{database_name}\""))
        .execute(&mut **admin)
        .await
        .unwrap_or_else(|e| panic!("drop test db ({database_name}): {e}"));
    sqlx::query(&format!(
        "CREATE DATABASE \"{database_name}\" TEMPLATE \"{template_name}\""
    ))
    .execute(&mut **admin)
    .await
    .unwrap_or_else(|e| panic!("create test db ({database_name}): {e}"));
    sqlx::query(&format!(
        "REVOKE TEMPORARY ON DATABASE \"{database_name}\" FROM PUBLIC"
    ))
    .execute(&mut **admin)
    .await
    .unwrap_or_else(|e| panic!("restrict temporary tables in test db ({database_name}): {e}"));
}

async fn lock_template_shared(admin: &mut sqlx::pool::PoolConnection<Postgres>, lock_name: &str) {
    sqlx::query("SELECT pg_advisory_lock_shared(hashtextextended($1, 0))")
        .bind(lock_name)
        .execute(&mut **admin)
        .await
        .unwrap_or_else(|e| panic!("acquire shared test template lock: {e}"));
}

async fn unlock_template_shared(admin: &mut sqlx::pool::PoolConnection<Postgres>, lock_name: &str) {
    let unlocked =
        sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock_shared(hashtextextended($1, 0))")
            .bind(lock_name)
            .fetch_one(&mut **admin)
            .await
            .unwrap_or_else(|e| panic!("release shared test template lock: {e}"));
    assert!(unlocked, "shared test template lock was not held");
}

async fn lock_template_exclusive(
    admin: &mut sqlx::pool::PoolConnection<Postgres>,
    lock_name: &str,
) {
    sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
        .bind(lock_name)
        .execute(&mut **admin)
        .await
        .unwrap_or_else(|e| panic!("acquire exclusive test template lock: {e}"));
}

async fn unlock_template_exclusive(
    admin: &mut sqlx::pool::PoolConnection<Postgres>,
    lock_name: &str,
) {
    let unlocked =
        sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
            .bind(lock_name)
            .fetch_one(&mut **admin)
            .await
            .unwrap_or_else(|e| panic!("release exclusive test template lock: {e}"));
    assert!(unlocked, "exclusive test template lock was not held");
}

pub async fn insert_test_order_header(
    db: &db::Db,
    tenant_id: TenantId,
    key: &str,
    inventory_owner_id: i64,
) -> i64 {
    let mut tx = tenant_tx(db, tenant_id).await;
    let address_id = repo::address::insert_address_tx(
        &mut tx,
        tenant_id,
        repo::address::NewAddress {
            name: Some("Test Recipient"),
            company: None,
            line1: "1 Main St",
            line2: None,
            city: Some("Reno"),
            state: Some("NV"),
            postal_code: Some("89501"),
            country: "US",
            phone: None,
            email: None,
        },
    )
    .await
    .unwrap();
    let created = db::now_iso();
    let order_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO orders
            (tenant_id, inventory_owner_id, order_key, created, rush, status, address_id)
        VALUES ($1, $2, $3, $4, false, 'open', $5)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(key)
    .bind(created)
    .bind(address_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO order_activity
            (tenant_id, inventory_owner_id, created, order_id, action)
        VALUES ($1, $2, $3, $4, 'created order')
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(created)
    .bind(order_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    order_id
}

pub struct Fixture {
    pub db: db::Db,
}

#[derive(Debug, Clone, Copy)]
pub struct ReceivedBalanceSetup<'a> {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub item_id: i64,
    pub qty: i64,
    pub key: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct ReceivedBalance {
    pub balance_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub location_id: i64,
    pub item_batch_id: i64,
    pub item_id: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct AllocatedReservation {
    pub reservation_id: i64,
    pub allocation_id: i64,
}

impl Fixture {
    pub async fn new() -> Self {
        Self { db: setup().await }
    }

    pub async fn user(&self, email: &str) -> wareboxes_core::models::User {
        auth::register_user(&self.db, email, "supersecret", None, None)
            .await
            .unwrap()
    }

    pub async fn wms_user(&self, email: &str) -> wareboxes_core::models::User {
        let user = self.user(email).await;
        let tenant_id = tenant_for_user(&self.db, user.id).await;
        let perm = wareboxes_persistence_postgres::permissions::add_permission(
            &self.db,
            tenant_id,
            "wms",
            Some("WMS"),
        )
        .await
        .unwrap();
        let role = wareboxes_persistence_postgres::roles::add_role(
            &self.db,
            tenant_id,
            &format!("{email}-wms"),
            Some("WMS worker"),
        )
        .await
        .unwrap();
        wareboxes_persistence_postgres::roles::add_role_permission(&self.db, tenant_id, role, perm)
            .await
            .unwrap();
        wareboxes_persistence_postgres::roles::add_role_to_user(&self.db, tenant_id, user.id, role)
            .await
            .unwrap();
        user
    }

    pub async fn inventory_owner(&self, tenant_id: TenantId, name: &str) -> i64 {
        repo::inventory_owners::add_inventory_owner(
            &self.db,
            tenant_id,
            name,
            &format!("{name}@test.local"),
        )
        .await
        .unwrap()
    }

    pub async fn facility(&self, tenant_id: TenantId, name: &str) -> i64 {
        wareboxes_persistence_postgres::facilities::add_facility(&self.db, tenant_id, name)
            .await
            .unwrap()
    }

    pub async fn assign_owner_to_facility(
        &self,
        tenant_id: TenantId,
        inventory_owner_id: i64,
        facility_id: i64,
    ) {
        let admin_db = admin_db_for(&self.db).await;
        sqlx::query(
            r#"
            INSERT INTO inventory_owner_facilities
                (tenant_id, created, inventory_owner_id, facility_id)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(tenant_id.get())
        .bind(db::now_iso())
        .bind(inventory_owner_id)
        .bind(facility_id)
        .execute(&admin_db)
        .await
        .unwrap();
        admin_db.close().await;
    }

    pub async fn location(&self, tenant_id: TenantId, facility_id: i64, scan_code: &str) -> i64 {
        wareboxes_persistence_postgres::locations::add_location(
            &self.db,
            tenant_id,
            facility_id,
            None,
            Some(scan_code),
            Some(scan_code),
            "bin",
            true,
            true,
            false,
        )
        .await
        .unwrap()
    }

    pub async fn item(&self, tenant_id: TenantId, name: &str, packaging_unit: &str) -> i64 {
        repo::items::add_item(
            &self.db,
            tenant_id,
            name,
            None,
            packaging_unit,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap()
    }

    pub async fn order_header(
        &self,
        tenant_id: TenantId,
        key: &str,
        inventory_owner_id: i64,
    ) -> i64 {
        insert_test_order_header(&self.db, tenant_id, key, inventory_owner_id).await
    }

    pub async fn order_item(
        &self,
        tenant_id: TenantId,
        order_id: i64,
        item_id: i64,
        qty: i64,
    ) -> i64 {
        let mut tx = tenant_tx(&self.db, tenant_id).await;
        sqlx::query("SELECT id FROM orders WHERE tenant_id = $1 AND id = $2 FOR UPDATE")
            .bind(tenant_id.get())
            .bind(order_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            r#"
            INSERT INTO inventory_owner_items
                (tenant_id, created, inventory_owner_id, item_id)
            SELECT orders.tenant_id, $1, orders.inventory_owner_id, $2
            FROM orders
            WHERE orders.tenant_id = $3 AND orders.id = $4
            ON CONFLICT (tenant_id, inventory_owner_id, item_id) DO UPDATE
            SET deleted = NULL
            "#,
        )
        .bind(db::now_iso())
        .bind(item_id)
        .bind(tenant_id.get())
        .bind(order_id)
        .execute(&mut *tx)
        .await
        .unwrap();
        let id = sqlx::query_scalar(
            r#"
            WITH next_line AS (
                SELECT COALESCE(MAX(line_number), 0) + 1 AS line_number
                FROM order_items
                WHERE tenant_id = $4 AND order_id = $5
            )
            INSERT INTO order_items
                (tenant_id, inventory_owner_id, created, line_key, line_number,
                 qty, item_id, order_id, uom)
            SELECT orders.tenant_id, orders.inventory_owner_id, $1,
                   'fixture-' || next_line.line_number::TEXT, next_line.line_number,
                   $2, $3, orders.id, items.packaging_unit
            FROM orders
            INNER JOIN items
                ON items.tenant_id = orders.tenant_id
               AND items.id = $3
            CROSS JOIN next_line
            WHERE orders.tenant_id = $4 AND orders.id = $5
            RETURNING id
            "#,
        )
        .bind(db::now_iso())
        .bind(qty)
        .bind(item_id)
        .bind(tenant_id.get())
        .bind(order_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        id
    }

    pub async fn received_balance(
        &self,
        access: &wareboxes_core::models::TenantAccess,
        setup: ReceivedBalanceSetup<'_>,
    ) -> ReceivedBalance {
        let location_id = self
            .location(access.tenant_id, setup.facility_id, setup.key)
            .await;
        let item_batch_id = repo::inventory::add_item_batch(
            &self.db,
            access.tenant_id,
            setup.inventory_owner_id,
            setup.item_id,
            None,
            Some(setup.key),
            None,
            None,
        )
        .await
        .unwrap();
        repo::inventory::receive_inventory(
            &self.db,
            access.tenant_id,
            access.user_id.get(),
            item_batch_id,
            location_id,
            setup.qty,
            None,
            Some("received balance fixture"),
            None,
            None,
            &format!("{}-receipt", setup.key),
        )
        .await
        .unwrap();
        let mut tx = tenant_tx(&self.db, access.tenant_id).await;
        let balance_id = sqlx::query_scalar(
            r#"
            SELECT id
            FROM inventory_balances
            WHERE tenant_id = $1
              AND inventory_owner_id = $2
              AND location_id = $3
              AND item_batch_id = $4
              AND deleted IS NULL
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(setup.inventory_owner_id)
        .bind(location_id)
        .bind(item_batch_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        ReceivedBalance {
            balance_id,
            inventory_owner_id: setup.inventory_owner_id,
            facility_id: setup.facility_id,
            location_id,
            item_batch_id,
            item_id: setup.item_id,
        }
    }

    pub async fn reservation_for_balance(
        &self,
        tenant_id: TenantId,
        user_id: i64,
        order_id: i64,
        inventory_balance_id: i64,
        qty: i64,
        idempotency_key: &str,
    ) -> i64 {
        let mut tx = tenant_tx(&self.db, tenant_id).await;
        let (facility_id, item_id): (i64, i64) = sqlx::query_as(
            r#"
            SELECT facility_id, item_id
            FROM inventory_balances
            WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL
            "#,
        )
        .bind(tenant_id.get())
        .bind(inventory_balance_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        let order_item_id: Option<i64> = sqlx::query_scalar(
            r#"
            SELECT id
            FROM order_items
            WHERE tenant_id = $1
              AND order_id = $2
              AND item_id = $3
              AND deleted IS NULL
            ORDER BY id
            LIMIT 1
            "#,
        )
        .bind(tenant_id.get())
        .bind(order_id)
        .bind(item_id)
        .fetch_optional(&mut *tx)
        .await
        .unwrap()
        .flatten();
        tx.rollback().await.unwrap();
        let order_item_id = match order_item_id {
            Some(id) => id,
            None => self.order_item(tenant_id, order_id, item_id, qty).await,
        };
        let access = default_tenant_for_user(&self.db, user_id)
            .await
            .expect("allocation actor has tenant access");
        let reservation = repo::inventory::create_inventory_reservation(
            &self.db,
            &access,
            &repo::inventory::CreateInventoryReservationCommand {
                order_id,
                order_item_id,
                facility_id,
                qty,
                idempotency_key: &format!("{idempotency_key}-reservation"),
            },
        )
        .await
        .unwrap();
        reservation.reservation_id
    }

    pub async fn allocated_reservation(
        &self,
        tenant_id: TenantId,
        user_id: i64,
        order_id: i64,
        inventory_balance_id: i64,
        qty: i64,
        idempotency_key: &str,
    ) -> AllocatedReservation {
        let reservation_id = self
            .reservation_for_balance(
                tenant_id,
                user_id,
                order_id,
                inventory_balance_id,
                qty,
                &format!("{idempotency_key}-reservation"),
            )
            .await;
        let access = default_tenant_for_user(&self.db, user_id)
            .await
            .expect("allocation actor has tenant access");
        let allocation = repo::inventory::allocate_inventory(
            &self.db,
            &access,
            &repo::inventory::AllocateInventoryCommand {
                reservation_id,
                inventory_balance_id,
                qty,
                idempotency_key: &format!("{idempotency_key}-allocation"),
            },
        )
        .await
        .unwrap();
        AllocatedReservation {
            reservation_id,
            allocation_id: allocation.allocation_id,
        }
    }
}
