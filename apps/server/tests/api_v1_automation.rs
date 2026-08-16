mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{auth, routes, state::AppState};
use wareboxes_api_contract::v1::{
    AcknowledgeAutomationCommandRequest, AutomationCommandDeliveryPage, AutomationCommandResponse,
    AutomationCommandResult, AutomationCommandStatus, AutomationControlMode, AutomationDeviceClass,
    AutomationDeviceCommand, AutomationDeviceResponse, AutomationEdgeDevicePage,
    AutomationHealthState, AutomationManualResolution, AutomationRecoveryPolicy,
    AutomationScaleCommand, AutomationScaleResult, AutomationWeightUnit,
    AutomationWorkspaceResponse, ChangeAutomationControlRequest, CreateServiceAccountRequest,
    EnqueueAutomationCommandRequest, IssueServiceAccountCredentialRequest,
    IssuedServiceAccountCredentialResponse, PullAutomationCommandsRequest,
    RecordAutomationHeartbeatRequest, RegisterAutomationDeviceRequest,
    ReportAutomationCommandRequest, ResolveAutomationCommandRequest, Revision,
    ServiceAccountAccessRequest, ServiceAccountResponse,
};

fn request<T: Serialize>(
    token: &str,
    tenant_id: TenantId,
    method: Method,
    uri: &str,
    key: Option<&str>,
    body: &T,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    if let Some(key) = key {
        builder = builder
            .header(IDEMPOTENCY_KEY_HEADER, key)
            .header(header::CONTENT_TYPE, "application/json");
    }
    builder
        .body(if key.is_some() {
            Body::from(serde_json::to_vec(body).unwrap())
        } else {
            Body::empty()
        })
        .unwrap()
}

