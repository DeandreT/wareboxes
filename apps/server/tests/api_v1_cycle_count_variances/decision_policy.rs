use super::*;
use serde::Serialize;
use wareboxes_api_contract::v1::{
    ConfigurationLifecycleRequest, ConfigurationResponse, ConfigurationScope,
    CountDecisionPolicySource, CreateConfigurationRequest, DecisionRule, Revision,
};

struct ActiveCountConfiguration {
    response: ConfigurationResponse,
}

async fn activate_count_configuration(
    rig: &Rig,
    scope: ConfigurationScope,
    absolute_tolerance: i64,
    approval_threshold: i64,
    prefix: &str,
) -> ActiveCountConfiguration {
    grant_permission(
        &rig.fixture,
        rig.tenant_id,
        rig.user_id,
        "admin",
        &format!("{prefix}-creator-admin"),
    )
    .await;
    let approver = rig
        .fixture
        .user(&format!("count-policy-{prefix}-approver@test.local"))
        .await;
    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships(tenant_id,user_id) VALUES ($1,$2)")
        .bind(rig.tenant_id.get())
        .bind(approver.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    grant_permission(
        &rig.fixture,
        rig.tenant_id,
        approver.id,
        "admin",
        &format!("{prefix}-approver-admin"),
    )
    .await;
    let approver_token = auth::create_session(&rig.fixture.db, approver.id)
        .await
        .unwrap();

    let created: ConfigurationResponse = response_json(
        expect_status(
            send_with_token(
                rig,
                &rig.token,
                Method::POST,
                "/api/v1/configurations",
                Some(&format!("{prefix}-create")),
                Some(&CreateConfigurationRequest {
                    scope,
                    effective_from: "2026-01-01T00:00:00Z".into(),
                    effective_until: None,
                    rule: DecisionRule::Count {
                        absolute_tolerance,
                        percentage_tolerance_basis_points: 0,
                        approval_threshold,
                    },
                    expected_revision: None,
                }),
            )
            .await,
            StatusCode::OK,
        )
        .await,
    )
    .await;
    let submitted = transition(
        rig,
        &rig.token,
        created.configuration_id,
        "submissions",
        created.revision,
        &format!("{prefix}-submit"),
    )
    .await;
    let approved = transition(
        rig,
        &approver_token,
        created.configuration_id,
        "approvals",
        submitted.revision,
        &format!("{prefix}-approve"),
    )
    .await;
    let response = transition(
        rig,
        &rig.token,
        created.configuration_id,
        "activations",
        approved.revision,
        &format!("{prefix}-activate"),
    )
    .await;
    ActiveCountConfiguration { response }
}

async fn transition(
    rig: &Rig,
    token: &str,
    configuration_id: i64,
    action: &str,
    revision: Revision,
    key: &str,
) -> ConfigurationResponse {
    response_json(
        expect_status(
            send_with_token(
                rig,
                token,
                Method::POST,
                &format!("/api/v1/configurations/{configuration_id}/{action}"),
                Some(key),
                Some(&ConfigurationLifecycleRequest {
                    expected_revision: revision,
                }),
            )
            .await,
            StatusCode::OK,
        )
        .await,
    )
    .await
}

async fn send_with_token<T: Serialize>(
    rig: &Rig,
    token: &str,
    method: Method,
    path: &str,
    key: Option<&str>,
    body: Option<&T>,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, rig.tenant_id.to_string());
    if let Some(key) = key {
        request = request.header(IDEMPOTENCY_KEY_HEADER, key);
    }
    let body = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(body).unwrap())
        }
        None => Body::empty(),
    };
    rig.app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn configured_threshold_routes_directly_to_approval_and_freezes_evidence() {
    let rig = Rig::new("configured-threshold").await;
    rig.configure_policy(2).await;
    let active = activate_count_configuration(
        &rig,
        ConfigurationScope::OwnerFacility {
            inventory_owner_id: rig.owner_id,
            facility_id: rig.facility_id,
        },
        1,
        3,
        "count-direct-approval",
    )
    .await;

    let task_id = rig.create_task("count-configured-task").await;
    rig.claim(task_id, "count-configured-claim").await;
    let confirmation = rig.confirm(task_id, 7, "count-configured-confirm").await;
    assert_eq!(
        confirmation.disposition,
        CycleCountDisposition::ApprovalRequired
    );
    assert_eq!(confirmation.next_recount_task_id, None);
    let policy = confirmation.decision_policy.as_ref().unwrap();
    assert_eq!(policy.source, CountDecisionPolicySource::Configuration);
    assert_eq!(
        policy.configuration_id,
        Some(active.response.configuration_id)
    );
    assert_eq!(
        policy.configuration_scope,
        Some(ConfigurationScope::OwnerFacility {
            inventory_owner_id: rig.owner_id,
            facility_id: rig.facility_id,
        })
    );
    assert_eq!(policy.approval_threshold_quantity, Some(3));

    let variances: CycleCountVariancePage = response_json(
        rig.send(
            Method::GET,
            "/api/v1/cycle-count-variances?limit=100",
            None,
            None,
        )
        .await,
    )
    .await;
    let variance = variances
        .items
        .iter()
        .find(|variance| variance.variance_id == confirmation.variance_id.unwrap())
        .unwrap();
    assert_eq!(variance.decision_policy, *policy);
    assert_eq!(variance.automatic_recounts_used, 0);

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let frozen: (
        String,
        Option<i64>,
        Option<i64>,
        Option<String>,
        i64,
        i32,
        Option<i64>,
        String,
    ) = sqlx::query_as(
        r#"
            SELECT count_policy_source,count_configuration_id,
                   count_configuration_revision,count_scope_level,
                   count_absolute_tolerance_qty,count_percentage_tolerance_bps,
                   count_approval_threshold_qty,count_policy_hash
            FROM cycle_count_item_location_results
            WHERE tenant_id=$1 AND task_id=$2
            "#,
    )
    .bind(rig.tenant_id.get())
    .bind(task_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let event: Value =
        sqlx::query_scalar("SELECT payload FROM outbox_events WHERE tenant_id=$1 AND event_key=$2")
            .bind(rig.tenant_id.get())
            .bind(format!("cycle-count-confirmation:{task_id}"))
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.rollback().await.unwrap();
    let admin = admin_db_for(&rig.fixture.db).await;
    let forged = sqlx::query(
        "UPDATE cycle_count_variance_cases SET count_policy_hash=repeat('0',64) WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(confirmation.variance_id.unwrap())
    .execute(&admin)
    .await
    .expect_err("Count decision evidence must be immutable");
    assert_eq!(
        forged.as_database_error().unwrap().code().as_deref(),
        Some("55000")
    );
    admin.close().await;
    assert_eq!(frozen.0, "configuration");
    assert_eq!(frozen.1, Some(active.response.configuration_id));
    assert_eq!(frozen.3.as_deref(), Some("owner_facility"));
    assert_eq!((frozen.4, frozen.5, frozen.6), (1, 0, Some(3)));
    assert_eq!(frozen.7, policy.policy_hash);
    assert_eq!(
        event["decision_policy"]["configuration_id"],
        active.response.configuration_id
    );
}

#[tokio::test]
async fn owner_override_precedes_facility_rule_and_retry_survives_retirement() {
    let rig = Rig::new("configured-precedence").await;
    rig.configure_policy(1).await;
    activate_count_configuration(
        &rig,
        ConfigurationScope::Facility {
            facility_id: rig.facility_id,
        },
        1,
        8,
        "count-facility",
    )
    .await;
    let owner = activate_count_configuration(
        &rig,
        ConfigurationScope::OwnerFacility {
            inventory_owner_id: rig.owner_id,
            facility_id: rig.facility_id,
        },
        1,
        3,
        "count-owner",
    )
    .await;
    let task_id = rig.create_task("count-precedence-task").await;
    rig.claim(task_id, "count-precedence-claim").await;
    let result = rig.confirm(task_id, 7, "count-precedence-confirm").await;
    assert_eq!(result.disposition, CycleCountDisposition::ApprovalRequired);
    assert_eq!(
        result.decision_policy.as_ref().unwrap().configuration_id,
        Some(owner.response.configuration_id)
    );

    let variance_id = result.variance_id.unwrap();
    let _: DecideCycleCountVarianceResponse = response_json(
        expect_status(
            rig.send(
                Method::POST,
                &format!("/api/v1/cycle-count-variances/{variance_id}/decisions"),
                Some("count-owner-approve"),
                Some(json!({
                    "expected_revision": result.variance_revision.unwrap().get(),
                    "decision": "approve_adjustment",
                    "reason": "verified_physical_count"
                })),
            )
            .await,
            StatusCode::OK,
        )
        .await,
    )
    .await;

    transition(
        &rig,
        &rig.token,
        owner.response.configuration_id,
        "retirements",
        owner.response.revision,
        "count-owner-retire",
    )
    .await;
    let replay = rig.confirm(task_id, 7, "count-precedence-confirm").await;
    assert_eq!(replay, result, "exact retry must return frozen evidence");

    let next_task = rig.create_task("count-facility-fallback-task").await;
    rig.claim(next_task, "count-facility-fallback-claim").await;
    let fallback = rig
        .confirm(next_task, 0, "count-facility-fallback-confirm")
        .await;
    assert_eq!(fallback.disposition, CycleCountDisposition::RecountRequired);
    let fallback_policy = fallback.decision_policy.unwrap();
    assert_eq!(fallback_policy.approval_threshold_quantity, Some(8));
    assert_eq!(
        fallback_policy.configuration_scope,
        Some(ConfigurationScope::Facility {
            facility_id: rig.facility_id,
        })
    );
}
