use super::super::*;
use wareboxes_api_contract::v1::{
    AutomationCommandDeliveryPage, AutomationCommandResponse, AutomationCommandStatus,
    AutomationControlMode, AutomationDeviceResponse, AutomationHealthState,
    CartonWeightEvidenceResponse, IssuedServiceAccountCredentialResponse, PackingScaleDevicePage,
    ReopenCartonResponse, ServiceAccountResponse,
};

async fn grant_permissions(fixture: &Fixture, tenant_id: TenantId, user_id: i64, names: &[&str]) {
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("packing-scale-manager-{user_id}"),
        Some("Packing scale acceptance manager"),
    )
    .await
    .unwrap();
    for name in names {
        let permission = match wareboxes_persistence_postgres::permissions::find_by_name(
            &fixture.db,
            tenant_id,
            name,
        )
        .await
        .unwrap()
        {
            Some(permission) => permission.id,
            None => wareboxes_persistence_postgres::permissions::add_permission(
                &fixture.db,
                tenant_id,
                name,
                Some("Packing scale acceptance permission"),
            )
            .await
            .unwrap(),
        };
        assert!(wareboxes_persistence_postgres::roles::add_role_permission(
            &fixture.db,
            tenant_id,
            role,
            permission,
        )
        .await
        .unwrap());
    }
    assert!(wareboxes_persistence_postgres::roles::add_role_to_user(
        &fixture.db,
        tenant_id,
        user_id,
        role,
    )
    .await
    .unwrap());
}

