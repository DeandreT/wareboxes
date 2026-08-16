use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use serde::Serialize;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    ConfigurationLifecycleRequest, ConfigurationResponse, ConfigurationScope,
    CreateConfigurationRequest, DecisionRule, InventoryRotation, OrderAllocationReadinessResponse,
    PlanOrderAllocationRequest, PlanOrderAllocationResponse, Revision,
};

use super::common::*;

pub(crate) struct Rig {
    pub(crate) fixture: Fixture,
    pub(crate) tenant_id: TenantId,
    pub(crate) operator_id: i64,
    pub(crate) operator_token: String,
    pub(crate) approver_token: String,
    pub(crate) owner_id: i64,
    pub(crate) facility_id: i64,
    pub(crate) app: axum::Router,
}

impl Rig {
    pub(crate) async fn new(suffix: &str) -> Self {
        let fixture = Fixture::new().await;
        let operator = fixture
            .user(&format!("allocation-policy-{suffix}@test.local"))
            .await;
        let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
        grant_permission(&fixture, tenant_id, operator.id, "orders", suffix).await;
        grant_permission(&fixture, tenant_id, operator.id, "admin", suffix).await;

        let approver = fixture
            .user(&format!("allocation-policy-approver-{suffix}@test.local"))
            .await;
        let mut tx = tenant_tx(&fixture.db, tenant_id).await;
        sqlx::query("INSERT INTO tenant_memberships(tenant_id,user_id) VALUES ($1,$2)")
            .bind(tenant_id.get())
            .bind(approver.id)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        grant_permission(
            &fixture,
            tenant_id,
            approver.id,
            "admin",
            &format!("{suffix}-approve"),
        )
        .await;

        let owner_id = fixture
            .inventory_owner(tenant_id, &format!("Allocation Policy Client {suffix}"))
            .await;
        let facility_id = fixture
            .facility(tenant_id, &format!("Allocation Policy DC {suffix}"))
            .await;
        fixture
            .assign_owner_to_facility(tenant_id, owner_id, facility_id)
            .await;
        let operator_token = wareboxes_api::auth::create_session(&fixture.db, operator.id)
            .await
            .unwrap();
        let approver_token = wareboxes_api::auth::create_session(&fixture.db, approver.id)
            .await
            .unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));
        Self {
            fixture,
            tenant_id,
            operator_id: operator.id,
            operator_token,
            approver_token,
            owner_id,
            facility_id,
            app,
        }
    }

    pub(crate) async fn send<T: Serialize>(
        &self,
        token: &str,
        method: Method,
        path: &str,
        key: Option<&str>,
        body: Option<&T>,
    ) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(request(token, self.tenant_id, method, path, key, body))
            .await
            .unwrap()
    }

    pub(crate) async fn create_policy(
        &self,
        prefix: &str,
        rotation: InventoryRotation,
        allow_partial: bool,
        require_complete_line: bool,
    ) -> ConfigurationResponse {
        response_json(
            self.send(
                &self.operator_token,
                Method::POST,
                "/api/v1/configurations",
                Some(&format!("{prefix}-create")),
                Some(&CreateConfigurationRequest {
                    scope: ConfigurationScope::OwnerFacility {
                        inventory_owner_id: self.owner_id,
                        facility_id: self.facility_id,
                    },
                    effective_from: "2026-01-01T00:00:00Z".into(),
                    effective_until: None,
                    rule: DecisionRule::Allocation {
                        rotation,
                        allow_partial,
                        require_complete_line,
                    },
                    expected_revision: None,
                }),
            )
            .await,
            StatusCode::OK,
        )
        .await
    }

    pub(crate) async fn transition_policy(
        &self,
        token: &str,
        configuration_id: i64,
        transition: &str,
        revision: i64,
        key: &str,
    ) -> ConfigurationResponse {
        response_json(
            self.send(
                token,
                Method::POST,
                &format!("/api/v1/configurations/{configuration_id}/{transition}"),
                Some(key),
                Some(&ConfigurationLifecycleRequest {
                    expected_revision: Revision::new(revision).unwrap(),
                }),
            )
            .await,
            StatusCode::OK,
        )
        .await
    }

    pub(crate) async fn approve_policy(
        &self,
        prefix: &str,
        rotation: InventoryRotation,
        allow_partial: bool,
        require_complete_line: bool,
    ) -> ConfigurationResponse {
        let created = self
            .create_policy(prefix, rotation, allow_partial, require_complete_line)
            .await;
        let submitted = self
            .transition_policy(
                &self.operator_token,
                created.configuration_id,
                "submissions",
                created.revision.get(),
                &format!("{prefix}-submit"),
            )
            .await;
        self.transition_policy(
            &self.approver_token,
            created.configuration_id,
            "approvals",
            submitted.revision.get(),
            &format!("{prefix}-approve"),
        )
        .await
    }

    pub(crate) async fn activate_approved(
        &self,
        prefix: &str,
        approved: &ConfigurationResponse,
    ) -> ConfigurationResponse {
        self.transition_policy(
            &self.operator_token,
            approved.configuration_id,
            "activations",
            approved.revision.get(),
            &format!("{prefix}-activate"),
        )
        .await
    }

    pub(crate) async fn activate_policy(
        &self,
        prefix: &str,
        rotation: InventoryRotation,
        allow_partial: bool,
        require_complete_line: bool,
    ) -> ConfigurationResponse {
        let approved = self
            .approve_policy(prefix, rotation, allow_partial, require_complete_line)
            .await;
        self.activate_approved(prefix, &approved).await
    }

    pub(crate) async fn order(&self, key: &str, lines: &[(i64, i64)]) -> i64 {
        let order_id = self
            .fixture
            .order_header(self.tenant_id, key, self.owner_id)
            .await;
        for (item_id, quantity) in lines {
            self.fixture
                .order_item(self.tenant_id, order_id, *item_id, *quantity)
                .await;
        }
        order_id
    }

    pub(crate) async fn readiness(&self, order_id: i64) -> OrderAllocationReadinessResponse {
        response_json(
            self.send::<()>(
                &self.operator_token,
                Method::GET,
                &format!(
                    "/api/v1/orders/{order_id}/allocation-readiness?facility_id={}",
                    self.facility_id
                ),
                None,
                None,
            )
            .await,
            StatusCode::OK,
        )
        .await
    }

    pub(crate) async fn plan(
        &self,
        order_id: i64,
        key: &str,
        request: &PlanOrderAllocationRequest,
    ) -> axum::response::Response {
        self.send(
            &self.operator_token,
            Method::POST,
            &format!("/api/v1/orders/{order_id}/allocation-runs"),
            Some(key),
            Some(request),
        )
        .await
    }
}

pub(crate) async fn response_json<T: serde::de::DeserializeOwned>(
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

fn request<T: Serialize>(
    token: &str,
    tenant_id: TenantId,
    method: Method,
    path: &str,
    key: Option<&str>,
    body: Option<&T>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    if let Some(key) = key {
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

async fn grant_permission(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    permission_name: &str,
    suffix: &str,
) {
    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        &fixture.db,
        tenant_id,
        permission_name,
        Some(permission_name),
    )
    .await
    .unwrap();
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("allocation-policy-{permission_name}-{suffix}"),
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

pub(crate) fn plan_request(
    readiness: &OrderAllocationReadinessResponse,
) -> PlanOrderAllocationRequest {
    PlanOrderAllocationRequest {
        facility_id: readiness.facility_id,
        expected_revision: readiness.revision,
        expected_policy: readiness.policy.reference(),
    }
}

pub(crate) async fn successful_plan(
    rig: &Rig,
    order_id: i64,
    key: &str,
    request: &PlanOrderAllocationRequest,
) -> PlanOrderAllocationResponse {
    response_json(rig.plan(order_id, key, request).await, StatusCode::OK).await
}
