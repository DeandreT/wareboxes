mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde::Serialize;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    CorrectIntegrationOrderRequest, CreateFulfillmentOrderLineRequest,
    CreateFulfillmentOrderRequest, ErrorReason, ErrorResponse, FulfillmentOrderDestination,
    InboundIntegrationDetailResponse, IntegrationOrderEnvelopeLineRequest,
    IntegrationOrderEnvelopeRequest, IntegrationOrderIntakeResponse,
    IntegrationOrderProcessingStatus, ReprocessIntegrationOrderRequest, Revision,
};
use wareboxes_core::dto::UpdateUserAccessScope;

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("wareboxes_api=debug")
        .with_test_writer()
        .try_init();
}

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
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = key {
        builder = builder.header(IDEMPOTENCY_KEY_HEADER, key);
    }
    builder
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

async fn response<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "could not decode {status} as {}: {error}; body={}",
            std::any::type_name::<T>(),
            String::from_utf8_lossy(&bytes)
        )
    })
}

async fn success<T: serde::de::DeserializeOwned>(
    response: axum::response::Response,
    expected_status: StatusCode,
) -> T {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    assert_eq!(
        status,
        expected_status,
        "unexpected response: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

async fn grant(fixture: &Fixture, tenant_id: TenantId, user_id: i64, permission_name: &str) {
    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        &fixture.db,
        tenant_id,
        permission_name,
        Some(permission_name),
    )
    .await
    .unwrap();
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("integration-order-{permission_name}-{user_id}"),
        Some("Integration order intake test role"),
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

async fn link_item(fixture: &Fixture, tenant_id: TenantId, owner_id: i64, item_id: i64) {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        r#"
        INSERT INTO inventory_owner_items
            (tenant_id,created,inventory_owner_id,item_id)
        VALUES ($1,clock_timestamp(),$2,$3)
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id)
    .bind(item_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

fn internal_order(owner_id: i64, item_id: i64, key: &str) -> CreateFulfillmentOrderRequest {
    CreateFulfillmentOrderRequest {
        inventory_owner_id: owner_id,
        order_key: key.into(),
        rush: false,
        ship_by: None,
        destination: FulfillmentOrderDestination {
            recipient_name: "Inbound Orders".into(),
            company: Some("Northstar Retail".into()),
            phone: None,
            email: None,
            line1: "125 Shipping Lane".into(),
            line2: None,
            city: "Reno".into(),
            region: "NV".into(),
            postal_code: "89502".into(),
            country: "US".into(),
        },
        lines: vec![CreateFulfillmentOrderLineRequest {
            line_key: "1".into(),
            item_id,
            quantity: 4,
            requested_uom: "case".into(),
        }],
    }
}

fn external_order(key: &str) -> IntegrationOrderEnvelopeRequest {
    IntegrationOrderEnvelopeRequest {
        order_key: key.into(),
        rush: false,
        ship_by: None,
        destination: FulfillmentOrderDestination {
            recipient_name: "Inbound Orders".into(),
            company: Some("Northstar Retail".into()),
            phone: None,
            email: None,
            line1: "125 Shipping Lane".into(),
            line2: None,
            city: "Reno".into(),
            region: "NV".into(),
            postal_code: "89502".into(),
            country: "US".into(),
        },
        lines: vec![IntegrationOrderEnvelopeLineRequest {
            line_key: "1".into(),
            external_item_key: "CLIENT-CASE".into(),
            external_uom: "CS".into(),
            quantity: 4,
        }],
    }
}

async fn configure_mapping(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    owner_id: i64,
    item_id: i64,
    source_key: &str,
) {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        r#"
        INSERT INTO integration_order_item_mappings
            (tenant_id,inventory_owner_id,source_key,external_item_key,external_uom,
             item_id,requested_uom,revision,effective_from,configured_by_user_id,configured_at)
        SELECT $1,$2,$3,'CLIENT-CASE','CS',$4,'case',1,
               clock.moment,$5,clock.moment
        FROM (SELECT clock_timestamp() AS moment) clock
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id)
    .bind(source_key)
    .bind(item_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn retained_order_envelope_quarantines_recovers_and_replays_exactly() {
    init_tracing();
    let fixture = Fixture::new().await;
    let user = fixture.user("integration-order-intake@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant(&fixture, tenant_id, user.id, "orders").await;
    grant(&fixture, tenant_id, user.id, "admin").await;
    let owner_id = fixture
        .inventory_owner(tenant_id, "Integration Order Client")
        .await;
    let item_id = fixture.item(tenant_id, "Integration Case", "case").await;
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let intake_uri =
        format!("/api/v1/integrations/order-intake/partner-api/inventory-owners/{owner_id}/orders");
    let payload = external_order("INTAKE-100");

    let quarantined_response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            &intake_uri,
            Some("partner-message-100"),
            &payload,
        ))
        .await
        .unwrap();
    let quarantined: IntegrationOrderIntakeResponse =
        success(quarantined_response, StatusCode::ACCEPTED).await;
    assert_eq!(
        quarantined.status,
        IntegrationOrderProcessingStatus::Quarantined
    );
    assert_eq!(quarantined.revision.get(), 1);
    assert_eq!(quarantined.attempt_count, 1);
    assert_eq!(
        quarantined.error_code.as_deref(),
        Some("item_mapping_not_found")
    );
    assert_eq!(quarantined.applied_mapping_count, 0);
    assert!(quarantined.order_id.is_none());

    let exact_replay = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            &intake_uri,
            Some("partner-message-100"),
            &payload,
        ))
        .await
        .unwrap();
    assert_eq!(exact_replay.status(), StatusCode::ACCEPTED);
    assert_eq!(
        response::<IntegrationOrderIntakeResponse>(exact_replay).await,
        quarantined
    );

    link_item(&fixture, tenant_id, owner_id, item_id).await;
    configure_mapping(
        &fixture,
        tenant_id,
        user.id,
        owner_id,
        item_id,
        "partner-api",
    )
    .await;
    let reprocess_uri = format!(
        "/api/v1/integration-monitor/inbound/{}/reprocessings",
        quarantined.receipt_id
    );
    let reprocess_body = ReprocessIntegrationOrderRequest {
        expected_revision: Revision::new(1).unwrap(),
    };
    let processed_response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            &reprocess_uri,
            Some("reprocess-message-100"),
            &reprocess_body,
        ))
        .await
        .unwrap();
    assert_eq!(processed_response.status(), StatusCode::OK);
    let processed: IntegrationOrderIntakeResponse = response(processed_response).await;
    assert_eq!(
        processed.status,
        IntegrationOrderProcessingStatus::Processed
    );
    assert_eq!(processed.revision.get(), 2);
    assert_eq!(processed.attempt_count, 2);
    assert_eq!(processed.applied_mapping_count, 1);
    assert!(processed.order_id.is_some());
    assert_eq!(processed.order_revision.unwrap().get(), 1);
    assert!(processed.error_code.is_none());

    let reprocess_replay = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            &reprocess_uri,
            Some("reprocess-message-100"),
            &reprocess_body,
        ))
        .await
        .unwrap();
    assert_eq!(reprocess_replay.status(), StatusCode::OK);
    assert_eq!(
        response::<IntegrationOrderIntakeResponse>(reprocess_replay).await,
        processed
    );

    let detail_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/integration-monitor/inbound/{}",
                    processed.receipt_id
                ))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, tenant_id.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail_response.status(), StatusCode::OK);
    let detail: InboundIntegrationDetailResponse = response(detail_response).await;
    let processing = detail.processing.unwrap();
    assert_eq!(
        processing.status,
        IntegrationOrderProcessingStatus::Processed
    );
    assert_eq!(processing.attempts.len(), 2);
    assert_eq!(
        processing.attempts[0].status,
        IntegrationOrderProcessingStatus::Processed
    );
    assert_eq!(processing.attempts[0].applied_mappings.len(), 1);
    let applied_mapping = &processing.attempts[0].applied_mappings[0];
    assert_eq!(applied_mapping.line_key, "1");
    assert_eq!(applied_mapping.external_item_key, "CLIENT-CASE");
    assert_eq!(applied_mapping.external_uom, "CS");
    assert_eq!(applied_mapping.item_id, item_id);
    assert_eq!(applied_mapping.requested_uom, "case");
    assert_eq!(applied_mapping.mapping_revision.get(), 1);
    assert_eq!(
        processing.attempts[1].status,
        IntegrationOrderProcessingStatus::Quarantined
    );
    assert!(processing.attempts[1].applied_mappings.is_empty());

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let counts: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM integration_inbox_receipts),
               (SELECT COUNT(*) FROM integration_inbox_processings),
               (SELECT COUNT(*) FROM integration_inbox_processing_attempts),
               (SELECT COUNT(*) FROM integration_inbox_processing_attempt_mappings),
               (SELECT COUNT(*) FROM orders WHERE order_key='INTAKE-100')
        "#,
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(counts, (1, 1, 2, 1, 1));

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let evidence: (String, String, i64, i64, String) = sqlx::query_as(
        r#"
        SELECT external_item_key,external_uom,mapping_revision,item_id,requested_uom
        FROM integration_inbox_processing_attempt_mappings
        WHERE processing_attempt_id=$1
        "#,
    )
    .bind(processed.processing_attempt_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        evidence,
        ("CLIENT-CASE".into(), "CS".into(), 1, item_id, "case".into())
    );
}

