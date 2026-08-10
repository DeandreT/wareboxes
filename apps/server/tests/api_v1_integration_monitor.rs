mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use common::*;
use serde::de::DeserializeOwned;
use serde_json::json;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    ErrorReason, ErrorResponse, InboundIntegrationPage, OutboundDeliveryStatus,
    OutboundIntegrationDetailResponse, OutboundIntegrationPage,
};
use wareboxes_application::integration::NewIntegrationInboxReceipt;
use wareboxes_application::outbox::DeliveryFailureClass;
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_domain::{FacilityId, InventoryOwnerId, TenantId};
use wareboxes_persistence_postgres::integration_inbox;
use wareboxes_persistence_postgres::outbox::{self, FailOutboxEvent, NewOutboxEvent};

fn request(token: &str, tenant_id: TenantId, uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .body(Body::empty())
        .unwrap()
}

async fn response<T: DeserializeOwned>(response: axum::response::Response) -> T {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "could not decode {status} response as {}: {error}; body={}",
            std::any::type_name::<T>(),
            String::from_utf8_lossy(&bytes)
        )
    })
}

async fn grant_permission(fixture: &Fixture, tenant_id: TenantId, user_id: i64, name: &str) {
    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        &fixture.db,
        tenant_id,
        name,
        Some(name),
    )
    .await
    .unwrap();
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("integration-monitor-{name}-{user_id}"),
        Some("Integration monitor test role"),
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

async fn receive(
    fixture: &Fixture,
    tenant_id: TenantId,
    owner_id: i64,
    facility_id: i64,
    source: &str,
    key: &str,
    payload: &[u8],
) -> i64 {
    integration_inbox::receive(
        &fixture.db,
        &NewIntegrationInboxReceipt {
            tenant_id,
            inventory_owner_id: Some(InventoryOwnerId::new(owner_id).unwrap()),
            facility_id: Some(FacilityId::new(facility_id).unwrap()),
            source_key: source,
            deduplication_key: key,
            content_type: "application/json",
            raw_payload: payload,
            request_id: Some(key),
        },
    )
    .await
    .unwrap()
    .receipt
    .id
}

