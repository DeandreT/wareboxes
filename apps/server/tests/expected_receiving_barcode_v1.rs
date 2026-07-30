mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use common::*;
use tower::ServiceExt;
use wareboxes_api_contract::v1::{ErrorReason, ErrorResponse, ExpectedReceivingSessionResponse};
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_core::models::{LoadStatus, LoadType};
use wareboxes_server::auth::TENANT_ID_HEADER;
use wareboxes_server::{routes, state::AppState};

fn lookup_request(token: Option<&str>, tenant_id: TenantId, barcode: &str) -> Request<Body> {
    let mut request = Request::builder()
        .uri(format!(
            "/api/v1/expected-receiving/loads/by-barcode/{barcode}"
        ))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    request.body(Body::empty()).unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

struct ReadyLoadSetup<'a> {
    tenant_id: TenantId,
    actor_id: i64,
    facility_id: i64,
    inventory_owner_id: i64,
    item_id: i64,
    execution_barcode: &'a str,
    dock_barcode: &'a str,
}

async fn create_ready_load(fixture: &Fixture, setup: ReadyLoadSetup<'_>) -> i64 {
    let dock_id = repo::locations::add_location(
        &fixture.db,
        setup.tenant_id,
        setup.facility_id,
        None,
        Some(setup.dock_barcode),
        Some(setup.dock_barcode),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let load_id = repo::loads::add_load_with_execution_barcode(
        &fixture.db,
        setup.tenant_id,
        setup.actor_id,
        setup.facility_id,
        setup.inventory_owner_id,
        setup.execution_barcode,
        LoadType::Inbound,
        Some(setup.execution_barcode),
        None,
        None,
        None,
        None,
        Some(dock_id),
        None,
        None,
    )
    .await
    .unwrap();
    repo::loads::add_line(
        &fixture.db,
        setup.tenant_id,
        setup.actor_id,
        load_id,
        setup.item_id,
        None,
        5,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let mut tx = tenant_tx(&fixture.db, setup.tenant_id).await;
    sqlx::query("UPDATE loads SET status = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(LoadStatus::Arrived.as_str())
        .bind(setup.tenant_id.get())
        .bind(load_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    load_id
}

async fn tenant_item_with_barcode(
    fixture: &Fixture,
    tenant_id: TenantId,
    name: &str,
    barcode: &str,
) -> i64 {
    let item_id = fixture.item(tenant_id, name, "each").await;
    repo::items::add_barcode(&fixture.db, tenant_id, item_id, barcode, "code128", None)
        .await
        .unwrap();
    item_id
}

#[tokio::test]
async fn execution_barcode_is_canonical_tenant_unique_and_immutable() {
    let fixture = Fixture::new().await;
    let first_user = fixture
        .wms_user("load-barcode-invariant-a@test.local")
        .await;
    let first_tenant = tenant_for_user(&fixture.db, first_user.id).await;
    let first_facility = fixture.facility(first_tenant, "Barcode Invariant A").await;
    let first_owner = fixture
        .inventory_owner(first_tenant, "Barcode Invariant Owner A")
        .await;
    fixture
        .assign_owner_to_facility(first_tenant, first_owner, first_facility)
        .await;

    assert_eq!(
        repo::loads::normalize_execution_barcode("  scan:load-01  ").unwrap(),
        "SCAN:LOAD-01"
    );
    for invalid in [
        "",
        "   ",
        "-BAD-START",
        "BAD SPACE",
        "BAD/SLASH",
        "BAD%ESCAPE",
        "BAD?QUERY",
        "BAD#FRAGMENT",
    ] {
        assert!(
            repo::loads::normalize_execution_barcode(invalid).is_err(),
            "{invalid:?} must be rejected"
        );
    }
    assert!(repo::loads::normalize_execution_barcode(&"A".repeat(201)).is_err());

    let first_load = repo::loads::add_load_with_execution_barcode(
        &fixture.db,
        first_tenant,
        first_user.id,
        first_facility,
        first_owner,
        "  scan:load-01  ",
        LoadType::Inbound,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let stored = repo::loads::get_load(&fixture.db, first_tenant, first_load, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.execution_barcode, "SCAN:LOAD-01");

    assert!(repo::loads::add_load_with_execution_barcode(
        &fixture.db,
        first_tenant,
        first_user.id,
        first_facility,
        first_owner,
        "scan:load-01",
        LoadType::Inbound,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .is_err());

    let second_user = fixture
        .wms_user("load-barcode-invariant-b@test.local")
        .await;
    let second_tenant = tenant_for_user(&fixture.db, second_user.id).await;
    let second_facility = fixture.facility(second_tenant, "Barcode Invariant B").await;
    let second_owner = fixture
        .inventory_owner(second_tenant, "Barcode Invariant Owner B")
        .await;
    fixture
        .assign_owner_to_facility(second_tenant, second_owner, second_facility)
        .await;
    assert!(repo::loads::add_load_with_execution_barcode(
        &fixture.db,
        second_tenant,
        second_user.id,
        second_facility,
        second_owner,
        "SCAN:LOAD-01",
        LoadType::Inbound,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .is_ok());

    let admin = admin_db_for(&fixture.db).await;
    assert!(sqlx::query(
        "UPDATE loads SET execution_barcode = 'SCAN:LOAD-02' WHERE tenant_id = $1 AND id = $2",
    )
    .bind(first_tenant.get())
    .bind(first_load)
    .execute(&admin)
    .await
    .is_err());
    assert!(sqlx::query(
        "UPDATE loads SET execution_barcode = execution_barcode WHERE tenant_id = $1 AND id = $2",
    )
    .bind(first_tenant.get())
    .bind(first_load)
    .execute(&admin)
    .await
    .is_ok());
    admin.close().await;
}

#[tokio::test]
async fn barcode_lookup_is_authenticated_normalized_and_scope_indistinguishable() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("load-barcode-lookup@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    let allowed_facility = fixture.facility(tenant_id, "Barcode Allowed DC").await;
    let denied_facility = fixture.facility(tenant_id, "Barcode Denied DC").await;
    let allowed_owner = fixture
        .inventory_owner(tenant_id, "Barcode Allowed Owner")
        .await;
    let denied_owner = fixture
        .inventory_owner(tenant_id, "Barcode Denied Owner")
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
    let item_id =
        tenant_item_with_barcode(&fixture, tenant_id, "Barcode Lookup Item", "LOOKUP-ITEM").await;
    let allowed_load = create_ready_load(
        &fixture,
        ReadyLoadSetup {
            tenant_id,
            actor_id: operator.id,
            facility_id: allowed_facility,
            inventory_owner_id: allowed_owner,
            item_id,
            execution_barcode: "SCAN:ALLOWED",
            dock_barcode: "SCAN-ALLOWED-DOCK",
        },
    )
    .await;
    create_ready_load(
        &fixture,
        ReadyLoadSetup {
            tenant_id,
            actor_id: operator.id,
            facility_id: denied_facility,
            inventory_owner_id: allowed_owner,
            item_id,
            execution_barcode: "SCAN:DENIED-FACILITY",
            dock_barcode: "SCAN-DENIED-FACILITY-DOCK",
        },
    )
    .await;
    create_ready_load(
        &fixture,
        ReadyLoadSetup {
            tenant_id,
            actor_id: operator.id,
            facility_id: allowed_facility,
            inventory_owner_id: denied_owner,
            item_id,
            execution_barcode: "SCAN:DENIED-OWNER",
            dock_barcode: "SCAN-DENIED-OWNER-DOCK",
        },
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

    let other_user = fixture
        .wms_user("load-barcode-other-tenant@test.local")
        .await;
    let other_tenant = tenant_for_user(&fixture.db, other_user.id).await;
    let other_facility = fixture.facility(other_tenant, "Barcode Other DC").await;
    let other_owner = fixture
        .inventory_owner(other_tenant, "Barcode Other Owner")
        .await;
    fixture
        .assign_owner_to_facility(other_tenant, other_owner, other_facility)
        .await;
    let other_item = tenant_item_with_barcode(
        &fixture,
        other_tenant,
        "Barcode Other Item",
        "OTHER-LOOKUP-ITEM",
    )
    .await;
    create_ready_load(
        &fixture,
        ReadyLoadSetup {
            tenant_id: other_tenant,
            actor_id: other_user.id,
            facility_id: other_facility,
            inventory_owner_id: other_owner,
            item_id: other_item,
            execution_barcode: "SCAN:OTHER-TENANT",
            dock_barcode: "SCAN-OTHER-DOCK",
        },
    )
    .await;

    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let found = app
        .clone()
        .oneshot(lookup_request(Some(&token), tenant_id, "scan:allowed"))
        .await
        .unwrap();
    assert_eq!(found.status(), StatusCode::OK);
    assert_eq!(
        response_json::<ExpectedReceivingSessionResponse>(found)
            .await
            .load_id,
        allowed_load
    );

    let mut indistinguishable = Vec::new();
    for barcode in [
        "SCAN:DENIED-FACILITY",
        "SCAN:DENIED-OWNER",
        "SCAN:OTHER-TENANT",
        "SCAN:UNKNOWN",
    ] {
        let response = app
            .clone()
            .oneshot(lookup_request(Some(&token), tenant_id, barcode))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let error = response_json::<ErrorResponse>(response).await;
        assert_eq!(error.reason, ErrorReason::NotFound);
        indistinguishable.push((error.reason, error.message, error.violations));
    }
    assert!(indistinguishable.windows(2).all(|pair| pair[0] == pair[1]));

    let no_permission = fixture.user("load-barcode-no-permission@test.local").await;
    let no_permission_tenant = tenant_for_user(&fixture.db, no_permission.id).await;
    let no_permission_facility = fixture
        .facility(no_permission_tenant, "Barcode No Permission DC")
        .await;
    let no_permission_owner = fixture
        .inventory_owner(no_permission_tenant, "Barcode No Permission Owner")
        .await;
    fixture
        .assign_owner_to_facility(
            no_permission_tenant,
            no_permission_owner,
            no_permission_facility,
        )
        .await;
    let no_permission_item = tenant_item_with_barcode(
        &fixture,
        no_permission_tenant,
        "Barcode No Permission Item",
        "NO-PERMISSION-ITEM",
    )
    .await;
    create_ready_load(
        &fixture,
        ReadyLoadSetup {
            tenant_id: no_permission_tenant,
            actor_id: no_permission.id,
            facility_id: no_permission_facility,
            inventory_owner_id: no_permission_owner,
            item_id: no_permission_item,
            execution_barcode: "SCAN:NO-PERMISSION",
            dock_barcode: "SCAN-NO-PERMISSION-DOCK",
        },
    )
    .await;
    let no_permission_token = auth::create_session(&fixture.db, no_permission.id)
        .await
        .unwrap();
    let denied = app
        .clone()
        .oneshot(lookup_request(
            Some(&no_permission_token),
            no_permission_tenant,
            "SCAN:NO-PERMISSION",
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response_json::<ErrorResponse>(denied).await.reason,
        ErrorReason::NotFound
    );

    let invalid = app
        .clone()
        .oneshot(lookup_request(Some(&token), tenant_id, "BAD%25BARCODE"))
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json::<ErrorResponse>(invalid).await.reason,
        ErrorReason::InvalidRequest
    );

    let unauthenticated = app
        .oneshot(lookup_request(None, tenant_id, "SCAN:ALLOWED"))
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
}
