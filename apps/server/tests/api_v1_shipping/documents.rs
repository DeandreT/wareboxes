use wareboxes_api_contract::v1::{
    CreateShipmentResponse, ErrorReason, ErrorResponse, GeneratePackingSlipResponse,
    ShipmentDocumentListResponse, ShipmentDocumentType,
};

use super::*;

#[tokio::test]
async fn packing_slip_is_replay_safe_immutable_scoped_and_downloadable() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("shipment-documents@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "shipment-documents-orders",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Shipment Documents Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Shipment Documents Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let station_id =
        execution_location(&fixture, access.tenant_id, facility_id, "SHIP-DOC-PACK").await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        station_id,
        "SHIP-DOC-TOTE",
    )
    .await;
    set_facility_address(
        &fixture,
        access.tenant_id,
        facility_id,
        "ship-doc-origin",
        true,
    )
    .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let ready = prepare_ready_shipment(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        station_id,
        "SHIP-DOC",
    )
    .await;
    let created = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{}/shipments", ready.order_id),
        Some("ship-doc-create"),
        Some(create_shipment_body(&ready)),
    )
    .await;
    let created: CreateShipmentResponse =
        response_json(expect_status(created, StatusCode::OK, "create document shipment").await)
            .await;
    let shipment_id = created.shipment.shipment_id;
    let generate_path = format!("/api/v1/shipments/{shipment_id}/documents/packing-slips");

    let stale = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &generate_path,
        Some("ship-doc-stale"),
        Some(json!({"expected_shipment_revision": 2})),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let request_body = json!({"expected_shipment_revision": 1});
    let generated = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &generate_path,
        Some("ship-doc-generate"),
        Some(request_body.clone()),
    )
    .await;
    let generated: GeneratePackingSlipResponse =
        response_json(expect_status(generated, StatusCode::OK, "generate packing slip").await)
            .await;
    assert_eq!(
        generated.document.document_type,
        ShipmentDocumentType::PackingSlip
    );
    assert_eq!(generated.document.shipment_id, shipment_id);
    assert_eq!(generated.document.order_id, ready.order_id);
    assert_eq!(generated.document.shipment_revision_at_generation.get(), 1);
    assert_eq!(generated.document.carton_count, 2);
    assert_eq!(generated.document.line_count, 2);
    assert_eq!(generated.document.demand.ordered_quantity, 5);
    assert_eq!(generated.document.demand.shipped_quantity, 5);
    assert_eq!(generated.document.demand.accepted_short_quantity, 0);
    assert_eq!(generated.document.content_sha256.len(), 64);

    let replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &generate_path,
        Some("ship-doc-generate"),
        Some(request_body.clone()),
    )
    .await;
    assert_eq!(
        response_json::<GeneratePackingSlipResponse>(
            expect_status(replay, StatusCode::OK, "replay packing slip").await
        )
        .await,
        generated
    );
    let reused = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &generate_path,
        Some("ship-doc-generate"),
        Some(json!({"expected_shipment_revision": 2})),
    )
    .await;
    assert_eq!(reused.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(reused).await.reason,
        ErrorReason::IdempotencyKeyReused
    );
    let duplicate = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &generate_path,
        Some("ship-doc-duplicate"),
        Some(request_body.clone()),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);

    let list_path = format!("/api/v1/shipments/{shipment_id}/documents");
    let listed = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &list_path,
        None,
        None,
    )
    .await;
    let listed: ShipmentDocumentListResponse =
        response_json(expect_status(listed, StatusCode::OK, "list shipment documents").await).await;
    assert_eq!(listed.documents, vec![generated.document.clone()]);

    let download_path = format!(
        "/api/v1/shipment-documents/{}/content",
        generated.document.document_id
    );
    let downloaded = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &download_path,
        None,
        None,
    )
    .await;
    assert_eq!(downloaded.status(), StatusCode::OK);
    assert_eq!(
        downloaded.headers()[header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );
    assert_eq!(
        downloaded.headers()[header::CONTENT_DISPOSITION],
        format!("attachment; filename=\"{}\"", generated.document.file_name)
    );
    let original_content = to_bytes(downloaded.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        i64::try_from(original_content.len()).unwrap(),
        generated.document.content_length
    );
    let original_html = std::str::from_utf8(&original_content).unwrap();
    assert!(original_html.contains("Packing slip"));
    assert!(original_html.contains("SHIP-DOC"));
    assert!(original_html.contains("SHIP-DOC-CARTON-1"));
    assert!(original_html.contains("SHIP-DOC item 0"));

    let admin = admin_db_for(&fixture.db).await;
    sqlx::query(
        r#"UPDATE items SET description = 'Changed after generation'
           WHERE tenant_id = $1 AND id IN (
               SELECT item_id FROM order_items WHERE tenant_id = $1 AND order_id = $2)"#,
    )
    .bind(access.tenant_id.get())
    .bind(ready.order_id)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;
    let downloaded_again = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &download_path,
        None,
        None,
    )
    .await;
    let retained_content = to_bytes(
        expect_status(downloaded_again, StatusCode::OK, "redownload packing slip")
            .await
            .into_body(),
        2 * 1024 * 1024,
    )
    .await
    .unwrap();
    assert_eq!(retained_content, original_content);

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let counts: (i64, i64, i64, i64) = sqlx::query_as(
        r#"SELECT
            (SELECT COUNT(*) FROM shipment_documents WHERE tenant_id = $1 AND shipment_id = $2),
            (SELECT COUNT(*) FROM shipment_document_lines WHERE tenant_id = $1 AND shipment_id = $2),
            (SELECT COUNT(*) FROM command_idempotency_records
              WHERE tenant_id = $1 AND operation = 'shipping.document.packing_slip.generate.v1'),
            (SELECT COUNT(*) FROM outbox_events
              WHERE tenant_id = $1 AND event_type = 'shipping.packing_slip_generated'
                AND aggregate_id = $3)"#,
    )
    .bind(access.tenant_id.get())
    .bind(shipment_id)
    .bind(ready.order_id.to_string())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(counts, (1, 2, 1, 1));
    assert!(sqlx::query(
        "UPDATE shipment_documents SET file_name = file_name WHERE tenant_id = $1"
    )
    .bind(access.tenant_id.get())
    .execute(&mut *tx)
    .await
    .is_err());
    tx.rollback().await.unwrap();

    let app_role = privileged_session_as_app(&fixture.db).await;
    for (table, can_insert) in [
        ("shipment_documents", true),
        ("shipment_document_lines", true),
    ] {
        let privileges: (bool, bool, bool, bool) = sqlx::query_as(
            "SELECT has_table_privilege('wareboxes_app', $1, 'SELECT'), has_table_privilege('wareboxes_app', $1, 'INSERT'), has_table_privilege('wareboxes_app', $1, 'UPDATE'), has_table_privilege('wareboxes_app', $1, 'DELETE')",
        )
        .bind(table)
        .fetch_one(&app_role)
        .await
        .unwrap();
        assert_eq!(privileges, (true, can_insert, false, false));
    }
    app_role.close().await;

    set_scope(
        &fixture.db,
        access.tenant_id,
        operator.id,
        Vec::new(),
        Vec::new(),
    )
    .await;
    for response in [
        send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &generate_path,
            Some("ship-doc-generate"),
            Some(request_body),
        )
        .await,
        send(
            &app,
            &token,
            access.tenant_id,
            Method::GET,
            &list_path,
            None,
            None,
        )
        .await,
        send(
            &app,
            &token,
            access.tenant_id,
            Method::GET,
            &download_path,
            None,
            None,
        )
        .await,
    ] {
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn carton_labels_require_manifest_and_retain_exact_tracking_barcodes() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("carton-labels@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "carton-label-orders",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Carton Label Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Carton Label Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let station_id =
        execution_location(&fixture, access.tenant_id, facility_id, "LABEL-PACK").await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        station_id,
        "LABEL-DOC-TOTE",
    )
    .await;
    set_facility_address(
        &fixture,
        access.tenant_id,
        facility_id,
        "label-origin",
        true,
    )
    .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let ready = prepare_ready_shipment(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        station_id,
        "LABEL-DOC",
    )
    .await;
    let created = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{}/shipments", ready.order_id),
        Some("label-create"),
        Some(create_shipment_body(&ready)),
    )
    .await;
    let created: CreateShipmentResponse =
        response_json(expect_status(created, StatusCode::OK, "create label shipment").await).await;
    let shipment_id = created.shipment.shipment_id;
    let generation_path = format!("/api/v1/shipments/{shipment_id}/documents/carton-label-sets");

    let before_manifest = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &generation_path,
        Some("label-before-manifest"),
        Some(json!({"expected_shipment_revision": 1})),
    )
    .await;
    assert_eq!(before_manifest.status(), StatusCode::CONFLICT);

    let manifested = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/shipments/{shipment_id}/manifests"),
        Some("label-manifest"),
        Some(manifest_body(&ready, "LABEL-MANIFEST", 1)),
    )
    .await;
    let manifested: RecordManualManifestResponse =
        response_json(expect_status(manifested, StatusCode::OK, "manifest label shipment").await)
            .await;
    assert_eq!(manifested.revision.get(), 2);

    let stale = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &generation_path,
        Some("label-stale"),
        Some(json!({"expected_shipment_revision": 1})),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let request_body = json!({"expected_shipment_revision": 2});
    let generated = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &generation_path,
        Some("label-generate"),
        Some(request_body.clone()),
    )
    .await;
    let generated: GenerateCartonLabelSetResponse =
        response_json(expect_status(generated, StatusCode::OK, "generate carton labels").await)
            .await;
    assert_eq!(
        generated.document.document_type,
        ShipmentDocumentType::CartonLabelSet
    );
    assert_eq!(
        generated.document.manifest_id,
        Some(manifested.manifest.manifest_id)
    );
    assert_eq!(generated.document.carrier_code.as_deref(), Some("UPS"));
    assert_eq!(generated.document.service_code.as_deref(), Some("GROUND"));
    assert_eq!(
        generated.document.manifest_reference.as_deref(),
        Some("LABEL-MANIFEST")
    );
    assert_eq!(generated.document.carton_count, 2);

    let replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &generation_path,
        Some("label-generate"),
        Some(request_body.clone()),
    )
    .await;
    assert_eq!(
        response_json::<GenerateCartonLabelSetResponse>(
            expect_status(replay, StatusCode::OK, "replay carton labels").await
        )
        .await,
        generated
    );
    let duplicate = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &generation_path,
        Some("label-duplicate"),
        Some(request_body.clone()),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);

    let download_path = format!(
        "/api/v1/shipment-documents/{}/content",
        generated.document.document_id
    );
    let downloaded = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &download_path,
        None,
        None,
    )
    .await;
    assert_eq!(downloaded.status(), StatusCode::OK);
    assert_eq!(
        downloaded.headers()[header::CONTENT_DISPOSITION],
        format!("attachment; filename=\"{}\"", generated.document.file_name)
    );
    let content = to_bytes(downloaded.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    let html = std::str::from_utf8(&content).unwrap();
    assert!(html.contains("@page{size:4in 6in"));
    assert_eq!(html.matches("<section class=\"label\">").count(), 2);
    assert!(html.matches("<svg").count() >= 4);
    assert!(html.contains(&format!("TRACK-{}-1", ready.order_id)));
    assert!(html.contains(&format!("TRACK-{}-2", ready.order_id)));
    assert!(html.contains(&ready.carton_barcodes[0]));
    assert!(html.contains(&ready.carton_barcodes[1]));

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let evidence: (i64, i64, i64, i64) = sqlx::query_as(
        r#"SELECT
            (SELECT COUNT(*) FROM shipment_documents
             WHERE tenant_id = $1 AND shipment_id = $2 AND document_type = 'carton_label_set'),
            (SELECT COUNT(*) FROM shipment_document_cartons
             WHERE tenant_id = $1 AND shipment_id = $2 AND tracking_number IS NOT NULL),
            (SELECT COUNT(*) FROM command_idempotency_records
             WHERE tenant_id = $1 AND operation = 'shipping.document.carton_label_set.generate.v1'),
            (SELECT COUNT(*) FROM outbox_events
             WHERE tenant_id = $1 AND event_type = 'shipping.carton_label_set_generated'
               AND aggregate_id = $3)"#,
    )
    .bind(access.tenant_id.get())
    .bind(shipment_id)
    .bind(ready.order_id.to_string())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(evidence, (1, 2, 1, 1));
    assert!(sqlx::query(
        "UPDATE shipment_document_cartons SET tracking_number = tracking_number WHERE tenant_id = $1"
    )
    .bind(access.tenant_id.get())
    .execute(&mut *tx)
    .await
    .is_err());
    tx.rollback().await.unwrap();

    set_scope(
        &fixture.db,
        access.tenant_id,
        operator.id,
        Vec::new(),
        Vec::new(),
    )
    .await;
    for response in [
        send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &generation_path,
            Some("label-generate"),
            Some(request_body),
        )
        .await,
        send(
            &app,
            &token,
            access.tenant_id,
            Method::GET,
            &download_path,
            None,
            None,
        )
        .await,
    ] {
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
