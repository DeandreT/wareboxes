mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    ErrorReason, ErrorResponse, InventoryHoldPage, InventoryHoldReason, InventoryHoldStatus,
    PlaceInventoryHoldRequest, PlaceInventoryHoldResponse, ReleaseInventoryHoldRequest,
    ReleaseInventoryHoldResponse, IDEMPOTENCY_KEY_HEADER,
};
use wareboxes_application::inventory::{
    InventoryHoldPageFilter, InventoryHoldReason as ApplicationInventoryHoldReason,
    InventoryHoldStatus as ApplicationInventoryHoldStatus,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InventoryHoldReason as CoreHoldReason, TenantAccess};
use wareboxes_domain::{FacilityId, InventoryOwnerId, OwnerScope, SiteScope};

fn command_context(access: &TenantAccess, key: &str) -> CommandContext {
    CommandContext {
        tenant_id: access.tenant_id,
        actor_id: access.user_id,
        request_id: format!("request-{key}"),
        idempotency_key: Some(key.to_owned()),
    }
}

fn api_request<T: serde::Serialize>(
    token: &str,
    tenant_id: TenantId,
    method: Method,
    uri: &str,
    idempotency_key: Option<&str>,
    body: Option<&T>,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    if let Some(key) = idempotency_key {
        request = request.header(IDEMPOTENCY_KEY_HEADER, key);
    }
    let body = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(body).unwrap())
        }
        None => Body::empty(),
    };
    request.body(body).unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn place_hold(
    fixture: &Fixture,
    access: &TenantAccess,
    balance_id: i64,
    quantity: i64,
    key: &str,
) -> i64 {
    repo::inventory::place_inventory_hold(
        &fixture.db,
        access,
        &command_context(access, key),
        &repo::inventory::PlaceInventoryHoldCommand {
            inventory_balance_id: balance_id,
            qty: quantity,
            reason: CoreHoldReason::QualityInspection,
            note: Some("Awaiting quality inspection"),
            reference_type: Some("receipt"),
            reference_id: Some(41),
        },
    )
    .await
    .unwrap()
    .hold_id
}

