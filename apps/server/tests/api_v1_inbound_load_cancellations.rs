mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    CancelInboundLoadResponse, ErrorReason, ErrorResponse, InboundLoadCancellationReason,
    InboundLoadCancelledStatus, InboundLoadPreArrivalStatus,
};
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_domain::Timestamp;

fn cancellation_request(
    token: &str,
    tenant_id: TenantId,
    load_id: i64,
    idempotency_key: &str,
    body: &Value,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/inbound-loads/{load_id}/cancellations"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(IDEMPOTENCY_KEY_HEADER, idempotency_key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn appointment_request(
    token: &str,
    tenant_id: TenantId,
    load_id: i64,
    idempotency_key: &str,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/inbound-loads/{load_id}/appointments"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(IDEMPOTENCY_KEY_HEADER, idempotency_key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "scheduled_for": "2027-08-12T17:00:00Z" }).to_string(),
        ))
        .unwrap()
}

fn legacy_cancel_request(token: &str, tenant_id: TenantId, load_id: i64) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/loads/update")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "load_id": load_id,
                "status": "cancelled",
                "type": null,
                "reference_number": null,
                "invoice_number": null,
                "carrier": null,
                "trailer_number": null,
                "seal_number": null,
                "dock_door_location_id": null,
                "expected_time": null,
                "appointment_time": null,
                "actual_time": null,
                "arrival": null,
                "departure": null,
                "rejected": null,
                "closed": null
            })
            .to_string(),
        ))
        .unwrap()
}

