use super::*;
use wareboxes_api_contract::v1::{
    ConfigurationLifecycleRequest, ConfigurationResponse, ConfigurationScope,
    CreateConfigurationRequest, DecisionRule, PickConfirmationHistoryPage,
    PickDecisionPolicySource,
};

async fn grant_permission(
    db: &db::Db,
    tenant_id: TenantId,
    user_id: i64,
    role_name: &str,
    permission_name: &str,
) {
    let permission_id = match wareboxes_persistence_postgres::permissions::find_by_name(
        db,
        tenant_id,
        permission_name,
    )
    .await
    .unwrap()
    {
        Some(permission) => permission.id,
        None => wareboxes_persistence_postgres::permissions::add_permission(
            db,
            tenant_id,
            permission_name,
            Some("Pick decision policy administration"),
        )
        .await
        .unwrap(),
    };
    let role = wareboxes_persistence_postgres::roles::add_role(
        db,
        tenant_id,
        role_name,
        Some("Pick decision policy acceptance role"),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        db,
        tenant_id,
        role,
        permission_id,
    )
    .await
    .unwrap());
    assert!(
        wareboxes_persistence_postgres::roles::add_role_to_user(db, tenant_id, user_id, role,)
            .await
            .unwrap()
    );
}

async fn add_admin_approver(fixture: &Fixture, tenant_id: TenantId) -> String {
    let user = fixture.user("pick-policy-approver@test.local").await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships(tenant_id,user_id) VALUES ($1,$2)")
        .bind(tenant_id.get())
        .bind(user.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    grant_permission(
        &fixture.db,
        tenant_id,
        user.id,
        "pick-policy-approver",
        "admin",
    )
    .await;
    auth::create_session(&fixture.db, user.id).await.unwrap()
}

async fn transition(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    configuration: &ConfigurationResponse,
    transition: &str,
    key: &str,
) -> ConfigurationResponse {
    let response = send(
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
    .await;
    response_json(expect_status(response, StatusCode::OK, transition).await).await
}

async fn activate_pick_policy(
    app: &axum::Router,
    creator_token: &str,
    approver_token: &str,
    tenant_id: TenantId,
    owner_id: i64,
    facility_id: i64,
) -> ConfigurationResponse {
    let created_response = send(
        app,
        creator_token,
        tenant_id,
        Method::POST,
        "/api/v1/configurations",
        Some("pick-policy-create"),
        Some(
            serde_json::to_value(CreateConfigurationRequest {
                scope: ConfigurationScope::OwnerFacility {
                    inventory_owner_id: owner_id,
                    facility_id,
                },
                effective_from: "2026-01-01T00:00:00Z".into(),
                effective_until: None,
                rule: DecisionRule::Pick {
                    require_source_location_scan: false,
                    require_item_scan: false,
                    require_destination_container_scan: false,
                },
                expected_revision: None,
            })
            .unwrap(),
        ),
    )
    .await;
    let created: ConfigurationResponse =
        response_json(expect_status(created_response, StatusCode::OK, "create Pick policy").await)
            .await;
    let submitted = transition(
        app,
        creator_token,
        tenant_id,
        &created,
        "submissions",
        "pick-policy-submit",
    )
    .await;
    let approved = transition(
        app,
        approver_token,
        tenant_id,
        &submitted,
        "approvals",
        "pick-policy-approve",
    )
    .await;
    transition(
        app,
        creator_token,
        tenant_id,
        &approved,
        "activations",
        "pick-policy-activate",
    )
    .await
}

#[tokio::test]
async fn effective_pick_policy_controls_scans_and_freezes_auditable_evidence() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("pick-policy-operator@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "pick-policy-orders",
    )
    .await;
    grant_permission(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "pick-policy-creator",
        "admin",
    )
    .await;
    let approver_token = add_admin_approver(&fixture, access.tenant_id).await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Pick Policy Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Pick Policy Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let destination_id =
        staging_location(&fixture, access.tenant_id, facility_id, "PICK-POLICY-STAGE").await;
    let destination_plate_id = plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        destination_id,
        "PICK-POLICY-TOTE",
    )
    .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let configuration = activate_pick_policy(
        &app,
        &token,
        &approver_token,
        access.tenant_id,
        owner_id,
        facility_id,
    )
    .await;
    let order = allocated_order(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        "PICK-POLICY",
        &[2, 3],
        &[2, 3],
    )
    .await;
    let released = release(
        &app,
        &token,
        access.tenant_id,
        order.order_id,
        Some("pick-policy-release"),
        release_body(facility_id, destination_id, 2),
    )
    .await;
    expect_status(released, StatusCode::OK, "release policy order").await;

    let first_claim = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/picking-claims/next",
        Some("pick-policy-first-claim"),
        Some(json!({})),
    )
    .await;
    let first_claim: PickClaimResponse = response_json::<Option<PickClaimResponse>>(
        expect_status(first_claim, StatusCode::OK, "first policy claim").await,
    )
    .await
    .unwrap();
    assert_eq!(
        first_claim.pick_policy.source,
        PickDecisionPolicySource::Configuration
    );
    assert_eq!(
        first_claim.pick_policy.configuration_id,
        Some(configuration.configuration_id)
    );
    assert!(!first_claim.pick_policy.require_source_location_scan);
    assert!(!first_claim.pick_policy.require_item_scan);
    assert!(!first_claim.pick_policy.require_destination_container_scan);
    assert!(first_claim
        .suggested_destination_license_plate_barcode
        .is_none());

    let released_claim = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/picking-claims/{}/releases", first_claim.task_id),
        Some("pick-policy-release-claim"),
        Some(json!({"reason": "work_interrupted"})),
    )
    .await;
    expect_status(
        released_claim,
        StatusCode::OK,
        "release policy-controlled claim",
    )
    .await;
    let reclaimed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/picking-claims/{}", first_claim.task_id),
        Some("pick-policy-reclaim"),
        Some(json!({})),
    )
    .await;
    let reclaimed: PickClaimResponse = response_json(
        expect_status(reclaimed, StatusCode::OK, "reclaim policy-controlled task").await,
    )
    .await;
    assert_eq!(reclaimed.pick_policy, first_claim.pick_policy);

    let first_confirmation = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/picking-tasks/{}/contents/{}/confirmations",
            first_claim.task_id, first_claim.content.content_id
        ),
        Some("pick-policy-first-confirm"),
        Some(json!({
            "destination_license_plate_barcode": "PICK-POLICY-TOTE"
        })),
    )
    .await;
    let first_confirmation: PickContentConfirmationResponse = response_json(
        expect_status(
            first_confirmation,
            StatusCode::OK,
            "fallback destination scan confirmation",
        )
        .await,
    )
    .await;
    assert!(!first_confirmation.source_location_scan_verified);
    assert!(!first_confirmation.item_scan_verified);
    assert!(first_confirmation.destination_container_scan_verified);
    assert_eq!(
        first_confirmation.destination_license_plate_id,
        destination_plate_id
    );

    let second_claim = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/picking-claims/next",
        Some("pick-policy-second-claim"),
        Some(json!({})),
    )
    .await;
    let second_claim: PickClaimResponse = response_json::<Option<PickClaimResponse>>(
        expect_status(second_claim, StatusCode::OK, "second policy claim").await,
    )
    .await
    .unwrap();
    assert_eq!(
        second_claim
            .suggested_destination_license_plate_barcode
            .as_deref(),
        Some("PICK-POLICY-TOTE")
    );
    let second_confirmation = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/picking-tasks/{}/contents/{}/confirmations",
            second_claim.task_id, second_claim.content.content_id
        ),
        Some("pick-policy-second-confirm"),
        Some(json!({})),
    )
    .await;
    let second_confirmation: PickContentConfirmationResponse = response_json(
        expect_status(
            second_confirmation,
            StatusCode::OK,
            "inferred scan confirmation",
        )
        .await,
    )
    .await;
    assert!(!second_confirmation.source_location_scan_verified);
    assert!(!second_confirmation.item_scan_verified);
    assert!(!second_confirmation.destination_container_scan_verified);
    assert_eq!(second_confirmation.pick_policy, first_claim.pick_policy);
    let second_replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/picking-tasks/{}/contents/{}/confirmations",
            second_claim.task_id, second_claim.content.content_id
        ),
        Some("pick-policy-second-confirm"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        response_json::<PickContentConfirmationResponse>(
            expect_status(
                second_replay,
                StatusCode::OK,
                "replay inferred confirmation"
            )
            .await,
        )
        .await,
        second_confirmation
    );
    let history = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &format!(
            "/api/v1/orders/{}/pick-confirmations?limit=10",
            order.order_id
        ),
        None,
        None,
    )
    .await;
    let history: PickConfirmationHistoryPage =
        response_json(expect_status(history, StatusCode::OK, "policy confirmation history").await)
            .await;
    assert_eq!(history.items.len(), 2);
    assert!(history.items.iter().all(|item| {
        item.pick_policy == first_claim.pick_policy
            && !item.source_location_scan_verified
            && !item.item_scan_verified
    }));

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let evidence: (i64, i64, bool, bool, bool, i64) = sqlx::query_as(
        r#"SELECT COUNT(*),COUNT(DISTINCT confirmation.pick_policy_hash),
        BOOL_AND(NOT confirmation.source_location_scan_verified),
        BOOL_AND(NOT confirmation.item_scan_verified),
        BOOL_OR(confirmation.destination_container_scan_verified),
        (SELECT COUNT(*) FROM outbox_events event
         WHERE event.tenant_id=$1 AND event.event_type='outbound.pick.confirmed'
           AND event.payload->'pick_policy'->>'configuration_id'=$3::text)
        FROM pick_confirmations confirmation
        WHERE confirmation.tenant_id=$1 AND confirmation.order_id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(order.order_id)
    .bind(configuration.configuration_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(evidence, (2, 1, true, true, true, 2));
    let tamper =
        sqlx::query("UPDATE pick_tasks SET require_item_scan=true WHERE tenant_id=$1 AND id=$2")
            .bind(access.tenant_id.get())
            .bind(second_claim.task_id)
            .execute(&mut *tx)
            .await;
    assert!(tamper.is_err());
    tx.rollback().await.unwrap();
}
