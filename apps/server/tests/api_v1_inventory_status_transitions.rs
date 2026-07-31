mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    ErrorReason, ErrorResponse, InventoryBalanceStatus, InventoryStatusTransitionResponse,
    IDEMPOTENCY_KEY_HEADER,
};
use wareboxes_core::dto::UpdateUserAccessScope;

fn request(
    token: &str,
    tenant_id: TenantId,
    balance_id: i64,
    idempotency_key: Option<&str>,
    body: Value,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/api/v1/inventory/balances/{balance_id}/status-transitions"
        ))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = idempotency_key {
        request = request.header(IDEMPOTENCY_KEY_HEADER, key);
    }
    request.body(Body::from(body.to_string())).unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn transition_count(db: &db::Db, tenant_id: TenantId) -> i64 {
    let mut tx = tenant_tx(db, tenant_id).await;
    let count = sqlx::query_scalar(
        "SELECT COUNT(*) FROM inventory_status_transitions WHERE tenant_id = $1",
    )
    .bind(tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    count
}

async fn balance_quantity(db: &db::Db, tenant_id: TenantId, balance_id: i64) -> (String, i64) {
    let mut tx = tenant_tx(db, tenant_id).await;
    let quantity = sqlx::query_as(
        r#"
        SELECT status, qty_on_hand
        FROM inventory_balances
        WHERE tenant_id = $1 AND id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    quantity
}

#[tokio::test]
async fn status_transition_v1_is_strict_scoped_and_replay_safe() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("v1-status-transition@test.local").await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let tenant_id = access.tenant_id;
    let allowed_owner = fixture
        .inventory_owner(tenant_id, "V1 Disposition Allowed Owner")
        .await;
    let denied_owner = fixture
        .inventory_owner(tenant_id, "V1 Disposition Denied Owner")
        .await;
    let allowed_facility = fixture
        .facility(tenant_id, "V1 Disposition Allowed DC")
        .await;
    let denied_facility = fixture
        .facility(tenant_id, "V1 Disposition Denied DC")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, allowed_owner, allowed_facility)
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, denied_owner, denied_facility)
        .await;
    let item = fixture.item(tenant_id, "V1 Disposition Item", "case").await;
    let allowed = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: allowed_owner,
                facility_id: allowed_facility,
                item_id: item,
                qty: 12,
                key: "V1-DISPOSITION-ALLOWED",
            },
        )
        .await;
    let denied = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: denied_owner,
                facility_id: denied_facility,
                item_id: item,
                qty: 8,
                key: "V1-DISPOSITION-DENIED",
            },
        )
        .await;
    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: user.id,
            all_facilities: false,
            facility_ids: vec![allowed_facility],
            all_inventory_owners: false,
            inventory_owner_ids: vec![allowed_owner],
        },
    )
    .await
    .unwrap());

    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let valid = json!({
        "quantity": 4,
        "to_status": "quarantine",
        "reason": "quality_inspection",
        "note": "Awaiting inbound quality review",
        "reference_type": "receipt",
        "reference_id": 72
    });
    let before = transition_count(&fixture.db, tenant_id).await;

    let missing_key = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            allowed.balance_id,
            None,
            valid.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);
    let missing_key: ErrorResponse = response_json(missing_key).await;
    assert_eq!(missing_key.reason, ErrorReason::IdempotencyKeyRequired);
    assert!(!missing_key.request_id.is_empty());

    for (key, body) in [
        (
            "v1-status-transition-zero",
            json!({
                "quantity": 0,
                "to_status": "quarantine",
                "reason": "quality_inspection",
                "note": null,
                "reference_type": null,
                "reference_id": null
            }),
        ),
        (
            "v1-status-transition-pair",
            json!({
                "quantity": 1,
                "to_status": "quarantine",
                "reason": "quality_inspection",
                "note": null,
                "reference_type": "receipt",
                "reference_id": null
            }),
        ),
        (
            "v1-status-transition-reason",
            json!({
                "quantity": 1,
                "to_status": "available",
                "reason": "damage_confirmed",
                "note": null,
                "reference_type": null,
                "reference_id": null
            }),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(request(
                &token,
                tenant_id,
                allowed.balance_id,
                Some(key),
                body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json::<ErrorResponse>(response).await.reason,
            ErrorReason::InvalidRequest
        );
    }

    let unknown_field = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            allowed.balance_id,
            Some("v1-status-transition-unknown"),
            json!({
                "quantity": 1,
                "to_status": "quarantine",
                "reason": "quality_inspection",
                "note": null,
                "reference_type": null,
                "reference_id": null,
                "force": true
            }),
        ))
        .await
        .unwrap();
    assert_eq!(unknown_field.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        response_json::<ErrorResponse>(unknown_field).await.reason,
        ErrorReason::ValidationFailed
    );

    let denied_scope = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            denied.balance_id,
            Some("v1-status-transition-denied"),
            valid.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(denied_scope.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response_json::<ErrorResponse>(denied_scope).await.reason,
        ErrorReason::Forbidden
    );
    assert_eq!(transition_count(&fixture.db, tenant_id).await, before);

    let changed = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            allowed.balance_id,
            Some("v1-status-transition-success"),
            valid.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::OK);
    let changed: InventoryStatusTransitionResponse = response_json(changed).await;
    assert_eq!(
        changed,
        InventoryStatusTransitionResponse {
            inventory_transaction_id: changed.inventory_transaction_id,
            source_inventory_balance_id: allowed.balance_id,
            target_inventory_balance_id: changed.target_inventory_balance_id,
            quantity: 4,
            from_status: InventoryBalanceStatus::Available,
            to_status: InventoryBalanceStatus::Quarantine,
        }
    );

    let replay = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            allowed.balance_id,
            Some("v1-status-transition-success"),
            valid.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<InventoryStatusTransitionResponse>(replay).await,
        changed
    );

    let reused = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            allowed.balance_id,
            Some("v1-status-transition-success"),
            json!({
                "quantity": 3,
                "to_status": "quarantine",
                "reason": "quality_inspection",
                "note": "Awaiting inbound quality review",
                "reference_type": "receipt",
                "reference_id": 72
            }),
        ))
        .await
        .unwrap();
    assert_eq!(reused.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(reused).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    assert_eq!(
        balance_quantity(&fixture.db, tenant_id, allowed.balance_id).await,
        ("available".into(), 8)
    );
    assert_eq!(
        balance_quantity(&fixture.db, tenant_id, changed.target_inventory_balance_id).await,
        ("quarantine".into(), 4)
    );
    assert_eq!(transition_count(&fixture.db, tenant_id).await, before + 1);
    assert!(
        repo::inventory::get_reconciliation_issues(&fixture.db, tenant_id)
            .await
            .unwrap()
            .is_empty()
    );
}