#[tokio::test]
async fn intake_rejects_key_reuse_and_conceals_scoped_receipts() {
    init_tracing();
    let fixture = Fixture::new().await;
    let user = fixture.user("integration-order-scope@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant(&fixture, tenant_id, user.id, "orders").await;
    let owner_id = fixture
        .inventory_owner(tenant_id, "Scoped Integration Client")
        .await;
    let other_owner_id = fixture
        .inventory_owner(tenant_id, "Other Integration Client")
        .await;
    let item_id = fixture
        .item(tenant_id, "Scoped Integration Item", "case")
        .await;
    link_item(&fixture, tenant_id, owner_id, item_id).await;
    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: user.id,
            all_facilities: true,
            facility_ids: Vec::new(),
            all_inventory_owners: false,
            inventory_owner_ids: vec![owner_id],
        },
    )
    .await
    .unwrap());
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let uri =
        format!("/api/v1/integrations/order-intake/source-a/inventory-owners/{owner_id}/orders");
    configure_mapping(&fixture, tenant_id, user.id, owner_id, item_id, "source-a").await;
    let payload = external_order("INTAKE-SCOPE");
    let first = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            &uri,
            Some("scope-message"),
            &payload,
        ))
        .await
        .unwrap();
    let first: IntegrationOrderIntakeResponse = success(first, StatusCode::ACCEPTED).await;
    assert_eq!(first.applied_mapping_count, 1);

    let changed = external_order("INTAKE-CHANGED");
    let conflict = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            &uri,
            Some("scope-message"),
            &changed,
        ))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        response::<ErrorResponse>(conflict).await.reason,
        ErrorReason::Conflict
    );

    let denied_uri = format!(
        "/api/v1/integrations/order-intake/source-b/inventory-owners/{other_owner_id}/orders"
    );
    let denied = app
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            &denied_uri,
            Some("denied-message"),
            &external_order("DENIED"),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let attempts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM integration_inbox_processing_attempts WHERE receipt_id=$1",
    )
    .bind(first.receipt_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(attempts, 1);
}

