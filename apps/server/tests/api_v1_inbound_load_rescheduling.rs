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
    ErrorReason, ErrorResponse, InboundLoadAppointmentRescheduleReason, InboundLoadScheduledStatus,
    RescheduleInboundLoadAppointmentResponse, ScheduleInboundLoadResponse,
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

async fn json_body<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

struct RescheduleFixture {
    fixture: Fixture,
    tenant_id: TenantId,
    actor_id: i64,
    facility_id: i64,
    owner_id: i64,
    token: String,
}

async fn scoped_fixture(email: &str) -> RescheduleFixture {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user(email).await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let facility_id = fixture.facility(tenant_id, "Appointment DC").await;
    let owner_id = fixture
        .inventory_owner(tenant_id, "Appointment Owner")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    RescheduleFixture {
        fixture,
        tenant_id,
        actor_id: operator.id,
        facility_id,
        owner_id,
        token,
    }
}

async fn scheduled_load(
    context: &RescheduleFixture,
    reference: &str,
    scheduled_for: &str,
) -> (i64, ScheduleInboundLoadResponse) {
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
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let response = routes::app(AppState::new(context.fixture.db.clone()))
        .oneshot(command_request(
            &context.token,
            context.tenant_id,
            load_id,
            "appointments",
            &format!("schedule-{load_id}"),
            &json!({"scheduled_for":scheduled_for}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    (load_id, json_body(response).await)
}

fn body(expected: &str, scheduled_for: &str, reason: &str) -> Value {
    json!({
        "expected_scheduled_for": expected,
        "scheduled_for": scheduled_for,
        "reason": reason,
        "note": "appointment changed during acceptance"
    })
}

#[tokio::test]
async fn appointment_reschedule_race_replays_once_and_is_immutable() {
    let context = scoped_fixture("reschedule-race@test.local").await;
    let (load_id, scheduled) =
        scheduled_load(&context, "RESCHEDULE-RACE", "2027-08-12T17:00:00Z").await;
    let first_body = body(
        &scheduled.scheduled_for,
        "2027-08-12T19:00:00Z",
        "carrier_delay",
    );
    let second_body = body(
        &scheduled.scheduled_for,
        "2027-08-12T20:00:00Z",
        "dock_capacity",
    );
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let first = app.clone().oneshot(command_request(
        &context.token,
        context.tenant_id,
        load_id,
        "appointment-reschedules",
        "reschedule-race-a",
        &first_body,
    ));
    let second = app.clone().oneshot(command_request(
        &context.token,
        context.tenant_id,
        load_id,
        "appointment-reschedules",
        "reschedule-race-b",
        &second_body,
    ));
    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap();
    let second = second.unwrap();
    let (winner_key, winner_body, winner) = if first.status() == StatusCode::OK {
        assert_eq!(second.status(), StatusCode::CONFLICT);
        (
            "reschedule-race-a",
            first_body.clone(),
            json_body::<RescheduleInboundLoadAppointmentResponse>(first).await,
        )
    } else {
        assert_eq!(first.status(), StatusCode::CONFLICT);
        assert_eq!(second.status(), StatusCode::OK);
        (
            "reschedule-race-b",
            second_body.clone(),
            json_body::<RescheduleInboundLoadAppointmentResponse>(second).await,
        )
    };
    assert_eq!(winner.status, InboundLoadScheduledStatus::Scheduled);
    assert_eq!(winner.sequence, 1);
    assert_eq!(winner.appointment_id, scheduled.appointment_id);
    assert_eq!(winner.previous_scheduled_for, scheduled.scheduled_for);

    let replay = app
        .clone()
        .oneshot(command_request(
            &context.token,
            context.tenant_id,
            load_id,
            "appointment-reschedules",
            winner_key,
            &winner_body,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        json_body::<RescheduleInboundLoadAppointmentResponse>(replay).await,
        winner
    );
    let changed = app
        .oneshot(command_request(
            &context.token,
            context.tenant_id,
            load_id,
            "appointment-reschedules",
            winner_key,
            &body(&scheduled.scheduled_for, "2027-08-13T17:00:00Z", "weather"),
        ))
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let evidence: (i64, i64, i64, i64, String, i64, String) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM inbound_load_appointment_reschedules WHERE load_id=$1),
          (SELECT COUNT(*) FROM command_idempotency_records
             WHERE operation='inbound.load.appointment.reschedule.v1'
               AND (result_json->>'load_id')::BIGINT=$1),
          (SELECT COUNT(*) FROM load_activity
             WHERE load_id=$1 AND action='appointment_rescheduled'),
          (SELECT COUNT(*) FROM outbox_events
             WHERE event_type='inbound.load.appointment_rescheduled'
               AND aggregate_id=$1::TEXT),
          (SELECT appointment_time::TEXT FROM loads WHERE id=$1),
          (SELECT aggregate_sequence FROM outbox_events
             WHERE event_type='inbound.load.appointment_rescheduled'
               AND aggregate_id=$1::TEXT),
          (SELECT payload->>'reason' FROM outbox_events
             WHERE event_type='inbound.load.appointment_rescheduled'
               AND aggregate_id=$1::TEXT)
        "#,
    )
    .bind(load_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        (evidence.0, evidence.1, evidence.2, evidence.3),
        (1, 1, 1, 1)
    );
    assert!(evidence
        .4
        .starts_with(&winner.scheduled_for[..19].replace('T', " ")));
    assert_eq!(evidence.5, 2);
    assert_eq!(
        evidence.6,
        match winner.reason {
            InboundLoadAppointmentRescheduleReason::CarrierDelay => "carrier_delay",
            InboundLoadAppointmentRescheduleReason::DockCapacity => "dock_capacity",
            _ => unreachable!(),
        }
    );

    let admin = admin_db_for(&context.fixture.db).await;
    assert!(sqlx::query(
        "UPDATE inbound_load_appointment_reschedules SET reason_code='weather' WHERE id=$1"
    )
    .bind(winner.reschedule_id)
    .execute(&admin)
    .await
    .is_err());
    assert!(
        sqlx::query("UPDATE loads SET appointment_time='2027-08-14T17:00:00Z' WHERE id=$1")
            .bind(load_id)
            .execute(&admin)
            .await
            .is_err()
    );
    admin.close().await;
}

#[tokio::test]
async fn repeated_reschedules_form_an_exact_optimistic_chain() {
    let context = scoped_fixture("reschedule-chain@test.local").await;
    let (load_id, scheduled) =
        scheduled_load(&context, "RESCHEDULE-CHAIN", "2027-09-12T17:00:00Z").await;
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let first_body = body(
        &scheduled.scheduled_for,
        "2027-09-12T19:00:00Z",
        "supplier_change",
    );
    let first = app
        .clone()
        .oneshot(command_request(
            &context.token,
            context.tenant_id,
            load_id,
            "appointment-reschedules",
            "reschedule-chain-1",
            &first_body,
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: RescheduleInboundLoadAppointmentResponse = json_body(first).await;
    let second_body = body(&first.scheduled_for, "2027-09-13T17:00:00Z", "correction");
    let second = app
        .clone()
        .oneshot(command_request(
            &context.token,
            context.tenant_id,
            load_id,
            "appointment-reschedules",
            "reschedule-chain-2",
            &second_body,
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second: RescheduleInboundLoadAppointmentResponse = json_body(second).await;
    assert_eq!(second.sequence, 2);
    assert_eq!(second.previous_scheduled_for, first.scheduled_for);

    let stale = app
        .oneshot(command_request(
            &context.token,
            context.tenant_id,
            load_id,
            "appointment-reschedules",
            "reschedule-chain-stale",
            &body(&scheduled.scheduled_for, "2027-09-14T17:00:00Z", "weather"),
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let mut tx = tenant_tx(&context.fixture.db, context.tenant_id).await;
    let chain: Vec<(i64, String, String)> = sqlx::query_as(
        r#"
        SELECT sequence,previous_scheduled_for::TEXT,scheduled_for::TEXT
        FROM inbound_load_appointment_reschedules
        WHERE load_id=$1 ORDER BY sequence
        "#,
    )
    .bind(load_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].0, 1);
    assert_eq!(chain[1].0, 2);
    assert_eq!(chain[0].2, chain[1].1);
}

#[tokio::test]
async fn reschedule_replays_fail_closed_after_scope_revocation_and_rls_is_minimal() {
    let context = scoped_fixture("reschedule-scope@test.local").await;
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
    let (load_id, scheduled) =
        scheduled_load(&context, "RESCHEDULE-SCOPE", "2027-10-12T17:00:00Z").await;
    let request_body = body(&scheduled.scheduled_for, "2027-10-12T19:00:00Z", "weather");
    let app = routes::app(AppState::new(context.fixture.db.clone()));
    let first = app
        .clone()
        .oneshot(command_request(
            &context.token,
            context.tenant_id,
            load_id,
            "appointment-reschedules",
            "reschedule-scope",
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
        body(
            &scheduled.scheduled_for,
            "2027-10-13T17:00:00Z",
            "correction",
        ),
    ] {
        let hidden = app
            .clone()
            .oneshot(command_request(
                &context.token,
                context.tenant_id,
                load_id,
                "appointment-reschedules",
                "reschedule-scope",
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
          has_table_privilege('wareboxes_app','inbound_load_appointment_reschedules','SELECT'),
          has_table_privilege('wareboxes_app','inbound_load_appointment_reschedules','INSERT'),
          has_table_privilege('wareboxes_app','inbound_load_appointment_reschedules','UPDATE'),
          has_table_privilege('wareboxes_app','inbound_load_appointment_reschedules','DELETE')
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(privileges, (true, true, false, false));
    admin.close().await;

    let outsider = context
        .fixture
        .wms_user("reschedule-outsider@test.local")
        .await;
    let outsider_tenant = tenant_for_user(&context.fixture.db, outsider.id).await;
    let outsider_token = auth::create_session(&context.fixture.db, outsider.id)
        .await
        .unwrap();
    let hidden = app
        .oneshot(command_request(
            &outsider_token,
            outsider_tenant,
            load_id,
            "appointment-reschedules",
            "reschedule-cross-tenant",
            &body(&scheduled.scheduled_for, "2027-10-14T17:00:00Z", "weather"),
        ))
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
}
