use wareboxes_api_contract::v1::{
    ConfigurationResponse, DocumentPolicySource, GenerateCartonLabelSetResponse,
    GeneratePackingSlipResponse, RecordManualManifestResponse, ShipmentDocumentListResponse,
};

use super::*;

async fn add_membership(fixture: &Fixture, tenant_id: TenantId, user_id: i64) {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships(tenant_id,user_id) VALUES ($1,$2)")
        .bind(tenant_id.get())
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn grant_admin(fixture: &Fixture, tenant_id: TenantId, user_id: i64, suffix: &str) {
    let permission = match wareboxes_persistence_postgres::permissions::find_by_name(
        &fixture.db,
        tenant_id,
        "admin",
    )
    .await
    .unwrap()
    {
        Some(permission) => permission.id,
        None => wareboxes_persistence_postgres::permissions::add_permission(
            &fixture.db,
            tenant_id,
            "admin",
            Some("Warehouse administrator"),
        )
        .await
        .unwrap(),
    };
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("document-policy-admin-{suffix}"),
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

struct ConfigurationActors {
    creator_token: String,
    approver_token: String,
}

async fn configuration_actors(
    fixture: &Fixture,
    tenant_id: TenantId,
    creator_id: i64,
    creator_token: String,
) -> ConfigurationActors {
    grant_admin(fixture, tenant_id, creator_id, "creator").await;
    let approver = fixture.user("shipping-policy-approver@test.local").await;
    add_membership(fixture, tenant_id, approver.id).await;
    grant_admin(fixture, tenant_id, approver.id, "approver").await;
    ConfigurationActors {
        creator_token,
        approver_token: auth::create_session(&fixture.db, approver.id)
            .await
            .unwrap(),
    }
}

async fn activate_document_policy(
    app: &axum::Router,
    actors: &ConfigurationActors,
    tenant_id: TenantId,
    owner_id: i64,
    facility_id: i64,
    prefix: &str,
    expected_revision: Option<i64>,
    generate_packing_slip: bool,
    generate_carton_label: bool,
    require_tracking_barcode: bool,
) -> ConfigurationResponse {
    let created = send(
        app,
        &actors.creator_token,
        tenant_id,
        Method::POST,
        "/api/v1/configurations",
        Some(&format!("{prefix}-create")),
        Some(json!({
            "scope": {
                "level": "owner_facility",
                "inventory_owner_id": owner_id,
                "facility_id": facility_id
            },
            "effective_from": "2026-01-01T00:00:00Z",
            "rule": {
                "kind": "document",
                "generate_packing_slip": generate_packing_slip,
                "generate_carton_label": generate_carton_label,
                "require_tracking_barcode": require_tracking_barcode
            },
            "expected_revision": expected_revision
        })),
    )
    .await;
    let mut version: ConfigurationResponse =
        response_json(expect_status(created, StatusCode::OK, "create document policy").await).await;
    for (token, transition) in [
        (&actors.creator_token, "submissions"),
        (&actors.approver_token, "approvals"),
        (&actors.creator_token, "activations"),
    ] {
        let response = send(
            app,
            token,
            tenant_id,
            Method::POST,
            &format!(
                "/api/v1/configurations/{}/{transition}",
                version.configuration_id
            ),
            Some(&format!("{prefix}-{transition}")),
            Some(json!({"expected_revision": version.revision.get()})),
        )
        .await;
        version = response_json(
            expect_status(response, StatusCode::OK, "transition document policy").await,
        )
        .await;
    }
    version
}

async fn insert_forged_label_header(
    fixture: &Fixture,
    tenant_id: TenantId,
    source_document_id: i64,
    configuration_id: i64,
    configuration_revision: i64,
    owner_id: i64,
    facility_id: i64,
    definition: Value,
    policy_hash: &str,
) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let result = sqlx::query(
        r#"INSERT INTO shipment_documents (
          tenant_id,inventory_owner_id,facility_id,shipment_id,order_id,document_type,
          carrier_manifest_id,carrier_code,service_code,manifest_reference,file_name,media_type,
          renderer_version,shipment_revision_at_generation,carton_count,line_count,ordered_qty,
          accepted_short_qty,accepted_substitute_qty,packed_qty,policy_source,
          policy_configuration_id,policy_configuration_revision,policy_scope_level,
          policy_inventory_owner_id,policy_facility_id,policy_definition,policy_hash,
          content,content_length,content_sha256,generated_by_user_id,generated_at)
        SELECT tenant_id,inventory_owner_id,facility_id,shipment_id,order_id,'carton_label_set',
          carrier_manifest_id,carrier_code,service_code,manifest_reference,
          file_name||'-forged',media_type,renderer_version,shipment_revision_at_generation,
          carton_count,line_count,ordered_qty,accepted_short_qty,accepted_substitute_qty,packed_qty,
          'configuration',$3,$4,'owner_facility',$5,$6,$7,$8,content,content_length,
          content_sha256,generated_by_user_id,statement_timestamp()
        FROM shipment_documents WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(source_document_id)
    .bind(configuration_id)
    .bind(configuration_revision)
    .bind(owner_id)
    .bind(facility_id)
    .bind(definition)
    .bind(policy_hash)
    .execute(&mut *tx)
    .await;
    tx.rollback().await.unwrap();
    result
}

#[tokio::test]
async fn effective_document_policy_drives_generation_and_freezes_evidence() {
    let fixture = Fixture::new().await;
    let operator = fixture
        .wms_user("shipping-policy-operator@test.local")
        .await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "shipping-policy-orders",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Document Policy Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Document Policy Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let station_id = execution_location(
        &fixture,
        access.tenant_id,
        facility_id,
        "DOCUMENT-POLICY-PACK",
    )
    .await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        station_id,
        "DOCUMENT-POLICY-TOTE",
    )
    .await;
    set_facility_address(
        &fixture,
        access.tenant_id,
        facility_id,
        "document-policy-origin",
        true,
    )
    .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let actors = configuration_actors(&fixture, access.tenant_id, operator.id, token.clone()).await;
    let app = routes::app(AppState::new(fixture.db.clone()));
    let ready = prepare_ready_shipment(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        station_id,
        "DOCUMENT-POLICY",
    )
    .await;
    let created: CreateShipmentResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/orders/{}/shipments", ready.order_id),
                Some("document-policy-shipment"),
                Some(create_shipment_body(&ready)),
            )
            .await,
            StatusCode::OK,
            "create policy shipment",
        )
        .await,
    )
    .await;
    let shipment_id = created.shipment.shipment_id;
    let list_path = format!("/api/v1/shipments/{shipment_id}/documents");
    let initial: ShipmentDocumentListResponse = response_json(
        expect_status(
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
            StatusCode::OK,
            "read product document policy",
        )
        .await,
    )
    .await;
    assert_eq!(initial.policy.source, DocumentPolicySource::ProductDefault);

    let active = activate_document_policy(
        &app,
        &actors,
        access.tenant_id,
        owner_id,
        facility_id,
        "document-policy-first",
        None,
        true,
        false,
        true,
    )
    .await;
    let resolved: ShipmentDocumentListResponse = response_json(
        expect_status(
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
            StatusCode::OK,
            "resolve configured document policy",
        )
        .await,
    )
    .await;
    assert_eq!(resolved.policy.source, DocumentPolicySource::Configuration);
    assert_eq!(
        resolved.policy.configuration_id,
        Some(active.configuration_id)
    );
    assert!(resolved.policy.require_tracking_barcode);
    assert!(!resolved.policy.generate_carton_label);

    let packing_path = format!("/api/v1/shipments/{shipment_id}/documents/packing-slips");
    let stale = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &packing_path,
        Some("document-policy-stale"),
        Some(json!({
            "expected_shipment_revision": 1,
            "expected_policy": initial.policy.expectation()
        })),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let expected_policy = resolved.policy.expectation();
    let before_manifest = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &packing_path,
        Some("document-policy-before-manifest"),
        Some(json!({
            "expected_shipment_revision": 1,
            "expected_policy": expected_policy
        })),
    )
    .await;
    assert_eq!(before_manifest.status(), StatusCode::CONFLICT);

    let manifested: RecordManualManifestResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/shipments/{shipment_id}/manifests"),
                Some("document-policy-manifest"),
                Some(manifest_body(&ready, "DOCUMENT-POLICY-MANIFEST", 1)),
            )
            .await,
            StatusCode::OK,
            "manifest policy shipment",
        )
        .await,
    )
    .await;
    let request_body = json!({
        "expected_shipment_revision": manifested.revision.get(),
        "expected_policy": expected_policy
    });
    let generated: GeneratePackingSlipResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &packing_path,
                Some("document-policy-generate"),
                Some(request_body.clone()),
            )
            .await,
            StatusCode::OK,
            "generate configured packing slip",
        )
        .await,
    )
    .await;
    assert_eq!(generated.document.policy, resolved.policy);
    assert_eq!(
        generated.document.manifest_id,
        Some(manifested.manifest.manifest_id)
    );
    let download = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &format!(
            "/api/v1/shipment-documents/{}/content",
            generated.document.document_id
        ),
        None,
        None,
    )
    .await;
    let html = to_bytes(download.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let html = std::str::from_utf8(&html).unwrap();
    assert_eq!(html.matches("class=\"tracking-barcode\"").count(), 2);
    assert!(html.contains(&format!("TRACK-{}-1", ready.order_id)));

    let disabled_labels = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/shipments/{shipment_id}/documents/carton-label-sets"),
        Some("document-policy-disabled-labels"),
        Some(json!({
            "expected_shipment_revision": manifested.revision.get(),
            "expected_policy": resolved.policy.expectation()
        })),
    )
    .await;
    assert_eq!(disabled_labels.status(), StatusCode::CONFLICT);

    let replacement = activate_document_policy(
        &app,
        &actors,
        access.tenant_id,
        owner_id,
        facility_id,
        "document-policy-second",
        Some(active.revision.get()),
        false,
        true,
        false,
    )
    .await;
    let replay: GeneratePackingSlipResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &packing_path,
                Some("document-policy-generate"),
                Some(request_body),
            )
            .await,
            StatusCode::OK,
            "replay document after policy supersession",
        )
        .await,
    )
    .await;
    assert_eq!(replay, generated);

    let current: ShipmentDocumentListResponse = response_json(
        expect_status(
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
            StatusCode::OK,
            "read replacement document policy",
        )
        .await,
    )
    .await;
    assert_eq!(
        current.policy.configuration_id,
        Some(replacement.configuration_id)
    );
    assert_eq!(current.documents, vec![generated.document.clone()]);
    let replacement_definition = json!({
        "kind": "document",
        "generate_packing_slip": false,
        "generate_carton_label": true,
        "require_tracking_barcode": false
    });
    let forged_hash = insert_forged_label_header(
        &fixture,
        access.tenant_id,
        generated.document.document_id,
        replacement.configuration_id,
        replacement.revision.get(),
        owner_id,
        facility_id,
        replacement_definition.clone(),
        &"0".repeat(64),
    )
    .await
    .unwrap_err();
    assert!(forged_hash
        .to_string()
        .contains("policy hash does not match"));
    let forged_identity = insert_forged_label_header(
        &fixture,
        access.tenant_id,
        generated.document.document_id,
        active.configuration_id,
        active.revision.get(),
        owner_id,
        facility_id,
        replacement_definition,
        &current.policy.policy_hash,
    )
    .await
    .unwrap_err();
    assert!(forged_identity
        .to_string()
        .contains("policy is stale or inapplicable"));
    let labels: GenerateCartonLabelSetResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/shipments/{shipment_id}/documents/carton-label-sets"),
                Some("document-policy-enabled-labels"),
                Some(json!({
                    "expected_shipment_revision": manifested.revision.get(),
                    "expected_policy": current.policy.expectation()
                })),
            )
            .await,
            StatusCode::OK,
            "generate enabled carton labels",
        )
        .await,
    )
    .await;
    assert_eq!(labels.document.policy, current.policy);

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let evidence: (i64, i64, i64) = sqlx::query_as(
        r#"SELECT
          (SELECT COUNT(*) FROM shipment_documents WHERE tenant_id=$1 AND shipment_id=$2),
          (SELECT COUNT(DISTINCT policy_configuration_id) FROM shipment_documents
             WHERE tenant_id=$1 AND shipment_id=$2),
          (SELECT COUNT(*) FROM outbox_events WHERE tenant_id=$1 AND aggregate_id=$3
             AND event_type IN ('shipping.packing_slip_generated','shipping.carton_label_set_generated'))"#,
    )
    .bind(access.tenant_id.get())
    .bind(shipment_id)
    .bind(ready.order_id.to_string())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(evidence, (2, 2, 2));
    tx.rollback().await.unwrap();
}
