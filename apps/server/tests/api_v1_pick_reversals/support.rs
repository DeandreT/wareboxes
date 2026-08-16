use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::{IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use wareboxes_api_contract::v1::{
    AllocationPolicyReference, PickClaimResponse, PickContentConfirmationResponse,
    PlanOrderAllocationRequest, PlanOrderAllocationResponse, ReleaseOrderResponse, Revision,
};
use wareboxes_core::dto::UpdateUserAccessScope;

use super::common::*;

pub async fn send(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    method: Method,
    path: &str,
    idempotency_key: Option<&str>,
    body: Option<Value>,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    if let Some(key) = idempotency_key {
        request = request
            .header(IDEMPOTENCY_KEY_HEADER, key)
            .header(REQUEST_ID_HEADER, format!("request-{key}"));
    }
    let body = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    app.clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}

pub async fn response_json<T: serde::de::DeserializeOwned>(
    response: axum::response::Response,
) -> T {
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

pub async fn expect_status(
    response: axum::response::Response,
    expected: StatusCode,
    operation: &str,
) -> axum::response::Response {
    if response.status() != expected {
        let actual = response.status();
        let body = response_json::<Value>(response).await;
        panic!("{operation}: expected {expected}, got {actual}: {body}");
    }
    response
}

pub async fn grant_permission(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    permission_name: &str,
    role_name: &str,
) {
    let permission_id = match wareboxes_persistence_postgres::permissions::find_by_name(
        &fixture.db,
        tenant_id,
        permission_name,
    )
    .await
    .unwrap()
    {
        Some(permission) => permission.id,
        None => wareboxes_persistence_postgres::permissions::add_permission(
            &fixture.db,
            tenant_id,
            permission_name,
            Some("Pick reversal test permission"),
        )
        .await
        .unwrap(),
    };
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        role_name,
        Some("Pick reversal test role"),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        permission_id,
    )
    .await
    .unwrap());
    assert!(wareboxes_persistence_postgres::roles::add_role_to_user(
        &fixture.db,
        tenant_id,
        user_id,
        role,
    )
    .await
    .unwrap());
}

pub async fn set_scope(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    facility_ids: Vec<i64>,
    owner_ids: Vec<i64>,
) {
    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id,
            all_facilities: false,
            facility_ids,
            all_inventory_owners: false,
            inventory_owner_ids: owner_ids,
        },
    )
    .await
    .unwrap());
}

pub async fn execution_location(
    fixture: &Fixture,
    tenant_id: TenantId,
    facility_id: i64,
    barcode: &str,
) -> i64 {
    wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some(barcode),
        Some(barcode),
        "packing",
        true,
        false,
        false,
    )
    .await
    .unwrap()
}

