use super::*;
use serde_json::Value;
use std::time::Duration;
use wareboxes_api_contract::v1::{
    ConfigurationLifecycleRequest, ConfigurationResponse, ConfigurationScope,
    CreateConfigurationRequest, DecisionRule,
};

async fn transition_configuration(
    rig: &Rig,
    token: &str,
    configuration_id: i64,
    action: &str,
    revision: Revision,
    key: &str,
) -> ConfigurationResponse {
    response_json(
        rig.send(
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
    .await
}

async fn activate_billing_configuration(
    rig: &Rig,
    scope: ConfigurationScope,
    rate_minor: u64,
    prefix: &str,
) -> ConfigurationResponse {
    let created: ConfigurationResponse = response_json(
        rig.send(
            &rig.creator_token,
            Method::POST,
            "/api/v1/configurations",
            Some(&format!("{prefix}-create")),
            Some(&CreateConfigurationRequest {
                scope,
                effective_from: "2026-01-01T00:00:00Z".into(),
                effective_until: None,
                rule: DecisionRule::Billing {
                    event_type: BillableEventType::Accessorial,
                    unit: BillingUnit::Event,
                    currency: "USD".into(),
                    rate_minor,
                    minimum_charge_minor: 0,
                },
                expected_revision: None,
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let submitted = transition_configuration(
        rig,
        &rig.creator_token,
        created.configuration_id,
        "submissions",
        created.revision,
        &format!("{prefix}-submit"),
    )
    .await;
    let approved = transition_configuration(
        rig,
        &rig.approver_token,
        created.configuration_id,
        "approvals",
        submitted.revision,
        &format!("{prefix}-approve"),
    )
    .await;
    transition_configuration(
        rig,
        &rig.creator_token,
        created.configuration_id,
        "activations",
        approved.revision,
        &format!("{prefix}-activate"),
    )
    .await
}

async fn capture_accessorial(
    rig: &Rig,
    contract_id: i64,
    occurred_at: wareboxes_domain::Timestamp,
    key: &str,
    reference: &str,
) -> BillableEventResponse {
    response_json(
        rig.send(
            &rig.creator_token,
            Method::POST,
            &format!("/api/v1/billing/contracts/{contract_id}/billable-events"),
            Some(key),
            Some(&CaptureBillableEventRequest {
                facility_id: rig.facility_id,
                event_type: BillableEventType::Accessorial,
                unit: BillingUnit::Event,
                quantity: 2,
                source_reference: reference.into(),
                description: "Configuration-priced accessorial".into(),
                occurred_at: occurred_at.to_rfc3339(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await
}

async fn generate_period(
    rig: &Rig,
    contract_id: i64,
    from: wareboxes_domain::Timestamp,
    until: wareboxes_domain::Timestamp,
    key: &str,
) -> BillingRunResponse {
    response_json(
        rig.send(
            &rig.creator_token,
            Method::POST,
            &format!("/api/v1/billing/contracts/{contract_id}/reconciliation-runs"),
            Some(key),
            Some(&GenerateBillingRunRequest {
                facility_id: Some(rig.facility_id),
                period_from: from.to_rfc3339(),
                period_until: until.to_rfc3339(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await
}

#[tokio::test]
async fn inherited_billing_configuration_executes_and_historical_terms_remain_frozen() {
    let rig = Rig::new().await;
    let contract = rig
        .create_contract("decision-contract", "DECISION-2026")
        .await;
    let baseline = rig
        .rate(
            contract.contract_id,
            "decision-contract-rate",
            BillableEventType::Accessorial,
            BillingUnit::Event,
            1_000,
        )
        .await;
    rig.activate(&contract).await;
    let facility = activate_billing_configuration(
        &rig,
        ConfigurationScope::Facility {
            facility_id: rig.facility_id,
        },
        2_000,
        "billing-facility",
    )
    .await;
    let owner = activate_billing_configuration(
        &rig,
        ConfigurationScope::OwnerFacility {
            inventory_owner_id: rig.owner_id,
            facility_id: rig.facility_id,
        },
        3_000,
        "billing-owner",
    )
    .await;

    let first_from = db::now_iso() - Duration::from_secs(1);
    let first_occurred = db::now_iso();
    capture_accessorial(
        &rig,
        contract.contract_id,
        first_occurred,
        "configured-billing-event",
        "CONFIGURED-SERVICE",
    )
    .await;
    let first_until = db::now_iso();
    let configured = generate_period(
        &rig,
        contract.contract_id,
        first_from,
        first_until,
        "configured-billing-run",
    )
    .await;
    assert_eq!(configured.total_minor, 6_000);
    assert_eq!(configured.charges.len(), 1);
    let charge = &configured.charges[0];
    assert_eq!(charge.rate_id, None);
    assert_eq!(
        charge.decision_policy.source,
        BillingDecisionPolicySource::Configuration
    );
    assert_eq!(
        charge.decision_policy.configuration_id,
        Some(owner.configuration_id)
    );
    assert_eq!(
        charge.decision_policy.configuration_scope,
        Some(ConfigurationScope::OwnerFacility {
            inventory_owner_id: rig.owner_id,
            facility_id: rig.facility_id,
        })
    );

    let retired_owner = transition_configuration(
        &rig,
        &rig.creator_token,
        owner.configuration_id,
        "retirements",
        owner.revision,
        "billing-owner-retire",
    )
    .await;
    transition_configuration(
        &rig,
        &rig.creator_token,
        facility.configuration_id,
        "retirements",
        facility.revision,
        "billing-facility-retire",
    )
    .await;
    assert!(retired_owner.revision.get() > owner.revision.get());
    let replay = generate_period(
        &rig,
        contract.contract_id,
        first_from,
        first_until,
        "configured-billing-run",
    )
    .await;
    assert_eq!(replay, configured, "exact retry must keep original terms");

    let second_from = first_until;
    let second_occurred = db::now_iso();
    capture_accessorial(
        &rig,
        contract.contract_id,
        second_occurred,
        "contract-billing-event",
        "CONTRACT-SERVICE",
    )
    .await;
    let second_until = db::now_iso();
    let fallback = generate_period(
        &rig,
        contract.contract_id,
        second_from,
        second_until,
        "contract-billing-run",
    )
    .await;
    assert_eq!(fallback.total_minor, 2_000);
    let fallback_charge = &fallback.charges[0];
    assert_eq!(fallback_charge.rate_id, Some(baseline.rate_id));
    assert_eq!(
        fallback_charge.decision_policy.source,
        BillingDecisionPolicySource::ContractRate
    );

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let event: Value =
        sqlx::query_scalar("SELECT payload FROM outbox_events WHERE tenant_id=$1 AND event_key=$2")
            .bind(rig.tenant_id.get())
            .bind(format!(
                "reconciliation_run:{}:generated",
                configured.run_id
            ))
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        event["charges"][0]["decision_policy"]["configuration_id"],
        owner.configuration_id
    );

    let admin = admin_db_for(&rig.fixture.db).await;
    let tamper = sqlx::query(
        "UPDATE billing_charges SET decision_policy_hash=repeat('0',64) WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(charge.charge_id)
    .execute(&admin)
    .await
    .expect_err("billing charge evidence must remain immutable");
    assert_eq!(
        tamper.as_database_error().unwrap().code().as_deref(),
        Some("55000")
    );
    admin.close().await;
}

#[tokio::test]
async fn concurrent_retirement_and_reconciliation_freeze_the_activation_revision() {
    let rig = Rig::new().await;
    let contract = rig.create_contract("race-contract", "RACE-2026").await;
    rig.rate(
        contract.contract_id,
        "race-contract-rate",
        BillableEventType::Accessorial,
        BillingUnit::Event,
        1_000,
    )
    .await;
    rig.activate(&contract).await;
    let configuration = activate_billing_configuration(
        &rig,
        ConfigurationScope::OwnerFacility {
            inventory_owner_id: rig.owner_id,
            facility_id: rig.facility_id,
        },
        4_000,
        "billing-race",
    )
    .await;
    let period_from = db::now_iso() - Duration::from_secs(1);
    let occurred_at = db::now_iso();
    capture_accessorial(
        &rig,
        contract.contract_id,
        occurred_at,
        "race-billing-event",
        "RACE-SERVICE",
    )
    .await;
    let period_until = db::now_iso();

    let (retired, run) = tokio::join!(
        transition_configuration(
            &rig,
            &rig.creator_token,
            configuration.configuration_id,
            "retirements",
            configuration.revision,
            "billing-race-retire",
        ),
        generate_period(
            &rig,
            contract.contract_id,
            period_from,
            period_until,
            "billing-race-run",
        )
    );
    assert!(retired.revision.get() > configuration.revision.get());
    assert_eq!(run.total_minor, 8_000);
    let evidence = &run.charges[0].decision_policy;
    assert_eq!(
        evidence.configuration_id,
        Some(configuration.configuration_id)
    );
    assert_eq!(
        evidence.configuration_revision,
        Some(configuration.revision),
        "retirement must not change the revision used for an earlier event"
    );
}
