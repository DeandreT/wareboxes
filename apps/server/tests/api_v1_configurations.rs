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
    ConfigurationLifecycleRequest, ConfigurationPage, ConfigurationResponse, ConfigurationScope,
    ConfigurationSimulationResponse, ConfigurationStatus, CreateConfigurationRequest, DecisionRule,
    DecisionRuleKind, InventoryRotation, Revision, RollbackConfigurationRequest,
    SimulateConfigurationRequest,
};
use wareboxes_core::dto::UpdateUserAccessScope;

async fn add_membership(fixture: &Fixture, tenant_id: TenantId, user_id: i64) {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships(tenant_id,user_id) VALUES ($1,$2)")
        .bind(tenant_id.get())
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn grant_admin(fixture: &Fixture, tenant_id: TenantId, user_id: i64, suffix: &str) {
    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        &fixture.db,
        tenant_id,
        "admin",
        Some("Warehouse administrator"),
    )
    .await
    .unwrap();
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("configuration-admin-{suffix}"),
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

fn request<T: Serialize>(
    token: &str,
    tenant_id: TenantId,
    method: Method,
    uri: &str,
    idempotency_key: Option<&str>,
    body: Option<&T>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    if let Some(key) = idempotency_key {
        builder = builder.header(IDEMPOTENCY_KEY_HEADER, key);
    }
    let body = match body {
        Some(body) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(body).unwrap())
        }
        None => Body::empty(),
    };
    builder.body(body).unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(
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
    other_tenant_id: TenantId,
    creator_id: i64,
    creator_token: String,
    approver_id: i64,
    approver_token: String,
    owner_id: i64,
    facility_id: i64,
    app: axum::Router,
}

impl Rig {
    async fn new() -> Self {
        let fixture = Fixture::new().await;
        let creator = fixture.user("configuration-creator@test.local").await;
        let tenant_id = tenant_for_user(&fixture.db, creator.id).await;
        grant_admin(&fixture, tenant_id, creator.id, "creator").await;

        let approver = fixture.user("configuration-approver@test.local").await;
        let other_tenant_id = tenant_for_user(&fixture.db, approver.id).await;
        add_membership(&fixture, tenant_id, approver.id).await;
        grant_admin(&fixture, tenant_id, approver.id, "approver").await;

        let owner_id = fixture
            .inventory_owner(tenant_id, "Configuration Client")
            .await;
        let facility_id = fixture.facility(tenant_id, "Configuration DC").await;
        fixture
            .assign_owner_to_facility(tenant_id, owner_id, facility_id)
            .await;
        let creator_token = wareboxes_api::auth::create_session(&fixture.db, creator.id)
            .await
            .unwrap();
        let approver_token = wareboxes_api::auth::create_session(&fixture.db, approver.id)
            .await
            .unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));
        Self {
            fixture,
            tenant_id,
            other_tenant_id,
            creator_id: creator.id,
            creator_token,
            approver_id: approver.id,
            approver_token,
            owner_id,
            facility_id,
            app,
        }
    }

    async fn send<T: Serialize>(
        &self,
        token: &str,
        method: Method,
        uri: &str,
        key: Option<&str>,
        body: Option<&T>,
    ) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(request(token, self.tenant_id, method, uri, key, body))
            .await
            .unwrap()
    }

    async fn create(
        &self,
        key: &str,
        scope: ConfigurationScope,
        rule: DecisionRule,
        expected_revision: Option<i64>,
    ) -> axum::response::Response {
        self.send(
            &self.creator_token,
            Method::POST,
            "/api/v1/configurations",
            Some(key),
            Some(&CreateConfigurationRequest {
                scope,
                effective_from: "2026-01-01T00:00:00Z".into(),
                effective_until: None,
                rule,
                expected_revision: expected_revision
                    .map(|revision| Revision::new(revision).unwrap()),
            }),
        )
        .await
    }

    async fn transition(
        &self,
        token: &str,
        configuration_id: i64,
        transition: &str,
        revision: i64,
        key: &str,
    ) -> axum::response::Response {
        self.send(
            token,
            Method::POST,
            &format!("/api/v1/configurations/{configuration_id}/{transition}"),
            Some(key),
            Some(&ConfigurationLifecycleRequest {
                expected_revision: Revision::new(revision).unwrap(),
            }),
        )
        .await
    }

    async fn activate(
        &self,
        scope: ConfigurationScope,
        rule: DecisionRule,
        prefix: &str,
    ) -> ConfigurationResponse {
        let created: ConfigurationResponse = response_json(
            self.create(&format!("{prefix}-create"), scope, rule, None)
                .await,
            StatusCode::OK,
        )
        .await;
        let submitted: ConfigurationResponse = response_json(
            self.transition(
                &self.creator_token,
                created.configuration_id,
                "submissions",
                created.revision.get(),
                &format!("{prefix}-submit"),
            )
            .await,
            StatusCode::OK,
        )
        .await;
        let approved: ConfigurationResponse = response_json(
            self.transition(
                &self.approver_token,
                created.configuration_id,
                "approvals",
                submitted.revision.get(),
                &format!("{prefix}-approve"),
            )
            .await,
            StatusCode::OK,
        )
        .await;
        response_json(
            self.transition(
                &self.creator_token,
                created.configuration_id,
                "activations",
                approved.revision.get(),
                &format!("{prefix}-activate"),
            )
            .await,
            StatusCode::OK,
        )
        .await
    }
}

