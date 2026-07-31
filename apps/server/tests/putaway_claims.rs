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
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_core::models::InboundReceiptExceptionReason;
use wareboxes_domain::CommandContext;

const OPERATION_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
struct ReceivedStock {
    inventory_balance_id: i64,
    item_batch_id: i64,
    license_plate_id: Option<i64>,
    license_plate_barcode: Option<String>,
    lot: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
struct TaskCounters {
    release_count: i64,
    started: i64,
    expired: i64,
    scope_revoked: i64,
}

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
    method: Method,
    uri: &str,
    idempotency_key: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    if let Some(idempotency_key) = idempotency_key {
        request = request.header(IDEMPOTENCY_KEY_HEADER, idempotency_key);
    }
    let body = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    request.body(body).unwrap()
}

async fn send(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    method: Method,
    uri: &str,
    idempotency_key: Option<&str>,
    body: Option<Value>,
) -> axum::response::Response {
    timeout(
        OPERATION_TIMEOUT,
        app.clone().oneshot(request(
            token,
            tenant_id,
            method,
            uri,
            idempotency_key,
            body,
        )),
    )
    .await
    .expect("typed putaway claim request completes within the bound")
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

async fn set_scope(
    db: &db::Db,
    tenant_id: TenantId,
    user_id: i64,
    facility_ids: Vec<i64>,
    inventory_owner_ids: Vec<i64>,
) {
    assert!(repo::tenants::update_user_access_scope(
        db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id,
            all_facilities: false,
            facility_ids,
            all_inventory_owners: false,
            inventory_owner_ids,
        },
    )
    .await
    .unwrap());
}

#[allow(clippy::too_many_arguments)]
async fn receive_stock(
    fixture: &Fixture,
    access: &wareboxes_core::models::TenantAccess,
    inventory_owner_id: i64,
    facility_id: i64,
    receiving_location_id: i64,
    item_id: i64,
    key: &str,
    containerized: bool,
) -> ReceivedStock {
    let lot = "PUTAWAY-CLAIM-LOT";
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
    let receipt = repo::inbound_receipt::receive_expected_inventory(
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
            license_plate_barcode: containerized.then_some(key),
            lot: Some(lot),
            serial: None,
            expiration: None,
            exception_reason: None::<InboundReceiptExceptionReason>,
            exception_note: None,
        },
    )
    .await
    .unwrap();
    ReceivedStock {
        inventory_balance_id: receipt
            .inventory_balance_id
            .expect("physical receipt identifies its balance"),
        item_batch_id: receipt
            .item_batch_id
            .expect("physical receipt identifies its item batch"),
        license_plate_id: receipt.license_plate_id,
        license_plate_barcode: containerized.then(|| key.to_owned()),
        lot: lot.to_owned(),
    }
}

#[allow(clippy::too_many_arguments)]
async fn receive_additional_plate_stock(
    fixture: &Fixture,
    access: &wareboxes_core::models::TenantAccess,
    inventory_owner_id: i64,
    facility_id: i64,
    receiving_location_id: i64,
    item_id: i64,
    license_plate_id: i64,
    lot: &str,
    key: &str,
) {
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
        1,
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
    repo::inbound_receipt::receive_expected_inventory(
        &fixture.db,
        access,
        &command(access, &format!("{key}-receipt")),
        load_line_id,
        &repo::inbound_receipt::ReceiveExpectedInventoryCommand {
            receiving_location_id: Some(receiving_location_id),
            received_qty: 1,
            rejected_qty: 0,
            missing_qty: 0,
            license_plate_id: Some(license_plate_id),
            license_plate_barcode: None,
            lot: Some(lot),
            serial: None,
            expiration: None,
            exception_reason: None::<InboundReceiptExceptionReason>,
            exception_note: None,
        },
    )
    .await
    .unwrap();
}

