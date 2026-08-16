use super::*;
use wareboxes_api_contract::v1::{
    ConfigurationLifecycleRequest, ConfigurationResponse, ConfigurationScope,
    CreateConfigurationRequest, DecisionRule, PickWavePolicyResolutionsResponse,
    ResolvePickWavePoliciesRequest, ResolvePickWavePolicyOrderRequest, WavePolicyResponse,
    WavePolicySource,
};

pub(super) async fn add_membership(fixture: &Fixture, tenant_id: TenantId, user_id: i64) {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships(tenant_id,user_id) VALUES ($1,$2)")
        .bind(tenant_id.get())
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

pub(super) async fn grant_permission(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    permission_name: &str,
    suffix: &str,
) {
    let permission = match wareboxes_persistence_postgres::permissions::find_by_name(
        &fixture.db,
        tenant_id,
        permission_name,
    )
    .await
    .unwrap()
    {
        Some(permission) => permission.id,
        None => wareboxes_persistence_postgres::permissions::add_permission(
            &fixture.db,
            tenant_id,
            permission_name,
            Some(permission_name),
        )
        .await
        .unwrap(),
    };
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("wave-policy-{permission_name}-{suffix}"),
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

async fn transition_configuration(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    configuration: &ConfigurationResponse,
    transition: &str,
    key: &str,
) -> ConfigurationResponse {
    json_response(
        expect(
            send(
                app,
                token,
                tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/configurations/{}/{transition}",
                    configuration.configuration_id
                ),
                Some(key),
                Some(
                    serde_json::to_value(ConfigurationLifecycleRequest {
                        expected_revision: configuration.revision,
                    })
                    .unwrap(),
                ),
            )
            .await,
            StatusCode::OK,
            transition,
        )
        .await,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn approve_policy(
    app: &axum::Router,
    operator_token: &str,
    approver_token: &str,
    tenant_id: TenantId,
    owner_id: i64,
    facility_id: i64,
    max_orders: u32,
    require_complete_allocation: bool,
    expected_revision: Option<Revision>,
    prefix: &str,
) -> ConfigurationResponse {
    let request = CreateConfigurationRequest {
        scope: ConfigurationScope::OwnerFacility {
            inventory_owner_id: owner_id,
            facility_id,
        },
        effective_from: "2026-01-01T00:00:00Z".into(),
        effective_until: None,
        rule: DecisionRule::Wave {
            max_orders,
            require_complete_allocation,
        },
        expected_revision,
    };
    let created: ConfigurationResponse = json_response(
        expect(
            send(
                app,
                operator_token,
                tenant_id,
                Method::POST,
                "/api/v1/configurations",
                Some(&format!("{prefix}-create")),
                Some(serde_json::to_value(request).unwrap()),
            )
            .await,
            StatusCode::OK,
            "create wave policy",
        )
        .await,
    )
    .await;
    let submitted = transition_configuration(
        app,
        operator_token,
        tenant_id,
        &created,
        "submissions",
        &format!("{prefix}-submit"),
    )
    .await;
    transition_configuration(
        app,
        approver_token,
        tenant_id,
        &submitted,
        "approvals",
        &format!("{prefix}-approve"),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn activate_policy(
    app: &axum::Router,
    operator_token: &str,
    approver_token: &str,
    tenant_id: TenantId,
    owner_id: i64,
    facility_id: i64,
    max_orders: u32,
    require_complete_allocation: bool,
    expected_revision: Option<Revision>,
    prefix: &str,
) -> ConfigurationResponse {
    let approved = approve_policy(
        app,
        operator_token,
        approver_token,
        tenant_id,
        owner_id,
        facility_id,
        max_orders,
        require_complete_allocation,
        expected_revision,
        prefix,
    )
    .await;
    transition_configuration(
        app,
        operator_token,
        tenant_id,
        &approved,
        "activations",
        &format!("{prefix}-activate"),
    )
    .await
}

async fn resolve_policy(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    facility_id: i64,
    orders: &[(i64, i64)],
) -> Vec<(i64, WavePolicyResponse)> {
    let body = ResolvePickWavePoliciesRequest {
        facility_id,
        orders: orders
            .iter()
            .map(|(order_id, revision)| ResolvePickWavePolicyOrderRequest {
                order_id: *order_id,
                expected_revision: Revision::new(*revision).unwrap(),
            })
            .collect(),
    };
    let response: PickWavePolicyResolutionsResponse = json_response(
        expect(
            send(
                app,
                token,
                tenant_id,
                Method::POST,
                "/api/v1/pick-waves/policy-resolutions",
                None,
                Some(serde_json::to_value(body).unwrap()),
            )
            .await,
            StatusCode::OK,
            "resolve wave policies",
        )
        .await,
    )
    .await;
    response
        .into_iter()
        .map(|resolution| (resolution.order_id, resolution.policy))
        .collect()
}

fn plan_with_policies(
    facility_id: i64,
    destination_id: i64,
    name: &str,
    orders: &[(i64, i64, &WavePolicyResponse)],
) -> Value {
    json!({
        "facility_id": facility_id,
        "destination_location_id": destination_id,
        "name": name,
        "orders": orders.iter().enumerate().map(|(index, (order_id, revision, policy))| json!({
            "order_id": order_id,
            "expected_revision": revision,
            "sequence": index + 1,
            "expected_policy": policy.expectation()
        })).collect::<Vec<_>>()
    })
}

#[tokio::test]
async fn configured_wave_policy_is_exact_enforced_audited_and_replay_safe() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("wave-policy-operator@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_permissions(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "configured-policy",
    )
    .await;
    grant_permission(&fixture, access.tenant_id, operator.id, "admin", "operator").await;
    let approver = fixture.user("wave-policy-approver@test.local").await;
    add_membership(&fixture, access.tenant_id, approver.id).await;
    grant_permission(&fixture, access.tenant_id, approver.id, "admin", "approver").await;

    let owner = fixture
        .inventory_owner(access.tenant_id, "Wave Policy Owner")
        .await;
    let facility = fixture
        .facility(access.tenant_id, "Wave Policy Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner, facility)
        .await;
    let destination =
        staging_location(&fixture, access.tenant_id, facility, "WAVE-POLICY-STAGE").await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let approver_token = auth::create_session(&fixture.db, approver.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let strict = activate_policy(
        &app,
        &token,
        &approver_token,
        access.tenant_id,
        owner,
        facility,
        1,
        true,
        None,
        "wave-strict",
    )
    .await;
    let first = allocated_order(
        &fixture,
        &app,
        &token,
        &access,
        owner,
        facility,
        "WAVE-POLICY-A",
        2,
    )
    .await;
    let second = allocated_order(
        &fixture,
        &app,
        &token,
        &access,
        owner,
        facility,
        "WAVE-POLICY-B",
        3,
    )
    .await;
    let unallocated_item = fixture
        .item(access.tenant_id, "Wave Unallocated Item", "each")
        .await;
    let unallocated = fixture
        .order_header(access.tenant_id, "WAVE-POLICY-NO-STOCK", owner)
        .await;
    fixture
        .order_item(access.tenant_id, unallocated, unallocated_item, 1)
        .await;

    let policies = resolve_policy(
        &app,
        &token,
        access.tenant_id,
        facility,
        &[first, second, (unallocated, 1)],
    )
    .await;
    assert_eq!(policies.len(), 3);
    let policy = &policies[0].1;
    assert_eq!(policy.source, WavePolicySource::Configuration);
    assert_eq!(policy.configuration_id, Some(strict.configuration_id));
    assert_eq!(policy.configuration_revision, Some(strict.revision.get()));
    assert_eq!(
        policy.configuration_scope,
        Some(ConfigurationScope::OwnerFacility {
            inventory_owner_id: owner,
            facility_id: facility,
        })
    );
    assert_eq!(policy.max_orders, 1);
    assert!(policy.require_complete_allocation);
    assert!(policies.iter().all(|(_, candidate)| candidate == policy));

    let incomplete = plan_with_policies(
        facility,
        destination,
        "Incomplete strict wave",
        &[(unallocated, 1, policy)],
    );
    assert_eq!(
        send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            "/api/v1/pick-waves",
            Some("wave-policy-incomplete"),
            Some(incomplete),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let over_limit = plan_with_policies(
        facility,
        destination,
        "Over-limit strict wave",
        &[(first.0, first.1, policy), (second.0, second.1, policy)],
    );
    assert_eq!(
        send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            "/api/v1/pick-waves",
            Some("wave-policy-limit"),
            Some(over_limit),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    let exact = plan_with_policies(
        facility,
        destination,
        "Exact strict wave",
        &[(first.0, first.1, policy)],
    );
    let planned: PickWaveResponse = json_response(
        expect(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                "/api/v1/pick-waves",
                Some("wave-policy-exact"),
                Some(exact.clone()),
            )
            .await,
            StatusCode::OK,
            "plan exact policy wave",
        )
        .await,
    )
    .await;
    assert_eq!(planned.orders.len(), 1);
    assert_eq!(planned.orders[0].wave_policy, *policy);

    let mut bypass_tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let raw_wave_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO pick_waves(
             tenant_id,facility_id,destination_location_id,name,status,revision,
             order_count,planned_by_user_id,planned_at)
           VALUES($1,$2,$3,'Raw strict bypass','planned',1,1,$4,statement_timestamp())
           RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(facility)
    .bind(destination)
    .bind(operator.id)
    .fetch_one(&mut *bypass_tx)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO pick_wave_orders(
             tenant_id,facility_id,pick_wave_id,inventory_owner_id,order_id,order_key,
             wave_sequence,expected_order_revision,wave_policy_source,
             wave_policy_configuration_id,wave_policy_configuration_revision,
             wave_policy_scope_level,wave_policy_scope_owner_id,wave_policy_scope_facility_id,
             wave_policy_definition,wave_policy_hash)
           VALUES($1,$2,$3,$4,$5,'WAVE-POLICY-NO-STOCK',1,1,'configuration',
             $6,$7,'owner_facility',$4,$2,$8,$9)"#,
    )
    .bind(access.tenant_id.get())
    .bind(facility)
    .bind(raw_wave_id)
    .bind(owner)
    .bind(unallocated)
    .bind(strict.configuration_id)
    .bind(strict.revision.get())
    .bind(json!({"max_orders": 1, "require_complete_allocation": true}))
    .bind(&policy.policy_hash)
    .execute(&mut *bypass_tx)
    .await
    .unwrap();
    let bypass = bypass_tx.commit().await.unwrap_err();
    assert_eq!(
        bypass.as_database_error().and_then(|error| error.code()),
        Some(std::borrow::Cow::Borrowed("23514"))
    );

    let mut tamper_tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let tamper = sqlx::query(
        "UPDATE pick_wave_orders SET wave_policy_hash=repeat('0',64) WHERE tenant_id=$1 AND pick_wave_id=$2",
    )
    .bind(access.tenant_id.get())
    .bind(planned.wave_id)
    .execute(&mut *tamper_tx)
    .await
    .unwrap_err();
    assert_eq!(
        tamper.as_database_error().and_then(|error| error.code()),
        Some(std::borrow::Cow::Borrowed("42501"))
    );
    tamper_tx.rollback().await.unwrap();

    let replacement = activate_policy(
        &app,
        &token,
        &approver_token,
        access.tenant_id,
        owner,
        facility,
        2,
        false,
        Some(strict.revision),
        "wave-relaxed",
    )
    .await;
    assert_ne!(replacement.configuration_id, strict.configuration_id);

    let replay: PickWaveResponse = json_response(
        expect(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                "/api/v1/pick-waves",
                Some("wave-policy-exact"),
                Some(exact.clone()),
            )
            .await,
            StatusCode::OK,
            "replay frozen wave after policy supersession",
        )
        .await,
    )
    .await;
    assert_eq!(replay, planned);
    let stale = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/pick-waves",
        Some("wave-policy-stale"),
        Some(plan_with_policies(
            facility,
            destination,
            "Stale policy wave",
            &[(second.0, second.1, policy)],
        )),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let changed_hash = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/pick-waves",
        Some("wave-policy-exact"),
        Some(plan_with_policies(
            facility,
            destination,
            "Changed replay payload",
            &[(first.0, first.1, policy)],
        )),
    )
    .await;
    assert_eq!(changed_hash.status(), StatusCode::CONFLICT);

    let current = resolve_policy(&app, &token, access.tenant_id, facility, &[second]).await;
    assert_eq!(
        current[0].1.configuration_id,
        Some(replacement.configuration_id)
    );
    assert_eq!(current[0].1.max_orders, 2);
    assert!(!current[0].1.require_complete_allocation);

    let mut evidence_tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let evidence: (String, Option<i64>, i64, serde_json::Value, String, i64) = sqlx::query_as(
        r#"SELECT member.wave_policy_source,member.wave_policy_configuration_id,
                  member.wave_policy_configuration_revision,member.wave_policy_definition,
                  member.wave_policy_hash,
                  (SELECT count(*) FROM outbox_events event
                   WHERE event.tenant_id=member.tenant_id
                     AND event.aggregate_type='pick_wave'
                     AND event.aggregate_id=member.pick_wave_id::text
                     AND event.schema_version=2
                     AND event.payload::text LIKE '%'||member.wave_policy_hash||'%')
           FROM pick_wave_orders member
           WHERE member.tenant_id=$1 AND member.pick_wave_id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(planned.wave_id)
    .fetch_one(&mut *evidence_tx)
    .await
    .unwrap();
    evidence_tx.rollback().await.unwrap();
    assert_eq!(evidence.0, "configuration");
    assert_eq!(evidence.1, Some(strict.configuration_id));
    assert_eq!(evidence.2, strict.revision.get());
    assert_eq!(
        evidence.3,
        json!({"max_orders": 1, "require_complete_allocation": true})
    );
    assert_eq!(evidence.4, policy.policy_hash);
    assert_eq!(evidence.5, 1);

    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        access.tenant_id,
        &UpdateUserAccessScope {
            user_id: operator.id,
            all_facilities: false,
            facility_ids: vec![],
            all_inventory_owners: false,
            inventory_owner_ids: vec![],
        },
    )
    .await
    .unwrap());
    let concealed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/pick-waves/policy-resolutions",
        None,
        Some(
            serde_json::to_value(ResolvePickWavePoliciesRequest {
                facility_id: facility,
                orders: vec![ResolvePickWavePolicyOrderRequest {
                    order_id: second.0,
                    expected_revision: Revision::new(second.1).unwrap(),
                }],
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(concealed.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn concurrent_policy_activation_and_wave_plan_freeze_one_explainable_order() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("wave-policy-race@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_permissions(&fixture.db, access.tenant_id, operator.id, "policy-race").await;
    grant_permission(
        &fixture,
        access.tenant_id,
        operator.id,
        "admin",
        "race-operator",
    )
    .await;
    let approver = fixture.user("wave-policy-race-approver@test.local").await;
    add_membership(&fixture, access.tenant_id, approver.id).await;
    grant_permission(
        &fixture,
        access.tenant_id,
        approver.id,
        "admin",
        "race-approver",
    )
    .await;
    let owner = fixture
        .inventory_owner(access.tenant_id, "Wave Policy Race Owner")
        .await;
    let facility = fixture
        .facility(access.tenant_id, "Wave Policy Race Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner, facility)
        .await;
    let destination = staging_location(
        &fixture,
        access.tenant_id,
        facility,
        "WAVE-POLICY-RACE-STAGE",
    )
    .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let approver_token = auth::create_session(&fixture.db, approver.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let order = allocated_order(
        &fixture,
        &app,
        &token,
        &access,
        owner,
        facility,
        "WAVE-POLICY-RACE-ORDER",
        2,
    )
    .await;
    let initial = resolve_policy(&app, &token, access.tenant_id, facility, &[order]).await;
    assert_eq!(initial[0].1.source, WavePolicySource::ProductDefault);
    let initial_body = plan_with_policies(
        facility,
        destination,
        "Activation race wave",
        &[(order.0, order.1, &initial[0].1)],
    );
    let approved = approve_policy(
        &app,
        &token,
        &approver_token,
        access.tenant_id,
        owner,
        facility,
        1,
        true,
        None,
        "wave-policy-race",
    )
    .await;

    let (activated, planned_response) = tokio::join!(
        transition_configuration(
            &app,
            &token,
            access.tenant_id,
            &approved,
            "activations",
            "wave-policy-race-activate",
        ),
        send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            "/api/v1/pick-waves",
            Some("wave-policy-race-plan"),
            Some(initial_body),
        ),
    );
    assert_eq!(activated.configuration_id, approved.configuration_id);

    let planned = if planned_response.status() == StatusCode::OK {
        let planned: PickWaveResponse = json_response(planned_response).await;
        assert_eq!(
            planned.orders[0].wave_policy.source,
            WavePolicySource::ProductDefault
        );
        planned
    } else {
        assert_eq!(planned_response.status(), StatusCode::CONFLICT);
        let current = resolve_policy(&app, &token, access.tenant_id, facility, &[order]).await;
        assert_eq!(
            current[0].1.configuration_id,
            Some(approved.configuration_id)
        );
        json_response(
            expect(
                send(
                    &app,
                    &token,
                    access.tenant_id,
                    Method::POST,
                    "/api/v1/pick-waves",
                    Some("wave-policy-race-refreshed"),
                    Some(plan_with_policies(
                        facility,
                        destination,
                        "Activation race refreshed",
                        &[(order.0, order.1, &current[0].1)],
                    )),
                )
                .await,
                StatusCode::OK,
                "plan after concurrent policy activation",
            )
            .await,
        )
        .await
    };

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let rows: Vec<(String, Option<i64>)> = sqlx::query_as(
        "SELECT wave_policy_source,wave_policy_configuration_id FROM pick_wave_orders WHERE tenant_id=$1 AND order_id=$2",
    )
    .bind(access.tenant_id.get())
    .bind(order.0)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].0,
        match planned.orders[0].wave_policy.source {
            WavePolicySource::ProductDefault => "product_default",
            WavePolicySource::Configuration => "configuration",
        }
    );
    assert_eq!(rows[0].1, planned.orders[0].wave_policy.configuration_id);
}
