use super::super::*;
use wareboxes_api_contract::v1::{
    AutomationCommandDeliveryPage, AutomationCommandStatus, AutomationControlMode,
    AutomationDeviceCommand, AutomationDeviceResponse, AutomationHealthState,
    AutomationPrintFormat, AutomationPrinterCommand, IssuedServiceAccountCredentialResponse,
    PrintShipmentDocumentResponse, ServiceAccountResponse, ShipmentDocumentPrintJobPage,
    ShipmentPrinterDevicePage,
};

async fn grant_permissions(fixture: &Fixture, tenant_id: TenantId, user_id: i64, names: &[&str]) {
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("shipping-printer-manager-{user_id}"),
        Some("Shipping printer acceptance manager"),
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
                Some("Shipping printer acceptance permission"),
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
async fn retained_labels_dispatch_to_scoped_edge_printer_with_replay_and_history() {
    let fixture = Fixture::new().await;
    let operator = fixture
        .wms_user("shipment-printer-operator@test.local")
        .await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "shipment-printer-orders",
    )
    .await;
    let manager = fixture.user("shipment-printer-manager@test.local").await;
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
        .inventory_owner(access.tenant_id, "Shipment Printer Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Shipment Printer Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let station_id =
        execution_location(&fixture, access.tenant_id, facility_id, "SHIP-PRINT-PACK").await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        station_id,
        "SHIP-PRINT-TOTE",
    )
    .await;
    set_facility_address(
        &fixture,
        access.tenant_id,
        facility_id,
        "ship-print-origin",
        true,
    )
    .await;
    let operator_token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let manager_token = auth::create_session(&fixture.db, manager.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let ready = prepare_ready_shipment(
        &fixture,
        &app,
        &operator_token,
        &access,
        owner_id,
        facility_id,
        station_id,
        "SHIP-PRINT",
    )
    .await;
    let created: CreateShipmentResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/orders/{}/shipments", ready.order_id),
                Some("ship-print-create"),
                Some(create_shipment_body(&ready)),
            )
            .await,
            StatusCode::OK,
            "create printer shipment",
        )
        .await,
    )
    .await;
    let shipment_id = created.shipment.shipment_id;
    let manifested: RecordManualManifestResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/shipments/{shipment_id}/manifests"),
                Some("ship-print-manifest"),
                Some(manifest_body(&ready, "SHIP-PRINT-MANIFEST", 1)),
            )
            .await,
            StatusCode::OK,
            "manifest printer shipment",
        )
        .await,
    )
    .await;
    let labels: GenerateCartonLabelSetResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/shipments/{shipment_id}/documents/carton-label-sets"),
                Some("ship-print-labels"),
                Some(json!({"expected_shipment_revision": manifested.revision})),
            )
            .await,
            StatusCode::OK,
            "generate printer labels",
        )
        .await,
    )
    .await;

    let service_account: ServiceAccountResponse = response_json(
        expect_status(
            send(
                &app,
                &manager_token,
                access.tenant_id,
                Method::POST,
                "/api/v1/service-accounts",
                Some("ship-print-edge-account"),
                Some(json!({
                    "name": "Shipping printer edge",
                    "description": "Acceptance printer identity",
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
            "create printer edge account",
        )
        .await,
    )
    .await;
    let edge_token = format!("wbs_sa_{}", "P".repeat(48));
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
                Some("ship-print-edge-credential"),
                Some(json!({
                    "expected_revision": 1,
                    "label": "shipping printer edge credential",
                    "expires_at": null,
                    "bearer_token": edge_token.clone()
                })),
            )
            .await,
            StatusCode::OK,
            "issue printer edge credential",
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
                Some("ship-print-register"),
                Some(json!({
                    "facility_id": facility_id,
                    "device_key": "shipping-printer-01",
                    "class": "printer",
                    "display_name": "Shipping printer 01"
                })),
            )
            .await,
            StatusCode::OK,
            "register shipping printer",
        )
        .await,
    )
    .await;
    for (key, mode) in [
        ("ship-print-heartbeat-disabled", "disabled"),
        ("ship-print-heartbeat-auto", "automatic"),
    ] {
        if mode == "automatic" {
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
                        Some("ship-print-enable"),
                        Some(json!({
                            "expected_revision": device.revision,
                            "target_mode": "automatic",
                            "reason": "printer interlock and media verified",
                            "safety_confirmation": "CONFIRM-SAFE-TO-RESUME"
                        })),
                    )
                    .await,
                    StatusCode::OK,
                    "enable shipping printer",
                )
                .await,
            )
            .await;
            assert_eq!(enabled.control_mode, AutomationControlMode::Automatic);
        }
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
                    Some(key),
                    Some(json!({
                        "agent_instance": "ship-edge/boot-1",
                        "health": "healthy",
                        "control_mode": mode,
                        "message": "printer ready",
                        "queued_commands": 0,
                        "manual_review_commands": 0,
                        "observed_at": chrono::Utc::now().to_rfc3339()
                    })),
                )
                .await,
                StatusCode::OK,
                "record printer heartbeat",
            )
            .await,
        )
        .await;
    }

    let document_id = labels.document.document_id;
    let printers: ShipmentPrinterDevicePage = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::GET,
                &format!("/api/v1/shipment-documents/{document_id}/printers"),
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "list shipment printers",
        )
        .await,
    )
    .await;
    assert_eq!(printers.items.len(), 1);
    assert_eq!(printers.items[0].health, AutomationHealthState::Healthy);

    let forbidden = send(
        &app,
        &operator_token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/automation/devices/{}/commands", device.device_id),
        Some("ship-print-forbidden-generic"),
        Some(json!({
            "correlation_id": "arbitrary-print",
            "recovery_policy": "device_deduplicated_replay",
            "command": {"device_class": "printer", "command": {
                "operation": "print_document", "document_id": "fake", "format": "html",
                "content": "<h1>not retained</h1>", "copies": 1
            }}
        })),
    )
    .await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let print_path = format!("/api/v1/shipment-documents/{document_id}/print-jobs");
    let print_body = json!({
        "device_id": device.device_id,
        "copies": 2,
        "expected_content_sha256": labels.document.content_sha256
    });
    let queued: PrintShipmentDocumentResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::POST,
                &print_path,
                Some("ship-print-dispatch"),
                Some(print_body.clone()),
            )
            .await,
            StatusCode::OK,
            "dispatch shipment labels",
        )
        .await,
    )
    .await;
    assert_eq!(queued.print_job.status, AutomationCommandStatus::Queued);
    assert_eq!(queued.print_job.copies, 2);
    let replay: PrintShipmentDocumentResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::POST,
                &print_path,
                Some("ship-print-dispatch"),
                Some(print_body.clone()),
            )
            .await,
            StatusCode::OK,
            "replay shipment labels",
        )
        .await,
    )
    .await;
    assert_eq!(replay, queued);

    let delivery: AutomationCommandDeliveryPage = response_json(
        expect_status(
            send(
                &app,
                &edge_token,
                access.tenant_id,
                Method::POST,
                "/api/v1/edge/automation/command-pulls",
                Some("ship-print-pull"),
                Some(json!({
                    "facility_id": facility_id,
                    "agent_instance": "ship-edge/boot-1",
                    "limit": 10
                })),
            )
            .await,
            StatusCode::OK,
            "pull shipment print",
        )
        .await,
    )
    .await;
    let delivered = &delivery.items[0];
    match &delivered.command.command {
        AutomationDeviceCommand::Printer(AutomationPrinterCommand::PrintDocument {
            document_id: frozen_document_id,
            format,
            content,
            copies,
        }) => {
            assert_eq!(frozen_document_id, &document_id.to_string());
            assert_eq!(*format, AutomationPrintFormat::Html);
            assert_eq!(*copies, 2);
            assert!(content.contains("<section class=\"label\">"));
        }
        other => panic!("unexpected printer command: {other:?}"),
    }
    let accepted: wareboxes_api_contract::v1::AutomationCommandResponse = response_json(
        expect_status(
            send(
                &app,
                &edge_token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/edge/automation/commands/{}/acknowledgements",
                    queued.print_job.command_id
                ),
                Some("ship-print-ack"),
                Some(json!({
                    "delivery_token": delivered.delivery_token,
                    "expected_revision": delivered.command.revision
                })),
            )
            .await,
            StatusCode::OK,
            "ack shipment print",
        )
        .await,
    )
    .await;
    let _: wareboxes_api_contract::v1::AutomationCommandResponse = response_json(
        expect_status(
            send(
                &app,
                &edge_token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/edge/automation/commands/{}/reports",
                    queued.print_job.command_id
                ),
                Some("ship-print-report"),
                Some(json!({
                    "expected_revision": accepted.revision,
                    "status": "succeeded",
                    "result": {"device_class": "printer", "result": {
                        "spool_job_id": "cups-job-4242"
                    }},
                    "error_code": null,
                    "error_message": null,
                    "occurred_at": chrono::Utc::now().to_rfc3339()
                })),
            )
            .await,
            StatusCode::OK,
            "report shipment print",
        )
        .await,
    )
    .await;
    let completed: wareboxes_api_contract::v1::ShipmentDocumentPrintJobResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::GET,
                &format!(
                    "/api/v1/shipment-documents/{document_id}/print-jobs/{}",
                    queued.print_job.command_id
                ),
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "read completed shipment print",
        )
        .await,
    )
    .await;
    assert_eq!(completed.status, AutomationCommandStatus::Succeeded);
    assert_eq!(completed.spool_job_id.as_deref(), Some("cups-job-4242"));

    let second: PrintShipmentDocumentResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::POST,
                &print_path,
                Some("ship-print-reprint"),
                Some(print_body),
            )
            .await,
            StatusCode::OK,
            "reprint shipment labels",
        )
        .await,
    )
    .await;
    assert_ne!(second.print_job.command_id, completed.command_id);
    let blocked_departure = send(
        &app,
        &operator_token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/shipments/{shipment_id}/departures"),
        Some("ship-print-depart-blocked"),
        Some(json!({
            "scanned_carton_barcodes": ready.carton_barcodes.clone(),
            "expected_shipment_revision": manifested.revision,
            "expected_order_revision": created.shipment.order_revision
        })),
    )
    .await;
    assert_eq!(blocked_departure.status(), StatusCode::CONFLICT);
    let cancellation_path = format!(
        "/api/v1/shipment-documents/{document_id}/print-jobs/{}/cancellations",
        second.print_job.command_id
    );
    let cancelled: PrintShipmentDocumentResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::POST,
                &cancellation_path,
                Some("ship-print-cancel-reprint"),
                Some(json!({"expected_revision": second.print_job.revision})),
            )
            .await,
            StatusCode::OK,
            "cancel queued reprint",
        )
        .await,
    )
    .await;
    assert_eq!(
        cancelled.print_job.status,
        AutomationCommandStatus::Cancelled
    );
    let cancellation_replay: PrintShipmentDocumentResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::POST,
                &cancellation_path,
                Some("ship-print-cancel-reprint"),
                Some(json!({"expected_revision": second.print_job.revision})),
            )
            .await,
            StatusCode::OK,
            "replay queued reprint cancellation",
        )
        .await,
    )
    .await;
    assert_eq!(cancellation_replay, cancelled);
    let first_page: ShipmentDocumentPrintJobPage = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::GET,
                &format!("{print_path}?limit=1"),
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "read print history",
        )
        .await,
    )
    .await;
    assert_eq!(first_page.items.len(), 1);
    let next = first_page.next_cursor.expect("second print history page");
    let second_page: ShipmentDocumentPrintJobPage = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::GET,
                &format!("{print_path}?limit=1&cursor={next}"),
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "read older print history",
        )
        .await,
    )
    .await;
    assert_eq!(second_page.items, vec![completed.clone()]);

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let evidence: (i64, i64) = sqlx::query_as(
        r#"SELECT
          (SELECT COUNT(*) FROM automation_commands
           WHERE tenant_id=$1 AND shipping_document_id=$2),
          (SELECT COUNT(*) FROM outbox_events
           WHERE tenant_id=$1 AND event_type='automation.command.enqueued'
             AND payload->'shipping_document_print_context'->>'document_id'=$2::text)"#,
    )
    .bind(access.tenant_id.get())
    .bind(document_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(evidence, (2, 2));

    let departed = send(
        &app,
        &operator_token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/shipments/{shipment_id}/departures"),
        Some("ship-print-depart"),
        Some(json!({
            "scanned_carton_barcodes": ready.carton_barcodes.clone(),
            "expected_shipment_revision": manifested.revision,
            "expected_order_revision": created.shipment.order_revision
        })),
    )
    .await;
    assert_eq!(departed.status(), StatusCode::OK);
    let post_departure_print = send(
        &app,
        &operator_token,
        access.tenant_id,
        Method::POST,
        &print_path,
        Some("ship-print-after-departure"),
        Some(json!({
            "device_id": device.device_id,
            "copies": 1,
            "expected_content_sha256": labels.document.content_sha256
        })),
    )
    .await;
    assert_eq!(post_departure_print.status(), StatusCode::CONFLICT);

    set_scope(
        &fixture.db,
        access.tenant_id,
        operator.id,
        Vec::new(),
        Vec::new(),
    )
    .await;
    for hidden in [
        send(
            &app,
            &operator_token,
            access.tenant_id,
            Method::GET,
            &format!("/api/v1/shipment-documents/{document_id}/printers"),
            None,
            None,
        )
        .await,
        send(
            &app,
            &operator_token,
            access.tenant_id,
            Method::GET,
            &print_path,
            None,
            None,
        )
        .await,
    ] {
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    }
}
