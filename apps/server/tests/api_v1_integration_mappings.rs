mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde::Serialize;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{repo, routes, state::AppState};
use wareboxes_api_contract::v1::{
    ConfigureIntegrationOrderItemMappingRequest, IntegrationOrderItemMappingPage,
    IntegrationOrderItemMappingResponse, IntegrationOrderItemMappingStatus,
    RetireIntegrationOrderItemMappingRequest, Revision,
};
use wareboxes_core::dto::UpdateUserAccessScope;

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("wareboxes_api=debug")
        .with_test_writer()
        .try_init();
}

async fn grant_admin(fixture: &Fixture, tenant_id: TenantId, user_id: i64) {
    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        &fixture.db,
        tenant_id,
        "admin",
        Some("admin"),
    )
    .await
    .unwrap();
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("integration-mapping-admin-{user_id}"),
        None,
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        permission,
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_to_user(&fixture.db, tenant_id, user_id, role)
        .await
        .unwrap();
}

async fn link_item(fixture: &Fixture, tenant_id: TenantId, owner_id: i64, item_id: i64) {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        r#"
        INSERT INTO inventory_owner_items(tenant_id,created,inventory_owner_id,item_id)
        VALUES ($1,clock_timestamp(),$2,$3)
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id)
    .bind(item_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

fn request<T: Serialize>(
    token: &str,
    tenant_id: TenantId,
    method: Method,
    uri: &str,
    key: Option<&str>,
    body: Option<&T>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    if let Some(key) = key {
        builder = builder.header(IDEMPOTENCY_KEY_HEADER, key);
    }
    let body = if let Some(body) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(body).unwrap())
    } else {
        Body::empty()
    };
    builder.body(body).unwrap()
}

