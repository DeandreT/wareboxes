use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use serde::Serialize;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    ChangeLicensePlateParentRequest, ChangeLicensePlateParentResponse,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InboundReceiptExceptionReason, LoadStatus, LoadType};

use crate::common::*;

pub(crate) struct Rig {
    pub(crate) fixture: Fixture,
    pub(crate) tenant_id: TenantId,
    pub(crate) user_id: i64,
    pub(crate) token: String,
    pub(crate) inventory_owner_id: i64,
    pub(crate) facility_id: i64,
    pub(crate) item_id: i64,
    pub(crate) source_location_id: i64,
    pub(crate) destination_location_id: i64,
    pub(crate) parent_id: i64,
    pub(crate) child_id: i64,
    pub(crate) grandchild_id: i64,
    pub(crate) app: axum::Router,
}

impl Rig {
    pub(crate) async fn new(suffix: &str) -> Self {
        let fixture = Fixture::new().await;
        let user = fixture
            .wms_user(&format!("lpn-hierarchy-{suffix}@test.local"))
            .await;
        let tenant_id = tenant_for_user(&fixture.db, user.id).await;
        let inventory_owner_id = fixture
            .inventory_owner(tenant_id, &format!("Hierarchy Client {suffix}"))
            .await;
        let facility_id = fixture
            .facility(tenant_id, &format!("Hierarchy DC {suffix}"))
            .await;
        fixture
            .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
            .await;
        let item_id = fixture
            .item(tenant_id, &format!("Hierarchy Item {suffix}"), "each")
            .await;
        let source_location_id = wareboxes_persistence_postgres::locations::add_location(
            &fixture.db,
            tenant_id,
            facility_id,
            None,
            Some(&format!("LPN-HIERARCHY-SOURCE-{suffix}")),
            Some(&format!("Hierarchy Receiving {suffix}")),
            "dock",
            true,
            false,
            true,
        )
        .await
        .unwrap();
        let destination_location_id = fixture
            .location(
                tenant_id,
                facility_id,
                &format!("LPN-HIERARCHY-DEST-{suffix}"),
            )
            .await;
        let parent_id = add_plate(
            &fixture,
            tenant_id,
            inventory_owner_id,
            facility_id,
            source_location_id,
            &format!("PALLET-{suffix}"),
        )
        .await;
        let child_id = add_plate(
            &fixture,
            tenant_id,
            inventory_owner_id,
            facility_id,
            source_location_id,
            &format!("CASE-{suffix}"),
        )
        .await;
        let grandchild_id = add_plate(
            &fixture,
            tenant_id,
            inventory_owner_id,
            facility_id,
            source_location_id,
            &format!("INNER-{suffix}"),
        )
        .await;
        let token = wareboxes_api::auth::create_session(&fixture.db, user.id)
            .await
            .unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));
        Self {
            fixture,
            tenant_id,
            user_id: user.id,
            token,
            inventory_owner_id,
            facility_id,
            item_id,
            source_location_id,
            destination_location_id,
            parent_id,
            child_id,
            grandchild_id,
            app,
        }
    }

    pub(crate) async fn send<T: Serialize>(
        &self,
        method: Method,
        uri: &str,
        key: Option<&str>,
        body: Option<&T>,
    ) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(request(&self.token, self.tenant_id, method, uri, key, body))
            .await
            .unwrap()
    }

    pub(crate) async fn change_parent(
        &self,
        license_plate_id: i64,
        parent_license_plate_id: Option<i64>,
        expected_revision: i64,
        reason: &str,
        key: &str,
    ) -> axum::response::Response {
        self.send(
            Method::POST,
            &format!("/api/v1/license-plates/{license_plate_id}/parent-changes"),
            Some(key),
            Some(&ChangeLicensePlateParentRequest {
                parent_license_plate_id,
                expected_revision,
                reason: reason.into(),
            }),
        )
        .await
    }

    pub(crate) async fn receive_into_plate(&self, license_plate_id: i64, suffix: &str) -> i64 {
        let access = default_tenant_for_user(&self.fixture.db, self.user_id)
            .await
            .expect("WMS user has tenant access");
        let load_id = wareboxes_api::repo::loads::add_load(
            &self.fixture.db,
            self.tenant_id,
            self.user_id,
            self.facility_id,
            self.inventory_owner_id,
            LoadType::Inbound,
            Some(&format!("LPN-HIERARCHY-LOAD-{suffix}")),
            None,
            None,
            None,
            None,
            Some(self.source_location_id),
            None,
            None,
        )
        .await
        .unwrap();
        let line_id = wareboxes_api::repo::loads::add_line(
            &self.fixture.db,
            self.tenant_id,
            self.user_id,
            load_id,
            self.item_id,
            None,
            12,
            Some(&format!("LOT-{suffix}")),
            None,
            None,
        )
        .await
        .unwrap();
        assert!(wareboxes_api::repo::loads::update_load(
            &self.fixture.db,
            self.tenant_id,
            self.user_id,
            load_id,
            Some(LoadStatus::Arrived),
            None,
            None,
            None,
            None,
            None,
            None,
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
        .unwrap());
        start_expected_receipt_unloading(
            &self.fixture.db,
            &access,
            load_id,
            self.source_location_id,
            &format!("hierarchy-unload-{suffix}"),
        )
        .await;
        wareboxes_api::repo::inbound_receipt::receive_expected_inventory(
            &self.fixture.db,
            &access,
            &CommandContext {
                tenant_id: self.tenant_id,
                actor_id: access.user_id,
                request_id: format!("hierarchy-receipt-{suffix}"),
                idempotency_key: Some(format!("hierarchy-receipt-{suffix}")),
            },
            line_id,
            &wareboxes_api::repo::inbound_receipt::ReceiveExpectedInventoryCommand {
                receiving_location_id: Some(self.source_location_id),
                received_qty: 12,
                rejected_qty: 0,
                missing_qty: 0,
                license_plate_id: Some(license_plate_id),
                license_plate_barcode: None,
                lot: Some(&format!("LOT-{suffix}")),
                serial: None,
                expiration: None,
                exception_reason: None::<InboundReceiptExceptionReason>,
                exception_note: None,
            },
        )
        .await
        .unwrap()
        .inventory_balance_id
        .expect("received inventory has a balance")
    }
}