#[tokio::test]
async fn carton_weight_uses_one_replay_safe_scale_reading_with_immutable_provenance() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("packing-scale-operator@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "packing-scale-orders",
    )
    .await;
    let manager = fixture.user("packing-scale-manager@test.local").await;
    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships(tenant_id,user_id) VALUES($1,$2)")
        .bind(access.tenant_id.get())
        .bind(manager.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    grant_permissions(
        &fixture,
        access.tenant_id,
        manager.id,
        &["admin", "wms_supervisor", "automation_edge"],
    )
    .await;

    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Packing Scale Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Packing Scale Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let packing_location_id = execution_location(
        &fixture,
        access.tenant_id,
        facility_id,
        "PACK-SCALE-STATION",
        "packing",
    )
    .await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        packing_location_id,
        "PACK-SCALE-TOTE",
    )
    .await;
    let operator_token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let manager_token = auth::create_session(&fixture.db, manager.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let order = prepare_order(
        &fixture,
        &app,
        &operator_token,
        &access,
        owner_id,
        facility_id,
        "PACK-SCALE",
        &[1],
    )
    .await;
    release_order(
        &app,
        &operator_token,
        access.tenant_id,
        order.order_id,
        facility_id,
        packing_location_id,
        "pack-scale-release",
    )
    .await;
    pick_order(
        &app,
        &operator_token,
        access.tenant_id,
        "PACK-SCALE-TOTE",
        1,
        "pack-scale",
    )
    .await;
    let opened = open_session(
        &app,
        &operator_token,
        access.tenant_id,
        order.order_id,
        facility_id,
        packing_location_id,
        "pack-scale-open",
    )
    .await;
    let carton = create_carton(
        &app,
        &operator_token,
        access.tenant_id,
        opened.session.session_id,
        "PACK-SCALE-CARTON",
        opened.session.revision.get(),
        "pack-scale-carton",
    )
    .await;
    let allocation = &opened.session.allocations[0];
    let packed = send(
        &app,
        &operator_token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/packing-sessions/{}/cartons/{}/contents",
            opened.session.session_id, carton.carton.carton_id
        ),
        Some("pack-scale-content"),
        Some(pack_body(
            allocation.inventory_allocation_id,
            &allocation.item_barcodes[0],
            allocation.lot.as_deref().unwrap(),
            "PACK-SCALE-TOTE",
            "PACK-SCALE-CARTON",
            carton.revision.get(),
        )),
    )
    .await;
    let packed: PackPickedAllocationResponse =
        response_json(expect_status(packed, StatusCode::OK, "pack scale content").await).await;

    let service_account: ServiceAccountResponse = response_json(
        expect_status(
            send(
                &app,
                &manager_token,
                access.tenant_id,
                Method::POST,
                "/api/v1/service-accounts",
                Some("pack-scale-edge-account"),
                Some(json!({
                    "name": "Packing scale edge",
                    "description": "Acceptance edge identity",
                    "access": {
                        "all_facilities": false,
                        "facility_ids": [facility_id],
                        "all_inventory_owners": true,
                        "inventory_owner_ids": [],
                        "permission_names": ["automation_edge"]
                    }
                })),
            )
            .await,
            StatusCode::OK,
            "create packing edge account",
        )
        .await,
    )
    .await;
    let edge_token = format!("wbs_sa_{}", "S".repeat(48));
    let _: IssuedServiceAccountCredentialResponse = response_json(
        expect_status(
            send(
                &app,
                &manager_token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/service-accounts/{}/credentials",
                    service_account.service_account_id
                ),
                Some("pack-scale-edge-credential"),
                Some(json!({
                    "expected_revision": 1,
                    "label": "packing scale edge credential",
                    "expires_at": null,
                    "bearer_token": edge_token.clone()
                })),
            )
            .await,
            StatusCode::OK,
            "issue packing edge credential",
        )
        .await,
    )
    .await;
    let device: AutomationDeviceResponse = response_json(
        expect_status(
            send(
                &app,
                &manager_token,
                access.tenant_id,
                Method::POST,
                "/api/v1/automation/devices",
                Some("pack-scale-register"),
                Some(json!({
                    "facility_id": facility_id,
                    "device_key": "packing-scale-01",
                    "class": "scale",
                    "display_name": "Packing scale 01"
                })),
            )
            .await,
            StatusCode::OK,
            "register packing scale",
        )
        .await,
    )
    .await;
    let _: Value = response_json(
        expect_status(
            send(
                &app,
                &edge_token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/edge/automation/devices/{}/heartbeats",
                    device.device_id
                ),
                Some("pack-scale-heartbeat-disabled"),
                Some(json!({
                    "agent_instance": "pack-edge/boot-1",
                    "health": "healthy",
                    "control_mode": "disabled",
                    "message": "scale ready",
                    "queued_commands": 0,
                    "manual_review_commands": 0,
                    "observed_at": chrono::Utc::now().to_rfc3339()
                })),
            )
            .await,
            StatusCode::OK,
            "record disabled scale heartbeat",
        )
        .await,
    )
    .await;
    let enabled: AutomationDeviceResponse = response_json(
        expect_status(
            send(
                &app,
                &manager_token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/automation/devices/{}/control-changes",
                    device.device_id
                ),
                Some("pack-scale-enable"),
                Some(json!({
                    "expected_revision": device.revision,
                    "target_mode": "automatic",
                    "reason": "scale interlock and calibration verified",
                    "safety_confirmation": "CONFIRM-SAFE-TO-RESUME"
                })),
            )
            .await,
            StatusCode::OK,
            "enable packing scale",
        )
        .await,
    )
    .await;
    assert_eq!(enabled.control_mode, AutomationControlMode::Automatic);
    let _: Value = response_json(
        expect_status(
            send(
                &app,
                &edge_token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/edge/automation/devices/{}/heartbeats",
                    device.device_id
                ),
                Some("pack-scale-heartbeat-auto"),
                Some(json!({
                    "agent_instance": "pack-edge/boot-1",
                    "health": "healthy",
                    "control_mode": "automatic",
                    "message": "automatic stable reads enabled",
                    "queued_commands": 0,
                    "manual_review_commands": 0,
                    "observed_at": chrono::Utc::now().to_rfc3339()
                })),
            )
            .await,
            StatusCode::OK,
            "record automatic scale heartbeat",
        )
        .await,
    )
    .await;

    let scales: PackingScaleDevicePage = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::GET,
                &format!(
                    "/api/v1/packing-sessions/{}/scale-devices",
                    opened.session.session_id
                ),
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "list packing scales",
        )
        .await,
    )
    .await;
    assert_eq!(scales.items.len(), 1);
    assert_eq!(scales.items[0].health, AutomationHealthState::Healthy);
    let queued: AutomationCommandResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/packing-sessions/{}/scale-readings",
                    opened.session.session_id
                ),
                Some("pack-scale-read"),
                Some(json!({
                    "device_id": device.device_id,
                    "carton_id": carton.carton.carton_id,
                    "timeout_ms": 30000
                })),
            )
            .await,
            StatusCode::OK,
            "request packing scale reading",
        )
        .await,
    )
    .await;
    assert_eq!(queued.status, AutomationCommandStatus::Queued);
    assert_eq!(
        queued
            .packing_scale_context
            .expect("packing read freezes carton context")
            .carton_id,
        carton.carton.carton_id
    );
    let forbidden_tare = send(
        &app,
        &operator_token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/automation/devices/{}/commands", device.device_id),
        Some("pack-scale-forbidden-tare"),
        Some(json!({
            "correlation_id": "operator-tare",
            "recovery_policy": "manual_review",
            "command": {"device_class": "scale", "command": {"operation": "tare"}}
        })),
    )
    .await;
    assert_eq!(forbidden_tare.status(), StatusCode::FORBIDDEN);

    let delivery: AutomationCommandDeliveryPage = response_json(
        expect_status(
            send(
                &app,
                &edge_token,
                access.tenant_id,
                Method::POST,
                "/api/v1/edge/automation/command-pulls",
                Some("pack-scale-pull"),
                Some(json!({
                    "facility_id": facility_id,
                    "agent_instance": "pack-edge/boot-1",
                    "limit": 10
                })),
            )
            .await,
            StatusCode::OK,
            "pull packing scale command",
        )
        .await,
    )
    .await;
    let delivered = &delivery.items[0];
    let accepted: AutomationCommandResponse = response_json(
        expect_status(
            send(
                &app,
                &edge_token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/edge/automation/commands/{}/acknowledgements",
                    queued.command_id
                ),
                Some("pack-scale-ack"),
                Some(json!({
                    "delivery_token": delivered.delivery_token,
                    "expected_revision": delivered.command.revision
                })),
            )
            .await,
            StatusCode::OK,
            "ack packing scale command",
        )
        .await,
    )
    .await;
    let succeeded: AutomationCommandResponse = response_json(
        expect_status(
            send(
                &app,
                &edge_token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/edge/automation/commands/{}/reports",
                    queued.command_id
                ),
                Some("pack-scale-report"),
                Some(json!({
                    "expected_revision": accepted.revision,
                    "status": "succeeded",
                    "result": {"device_class": "scale", "result": {
                        "mass_milligrams": 1250000,
                        "stable": true
                    }},
                    "error_code": null,
                    "error_message": null,
                    "occurred_at": chrono::Utc::now().to_rfc3339()
                })),
            )
            .await,
            StatusCode::OK,
            "report packing scale result",
        )
        .await,
    )
    .await;
    assert_eq!(succeeded.status, AutomationCommandStatus::Succeeded);
    let reading: AutomationCommandResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::GET,
                &format!(
                    "/api/v1/packing-sessions/{}/scale-readings/{}",
                    opened.session.session_id, queued.command_id
                ),
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "read completed packing weight",
        )
        .await,
    )
    .await;
    assert_eq!(reading, succeeded);

    let closure_path = format!(
        "/api/v1/packing-sessions/{}/cartons/{}/closures",
        opened.session.session_id, carton.carton.carton_id
    );
    let mismatched = send(
        &app,
        &operator_token,
        access.tenant_id,
        Method::POST,
        &closure_path,
        Some("pack-scale-close-mismatch"),
        Some(json!({
            "carton_barcode": "PACK-SCALE-CARTON",
            "measurements": {"weight_grams": 1249},
            "weight_automation_command_id": queued.command_id,
            "expected_revision": packed.revision
        })),
    )
    .await;
    assert_eq!(mismatched.status(), StatusCode::CONFLICT);
    let close_body = json!({
        "carton_barcode": "PACK-SCALE-CARTON",
        "measurements": {"weight_grams": 1250},
        "weight_automation_command_id": queued.command_id,
        "expected_revision": packed.revision
    });
    let closed: CloseCartonResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::POST,
                &closure_path,
                Some("pack-scale-close"),
                Some(close_body.clone()),
            )
            .await,
            StatusCode::OK,
            "close carton with scale evidence",
        )
        .await,
    )
    .await;
    let exact: CloseCartonResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::POST,
                &closure_path,
                Some("pack-scale-close"),
                Some(close_body),
            )
            .await,
            StatusCode::OK,
            "replay scale carton close",
        )
        .await,
    )
    .await;
    assert_eq!(exact, closed);
    match &closed.lifecycle {
        PackCartonLifecycleResponse::Closed {
            weight_evidence:
                Some(CartonWeightEvidenceResponse::AutomationScale {
                    automation_command_id,
                    device_key,
                    ..
                }),
            ..
        } => {
            assert_eq!(*automation_command_id, queued.command_id);
            assert_eq!(device_key, "packing-scale-01");
        }
        other => panic!("unexpected scale-backed lifecycle: {other:?}"),
    }

    let reopened: ReopenCartonResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/packing-sessions/{}/cartons/{}/reopenings",
                    opened.session.session_id, carton.carton.carton_id
                ),
                Some("pack-scale-reopen"),
                Some(json!({
                    "carton_barcode": "PACK-SCALE-CARTON",
                    "reason": "packing_correction",
                    "expected_revision": closed.revision
                })),
            )
            .await,
            StatusCode::OK,
            "reopen scale-backed carton",
        )
        .await,
    )
    .await;
    assert!(matches!(
        reopened.previous_weight_evidence,
        Some(CartonWeightEvidenceResponse::AutomationScale { .. })
    ));
    let reused = send(
        &app,
        &operator_token,
        access.tenant_id,
        Method::POST,
        &closure_path,
        Some("pack-scale-reuse"),
        Some(json!({
            "carton_barcode": "PACK-SCALE-CARTON",
            "measurements": {"weight_grams": 1250},
            "weight_automation_command_id": queued.command_id,
            "expected_revision": reopened.revision
        })),
    )
    .await;
    assert_eq!(reused.status(), StatusCode::CONFLICT);

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let evidence: (i64, i64, i64) = sqlx::query_as(
        r#"SELECT COUNT(*),COUNT(DISTINCT automation_command_id),
                  COUNT(*) FILTER(WHERE source='automation_scale')
           FROM carton_weight_evidence WHERE tenant_id=$1"#,
    )
    .bind(access.tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let outbox_source: String = sqlx::query_scalar(
        r#"SELECT payload#>>'{carton_lifecycle,weight_evidence,source}'
           FROM outbox_events
           WHERE tenant_id=$1 AND event_type='packing.carton_closed'
           ORDER BY id DESC LIMIT 1"#,
    )
    .bind(access.tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(evidence, (1, 1, 1));
    assert_eq!(outbox_source, "automation_scale");
}