async fn json_body<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn planned_load(
    fixture: &Fixture,
    tenant_id: TenantId,
    actor_id: i64,
    facility_id: i64,
    inventory_owner_id: i64,
    reference: &str,
) -> i64 {
    repo::loads::add_load(
        &fixture.db,
        tenant_id,
        actor_id,
        facility_id,
        inventory_owner_id,
        LoadType::Inbound,
        Some(reference),
        None,
        Some("Parity Freight"),
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap()
}

async fn scoped_fixture(email: &str) -> (Fixture, TenantId, i64, i64, i64, String) {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user(email).await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let facility_id = fixture.facility(tenant_id, "Cancellation DC").await;
    let inventory_owner_id = fixture
        .inventory_owner(tenant_id, "Cancellation Owner")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    (
        fixture,
        tenant_id,
        operator.id,
        facility_id,
        inventory_owner_id,
        token,
    )
}

#[tokio::test]
async fn planned_cancellation_is_atomic_replay_safe_and_audited() {
    let (fixture, tenant_id, actor_id, facility_id, owner_id, token) =
        scoped_fixture("cancel-planned@test.local").await;
    let load_id = planned_load(
        &fixture,
        tenant_id,
        actor_id,
        facility_id,
        owner_id,
        "CANCEL-PLANNED",
    )
    .await;
    let app = routes::app(AppState::new(fixture.db.clone()));
    let body = json!({
        "reason": "supplier_cancelled",
        "note": "supplier withdrew the shipment"
    });

    let legacy = app
        .clone()
        .oneshot(legacy_cancel_request(&token, tenant_id, load_id))
        .await
        .unwrap();
    assert_eq!(legacy.status(), StatusCode::CONFLICT);

    let first = app
        .clone()
        .oneshot(cancellation_request(
            &token,
            tenant_id,
            load_id,
            "cancel-planned",
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: CancelInboundLoadResponse = json_body(first).await;
    assert_eq!(first.previous_status, InboundLoadPreArrivalStatus::Planned);
    assert_eq!(first.status, InboundLoadCancelledStatus::Cancelled);
    assert_eq!(
        first.reason,
        InboundLoadCancellationReason::SupplierCancelled
    );
    assert_eq!(
        first.note.as_deref(),
        Some("supplier withdrew the shipment")
    );

    let replay = app
        .clone()
        .oneshot(cancellation_request(
            &token,
            tenant_id,
            load_id,
            "cancel-planned",
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(json_body::<CancelInboundLoadResponse>(replay).await, first);

    let conflict = app
        .clone()
        .oneshot(cancellation_request(
            &token,
            tenant_id,
            load_id,
            "cancel-planned",
            &json!({ "reason": "duplicate_plan", "note": null }),
        ))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body::<ErrorResponse>(conflict).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let evidence: (i64, i64, i64, i64, String) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM inbound_load_cancellations WHERE load_id=$2),
          (SELECT COUNT(*) FROM load_activity WHERE load_id=$2 AND action='cancelled'),
          (SELECT COUNT(*) FROM command_idempotency_records
             WHERE operation='inbound.load.cancel.v1' AND (result_json->>'load_id')::BIGINT=$2),
          (SELECT COUNT(*) FROM outbox_events
             WHERE event_type='inbound.load.cancelled' AND aggregate_id=$2::TEXT),
          (SELECT status FROM loads WHERE id=$2)
        "#,
    )
    .bind(tenant_id.get())
    .bind(load_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(evidence, (1, 1, 1, 1, "cancelled".to_owned()));

    let admin = admin_db_for(&fixture.db).await;
    let immutable = sqlx::query(
        "UPDATE inbound_load_cancellations SET reason_code='duplicate_plan' WHERE id=$1",
    )
    .bind(first.cancellation_id)
    .execute(&admin)
    .await;
    assert!(immutable.is_err());
    let bypass = sqlx::query(
        r#"
        INSERT INTO loads
            (tenant_id,created,facility_id,inventory_owner_id,execution_barcode,status,type,receive_completed)
        VALUES ($1,now(),$2,$3,$4,'cancelled','inbound',false)
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id)
    .bind(owner_id)
    .bind(format!("BYPASS-CANCEL-{load_id}"))
    .execute(&admin)
    .await;
    assert!(bypass.is_err());
    admin.close().await;
}

#[tokio::test]
async fn scheduled_cancellation_preserves_appointment_and_rejects_late_retries() {
    let (fixture, tenant_id, actor_id, facility_id, owner_id, token) =
        scoped_fixture("cancel-scheduled@test.local").await;
    let load_id = planned_load(
        &fixture,
        tenant_id,
        actor_id,
        facility_id,
        owner_id,
        "CANCEL-SCHEDULED",
    )
    .await;
    let app = routes::app(AppState::new(fixture.db.clone()));
    let scheduled = app
        .clone()
        .oneshot(appointment_request(
            &token,
            tenant_id,
            load_id,
            "schedule-before-cancel",
        ))
        .await
        .unwrap();
    assert_eq!(scheduled.status(), StatusCode::OK);

    let cancellation = app
        .clone()
        .oneshot(cancellation_request(
            &token,
            tenant_id,
            load_id,
            "cancel-scheduled",
            &json!({ "reason": "warehouse_capacity", "note": null }),
        ))
        .await
        .unwrap();
    assert_eq!(cancellation.status(), StatusCode::OK);
    let cancellation: CancelInboundLoadResponse = json_body(cancellation).await;
    assert_eq!(
        cancellation.previous_status,
        InboundLoadPreArrivalStatus::Scheduled
    );

    let late = app
        .clone()
        .oneshot(cancellation_request(
            &token,
            tenant_id,
            load_id,
            "cancel-late",
            &json!({ "reason": "carrier_cancelled", "note": null }),
        ))
        .await
        .unwrap();
    assert_eq!(late.status(), StatusCode::CONFLICT);

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let appointment: Option<Timestamp> =
        sqlx::query_scalar("SELECT appointment_time FROM loads WHERE id=$1")
            .bind(load_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        appointment.unwrap().to_rfc3339(),
        "2027-08-12T17:00:00+00:00"
    );
}

#[tokio::test]
async fn concurrent_cancellations_have_one_winner_and_one_effect() {
    let (fixture, tenant_id, actor_id, facility_id, owner_id, token) =
        scoped_fixture("cancel-race@test.local").await;
    let load_id = planned_load(
        &fixture,
        tenant_id,
        actor_id,
        facility_id,
        owner_id,
        "CANCEL-RACE",
    )
    .await;
    let app = routes::app(AppState::new(fixture.db.clone()));
    let body = json!({ "reason": "duplicate_plan", "note": null });
    let (left, right) = tokio::join!(
        app.clone().oneshot(cancellation_request(
            &token,
            tenant_id,
            load_id,
            "cancel-race-left",
            &body,
        )),
        app.clone().oneshot(cancellation_request(
            &token,
            tenant_id,
            load_id,
            "cancel-race-right",
            &body,
        )),
    );
    let statuses = [left.unwrap().status(), right.unwrap().status()];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CONFLICT)
            .count(),
        1
    );

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let counts: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM inbound_load_cancellations WHERE load_id=$1),
          (SELECT COUNT(*) FROM load_activity WHERE load_id=$1 AND action='cancelled'),
          (SELECT COUNT(*) FROM outbox_events WHERE event_type='inbound.load.cancelled' AND aggregate_id=$1::TEXT)
        "#,
    )
    .bind(load_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(counts, (1, 1, 1));
}

#[tokio::test]
async fn cancellation_replays_fail_closed_after_scope_revocation_and_rls_is_minimal() {
    let (fixture, tenant_id, actor_id, facility_id, owner_id, token) =
        scoped_fixture("cancel-scope@test.local").await;
    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: actor_id,
            all_facilities: false,
            facility_ids: vec![facility_id],
            all_inventory_owners: false,
            inventory_owner_ids: vec![owner_id],
        },
    )
    .await
    .unwrap());
    let load_id = planned_load(
        &fixture,
        tenant_id,
        actor_id,
        facility_id,
        owner_id,
        "CANCEL-SCOPE",
    )
    .await;
    let app = routes::app(AppState::new(fixture.db.clone()));
    let body = json!({ "reason": "carrier_cancelled", "note": null });
    let first = app
        .clone()
        .oneshot(cancellation_request(
            &token,
            tenant_id,
            load_id,
            "cancel-scope",
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: actor_id,
            all_facilities: false,
            facility_ids: vec![],
            all_inventory_owners: false,
            inventory_owner_ids: vec![],
        },
    )
    .await
    .unwrap());
    for request_body in [
        body,
        json!({ "reason": "supplier_cancelled", "note": null }),
    ] {
        let hidden = app
            .clone()
            .oneshot(cancellation_request(
                &token,
                tenant_id,
                load_id,
                "cancel-scope",
                &request_body,
            ))
            .await
            .unwrap();
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json_body::<ErrorResponse>(hidden).await.reason,
            ErrorReason::NotFound
        );
    }

    let admin = admin_db_for(&fixture.db).await;
    let privileges: (bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT
          has_table_privilege('wareboxes_app','inbound_load_cancellations','SELECT'),
          has_table_privilege('wareboxes_app','inbound_load_cancellations','INSERT'),
          has_table_privilege('wareboxes_app','inbound_load_cancellations','UPDATE'),
          has_table_privilege('wareboxes_app','inbound_load_cancellations','DELETE')
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(privileges, (true, true, false, false));
    admin.close().await;

    let outsider = fixture.wms_user("cancel-outsider@test.local").await;
    let outsider_tenant = tenant_for_user(&fixture.db, outsider.id).await;
    let outsider_token = auth::create_session(&fixture.db, outsider.id)
        .await
        .unwrap();
    let hidden = app
        .oneshot(cancellation_request(
            &outsider_token,
            outsider_tenant,
            load_id,
            "cancel-cross-tenant",
            &json!({ "reason": "duplicate_plan", "note": null }),
        ))
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
}
