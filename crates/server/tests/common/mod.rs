#![allow(dead_code, unused_imports)]

use std::sync::atomic::{AtomicU64, Ordering};

use sqlx::postgres::PgPoolOptions;
use tokio::sync::OnceCell;
use url::Url;

pub use wareboxes_core::dto::{NewOrder, OrderUpdate};
pub use wareboxes_core::models::{
    LoadLineStatus, LoadStatus, LoadType, MovementType, OrderStatus, WorkTaskProgressAction,
    WorkTaskStatus, WorkTaskType,
};
pub use wareboxes_core::CoreError;
pub use wareboxes_server::error::AppError;
pub use wareboxes_server::{auth, db, permissions, repo};

const DEFAULT_TEST_DATABASE_URL: &str = "postgres://wareboxes:wareboxes@127.0.0.1:5433/wareboxes";
static NEXT_TEST_DB_ID: AtomicU64 = AtomicU64::new(1);
static TEMPLATE_DB_NAME: OnceCell<String> = OnceCell::const_new();

fn set_db_name(database_url: &str, db_name: &str) -> String {
    let mut parsed = Url::parse(database_url).expect("valid TEST_DATABASE_URL");
    parsed.set_path(&format!("/{db_name}"));
    parsed.to_string()
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
    let test_url = set_db_name(&base_url, &database_name);

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

    let pool = db::connect(&test_url)
        .await
        .unwrap_or_else(|e| panic!("connect test db ({test_url}): {e}"));
    db::run_migrations(&pool).await.unwrap();
    pool
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

pub fn new_order(key: &str, account_id: Option<i64>) -> NewOrder {
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
        account_id,
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
        let perm = repo::permissions::add_permission(&self.db, "wms", Some("WMS"))
            .await
            .unwrap();
        let role = repo::roles::add_role(&self.db, &format!("{email}-wms"), Some("WMS worker"))
            .await
            .unwrap();
        repo::roles::add_role_permission(&self.db, role, perm)
            .await
            .unwrap();
        repo::roles::add_role_to_user(&self.db, user.id, role)
            .await
            .unwrap();
        user
    }

    pub async fn account(&self, name: &str) -> i64 {
        repo::accounts::add_account(&self.db, name, &format!("{name}@test.local"))
            .await
            .unwrap()
    }

    pub async fn warehouse(&self, name: &str) -> i64 {
        repo::warehouses::add_warehouse(&self.db, name)
            .await
            .unwrap()
    }

    pub async fn location(&self, warehouse_id: i64, scan_code: &str) -> i64 {
        repo::locations::add_location(
            &self.db,
            warehouse_id,
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

    pub async fn item(&self, name: &str, packaging_unit: &str) -> i64 {
        repo::items::add_item(
            &self.db,
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

    pub async fn order(&self, key: &str, account_id: Option<i64>) -> i64 {
        repo::orders::add_order(&self.db, &new_order(key, account_id))
            .await
            .unwrap();
        repo::orders::get_orders(&self.db)
            .await
            .unwrap()
            .into_iter()
            .find(|order| order.order_key == key)
            .unwrap()
            .id
    }

    pub async fn order_item(&self, order_id: i64, item_id: i64, qty: i64) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO order_items (created, qty, item_id, order_id) VALUES ($1, $2, $3, $4) RETURNING id",
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