async fn response<T: DeserializeOwned>(
    response: axum::response::Response,
    expected: StatusCode,
) -> T {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(
        status,
        expected,
        "unexpected response: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

async fn grant(fixture: &Fixture, tenant_id: TenantId, user_id: i64, names: &[&str]) {
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("automation-test-{user_id}"),
        Some("Automation acceptance role"),
    )
    .await
    .unwrap();
    for name in names {
        let permission = wareboxes_persistence_postgres::permissions::add_permission(
            &fixture.db,
            tenant_id,
            name,
            Some("Automation acceptance permission"),
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
    }
    wareboxes_persistence_postgres::roles::add_role_to_user(&fixture.db, tenant_id, user_id, role)
        .await
        .unwrap();
}

async fn edge_identity(
    app: &axum::Router,
    manager_token: &str,
    tenant_id: TenantId,
    facility_id: i64,
    identity_tag: &str,
    token_character: char,
) -> (ServiceAccountResponse, String) {
    let created: ServiceAccountResponse = response(
        app.clone()
            .oneshot(request(
                manager_token,
                tenant_id,
                Method::POST,
                "/api/v1/service-accounts",
                Some(&format!("create-edge-agent-{identity_tag}")),
                &CreateServiceAccountRequest {
                    name: format!("Facility edge agent {identity_tag}"),
                    description: Some("Outbound local automation connection".into()),
                    access: ServiceAccountAccessRequest {
                        all_facilities: false,
                        facility_ids: vec![facility_id],
                        all_inventory_owners: true,
                        inventory_owner_ids: vec![],
                        permission_names: vec!["automation_edge".into()],
                    },
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let bearer = format!("wbs_sa_{}", token_character.to_string().repeat(48));
    let issued: IssuedServiceAccountCredentialResponse = response(
        app.clone()
            .oneshot(request(
                manager_token,
                tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/service-accounts/{}/credentials",
                    created.service_account_id
                ),
                Some(&format!("issue-edge-agent-{identity_tag}")),
                &IssueServiceAccountCredentialRequest {
                    expected_revision: Revision::new(1).unwrap(),
                    label: format!("edge outbound credential {identity_tag}"),
                    expires_at: None,
                    bearer_token: bearer.clone(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        issued.service_account.service_account_id,
        created.service_account_id
    );
    (issued.service_account, bearer)
}

#[tokio::test]
async fn automation_commands_cross_the_cloud_edge_boundary_once_with_frozen_evidence() {
    let fixture = Fixture::new().await;
    let manager = fixture.user("automation-manager@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, manager.id).await;
    grant(
        &fixture,
        tenant_id,
        manager.id,
        &["admin", "wms_supervisor", "automation_edge"],
    )
    .await;
    let facility_id = fixture.facility(tenant_id, "Automated DC").await;
    let unassigned_facility_id = fixture.facility(tenant_id, "Manual DC").await;
    let other_tenant = fixture.user("automation-other@test.local").await;
    let other_tenant_id = tenant_for_user(&fixture.db, other_tenant.id).await;
    let manager_token = auth::create_session(&fixture.db, manager.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let (edge_account, edge_token) =
        edge_identity(&app, &manager_token, tenant_id, facility_id, "a", 'E').await;

    let registered: AutomationDeviceResponse = response(
        app.clone()
            .oneshot(request(
                &manager_token,
                tenant_id,
                Method::POST,
                "/api/v1/automation/devices",
                Some("register-pack-scale"),
                &RegisterAutomationDeviceRequest {
                    facility_id,
                    device_key: "pack-scale-01".into(),
                    class: AutomationDeviceClass::Scale,
                    display_name: "Pack scale 01".into(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(registered.control_mode, AutomationControlMode::Disabled);
    assert_eq!(registered.health, AutomationHealthState::Unknown);
    let assigned_devices: AutomationEdgeDevicePage = response(
        app.clone()
            .oneshot(request(
                &edge_token,
                tenant_id,
                Method::GET,
                &format!("/api/v1/edge/automation/devices?facility_id={facility_id}"),
                None,
                &(),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(assigned_devices.items, vec![registered.clone()]);

    let observed_at = chrono::Utc::now().to_rfc3339();
    let heartbeat: wareboxes_api_contract::v1::AutomationHeartbeatResponse = response(
        app.clone()
            .oneshot(request(
                &edge_token,
                tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/edge/automation/devices/{}/heartbeats",
                    registered.device_id
                ),
                Some("edge-heartbeat-1"),
                &RecordAutomationHeartbeatRequest {
                    agent_instance: "edge-host-a/boot-1".into(),
                    health: AutomationHealthState::Healthy,
                    control_mode: AutomationControlMode::Disabled,
                    message: Some("controller links ready".into()),
                    queued_commands: 0,
                    manual_review_commands: 0,
                    observed_at,
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        heartbeat.service_account_id,
        edge_account.service_account_id
    );
    let stale_heartbeat = app
        .clone()
        .oneshot(request(
            &edge_token,
            tenant_id,
            Method::POST,
            &format!(
                "/api/v1/edge/automation/devices/{}/heartbeats",
                registered.device_id
            ),
            Some("edge-heartbeat-stale"),
            &RecordAutomationHeartbeatRequest {
                agent_instance: "edge-host-a/boot-1".into(),
                health: AutomationHealthState::Faulted,
                control_mode: AutomationControlMode::Disabled,
                message: Some("stale fault sample".into()),
                queued_commands: 0,
                manual_review_commands: 0,
                observed_at: heartbeat.observed_at.clone(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(stale_heartbeat.status(), StatusCode::CONFLICT);

    let enabled: AutomationDeviceResponse = response(
        app.clone()
            .oneshot(request(
                &manager_token,
                tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/automation/devices/{}/control-changes",
                    registered.device_id
                ),
                Some("enable-pack-scale"),
                &ChangeAutomationControlRequest {
                    expected_revision: registered.revision,
                    target_mode: AutomationControlMode::Automatic,
                    reason: "local guarding checked and command queue reconciled".into(),
                    safety_confirmation: Some("CONFIRM-SAFE-TO-RESUME".into()),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(enabled.control_mode, AutomationControlMode::Automatic);
    let local_automatic: wareboxes_api_contract::v1::AutomationHeartbeatResponse = response(
        app.clone()
            .oneshot(request(
                &edge_token,
                tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/edge/automation/devices/{}/heartbeats",
                    registered.device_id
                ),
                Some("edge-heartbeat-automatic"),
                &RecordAutomationHeartbeatRequest {
                    agent_instance: "edge-host-a/boot-1".into(),
                    health: AutomationHealthState::Healthy,
                    control_mode: AutomationControlMode::Automatic,
                    message: Some("local safety interlock permits automatic work".into()),
                    queued_commands: 0,
                    manual_review_commands: 0,
                    observed_at: chrono::Utc::now().to_rfc3339(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        local_automatic.control_mode,
        AutomationControlMode::Automatic
    );

    let enqueue = EnqueueAutomationCommandRequest {
        correlation_id: "pack-carton-100-weight".into(),
        recovery_policy: AutomationRecoveryPolicy::ManualReview,
        command: AutomationDeviceCommand::Scale(AutomationScaleCommand::ReadStableWeight {
            requested_unit: AutomationWeightUnit::Gram,
            timeout_ms: 5_000,
        }),
    };
    let queued: AutomationCommandResponse = response(
        app.clone()
            .oneshot(request(
                &manager_token,
                tenant_id,
                Method::POST,
                &format!("/api/v1/automation/devices/{}/commands", enabled.device_id),
                Some("enqueue-carton-weight"),
                &enqueue,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(queued.status, AutomationCommandStatus::Queued);
    let exact_queue: AutomationCommandResponse = response(
        app.clone()
            .oneshot(request(
                &manager_token,
                tenant_id,
                Method::POST,
                &format!("/api/v1/automation/devices/{}/commands", enabled.device_id),
                Some("enqueue-carton-weight"),
                &enqueue,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(exact_queue, queued);
    let duplicate_correlation = app
        .clone()
        .oneshot(request(
            &manager_token,
            tenant_id,
            Method::POST,
            &format!("/api/v1/automation/devices/{}/commands", enabled.device_id),
            Some("enqueue-carton-weight-different-command"),
            &enqueue,
        ))
        .await
        .unwrap();
    assert_eq!(duplicate_correlation.status(), StatusCode::CONFLICT);

    let pull = PullAutomationCommandsRequest {
        facility_id,
        agent_instance: "edge-host-a/boot-1".into(),
        limit: 10,
    };
    let delivery: AutomationCommandDeliveryPage = response(
        app.clone()
            .oneshot(request(
                &edge_token,
                tenant_id,
                Method::POST,
                "/api/v1/edge/automation/command-pulls",
                Some("pull-1"),
                &pull,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(delivery.items.len(), 1);
    let delivered = &delivery.items[0];
    assert_eq!(delivered.command.command_id, queued.command_id);
    assert_eq!(delivered.command.status, AutomationCommandStatus::Delivered);
    assert_eq!(delivered.command.delivery_attempts, 1);
    let exact_delivery: AutomationCommandDeliveryPage = response(
        app.clone()
            .oneshot(request(
                &edge_token,
                tenant_id,
                Method::POST,
                "/api/v1/edge/automation/command-pulls",
                Some("pull-1"),
                &pull,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(exact_delivery, delivery);

    let accepted: AutomationCommandResponse = response(
        app.clone()
            .oneshot(request(
                &edge_token,
                tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/edge/automation/commands/{}/acknowledgements",
                    queued.command_id
                ),
                Some("ack-1"),
                &AcknowledgeAutomationCommandRequest {
                    delivery_token: delivered.delivery_token.clone(),
                    expected_revision: delivered.command.revision,
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(accepted.status, AutomationCommandStatus::Accepted);

    let report = ReportAutomationCommandRequest {
        expected_revision: accepted.revision,
        status: AutomationCommandStatus::Succeeded,
        result: Some(AutomationCommandResult::Scale(AutomationScaleResult {
            mass_milligrams: 1_250_000,
            stable: true,
        })),
        error_code: None,
        error_message: None,
        occurred_at: chrono::Utc::now().to_rfc3339(),
    };
    let succeeded: AutomationCommandResponse = response(
        app.clone()
            .oneshot(request(
                &edge_token,
                tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/edge/automation/commands/{}/reports",
                    queued.command_id
                ),
                Some("report-1"),
                &report,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(succeeded.status, AutomationCommandStatus::Succeeded);
    assert_eq!(succeeded.result, report.result);
    let exact_report: AutomationCommandResponse = response(
        app.clone()
            .oneshot(request(
                &edge_token,
                tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/edge/automation/commands/{}/reports",
                    queued.command_id
                ),
                Some("report-1"),
                &report,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(exact_report, succeeded);

    let review_queued: AutomationCommandResponse = response(
        app.clone()
            .oneshot(request(
                &manager_token,
                tenant_id,
                Method::POST,
                &format!("/api/v1/automation/devices/{}/commands", enabled.device_id),
                Some("enqueue-scale-tare-review"),
                &EnqueueAutomationCommandRequest {
                    correlation_id: "pack-scale-tare-review".into(),
                    recovery_policy: AutomationRecoveryPolicy::ManualReview,
                    command: AutomationDeviceCommand::Scale(AutomationScaleCommand::Tare),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let review_delivery: AutomationCommandDeliveryPage = response(
        app.clone()
            .oneshot(request(
                &edge_token,
                tenant_id,
                Method::POST,
                "/api/v1/edge/automation/command-pulls",
                Some("pull-review-command"),
                &pull,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(review_delivery.items.len(), 1);
    let review_delivery = &review_delivery.items[0];
    assert_eq!(review_delivery.command.command_id, review_queued.command_id);
    let review_accepted: AutomationCommandResponse = response(
        app.clone()
            .oneshot(request(
                &edge_token,
                tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/edge/automation/commands/{}/acknowledgements",
                    review_queued.command_id
                ),
                Some("ack-review-command"),
                &AcknowledgeAutomationCommandRequest {
                    delivery_token: review_delivery.delivery_token.clone(),
                    expected_revision: review_delivery.command.revision,
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let manual_review: AutomationCommandResponse = response(
        app.clone()
            .oneshot(request(
                &edge_token,
                tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/edge/automation/commands/{}/reports",
                    review_queued.command_id
                ),
                Some("report-manual-review"),
                &ReportAutomationCommandRequest {
                    expected_revision: review_accepted.revision,
                    status: AutomationCommandStatus::ManualReview,
                    result: None,
                    error_code: Some("AMBIGUOUS_CONTROLLER_TIMEOUT".into()),
                    error_message: Some(
                        "controller timed out after accepting the tare request".into(),
                    ),
                    occurred_at: chrono::Utc::now().to_rfc3339(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let blocked_enqueue = app
        .clone()
        .oneshot(request(
            &manager_token,
            tenant_id,
            Method::POST,
            &format!("/api/v1/automation/devices/{}/commands", enabled.device_id),
            Some("enqueue-while-review-open"),
            &EnqueueAutomationCommandRequest {
                correlation_id: "blocked-by-manual-review".into(),
                recovery_policy: AutomationRecoveryPolicy::ManualReview,
                command: AutomationDeviceCommand::Scale(AutomationScaleCommand::Tare),
            },
        ))
        .await
        .unwrap();
    assert_eq!(blocked_enqueue.status(), StatusCode::CONFLICT);
    let fallback: AutomationDeviceResponse = response(
        app.clone()
            .oneshot(request(
                &manager_token,
                tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/automation/devices/{}/control-changes",
                    enabled.device_id
                ),
                Some("manual-fallback-for-reconciliation"),
                &ChangeAutomationControlRequest {
                    expected_revision: enabled.revision,
                    target_mode: AutomationControlMode::ManualFallback,
                    reason: "ambiguous tare requires physical reconciliation".into(),
                    safety_confirmation: None,
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let blocked_resume = app
        .clone()
        .oneshot(request(
            &manager_token,
            tenant_id,
            Method::POST,
            &format!(
                "/api/v1/automation/devices/{}/control-changes",
                enabled.device_id
            ),
            Some("resume-before-reconciliation"),
            &ChangeAutomationControlRequest {
                expected_revision: fallback.revision,
                target_mode: AutomationControlMode::Automatic,
                reason: "attempted before evidence was reconciled".into(),
                safety_confirmation: Some("CONFIRM-SAFE-TO-RESUME".into()),
            },
        ))
        .await
        .unwrap();
    assert_eq!(blocked_resume.status(), StatusCode::CONFLICT);
    let resolution_request = ResolveAutomationCommandRequest {
        expected_revision: manual_review.revision,
        outcome: AutomationManualResolution::ConfirmedExecuted,
        reason: "local controller audit confirms tare sequence 847 completed".into(),
    };
    let resolved: AutomationCommandResponse = response(
        app.clone()
            .oneshot(request(
                &manager_token,
                tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/automation/commands/{}/resolutions",
                    manual_review.command_id
                ),
                Some("resolve-tare-review"),
                &resolution_request,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(resolved.status, AutomationCommandStatus::ResolvedManually);
    assert_eq!(
        resolved.resolution_outcome,
        Some(AutomationManualResolution::ConfirmedExecuted)
    );
    let exact_resolution: AutomationCommandResponse = response(
        app.clone()
            .oneshot(request(
                &manager_token,
                tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/automation/commands/{}/resolutions",
                    manual_review.command_id
                ),
                Some("resolve-tare-review"),
                &resolution_request,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(exact_resolution, resolved);
    let resumed: AutomationDeviceResponse = response(
        app.clone()
            .oneshot(request(
                &manager_token,
                tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/automation/devices/{}/control-changes",
                    enabled.device_id
                ),
                Some("resume-after-reconciliation"),
                &ChangeAutomationControlRequest {
                    expected_revision: fallback.revision,
                    target_mode: AutomationControlMode::Automatic,
                    reason: "manual review reconciled against the controller audit".into(),
                    safety_confirmation: Some("CONFIRM-SAFE-TO-RESUME".into()),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(resumed.control_mode, AutomationControlMode::Automatic);

    let workspace: AutomationWorkspaceResponse = response(
        app.clone()
            .oneshot(request(
                &manager_token,
                tenant_id,
                Method::GET,
                &format!(
                    "/api/v1/automation/workspace?facility_id={facility_id}&include_history=true"
                ),
                None,
                &(),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(workspace.devices.len(), 1);
    assert_eq!(workspace.commands, vec![resolved.clone(), succeeded]);
    assert_eq!(workspace.heartbeats.len(), 2);

    let wrong_tenant = app
        .clone()
        .oneshot(request(
            &edge_token,
            other_tenant_id,
            Method::POST,
            "/api/v1/edge/automation/command-pulls",
            Some("wrong-tenant-pull"),
            &pull,
        ))
        .await
        .unwrap();
    assert_eq!(wrong_tenant.status(), StatusCode::FORBIDDEN);
    let wrong_facility = app
        .clone()
        .oneshot(request(
            &edge_token,
            tenant_id,
            Method::POST,
            "/api/v1/edge/automation/command-pulls",
            Some("wrong-facility-pull"),
            &PullAutomationCommandsRequest {
                facility_id: unassigned_facility_id,
                agent_instance: "edge-host-a/boot-1".into(),
                limit: 10,
            },
        ))
        .await
        .unwrap();
    assert_eq!(wrong_facility.status(), StatusCode::NOT_FOUND);

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let tamper = sqlx::query(
        "UPDATE automation_commands SET result_payload='{}'::jsonb WHERE tenant_id=$1 AND id=$2",
    )
    .bind(tenant_id.get())
    .bind(queued.command_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert_eq!(
        tamper
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23514")
    );
    tx.rollback().await.unwrap();

    let mut outbox_tx = tenant_tx(&fixture.db, tenant_id).await;
    let outbox_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox_events WHERE tenant_id=$1 AND aggregate_type='automation_command' AND aggregate_id=$2 AND facility_id=$3",
    )
    .bind(tenant_id.get())
    .bind(queued.command_id.to_string())
    .bind(facility_id)
    .fetch_one(&mut *outbox_tx)
    .await
    .unwrap();
    assert_eq!(outbox_count, 2);
    let resolution_outbox_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox_events WHERE tenant_id=$1 AND aggregate_type='automation_command' AND aggregate_id=$2 AND facility_id=$3",
    )
    .bind(tenant_id.get())
    .bind(resolved.command_id.to_string())
    .bind(facility_id)
    .fetch_one(&mut *outbox_tx)
    .await
    .unwrap();
    assert_eq!(resolution_outbox_count, 3);
    outbox_tx.commit().await.unwrap();

    let (_replacement_edge_account, replacement_edge_token) =
        edge_identity(&app, &manager_token, tenant_id, facility_id, "b", 'F').await;
    let replacement_instance = "edge-host-b/boot-1";
    let _replacement_heartbeat: wareboxes_api_contract::v1::AutomationHeartbeatResponse = response(
        app.clone()
            .oneshot(request(
                &replacement_edge_token,
                tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/edge/automation/devices/{}/heartbeats",
                    resumed.device_id
                ),
                Some("replacement-edge-heartbeat"),
                &RecordAutomationHeartbeatRequest {
                    agent_instance: replacement_instance.into(),
                    health: AutomationHealthState::Healthy,
                    control_mode: AutomationControlMode::Automatic,
                    message: Some("replacement agent owns the live controller link".into()),
                    queued_commands: 0,
                    manual_review_commands: 0,
                    observed_at: (chrono::Utc::now() + chrono::Duration::seconds(1)).to_rfc3339(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;

    let concurrent_queued: AutomationCommandResponse = response(
        app.clone()
            .oneshot(request(
                &manager_token,
                tenant_id,
                Method::POST,
                &format!("/api/v1/automation/devices/{}/commands", resumed.device_id),
                Some("enqueue-concurrent-pull"),
                &EnqueueAutomationCommandRequest {
                    correlation_id: "concurrent-edge-pull".into(),
                    recovery_policy: AutomationRecoveryPolicy::ManualReview,
                    command: AutomationDeviceCommand::Scale(AutomationScaleCommand::Tare),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let superseded_agent_pull: AutomationCommandDeliveryPage = response(
        app.clone()
            .oneshot(request(
                &edge_token,
                tenant_id,
                Method::POST,
                "/api/v1/edge/automation/command-pulls",
                Some("superseded-agent-pull"),
                &pull,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert!(superseded_agent_pull.items.is_empty());
    let replacement_pull = PullAutomationCommandsRequest {
        facility_id,
        agent_instance: replacement_instance.into(),
        limit: 10,
    };
    let first_pull = app.clone().oneshot(request(
        &replacement_edge_token,
        tenant_id,
        Method::POST,
        "/api/v1/edge/automation/command-pulls",
        Some("concurrent-pull-a"),
        &replacement_pull,
    ));
    let second_pull = app.clone().oneshot(request(
        &replacement_edge_token,
        tenant_id,
        Method::POST,
        "/api/v1/edge/automation/command-pulls",
        Some("concurrent-pull-b"),
        &replacement_pull,
    ));
    let (first_response, second_response) = tokio::join!(first_pull, second_pull);
    let first_page: AutomationCommandDeliveryPage =
        response(first_response.unwrap(), StatusCode::OK).await;
    let second_page: AutomationCommandDeliveryPage =
        response(second_response.unwrap(), StatusCode::OK).await;
    assert_eq!(first_page.items.len() + second_page.items.len(), 1);
    let delivered_once = first_page
        .items
        .first()
        .or_else(|| second_page.items.first());
    assert_eq!(
        delivered_once.map(|item| item.command.command_id),
        Some(concurrent_queued.command_id)
    );
}
