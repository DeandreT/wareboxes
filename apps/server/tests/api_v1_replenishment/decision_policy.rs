use super::*;
use wareboxes_api_contract::v1::{
    ConfigurationLifecycleRequest, ConfigurationResponse, ConfigurationScope,
    CreateConfigurationRequest, DecisionRule, ReplenishmentDecisionPolicySource, Revision,
};

#[derive(Clone, Copy)]
struct ReplenishmentRuleVersion {
    scope: ConfigurationScope,
    minimum_percent: u8,
    target_percent: u8,
    include_inbound_projection: bool,
    expected_revision: Option<i64>,
}

async fn add_approver(rig: &ReplenishmentFixture, suffix: &str) -> String {
    grant_permission(
        &rig.fixture.db,
        rig.access.tenant_id,
        rig.access.user_id.get(),
        &format!("replenishment-config-creator-{suffix}"),
        "admin",
    )
    .await;
    let approver = rig
        .fixture
        .user(&format!(
            "replenishment-config-approver-{suffix}@test.local"
        ))
        .await;
    let mut tx = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships(tenant_id,user_id) VALUES ($1,$2)")
        .bind(rig.access.tenant_id.get())
        .bind(approver.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    grant_permission(
        &rig.fixture.db,
        rig.access.tenant_id,
        approver.id,
        &format!("replenishment-config-approver-{suffix}"),
        "admin",
    )
    .await;
    auth::create_session(&rig.fixture.db, approver.id)
        .await
        .unwrap()
}

async fn create_configuration(
    rig: &ReplenishmentFixture,
    version: ReplenishmentRuleVersion,
    prefix: &str,
) -> ConfigurationResponse {
    let response = rig
        .request(
            Method::POST,
            "/api/v1/configurations",
            Some(&format!("{prefix}-create")),
            Some(
                serde_json::to_value(CreateConfigurationRequest {
                    scope: version.scope,
                    effective_from: "2026-01-01T00:00:00Z".into(),
                    effective_until: None,
                    rule: DecisionRule::Replenishment {
                        minimum_percent: version.minimum_percent,
                        target_percent: version.target_percent,
                        include_inbound_projection: version.include_inbound_projection,
                    },
                    expected_revision: version
                        .expected_revision
                        .map(|value| Revision::new(value).unwrap()),
                })
                .unwrap(),
            ),
        )
        .await;
    response_json(expect_status(response, StatusCode::OK, "create replenishment rule").await).await
}

async fn transition_configuration(
    rig: &ReplenishmentFixture,
    token: &str,
    configuration: &ConfigurationResponse,
    transition: &str,
    prefix: &str,
) -> ConfigurationResponse {
    let response = send(
        &rig.app,
        token,
        rig.access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/configurations/{}/{transition}",
            configuration.configuration_id
        ),
        Some(&format!("{prefix}-{transition}")),
        Some(
            serde_json::to_value(ConfigurationLifecycleRequest {
                expected_revision: configuration.revision,
            })
            .unwrap(),
        ),
    )
    .await;
    response_json(expect_status(response, StatusCode::OK, transition).await).await
}

async fn approve_configuration(
    rig: &ReplenishmentFixture,
    approver_token: &str,
    version: ReplenishmentRuleVersion,
    prefix: &str,
) -> ConfigurationResponse {
    let created = create_configuration(rig, version, prefix).await;
    let submitted =
        transition_configuration(rig, &rig.token, &created, "submissions", prefix).await;
    transition_configuration(rig, approver_token, &submitted, "approvals", prefix).await
}

async fn activate_configuration(
    rig: &ReplenishmentFixture,
    approved: &ConfigurationResponse,
    prefix: &str,
) -> ConfigurationResponse {
    transition_configuration(rig, &rig.token, approved, "activations", prefix).await
}

async fn configured_operational_policy(
    rig: &ReplenishmentFixture,
    prefix: &str,
) -> ConfigureReplenishmentPolicyResponse {
    let (source_a, barcode_a) = rig.reserve_source(&format!("{prefix}-A")).await;
    let (source_b, barcode_b) = rig.reserve_source(&format!("{prefix}-B")).await;
    rig.seed_stock(
        source_a,
        &barcode_a,
        10,
        "LOT-A",
        None,
        &format!("{prefix}-stock-a"),
    )
    .await;
    rig.seed_stock(
        source_b,
        &barcode_b,
        10,
        "LOT-B",
        None,
        &format!("{prefix}-stock-b"),
    )
    .await;
    let response = rig
        .configure(
            &format!("{prefix}-operational-policy"),
            &[source_a, source_b],
            2,
            10,
            None,
        )
        .await;
    response_json(expect_status(response, StatusCode::OK, "configure operational policy").await)
        .await
}

