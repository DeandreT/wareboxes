#![allow(dead_code, unused_imports)]

use std::sync::atomic::{AtomicU64, Ordering};

use sqlx::postgres::PgPoolOptions;
use sqlx::Postgres;
use tokio::sync::OnceCell;
use url::Url;

pub use wareboxes_core::dto::{NewOrder, OrderUpdate};
pub use wareboxes_core::models::{
    InventoryTransactionType, LoadLineStatus, LoadStatus, LoadType, OrderStatus,
    WorkTaskProgressAction, WorkTaskStatus, WorkTaskType,
};
pub use wareboxes_core::CoreError;
pub use wareboxes_domain::TenantId;
pub use wareboxes_server::error::AppError;
pub use wareboxes_server::{auth, db, permissions, repo};

const DEFAULT_TEST_DATABASE_URL: &str =
    "postgres://wareboxes_admin:wareboxes_admin@127.0.0.1:5433/wareboxes";
const TEST_APP_ROLE: &str = "wareboxes_app";
const TEST_APP_PASSWORD: &str = "wareboxes_app";
static NEXT_TEST_DB_ID: AtomicU64 = AtomicU64::new(1);
static TEMPLATE_DB_NAME: OnceCell<String> = OnceCell::const_new();

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
    let template_name = template_database(&base_url).await;
    let database_name = format!(
        "wareboxes_test_{}_{}",
        std::process::id(),
        NEXT_TEST_DB_ID.fetch_add(1, Ordering::Relaxed)
    );

    let admin_url = set_db_name(&base_url, "postgres");
    let test_admin_url = set_db_name(&base_url, &database_name);

    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .unwrap_or_else(|e| panic!("connect admin db ({admin_url}): {e}"));

    sqlx::query(&format!("DROP DATABASE IF EXISTS \"{database_name}\""))
        .execute(&admin_pool)
        .await
        .unwrap_or_else(|e| panic!("drop test db ({database_name}): {e}"));
    sqlx::query(&format!(
        "CREATE DATABASE \"{database_name}\" TEMPLATE \"{template_name}\""
    ))
    .execute(&admin_pool)
    .await
    .unwrap_or_else(|e| panic!("create test db ({database_name}): {e}"));
    sqlx::query(&format!(
        "REVOKE TEMPORARY ON DATABASE \"{database_name}\" FROM PUBLIC"
    ))
    .execute(&admin_pool)
    .await
    .unwrap_or_else(|e| panic!("restrict temporary tables in test db ({database_name}): {e}"));

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
    repo::tenants::default_for_user(db, user_id)
        .await
        .unwrap()
        .expect("registered test user has a tenant")
        .tenant_id
}

async fn template_database(base_url: &str) -> String {
    TEMPLATE_DB_NAME
        .get_or_init(|| async {
            let template_name = format!("wareboxes_template_{}", std::process::id());
            let admin_url = set_db_name(base_url, "postgres");
            let template_url = set_db_name(base_url, &template_name);

            let admin_pool = PgPoolOptions::new()
                .max_connections(1)
                .connect(&admin_url)
                .await
                .unwrap_or_else(|e| panic!("connect admin db ({admin_url}): {e}"));
            ensure_test_app_role(&admin_pool).await;

            sqlx::query(&format!("DROP DATABASE IF EXISTS \"{template_name}\""))
                .execute(&admin_pool)
                .await
                .unwrap_or_else(|e| panic!("drop template db ({template_name}): {e}"));
            sqlx::query(&format!("CREATE DATABASE \"{template_name}\""))
                .execute(&admin_pool)
                .await
                .unwrap_or_else(|e| panic!("create template db ({template_name}): {e}"));

            let template_pool = db::connect(&template_url)
                .await
                .unwrap_or_else(|e| panic!("connect template db ({template_url}): {e}"));
            db::run_migrations(&template_pool).await.unwrap();
            template_pool.close().await;

            template_name
        })
        .await
        .clone()
}

pub fn new_order(key: &str, inventory_owner_id: i64) -> NewOrder {
    NewOrder {
        order_key: key.to_string(),
        rush: Some(false),
        ship_by: None,
        line1: Some("1 Main St".into()),
        line2: None,
        city: "Reno".into(),
        state: "NV".into(),
        postal_code: "89501".into(),
        country: "US".into(),
        inventory_owner_id,
    }
}

pub struct Fixture {
    pub db: db::Db,
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
        let perm = repo::permissions::add_permission(&self.db, tenant_id, "wms", Some("WMS"))
            .await
            .unwrap();
        let role = repo::roles::add_role(
            &self.db,
            tenant_id,
            &format!("{email}-wms"),
            Some("WMS worker"),
        )
        .await
        .unwrap();
        repo::roles::add_role_permission(&self.db, tenant_id, role, perm)
            .await
            .unwrap();
        repo::roles::add_role_to_user(&self.db, tenant_id, user.id, role)
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
        repo::facilities::add_facility(&self.db, tenant_id, name)
            .await
            .unwrap()
    }

    pub async fn location(&self, tenant_id: TenantId, facility_id: i64, scan_code: &str) -> i64 {
        repo::locations::add_location(
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

    pub async fn order(&self, tenant_id: TenantId, key: &str, inventory_owner_id: i64) -> i64 {
        repo::orders::add_order(&self.db, tenant_id, &new_order(key, inventory_owner_id))
            .await
            .unwrap();
        repo::orders::get_orders(&self.db, tenant_id)
            .await
            .unwrap()
            .into_iter()
            .find(|order| order.order_key == key)
            .unwrap()
            .id
    }

    pub async fn order_item(&self, order_id: i64, item_id: i64, qty: i64) -> i64 {
        sqlx::query_scalar(
            r#"
            INSERT INTO order_items
                (tenant_id, inventory_owner_id, created, qty, item_id, order_id)
            SELECT tenant_id, inventory_owner_id, $1, $2, $3, id
            FROM orders
            WHERE id = $4
            RETURNING id
            "#,
        )
        .bind(db::now_iso())
        .bind(qty)
        .bind(item_id)
        .bind(order_id)
        .fetch_one(&self.db)
        .await
        .unwrap()
    }
}