#[tokio::test]
async fn hold_repository_page_is_newest_first_scoped_and_display_ready() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("v1-hold-repository@test.local").await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let tenant_id = access.tenant_id;

    let allowed_owner = fixture
        .inventory_owner(tenant_id, "V1 Hold Allowed Owner")
        .await;
    let denied_owner = fixture
        .inventory_owner(tenant_id, "V1 Hold Denied Owner")
        .await;
    let allowed_facility = fixture.facility(tenant_id, "V1 Hold Allowed DC").await;
    let denied_facility = fixture.facility(tenant_id, "V1 Hold Denied DC").await;
    fixture
        .assign_owner_to_facility(tenant_id, allowed_owner, allowed_facility)
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, denied_owner, denied_facility)
        .await;
    let allowed_item = fixture
        .item(tenant_id, "V1 Hold Allowed Item", "case")
        .await;
    let denied_item = fixture.item(tenant_id, "V1 Hold Denied Item", "each").await;
    let allowed_balance = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: allowed_owner,
                facility_id: allowed_facility,
                item_id: allowed_item,
                qty: 20,
                key: "V1-HOLD-ALLOWED",
            },
        )
        .await;
    let denied_balance = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: denied_owner,
                facility_id: denied_facility,
                item_id: denied_item,
                qty: 10,
                key: "V1-HOLD-DENIED",
            },
        )
        .await;
    let released_hold = place_hold(
        &fixture,
        &access,
        allowed_balance.balance_id,
        2,
        "v1-hold-released",
    )
    .await;
    repo::inventory::release_inventory_hold(
        &fixture.db,
        &access,
        &command_context(&access, "v1-hold-released-release"),
        &repo::inventory::ReleaseInventoryHoldCommand {
            hold_id: released_hold,
        },
    )
    .await
    .unwrap();
    let first_active = place_hold(
        &fixture,
        &access,
        allowed_balance.balance_id,
        3,
        "v1-hold-first-active",
    )
    .await;
    let newest_active = place_hold(
        &fixture,
        &access,
        allowed_balance.balance_id,
        4,
        "v1-hold-newest-active",
    )
    .await;
    let denied_hold = place_hold(
        &fixture,
        &access,
        denied_balance.balance_id,
        2,
        "v1-hold-denied",
    )
    .await;

    let mut restricted = access.clone();
    restricted.site_scope = SiteScope {
        all_facilities: false,
        facility_ids: vec![FacilityId::new(allowed_facility).unwrap()],
    };
    restricted.owner_scope = OwnerScope {
        all_inventory_owners: false,
        inventory_owner_ids: vec![InventoryOwnerId::new(allowed_owner).unwrap()],
    };

    let first_page = wareboxes_persistence_postgres::inventory_holds::get_inventory_hold_page(
        &fixture.db,
        restricted.tenant_id,
        &restricted.site_scope,
        &restricted.owner_scope,
        InventoryHoldPageFilter {
            before_id: None,
            limit: 1,
            status: Some(ApplicationInventoryHoldStatus::Active),
        },
    )
    .await
    .unwrap();
    assert_eq!(first_page.items.len(), 1);
    assert_eq!(first_page.items[0].id, newest_active);
    assert_eq!(first_page.next_before_id, Some(newest_active));
    assert_eq!(
        first_page.items[0].inventory_owner_name,
        "V1 Hold Allowed Owner"
    );
    assert_eq!(
        first_page.items[0].facility_name.as_deref(),
        Some("V1 Hold Allowed DC")
    );
    assert_eq!(
        first_page.items[0].location_barcode.as_deref(),
        Some("V1-HOLD-ALLOWED")
    );
    assert_eq!(
        first_page.items[0].item_description.as_deref(),
        Some("V1 Hold Allowed Item")
    );
    assert_eq!(first_page.items[0].license_plate_barcode, None);
    assert_eq!(
        first_page.items[0].reason,
        ApplicationInventoryHoldReason::QualityInspection
    );
    assert_eq!(
        first_page.items[0].status,
        ApplicationInventoryHoldStatus::Active
    );
    assert_eq!(first_page.items[0].quantity.get(), 4);

    let second_page = wareboxes_persistence_postgres::inventory_holds::get_inventory_hold_page(
        &fixture.db,
        restricted.tenant_id,
        &restricted.site_scope,
        &restricted.owner_scope,
        InventoryHoldPageFilter {
            before_id: first_page.next_before_id,
            limit: 1,
            status: Some(ApplicationInventoryHoldStatus::Active),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        second_page
            .items
            .iter()
            .map(|hold| hold.id)
            .collect::<Vec<_>>(),
        vec![first_active]
    );
    assert_eq!(second_page.next_before_id, None);
    assert!(!first_page
        .items
        .iter()
        .chain(&second_page.items)
        .any(|hold| hold.id == denied_hold));

    let released = wareboxes_persistence_postgres::inventory_holds::get_inventory_hold_page(
        &fixture.db,
        restricted.tenant_id,
        &restricted.site_scope,
        &restricted.owner_scope,
        InventoryHoldPageFilter {
            before_id: None,
            limit: 10,
            status: Some(ApplicationInventoryHoldStatus::Released),
        },
    )
    .await
    .unwrap();
    assert_eq!(released.items.len(), 1);
    assert_eq!(released.items[0].id, released_hold);
    assert!(released.items[0].released_at.is_some());
    assert_eq!(released.items[0].released_by_user_id, Some(user.id));
}