fn tenant_allocation_rule() -> DecisionRule {
    DecisionRule::Allocation {
        rotation: InventoryRotation::Fifo,
        allow_partial: false,
        require_complete_line: true,
    }
}

fn owner_facility_allocation_rule() -> DecisionRule {
    DecisionRule::Allocation {
        rotation: InventoryRotation::Fefo,
        allow_partial: true,
        require_complete_line: false,
    }
}

#[tokio::test]
async fn configuration_lifecycle_inheritance_replay_rollback_and_page_are_complete() {
    let rig = Rig::new().await;
    let tenant = rig
        .activate(
            ConfigurationScope::Tenant,
            tenant_allocation_rule(),
            "tenant-allocation",
        )
        .await;
    assert_eq!(tenant.status, ConfigurationStatus::Active);
    assert_eq!(tenant.revision.get(), 4);

    let owner_facility = rig
        .activate(
            ConfigurationScope::OwnerFacility {
                inventory_owner_id: rig.owner_id,
                facility_id: rig.facility_id,
            },
            owner_facility_allocation_rule(),
            "owner-facility-allocation",
        )
        .await;
    assert_eq!(owner_facility.status, ConfigurationStatus::Active);
    assert_eq!(owner_facility.approved_by, Some(rig.approver_id));

    let replay: ConfigurationResponse = response_json(
        rig.transition(
            &rig.creator_token,
            owner_facility.configuration_id,
            "activations",
            3,
            "owner-facility-allocation-activate",
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay, owner_facility);

    let simulation: ConfigurationSimulationResponse = response_json(
        rig.send(
            &rig.creator_token,
            Method::POST,
            "/api/v1/configuration-simulations",
            None,
            Some(&SimulateConfigurationRequest {
                kind: DecisionRuleKind::Allocation,
                inventory_owner_id: rig.owner_id,
                facility_id: rig.facility_id,
                effective_at: "2026-08-12T12:00:00Z".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(simulation.evaluated_candidate_count, 2);
    let matched = simulation.matched_configuration.unwrap();
    assert_eq!(matched.configuration_id, owner_facility.configuration_id);
    assert_eq!(matched.rule, owner_facility_allocation_rule());

    let rollback_body = RollbackConfigurationRequest {
        expected_source_revision: owner_facility.revision,
        effective_from: "2026-09-01T00:00:00Z".into(),
        effective_until: None,
    };
    let rollback: ConfigurationResponse = response_json(
        rig.send(
            &rig.creator_token,
            Method::POST,
            &format!(
                "/api/v1/configurations/{}/rollbacks",
                owner_facility.configuration_id
            ),
            Some("owner-facility-rollback"),
            Some(&rollback_body),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(rollback.status, ConfigurationStatus::Draft);
    assert_eq!(rollback.rule, owner_facility.rule);
    assert_eq!(
        rollback.rollback_of_configuration_id,
        Some(owner_facility.configuration_id)
    );
    let rollback_replay: ConfigurationResponse = response_json(
        rig.send(
            &rig.creator_token,
            Method::POST,
            &format!(
                "/api/v1/configurations/{}/rollbacks",
                owner_facility.configuration_id
            ),
            Some("owner-facility-rollback"),
            Some(&rollback_body),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(rollback_replay, rollback);

    let first_page: ConfigurationPage = response_json(
        rig.send::<serde_json::Value>(
            &rig.creator_token,
            Method::GET,
            "/api/v1/configurations?kind=allocation&limit=1",
            None,
            None,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(first_page.items, vec![rollback]);
    let cursor = first_page.next_cursor.unwrap();
    let second_page: ConfigurationPage = response_json(
        rig.send::<serde_json::Value>(
            &rig.creator_token,
            Method::GET,
            &format!(
                "/api/v1/configurations?kind=allocation&limit=1&cursor={}",
                cursor.as_str()
            ),
            None,
            None,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(second_page.items, vec![owner_facility]);

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let outbox_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox_events WHERE tenant_id=$1 AND aggregate_type='configuration_version'",
    )
    .bind(rig.tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(outbox_events, 9);
}

#[tokio::test]
async fn approval_separation_races_permissions_and_scope_replays_fail_closed() {
    let rig = Rig::new().await;
    let body_scope = ConfigurationScope::InventoryOwner {
        inventory_owner_id: rig.owner_id,
    };
    let created: ConfigurationResponse = response_json(
        rig.create(
            "scope-create",
            body_scope,
            DecisionRule::Pick {
                require_source_location_scan: true,
                require_item_scan: true,
                require_destination_container_scan: true,
            },
            None,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let submitted: ConfigurationResponse = response_json(
        rig.transition(
            &rig.creator_token,
            created.configuration_id,
            "submissions",
            1,
            "scope-submit",
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        rig.transition(
            &rig.creator_token,
            created.configuration_id,
            "approvals",
            submitted.revision.get(),
            "scope-self-approve",
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    let race_rule = DecisionRule::Wave {
        max_orders: 100,
        require_complete_allocation: true,
    };
    let (left, right) = tokio::join!(
        rig.create(
            "race-left",
            ConfigurationScope::Tenant,
            race_rule.clone(),
            None
        ),
        rig.create("race-right", ConfigurationScope::Tenant, race_rule, None)
    );
    let statuses = [left.status(), right.status()];
    assert_eq!(
        statuses
            .iter()
            .filter(|&&status| status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|&&status| status == StatusCode::CONFLICT)
            .count(),
        1
    );

    let unauthorized = rig.fixture.user("configuration-viewer@test.local").await;
    add_membership(&rig.fixture, rig.tenant_id, unauthorized.id).await;
    let unauthorized_token = wareboxes_api::auth::create_session(&rig.fixture.db, unauthorized.id)
        .await
        .unwrap();
    assert_eq!(
        rig.send(
            &unauthorized_token,
            Method::POST,
            "/api/v1/configuration-simulations",
            None,
            Some(&SimulateConfigurationRequest {
                kind: DecisionRuleKind::Pick,
                inventory_owner_id: rig.owner_id,
                facility_id: rig.facility_id,
                effective_at: "2026-08-12T12:00:00Z".into(),
            }),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    assert!(repo::tenants::update_user_access_scope(
        &rig.fixture.db,
        rig.tenant_id,
        &UpdateUserAccessScope {
            user_id: rig.creator_id,
            all_inventory_owners: false,
            all_facilities: true,
            inventory_owner_ids: vec![],
            facility_ids: vec![],
        },
    )
    .await
    .unwrap());
    let refreshed_token = wareboxes_api::auth::create_session(&rig.fixture.db, rig.creator_id)
        .await
        .unwrap();
    assert_eq!(
        rig.send::<serde_json::Value>(
            &refreshed_token,
            Method::GET,
            &format!("/api/v1/configurations?inventory_owner_id={}", rig.owner_id),
            None,
            None,
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        rig.send(
            &refreshed_token,
            Method::POST,
            "/api/v1/configurations",
            Some("scope-create"),
            Some(&CreateConfigurationRequest {
                scope: body_scope,
                effective_from: "2026-01-01T00:00:00Z".into(),
                effective_until: None,
                rule: DecisionRule::Pick {
                    require_source_location_scan: true,
                    require_item_scan: true,
                    require_destination_container_scan: true,
                },
                expected_revision: None,
            }),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn database_enforces_exact_types_immutability_minimal_grants_and_rls() {
    let rig = Rig::new().await;
    let mut invalid = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let invalid_insert = sqlx::query(
        r#"
        INSERT INTO configuration_versions
          (tenant_id,kind,scope_level,revision,status,effective_from,definition,created_by_user_id)
        VALUES ($1,'pick','tenant',1,'draft','2026-01-01T00:00:00Z',
                '{"kind":"pick","require_item_scan":true}'::jsonb,$2)
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(rig.creator_id)
    .execute(&mut *invalid)
    .await;
    assert!(invalid_insert.is_err());
    invalid.rollback().await.unwrap();

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let configuration_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO configuration_versions
          (tenant_id,kind,scope_level,revision,status,effective_from,definition,created_by_user_id)
        VALUES ($1,'pick','tenant',1,'draft','2026-01-01T00:00:00Z',
                '{"kind":"pick","require_source_location_scan":true,
                  "require_item_scan":true,"require_destination_container_scan":true}'::jsonb,$2)
        RETURNING id
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(rig.creator_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let grants: (bool, bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT has_table_privilege(current_user,'configuration_versions','SELECT'),
               has_table_privilege(current_user,'configuration_versions','INSERT'),
               has_table_privilege(current_user,'configuration_versions','DELETE'),
               has_column_privilege(current_user,'configuration_versions','status','UPDATE'),
               has_column_privilege(current_user,'configuration_versions','definition','UPDATE')
        "#,
    )
    .fetch_one(&rig.fixture.db)
    .await
    .unwrap();
    assert_eq!(grants, (true, true, false, true, false));

    let mut immutable = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    assert!(
        sqlx::query("UPDATE configuration_versions SET definition=definition WHERE id=$1")
            .bind(configuration_id)
            .execute(&mut *immutable)
            .await
            .is_err()
    );
    immutable.rollback().await.unwrap();
    let mut undeletable = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    assert!(
        sqlx::query("DELETE FROM configuration_versions WHERE id=$1")
            .bind(configuration_id)
            .execute(&mut *undeletable)
            .await
            .is_err()
    );
    undeletable.rollback().await.unwrap();

    let mut other_tenant = tenant_tx(&rig.fixture.db, rig.other_tenant_id).await;
    let visible: i64 =
        sqlx::query_scalar("SELECT count(*) FROM configuration_versions WHERE id=$1")
            .bind(configuration_id)
            .fetch_one(&mut *other_tenant)
            .await
            .unwrap();
    other_tenant.commit().await.unwrap();
    assert_eq!(visible, 0);
}