async fn enqueue(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    owner_id: i64,
    facility_id: i64,
    event_key: &str,
    event_type: &str,
) -> i64 {
    let mut tx = fixture.db.begin().await.unwrap();
    let payload = json!({"event_key": event_key, "visible": true});
    let id = outbox::enqueue(
        &mut tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(InventoryOwnerId::new(owner_id).unwrap()),
            facility_id: Some(FacilityId::new(facility_id).unwrap()),
            actor_user_id: Some(user_id),
            event_key,
            aggregate_type: "shipment",
            aggregate_id: event_key,
            ordering_key: event_key,
            aggregate_sequence: 1,
            event_type,
            schema_version: 1,
            payload: &payload,
            occurred_at: db::now_iso(),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

#[tokio::test]
async fn integration_monitor_is_scope_safe_server_sorted_and_exposes_attempt_evidence() {
    let fixture = Fixture::new().await;
    let admin = fixture.user("integration-admin@test.local").await;
    let operator = fixture.wms_user("integration-operator@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, admin.id).await;
    grant_permission(&fixture, tenant_id, admin.id, "admin").await;
    let allowed_owner = fixture.inventory_owner(tenant_id, "Visible Client").await;
    let denied_owner = fixture.inventory_owner(tenant_id, "Hidden Client").await;
    let allowed_facility = fixture.facility(tenant_id, "Visible DC").await;
    let denied_facility = fixture.facility(tenant_id, "Hidden DC").await;
    fixture
        .assign_owner_to_facility(tenant_id, allowed_owner, allowed_facility)
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, denied_owner, denied_facility)
        .await;

    let small_receipt = receive(
        &fixture,
        tenant_id,
        allowed_owner,
        allowed_facility,
        "edi-orders",
        "small-request",
        b"{}",
    )
    .await;
    let large_receipt = receive(
        &fixture,
        tenant_id,
        allowed_owner,
        allowed_facility,
        "partner-sftp",
        "large-request",
        br#"{"order":"large-payload"}"#,
    )
    .await;
    let _hidden_receipt = receive(
        &fixture,
        tenant_id,
        denied_owner,
        denied_facility,
        "hidden-source",
        "hidden-request",
        b"hidden payload",
    )
    .await;

    let published_event = enqueue(
        &fixture,
        tenant_id,
        admin.id,
        allowed_owner,
        allowed_facility,
        "shipping-published",
        "shipping.shipment_departed",
    )
    .await;
    let failed_event = enqueue(
        &fixture,
        tenant_id,
        admin.id,
        allowed_owner,
        allowed_facility,
        "shipping-failed",
        "shipping.shipment_created",
    )
    .await;
    let _hidden_event = enqueue(
        &fixture,
        tenant_id,
        admin.id,
        denied_owner,
        denied_facility,
        "hidden-event",
        "hidden.event",
    )
    .await;
    let claimed = outbox::claim_events(
        &fixture.db,
        tenant_id,
        "integration-monitor-worker",
        "integration-monitor-publisher",
        10,
        60,
    )
    .await
    .unwrap();
    let published = claimed
        .iter()
        .find(|event| event.id == published_event)
        .unwrap();
    assert!(outbox::mark_published(
        &fixture.db,
        tenant_id,
        published.id,
        "integration-monitor-worker",
        published.claim_version,
    )
    .await
    .unwrap());
    let failed = claimed
        .iter()
        .find(|event| event.id == failed_event)
        .unwrap();
    assert!(outbox::mark_failed(
        &fixture.db,
        &FailOutboxEvent {
            tenant_id,
            event_id: failed.id,
            worker_id: "integration-monitor-worker",
            claim_version: failed.claim_version,
            failure_class: DeliveryFailureClass::Permanent,
            error: "partner rejected schema",
            retry_after_seconds: 0,
            max_attempts: 3,
        },
    )
    .await
    .unwrap());

    assert!(wareboxes_api::repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: admin.id,
            all_facilities: false,
            facility_ids: vec![allowed_facility],
            all_inventory_owners: false,
            inventory_owner_ids: vec![allowed_owner],
        },
    )
    .await
    .unwrap());
    let token = wareboxes_api::auth::create_session(&fixture.db, admin.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let first = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            "/api/v1/integration-monitor/inbound?sort=payload_size&direction=ascending&limit=1",
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: InboundIntegrationPage = response(first).await;
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].id, small_receipt);
    assert_eq!(
        first.items[0].inventory_owner_name.as_deref(),
        Some("Visible Client")
    );
    assert_eq!(first.items[0].facility_name.as_deref(), Some("Visible DC"));
    assert_eq!(first.items[0].payload_sha256.len(), 64);
    let cursor = first.next_cursor.unwrap();

    let second = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            &format!(
                "/api/v1/integration-monitor/inbound?sort=payload_size&direction=ascending&limit=1&cursor={cursor}"
            ),
        ))
        .await
        .unwrap();
    let second: InboundIntegrationPage = response(second).await;
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].id, large_receipt);
    assert!(second.next_cursor.is_none());

    let mismatched_cursor = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            &format!(
                "/api/v1/integration-monitor/inbound?sort=received_at&direction=ascending&limit=1&cursor={cursor}"
            ),
        ))
        .await
        .unwrap();
    assert_eq!(mismatched_cursor.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response::<ErrorResponse>(mismatched_cursor).await.reason,
        ErrorReason::InvalidCursor
    );

    let published = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            "/api/v1/integration-monitor/outbound?status=published",
        ))
        .await
        .unwrap();
    assert_eq!(published.status(), StatusCode::OK);
    let published: OutboundIntegrationPage = response(published).await;
    assert_eq!(published.items.len(), 1);
    assert_eq!(published.items[0].id, published_event);
    assert_eq!(published.items[0].status, OutboundDeliveryStatus::Published);

    let failed = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            &format!("/api/v1/integration-monitor/outbound/{failed_event}"),
        ))
        .await
        .unwrap();
    assert_eq!(failed.status(), StatusCode::OK);
    let failed: OutboundIntegrationDetailResponse = response(failed).await;
    assert_eq!(failed.event.status, OutboundDeliveryStatus::DeadLettered);
    assert_eq!(
        failed.event.last_error.as_deref(),
        Some("partner rejected schema")
    );
    assert_eq!(failed.attempts.len(), 1);
    assert_eq!(
        failed.attempts[0].error.as_deref(),
        Some("partner rejected schema")
    );
    assert_eq!(failed.payload["event_key"], "shipping-failed");

    let operator_token = wareboxes_api::auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let operator_tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let forbidden = app
        .oneshot(request(
            &operator_token,
            operator_tenant_id,
            "/api/v1/integration-monitor/outbound",
        ))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
}
