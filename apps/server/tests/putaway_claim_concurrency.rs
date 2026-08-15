mod common;

use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tokio::time::timeout;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{ErrorReason, ErrorResponse};
use wareboxes_application::CommandContext;
use wareboxes_core::models::InboundReceiptExceptionReason;

const OPERATION_TIMEOUT: Duration = Duration::from_secs(3);

fn command(access: &wareboxes_core::models::TenantAccess, key: &str) -> CommandContext {
    CommandContext {
        tenant_id: access.tenant_id,
        actor_id: access.user_id,
        request_id: format!("request-{key}"),
        idempotency_key: Some(key.to_owned()),
    }
}

fn request(
    token: &str,
    tenant_id: TenantId,
    uri: &str,
    idempotency_key: &str,
    body: Value,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(IDEMPOTENCY_KEY_HEADER, idempotency_key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn send(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    uri: &str,
    idempotency_key: &str,
    body: Value,
) -> axum::response::Response {
    timeout(
        OPERATION_TIMEOUT,
        app.clone()
            .oneshot(request(token, tenant_id, uri, idempotency_key, body)),
    )
    .await
    .expect("concurrent typed putaway claim completes within the bound")
    .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn add_membership(db: &db::Db, tenant_id: TenantId, user_id: i64) {
    let mut tx = tenant_tx(db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
        .bind(tenant_id.get())
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn grant_wms_role(db: &db::Db, tenant_id: TenantId, user_ids: &[i64]) {
    let permission =
        wareboxes_persistence_postgres::permissions::find_by_name(db, tenant_id, "wms")
            .await
            .unwrap()
            .unwrap();
    let role = wareboxes_persistence_postgres::roles::add_role(
        db,
        tenant_id,
        "putaway-claim-race-workers",
        Some("Putaway claim race workers"),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        db,
        tenant_id,
        role,
        permission.id,
    )
    .await
    .unwrap());
    for user_id in user_ids {
        assert!(wareboxes_persistence_postgres::roles::add_role_to_user(
            db, tenant_id, *user_id, role
        )
        .await
        .unwrap());
    }
}

#[allow(clippy::too_many_arguments)]
async fn receive_loose_balance(
    fixture: &Fixture,
    access: &wareboxes_core::models::TenantAccess,
    inventory_owner_id: i64,
    facility_id: i64,
    receiving_location_id: i64,
    item_id: i64,
    key: &str,
) -> i64 {
    let lot = "PUTAWAY-CLAIM-RACE-LOT";
    let load_id = repo::loads::add_load(
        &fixture.db,
        access.tenant_id,
        access.user_id.get(),
        facility_id,
        inventory_owner_id,
        LoadType::Inbound,
        Some(key),
        None,
        None,
        None,
        None,
        Some(receiving_location_id),
        None,
        None,
    )
    .await
    .unwrap();
    let load_line_id = repo::loads::add_line(
        &fixture.db,
        access.tenant_id,
        access.user_id.get(),
        load_id,
        item_id,
        None,
        10,
        Some(lot),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(repo::loads::update_load(
        &fixture.db,
        access.tenant_id,
        access.user_id.get(),
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
        &fixture.db,
        access,
        load_id,
        receiving_location_id,
        &format!("{key}-unloading"),
    )
    .await;
    repo::inbound_receipt::receive_expected_inventory(
        &fixture.db,
        access,
        &command(access, &format!("{key}-receipt")),
        load_line_id,
        &repo::inbound_receipt::ReceiveExpectedInventoryCommand {
            receiving_location_id: Some(receiving_location_id),
            received_qty: 10,
            rejected_qty: 0,
            missing_qty: 0,
            license_plate_id: None,
            license_plate_barcode: None,
            lot: Some(lot),
            serial: None,
            expiration: None,
            exception_reason: None::<InboundReceiptExceptionReason>,
            exception_note: None,
        },
    )
    .await
    .unwrap()
    .inventory_balance_id
    .expect("physical receipt identifies its balance")
}

#[allow(clippy::too_many_arguments)]
async fn create_loose_task(
    fixture: &Fixture,
    access: &wareboxes_core::models::TenantAccess,
    inventory_balance_id: i64,
    destination_location_id: i64,
    priority: i64,
    key: &str,
) -> i64 {
    repo::tasks::create_putaway_task_in_scope(
        &fixture.db,
        access,
        &command(access, key),
        inventory_balance_id,
        destination_location_id,
        4,
        priority,
        None,
        None,
        None,
        Some("Concurrent putaway claim"),
    )
    .await
    .unwrap()
}

async fn inventory_snapshot(db: &db::Db, tenant_id: TenantId) -> String {
    let mut tx = tenant_tx(db, tenant_id).await;
    let snapshot: String = sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
            'balances', COALESCE((
                SELECT jsonb_agg(
                    jsonb_build_array(
                        id, location_id, license_plate_id, qty_on_hand,
                        qty_reserved, qty_held, status, deleted
                    )
                    ORDER BY id
                )
                FROM inventory_balances
                WHERE tenant_id = $1
            ), '[]'::JSONB),
            'plates', COALESCE((
                SELECT jsonb_agg(
                    jsonb_build_array(id, location_id, deleted)
                    ORDER BY id
                )
                FROM license_plates
                WHERE tenant_id = $1
            ), '[]'::JSONB),
            'transactions', (
                SELECT COUNT(*)
                FROM inventory_transactions
                WHERE tenant_id = $1
            ),
            'entries', (
                SELECT COUNT(*)
                FROM inventory_entries
                WHERE tenant_id = $1
            ),
            'loose_results', (
                SELECT COUNT(*)
                FROM putaway_results
                WHERE tenant_id = $1
            ),
            'plate_results', (
                SELECT COUNT(*)
                FROM license_plate_putaway_results
                WHERE tenant_id = $1
            ),
            'outbox', (
                SELECT COUNT(*)
                FROM outbox_events
                WHERE tenant_id = $1
            ),
            'reconciliation', (
                SELECT COUNT(*)
                FROM inventory_reconciliation
            )
        )::TEXT
        "#,
    )
    .bind(tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    snapshot
}

async fn claim_state(
    db: &db::Db,
    tenant_id: TenantId,
    task_ids: &[i64],
    user_id: Option<i64>,
) -> (i64, i64) {
    let mut tx = tenant_tx(db, tenant_id).await;
    let state = sqlx::query_as(
        r#"
        SELECT
            (
                SELECT COUNT(*)
                FROM work_tasks
                WHERE tenant_id = $1
                  AND id = ANY($2)
                  AND deleted IS NULL
                  AND status = 'in_progress'
                  AND ($3::BIGINT IS NULL OR assigned_user_id = $3)
                  AND lease_expires_at > statement_timestamp()
            ),
            (
                SELECT COUNT(*)
                FROM work_task_progress
                WHERE tenant_id = $1
                  AND task_id = ANY($2)
                  AND action = 'started'
                  AND ($3::BIGINT IS NULL OR user_id = $3)
            )
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_ids)
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    state
}

#[tokio::test]
async fn typed_putaway_claim_races_choose_one_task_without_inventory_effects() {
    let fixture = Fixture::new().await;
    let administrator = fixture
        .wms_user("putaway-claim-race-admin@test.local")
        .await;
    let access = default_tenant_for_user(&fixture.db, administrator.id)
        .await
        .unwrap();
    let tenant_id = access.tenant_id;
    let first_worker = fixture.user("putaway-claim-race-a@test.local").await;
    let second_worker = fixture.user("putaway-claim-race-b@test.local").await;
    for user_id in [first_worker.id, second_worker.id] {
        add_membership(&fixture.db, tenant_id, user_id).await;
    }
    grant_wms_role(&fixture.db, tenant_id, &[first_worker.id, second_worker.id]).await;
    let first_token = auth::create_session(&fixture.db, first_worker.id)
        .await
        .unwrap();
    let second_token = auth::create_session(&fixture.db, second_worker.id)
        .await
        .unwrap();
    let administrator_token = auth::create_session(&fixture.db, administrator.id)
        .await
        .unwrap();
    let facility_id = fixture
        .facility(tenant_id, "Putaway Claim Race Facility")
        .await;
    let inventory_owner_id = fixture
        .inventory_owner(tenant_id, "Putaway Claim Race Owner")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let receiving_location_id = wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some("PUTAWAY-CLAIM-RACE-RECEIVING"),
        Some("Putaway Claim Race Receiving"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let destination_location_id = fixture
        .location(tenant_id, facility_id, "PUTAWAY-CLAIM-RACE-DESTINATION")
        .await;
    let item_id = fixture
        .item(tenant_id, "Putaway Claim Race Item", "case")
        .await;
    let app = routes::app(AppState::new(fixture.db.clone()));

    let contested_balance = receive_loose_balance(
        &fixture,
        &access,
        inventory_owner_id,
        facility_id,
        receiving_location_id,
        item_id,
        "PUTAWAY-CLAIM-RACE-CONTESTED",
    )
    .await;
    let contested_task = create_loose_task(
        &fixture,
        &access,
        contested_balance,
        destination_location_id,
        100,
        "putaway-claim-race-contested-create",
    )
    .await;
    let before_contested = inventory_snapshot(&fixture.db, tenant_id).await;
    let (first, second) = tokio::join!(
        send(
            &app,
            &first_token,
            tenant_id,
            "/api/v1/putaway-claims/next",
            "putaway-claim-race-worker-a",
            json!({"workflow": "loose"}),
        ),
        send(
            &app,
            &second_token,
            tenant_id,
            "/api/v1/putaway-claims/next",
            "putaway-claim-race-worker-b",
            json!({"workflow": "loose"}),
        ),
    );
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    let claims = [
        response_json::<Value>(first).await,
        response_json::<Value>(second).await,
    ];
    let claimed = claims
        .iter()
        .filter(|claim| !claim.is_null())
        .collect::<Vec<_>>();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0]["task_id"], contested_task);
    assert_eq!(claims.iter().filter(|claim| claim.is_null()).count(), 1);
    assert_eq!(
        claim_state(&fixture.db, tenant_id, &[contested_task], None).await,
        (1, 1)
    );
    assert_eq!(
        inventory_snapshot(&fixture.db, tenant_id).await,
        before_contested
    );

    let selected_balance = receive_loose_balance(
        &fixture,
        &access,
        inventory_owner_id,
        facility_id,
        receiving_location_id,
        item_id,
        "PUTAWAY-CLAIM-RACE-SELECTED",
    )
    .await;
    let selected_task = create_loose_task(
        &fixture,
        &access,
        selected_balance,
        destination_location_id,
        10,
        "putaway-claim-race-selected-create",
    )
    .await;
    let next_balance = receive_loose_balance(
        &fixture,
        &access,
        inventory_owner_id,
        facility_id,
        receiving_location_id,
        item_id,
        "PUTAWAY-CLAIM-RACE-NEXT",
    )
    .await;
    let next_task = create_loose_task(
        &fixture,
        &access,
        next_balance,
        destination_location_id,
        100,
        "putaway-claim-race-next-create",
    )
    .await;
    let before_same_user = inventory_snapshot(&fixture.db, tenant_id).await;
    let selected_uri = format!("/api/v1/putaway-claims/{selected_task}");
    let (selected, next) = tokio::join!(
        send(
            &app,
            &administrator_token,
            tenant_id,
            &selected_uri,
            "putaway-claim-race-selected",
            json!({}),
        ),
        send(
            &app,
            &administrator_token,
            tenant_id,
            "/api/v1/putaway-claims/next",
            "putaway-claim-race-next",
            json!({"workflow": "loose"}),
        ),
    );
    let selected_status = selected.status();
    let next_status = next.status();
    let statuses = [selected_status, next_status];
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
    let successful_claim = if selected_status == StatusCode::OK {
        let claim = response_json::<Value>(selected).await;
        assert_eq!(
            response_json::<ErrorResponse>(next).await.reason,
            ErrorReason::Conflict
        );
        claim
    } else {
        assert_eq!(
            response_json::<ErrorResponse>(selected).await.reason,
            ErrorReason::Conflict
        );
        response_json::<Value>(next).await
    };
    assert!([selected_task, next_task].contains(
        &successful_claim["task_id"]
            .as_i64()
            .expect("successful race response identifies its task")
    ));
    assert_eq!(
        claim_state(
            &fixture.db,
            tenant_id,
            &[selected_task, next_task],
            Some(administrator.id),
        )
        .await,
        (1, 1)
    );
    assert_eq!(
        inventory_snapshot(&fixture.db, tenant_id).await,
        before_same_user
    );
}