#[tokio::test]
async fn hold_v1_http_contract_places_pages_replays_and_releases() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("v1-hold-http@test.local").await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let owner = fixture
        .inventory_owner(access.tenant_id, "V1 Hold HTTP Owner")
        .await;
    let facility = fixture
        .facility(access.tenant_id, "V1 Hold HTTP Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner, facility)
        .await;
    let item = fixture
        .item(access.tenant_id, "V1 Hold HTTP Item", "each")
        .await;
    let balance = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner,
                facility_id: facility,
                item_id: item,
                qty: 10,
                key: "V1-HOLD-HTTP",
            },
        )
        .await;
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let place = PlaceInventoryHoldRequest {
        inventory_balance_id: balance.balance_id,
        quantity: 4,
        reason: InventoryHoldReason::InventoryDiscrepancy,
        note: Some("Count requires review".into()),
        reference_type: Some("cycle_count".into()),
        reference_id: Some(77),
    };

    let missing_key = app
        .clone()
        .oneshot(api_request(
            &token,
            access.tenant_id,
            Method::POST,
            "/api/v1/inventory/holds",
            None,
            Some(&place),
        ))
        .await
        .unwrap();
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json::<ErrorResponse>(missing_key).await.reason,
        ErrorReason::IdempotencyKeyRequired
    );

    let placed_response = app
        .clone()
        .oneshot(api_request(
            &token,
            access.tenant_id,
            Method::POST,
            "/api/v1/inventory/holds",
            Some("v1-hold-http-place"),
            Some(&place),
        ))
        .await
        .unwrap();
    assert_eq!(placed_response.status(), StatusCode::OK);
    let placed: PlaceInventoryHoldResponse = response_json(placed_response).await;

    let replay_response = app
        .clone()
        .oneshot(api_request(
            &token,
            access.tenant_id,
            Method::POST,
            "/api/v1/inventory/holds",
            Some("v1-hold-http-place"),
            Some(&place),
        ))
        .await
        .unwrap();
    assert_eq!(replay_response.status(), StatusCode::OK);
    assert_eq!(
        response_json::<PlaceInventoryHoldResponse>(replay_response).await,
        placed
    );

    let changed = PlaceInventoryHoldRequest {
        quantity: 3,
        ..place.clone()
    };
    let changed_response = app
        .clone()
        .oneshot(api_request(
            &token,
            access.tenant_id,
            Method::POST,
            "/api/v1/inventory/holds",
            Some("v1-hold-http-place"),
            Some(&changed),
        ))
        .await
        .unwrap();
    assert_eq!(changed_response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed_response)
            .await
            .reason,
        ErrorReason::IdempotencyKeyReused
    );

    let active = app
        .clone()
        .oneshot(api_request::<()>(
            &token,
            access.tenant_id,
            Method::GET,
            "/api/v1/inventory/holds?limit=1&status=active",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(active.status(), StatusCode::OK);
    let active: InventoryHoldPage = response_json(active).await;
    assert_eq!(active.items.len(), 1);
    assert_eq!(active.items[0].id, placed.hold_id);
    assert_eq!(active.items[0].quantity, 4);
    assert_eq!(
        active.items[0].reason,
        InventoryHoldReason::InventoryDiscrepancy
    );
    assert_eq!(active.items[0].inventory_owner_name, "V1 Hold HTTP Owner");

    let invalid_cursor = app
        .clone()
        .oneshot(api_request::<()>(
            &token,
            access.tenant_id,
            Method::GET,
            "/api/v1/inventory/holds?cursor=not-a-hold-cursor",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(invalid_cursor.status(), StatusCode::BAD_REQUEST);
    let invalid_cursor: ErrorResponse = response_json(invalid_cursor).await;
    assert_eq!(invalid_cursor.reason, ErrorReason::InvalidCursor);
    assert_eq!(invalid_cursor.message, "invalid inventory hold cursor");

    let release = ReleaseInventoryHoldRequest::default();
    let release_uri = format!("/api/v1/inventory/holds/{}/releases", placed.hold_id);
    let released_response = app
        .clone()
        .oneshot(api_request(
            &token,
            access.tenant_id,
            Method::POST,
            &release_uri,
            Some("v1-hold-http-release"),
            Some(&release),
        ))
        .await
        .unwrap();
    assert_eq!(released_response.status(), StatusCode::OK);
    let released: ReleaseInventoryHoldResponse = response_json(released_response).await;
    assert_eq!(released.hold_id, placed.hold_id);
    assert_eq!(released.released_quantity, 4);

    let release_replay = app
        .clone()
        .oneshot(api_request(
            &token,
            access.tenant_id,
            Method::POST,
            &release_uri,
            Some("v1-hold-http-release"),
            Some(&release),
        ))
        .await
        .unwrap();
    assert_eq!(release_replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<ReleaseInventoryHoldResponse>(release_replay).await,
        released
    );

    let released_page = app
        .clone()
        .oneshot(api_request::<()>(
            &token,
            access.tenant_id,
            Method::GET,
            "/api/v1/inventory/holds?status=released",
            None,
            None,
        ))
        .await
        .unwrap();
    let released_page: InventoryHoldPage = response_json(released_page).await;
    assert_eq!(released_page.items.len(), 1);
    assert_eq!(released_page.items[0].status, InventoryHoldStatus::Released);
    assert!(released_page.items[0].released_at.is_some());

    let unprivileged = fixture.user("v1-hold-unprivileged@test.local").await;
    let unprivileged_token = auth::create_session(&fixture.db, unprivileged.id)
        .await
        .unwrap();
    let forbidden = app
        .oneshot(api_request::<()>(
            &unprivileged_token,
            access.tenant_id,
            Method::GET,
            "/api/v1/inventory/holds",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
}
