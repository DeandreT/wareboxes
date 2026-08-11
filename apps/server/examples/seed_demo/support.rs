use std::env;

use anyhow::{anyhow, bail, Context};
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::{IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use wareboxes_api::{auth, db, repo, routes, state::AppState};
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_domain::TenantId;

pub struct ReceivedBalance {
    pub balance_id: i64,
}

pub struct SeedContext {
    pub db: db::Db,
    pub admin: db::Db,
    pub app: axum::Router,
    pub tenant_id: TenantId,
    pub user_id: i64,
    pub token: String,
    pub email: String,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
}

impl SeedContext {
    pub async fn connect() -> anyhow::Result<Self> {
        let runtime_url = env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://wareboxes_app:wareboxes_app@127.0.0.1:5433/wareboxes".into()
        });
        let admin_url = env::var("MIGRATION_DATABASE_URL").unwrap_or_else(|_| {
            "postgres://wareboxes_admin:wareboxes_admin@127.0.0.1:5433/wareboxes".into()
        });
        let runtime = db::connect_runtime(&runtime_url)
            .await
            .context("connecting to the runtime database")?;
        let admin = db::connect(&admin_url)
            .await
            .context("connecting to the migration database")?;
        db::validate_same_database(&admin, &runtime)
            .await
            .context("validating seed database targets")?;

        let email = match env::var("SEED_USER_EMAIL")
            .ok()
            .or_else(|| env::var("BOOTSTRAP_ADMIN_EMAIL").ok())
        {
            Some(email) => email,
            None => sqlx::query_scalar::<_, String>(
                "SELECT email FROM users WHERE deleted IS NULL ORDER BY id LIMIT 1",
            )
            .fetch_optional(&admin)
            .await?
            .ok_or_else(|| anyhow!("seed-demo requires an active user"))?,
        };
        let user =
            wareboxes_persistence_postgres::users::find_user_by_email(&runtime, &email, false)
                .await?
                .ok_or_else(|| anyhow!("seed user {email} does not exist"))?;
        let token = auth::create_session(&runtime, user.id.get()).await?;
        let access = auth::default_tenant_for_session(&runtime, &token)
            .await?
            .ok_or_else(|| anyhow!("seed user {email} has no tenant"))?;
        let tenant_id = access.tenant_id;

        ensure_permissions(&runtime, tenant_id, user.id.get()).await?;
        repo::tenants::update_user_access_scope(
            &runtime,
            tenant_id,
            &UpdateUserAccessScope {
                user_id: user.id.get(),
                all_facilities: true,
                facility_ids: vec![],
                all_inventory_owners: true,
                inventory_owner_ids: vec![],
            },
        )
        .await?;

        let mut tx = runtime.begin().await?;
        db::bind_tenant_context(&mut tx, tenant_id).await?;
        let inventory_owner_id: i64 = sqlx::query_scalar(
            "SELECT id FROM inventory_owners WHERE tenant_id=$1 AND deleted IS NULL ORDER BY CASE WHEN name='Northstar Retail' THEN 0 ELSE 1 END, id LIMIT 1",
        )
        .bind(tenant_id.get())
        .fetch_one(&mut *tx)
        .await
        .context("finding a seeded inventory owner")?;
        let facility_id: i64 = sqlx::query_scalar(
            "SELECT id FROM facilities WHERE tenant_id=$1 AND deleted IS NULL ORDER BY CASE WHEN name='Riverside Distribution Center' THEN 0 ELSE 1 END, id LIMIT 1",
        )
        .bind(tenant_id.get())
        .fetch_one(&mut *tx)
        .await
        .context("finding a seeded facility")?;
        tx.rollback().await?;

        let app = routes::app(AppState::new(runtime.clone()));
        Ok(Self {
            db: runtime,
            admin,
            app,
            tenant_id,
            user_id: user.id.get(),
            token,
            email,
            inventory_owner_id,
            facility_id,
        })
    }

    pub async fn send(
        &self,
        method: Method,
        path: &str,
        key: Option<&str>,
        body: Option<Value>,
    ) -> anyhow::Result<axum::response::Response> {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
            .header(TENANT_ID_HEADER, self.tenant_id.to_string());
        if let Some(key) = key {
            request = request
                .header(IDEMPOTENCY_KEY_HEADER, key)
                .header(REQUEST_ID_HEADER, format!("seed-{key}"));
        }
        let body = match body {
            Some(value) => {
                request = request.header(header::CONTENT_TYPE, "application/json");
                Body::from(value.to_string())
            }
            None => Body::empty(),
        };
        self.app
            .clone()
            .oneshot(request.body(body)?)
            .await
            .map_err(|error| anyhow!("seed request failed: {error}"))
    }

    pub async fn command<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        key: &str,
        body: Value,
    ) -> anyhow::Result<T> {
        let response = self.send(method, path, Some(key), Some(body)).await?;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024).await?;
        if status != StatusCode::OK {
            let body = String::from_utf8_lossy(&bytes);
            bail!("{key}: expected 200 from {path}, got {status}: {body}");
        }
        serde_json::from_slice(&bytes)
            .with_context(|| format!("decoding {key} response from {path}"))
    }

    pub async fn scenario_exists(&self, order_key: &str) -> anyhow::Result<bool> {
        let mut tx = self.tenant_tx().await?;
        let found = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM orders WHERE tenant_id=$1 AND order_key=$2)",
        )
        .bind(self.tenant_id.get())
        .bind(order_key)
        .fetch_one(&mut *tx)
        .await?;
        tx.rollback().await?;
        Ok(found)
    }

    pub async fn location(
        &self,
        barcode: &str,
        kind: &str,
        pickable: bool,
        receivable: bool,
    ) -> anyhow::Result<i64> {
        if let Some(id) = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM locations WHERE tenant_id=$1 AND barcode=$2 AND deleted IS NULL",
        )
        .bind(self.tenant_id.get())
        .bind(barcode)
        .fetch_optional(&self.admin)
        .await?
        {
            return Ok(id);
        }
        wareboxes_persistence_postgres::locations::add_location(
            &self.db,
            self.tenant_id,
            self.facility_id,
            None,
            Some(barcode),
            Some(barcode),
            kind,
            true,
            pickable,
            receivable,
        )
        .await
        .map_err(Into::into)
    }

    pub async fn plate_at(&self, barcode: &str, location_id: i64) -> anyhow::Result<i64> {
        if let Some(id) = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM license_plates WHERE tenant_id=$1 AND barcode=$2 AND deleted IS NULL",
        )
        .bind(self.tenant_id.get())
        .bind(barcode)
        .fetch_optional(&self.admin)
        .await?
        {
            return Ok(id);
        }
        let id = repo::license_plates::add_license_plate(
            &self.db,
            self.tenant_id,
            self.inventory_owner_id,
            self.facility_id,
            Some(barcode),
        )
        .await?;
        sqlx::query("UPDATE license_plates SET location_id=$1 WHERE tenant_id=$2 AND id=$3")
            .bind(location_id)
            .bind(self.tenant_id.get())
            .bind(id)
            .execute(&self.admin)
            .await?;
        Ok(id)
    }

    pub async fn ensure_shipping_origin(&self) -> anyhow::Result<()> {
        let (address_id, revision): (Option<i64>, i64) = sqlx::query_as(
            "SELECT address_id, revision FROM facilities WHERE tenant_id=$1 AND id=$2",
        )
        .bind(self.tenant_id.get())
        .bind(self.facility_id)
        .fetch_one(&self.admin)
        .await?;
        if address_id.is_some() {
            return Ok(());
        }
        let _: wareboxes_api_contract::v1::ConfigureFacilityShippingOriginResponse = self
            .command(
                Method::POST,
                &format!(
                    "/api/v1/facilities/{}/shipping-origin-configurations",
                    self.facility_id
                ),
                "demo-facility-origin",
                serde_json::json!({
                    "expected_revision": revision,
                    "name": "Riverside Shipping",
                    "company": "Wareboxes Demo Fulfillment",
                    "line1": "100 Distribution Way",
                    "line2": "Outbound Office",
                    "city": "Reno",
                    "state": "NV",
                    "postal_code": "89502",
                    "country": "US",
                    "phone": "+1 775 555 0100",
                    "email": "shipping@wareboxes.local"
                }),
            )
            .await?;
        Ok(())
    }

    pub async fn item(&self, name: &str, packaging_unit: &str) -> anyhow::Result<i64> {
        repo::items::add_item(
            &self.db,
            self.tenant_id,
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
        .map_err(Into::into)
    }

    pub async fn order_header(&self, key: &str) -> anyhow::Result<i64> {
        let mut tx = self.tenant_tx().await?;
        let address_id = repo::address::insert_address_tx(
            &mut tx,
            self.tenant_id,
            repo::address::NewAddress {
                name: Some("Demo Recipient"),
                company: Some("Wareboxes Demo Customer"),
                line1: "500 Operations Way",
                line2: None,
                city: Some("Reno"),
                state: Some("NV"),
                postal_code: Some("89501"),
                country: "US",
                phone: None,
                email: Some("receiving@example.test"),
            },
        )
        .await?;
        let created = db::now_iso();
        let order_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO orders
                (tenant_id, inventory_owner_id, order_key, created, rush, status, address_id)
            VALUES ($1, $2, $3, $4, false, 'open', $5)
            RETURNING id
            "#,
        )
        .bind(self.tenant_id.get())
        .bind(self.inventory_owner_id)
        .bind(key)
        .bind(created)
        .bind(address_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO order_activity
                (tenant_id, inventory_owner_id, created, order_id, action)
            VALUES ($1, $2, $3, $4, 'demo order created')
            "#,
        )
        .bind(self.tenant_id.get())
        .bind(self.inventory_owner_id)
        .bind(created)
        .bind(order_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(order_id)
    }

    pub async fn order_item(
        &self,
        order_id: i64,
        item_id: i64,
        quantity: i64,
    ) -> anyhow::Result<i64> {
        let mut tx = self.tenant_tx().await?;
        sqlx::query("SELECT id FROM orders WHERE tenant_id=$1 AND id=$2 FOR UPDATE")
            .bind(self.tenant_id.get())
            .bind(order_id)
            .fetch_one(&mut *tx)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO inventory_owner_items
                (tenant_id, created, inventory_owner_id, item_id)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (tenant_id, inventory_owner_id, item_id)
            DO UPDATE SET deleted=NULL
            "#,
        )
        .bind(self.tenant_id.get())
        .bind(db::now_iso())
        .bind(self.inventory_owner_id)
        .bind(item_id)
        .execute(&mut *tx)
        .await?;
        let line_id: i64 = sqlx::query_scalar(
            r#"
            WITH next_line AS (
                SELECT COALESCE(MAX(line_number), 0) + 1 AS line_number
                FROM order_items WHERE tenant_id=$1 AND order_id=$2
            )
            INSERT INTO order_items
                (tenant_id, inventory_owner_id, created, line_key, line_number,
                 qty, item_id, order_id, uom)
            SELECT $1, $3, $4, next_line.line_number::text, next_line.line_number,
                   $5, $6, $2, item.packaging_unit
            FROM next_line
            INNER JOIN items item ON item.tenant_id=$1 AND item.id=$6
            RETURNING id
            "#,
        )
        .bind(self.tenant_id.get())
        .bind(order_id)
        .bind(self.inventory_owner_id)
        .bind(db::now_iso())
        .bind(quantity)
        .bind(item_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(line_id)
    }

    pub async fn received_balance(
        &self,
        item_id: i64,
        quantity: i64,
        key: &str,
    ) -> anyhow::Result<ReceivedBalance> {
        let location_id = self.location(key, "pick", true, false).await?;
        let batch_id = repo::inventory::add_item_batch(
            &self.db,
            self.tenant_id,
            self.inventory_owner_id,
            item_id,
            None,
            Some(key),
            None,
            None,
        )
        .await?;
        repo::inventory::receive_inventory(
            &self.db,
            self.tenant_id,
            self.user_id,
            batch_id,
            location_id,
            quantity,
            None,
            Some("demo workflow stock"),
            None,
            None,
            &format!("{key}-receipt"),
        )
        .await?;
        let mut tx = self.tenant_tx().await?;
        let balance_id = sqlx::query_scalar(
            "SELECT id FROM inventory_balances WHERE tenant_id=$1 AND inventory_owner_id=$2 AND location_id=$3 AND item_batch_id=$4 AND deleted IS NULL",
        )
        .bind(self.tenant_id.get())
        .bind(self.inventory_owner_id)
        .bind(location_id)
        .bind(batch_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.rollback().await?;
        Ok(ReceivedBalance { balance_id })
    }

    async fn tenant_tx(&self) -> anyhow::Result<sqlx::Transaction<'_, sqlx::Postgres>> {
        let mut tx = self.db.begin().await?;
        db::bind_tenant_context(&mut tx, self.tenant_id).await?;
        Ok(tx)
    }

    pub async fn verify(&self) -> anyhow::Result<()> {
        let required = [
            ("packing", "packing_sessions"),
            ("shipping", "shipments"),
            ("outbound loads", "outbound_loads"),
            ("pick waves", "pick_waves"),
            ("replenishment", "replenishment_policies"),
            ("replenishment work", "replenishment_tasks"),
            ("putaway", "putaway_tasks"),
            ("inventory holds", "inventory_holds"),
            ("integration monitor", "integration_inbox_receipts"),
        ];
        let mut missing = Vec::new();
        for (label, table) in required {
            let query = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE tenant_id=$1)");
            if !sqlx::query_scalar::<_, bool>(&query)
                .bind(self.tenant_id.get())
                .fetch_one(&self.admin)
                .await?
            {
                missing.push(label);
            }
        }
        let has_cycle_counts = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM cycle_count_location_tasks WHERE tenant_id=$1) OR EXISTS(SELECT 1 FROM cycle_count_item_location_tasks WHERE tenant_id=$1)",
        )
        .bind(self.tenant_id.get())
        .fetch_one(&self.admin)
        .await?;
        if !has_cycle_counts {
            missing.push("cycle counts");
        }
        if !missing.is_empty() {
            bail!("typed demo coverage is incomplete: {}", missing.join(", "));
        }
        Ok(())
    }

    pub async fn close(self) -> anyhow::Result<()> {
        auth::destroy_session(&self.db, &self.token).await?;
        self.db.close().await;
        self.admin.close().await;
        Ok(())
    }
}

