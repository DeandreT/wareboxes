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
    CreateLicensePlatePutawayTaskResponse, ErrorReason, ErrorResponse,
    LicensePlatePutawayConfirmationResponse, PutawayCandidatePage, PutawayWorkPage,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{
    InboundReceiptExceptionReason, ReceiveExpectedInventoryResult, TenantAccess,
};

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct MoveEffects {
    plate_location_id: Option<i64>,
    source_balance_count: i64,
    destination_balance_count: i64,
    transaction_count: i64,
    entry_count: i64,
    projection_change_count: i64,
    result_count: i64,
    confirmation_progress_count: i64,
    confirmation_outbox_count: i64,
    confirmation_command_count: i64,
    task_status: String,
}

fn command(access: &TenantAccess, key: &str) -> CommandContext {
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
    app.clone()
        .oneshot(request(token, tenant_id, uri, idempotency_key, body))
        .await
        .unwrap()
}

async fn get(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    uri: &str,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, tenant_id.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[allow(clippy::too_many_arguments)]
async fn receive_line(
    fixture: &Fixture,
    access: &TenantAccess,
    load_line_id: i64,
    receiving_location_id: i64,
    quantity: i64,
    license_plate_id: Option<i64>,
    license_plate_barcode: Option<&str>,
    lot: &str,
    key: &str,
) -> ReceiveExpectedInventoryResult {
    repo::inbound_receipt::receive_expected_inventory(
        &fixture.db,
        access,
        &command(access, key),
        load_line_id,
        &repo::inbound_receipt::ReceiveExpectedInventoryCommand {
            receiving_location_id: Some(receiving_location_id),
            received_qty: quantity,
            rejected_qty: 0,
            missing_qty: 0,
            license_plate_id,
            license_plate_barcode,
            lot: Some(lot),
            serial: None,
            expiration: None,
            exception_reason: None::<InboundReceiptExceptionReason>,
            exception_note: None,
        },
    )
    .await
    .unwrap()
}

async fn move_effects(
    fixture: &Fixture,
    tenant_id: TenantId,
    task_id: i64,
    license_plate_id: i64,
    source_location_id: i64,
    destination_location_id: i64,
) -> MoveEffects {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let effects = sqlx::query_as(
        r#"
        SELECT
            (
                SELECT location_id
                FROM license_plates
                WHERE tenant_id = $1 AND id = $3
            ) AS plate_location_id,
            (
                SELECT COUNT(*)
                FROM inventory_balances
                WHERE tenant_id = $1
                  AND license_plate_id = $3
                  AND location_id = $4
                  AND deleted IS NULL
                  AND qty_on_hand > 0
            ) AS source_balance_count,
            (
                SELECT COUNT(*)
                FROM inventory_balances
                WHERE tenant_id = $1
                  AND license_plate_id = $3
                  AND location_id = $5
                  AND deleted IS NULL
                  AND qty_on_hand > 0
            ) AS destination_balance_count,
            (
                SELECT COUNT(*)
                FROM inventory_transactions
                WHERE tenant_id = $1
                  AND operation = 'task.confirm_license_plate_putaway.v1'
                  AND reference_type = 'license_plate_putaway_task'
                  AND reference_id = $2
            ) AS transaction_count,
            (
                SELECT COUNT(*)
                FROM inventory_entries entry
                INNER JOIN inventory_transactions transaction
                  ON transaction.tenant_id = entry.tenant_id
                 AND transaction.inventory_owner_id = entry.inventory_owner_id
                 AND transaction.id = entry.transaction_id
                WHERE transaction.tenant_id = $1
                  AND transaction.operation =
                      'task.confirm_license_plate_putaway.v1'
                  AND transaction.reference_type =
                      'license_plate_putaway_task'
                  AND transaction.reference_id = $2
            ) AS entry_count,
            (
                SELECT COUNT(*)
                FROM inventory_projection_changes change
                INNER JOIN inventory_transactions transaction
                  ON transaction.tenant_id = change.tenant_id
                 AND transaction.inventory_owner_id =
                     change.inventory_owner_id
                 AND transaction.id = change.transaction_id
                WHERE transaction.tenant_id = $1
                  AND transaction.operation =
                      'task.confirm_license_plate_putaway.v1'
                  AND transaction.reference_type =
                      'license_plate_putaway_task'
                  AND transaction.reference_id = $2
            ) AS projection_change_count,
            (
                SELECT COUNT(*)
                FROM license_plate_putaway_results
                WHERE tenant_id = $1 AND task_id = $2
            ) AS result_count,
            (
                SELECT COUNT(*)
                FROM work_task_progress
                WHERE tenant_id = $1
                  AND task_id = $2
                  AND action = 'license_plate_putaway_confirmed'
            ) AS confirmation_progress_count,
            (
                SELECT COUNT(*)
                FROM outbox_events
                WHERE tenant_id = $1
                  AND event_type =
                      'inventory.license_plate_putaway.confirmed'
                  AND aggregate_id = $2::TEXT
            ) AS confirmation_outbox_count,
            (
                SELECT COUNT(*)
                FROM command_idempotency_records
                WHERE tenant_id = $1
                  AND operation =
                      'task.confirm_license_plate_putaway.v1'
            ) AS confirmation_command_count,
            (
                SELECT status
                FROM work_tasks
                WHERE tenant_id = $1 AND id = $2
            ) AS task_status
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .bind(license_plate_id)
    .bind(source_location_id)
    .bind(destination_location_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    effects
}

async fn assert_conflict(response: axum::response::Response) {
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(response).await.reason,
        ErrorReason::Conflict
    );
}

#[tokio::test]
async fn whole_license_plate_putaway_is_atomic_replay_safe_and_rejects_content_drift() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("license-plate-putaway@test.local").await;
    let access = default_tenant_for_user(&fixture.db, user.id)
        .await
        .expect("WMS user has tenant access");
    let tenant_id = access.tenant_id;
    let facility_id = fixture
        .facility(tenant_id, "License Plate Putaway DC")
        .await;
    let inventory_owner_id = fixture
        .inventory_owner(tenant_id, "License Plate Putaway Owner")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let receiving_location_id = wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some("LP-PUTAWAY-RECEIVING"),
        Some("License Plate Putaway Receiving"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let destination_barcode = "LP-PUTAWAY-A-01";
    let destination_location_id = fixture
        .location(tenant_id, facility_id, destination_barcode)
        .await;
    let wrong_destination_barcode = "LP-PUTAWAY-A-02";
    fixture
        .location(tenant_id, facility_id, wrong_destination_barcode)
        .await;

    let item_ids = [
        fixture.item(tenant_id, "LP Putaway Item A", "case").await,
        fixture.item(tenant_id, "LP Putaway Item B", "each").await,
        fixture
            .item(tenant_id, "LP Putaway Drift Item A", "case")
            .await,
        fixture
            .item(tenant_id, "LP Putaway Drift Item B", "each")
            .await,
    ];
    let quantities = [5_i64, 7, 11, 13];
    let lots = [
        "LP-PUTAWAY-LOT-A",
        "LP-PUTAWAY-LOT-B",
        "LP-PUTAWAY-DRIFT-LOT-A",
        "LP-PUTAWAY-DRIFT-LOT-B",
    ];
    let load_id = repo::loads::add_load(
        &fixture.db,
        tenant_id,
        user.id,
        facility_id,
        inventory_owner_id,
        LoadType::Inbound,
        Some("LP-PUTAWAY-INBOUND"),
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
    let mut load_line_ids = Vec::new();
    for ((item_id, quantity), lot) in item_ids.into_iter().zip(quantities).zip(lots) {
        load_line_ids.push(
            repo::loads::add_line(
                &fixture.db,
                tenant_id,
                user.id,
                load_id,
                item_id,
                None,
                quantity,
                Some(lot),
                None,
                None,
            )
            .await
            .unwrap(),
        );
    }
    assert!(repo::loads::update_load(
        &fixture.db,
        tenant_id,
        user.id,
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

    let plate_barcode = "LP-PUTAWAY-MAIN";
    let first_receipt = receive_line(
        &fixture,
        &access,
        load_line_ids[0],
        receiving_location_id,
        quantities[0],
        None,
        Some(plate_barcode),
        lots[0],
        "license-plate-putaway-receipt-a",
    )
    .await;
    let license_plate_id = first_receipt
        .license_plate_id
        .expect("container receipt creates a license plate");
    let first_balance_id = first_receipt
        .inventory_balance_id
        .expect("physical receipt identifies its balance");
    let second_receipt = receive_line(
        &fixture,
        &access,
        load_line_ids[1],
        receiving_location_id,
        quantities[1],
        Some(license_plate_id),
        None,
        lots[1],
        "license-plate-putaway-receipt-b",
    )
    .await;
    let second_balance_id = second_receipt
        .inventory_balance_id
        .expect("physical receipt identifies its balance");
    assert_ne!(first_balance_id, second_balance_id);

    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let candidates = get(
        &app,
        &token,
        tenant_id,
        "/api/v1/putaway-candidates?limit=100&workflow=license_plate&sort=quantity&direction=desc",
    )
    .await;
    assert_eq!(candidates.status(), StatusCode::OK);
    let candidates: PutawayCandidatePage = response_json(candidates).await;
    let candidate = candidates
        .items
        .iter()
        .find(|candidate| candidate.license_plate_id == Some(license_plate_id))
        .expect("received plate is eligible for putaway");
    assert_eq!(candidate.balance_count, 2);
    assert_eq!(candidate.item_count, 2);
    assert_eq!(candidate.available_quantity, quantities[0] + quantities[1]);

    let create_body = json!({
        "license_plate_id": license_plate_id,
        "destination_location_id": destination_location_id,
        "priority": 80,
        "assigned_user_id": user.id,
        "instructions": "Scan the whole license plate and directed location"
    });
    let created = send(
        &app,
        &token,
        tenant_id,
        "/api/v1/license-plate-putaway-tasks",
        "license-plate-putaway-create",
        create_body.clone(),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    let created: CreateLicensePlatePutawayTaskResponse = response_json(created).await;
    assert!(created.task_id > 0);
    let work = get(
        &app,
        &token,
        tenant_id,
        "/api/v1/putaway-tasks?limit=100&workflow=license_plate&sort=priority&direction=desc",
    )
    .await;
    assert_eq!(work.status(), StatusCode::OK);
    let work: PutawayWorkPage = response_json(work).await;
    let work = work
        .items
        .iter()
        .find(|work| work.task_id == created.task_id)
        .expect("planned plate task is visible");
    assert_eq!(work.balance_count, 2);
    assert_eq!(work.planned_quantity, quantities[0] + quantities[1]);

    let replayed_create = send(
        &app,
        &token,
        tenant_id,
        "/api/v1/license-plate-putaway-tasks",
        "license-plate-putaway-create",
        create_body,
    )
    .await;
    assert_eq!(replayed_create.status(), StatusCode::OK);
    assert_eq!(
        response_json::<CreateLicensePlatePutawayTaskResponse>(replayed_create).await,
        created
    );
    let changed_create = send(
        &app,
        &token,
        tenant_id,
        "/api/v1/license-plate-putaway-tasks",
        "license-plate-putaway-create",
        json!({
            "license_plate_id": license_plate_id,
            "destination_location_id": destination_location_id,
            "priority": 79,
            "assigned_user_id": user.id,
            "instructions": "Scan the whole license plate and directed location"
        }),
    )
    .await;
    assert_eq!(changed_create.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed_create).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let started = send(
        &app,
        &token,
        tenant_id,
        "/api/tasks/start",
        "license-plate-putaway-start",
        json!({"task_id": created.task_id}),
    )
    .await;
    assert_eq!(started.status(), StatusCode::OK);
    assert!(response_json::<bool>(started).await);

    let confirmation_uri = format!(
        "/api/v1/license-plate-putaway-tasks/{}/confirmations",
        created.task_id
    );
    let effects_before_scans = move_effects(
        &fixture,
        tenant_id,
        created.task_id,
        license_plate_id,
        receiving_location_id,
        destination_location_id,
    )
    .await;
    assert_conflict(
        send(
            &app,
            &token,
            tenant_id,
            &confirmation_uri,
            "license-plate-putaway-wrong-plate",
            json!({
                "license_plate_barcode": "LP-PUTAWAY-WRONG",
                "destination_location_barcode": destination_barcode
            }),
        )
        .await,
    )
    .await;
    assert_eq!(
        move_effects(
            &fixture,
            tenant_id,
            created.task_id,
            license_plate_id,
            receiving_location_id,
            destination_location_id,
        )
        .await,
        effects_before_scans
    );
    assert_conflict(
        send(
            &app,
            &token,
            tenant_id,
            &confirmation_uri,
            "license-plate-putaway-wrong-destination",
            json!({
                "license_plate_barcode": plate_barcode,
                "destination_location_barcode": wrong_destination_barcode
            }),
        )
        .await,
    )
    .await;
    assert_eq!(
        move_effects(
            &fixture,
            tenant_id,
            created.task_id,
            license_plate_id,
            receiving_location_id,
            destination_location_id,
        )
        .await,
        effects_before_scans
    );

    let confirmation_body = json!({
        "license_plate_barcode": plate_barcode,
        "destination_location_barcode": destination_barcode
    });
    let confirmed = send(
        &app,
        &token,
        tenant_id,
        &confirmation_uri,
        "license-plate-putaway-confirm",
        confirmation_body.clone(),
    )
    .await;
    assert_eq!(confirmed.status(), StatusCode::OK);
    let confirmed: LicensePlatePutawayConfirmationResponse = response_json(confirmed).await;
    assert_eq!(confirmed.task_id, created.task_id);
    assert_eq!(confirmed.license_plate_id, license_plate_id);
    assert_eq!(confirmed.license_plate_barcode, plate_barcode);
    assert_eq!(confirmed.inventory_owner_id, inventory_owner_id);
    assert_eq!(confirmed.facility_id, facility_id);
    assert_eq!(confirmed.source_location_id, receiving_location_id);
    assert_eq!(confirmed.destination_location_id, destination_location_id);
    assert_eq!(confirmed.destination_location_barcode, destination_barcode);
    assert_eq!(confirmed.moved_balance_count, 2);
    assert_eq!(confirmed.confirmed_by, user.id);

    let replayed_confirmation = send(
        &app,
        &token,
        tenant_id,
        &confirmation_uri,
        "license-plate-putaway-confirm",
        confirmation_body,
    )
    .await;
    assert_eq!(replayed_confirmation.status(), StatusCode::OK);
    assert_eq!(
        response_json::<LicensePlatePutawayConfirmationResponse>(replayed_confirmation).await,
        confirmed
    );

    let effects_after_confirmation = move_effects(
        &fixture,
        tenant_id,
        created.task_id,
        license_plate_id,
        receiving_location_id,
        destination_location_id,
    )
    .await;
    assert_eq!(
        effects_after_confirmation,
        MoveEffects {
            plate_location_id: Some(destination_location_id),
            source_balance_count: 0,
            destination_balance_count: 2,
            transaction_count: 1,
            entry_count: 4,
            projection_change_count: 4,
            result_count: 1,
            confirmation_progress_count: 1,
            confirmation_outbox_count: 1,
            confirmation_command_count: 1,
            task_status: "completed".to_owned(),
        }
    );

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let balances: Vec<(i64, i64, i64)> = sqlx::query_as(
        r#"
        SELECT id, location_id, qty_on_hand
        FROM inventory_balances
        WHERE tenant_id = $1
          AND id = ANY($2)
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(vec![first_balance_id, second_balance_id])
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    let mut expected_balances = vec![
        (first_balance_id, destination_location_id, quantities[0]),
        (second_balance_id, destination_location_id, quantities[1]),
    ];
    expected_balances.sort_unstable();
    assert_eq!(balances, expected_balances);

    let snapshot: Vec<(i64, i64, String, i64)> = sqlx::query_as(
        r#"
        SELECT inventory_balance_id, item_id, inventory_status,
               planned_quantity
        FROM license_plate_putaway_task_contents
        WHERE tenant_id = $1 AND task_id = $2
        ORDER BY inventory_balance_id
        "#,
    )
    .bind(tenant_id.get())
    .bind(created.task_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(snapshot.len(), 2);
    assert_eq!(
        snapshot
            .iter()
            .map(|row| (row.0, row.2.as_str(), row.3))
            .collect::<Vec<_>>(),
        expected_balances
            .iter()
            .map(|row| (row.0, "available", row.2))
            .collect::<Vec<_>>()
    );

    let transaction_entries: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            COUNT(*),
            COALESCE(SUM(quantity_delta), 0)::BIGINT,
            COUNT(*) FILTER (
                WHERE location_id = $2 AND quantity_delta < 0
            ),
            COUNT(*) FILTER (
                WHERE location_id = $3 AND quantity_delta > 0
            )
        FROM inventory_entries
        WHERE tenant_id = $1 AND transaction_id = $4
        "#,
    )
    .bind(tenant_id.get())
    .bind(receiving_location_id)
    .bind(destination_location_id)
    .bind(confirmed.inventory_transaction_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(transaction_entries, (4, 0, 2, 2));

    let task_state: (String, Option<i64>, Option<i64>, bool, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            task.status,
            task.completed_by,
            result.inventory_transaction_id,
            detail.closed_at IS NOT NULL,
            (
                SELECT COUNT(*)
                FROM work_task_progress progress
                WHERE progress.tenant_id = task.tenant_id
                  AND progress.task_id = task.id
                  AND progress.action = 'started'
            ),
            (
                SELECT COUNT(*)
                FROM work_task_progress progress
                WHERE progress.tenant_id = task.tenant_id
                  AND progress.task_id = task.id
                  AND progress.action =
                      'license_plate_putaway_confirmed'
            )
        FROM work_tasks task
        INNER JOIN license_plate_putaway_tasks detail
          ON detail.tenant_id = task.tenant_id
         AND detail.task_id = task.id
        INNER JOIN license_plate_putaway_results result
          ON result.tenant_id = task.tenant_id
         AND result.task_id = task.id
        WHERE task.tenant_id = $1 AND task.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(created.task_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        task_state,
        (
            "completed".to_owned(),
            Some(user.id),
            Some(confirmed.inventory_transaction_id),
            true,
            1,
            1,
        )
    );

    let outbox_events: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            COUNT(*) FILTER (
                WHERE event_type = 'inbound.expected_receipt.confirmed'
                  AND payload ->> 'license_plate_id' = $2::TEXT
            ),
            COUNT(*) FILTER (
                WHERE event_type =
                    'inventory.license_plate_putaway.confirmed'
                  AND aggregate_id = $3::TEXT
            )
        FROM outbox_events
        WHERE tenant_id = $1
        "#,
    )
    .bind(tenant_id.get())
    .bind(license_plate_id)
    .bind(created.task_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(outbox_events, (2, 1));

    let idempotency: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            COUNT(*),
            COUNT(*) FILTER (
                WHERE operation =
                    'task.confirm_license_plate_putaway.v1'
                  AND inventory_transaction_id = $2
            )
        FROM command_idempotency_records
        WHERE tenant_id = $1
          AND operation IN (
              'task.create_license_plate_putaway.v1',
              'task.confirm_license_plate_putaway.v1'
          )
        "#,
    )
    .bind(tenant_id.get())
    .bind(confirmed.inventory_transaction_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(idempotency, (2, 1));
    let reconciliation_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM inventory_reconciliation WHERE tenant_id = $1")
            .bind(tenant_id.get())
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(reconciliation_count, 0);
    tx.rollback().await.unwrap();

    let admin_db = admin_db_for(&fixture.db).await;
    let snapshot_mutation = sqlx::query(
        r#"
        UPDATE license_plate_putaway_task_contents
        SET planned_quantity = planned_quantity
        WHERE tenant_id = $1 AND task_id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(created.task_id)
    .execute(&admin_db)
    .await
    .unwrap_err();
    assert_eq!(
        snapshot_mutation
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .as_deref(),
        Some("55000")
    );
    let result_mutation = sqlx::query(
        r#"
        UPDATE license_plate_putaway_results
        SET moved_balance_count = moved_balance_count
        WHERE tenant_id = $1 AND task_id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(created.task_id)
    .execute(&admin_db)
    .await
    .unwrap_err();
    assert_eq!(
        result_mutation
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .as_deref(),
        Some("55000")
    );
    admin_db.close().await;

    let drift_plate_barcode = "LP-PUTAWAY-DRIFT";
    let drift_receipt = receive_line(
        &fixture,
        &access,
        load_line_ids[2],
        receiving_location_id,
        quantities[2],
        None,
        Some(drift_plate_barcode),
        lots[2],
        "license-plate-putaway-drift-receipt-a",
    )
    .await;
    let drift_plate_id = drift_receipt
        .license_plate_id
        .expect("container receipt creates the drift license plate");
    let drift_created = send(
        &app,
        &token,
        tenant_id,
        "/api/v1/license-plate-putaway-tasks",
        "license-plate-putaway-drift-create",
        json!({
            "license_plate_id": drift_plate_id,
            "destination_location_id": destination_location_id,
            "assigned_user_id": user.id,
        }),
    )
    .await;
    assert_eq!(drift_created.status(), StatusCode::OK);
    let drift_created: CreateLicensePlatePutawayTaskResponse = response_json(drift_created).await;
    let drift_started = send(
        &app,
        &token,
        tenant_id,
        "/api/tasks/start",
        "license-plate-putaway-drift-start",
        json!({"task_id": drift_created.task_id}),
    )
    .await;
    assert_eq!(drift_started.status(), StatusCode::OK);
    assert!(response_json::<bool>(drift_started).await);

    let added_drift_receipt = receive_line(
        &fixture,
        &access,
        load_line_ids[3],
        receiving_location_id,
        quantities[3],
        Some(drift_plate_id),
        None,
        lots[3],
        "license-plate-putaway-drift-receipt-b",
    )
    .await;
    assert!(
        added_drift_receipt.inventory_balance_id.is_some(),
        "the additional receipt adds a positive balance after planning"
    );
    let drift_effects_before_confirmation = move_effects(
        &fixture,
        tenant_id,
        drift_created.task_id,
        drift_plate_id,
        receiving_location_id,
        destination_location_id,
    )
    .await;
    assert_eq!(drift_effects_before_confirmation.source_balance_count, 2);
    assert_eq!(
        drift_effects_before_confirmation.destination_balance_count,
        0
    );
    assert_eq!(drift_effects_before_confirmation.task_status, "in_progress");

    assert_conflict(
        send(
            &app,
            &token,
            tenant_id,
            &format!(
                "/api/v1/license-plate-putaway-tasks/{}/confirmations",
                drift_created.task_id
            ),
            "license-plate-putaway-drift-confirm",
            json!({
                "license_plate_barcode": drift_plate_barcode,
                "destination_location_barcode": destination_barcode,
            }),
        )
        .await,
    )
    .await;
    assert_eq!(
        move_effects(
            &fixture,
            tenant_id,
            drift_created.task_id,
            drift_plate_id,
            receiving_location_id,
            destination_location_id,
        )
        .await,
        drift_effects_before_confirmation
    );

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let drift_counts: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (
                SELECT COUNT(*)
                FROM license_plate_putaway_task_contents
                WHERE tenant_id = $1 AND task_id = $2
            ),
            (
                SELECT COUNT(*)
                FROM inventory_balances
                WHERE tenant_id = $1
                  AND license_plate_id = $3
                  AND location_id = $4
                  AND deleted IS NULL
                  AND qty_on_hand > 0
            ),
            (
                SELECT COUNT(*)
                FROM inventory_reconciliation
                WHERE tenant_id = $1
            )
        "#,
    )
    .bind(tenant_id.get())
    .bind(drift_created.task_id)
    .bind(drift_plate_id)
    .bind(receiving_location_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(drift_counts, (1, 2, 0));
}
