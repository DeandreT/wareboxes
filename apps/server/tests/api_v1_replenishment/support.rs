use super::*;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request};
use serde::de::DeserializeOwned;
use sqlx::Row;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::{IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_core::models::TenantAccess;

pub(super) fn init_test_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("wareboxes_api=debug")
        .with_test_writer()
        .try_init();
}

pub(super) struct ReplenishmentFixture {
    pub(super) fixture: Fixture,
    pub(super) app: axum::Router,
    pub(super) access: TenantAccess,
    pub(super) token: String,
    pub(super) inventory_owner_id: i64,
    pub(super) facility_id: i64,
    pub(super) item_id: i64,
    pub(super) item_barcode: String,
    pub(super) pick_face_location_id: i64,
    pub(super) pick_face_barcode: String,
}

#[derive(Debug, Clone)]
pub(super) struct SeededStock {
    pub(super) balance_id: i64,
    pub(super) location_id: i64,
    pub(super) location_barcode: String,
}

pub(super) struct PolicyDimensions {
    pub(super) item_id: i64,
    pub(super) pick_face_location_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EffectCounts {
    pub(super) policies: i64,
    pub(super) plans: i64,
    pub(super) tasks: i64,
    pub(super) confirmations: i64,
    pub(super) transactions: i64,
    pub(super) entries: i64,
    pub(super) allocations: i64,
}

impl EffectCounts {
    pub(super) fn assert_unchanged(&self, current: &Self, context: &str) {
        assert_eq!(current.policies, self.policies, "{context}: policies");
        assert_eq!(current.plans, self.plans, "{context}: plans");
        assert_eq!(current.tasks, self.tasks, "{context}: tasks");
        assert_eq!(
            current.confirmations, self.confirmations,
            "{context}: confirmations"
        );
        assert_eq!(
            current.transactions, self.transactions,
            "{context}: transactions"
        );
        assert_eq!(current.entries, self.entries, "{context}: entries");
        assert_eq!(
            current.allocations, self.allocations,
            "{context}: allocations"
        );
    }
}

impl ReplenishmentFixture {
    pub(super) async fn new(key: &str) -> Self {
        let fixture = Fixture::new().await;
        let operator = fixture.wms_user(&format!("{key}@test.local")).await;
        let access = default_tenant_for_user(&fixture.db, operator.id)
            .await
            .expect("WMS operator has tenant access");
        grant_permission(
            &fixture.db,
            access.tenant_id,
            operator.id,
            &format!("{key}-supervisor"),
            "wms_supervisor",
        )
        .await;
        grant_permission(
            &fixture.db,
            access.tenant_id,
            operator.id,
            &format!("{key}-orders"),
            "orders",
        )
        .await;
        let inventory_owner_id = fixture
            .inventory_owner(access.tenant_id, &format!("{key} owner"))
            .await;
        let facility_id = fixture
            .facility(access.tenant_id, &format!("{key} facility"))
            .await;
        fixture
            .assign_owner_to_facility(access.tenant_id, inventory_owner_id, facility_id)
            .await;
        let item_id = fixture
            .item(access.tenant_id, &format!("{key} item"), "each")
            .await;
        repo::inventory::add_item_batch(
            &fixture.db,
            access.tenant_id,
            inventory_owner_id,
            item_id,
            None,
            Some(&format!("{key}-CATALOG-LINK")),
            None,
            None,
        )
        .await
        .unwrap();
        let item_barcode = format!("{key}-ITEM");
        repo::items::add_barcode(
            &fixture.db,
            access.tenant_id,
            item_id,
            &item_barcode,
            "code128",
            None,
        )
        .await
        .unwrap();
        let pick_face_barcode = format!("{key}-PICK");
        let pick_face_location_id = fixture
            .location(access.tenant_id, facility_id, &pick_face_barcode)
            .await;
        let token = auth::create_session(&fixture.db, operator.id)
            .await
            .unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));