async fn policy_readiness(rig: &ReplenishmentFixture) -> ReplenishmentPolicyPage {
    let response = rig
        .request(
            Method::GET,
            &format!("/api/v1/replenishment-policies?item_id={}", rig.item_id),
            None,
            None,
        )
        .await;
    response_json(expect_status(response, StatusCode::OK, "read replenishment readiness").await)
        .await
}

#[tokio::test]
async fn inherited_rule_controls_readiness_planning_replay_and_frozen_evidence() {
    init_test_tracing();
    let rig = ReplenishmentFixture::new("replenishment-decision").await;
    let approver_token = add_approver(&rig, "lifecycle").await;
    let operational = configured_operational_policy(&rig, "decision").await;

    let initial = policy_readiness(&rig).await;
    let initial = &initial.items[0];
    assert_eq!(
        initial.decision_policy.source,
        ReplenishmentDecisionPolicySource::ProductDefault
    );
    assert_eq!(initial.decision_policy.effective_minimum_quantity, 2);
    assert_eq!(initial.decision_policy.effective_target_quantity, 10);

    let default_plan: PlanReplenishmentResponse = response_json(
        expect_status(
            rig.plan(operational.policy_id, 1, "decision-default-plan")
                .await,
            StatusCode::OK,
            "plan with product default",
        )
        .await,
    )
    .await;
    assert_eq!(default_plan.planned_quantity, 10);
    assert_eq!(default_plan.observed_active_inbound, 0);

    let approved = approve_configuration(
        &rig,
        &approver_token,
        ReplenishmentRuleVersion {
            scope: ConfigurationScope::Tenant,
            minimum_percent: 30,
            target_percent: 60,
            include_inbound_projection: false,
            expected_revision: None,
        },
        "decision-config",
    )
    .await;
    let active = activate_configuration(&rig, &approved, "decision-config").await;

    let readiness = policy_readiness(&rig).await;
    let readiness = &readiness.items[0];
    assert_eq!(
        readiness.decision_policy.source,
        ReplenishmentDecisionPolicySource::Configuration
    );
    assert_eq!(
        readiness.decision_policy.configuration_id,
        Some(active.configuration_id)
    );
    assert_eq!(
        readiness.decision_policy.configuration_scope,
        Some(ConfigurationScope::Tenant)
    );
    assert_eq!(readiness.decision_policy.minimum_percent, Some(30));
    assert_eq!(readiness.decision_policy.target_percent, Some(60));
    assert_eq!(readiness.decision_policy.effective_minimum_quantity, 3);
    assert_eq!(readiness.decision_policy.effective_target_quantity, 6);
    assert_eq!(readiness.observed_active_inbound, 10);
    assert_eq!(readiness.snapshot.active_inbound, 0);
    assert_eq!(readiness.suggested_quantity, 6);

    let override_attempt = rig
        .request(
            Method::POST,
            &format!(
                "/api/v1/replenishment-policies/{}/plan-runs",
                operational.policy_id
            ),
            Some("decision-client-override"),
            Some(json!({
                "expected_policy_revision": 1,
                "include_inbound_projection": true
            })),
        )
        .await;
    assert_eq!(override_attempt.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let configured_plan: PlanReplenishmentResponse = response_json(
        expect_status(
            rig.plan(operational.policy_id, 1, "decision-configured-plan")
                .await,
            StatusCode::OK,
            "plan with inherited configuration",
        )
        .await,
    )
    .await;
    assert_eq!(configured_plan.decision_policy, readiness.decision_policy);
    assert_eq!(configured_plan.observed_active_inbound, 10);
    assert_eq!(configured_plan.snapshot.active_inbound, 0);
    assert_eq!(configured_plan.planned_quantity, 6);
    let mut evidence_tx = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    let event_payload: Value =
        sqlx::query_scalar("SELECT payload FROM outbox_events WHERE tenant_id=$1 AND event_key=$2")
            .bind(rig.access.tenant_id.get())
            .bind(format!(
                "replenishment_policy:{}:plan:{}",
                operational.policy_id, configured_plan.plan_id
            ))
            .fetch_one(&mut *evidence_tx)
            .await
            .unwrap();
    evidence_tx.rollback().await.unwrap();
    assert_eq!(
        event_payload["decision_policy"]["configuration_id"],
        active.configuration_id
    );
    assert_eq!(
        event_payload["decision_policy"]["policy_hash"],
        configured_plan.decision_policy.policy_hash
    );

    let override_approved = approve_configuration(
        &rig,
        &approver_token,
        ReplenishmentRuleVersion {
            scope: ConfigurationScope::OwnerFacility {
                inventory_owner_id: rig.inventory_owner_id,
                facility_id: rig.facility_id,
            },
            minimum_percent: 40,
            target_percent: 80,
            include_inbound_projection: true,
            expected_revision: None,
        },
        "decision-owner-facility",
    )
    .await;
    let inherited_override =
        activate_configuration(&rig, &override_approved, "decision-owner-facility").await;
    let overridden_readiness = policy_readiness(&rig).await;
    assert_eq!(
        overridden_readiness.items[0]
            .decision_policy
            .configuration_id,
        Some(inherited_override.configuration_id)
    );

    let replay: PlanReplenishmentResponse = response_json(
        expect_status(
            rig.plan(operational.policy_id, 1, "decision-configured-plan")
                .await,
            StatusCode::OK,
            "replay plan after rule supersession",
        )
        .await,
    )
    .await;
    assert_eq!(replay, configured_plan);

    let successor_approved = approve_configuration(
        &rig,
        &approver_token,
        ReplenishmentRuleVersion {
            scope: ConfigurationScope::OwnerFacility {
                inventory_owner_id: rig.inventory_owner_id,
                facility_id: rig.facility_id,
            },
            minimum_percent: 50,
            target_percent: 90,
            include_inbound_projection: true,
            expected_revision: Some(inherited_override.revision.get()),
        },
        "decision-successor",
    )
    .await;
    let successor = activate_configuration(&rig, &successor_approved, "decision-successor").await;
    assert_ne!(
        successor.configuration_id,
        inherited_override.configuration_id
    );

    let mut tx = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    let forged = sqlx::query(
        r#"
        INSERT INTO replenishment_plan_runs (
          tenant_id,inventory_owner_id,facility_id,policy_id,policy_revision,
          pick_face_location_id,item_id,uom,minimum_qty,target_qty,source_location_count,
          decision_policy_source,decision_configuration_id,decision_configuration_revision,
          decision_scope_level,decision_inventory_owner_id,decision_facility_id,
          decision_minimum_percent,decision_target_percent,include_inbound_projection,
          operational_minimum_qty,operational_target_qty,decision_policy_hash,
          pick_face_free_qty,observed_active_inbound_qty,active_inbound_qty,
          projected_free_qty,unallocated_demand_qty,required_level_qty,target_gap_qty,
          reserve_free_qty,planned_qty,work_count,outcome,planned_by_user_id,planned_at)
        SELECT tenant_id,inventory_owner_id,facility_id,policy_id,policy_revision,
          pick_face_location_id,item_id,uom,minimum_qty,target_qty,source_location_count,
          decision_policy_source,decision_configuration_id,decision_configuration_revision,
          decision_scope_level,decision_inventory_owner_id,decision_facility_id,
          decision_minimum_percent,decision_target_percent,include_inbound_projection,
          operational_minimum_qty,operational_target_qty,decision_policy_hash,
          pick_face_free_qty,observed_active_inbound_qty,active_inbound_qty,
          projected_free_qty,unallocated_demand_qty,required_level_qty,target_gap_qty,
          reserve_free_qty,planned_qty,work_count,outcome,planned_by_user_id,transaction_timestamp()
        FROM replenishment_plan_runs WHERE tenant_id=$1 AND id=$2
        "#,
    )
    .bind(rig.access.tenant_id.get())
    .bind(configured_plan.plan_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert_eq!(
        forged.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn concurrent_activation_and_planning_freeze_one_serial_policy_order() {
    let rig = ReplenishmentFixture::new("replenishment-decision-race").await;
    let approver_token = add_approver(&rig, "race").await;
    let operational = configured_operational_policy(&rig, "decision-race").await;
    let approved = approve_configuration(
        &rig,
        &approver_token,
        ReplenishmentRuleVersion {
            scope: ConfigurationScope::Tenant,
            minimum_percent: 25,
            target_percent: 75,
            include_inbound_projection: true,
            expected_revision: None,
        },
        "decision-race-config",
    )
    .await;

    let (activation, planning) = tokio::join!(
        activate_configuration(&rig, &approved, "decision-race-config"),
        rig.plan(operational.policy_id, 1, "decision-race-plan"),
    );
    let plan: PlanReplenishmentResponse = response_json(
        expect_status(planning, StatusCode::OK, "concurrent replenishment plan").await,
    )
    .await;
    match plan.decision_policy.source {
        ReplenishmentDecisionPolicySource::ProductDefault => {
            assert_eq!(plan.decision_policy.configuration_id, None)
        }
        ReplenishmentDecisionPolicySource::Configuration => assert_eq!(
            plan.decision_policy.configuration_id,
            Some(activation.configuration_id)
        ),
    }
    let mut tx = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    let frozen: (String, Option<i64>) = sqlx::query_as(
        "SELECT decision_policy_source,decision_configuration_id FROM replenishment_plan_runs WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.access.tenant_id.get())
    .bind(plan.plan_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        frozen.0,
        match plan.decision_policy.source {
            ReplenishmentDecisionPolicySource::ProductDefault => "product_default",
            ReplenishmentDecisionPolicySource::Configuration => "configuration",
        }
    );
    assert_eq!(frozen.1, plan.decision_policy.configuration_id);
}
