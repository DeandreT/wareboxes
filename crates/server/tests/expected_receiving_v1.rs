mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use common::*;
use tower::ServiceExt;
use wareboxes_api_contract::v1::{
    ErrorReason, ErrorResponse, ExpectedReceivingLoadStatus, ExpectedReceivingSessionResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_server::auth::TENANT_ID_HEADER;
use wareboxes_server::{routes, state::AppState};

fn request(token: &str, tenant_id: TenantId, load_id: i64) -> Request<Body> {
    Request::builder()
        .uri(format!("/api/v1/expected-receiving/loads/{load_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .body(Body::empty())
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn assert_error(response: axum::response::Response, status: StatusCode, reason: ErrorReason) {
    assert_eq!(response.status(), status);
    assert_eq!(
        response_json::<ErrorResponse>(response).await.reason,
        reason
    );
}

async fn receiving_dock(
    fixture: &Fixture,
    tenant_id: TenantId,
    facility_id: i64,
    barcode: Option<&str>,
    name: &str,
) -> i64 {
    repo::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        barcode,
        Some(name),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
async fn expected_load(
    fixture: &Fixture,
    tenant_id: TenantId,
    actor_id: i64,
    facility_id: i64,
    inventory_owner_id: i64,
    item_id: Option<i64>,
    dock_id: Option<i64>,
    status: LoadStatus,
    reference: &str,
) -> (i64, Option<i64>) {
    let load_id = repo::loads::add_load(
        &fixture.db,
        tenant_id,
        actor_id,
        facility_id,
        inventory_owner_id,
        LoadType::Inbound,
        Some(reference),
        None,
        None,
        None,
        None,
        dock_id,
        None,
        None,
    )
    .await
    .unwrap();
    let line_id = match item_id {
        Some(item_id) => Some(
            repo::loads::add_line(
                &fixture.db,
                tenant_id,
                actor_id,
                load_id,
                item_id,
                None,
                10,
                Some("LOT-READ"),
                None,
                None,
            )
            .await
            .unwrap(),
        ),
        None => None,
    };
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("UPDATE loads SET status = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(status.as_str())
        .bind(tenant_id.get())
        .bind(load_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    (load_id, line_id)
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct ReadEffects {
    load_status: String,
    received_quantity: i64,
    rejected_quantity: i64,
    missing_quantity: i64,
    line_status: String,
    load_activity_count: i64,
    command_count: i64,
    work_task_count: i64,
}

async fn read_effects(
    fixture: &Fixture,
    tenant_id: TenantId,
    load_id: i64,
    load_line_id: i64,
) -> ReadEffects {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let effects = sqlx::query_as(
        r#"
        SELECT load.status AS load_status,
               line.received_qty AS received_quantity,
               line.rejected_qty AS rejected_quantity,
               line.missing_qty AS missing_quantity,
               line.status AS line_status,
               (
                   SELECT COUNT(*)
                   FROM load_activity activity
                   WHERE activity.tenant_id = load.tenant_id
                     AND activity.load_id = load.id
               ) AS load_activity_count,
               (
                   SELECT COUNT(*)
                   FROM command_idempotency_records command
                   WHERE command.tenant_id = load.tenant_id
               ) AS command_count,
               (
                   SELECT COUNT(*)
                   FROM work_tasks task
                   WHERE task.tenant_id = load.tenant_id
               ) AS work_task_count
        FROM loads load
        INNER JOIN load_lines line
          ON line.tenant_id = load.tenant_id
         AND line.load_id = load.id
         AND line.id = $3
        WHERE load.tenant_id = $1
          AND load.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(load_id)
    .bind(load_line_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    effects
}

#[tokio::test]
async fn expected_receiving_session_returns_cumulative_read_only_projection() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("expected-receiving-read@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let facility_id = fixture
        .facility(tenant_id, "Expected Receiving Read DC")
        .await;
    let inventory_owner_id = fixture
        .inventory_owner(tenant_id, "Expected Receiving Read Owner")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let dock_id = receiving_dock(
        &fixture,
        tenant_id,
        facility_id,
        Some("READ-DOCK-01"),
        "Read Dock 1",
    )
    .await;
    let item_id = fixture
        .item(tenant_id, "Expected Receiving Read Item", "case")
        .await;
    repo::items::add_barcode(
        &fixture.db,
        tenant_id,
        item_id,
        "READ-ITEM-B",
        "code128",
        None,
    )
    .await
    .unwrap();
    repo::items::add_barcode(
        &fixture.db,
        tenant_id,
        item_id,
        "READ-ITEM-A",
        "code128",
        None,
    )
    .await
    .unwrap();
    let (load_id, load_line_id) = expected_load(
        &fixture,
        tenant_id,
        operator.id,
        facility_id,
        inventory_owner_id,
        Some(item_id),
        Some(dock_id),
        LoadStatus::Receiving,
        "ASN-READ-01",
    )
    .await;
    let load_line_id = load_line_id.unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        r#"
        UPDATE load_lines
        SET received_qty = 3,
            rejected_qty = 1,
            missing_qty = 2,
            missing_confirmed_by = $1,
            missing_confirmed_at = $2,
            status = 'partial'
        WHERE tenant_id = $3
          AND id = $4
        "#,
    )
    .bind(operator.id)
    .bind(db::now_iso())
    .bind(tenant_id.get())
    .bind(load_line_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let before = read_effects(&fixture, tenant_id, load_id, load_line_id).await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let first = app
        .clone()
        .oneshot(request(&token, tenant_id, load_id))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let session: ExpectedReceivingSessionResponse = response_json(first).await;

    assert_eq!(session.load_id, load_id);
    assert_eq!(session.inventory_owner_id, inventory_owner_id);
    assert_eq!(session.facility_id, facility_id);
    assert_eq!(session.reference_number.as_deref(), Some("ASN-READ-01"));
    assert_eq!(session.status, ExpectedReceivingLoadStatus::Receiving);
    assert_eq!(session.receiving_location.location_id, dock_id);
    assert_eq!(session.receiving_location.barcode, "READ-DOCK-01");
    assert_eq!(
        session.receiving_location.name.as_deref(),
        Some("Read Dock 1")
    );
    assert_eq!(session.lines.len(), 1);
    let line = &session.lines[0];
    assert_eq!(line.load_line_id, load_line_id);
    assert_eq!(line.item_id, item_id);
    assert_eq!(
        line.item_description.as_deref(),
        Some("Expected Receiving Read Item")
    );
    assert_eq!(line.uom, "case");
    assert_eq!(line.item_barcodes, ["READ-ITEM-A", "READ-ITEM-B"]);
    assert_eq!(line.expected_quantity, 10);
    assert_eq!(line.received_quantity, 3);
    assert_eq!(line.rejected_quantity, 1);
    assert_eq!(line.missing_quantity, 2);
    assert_eq!(line.remaining_quantity, 4);
    assert_eq!(line.lot.as_deref(), Some("LOT-READ"));

    let second = app
        .oneshot(request(&token, tenant_id, load_id))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(
        response_json::<ExpectedReceivingSessionResponse>(second).await,
        session
    );
    assert_eq!(
        read_effects(&fixture, tenant_id, load_id, load_line_id).await,
        before
    );
}

#[tokio::test]
async fn expected_receiving_session_fails_closed_for_tenant_owner_and_facility_scope() {
    let fixture = Fixture::new().await;
    let operator = fixture
        .wms_user("expected-receiving-scope@test.local")
        .await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let allowed_facility = fixture.facility(tenant_id, "Allowed Receiving DC").await;
    let denied_facility = fixture.facility(tenant_id, "Denied Receiving DC").await;
    let allowed_owner = fixture
        .inventory_owner(tenant_id, "Allowed Receiving Owner")
        .await;
    let denied_owner = fixture
        .inventory_owner(tenant_id, "Denied Receiving Owner")
        .await;
    for (owner_id, facility_id) in [
        (allowed_owner, allowed_facility),
        (allowed_owner, denied_facility),
        (denied_owner, allowed_facility),
    ] {
        fixture
            .assign_owner_to_facility(tenant_id, owner_id, facility_id)
            .await;
    }
    let allowed_dock = receiving_dock(
        &fixture,
        tenant_id,
        allowed_facility,
        Some("SCOPE-ALLOWED-DOCK"),
        "Allowed Dock",
    )
    .await;
    let denied_dock = receiving_dock(
        &fixture,
        tenant_id,
        denied_facility,
        Some("SCOPE-DENIED-DOCK"),
        "Denied Dock",
    )
    .await;
    let item_id = fixture
        .item(tenant_id, "Scoped Expected Receiving Item", "each")
        .await;
    repo::items::add_barcode(
        &fixture.db,
        tenant_id,
        item_id,
        "SCOPE-ITEM",
        "code128",
        None,
    )
    .await
    .unwrap();
    let (allowed_load, _) = expected_load(
        &fixture,
        tenant_id,
        operator.id,
        allowed_facility,
        allowed_owner,
        Some(item_id),
        Some(allowed_dock),
        LoadStatus::Arrived,
        "SCOPE-ALLOWED",
    )
    .await;
    let (denied_facility_load, _) = expected_load(
        &fixture,
        tenant_id,
        operator.id,
        denied_facility,
        allowed_owner,
        Some(item_id),
        Some(denied_dock),
        LoadStatus::Arrived,
        "SCOPE-DENIED-FACILITY",
    )
    .await;
    let (denied_owner_load, _) = expected_load(
        &fixture,
        tenant_id,
        operator.id,
        allowed_facility,
        denied_owner,
        Some(item_id),
        Some(allowed_dock),
        LoadStatus::Arrived,
        "SCOPE-DENIED-OWNER",
    )
    .await;

    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: operator.id,
            all_facilities: false,
            facility_ids: vec![allowed_facility],
            all_inventory_owners: false,
            inventory_owner_ids: vec![allowed_owner],
        },
    )
    .await
    .unwrap());
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let allowed = app
        .clone()
        .oneshot(request(&token, tenant_id, allowed_load))
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
    for denied_load in [denied_facility_load, denied_owner_load] {
        assert_error(
            app.clone()
                .oneshot(request(&token, tenant_id, denied_load))
                .await
                .unwrap(),
            StatusCode::NOT_FOUND,
            ErrorReason::NotFound,
        )
        .await;
    }

    let other_user = fixture
        .wms_user("expected-receiving-other-tenant@test.local")
        .await;
    let other_tenant_id = tenant_for_user(&fixture.db, other_user.id).await;
    let other_facility = fixture
        .facility(other_tenant_id, "Other Tenant Receiving DC")
        .await;
    let other_owner = fixture
        .inventory_owner(other_tenant_id, "Other Tenant Receiving Owner")
        .await;
    fixture
        .assign_owner_to_facility(other_tenant_id, other_owner, other_facility)
        .await;
    let other_dock = receiving_dock(
        &fixture,
        other_tenant_id,
        other_facility,
        Some("OTHER-TENANT-DOCK"),
        "Other Tenant Dock",
    )
    .await;
    let other_item = fixture
        .item(other_tenant_id, "Other Tenant Item", "each")
        .await;
    repo::items::add_barcode(
        &fixture.db,
        other_tenant_id,
        other_item,
        "OTHER-TENANT-ITEM",
        "code128",
        None,
    )
    .await
    .unwrap();
    let (other_load, _) = expected_load(
        &fixture,
        other_tenant_id,
        other_user.id,
        other_facility,
        other_owner,
        Some(other_item),
        Some(other_dock),
        LoadStatus::Arrived,
        "OTHER-TENANT-LOAD",
    )
    .await;
    assert_error(
        app.clone()
            .oneshot(request(&token, tenant_id, other_load))
            .await
            .unwrap(),
        StatusCode::NOT_FOUND,
        ErrorReason::NotFound,
    )
    .await;
    assert_error(
        app.oneshot(request(&token, tenant_id, 0)).await.unwrap(),
        StatusCode::BAD_REQUEST,
        ErrorReason::InvalidRequest,
    )
    .await;
}

#[tokio::test]
async fn expected_receiving_session_rejects_unready_loads() {
    let fixture = Fixture::new().await;
    let operator = fixture
        .wms_user("expected-receiving-readiness@test.local")
        .await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let facility_id = fixture
        .facility(tenant_id, "Expected Receiving Readiness DC")
        .await;
    let inventory_owner_id = fixture
        .inventory_owner(tenant_id, "Expected Receiving Readiness Owner")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let dock_id = receiving_dock(
        &fixture,
        tenant_id,
        facility_id,
        Some("READINESS-DOCK"),
        "Readiness Dock",
    )
    .await;
    let unscannable_dock_id =
        receiving_dock(&fixture, tenant_id, facility_id, None, "Unscannable Dock").await;
    let item_id = fixture
        .item(tenant_id, "Ready Expected Receiving Item", "case")
        .await;
    repo::items::add_barcode(
        &fixture.db,
        tenant_id,
        item_id,
        "READINESS-ITEM",
        "code128",
        None,
    )
    .await
    .unwrap();
    let unscannable_item_id = fixture
        .item(tenant_id, "Unscannable Expected Receiving Item", "case")
        .await;

    let mut rejected_loads = Vec::new();
    for (dock, item, reference) in [
        (None, Some(item_id), "READINESS-NO-DOCK"),
        (
            Some(unscannable_dock_id),
            Some(item_id),
            "READINESS-UNSCANNABLE-DOCK",
        ),
        (
            Some(dock_id),
            Some(unscannable_item_id),
            "READINESS-UNSCANNABLE-ITEM",
        ),
        (Some(dock_id), None, "READINESS-NO-OPEN-LINES"),
    ] {
        rejected_loads.push(
            expected_load(
                &fixture,
                tenant_id,
                operator.id,
                facility_id,
                inventory_owner_id,
                item,
                dock,
                LoadStatus::Arrived,
                reference,
            )
            .await
            .0,
        );
    }

    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    for load_id in rejected_loads {
        assert_error(
            app.clone()
                .oneshot(request(&token, tenant_id, load_id))
                .await
                .unwrap(),
            StatusCode::CONFLICT,
            ErrorReason::Conflict,
        )
        .await;
    }
}