pub(crate) fn command(access: &wareboxes_core::models::TenantAccess, key: &str) -> CommandContext {
    CommandContext {
        tenant_id: access.tenant_id,
        actor_id: access.user_id,
        request_id: format!("request-{key}"),
        idempotency_key: Some(key.to_owned()),
    }
}

pub(crate) async fn add_plate(
    fixture: &Fixture,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
    location_id: i64,
    barcode: &str,
) -> i64 {
    let id = wareboxes_api::repo::license_plates::add_license_plate(
        &fixture.db,
        tenant_id,
        inventory_owner_id,
        facility_id,
        Some(barcode),
    )
    .await
    .unwrap();
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("UPDATE license_plates SET location_id=$1 WHERE tenant_id=$2 AND id=$3")
        .bind(location_id)
        .bind(tenant_id.get())
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    id
}

pub(crate) fn request<T: Serialize>(
    token: &str,
    tenant_id: TenantId,
    method: Method,
    uri: &str,
    key: Option<&str>,
    body: Option<&T>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    if let Some(key) = key {
        builder = builder.header(IDEMPOTENCY_KEY_HEADER, key);
    }
    let body = match body {
        Some(body) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(body).unwrap())
        }
        None => Body::empty(),
    };
    builder.body(body).unwrap()
}

pub(crate) async fn json<T: serde::de::DeserializeOwned>(
    response: axum::response::Response,
    expected: StatusCode,
) -> T {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    assert_eq!(
        status,
        expected,
        "unexpected response: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

pub(crate) async fn change_json(
    response: axum::response::Response,
    expected: StatusCode,
) -> ChangeLicensePlateParentResponse {
    json(response, expected).await
}