#[tokio::test]
async fn processing_ledgers_are_forced_rls_minimally_granted_and_immutable() {
    let fixture = Fixture::new().await;
    let user = fixture.user("integration-order-ledger@test.local").await;
    let other_user = fixture.user("integration-order-other@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let other_tenant_id = tenant_for_user(&fixture.db, other_user.id).await;
    grant(&fixture, tenant_id, user.id, "orders").await;
    let owner_id = fixture
        .inventory_owner(tenant_id, "Integration Ledger Client")
        .await;
    let item_id = fixture
        .item(tenant_id, "Integration Ledger Item", "case")
        .await;
    link_item(&fixture, tenant_id, owner_id, item_id).await;
    configure_mapping(&fixture, tenant_id, user.id, owner_id, item_id, "ledger").await;
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let uri =
        format!("/api/v1/integrations/order-intake/ledger/inventory-owners/{owner_id}/orders");
    let quarantined: IntegrationOrderIntakeResponse = success(
        app.oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            &uri,
            Some("ledger-message"),
            &serde_json::json!({"not": "an order"}),
        ))
        .await
        .unwrap(),
        StatusCode::ACCEPTED,
    )
    .await;
    assert_eq!(
        quarantined.status,
        IntegrationOrderProcessingStatus::Quarantined
    );
    let mapped_uri =
        format!("/api/v1/integrations/order-intake/ledger/inventory-owners/{owner_id}/orders");
    let mapped: IntegrationOrderIntakeResponse = success(
        routes::app(AppState::new(fixture.db.clone()))
            .oneshot(request(
                &token,
                tenant_id,
                Method::POST,
                &mapped_uri,
                Some("ledger-mapped-message"),
                &external_order("INTAKE-LEDGER"),
            ))
            .await
            .unwrap(),
        StatusCode::ACCEPTED,
    )
    .await;
    assert_eq!(mapped.applied_mapping_count, 1);

    let admin = admin_db_for(&fixture.db).await;
    for (table, policy, can_update) in [
        (
            "integration_inbox_processings",
            "integration_inbox_processings_tenant_isolation",
            true,
        ),
        (
            "integration_inbox_processing_attempts",
            "integration_inbox_processing_attempts_tenant_isolation",
            false,
        ),
        (
            "integration_inbox_processing_attempt_mappings",
            "integration_inbox_processing_attempt_mappings_tenant_isolation",
            false,
        ),
        (
            "integration_inbox_processing_corrections",
            "integration_inbox_processing_corrections_tenant_isolation",
            false,
        ),
    ] {
        let rls: (bool, bool) = sqlx::query_as(
            "SELECT relrowsecurity,relforcerowsecurity FROM pg_class WHERE oid=$1::regclass",
        )
        .bind(table)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(rls, (true, true), "RLS is not forced for {table}");
        let policy_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_policies WHERE tablename=$1 AND policyname=$2)",
        )
        .bind(table)
        .bind(policy)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(policy_exists, "missing tenant policy for {table}");
        let privileges: (bool, bool, bool, bool, bool, bool, bool) = sqlx::query_as(
            r#"
            SELECT has_table_privilege('wareboxes_app',$1,'SELECT'),
                   has_table_privilege('wareboxes_app',$1,'INSERT'),
                   has_table_privilege('wareboxes_app',$1,'UPDATE'),
                   has_table_privilege('wareboxes_app',$1,'DELETE'),
                   has_table_privilege('wareboxes_app',$1,'TRUNCATE'),
                   has_table_privilege('wareboxes_app',$1,'REFERENCES'),
                   has_table_privilege('wareboxes_app',$1,'TRIGGER')
            "#,
        )
        .bind(table)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(
            privileges,
            (true, true, can_update, false, false, false, false),
            "unexpected runtime grants for {table}"
        );
    }
    for sequence in [
        "integration_inbox_processings_id_seq",
        "integration_inbox_processing_attempts_id_seq",
        "integration_inbox_processing_attempt_mappings_id_seq",
        "integration_inbox_processing_corrections_id_seq",
    ] {
        let privileges: (bool, bool, bool) = sqlx::query_as(
            r#"
            SELECT has_sequence_privilege('wareboxes_app',$1,'USAGE'),
                   has_sequence_privilege('wareboxes_app',$1,'SELECT'),
                   has_sequence_privilege('wareboxes_app',$1,'UPDATE')
            "#,
        )
        .bind(sequence)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(privileges, (true, false, false));
    }

    let unbound: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM integration_inbox_processings),(SELECT COUNT(*) FROM integration_inbox_processing_attempts),(SELECT COUNT(*) FROM integration_inbox_processing_attempt_mappings)",
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(unbound, (0, 0, 0));
    let mut other_tx = tenant_tx(&fixture.db, other_tenant_id).await;
    let guessed: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM integration_inbox_processings WHERE id=$1),(SELECT COUNT(*) FROM integration_inbox_processing_attempts WHERE id=$2),(SELECT COUNT(*) FROM integration_inbox_processing_attempt_mappings WHERE processing_attempt_id=$3)",
    )
    .bind(quarantined.processing_id)
    .bind(quarantined.processing_attempt_id)
    .bind(mapped.processing_attempt_id)
    .fetch_one(&mut *other_tx)
    .await
    .unwrap();
    other_tx.rollback().await.unwrap();
    assert_eq!(guessed, (0, 0, 0));

    let processing_tamper =
        sqlx::query("UPDATE integration_inbox_processings SET adapter_key='tampered' WHERE id=$1")
            .bind(quarantined.processing_id)
            .execute(&admin)
            .await;
    assert!(processing_tamper.is_err());
    let attempt_tamper = sqlx::query(
        "UPDATE integration_inbox_processing_attempts SET attempted_at=clock_timestamp() WHERE id=$1",
    )
    .bind(quarantined.processing_attempt_id)
    .execute(&admin)
    .await;
    assert!(attempt_tamper.is_err());
    let mapping_tamper = sqlx::query(
        "UPDATE integration_inbox_processing_attempt_mappings SET external_item_key='tampered' WHERE processing_attempt_id=$1",
    )
    .bind(mapped.processing_attempt_id)
    .execute(&admin)
    .await;
    assert!(mapping_tamper.is_err());
}