        Self {
            fixture,
            app,
            access,
            token,
            inventory_owner_id,
            facility_id,
            item_id,
            item_barcode,
            pick_face_location_id,
            pick_face_barcode,
        }
    }

    pub(super) async fn request(
        &self,
        method: Method,
        path: &str,
        key: Option<&str>,
        body: Option<Value>,
    ) -> axum::response::Response {
        send(
            &self.app,
            &self.token,
            self.access.tenant_id,
            method,
            path,
            key,
            body,
        )
        .await
    }

    pub(super) async fn reserve_source(&self, key: &str) -> (i64, String) {
        let barcode = format!("{key}-RESERVE");
        let id = wareboxes_persistence_postgres::locations::add_location(
            &self.fixture.db,
            self.access.tenant_id,
            self.facility_id,
            None,
            Some(&barcode),
            Some(&barcode),
            "reserve",
            true,
            false,
            false,
        )
        .await
        .unwrap();
        (id, barcode)
    }

    pub(super) async fn policy_dimensions(&self, key: &str) -> PolicyDimensions {
        let item_id = self
            .fixture
            .item(self.access.tenant_id, &format!("{key} item"), "each")
            .await;
        repo::inventory::add_item_batch(
            &self.fixture.db,
            self.access.tenant_id,
            self.inventory_owner_id,
            item_id,
            None,
            Some(&format!("{key}-CATALOG-LINK")),
            None,
            None,
        )
        .await
        .unwrap();
        let item_barcode = format!("{key}-ITEM");
        repo::items::add_barcode(
            &self.fixture.db,
            self.access.tenant_id,
            item_id,
            &item_barcode,
            "code128",
            None,
        )
        .await
        .unwrap();
        let pick_face_barcode = format!("{key}-PICK");
        let pick_face_location_id = self
            .fixture
            .location(self.access.tenant_id, self.facility_id, &pick_face_barcode)
            .await;
        PolicyDimensions {
            item_id,
            pick_face_location_id,
        }
    }

    pub(super) async fn seed_stock(
        &self,
        location_id: i64,
        location_barcode: &str,
        quantity: i64,
        lot: &str,
        expiration: Option<&str>,
        key: &str,
    ) -> SeededStock {
        self.seed_item_stock(
            self.item_id,
            location_id,
            location_barcode,
            quantity,
            lot,
            expiration,
            key,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn seed_item_stock(
        &self,
        item_id: i64,
        location_id: i64,
        location_barcode: &str,
        quantity: i64,
        lot: &str,
        expiration: Option<&str>,
        key: &str,
    ) -> SeededStock {
        self.seed_item_stock_with_serial(
            item_id,
            location_id,
            location_barcode,
            quantity,
            lot,
            None,
            expiration,
            key,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn seed_item_stock_with_serial(
        &self,
        item_id: i64,
        location_id: i64,
        location_barcode: &str,
        quantity: i64,
        lot: &str,
        serial: Option<&str>,
        expiration: Option<&str>,
        key: &str,
    ) -> SeededStock {
        let expiration_value = expiration.map(|value| value.parse().unwrap());
        let batch_id = repo::inventory::add_item_batch(
            &self.fixture.db,
            self.access.tenant_id,
            self.inventory_owner_id,
            item_id,
            None,
            Some(lot),
            serial,
            expiration_value,
        )
        .await
        .unwrap();
        repo::inventory::receive_inventory(
            &self.fixture.db,
            self.access.tenant_id,
            self.access.user_id.get(),
            batch_id,
            location_id,
            quantity,
            None,
            Some("replenishment test stock"),
            None,
            None,
            key,
        )
        .await
        .unwrap();
        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        let balance_id = sqlx::query_scalar(
            r#"
            SELECT id
            FROM inventory_balances
            WHERE tenant_id = $1
              AND inventory_owner_id = $2
              AND facility_id = $3
              AND location_id = $4
              AND item_batch_id = $5
              AND deleted IS NULL
            "#,
        )
        .bind(self.access.tenant_id.get())
        .bind(self.inventory_owner_id)
        .bind(self.facility_id)
        .bind(location_id)
        .bind(batch_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        SeededStock {
            balance_id,
            location_id,
            location_barcode: location_barcode.to_owned(),
        }
    }

    pub(super) async fn configure(
        &self,
        key: &str,
        source_ids: &[i64],
        minimum: i64,
        target: i64,
        expected_revision: Option<i64>,
    ) -> axum::response::Response {
        self.configure_for(
            self.item_id,
            self.pick_face_location_id,
            key,
            source_ids,
            minimum,
            target,
            expected_revision,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn configure_for(
        &self,
        item_id: i64,
        pick_face_location_id: i64,
        key: &str,
        source_ids: &[i64],
        minimum: i64,
        target: i64,
        expected_revision: Option<i64>,
    ) -> axum::response::Response {
        let mut body = json!({
            "inventory_owner_id": self.inventory_owner_id,
            "facility_id": self.facility_id,
            "item_id": item_id,
            "uom": "each",
            "pick_face_location_id": pick_face_location_id,
            "minimum_quantity": minimum,
            "target_quantity": target,
            "reserve_source_location_ids": source_ids,
        });
        if let Some(revision) = expected_revision {
            body["expected_revision"] = json!(revision);
        }
        self.request(
            Method::POST,
            "/api/v1/replenishment-policies",
            Some(key),
            Some(body),
        )
        .await
    }

    pub(super) async fn plan(
        &self,
        policy_id: i64,
        revision: i64,
        key: &str,
    ) -> axum::response::Response {
        self.request(
            Method::POST,
            &format!("/api/v1/replenishment-policies/{policy_id}/plan-runs"),
            Some(key),
            Some(json!({"expected_policy_revision": revision})),
        )
        .await
    }

    pub(super) async fn claim_by_id(&self, work_id: i64, key: &str) -> axum::response::Response {
        self.request(
            Method::POST,
            &format!("/api/v1/replenishment-claims/{work_id}"),
            Some(key),
            Some(json!({})),
        )
        .await
    }

    pub(super) async fn confirm(
        &self,
        claim: &ReplenishmentClaimResponse,
        key: &str,
        body: Value,
    ) -> axum::response::Response {
        self.request(
            Method::POST,
            &format!(
                "/api/v1/replenishment-tasks/{}/confirmations",
                claim.work_id
            ),
            Some(key),
            Some(body),
        )
        .await
    }

    pub(super) fn exact_scans(&self, claim: &ReplenishmentClaimResponse) -> Value {
        json!({
            "source_location_barcode": claim.source_location.barcode,
            "item_barcode": self.item_barcode,
            "lot_scan": claim.lot,
            "serial_scan": claim.serial,
            "destination_pick_face_barcode": claim.destination_pick_face.barcode,
        })
    }

    pub(super) async fn set_scope(&self, facility_ids: Vec<i64>, inventory_owner_ids: Vec<i64>) {
        assert!(repo::tenants::update_user_access_scope(
            &self.fixture.db,
            self.access.tenant_id,
            &UpdateUserAccessScope {
                user_id: self.access.user_id.get(),
                all_facilities: false,
                facility_ids,
                all_inventory_owners: false,
                inventory_owner_ids,
            },
        )
        .await
        .unwrap());
    }

    pub(super) async fn effect_counts(&self) -> EffectCounts {
        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        let row = sqlx::query(
            r#"
            SELECT
              (SELECT COUNT(*) FROM replenishment_policies WHERE tenant_id = $1) policies,
              (SELECT COUNT(*) FROM replenishment_plan_runs WHERE tenant_id = $1) plans,
              (SELECT COUNT(*) FROM replenishment_tasks WHERE tenant_id = $1) tasks,
              (SELECT COUNT(*) FROM replenishment_confirmations WHERE tenant_id = $1) confirmations,
              (SELECT COUNT(*) FROM inventory_transactions WHERE tenant_id = $1) transactions,
              (SELECT COUNT(*) FROM inventory_entries WHERE tenant_id = $1) entries,
              (SELECT COUNT(*) FROM inventory_allocations WHERE tenant_id = $1) allocations
            "#,
        )
        .bind(self.access.tenant_id.get())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        EffectCounts {
            policies: row.get("policies"),
            plans: row.get("plans"),
            tasks: row.get("tasks"),
            confirmations: row.get("confirmations"),
            transactions: row.get("transactions"),
            entries: row.get("entries"),
            allocations: row.get("allocations"),
        }
    }
}

pub(super) async fn grant_permission(
    db: &db::Db,
    tenant_id: TenantId,
    user_id: i64,
    role_name: &str,
    permission_name: &str,
) {
    let permission = match wareboxes_persistence_postgres::permissions::find_by_name(
        db,
        tenant_id,
        permission_name,
    )
    .await
    .unwrap()
    {
        Some(permission) => permission.id,
        None => wareboxes_persistence_postgres::permissions::add_permission(
            db,
            tenant_id,
            permission_name,
            Some(permission_name),
        )
        .await
        .unwrap(),
    };
    let role =
        wareboxes_persistence_postgres::roles::add_role(db, tenant_id, role_name, Some(role_name))
            .await
            .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        db, tenant_id, role, permission,
    )
    .await
    .unwrap());
    assert!(
        wareboxes_persistence_postgres::roles::add_role_to_user(db, tenant_id, user_id, role,)
            .await
            .unwrap()
    );
}

pub(super) async fn send(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    method: Method,
    path: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    if let Some(key) = key {
        request = request
            .header(IDEMPOTENCY_KEY_HEADER, key)
            .header(REQUEST_ID_HEADER, format!("request-{key}"));
    }
    let body = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    app.clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}

pub(super) async fn response_json<T: DeserializeOwned>(response: axum::response::Response) -> T {
    let bytes = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

pub(super) async fn expect_status(
    response: axum::response::Response,
    expected: StatusCode,
    context: &str,
) -> axum::response::Response {
    let status = response.status();
    if status == expected {
        return response;
    }
    let bytes = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    panic!(
        "{context}: expected {expected}, got {status}: {}",
        String::from_utf8_lossy(&bytes)
    );
}

pub(super) async fn assert_error_reason(
    response: axum::response::Response,
    expected_status: StatusCode,
    expected_reason: ErrorReason,
    context: &str,
) {
    let response = expect_status(response, expected_status, context).await;
    let error: ErrorResponse = response_json(response).await;
    assert_eq!(error.reason, expected_reason, "{context}");
}
