use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use serde::Serialize;
use serde_json::Value;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{auth, repo, routes, state::AppState};
use wareboxes_api_contract::v1::{
    ConfigurationLifecycleRequest, ConfigurationResponse, ConfigurationScope,
    CreateConfigurationRequest, DecisionRule, ExpectedReceivingSessionResponse,
    ReceiptPolicyResponse, ReceiptPolicySource, Revision, UnexpectedReceiptConfirmationResponse,
};

use super::common::*;
use super::{body, command_request, response_json, setup};

struct PolicyRig {
    fixture: Fixture,
    setup: super::Setup,
    operator_token: String,
    approver_token: String,
    app: axum::Router,
}

impl PolicyRig {
    async fn new(suffix: &str) -> Self {
        let fixture = Fixture::new().await;
        let setup = setup(&fixture, &format!("unexpected-policy-{suffix}@test.local")).await;
        grant_admin(&fixture, setup.tenant_id, setup.operator_id, suffix).await;

        let approver = fixture
            .user(&format!("unexpected-policy-approver-{suffix}@test.local"))
            .await;
        let mut tx = tenant_tx(&fixture.db, setup.tenant_id).await;
        sqlx::query("INSERT INTO tenant_memberships(tenant_id,user_id) VALUES ($1,$2)")
            .bind(setup.tenant_id.get())
            .bind(approver.id)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        grant_admin(
            &fixture,
            setup.tenant_id,
            approver.id,
            &format!("{suffix}-approver"),
        )
        .await;

        let operator_token = auth::create_session(&fixture.db, setup.operator_id)
            .await
            .unwrap();
        let approver_token = auth::create_session(&fixture.db, approver.id)
            .await
            .unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));
        Self {
            fixture,
            setup,
            operator_token,
            approver_token,
            app,
        }
    }

    async fn send<T: Serialize>(
        &self,
        token: &str,
        method: Method,
        path: &str,
        key: Option<&str>,
        body: Option<&T>,
    ) -> axum::response::Response {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(TENANT_ID_HEADER, self.setup.tenant_id.to_string());
        if let Some(key) = key {
            builder = builder.header(IDEMPOTENCY_KEY_HEADER, key);
        }
        let request = if let Some(body) = body {
            builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(body).unwrap()))
                .unwrap()
        } else {
            builder.body(Body::empty()).unwrap()
        };
        self.app.clone().oneshot(request).await.unwrap()
    }

    async fn transition(
        &self,
        token: &str,
        configuration_id: i64,
        transition: &str,
        revision: Revision,
        key: &str,
    ) -> ConfigurationResponse {
        let response = self
            .send(
                token,
                Method::POST,
                &format!("/api/v1/configurations/{configuration_id}/{transition}"),
                Some(key),
                Some(&ConfigurationLifecycleRequest {
                    expected_revision: revision,
                }),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        response_json(response).await
    }

    async fn activate_policy(
        &self,
        prefix: &str,
        allow_unexpected: bool,
        quarantine_unmapped_items: bool,
        tolerance: u16,
        expected_revision: Option<Revision>,
    ) -> ConfigurationResponse {
        let created_response = self
            .send(
                &self.operator_token,
                Method::POST,
                "/api/v1/configurations",
                Some(&format!("{prefix}-create")),
                Some(&CreateConfigurationRequest {
                    scope: ConfigurationScope::OwnerFacility {
                        inventory_owner_id: self.setup.owner_id,
                        facility_id: self.setup.facility_id,
                    },
                    effective_from: "2026-01-01T00:00:00Z".into(),
                    effective_until: None,
                    rule: DecisionRule::Receipt {
                        allow_unexpected,
                        quarantine_unmapped_items,
                        over_receipt_tolerance_basis_points: tolerance,
                    },
                    expected_revision,
                }),
            )
            .await;
        assert_eq!(created_response.status(), StatusCode::OK);
        let created: ConfigurationResponse = response_json(created_response).await;
        let submitted = self
            .transition(
                &self.operator_token,
                created.configuration_id,
                "submissions",
                created.revision,
                &format!("{prefix}-submit"),
            )
            .await;
        let approved = self
            .transition(
                &self.approver_token,
                created.configuration_id,
                "approvals",
                submitted.revision,
                &format!("{prefix}-approve"),
            )
            .await;
        self.transition(
            &self.operator_token,
            created.configuration_id,
            "activations",
            approved.revision,
            &format!("{prefix}-activate"),
        )
        .await
    }

    async fn session(&self) -> ExpectedReceivingSessionResponse {
        let response = self
            .send::<Value>(
                &self.operator_token,
                Method::GET,
                &format!("/api/v1/expected-receiving/loads/{}", self.setup.load_id),
                None,
                None,
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        response_json(response).await
    }
}

fn with_expected_policy(mut receipt: Value, policy: &ReceiptPolicyResponse) -> Value {
    receipt["expected_policy"] = serde_json::to_value(policy.expectation()).unwrap();
    receipt
}

#[tokio::test]
async fn receipt_policy_controls_mapping_disable_and_replay_with_frozen_evidence() {
    let rig = PolicyRig::new("lifecycle").await;
    let active = rig
        .activate_policy("mapping", true, false, 5_000, None)
        .await;
    let session = rig.session().await;
    assert_eq!(
        session.receipt_policy.source,
        ReceiptPolicySource::Configuration
    );
    assert_eq!(
        session.receipt_policy.configuration_id,
        Some(active.configuration_id)
    );
    assert!(!session.receipt_policy.quarantine_unmapped_items);

    let stale_default = rig
        .app
        .clone()
        .oneshot(command_request(
            &rig.operator_token,
            rig.setup.tenant_id,
            rig.setup.load_id,
            "receipt-policy-stale-default",
            &body("UNEXPECTED-CASE-01", "unexpected_item", 1),
        ))
        .await
        .unwrap();
    assert_eq!(stale_default.status(), StatusCode::CONFLICT);

    let exact_request = with_expected_policy(
        body("UNEXPECTED-CASE-01", "unexpected_item", 1),
        &session.receipt_policy,
    );
    let unmapped = rig
        .app
        .clone()
        .oneshot(command_request(
            &rig.operator_token,
            rig.setup.tenant_id,
            rig.setup.load_id,
            "receipt-policy-unmapped",
            &exact_request,
        ))
        .await
        .unwrap();
    assert_eq!(unmapped.status(), StatusCode::CONFLICT);

    repo::items::add_inventory_owner_item(
        &rig.fixture.db,
        rig.setup.tenant_id,
        rig.setup.owner_id,
        rig.setup.unexpected_item_id,
    )
    .await
    .unwrap();
    let accepted = rig
        .app
        .clone()
        .oneshot(command_request(
            &rig.operator_token,
            rig.setup.tenant_id,
            rig.setup.load_id,
            "receipt-policy-accepted",
            &exact_request,
        ))
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);
    let accepted: UnexpectedReceiptConfirmationResponse = response_json(accepted).await;
    assert_eq!(accepted.receipt_policy, session.receipt_policy);

    let mut tx = tenant_tx(&rig.fixture.db, rig.setup.tenant_id).await;
    let evidence: (String, Option<i64>, bool, String) = sqlx::query_as(
        "SELECT policy_source,policy_configuration_id,owner_item_was_preexisting,policy_hash FROM unexpected_receipts WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.setup.tenant_id.get())
    .bind(accepted.unexpected_receipt_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM outbox_events WHERE tenant_id=$1 AND event_type='inbound.unexpected_receipt.confirmed' AND payload->'result'->>'unexpected_receipt_id'=$2",
    )
    .bind(rig.setup.tenant_id.get())
    .bind(accepted.unexpected_receipt_id.to_string())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(evidence.0, "configuration");
    assert_eq!(evidence.1, Some(active.configuration_id));
    assert!(evidence.2);
    assert_eq!(evidence.3, accepted.receipt_policy.policy_hash);
    assert_eq!(event_count, 1);

    let disabled = rig
        .activate_policy("disabled", false, true, 10_000, Some(active.revision))
        .await;
    let disabled_session = rig.session().await;
    assert_eq!(
        disabled_session.receipt_policy.configuration_id,
        Some(disabled.configuration_id)
    );
    assert!(!disabled_session.receipt_policy.allow_unexpected);

    let replay = rig
        .app
        .clone()
        .oneshot(command_request(
            &rig.operator_token,
            rig.setup.tenant_id,
            rig.setup.load_id,
            "receipt-policy-accepted",
            &exact_request,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<UnexpectedReceiptConfirmationResponse>(replay).await,
        accepted
    );

    let disabled_request = with_expected_policy(
        body("UNEXPECTED-CASE-01", "unexpected_item", 1),
        &disabled_session.receipt_policy,
    );
    let blocked = rig
        .app
        .clone()
        .oneshot(command_request(
            &rig.operator_token,
            rig.setup.tenant_id,
            rig.setup.load_id,
            "receipt-policy-disabled",
            &disabled_request,
        ))
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn concurrent_excess_receipts_cannot_cross_the_effective_tolerance() {
    let rig = PolicyRig::new("concurrency").await;
    rig.activate_policy("tolerance", true, true, 5_000, None)
        .await;
    let session = rig.session().await;
    let request = with_expected_policy(
        body("EXPECTED-CASE-01", "excess", 1),
        &session.receipt_policy,
    );
    let first = rig.app.clone().oneshot(command_request(
        &rig.operator_token,
        rig.setup.tenant_id,
        rig.setup.load_id,
        "receipt-policy-race-a",
        &request,
    ));
    let second = rig.app.clone().oneshot(command_request(
        &rig.operator_token,
        rig.setup.tenant_id,
        rig.setup.load_id,
        "receipt-policy-race-b",
        &request,
    ));
    let (first, second) = tokio::join!(first, second);
    let mut statuses = [first.unwrap().status(), second.unwrap().status()];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::OK, StatusCode::CONFLICT]);

    let mut tx = tenant_tx(&rig.fixture.db, rig.setup.tenant_id).await;
    let (quantity, events): (i64, i64) = sqlx::query_as(
        r#"
        SELECT COALESCE(SUM(receipt.quantity),0)::bigint,
          (SELECT COUNT(*) FROM outbox_events event
           WHERE event.tenant_id=$1 AND event.event_type='inbound.unexpected_receipt.confirmed')
        FROM unexpected_receipts receipt
        WHERE receipt.tenant_id=$1 AND receipt.load_id=$2 AND receipt.reason_code='excess'
        "#,
    )
    .bind(rig.setup.tenant_id.get())
    .bind(rig.setup.load_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(quantity, 1);
    assert_eq!(events, 1);
}

async fn grant_admin(fixture: &Fixture, tenant_id: TenantId, user_id: i64, suffix: &str) {
    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        &fixture.db,
        tenant_id,
        "admin",
        Some("Configure receipt policy"),
    )
    .await
    .unwrap();
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("receipt-policy-admin-{suffix}-{user_id}"),
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