async fn json_response<T: serde::de::DeserializeOwned>(
    response: axum::response::Response,
    expected: StatusCode,
) -> T {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    assert_eq!(
        status,
        expected,
        "unexpected response: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

struct Rig {
    fixture: Fixture,
    tenant_id: TenantId,
    user_id: i64,
    token: String,
    app: axum::Router,
    owner_id: i64,
    item_id: i64,
    replacement_item_id: i64,
}

impl Rig {
    async fn new() -> Self {
        let fixture = Fixture::new().await;
        let user = fixture.user("integration-mapping@test.local").await;
        let tenant_id = tenant_for_user(&fixture.db, user.id).await;
        grant_admin(&fixture, tenant_id, user.id).await;
        let owner_id = fixture
            .inventory_owner(tenant_id, "Mapped Retail Client")
            .await;
        let item_id = fixture.item(tenant_id, "Mapped Case A", "case").await;
        let replacement_item_id = fixture.item(tenant_id, "Mapped Case B", "case").await;
        link_item(&fixture, tenant_id, owner_id, item_id).await;
        link_item(&fixture, tenant_id, owner_id, replacement_item_id).await;
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
            owner_id,
            item_id,
            replacement_item_id,
        }
    }

    fn configure_body(
        &self,
        external_item_key: &str,
        item_id: i64,
        expected_revision: Option<i64>,
    ) -> ConfigureIntegrationOrderItemMappingRequest {
        ConfigureIntegrationOrderItemMappingRequest {
            inventory_owner_id: self.owner_id,
            source_key: "retail-edi".into(),
            external_item_key: external_item_key.into(),
            external_uom: "CS".into(),
            item_id,
            requested_uom: "case".into(),
            expected_revision: expected_revision.map(|value| Revision::new(value).unwrap()),
        }
    }

    async fn configure(
        &self,
        key: &str,
        body: &ConfigureIntegrationOrderItemMappingRequest,
    ) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(request(
                &self.token,
                self.tenant_id,
                Method::POST,
                "/api/v1/integration-order-item-mappings",
                Some(key),
                Some(body),
            ))
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn mapping_versions_page_retire_and_reenable_with_exact_replay() {
    init_tracing();
    let rig = Rig::new().await;
    let initial_body = rig.configure_body("CLIENT-SKU-1", rig.item_id, None);
    let initial: IntegrationOrderItemMappingResponse = json_response(
        rig.configure("mapping-initial", &initial_body).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(initial.revision.get(), 1);
    assert_eq!(initial.status, IntegrationOrderItemMappingStatus::Active);
    assert_eq!(initial.item_id, rig.item_id);

    let replay: IntegrationOrderItemMappingResponse = json_response(
        rig.configure("mapping-initial", &initial_body).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay, initial);
    let changed = rig.configure_body("CLIENT-SKU-1", rig.replacement_item_id, None);
    assert_eq!(
        rig.configure("mapping-initial", &changed).await.status(),
        StatusCode::CONFLICT
    );

    let replacement_body = rig.configure_body("CLIENT-SKU-1", rig.replacement_item_id, Some(1));
    let replacement: IntegrationOrderItemMappingResponse = json_response(
        rig.configure("mapping-replacement", &replacement_body)
            .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(replacement.revision.get(), 2);
    assert_eq!(replacement.item_id, rig.replacement_item_id);

    let second_body = rig.configure_body("CLIENT-SKU-2", rig.item_id, None);
    let second: IntegrationOrderItemMappingResponse = json_response(
        rig.configure("mapping-second", &second_body).await,
        StatusCode::OK,
    )
    .await;
    let first_page: IntegrationOrderItemMappingPage = json_response(
        rig.app
            .clone()
            .oneshot(request::<serde_json::Value>(
                &rig.token,
                rig.tenant_id,
                Method::GET,
                &format!(
                    "/api/v1/integration-order-item-mappings?inventory_owner_id={}&source_key=retail-edi&limit=1",
                    rig.owner_id
                ),
                None,
                None,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(first_page.items.len(), 1);
    let cursor = first_page.next_cursor.unwrap();
    let second_page: IntegrationOrderItemMappingPage = json_response(
        rig.app
            .clone()
            .oneshot(request::<serde_json::Value>(
                &rig.token,
                rig.tenant_id,
                Method::GET,
                &format!(
                    "/api/v1/integration-order-item-mappings?inventory_owner_id={}&source_key=retail-edi&limit=1&cursor={}",
                    rig.owner_id,
                    cursor.as_str()
                ),
                None,
                None,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(second_page.items, vec![second.clone()]);

    let retire = RetireIntegrationOrderItemMappingRequest {
        expected_revision: Revision::new(2).unwrap(),
    };
    let retired: IntegrationOrderItemMappingResponse = json_response(
        rig.app
            .clone()
            .oneshot(request(
                &rig.token,
                rig.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/integration-order-item-mappings/{}/retirements",
                    replacement.mapping_id
                ),
                Some("mapping-retire"),
                Some(&retire),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(retired.status, IntegrationOrderItemMappingStatus::Retired);
    let reenabled: IntegrationOrderItemMappingResponse = json_response(
        rig.configure("mapping-reenable", &changed).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(reenabled.revision.get(), 3);
    assert_eq!(reenabled.item_id, rig.replacement_item_id);

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let counts: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM integration_order_item_mappings),
               (SELECT COUNT(*) FROM command_idempotency_records
                WHERE operation IN ('integration.order_item_mapping.configure.v1',
                                    'integration.order_item_mapping.retire.v1')),
               (SELECT COUNT(*) FROM outbox_events
                WHERE event_type LIKE 'integration.order_item_mapping.%')
        "#,
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(counts, (4, 5, 6));

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let ordered_events: Vec<(String, i64)> = sqlx::query_as(
        r#"
        SELECT event_type,aggregate_sequence
        FROM outbox_events
        WHERE payload->'definition'->>'external_item_key'='CLIENT-SKU-1'
        ORDER BY aggregate_sequence
        "#,
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        ordered_events,
        vec![
            ("integration.order_item_mapping.configured".into(), 1),
            ("integration.order_item_mapping.retired".into(), 2),
            ("integration.order_item_mapping.configured".into(), 3),
            ("integration.order_item_mapping.retired".into(), 4),
            ("integration.order_item_mapping.configured".into(), 5),
        ]
    );
}

#[tokio::test]
async fn mapping_races_scope_and_authorization_fail_closed() {
    let rig = Rig::new().await;
    let body = rig.configure_body("RACE-SKU", rig.item_id, None);
    let (left, right) = tokio::join!(
        rig.configure("mapping-race-left", &body),
        rig.configure("mapping-race-right", &body)
    );
    let statuses = [left.status(), right.status()];
    assert_eq!(statuses.iter().filter(|&&s| s == StatusCode::OK).count(), 1);
    assert_eq!(
        statuses
            .iter()
            .filter(|&&s| s == StatusCode::CONFLICT)
            .count(),
        1
    );

    let no_admin = rig.fixture.user("mapping-no-admin@test.local").await;
    let no_admin_token = wareboxes_api::auth::create_session(&rig.fixture.db, no_admin.id)
        .await
        .unwrap();
    assert_eq!(
        rig.app
            .clone()
            .oneshot(request(
                &no_admin_token,
                rig.tenant_id,
                Method::POST,
                "/api/v1/integration-order-item-mappings",
                Some("mapping-denied"),
                Some(&body),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );

    assert!(repo::tenants::update_user_access_scope(
        &rig.fixture.db,
        rig.tenant_id,
        &UpdateUserAccessScope {
            user_id: rig.user_id,
            all_inventory_owners: false,
            all_facilities: true,
            inventory_owner_ids: vec![],
            facility_ids: vec![],
        },
    )
    .await
    .unwrap());
    let refreshed_token = wareboxes_api::auth::create_session(&rig.fixture.db, rig.user_id)
        .await
        .unwrap();
    assert_eq!(
        rig.app
            .clone()
            .oneshot(request(
                &refreshed_token,
                rig.tenant_id,
                Method::POST,
                "/api/v1/integration-order-item-mappings",
                Some("mapping-race-left"),
                Some(&body),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        rig.app
            .clone()
            .oneshot(request(
                &refreshed_token,
                rig.tenant_id,
                Method::POST,
                "/api/v1/integration-order-item-mappings",
                Some("mapping-race-left"),
                Some(&rig.configure_body("CHANGED", rig.item_id, None)),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn mapping_ledger_is_forced_rls_minimally_granted_and_immutable() {
    let rig = Rig::new().await;
    let body = rig.configure_body("LEDGER-SKU", rig.item_id, None);
    let mapped: IntegrationOrderItemMappingResponse =
        json_response(rig.configure("mapping-ledger", &body).await, StatusCode::OK).await;
    let admin = admin_db_for(&rig.fixture.db).await;
    let rls: (bool, bool) = sqlx::query_as(
        "SELECT relrowsecurity,relforcerowsecurity FROM pg_class WHERE oid='integration_order_item_mappings'::regclass",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(rls, (true, true));
    let policy: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pg_policies WHERE tablename='integration_order_item_mappings' AND policyname='integration_order_item_mappings_tenant_isolation')",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(policy);
    let grants: (bool, bool, bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT has_table_privilege('wareboxes_app','integration_order_item_mappings','SELECT'),
               has_table_privilege('wareboxes_app','integration_order_item_mappings','INSERT'),
               has_table_privilege('wareboxes_app','integration_order_item_mappings','UPDATE'),
               has_column_privilege('wareboxes_app','integration_order_item_mappings','effective_to','UPDATE'),
               has_table_privilege('wareboxes_app','integration_order_item_mappings','DELETE'),
               has_sequence_privilege('wareboxes_app','integration_order_item_mappings_id_seq','USAGE')
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(grants, (true, true, false, true, false, true));

    let mutation = sqlx::query(
        "UPDATE integration_order_item_mappings SET external_item_key='FORGED' WHERE id=$1",
    )
    .bind(mapped.mapping_id)
    .execute(&admin)
    .await;
    assert!(mutation.is_err());
    let deletion = sqlx::query("DELETE FROM integration_order_item_mappings WHERE id=$1")
        .bind(mapped.mapping_id)
        .execute(&admin)
        .await;
    assert!(deletion.is_err());

    let other = rig.fixture.user("mapping-other-tenant@test.local").await;
    let other_tenant = tenant_for_user(&rig.fixture.db, other.id).await;
    let mut other_tx = tenant_tx(&rig.fixture.db, other_tenant).await;
    let visible: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM integration_order_item_mappings")
        .fetch_one(&mut *other_tx)
        .await
        .unwrap();
    assert_eq!(visible, 0);
    other_tx.commit().await.unwrap();
}
