mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{repo, routes, state::AppState};
use wareboxes_api_contract::v1::{
    ItemTraceabilityPolicyPage, ItemTraceabilityPolicyResponse, ItemTraceabilityPolicyStatus,
};
use wareboxes_core::dto::UpdateUserAccessScope;

struct Rig {
    fixture: Fixture,
    tenant_id: TenantId,
    user_id: i64,
    token: String,
    app: axum::Router,
    inventory_owner_id: i64,
    facility_id: i64,
    item_id: i64,
    first_location_id: i64,
    second_location_id: i64,
}

impl Rig {
    async fn new(test_name: &str) -> Self {
        let fixture = Fixture::new().await;
        let user = fixture
            .wms_user(&format!("item-traceability-{test_name}@test.local"))
            .await;
        let tenant_id = tenant_for_user(&fixture.db, user.id).await;
        grant_supervisor(&fixture, tenant_id, user.id, test_name).await;
        let inventory_owner_id = fixture
            .inventory_owner(tenant_id, &format!("Traceability Client {test_name}"))
            .await;
        let facility_id = fixture
            .facility(tenant_id, &format!("Traceability Facility {test_name}"))
            .await;
        fixture
            .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
            .await;
        let item_id = fixture
            .item(tenant_id, &format!("Traceability Item {test_name}"), "case")
            .await;
        repo::inventory::add_item_batch(
            &fixture.db,
            tenant_id,
            inventory_owner_id,
            item_id,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let first_location_id = fixture
            .location(tenant_id, facility_id, &format!("TRACE-A-{test_name}"))
            .await;
        let second_location_id = fixture
            .location(tenant_id, facility_id, &format!("TRACE-B-{test_name}"))
            .await;
        let token = wareboxes_api::auth::create_session(&fixture.db, user.id)
            .await
            .unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));
        Self {
            fixture,
            tenant_id,
            user_id: user.id,
            token,
            app,
            inventory_owner_id,
            facility_id,
            item_id,
            first_location_id,
            second_location_id,
        }
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        key: Option<&str>,
        body: Option<Value>,
    ) -> axum::response::Response {
        self.send_as(&self.token, self.tenant_id, method, path, key, body)
            .await
    }

    async fn send_as(
        &self,
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
            request = request.header(IDEMPOTENCY_KEY_HEADER, key);
        }
        let body = match body {
            Some(body) => {
                request = request.header(header::CONTENT_TYPE, "application/json");
                Body::from(body.to_string())
            }
            None => Body::empty(),
        };
        self.app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap()
    }

    async fn set_scope(&self, facility_ids: Vec<i64>, inventory_owner_ids: Vec<i64>) {
        assert!(repo::tenants::update_user_access_scope(
            &self.fixture.db,
            self.tenant_id,
            &UpdateUserAccessScope {
                user_id: self.user_id,
                all_facilities: false,
                facility_ids,
                all_inventory_owners: false,
                inventory_owner_ids,
            },
        )
        .await
        .unwrap());
    }

    async fn configure_policy(
        &self,
        key: &str,
        lot: &str,
        serial: &str,
        expiration: &str,
        minimum_shelf_life_days: Option<u32>,
        expected_revision: Option<i64>,
    ) -> axum::response::Response {
        self.send(
            Method::POST,
            "/api/v1/item-traceability-policies",
            Some(key),
            Some(json!({
                "inventory_owner_id": self.inventory_owner_id,
                "facility_id": self.facility_id,
                "item_id": self.item_id,
                "uom": "case",
                "lot": lot,
                "serial": serial,
                "expiration": expiration,
                "minimum_shelf_life_days": minimum_shelf_life_days,
                "expected_revision": expected_revision
            })),
        )
        .await
    }

    async fn batch(
        &self,
        lot: Option<&str>,
        serial: Option<&str>,
        expiration: Option<wareboxes_domain::Timestamp>,
    ) -> i64 {
        repo::inventory::add_item_batch(
            &self.fixture.db,
            self.tenant_id,
            self.inventory_owner_id,
            self.item_id,
            None,
            lot,
            serial,
            expiration,
        )
        .await
        .unwrap()
    }

    async fn receive(
        &self,
        batch_id: i64,
        location_id: i64,
        qty: i64,
        key: &str,
    ) -> wareboxes_api::error::AppResult<i64> {
        repo::inventory::receive_inventory(
            &self.fixture.db,
            self.tenant_id,
            self.user_id,
            batch_id,
            location_id,
            qty,
            None,
            Some("item traceability policy test"),
            None,
            None,
            key,
        )
        .await
    }
}

async fn json_response<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "failed to decode {status} as {}: {error}; body={}",
            std::any::type_name::<T>(),
            String::from_utf8_lossy(&bytes)
        )
    })
}

fn future_timestamp(days: u64) -> wareboxes_domain::Timestamp {
    wareboxes_api::db::now_iso() + std::time::Duration::from_secs(days * 24 * 60 * 60)
}