#[allow(clippy::too_many_arguments)]
async fn create_loose_task(
    fixture: &Fixture,
    access: &wareboxes_core::models::TenantAccess,
    stock: &ReceivedStock,
    destination_location_id: i64,
    quantity: i64,
    priority: i64,
    assigned_user_id: Option<i64>,
    key: &str,
    instructions: &str,
) -> i64 {
    repo::tasks::create_putaway_task_in_scope(
        &fixture.db,
        access,
        &command(access, key),
        stock.inventory_balance_id,
        destination_location_id,
        quantity,
        priority,
        assigned_user_id,
        None,
        None,
        Some(instructions),
    )
    .await
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
async fn create_plate_task(
    fixture: &Fixture,
    access: &wareboxes_core::models::TenantAccess,
    stock: &ReceivedStock,
    destination_location_id: i64,
    priority: i64,
    assigned_user_id: Option<i64>,
    key: &str,
    instructions: &str,
) -> i64 {
    repo::tasks::create_license_plate_putaway_task_in_scope(
        &fixture.db,
        access,
        &command(access, key),
        stock
            .license_plate_id
            .expect("license plate task stock is containerized"),
        destination_location_id,
        priority,
        assigned_user_id,
        None,
        None,
        Some(instructions),
    )
    .await
    .unwrap()
}

async fn side_effect_snapshot(db: &db::Db, tenant_id: TenantId) -> String {
    let mut tx = tenant_tx(db, tenant_id).await;
    let snapshot: String = sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
            'tasks', COALESCE((
                SELECT jsonb_agg(
                    jsonb_build_array(
                        id, task_type, status, assigned_user_id,
                        lease_expires_at, release_count, completed_at, deleted
                    )
                    ORDER BY id
                )
                FROM work_tasks
                WHERE tenant_id = $1
            ), '[]'::JSONB),
            'details', jsonb_build_object(
                'loose', COALESCE((
                    SELECT jsonb_agg(
                        jsonb_build_array(task_id, closed_at)
                        ORDER BY task_id
                    )
                    FROM putaway_tasks
                    WHERE tenant_id = $1
                ), '[]'::JSONB),
                'plate', COALESCE((
                    SELECT jsonb_agg(
                        jsonb_build_array(task_id, closed_at)
                        ORDER BY task_id
                    )
                    FROM license_plate_putaway_tasks
                    WHERE tenant_id = $1
                ), '[]'::JSONB)
            ),
            'progress', (
                SELECT COUNT(*)
                FROM work_task_progress
                WHERE tenant_id = $1
            ),
            'commands', (
                SELECT COUNT(*)
                FROM command_idempotency_records
                WHERE tenant_id = $1
            ),
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

async fn task_counters(db: &db::Db, tenant_id: TenantId, task_id: i64) -> TaskCounters {
    let mut tx = tenant_tx(db, tenant_id).await;
    let counters = sqlx::query_as(
        r#"
        SELECT
            task.release_count,
            COUNT(progress.id) FILTER (
                WHERE progress.action = 'started'
            ) AS started,
            COUNT(progress.id) FILTER (
                WHERE progress.action = 'expired'
            ) AS expired,
            COUNT(progress.id) FILTER (
                WHERE progress.action = 'scope_revoked'
            ) AS scope_revoked
        FROM work_tasks task
        LEFT JOIN work_task_progress progress
          ON progress.tenant_id = task.tenant_id
         AND progress.task_id = task.id
        WHERE task.tenant_id = $1 AND task.id = $2
        GROUP BY task.release_count
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    counters
}

#[allow(clippy::too_many_arguments)]
fn assert_common_claim(
    claim: &Value,
    task_id: i64,
    inventory_owner_id: i64,
    facility_id: i64,
    priority: i64,
    instructions: &str,
    source_location_id: i64,
    source_barcode: &str,
    source_name: &str,
    destination_location_id: i64,
    destination_barcode: &str,
    destination_name: &str,
) {
    assert_eq!(claim["task_id"], task_id);
    assert_eq!(claim["inventory_owner_id"], inventory_owner_id);
    assert_eq!(claim["facility_id"], facility_id);
    assert_eq!(claim["priority"], priority);
    assert_eq!(claim["instructions"], instructions);
    assert!(claim["due_at"].is_null());
    assert!(claim["lease_expires_at"].as_str().is_some());
    assert_eq!(
        claim["source_location"],
        json!({
            "location_id": source_location_id,
            "barcode": source_barcode,
            "name": source_name,
        })
    );
    assert_eq!(
        claim["destination_location"],
        json!({
            "location_id": destination_location_id,
            "barcode": destination_barcode,
            "name": destination_name,
        })
    );
}

#[tokio::test]
async fn typed_putaway_claims_are_exact_scoped_replay_safe_and_reclaimable() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("putaway-claims@test.local").await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let tenant_id = access.tenant_id;
    let facility_id = fixture.facility(tenant_id, "Putaway Claim Facility").await;
    let inventory_owner_id = fixture
        .inventory_owner(tenant_id, "Putaway Claim Owner")
        .await;
    let denied_owner_id = fixture
        .inventory_owner(tenant_id, "Putaway Claim Denied Owner")
        .await;
    for owner_id in [inventory_owner_id, denied_owner_id] {
        fixture
            .assign_owner_to_facility(tenant_id, owner_id, facility_id)
            .await;
    }
    let source_barcode = "PUTAWAY-CLAIM-RECEIVING";
    let source_name = "Putaway Claim Receiving";
    let source_location_id = repo::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some(source_barcode),
        Some(source_name),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let destination_barcode = "PUTAWAY-CLAIM-DESTINATION";
    let destination_location_id = fixture
        .location(tenant_id, facility_id, destination_barcode)
        .await;
    let item_name = "Putaway Claim Item";
    let item_id = fixture.item(tenant_id, item_name, "case").await;
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let loose_stock = receive_stock(
        &fixture,
        &access,
        inventory_owner_id,
        facility_id,
        source_location_id,
        item_id,
        "PUTAWAY-CLAIM-LOOSE",
        false,
    )
    .await;
    let loose_instructions = "Put away the exact loose quantity";
    let loose_task = create_loose_task(
        &fixture,
        &access,
        &loose_stock,
        destination_location_id,
        4,
        90,
        None,
        "putaway-claim-loose-create",
        loose_instructions,
    )
    .await;
    let loose_claim = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/v1/putaway-claims/next",
        Some("putaway-claim-loose-next"),
        Some(json!({"workflow": "loose"})),
    )
    .await;
    assert_eq!(loose_claim.status(), StatusCode::OK);
    let loose_claim: Value = response_json(loose_claim).await;
    assert_common_claim(
        &loose_claim,
        loose_task,
        inventory_owner_id,
        facility_id,
        90,
        loose_instructions,
        source_location_id,
        source_barcode,
        source_name,
        destination_location_id,
        destination_barcode,
        destination_barcode,
    );
    assert_eq!(
        loose_claim["work"],
        json!({
            "workflow": "loose",
            "source_inventory_balance_id": loose_stock.inventory_balance_id,
            "item_batch_id": loose_stock.item_batch_id,
            "item_id": item_id,
            "item_description": item_name,
            "uom": "case",
            "lot": loose_stock.lot,
            "serial": null,
            "expiration": null,
            "inventory_status": "available",
            "quantity": 4,
        })
    );

    let before_current = side_effect_snapshot(&fixture.db, tenant_id).await;
    let current = send(
        &app,
        &token,
        tenant_id,
        Method::GET,
        "/api/v1/putaway-claims/current",
        None,
        None,
    )
    .await;
    assert_eq!(current.status(), StatusCode::OK);
    assert_eq!(response_json::<Value>(current).await, loose_claim);
    assert_eq!(
        side_effect_snapshot(&fixture.db, tenant_id).await,
        before_current
    );

    let replay = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/v1/putaway-claims/next",
        Some("putaway-claim-loose-next"),
        Some(json!({"workflow": "loose"})),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(response_json::<Value>(replay).await, loose_claim);
    assert_eq!(
        side_effect_snapshot(&fixture.db, tenant_id).await,
        before_current
    );
    let changed_replay = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/v1/putaway-claims/next",
        Some("putaway-claim-loose-next"),
        Some(json!({"workflow": "license_plate"})),
    )
    .await;
    assert_error(
        changed_replay,
        StatusCode::CONFLICT,
        ErrorReason::IdempotencyKeyReused,
    )
    .await;
    assert_eq!(
        side_effect_snapshot(&fixture.db, tenant_id).await,
        before_current
    );
    assert!(
        repo::tasks::cancel_task(&fixture.db, tenant_id, loose_task, user.id)
            .await
            .unwrap()
    );

    let open_plate_stock = receive_stock(
        &fixture,
        &access,
        inventory_owner_id,
        facility_id,
        source_location_id,
        item_id,
        "PUTAWAY-CLAIM-LP-OPEN",
        true,
    )
    .await;
    let open_plate_task = create_plate_task(
        &fixture,
        &access,
        &open_plate_stock,
        destination_location_id,
        100,
        None,
        "putaway-claim-lp-open-create",
        "Open license plate work",
    )
    .await;
    let assigned_plate_stock = receive_stock(
        &fixture,
        &access,
        inventory_owner_id,
        facility_id,
        source_location_id,
        item_id,
        "PUTAWAY-CLAIM-LP-ASSIGNED",
        true,
    )
    .await;
    let assigned_plate_instructions = "Start assigned license plate work first";
    let assigned_plate_task = create_plate_task(
        &fixture,
        &access,
        &assigned_plate_stock,
        destination_location_id,
        10,
        Some(user.id),
        "putaway-claim-lp-assigned-create",
        assigned_plate_instructions,
    )
    .await;
    let assigned_claim = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/v1/putaway-claims/next",
        Some("putaway-claim-lp-next"),
        Some(json!({"workflow": "license_plate"})),
    )
    .await;
    assert_eq!(assigned_claim.status(), StatusCode::OK);
    let assigned_claim: Value = response_json(assigned_claim).await;
    assert_common_claim(
        &assigned_claim,
        assigned_plate_task,
        inventory_owner_id,
        facility_id,
        10,
        assigned_plate_instructions,
        source_location_id,
        source_barcode,
        source_name,
        destination_location_id,
        destination_barcode,
        destination_barcode,
    );
    assert_eq!(
        assigned_claim["work"],
        json!({
            "workflow": "license_plate",
            "license_plate_id": assigned_plate_stock.license_plate_id.unwrap(),
            "license_plate_barcode": assigned_plate_stock.license_plate_barcode.unwrap(),
            "planned_balance_count": 1,
        })
    );
    assert!(
        repo::tasks::cancel_task(&fixture.db, tenant_id, assigned_plate_task, user.id,)
            .await
            .unwrap()
    );
    assert!(
        repo::tasks::cancel_task(&fixture.db, tenant_id, open_plate_task, user.id)
            .await
            .unwrap()
    );
    let no_work = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/v1/putaway-claims/next",
        Some("putaway-claim-lp-none"),
        Some(json!({"workflow": "license_plate"})),
    )
    .await;
    assert_eq!(no_work.status(), StatusCode::OK);
    assert!(response_json::<Value>(no_work).await.is_null());

    let stale_plate_stock = receive_stock(
        &fixture,
        &access,
        inventory_owner_id,
        facility_id,
        source_location_id,
        item_id,
        "PUTAWAY-CLAIM-LP-STALE",
        true,
    )
    .await;
    let stale_plate_task = create_plate_task(
        &fixture,
        &access,
        &stale_plate_stock,
        destination_location_id,
        20,
        None,
        "putaway-claim-lp-stale-create",
        "Reject a stale license plate plan",
    )
    .await;
    receive_additional_plate_stock(
        &fixture,
        &access,
        inventory_owner_id,
        facility_id,
        source_location_id,
        item_id,
        stale_plate_stock.license_plate_id.unwrap(),
        &stale_plate_stock.lot,
        "PUTAWAY-CLAIM-LP-STALE-ADDITIONAL",
    )
    .await;
    let before_stale_claim = side_effect_snapshot(&fixture.db, tenant_id).await;
    let stale_claim = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        &format!("/api/v1/putaway-claims/{stale_plate_task}"),
        Some("putaway-claim-stale-selected"),
        Some(json!({})),
    )
    .await;
    assert_error(stale_claim, StatusCode::CONFLICT, ErrorReason::Conflict).await;
    assert_eq!(
        side_effect_snapshot(&fixture.db, tenant_id).await,
        before_stale_claim
    );
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let stale_task_state: (String, Option<i64>, bool, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            task.status,
            task.assigned_user_id,
            task.lease_expires_at IS NULL,
            (
                SELECT COUNT(*)
                FROM work_task_progress progress
                WHERE progress.tenant_id = task.tenant_id
                  AND progress.task_id = task.id
                  AND progress.action = 'started'
            ),
            (
                SELECT COUNT(*)
                FROM command_idempotency_records command
                WHERE command.tenant_id = task.tenant_id
                  AND command.operation = 'putaway.claim_by_id.v1'
                  AND command.idempotency_key = 'putaway-claim-stale-selected'
            )
        FROM work_tasks task
        WHERE task.tenant_id = $1 AND task.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(stale_plate_task)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(stale_task_state, ("open".to_owned(), None, true, 0, 0));
    assert!(
        repo::tasks::cancel_task(&fixture.db, tenant_id, stale_plate_task, user.id)
            .await
            .unwrap()
    );

    let cycle_location = fixture
        .location(tenant_id, facility_id, "PUTAWAY-CLAIM-CYCLE")
        .await;
    let cycle_task = repo::tasks::create_location_cycle_count_task(
        &fixture.db,
        tenant_id,
        user.id,
        cycle_location,
        None,
        None,
        None,
        None,
        Some("cross-type claim guard".to_owned()),
    )
    .await
    .unwrap();
    let denied_stock = receive_stock(
        &fixture,
        &access,
        denied_owner_id,
        facility_id,
        source_location_id,
        item_id,
        "PUTAWAY-CLAIM-DENIED",
        false,
    )
    .await;
    let denied_task = create_loose_task(
        &fixture,
        &access,
        &denied_stock,
        destination_location_id,
        2,
        50,
        None,
        "putaway-claim-denied-create",
        "Hidden owner work",
    )
    .await;
    set_scope(
        &fixture.db,
        tenant_id,
        user.id,
        vec![facility_id],
        vec![inventory_owner_id],
    )
    .await;
    let before_hidden = side_effect_snapshot(&fixture.db, tenant_id).await;
    for task_id in [cycle_task, denied_task] {
        let hidden = send(
            &app,
            &token,
            tenant_id,
            Method::POST,
            &format!("/api/v1/putaway-claims/{task_id}"),
            Some(&format!("putaway-claim-hidden-{task_id}")),
            Some(json!({})),
        )
        .await;
        assert_error(hidden, StatusCode::NOT_FOUND, ErrorReason::NotFound).await;
    }
    assert_eq!(
        side_effect_snapshot(&fixture.db, tenant_id).await,
        before_hidden
    );

    assert!(repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: user.id,
            all_facilities: false,
            facility_ids: vec![facility_id],
            all_inventory_owners: true,
            inventory_owner_ids: Vec::new(),
        },
    )
    .await
    .unwrap());
    assert!(repo::tasks::start_task_in_scope(
        &fixture.db,
        &access,
        &command(&access, "putaway-claim-cycle-start"),
        cycle_task,
    )
    .await
    .unwrap());
    let active_target_stock = receive_stock(
        &fixture,
        &access,
        inventory_owner_id,
        facility_id,
        source_location_id,
        item_id,
        "PUTAWAY-CLAIM-ACTIVE-TARGET",
        false,
    )
    .await;
    let active_target = create_loose_task(
        &fixture,
        &access,
        &active_target_stock,
        destination_location_id,
        3,
        80,
        None,
        "putaway-claim-active-target-create",
        "Blocked by another active workflow",
    )
    .await;
    let before_active_conflicts = side_effect_snapshot(&fixture.db, tenant_id).await;
    for (uri, body, key) in [
        (
            "/api/v1/putaway-claims/next".to_owned(),
            json!({"workflow": "loose"}),
            "putaway-claim-active-next",
        ),
        (
            format!("/api/v1/putaway-claims/{active_target}"),
            json!({}),
            "putaway-claim-active-selected",
        ),
    ] {
        let blocked = send(
            &app,
            &token,
            tenant_id,
            Method::POST,
            &uri,
            Some(key),
            Some(body),
        )
        .await;
        assert_error(blocked, StatusCode::CONFLICT, ErrorReason::Conflict).await;
    }
    assert_eq!(
        side_effect_snapshot(&fixture.db, tenant_id).await,
        before_active_conflicts
    );
    assert!(
        repo::tasks::cancel_task(&fixture.db, tenant_id, cycle_task, user.id)
            .await
            .unwrap()
    );

    let scope_claim = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/v1/putaway-claims/next",
        Some("putaway-claim-scope-next"),
        Some(json!({"workflow": "loose"})),
    )
    .await;
    assert_eq!(scope_claim.status(), StatusCode::OK);
    let scope_claim: Value = response_json(scope_claim).await;
    assert_eq!(scope_claim["task_id"], active_target);
    set_scope(
        &fixture.db,
        tenant_id,
        user.id,
        vec![facility_id],
        Vec::new(),
    )
    .await;
    let after_scope_release = side_effect_snapshot(&fixture.db, tenant_id).await;
    let concealed_replay = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/v1/putaway-claims/next",
        Some("putaway-claim-scope-next"),
        Some(json!({"workflow": "loose"})),
    )
    .await;
    assert_error(
        concealed_replay,
        StatusCode::NOT_FOUND,
        ErrorReason::NotFound,
    )
    .await;
    let concealed_current = send(
        &app,
        &token,
        tenant_id,
        Method::GET,
        "/api/v1/putaway-claims/current",
        None,
        None,
    )
    .await;
    assert_eq!(concealed_current.status(), StatusCode::OK);
    assert!(response_json::<Value>(concealed_current).await.is_null());
    assert_eq!(
        side_effect_snapshot(&fixture.db, tenant_id).await,
        after_scope_release
    );

    set_scope(
        &fixture.db,
        tenant_id,
        user.id,
        vec![facility_id],
        vec![inventory_owner_id],
    )
    .await;
    let reclaimed = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        &format!("/api/v1/putaway-claims/{active_target}"),
        Some("putaway-claim-scope-reclaim"),
        Some(json!({})),
    )
    .await;
    assert_eq!(reclaimed.status(), StatusCode::OK);
    assert_eq!(
        response_json::<Value>(reclaimed).await["task_id"],
        active_target
    );
    let before_expiry = task_counters(&fixture.db, tenant_id, active_target).await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        r#"
        UPDATE work_tasks
        SET lease_expires_at = statement_timestamp() - INTERVAL '1 second'
        WHERE tenant_id = $1 AND id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(active_target)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let after_forced_expiry = side_effect_snapshot(&fixture.db, tenant_id).await;
    let expired_current = send(
        &app,
        &token,
        tenant_id,
        Method::GET,
        "/api/v1/putaway-claims/current",
        None,
        None,
    )
    .await;
    assert_eq!(expired_current.status(), StatusCode::OK);
    assert!(response_json::<Value>(expired_current).await.is_null());
    assert_eq!(
        side_effect_snapshot(&fixture.db, tenant_id).await,
        after_forced_expiry
    );
    let expiry_reclaim = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        &format!("/api/v1/putaway-claims/{active_target}"),
        Some("putaway-claim-expiry-reclaim"),
        Some(json!({})),
    )
    .await;
    assert_eq!(expiry_reclaim.status(), StatusCode::OK);
    assert_eq!(
        response_json::<Value>(expiry_reclaim).await["task_id"],
        active_target
    );
    let after_expiry = task_counters(&fixture.db, tenant_id, active_target).await;
    assert_eq!(after_expiry.release_count, before_expiry.release_count + 1);
    assert_eq!(after_expiry.expired, before_expiry.expired + 1);
    assert_eq!(after_expiry.started, before_expiry.started + 1);
    assert_eq!(after_expiry.scope_revoked, before_expiry.scope_revoked);
    assert_eq!(
        repo::inventory::get_reconciliation_issues(&fixture.db, tenant_id)
            .await
            .unwrap(),
        Vec::new()
    );
}
