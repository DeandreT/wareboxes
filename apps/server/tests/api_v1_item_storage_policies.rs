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
    ItemStoragePolicyPage, ItemStoragePolicyResponse, ItemStoragePolicyStatus, StorageZoneResponse,
};

fn init_test_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("wareboxes_api=trace")
        .with_test_writer()
        .try_init();
}

struct Rig {
    fixture: Fixture,
    tenant_id: TenantId,
    user_id: i64,
    token: String,
    app: axum::Router,
    inventory_owner_id: i64,
    facility_id: i64,
    item_id: i64,
    reserve_location_id: i64,
    pick_location_id: i64,
    shipping_location_id: i64,
    item_batch_id: i64,
}

impl Rig {
    async fn new() -> Self {
        let fixture = Fixture::new().await;
        let user = fixture.wms_user("item-storage-policy@test.local").await;
        let tenant_id = tenant_for_user(&fixture.db, user.id).await;
        grant_supervisor(&fixture, tenant_id, user.id).await;
        let inventory_owner_id = fixture.inventory_owner(tenant_id, "Storage Client").await;
        let facility_id = fixture.facility(tenant_id, "Storage Policy Facility").await;
        fixture
            .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
            .await;
        let item_id = fixture.item(tenant_id, "Storage Policy Item", "case").await;
        let item_batch_id = repo::inventory::add_item_batch(
            &fixture.db,
            tenant_id,
            inventory_owner_id,
            item_id,
            None,
            Some("POLICY-LOT"),
            None,
            None,
        )
        .await
        .unwrap();
        let reserve_location_id = wareboxes_persistence_postgres::locations::add_location(
            &fixture.db,
            tenant_id,
            facility_id,
            None,
            Some("POLICY-RESERVE"),
            Some("Policy reserve"),
            "reserve",
            true,
            false,
            false,
        )
        .await
        .unwrap();
        let pick_location_id = fixture
            .location(tenant_id, facility_id, "POLICY-PICK")
            .await;
        let shipping_location_id = wareboxes_persistence_postgres::locations::add_location(
            &fixture.db,
            tenant_id,
            facility_id,
            None,
            Some("POLICY-SHIP"),
            Some("Policy shipping"),
            "shipping",
            true,
            false,
            false,
        )
        .await
        .unwrap();
        let token = wareboxes_api::auth::create_session(&fixture.db, user.id)
            .await
            .unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));
        let rig = Self {
            fixture,
            tenant_id,
            user_id: user.id,
            token,
            app,
            inventory_owner_id,
            facility_id,
            item_id,
            reserve_location_id,
            pick_location_id,
            shipping_location_id,
            item_batch_id,
        };
        rig.configure_zone("RES-POL", "reserve", 10, reserve_location_id)
            .await;
        rig.configure_zone("PICK-POL", "pick", 20, pick_location_id)
            .await;
        rig.configure_zone("SHIP-POL", "shipping", 30, shipping_location_id)
            .await;
        rig
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        key: Option<&str>,
        body: Option<Value>,
    ) -> axum::response::Response {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
            .header(TENANT_ID_HEADER, self.tenant_id.to_string());
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

    async fn configure_zone(
        &self,
        code: &str,
        purpose: &str,
        sequence: u32,
        location_id: i64,
    ) -> StorageZoneResponse {
        let response = self
            .send(
                Method::POST,
                "/api/v1/storage-zones",
                Some(&format!("zone-{code}")),
                Some(json!({
                    "facility_id": self.facility_id,
                    "code": code,
                    "name": format!("{code} zone"),
                    "purpose": purpose,
                    "travel_sequence": sequence,
                    "location_ids": [location_id]
                })),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        json_response(response).await
    }

    async fn configure_policy(
        &self,
        key: &str,
        purposes: &[&str],
        capacity: Option<i64>,
        expected_revision: Option<i64>,
    ) -> axum::response::Response {
        self.send(
            Method::POST,
            "/api/v1/item-storage-policies",
            Some(key),
            Some(json!({
                "inventory_owner_id": self.inventory_owner_id,
                "facility_id": self.facility_id,
                "item_id": self.item_id,
                "uom": "case",
                "allowed_zone_purposes": purposes,
                "max_quantity_per_location": capacity,
                "expected_revision": expected_revision
            })),
        )
        .await
    }

    async fn receive(
        &self,
        location_id: i64,
        qty: i64,
        key: &str,
    ) -> wareboxes_api::error::AppResult<i64> {
        repo::inventory::receive_inventory(
            &self.fixture.db,
            self.tenant_id,
            self.user_id,
            self.item_batch_id,
            location_id,
            qty,
            None,
            Some("item storage policy test"),
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

async fn grant_supervisor(fixture: &Fixture, tenant_id: TenantId, user_id: i64) {
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
        "item-storage-policy-supervisor",
        Some("Item storage policy supervisor test role"),
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
async fn policy_lifecycle_enforces_compatibility_capacity_and_history() {
    init_test_tracing();
    let rig = Rig::new().await;
    rig.receive(rig.reserve_location_id, 6, "policy-receive-six")
        .await
        .unwrap();
    let response = rig
        .configure_policy("policy-create", &["reserve", "pick"], Some(10), None)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let created: ItemStoragePolicyResponse = json_response(response).await;
    assert_eq!(created.revision.get(), 1);
    assert_eq!(created.max_quantity_per_location, Some(10));
    assert_eq!(created.allowed_zone_purposes.len(), 2);
    let replay: ItemStoragePolicyResponse = json_response(
        rig.configure_policy("policy-create", &["reserve", "pick"], Some(10), None)
            .await,
    )
    .await;
    assert_eq!(replay, created);

    rig.receive(rig.reserve_location_id, 4, "policy-receive-four")
        .await
        .unwrap();
    assert!(rig
        .receive(rig.reserve_location_id, 1, "policy-over-capacity")
        .await
        .is_err());
    rig.receive(rig.pick_location_id, 1, "policy-pick-allowed")
        .await
        .unwrap();
    assert!(rig
        .receive(rig.shipping_location_id, 1, "policy-ship-denied")
        .await
        .is_err());

    let incompatible = rig
        .configure_policy("policy-drop-reserve", &["pick"], Some(12), Some(1))
        .await;
    assert_eq!(incompatible.status(), StatusCode::CONFLICT);
    let too_small = rig
        .configure_policy(
            "policy-smaller-capacity",
            &["reserve", "pick"],
            Some(9),
            Some(1),
        )
        .await;
    assert_eq!(too_small.status(), StatusCode::CONFLICT);
    let replacement: ItemStoragePolicyResponse = json_response(
        rig.configure_policy(
            "policy-reconfigure",
            &["reserve", "pick"],
            Some(12),
            Some(1),
        )
        .await,
    )
    .await;
    assert_eq!(replacement.revision.get(), 2);

    let page: ItemStoragePolicyPage = json_response(
        rig.send(
            Method::GET,
            &format!(
                "/api/v1/item-storage-policies?inventory_owner_id={}&facility_id={}&purpose=pick",
                rig.inventory_owner_id, rig.facility_id
            ),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0], replacement);

    let retired: ItemStoragePolicyResponse = json_response(
        rig.send(
            Method::POST,
            &format!(
                "/api/v1/item-storage-policies/{}/retirements",
                replacement.item_storage_policy_id
            ),
            Some("policy-retire"),
            Some(json!({"expected_revision": 2})),
        )
        .await,
    )
    .await;
    assert_eq!(retired.status, ItemStoragePolicyStatus::Retired);
    rig.receive(rig.reserve_location_id, 3, "policy-after-retirement")
        .await
        .unwrap();
    let history: ItemStoragePolicyPage = json_response(
        rig.send(
            Method::GET,
            &format!(
                "/api/v1/item-storage-policies?inventory_owner_id={}&status=retired",
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
async fn concurrent_receipts_cannot_exceed_one_location_capacity() {
    init_test_tracing();
    let rig = Rig::new().await;
    let response = rig
        .configure_policy("policy-race", &["reserve"], Some(10), None)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let (first, second) = tokio::join!(
        rig.receive(rig.reserve_location_id, 6, "capacity-race-a"),
        rig.receive(rig.reserve_location_id, 6, "capacity-race-b"),
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let admin = admin_db_for(&rig.fixture.db).await;
    let totals: (i64, i64) = sqlx::query_as(
        r#"
        SELECT COALESCE(sum(qty_on_hand),0)::BIGINT,
               (SELECT count(*) FROM inventory_transactions
                WHERE tenant_id=$1 AND idempotency_key IN ('capacity-race-a','capacity-race-b'))
        FROM inventory_balances
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND location_id=$3 AND deleted IS NULL
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(rig.inventory_owner_id)
    .bind(rig.reserve_location_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(totals, (6, 1));
    admin.close().await;
}

#[tokio::test]
async fn rls_grants_immutability_and_zone_reconfiguration_fail_closed() {
    init_test_tracing();
    let rig = Rig::new().await;
    rig.receive(rig.reserve_location_id, 2, "policy-zone-position")
        .await
        .unwrap();
    let created: ItemStoragePolicyResponse = json_response(
        rig.configure_policy("policy-evidence", &["reserve"], Some(10), None)
            .await,
    )
    .await;
    let zone_change = rig
        .send(
            Method::POST,
            "/api/v1/storage-zones",
            Some("policy-zone-change"),
            Some(json!({
                "facility_id": rig.facility_id,
                "code": "RES-POL",
                "name": "Shipping replacement",
                "purpose": "shipping",
                "travel_sequence": 10,
                "location_ids": [rig.reserve_location_id],
                "expected_revision": 1
            })),
        )
        .await;
    assert_eq!(zone_change.status(), StatusCode::CONFLICT);

    let mut unbound = rig.fixture.db.begin().await.unwrap();
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM item_storage_policies),(SELECT count(*) FROM item_storage_policy_zone_purposes)",
    )
    .fetch_one(&mut *unbound)
    .await
    .unwrap();
    assert_eq!(counts, (0, 0));
    unbound.rollback().await.unwrap();

    let admin = admin_db_for(&rig.fixture.db).await;
    let grants: Vec<bool> = sqlx::query_scalar(
        r#"
        SELECT ARRAY[
          has_table_privilege('wareboxes_app','item_storage_policies','SELECT'),
          has_table_privilege('wareboxes_app','item_storage_policies','INSERT'),
          has_table_privilege('wareboxes_app','item_storage_policies','UPDATE'),
          has_table_privilege('wareboxes_app','item_storage_policies','DELETE'),
          has_column_privilege('wareboxes_app','item_storage_policies','effective_to','UPDATE'),
          has_column_privilege('wareboxes_app','item_storage_policies','max_quantity_per_location','UPDATE'),
          has_table_privilege('wareboxes_app','item_storage_policy_zone_purposes','SELECT'),
          has_table_privilege('wareboxes_app','item_storage_policy_zone_purposes','INSERT'),
          has_table_privilege('wareboxes_app','item_storage_policy_zone_purposes','UPDATE'),
          has_table_privilege('wareboxes_app','item_storage_policy_zone_purposes','DELETE'),
          has_sequence_privilege('wareboxes_app','item_storage_policies_id_seq','USAGE')
        ]
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        grants,
        vec![true, true, false, false, true, false, true, true, false, false, true]
    );
    let immutable = sqlx::query(
        "UPDATE item_storage_policy_zone_purposes SET purpose='pick' WHERE tenant_id=$1 AND item_storage_policy_id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(created.item_storage_policy_id)
    .execute(&admin)
    .await;
    assert!(immutable.is_err());
    let evidence: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT count(*) FROM command_idempotency_records
           WHERE tenant_id=$1 AND operation='topology.item_storage_policy.configure.v1'
             AND idempotency_key='policy-evidence'),
          (SELECT count(*) FROM outbox_events
           WHERE tenant_id=$1 AND event_type='topology.item_storage_policy.configured'
             AND aggregate_id=$2::TEXT),
          (SELECT count(*) FROM item_storage_policy_zone_purposes
           WHERE tenant_id=$1 AND item_storage_policy_id=$2)
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(created.item_storage_policy_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(evidence, (1, 1, 1));
    admin.close().await;
}