async fn ensure_permissions(db: &db::Db, tenant_id: TenantId, user_id: i64) -> anyhow::Result<()> {
    let roles = wareboxes_persistence_postgres::roles::get_roles(db, tenant_id, true, true).await?;
    let role_id = if let Some(role) = roles.iter().find(|role| role.name == "Demo seed operator") {
        role.id
    } else {
        wareboxes_persistence_postgres::roles::add_role(
            db,
            tenant_id,
            "Demo seed operator",
            Some("Local full-workflow demo seeding"),
        )
        .await?
    };
    for name in ["admin", "orders", "wms", "wms_supervisor"] {
        let permission =
            match wareboxes_persistence_postgres::permissions::find_by_name(db, tenant_id, name)
                .await?
            {
                Some(permission) => permission.id,
                None => {
                    wareboxes_persistence_postgres::permissions::add_permission(
                        db,
                        tenant_id,
                        name,
                        Some("Demo seed permission"),
                    )
                    .await?
                }
            };
        let _ = wareboxes_persistence_postgres::roles::add_role_permission(
            db, tenant_id, role_id, permission,
        )
        .await?;
    }
    let _ =
        wareboxes_persistence_postgres::roles::add_role_to_user(db, tenant_id, user_id, role_id)
            .await?;
    Ok(())
}
