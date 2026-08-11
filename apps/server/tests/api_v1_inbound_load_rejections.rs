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
    ErrorReason, ErrorResponse, InboundLoadArrivedStatus, InboundLoadRejectedStatus,
    InboundLoadRejectionReason, RejectInboundLoadResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;

fn command_request(
    token: &str,
    tenant_id: TenantId,
    load_id: i64,
    path: &str,
    idempotency_key: &str,
    body: &Value,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/inbound-loads/{load_id}/{path}"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(IDEMPOTENCY_KEY_HEADER, idempotency_key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn rejection_request(
    token: &str,
    tenant_id: TenantId,
    load_id: i64,
    idempotency_key: &str,
    body: &Value,
) -> Request<Body> {
    command_request(
        token,
        tenant_id,
        load_id,
        "rejections",
        idempotency_key,
        body,
    )
}

fn legacy_reject_request(token: &str, tenant_id: TenantId, load_id: i64) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/loads/update")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "load_id": load_id,
                "status": "rejected",
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

struct RejectionFixture {
    fixture: Fixture,
    tenant_id: TenantId,
    actor_id: i64,
    facility_id: i64,
    owner_id: i64,
    dock_id: i64,
    dock_barcode: String,
    token: String,
}

async fn scoped_fixture(email: &str) -> RejectionFixture {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user(email).await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let facility_id = fixture.facility(tenant_id, "Rejection DC").await;
    let owner_id = fixture.inventory_owner(tenant_id, "Rejection Owner").await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;
    let dock_barcode = format!("REJECT-DOCK-{}", operator.id);
    let dock_id = wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some(&dock_barcode),
        Some("Rejection dock"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    RejectionFixture {
        fixture,
        tenant_id,
        actor_id: operator.id,
        facility_id,
        owner_id,
        dock_id,
        dock_barcode,
        token,
    }
}

async fn arrived_load(context: &RejectionFixture, reference: &str, key: &str) -> (i64, String) {
    let load_id = repo::loads::add_load(
        &context.fixture.db,
        context.tenant_id,
        context.actor_id,
        context.facility_id,
        context.owner_id,
        LoadType::Inbound,
        Some(reference),
        None,
        Some("Parity Freight"),
        None,
        None,
        Some(context.dock_id),
        None,
        None,
    )
    .await
    .unwrap();
    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let execution_barcode: String =
        sqlx::query_scalar("SELECT execution_barcode FROM loads WHERE id=$1")
            .bind(load_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.rollback().await.unwrap();
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let response = app
        .oneshot(command_request(
            &context.token,
            context.tenant_id,
            load_id,
            "arrivals",
            key,
            &json!({
                "load_scan": execution_barcode,
                "receiving_location_scan": context.dock_barcode,
                "arrived_at": null
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    (load_id, execution_barcode)
}

fn body(load_scan: &str, dock_scan: &str) -> Value {
    json!({
        "load_scan": load_scan,
        "receiving_location_scan": dock_scan,
        "reason": "seal_discrepancy",
        "note": "seal was broken at check-in"
    })
}

#[tokio::test]
async fn arrived_rejection_is_scan_exact_replay_safe_and_audited() {
    let context = scoped_fixture("reject-atomic@test.local").await;
    let (load_id, execution_barcode) =
        arrived_load(&context, "REJECT-ATOMIC", "arrive-atomic").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));

    let legacy = app
        .clone()
        .oneshot(legacy_reject_request(
            &context.token,
            context.tenant_id,
            load_id,
        ))
        .await
        .unwrap();
    assert_eq!(legacy.status(), StatusCode::CONFLICT);

    for (key, invalid_body) in [
        (
            "reject-wrong-load",
            body("WRONG-LOAD", &context.dock_barcode),
        ),
        ("reject-wrong-dock", body(&execution_barcode, "WRONG-DOCK")),
    ] {
        let response = app
            .clone()
            .oneshot(rejection_request(
                &context.token,
                context.tenant_id,
                load_id,
                key,
                &invalid_body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    let request_body = body(&execution_barcode, &context.dock_barcode);
    let first = app
        .clone()
        .oneshot(rejection_request(
            &context.token,
            context.tenant_id,
            load_id,
            "reject-atomic",
            &request_body,
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: RejectInboundLoadResponse = json_body(first).await;
    assert_eq!(first.previous_status, InboundLoadArrivedStatus::Arrived);
    assert_eq!(first.status, InboundLoadRejectedStatus::Rejected);
    assert_eq!(first.reason, InboundLoadRejectionReason::SealDiscrepancy);
    assert_eq!(first.receiving_location_id, context.dock_id);

    let replay = app
        .clone()
        .oneshot(rejection_request(
            &context.token,
            context.tenant_id,
            load_id,
            "reject-atomic",
            &request_body,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(json_body::<RejectInboundLoadResponse>(replay).await, first);

    let changed = app
        .clone()
        .oneshot(rejection_request(
            &context.token,
            context.tenant_id,
            load_id,
            "reject-atomic",
            &json!({
                "load_scan": execution_barcode,
                "receiving_location_scan": context.dock_barcode,
                "reason": "load_damaged",
                "note": null
            }),
        ))
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let evidence: (i64, i64, i64, i64, String, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM inbound_load_rejections WHERE load_id=$1),
          (SELECT COUNT(*) FROM load_activity WHERE load_id=$1 AND action='rejected'),
          (SELECT COUNT(*) FROM command_idempotency_records
             WHERE operation='inbound.load.reject.v1' AND (result_json->>'load_id')::BIGINT=$1),
          (SELECT COUNT(*) FROM outbox_events
             WHERE event_type='inbound.load.rejected' AND aggregate_id=$1::TEXT),
          (SELECT status FROM loads WHERE id=$1),
          (SELECT COUNT(*) FROM inbound_load_unloading_starts WHERE load_id=$1)
        "#,
    )
    .bind(load_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(evidence, (1, 1, 1, 1, "rejected".to_owned(), 0));

    let admin = admin_db_for(&context.fixture.db).await;
    assert!(sqlx::query(
        "UPDATE inbound_load_rejections SET reason_code='load_damaged' WHERE id=$1"
    )
    .bind(first.rejection_id)
    .execute(&admin)
    .await
    .is_err());
    assert!(sqlx::query(
        r#"
        INSERT INTO loads
            (tenant_id,created,facility_id,inventory_owner_id,execution_barcode,status,type,receive_completed)
        VALUES ($1,now(),$2,$3,$4,'rejected','inbound',false)
        "#,
    )
    .bind(context.tenant_id.get())
    .bind(context.facility_id)
    .bind(context.owner_id)
    .bind(format!("BYPASS-REJECT-{load_id}"))
    .execute(&admin)
    .await
    .is_err());
    admin.close().await;
}

#[tokio::test]
async fn rejection_and_unloading_race_to_one_terminal_effect() {
    let context = scoped_fixture("reject-race@test.local").await;
    let (late_load_id, late_barcode) = arrived_load(&context, "REJECT-LATE", "arrive-late").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let unloading_body = json!({
        "load_scan": late_barcode,
        "receiving_location_scan": context.dock_barcode,
        "seal_scan": null,
        "started_at": null
    });
    let unloading = app
        .clone()
        .oneshot(command_request(
            &context.token,
            context.tenant_id,
            late_load_id,
            "unloading-starts",
            "unload-before-reject",
            &unloading_body,
        ))
        .await
        .unwrap();
    assert_eq!(unloading.status(), StatusCode::OK);
    let late = app
        .clone()
        .oneshot(rejection_request(
            &context.token,
            context.tenant_id,
            late_load_id,
            "reject-after-unload",
            &body(&late_barcode, &context.dock_barcode),
        ))
        .await
        .unwrap();
    assert_eq!(late.status(), StatusCode::CONFLICT);

    let (load_id, execution_barcode) = arrived_load(&context, "REJECT-RACE", "arrive-race").await;
    let reject = app.clone().oneshot(rejection_request(
        &context.token,
        context.tenant_id,
        load_id,
        "reject-race",
        &body(&execution_barcode, &context.dock_barcode),
    ));
    let unload = app.clone().oneshot(command_request(
        &context.token,
        context.tenant_id,
        load_id,
        "unloading-starts",
        "unload-race",
        &json!({
            "load_scan": execution_barcode,
            "receiving_location_scan": context.dock_barcode,
            "seal_scan": null,
            "started_at": null
        }),
    ));
    let (reject, unload) = tokio::join!(reject, unload);
    let statuses = [reject.unwrap().status(), unload.unwrap().status()];
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

    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let effects: (i64, i64, String) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM inbound_load_rejections WHERE load_id=$1),
          (SELECT COUNT(*) FROM inbound_load_unloading_starts WHERE load_id=$1),
          (SELECT status FROM loads WHERE id=$1)
        "#,
    )
    .bind(load_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert!(
        matches!(effects, (1, 0, ref status) if status == "rejected")
            || matches!(effects, (0, 1, ref status) if status == "receiving")
    );
}

#[tokio::test]
async fn rejection_replays_fail_closed_after_scope_revocation_and_rls_is_minimal() {
    let context = scoped_fixture("reject-scope@test.local").await;
    assert!(repo::tenants::update_user_access_scope(
        &context.fixture.db,
        context.tenant_id,
        &UpdateUserAccessScope {
            user_id: context.actor_id,
            all_facilities: false,
            facility_ids: vec![context.facility_id],
            all_inventory_owners: false,
            inventory_owner_ids: vec![context.owner_id],
        },
    )
    .await
    .unwrap());
    let (load_id, execution_barcode) = arrived_load(&context, "REJECT-SCOPE", "arrive-scope").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let request_body = body(&execution_barcode, &context.dock_barcode);
    let first = app
        .clone()
        .oneshot(rejection_request(
            &context.token,
            context.tenant_id,
            load_id,
            "reject-scope",
            &request_body,
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    assert!(repo::tenants::update_user_access_scope(
        &context.fixture.db,
        context.tenant_id,
        &UpdateUserAccessScope {
            user_id: context.actor_id,
            all_facilities: false,
            facility_ids: vec![],
            all_inventory_owners: false,
            inventory_owner_ids: vec![],
        },
    )
    .await
    .unwrap());
    for hidden_body in [
        request_body,
        json!({
            "load_scan": execution_barcode,
            "receiving_location_scan": context.dock_barcode,
            "reason": "wrong_facility",
            "note": null
        }),
    ] {
        let hidden = app
            .clone()
            .oneshot(rejection_request(
                &context.token,
                context.tenant_id,
                load_id,
                "reject-scope",
                &hidden_body,
            ))
            .await
            .unwrap();
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json_body::<ErrorResponse>(hidden).await.reason,
            ErrorReason::NotFound
        );
    }

    let admin = admin_db_for(&context.fixture.db).await;
    let privileges: (bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT
          has_table_privilege('wareboxes_app','inbound_load_rejections','SELECT'),
          has_table_privilege('wareboxes_app','inbound_load_rejections','INSERT'),
          has_table_privilege('wareboxes_app','inbound_load_rejections','UPDATE'),
          has_table_privilege('wareboxes_app','inbound_load_rejections','DELETE')
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(privileges, (true, true, false, false));
    admin.close().await;

    let outsider = context.fixture.wms_user("reject-outsider@test.local").await;
    let outsider_tenant = tenant_for_user(&context.fixture.db, outsider.id).await;
    let outsider_token = auth::create_session(&context.fixture.db, outsider.id)
        .await
        .unwrap();
    let hidden = app
        .oneshot(rejection_request(
            &outsider_token,
            outsider_tenant,
            load_id,
            "reject-cross-tenant",
            &json!({
                "load_scan": "GUESSED",
                "receiving_location_scan": "GUESSED",
                "reason": "wrong_facility",
                "note": null
            }),
        ))
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
}