#[tokio::test]
async fn corrected_envelope_is_immutable_replayable_and_drives_later_reprocessing() {
    let fixture = Fixture::new().await;
    let user = fixture
        .user("integration-order-correction@test.local")
        .await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant(&fixture, tenant_id, user.id, "orders").await;
    grant(&fixture, tenant_id, user.id, "admin").await;
    let owner_id = fixture
        .inventory_owner(tenant_id, "Integration Correction Client")
        .await;
    let item_id = fixture
        .item(tenant_id, "Integration Correction Item", "case")
        .await;
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let intake_uri =
        format!("/api/v1/integrations/order-intake/correction/inventory-owners/{owner_id}/orders");
    let initial: IntegrationOrderIntakeResponse = success(
        app.clone()
            .oneshot(request(
                &token,
                tenant_id,
                Method::POST,
                &intake_uri,
                Some("correction-intake"),
                &serde_json::json!({"malformed": true}),
            ))
            .await
            .unwrap(),
        StatusCode::ACCEPTED,
    )
    .await;
    assert_eq!(initial.error_code.as_deref(), Some("invalid_payload"));
    assert!(initial.correction_id.is_none());

    let correction_uri = format!(
        "/api/v1/integration-monitor/inbound/{}/corrections",
        initial.receipt_id
    );
    let correction_body = CorrectIntegrationOrderRequest {
        expected_revision: Revision::new(1).unwrap(),
        reason: "Corrected malformed partner envelope".into(),
        order: internal_order(owner_id, item_id, "INTAKE-CORRECTED"),
    };
    let invalid_reason = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            &correction_uri,
            Some("invalid-correction"),
            &CorrectIntegrationOrderRequest {
                reason: " ".into(),
                ..correction_body.clone()
            },
        ))
        .await
        .unwrap();
    assert_eq!(invalid_reason.status(), StatusCode::BAD_REQUEST);
    let correction_response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            &correction_uri,
            Some("correction-command"),
            &correction_body,
        ))
        .await
        .unwrap();
    let corrected: IntegrationOrderIntakeResponse =
        success(correction_response, StatusCode::OK).await;
    assert_eq!(
        corrected.status,
        IntegrationOrderProcessingStatus::Quarantined
    );
    assert_eq!(corrected.revision.get(), 2);
    assert_eq!(corrected.attempt_count, 2);
    let correction_id = corrected.correction_id.unwrap();
    assert_ne!(corrected.input_payload_sha256, initial.input_payload_sha256);

    let exact_replay = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            &correction_uri,
            Some("correction-command"),
            &correction_body,
        ))
        .await
        .unwrap();
    assert_eq!(exact_replay.status(), StatusCode::OK);
    assert_eq!(
        response::<IntegrationOrderIntakeResponse>(exact_replay).await,
        corrected
    );

    let changed_replay = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            &correction_uri,
            Some("correction-command"),
            &CorrectIntegrationOrderRequest {
                reason: "Different rationale".into(),
                ..correction_body.clone()
            },
        ))
        .await
        .unwrap();
    assert_eq!(changed_replay.status(), StatusCode::CONFLICT);

    link_item(&fixture, tenant_id, owner_id, item_id).await;
    let reprocess_uri = format!(
        "/api/v1/integration-monitor/inbound/{}/reprocessings",
        initial.receipt_id
    );
    let processed: IntegrationOrderIntakeResponse = success(
        app.clone()
            .oneshot(request(
                &token,
                tenant_id,
                Method::POST,
                &reprocess_uri,
                Some("correction-reprocess"),
                &ReprocessIntegrationOrderRequest {
                    expected_revision: Revision::new(2).unwrap(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        processed.status,
        IntegrationOrderProcessingStatus::Processed
    );
    assert_eq!(processed.revision.get(), 3);
    assert_eq!(processed.correction_id, Some(correction_id));
    assert_eq!(
        processed.input_payload_sha256,
        corrected.input_payload_sha256
    );

    let detail: InboundIntegrationDetailResponse = success(
        app.oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/integration-monitor/inbound/{}",
                    initial.receipt_id
                ))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, tenant_id.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap(),
        StatusCode::OK,
    )
    .await;
    let processing = detail.processing.unwrap();
    assert_eq!(processing.latest_correction_id, Some(correction_id));
    assert!(processing
        .latest_correction_payload
        .as_deref()
        .is_some_and(|payload| payload.contains("INTAKE-CORRECTED")));
    assert_eq!(processing.attempts.len(), 3);
    assert_eq!(processing.attempts[0].correction_id, Some(correction_id));
    assert_eq!(processing.attempts[1].correction_id, Some(correction_id));
    assert_eq!(
        processing.attempts[1].correction_reason.as_deref(),
        Some("Corrected malformed partner envelope")
    );
    assert!(processing.attempts[2].correction_id.is_none());

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let evidence: (Vec<u8>, Vec<u8>, String, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT receipt.raw_payload,correction.corrected_payload,correction.reason,
               (SELECT COUNT(*) FROM integration_inbox_processing_corrections),
               (SELECT COUNT(*) FROM integration_inbox_processing_attempts),
               (SELECT COUNT(*) FROM orders WHERE order_key='INTAKE-CORRECTED')
        FROM integration_inbox_receipts receipt
        JOIN integration_inbox_processing_corrections correction
          ON correction.tenant_id=receipt.tenant_id AND correction.receipt_id=receipt.id
        WHERE receipt.id=$1
        "#,
    )
    .bind(initial.receipt_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&evidence.0).unwrap(),
        serde_json::json!({"malformed": true})
    );
    assert_ne!(evidence.0, evidence.1);
    assert_eq!(evidence.2, "Corrected malformed partner envelope");
    assert_eq!((evidence.3, evidence.4, evidence.5), (1, 3, 1));
    let admin = admin_db_for(&fixture.db).await;
    assert!(sqlx::query(
        "UPDATE integration_inbox_processing_corrections SET reason='tampered' WHERE id=$1"
    )
    .bind(correction_id)
    .execute(&admin)
    .await
    .is_err());
    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: user.id,
            all_facilities: true,
            facility_ids: Vec::new(),
            all_inventory_owners: false,
            inventory_owner_ids: Vec::new(),
        },
    )
    .await
    .unwrap());
    let concealed_replay = routes::app(AppState::new(fixture.db.clone()))
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            &correction_uri,
            Some("correction-command"),
            &correction_body,
        ))
        .await
        .unwrap();
    assert_eq!(concealed_replay.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn concurrent_reprocess_with_stale_revision_has_one_winner() {
    let fixture = Fixture::new().await;
    let user = fixture.user("integration-order-race@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    grant(&fixture, tenant_id, user.id, "orders").await;
    let owner_id = fixture
        .inventory_owner(tenant_id, "Integration Race Client")
        .await;
    let item_id = fixture
        .item(tenant_id, "Integration Race Item", "case")
        .await;
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let intake_uri =
        format!("/api/v1/integrations/order-intake/race/inventory-owners/{owner_id}/orders");
    let payload = external_order("INTAKE-RACE");
    let quarantined: IntegrationOrderIntakeResponse = success(
        app.clone()
            .oneshot(request(
                &token,
                tenant_id,
                Method::POST,
                &intake_uri,
                Some("race-intake"),
                &payload,
            ))
            .await
            .unwrap(),
        StatusCode::ACCEPTED,
    )
    .await;
    link_item(&fixture, tenant_id, owner_id, item_id).await;
    configure_mapping(&fixture, tenant_id, user.id, owner_id, item_id, "race").await;
    let uri = format!(
        "/api/v1/integration-monitor/inbound/{}/reprocessings",
        quarantined.receipt_id
    );
    let body = ReprocessIntegrationOrderRequest {
        expected_revision: Revision::new(1).unwrap(),
    };
    let (left, right) = tokio::join!(
        app.clone().oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            &uri,
            Some("race-left"),
            &body,
        )),
        app.clone().oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            &uri,
            Some("race-right"),
            &body,
        )),
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert!(
        (left.status() == StatusCode::OK && right.status() == StatusCode::CONFLICT)
            || (left.status() == StatusCode::CONFLICT && right.status() == StatusCode::OK)
    );
    let winner = if left.status() == StatusCode::OK {
        response::<IntegrationOrderIntakeResponse>(left).await
    } else {
        response::<IntegrationOrderIntakeResponse>(right).await
    };
    assert_eq!(winner.status, IntegrationOrderProcessingStatus::Processed);
    assert_eq!(winner.revision.get(), 2);
    assert_eq!(winner.applied_mapping_count, 1);

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let counts: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM integration_inbox_processing_attempts WHERE receipt_id=$1),
               (SELECT COUNT(*) FROM integration_inbox_processing_attempt_mappings WHERE receipt_id=$1),
               (SELECT COUNT(*) FROM orders WHERE order_key='INTAKE-RACE'),
               (SELECT COUNT(*) FROM command_idempotency_records WHERE operation='integration.order_intake.reprocess.v1')
        "#,
    )
    .bind(quarantined.receipt_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(counts, (2, 1, 1, 1));
}