pub async fn plate_at(
    fixture: &Fixture,
    tenant_id: TenantId,
    owner_id: i64,
    facility_id: i64,
    location_id: i64,
    barcode: &str,
) -> i64 {
    let plate_id = repo::license_plates::add_license_plate(
        &fixture.db,
        tenant_id,
        owner_id,
        facility_id,
        Some(barcode),
    )
    .await
    .unwrap();
    let admin = admin_db_for(&fixture.db).await;
    sqlx::query("UPDATE license_plates SET location_id = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(location_id)
        .bind(tenant_id.get())
        .bind(plate_id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    plate_id
}

#[derive(Debug)]
pub struct PickedFixture {
    pub order_id: i64,
    pub facility_id: i64,
    pub execution_location_id: i64,
    pub execution_location_barcode: String,
    pub tote_barcode: String,
    pub source_location_barcode: String,
    pub item_barcode: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub task_id: i64,
    pub confirmation: PickContentConfirmationResponse,
}

pub async fn completed_pick(
    fixture: &Fixture,
    app: &axum::Router,
    token: &str,
    access: &wareboxes_core::models::TenantAccess,
    key: &str,
) -> PickedFixture {
    let owner_id = fixture
        .inventory_owner(access.tenant_id, &format!("{key} Owner"))
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, &format!("{key} Facility"))
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let execution_location_barcode = format!("{key}-PACK");
    let execution_location_id = execution_location(
        fixture,
        access.tenant_id,
        facility_id,
        &execution_location_barcode,
    )
    .await;
    let tote_barcode = format!("{key}-TOTE");
    plate_at(
        fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        execution_location_id,
        &tote_barcode,
    )
    .await;

    let order_id = fixture.order_header(access.tenant_id, key, owner_id).await;
    let item_id = fixture
        .item(access.tenant_id, "Reversible item", "each")
        .await;
    let item_barcode = format!("{key}-ITEM");
    repo::items::add_barcode(
        &fixture.db,
        access.tenant_id,
        item_id,
        &item_barcode,
        "code128",
        None,
    )
    .await
    .unwrap();
    fixture
        .order_item(access.tenant_id, order_id, item_id, 4)
        .await;
    let source_location_barcode = format!("{key}-SOURCE");
    fixture
        .received_balance(
            access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id,
                qty: 7,
                key: &source_location_barcode,
            },
        )
        .await;

    let allocation = send(
        app,
        token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{order_id}/allocation-runs"),
        Some(&format!("{key}-allocate")),
        Some(
            serde_json::to_value(PlanOrderAllocationRequest {
                facility_id,
                expected_revision: Revision::new(1).unwrap(),
                expected_policy: AllocationPolicyReference::product_default(),
            })
            .unwrap(),
        ),
    )
    .await;
    let allocation: PlanOrderAllocationResponse =
        response_json(expect_status(allocation, StatusCode::OK, "allocate reversal order").await)
            .await;
    assert_eq!(allocation.revision.get(), 2);

    let release = send(
        app,
        token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{order_id}/releases"),
        Some(&format!("{key}-release")),
        Some(json!({
            "facility_id": facility_id,
            "destination_location_id": execution_location_id,
            "expected_revision": 2
        })),
    )
    .await;
    let release: ReleaseOrderResponse =
        response_json(expect_status(release, StatusCode::OK, "release reversal order").await).await;
    assert_eq!(release.revision.get(), 3);

    let claim = send(
        app,
        token,
        access.tenant_id,
        Method::POST,
        "/api/v1/picking-claims/next",
        Some(&format!("{key}-claim")),
        Some(json!({})),
    )
    .await;
    let claim: PickClaimResponse = response_json::<Option<PickClaimResponse>>(
        expect_status(claim, StatusCode::OK, "claim reversal pick").await,
    )
    .await
    .unwrap();
    let confirmation = send(
        app,
        token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/picking-tasks/{}/contents/{}/confirmations",
            claim.task_id, claim.content.content_id
        ),
        Some(&format!("{key}-confirm")),
        Some(json!({
            "source_location_barcode": claim.content.source_location_barcode,
            "item_barcode": claim.content.item_barcodes[0],
            "destination_license_plate_barcode": tote_barcode
        })),
    )
    .await;
    let confirmation: PickContentConfirmationResponse =
        response_json(expect_status(confirmation, StatusCode::OK, "confirm reversible pick").await)
            .await;
    assert_eq!(confirmation.order_revision.get(), 4);

    PickedFixture {
        order_id,
        facility_id,
        execution_location_id,
        execution_location_barcode,
        tote_barcode,
        source_location_barcode,
        item_barcode,
        lot: claim.content.lot,
        serial: claim.content.serial,
        task_id: claim.task_id,
        confirmation,
    }
}

pub fn reversal_body(picked: &PickedFixture, revision: i64) -> Value {
    json!({
        "expected_order_revision": revision,
        "staged_location_barcode": picked.execution_location_barcode,
        "staged_license_plate_barcode": picked.tote_barcode,
        "item_barcode": picked.item_barcode,
        "lot_scan": picked.lot,
        "serial_scan": picked.serial,
        "return_location_barcode": picked.source_location_barcode,
        "reason": "mis_pick",
        "note": "Operator selected the wrong unit"
    })
}