async fn grant_supervisor(fixture: &Fixture, tenant_id: TenantId, user_id: i64, test_name: &str) {
    let permission = match wareboxes_persistence_postgres::permissions::find_by_name(
        &fixture.db,
        tenant_id,
        "wms_supervisor",
    )
    .await
    .unwrap()
    {
        Some(permission) => permission.id,
        None => wareboxes_persistence_postgres::permissions::add_permission(
            &fixture.db,
            tenant_id,
            "wms_supervisor",
            Some("WMS supervisor"),
        )
        .await
        .unwrap(),
    };
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("traceability-policy-{test_name}"),
        Some("Item traceability policy supervisor test role"),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        permission,
    )
    .await
    .unwrap());
    assert!(wareboxes_persistence_postgres::roles::add_role_to_user(
        &fixture.db,
        tenant_id,
        user_id,
        role,
    )
    .await
    .unwrap());
}

#[tokio::test]
async fn lifecycle_enforces_identity_shelf_life_replay_and_history() {
    let rig = Rig::new("lifecycle").await;
    let response = rig
        .configure_policy(
            "trace-policy-create",
            "required",
            "not_tracked",
            "required",
            Some(30),
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let created: ItemTraceabilityPolicyResponse = json_response(response).await;
    assert_eq!(created.revision.get(), 1);
    assert_eq!(created.minimum_shelf_life_days, Some(30));
    let replay: ItemTraceabilityPolicyResponse = json_response(
        rig.configure_policy(
            "trace-policy-create",
            "required",
            "not_tracked",
            "required",
            Some(30),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(replay, created);
    let changed = rig
        .configure_policy(
            "trace-policy-create",
            "required",
            "not_tracked",
            "required",
            Some(31),
            None,
        )
        .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);

    let valid = rig
        .batch(Some("LOT-GOOD"), None, Some(future_timestamp(45)))
        .await;
    rig.receive(valid, rig.first_location_id, 5, "trace-valid")
        .await
        .unwrap();
    let missing_lot = rig.batch(None, None, Some(future_timestamp(45))).await;
    assert!(rig
        .receive(missing_lot, rig.second_location_id, 1, "trace-missing-lot")
        .await
        .is_err());
    let short_life = rig
        .batch(Some("LOT-SHORT"), None, Some(future_timestamp(5)))
        .await;
    assert!(rig
        .receive(short_life, rig.second_location_id, 1, "trace-short-life")
        .await
        .is_err());
    let unexpected_serial = rig
        .batch(
            Some("LOT-SERIAL"),
            Some("SER-NOT-ALLOWED"),
            Some(future_timestamp(45)),
        )
        .await;
    assert!(rig
        .receive(
            unexpected_serial,
            rig.second_location_id,
            1,
            "trace-unexpected-serial"
        )
        .await
        .is_err());

    let incompatible = rig
        .configure_policy(
            "trace-policy-incompatible",
            "required",
            "required",
            "required",
            Some(30),
            Some(1),
        )
        .await;
    assert_eq!(incompatible.status(), StatusCode::CONFLICT);
    let replacement: ItemTraceabilityPolicyResponse = json_response(
        rig.configure_policy(
            "trace-policy-reconfigure",
            "required",
            "not_tracked",
            "required",
            Some(20),
            Some(1),
        )
        .await,
    )
    .await;
    assert_eq!(replacement.revision.get(), 2);

    let page: ItemTraceabilityPolicyPage = json_response(
        rig.send(
            Method::GET,
            &format!(
                "/api/v1/item-traceability-policies?inventory_owner_id={}&facility_id={}&lot=required&expiration=required",
                rig.inventory_owner_id, rig.facility_id
            ),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(page.items, vec![replacement.clone()]);

    let retired: ItemTraceabilityPolicyResponse = json_response(
        rig.send(
            Method::POST,
            &format!(
                "/api/v1/item-traceability-policies/{}/retirements",
                replacement.item_traceability_policy_id
            ),
            Some("trace-policy-retire"),
            Some(json!({"expected_revision": 2})),
        )
        .await,
    )
    .await;
    assert_eq!(retired.status, ItemTraceabilityPolicyStatus::Retired);
    let ungoverned = rig.batch(None, Some("FREE-SERIAL"), None).await;
    rig.receive(
        ungoverned,
        rig.second_location_id,
        2,
        "trace-after-retirement",
    )
    .await
    .unwrap();
    let history: ItemTraceabilityPolicyPage = json_response(
        rig.send(
            Method::GET,
            &format!(
                "/api/v1/item-traceability-policies?inventory_owner_id={}&status=retired",
                rig.inventory_owner_id
            ),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(history.items.len(), 2);
}

#[tokio::test]
async fn serial_identity_is_one_unit_and_concurrent_receipts_have_one_winner() {
    let rig = Rig::new("serial-race").await;
    let response = rig
        .configure_policy(
            "serial-policy-create",
            "not_tracked",
            "required",
            "not_tracked",
            None,
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let first_batch = rig.batch(None, Some("SERIAL-ONE"), None).await;
    let second_batch = rig.batch(None, Some("SERIAL-ONE"), None).await;
    let (first, second) = tokio::join!(
        rig.receive(first_batch, rig.first_location_id, 1, "serial-race-first"),
        rig.receive(
            second_batch,
            rig.second_location_id,
            1,
            "serial-race-second"
        ),
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let quantity_two = rig.batch(None, Some("SERIAL-TWO"), None).await;
    assert!(rig
        .receive(
            quantity_two,
            rig.first_location_id,
            2,
            "serial-quantity-two"
        )
        .await
        .is_err());

    let admin = admin_db_for(&rig.fixture.db).await;
    let totals: (i64, i64) = sqlx::query_as(
        r#"
        SELECT COALESCE(sum(balance.qty_on_hand),0)::BIGINT,
               (SELECT count(*) FROM inventory_transactions
                WHERE tenant_id=$1 AND idempotency_key IN
                    ('serial-race-first','serial-race-second','serial-quantity-two'))
        FROM inventory_balances balance
        JOIN item_batches batch ON batch.id=balance.item_batch_id
        WHERE balance.tenant_id=$1 AND balance.inventory_owner_id=$2
          AND batch.serial IN ('SERIAL-ONE','SERIAL-TWO') AND balance.deleted IS NULL
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(rig.inventory_owner_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(totals, (1, 1));
    admin.close().await;
}

#[tokio::test]
async fn rls_grants_immutability_and_audit_evidence_fail_closed() {
    let rig = Rig::new("rls").await;
    let created: ItemTraceabilityPolicyResponse = json_response(
        rig.configure_policy(
            "trace-policy-evidence",
            "required",
            "not_tracked",
            "required",
            Some(7),
            None,
        )
        .await,
    )
    .await;

    let mut unbound = rig.fixture.db.begin().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM item_traceability_policies")
        .fetch_one(&mut *unbound)
        .await
        .unwrap();
    assert_eq!(count, 0);
    unbound.rollback().await.unwrap();

    let admin = admin_db_for(&rig.fixture.db).await;
    let grants: Vec<bool> = sqlx::query_scalar(
        r#"
        SELECT ARRAY[
          has_table_privilege('wareboxes_app','item_traceability_policies','SELECT'),
          has_table_privilege('wareboxes_app','item_traceability_policies','INSERT'),
          has_table_privilege('wareboxes_app','item_traceability_policies','UPDATE'),
          has_table_privilege('wareboxes_app','item_traceability_policies','DELETE'),
          has_column_privilege('wareboxes_app','item_traceability_policies','effective_to','UPDATE'),
          has_column_privilege('wareboxes_app','item_traceability_policies','serial_requirement','UPDATE'),
          has_sequence_privilege('wareboxes_app','item_traceability_policies_id_seq','USAGE')
        ]
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(grants, vec![true, true, false, false, true, false, true]);
    let immutable = sqlx::query(
        "UPDATE item_traceability_policies SET serial_requirement='required' WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(created.item_traceability_policy_id)
    .execute(&admin)
    .await;
    assert!(immutable.is_err());
    let evidence: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT count(*) FROM command_idempotency_records
           WHERE tenant_id=$1
             AND operation='inventory.item_traceability_policy.configure.v1'
             AND idempotency_key='trace-policy-evidence'),
          (SELECT count(*) FROM outbox_events
           WHERE tenant_id=$1
             AND event_type='inventory.item_traceability_policy.configured'
             AND aggregate_id=$2::TEXT)
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(created.item_traceability_policy_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(evidence, (1, 1));
    admin.close().await;

    let other_user = rig
        .fixture
        .wms_user("item-traceability-cross-tenant@test.local")
        .await;
    let other_tenant_id = tenant_for_user(&rig.fixture.db, other_user.id).await;
    grant_supervisor(&rig.fixture, other_tenant_id, other_user.id, "cross-tenant").await;
    let other_token = wareboxes_api::auth::create_session(&rig.fixture.db, other_user.id)
        .await
        .unwrap();
    let cross_tenant = rig
        .send_as(
            &other_token,
            other_tenant_id,
            Method::POST,
            "/api/v1/item-traceability-policies",
            Some("cross-tenant-guessed-policy"),
            Some(json!({
                "inventory_owner_id": rig.inventory_owner_id,
                "facility_id": rig.facility_id,
                "item_id": rig.item_id,
                "uom": "case",
                "lot": "required",
                "serial": "not_tracked",
                "expiration": "required",
                "minimum_shelf_life_days": 7,
                "expected_revision": null
            })),
        )
        .await;
    assert_eq!(cross_tenant.status(), StatusCode::NOT_FOUND);

    rig.set_scope(Vec::new(), Vec::new()).await;
    let revoked_replay = rig
        .configure_policy(
            "trace-policy-evidence",
            "required",
            "not_tracked",
            "required",
            Some(7),
            None,
        )
        .await;
    assert_eq!(revoked_replay.status(), StatusCode::NOT_FOUND);
    let revoked_changed_replay = rig
        .configure_policy(
            "trace-policy-evidence",
            "required",
            "not_tracked",
            "required",
            Some(8),
            None,
        )
        .await;
    assert_eq!(revoked_changed_replay.status(), StatusCode::NOT_FOUND);
}
