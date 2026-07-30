mod common;

use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tokio::time::timeout;
use tower::ServiceExt;
use wareboxes_api_contract::v1::{
    ErrorReason, ErrorResponse, PutawayClaimHeartbeatResponse, PutawayClaimReleaseReason,
    PutawayClaimReleaseResponse, PutawayClaimResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_core::models::{
    InboundReceiptExceptionReason, ReceiveExpectedInventoryResult, TenantAccess,
};
use wareboxes_domain::CommandContext;
use wareboxes_server::auth::TENANT_ID_HEADER;
use wareboxes_server::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_server::{routes, state::AppState};

const OPERATION_TIMEOUT: Duration = Duration::from_secs(3);
const RECEIPT_LOT: &str = "PUTAWAY-LIFECYCLE-LOT";

struct LifecycleContext {
    fixture: Fixture,
    access: TenantAccess,
    token: String,
    app: axum::Router,
    facility_id: i64,
    inventory_owner_id: i64,
    denied_inventory_owner_id: i64,
    source_location_id: i64,
    destination_location_id: i64,
    destination_barcode: String,
    item_id: i64,
}

#[derive(Debug, Clone)]
struct ReceivedStock {
    inventory_balance_id: i64,
    license_plate_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct LifecycleEffects {
    status: String,
    assigned_user_id: Option<i64>,
    started_at_is_null: bool,
    lease_expires_at_is_null: bool,
    lease_is_current: Option<bool>,
    release_count: i64,
    heartbeat_progress_count: i64,
    release_progress_count: i64,
    heartbeat_command_count: i64,
    release_command_count: i64,
    inventory_transaction_count: i64,
    inventory_entry_count: i64,
    outbox_count: i64,
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
    idempotency_key: Option<&str>,
    body: Value,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(idempotency_key) = idempotency_key {
        request = request.header(IDEMPOTENCY_KEY_HEADER, idempotency_key);
    }
    request.body(Body::from(body.to_string())).unwrap()
}

async fn send(
    context: &LifecycleContext,
    uri: &str,
    idempotency_key: Option<&str>,
    body: Value,
) -> axum::response::Response {
    timeout(
        OPERATION_TIMEOUT,
        context.app.clone().oneshot(request(
            &context.token,
            context.access.tenant_id,
            uri,
            idempotency_key,
            body,
        )),
    )
    .await
    .expect("putaway lifecycle request completes within the bound")
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

fn lifecycle_body(suffix: &str) -> Value {
    if suffix == "releases" {
        json!({ "reason": "work_interrupted" })
    } else {
        json!({})
    }
}

async fn setup_context(email: &str) -> LifecycleContext {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user(email).await;
    let access = default_tenant_for_user(&fixture.db, user.id)
        .await
        .expect("WMS user has tenant access");
    let tenant_id = access.tenant_id;
    let facility_id = fixture
        .facility(tenant_id, "Putaway Lifecycle Facility")
        .await;
    let inventory_owner_id = fixture
        .inventory_owner(tenant_id, "Putaway Lifecycle Owner")
        .await;
    let denied_inventory_owner_id = fixture
        .inventory_owner(tenant_id, "Putaway Lifecycle Denied Owner")
        .await;
    for owner_id in [inventory_owner_id, denied_inventory_owner_id] {
        fixture
            .assign_owner_to_facility(tenant_id, owner_id, facility_id)
            .await;
    }
    let source_location_id = repo::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some("PUTAWAY-LIFECYCLE-RECEIVING"),
        Some("Putaway Lifecycle Receiving"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let destination_barcode = "PUTAWAY-LIFECYCLE-DESTINATION".to_owned();
    let destination_location_id = fixture
        .location(tenant_id, facility_id, &destination_barcode)
        .await;
    let item_id = fixture
        .item(tenant_id, "Putaway Lifecycle Item", "case")
        .await;
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    LifecycleContext {
        fixture,
        access,
        token,
        app,
        facility_id,
        inventory_owner_id,
        denied_inventory_owner_id,
        source_location_id,
        destination_location_id,
        destination_barcode,
        item_id,
    }
}

async fn receive_stock(
    context: &LifecycleContext,
    inventory_owner_id: i64,
    key: &str,
    containerized: bool,
) -> ReceivedStock {
    let load_id = repo::loads::add_load(
        &context.fixture.db,
        context.access.tenant_id,
        context.access.user_id.get(),
        context.facility_id,
        inventory_owner_id,
        LoadType::Inbound,
        Some(key),
        None,
        None,
        None,
        None,
        Some(context.source_location_id),
        None,
        None,
    )
    .await
    .unwrap();
    let load_line_id = repo::loads::add_line(
        &context.fixture.db,
        context.access.tenant_id,
        context.access.user_id.get(),
        load_id,
        context.item_id,
        None,
        10,
        Some(RECEIPT_LOT),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(repo::loads::update_load(
        &context.fixture.db,
        context.access.tenant_id,
        context.access.user_id.get(),
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
    let receipt: ReceiveExpectedInventoryResult =
        repo::inbound_receipt::receive_expected_inventory(
            &context.fixture.db,
            &context.access,
            &command(&context.access, &format!("{key}-receipt")),
            load_line_id,
            &repo::inbound_receipt::ReceiveExpectedInventoryCommand {
                receiving_location_id: Some(context.source_location_id),
                received_qty: 10,
                rejected_qty: 0,
                missing_qty: 0,
                license_plate_id: None,
                license_plate_barcode: containerized.then_some(key),
                lot: Some(RECEIPT_LOT),
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
        license_plate_id: receipt.license_plate_id,
    }
}

async fn create_loose_task(context: &LifecycleContext, stock: &ReceivedStock, key: &str) -> i64 {
    repo::tasks::create_putaway_task_in_scope(
        &context.fixture.db,
        &context.access,
        &command(&context.access, key),
        stock.inventory_balance_id,
        context.destination_location_id,
        4,
        80,
        None,
        None,
        None,
        Some("Lifecycle loose putaway"),
    )
    .await
    .unwrap()
}

async fn create_plate_task(context: &LifecycleContext, stock: &ReceivedStock, key: &str) -> i64 {
    repo::tasks::create_license_plate_putaway_task_in_scope(
        &context.fixture.db,
        &context.access,
        &command(&context.access, key),
        stock
            .license_plate_id
            .expect("containerized stock has a license plate"),
        context.destination_location_id,
        80,
        None,
        None,
        None,
        Some("Lifecycle license plate putaway"),
    )
    .await
    .unwrap()
}

async fn claim_next(context: &LifecycleContext, workflow: &str, key: &str) -> PutawayClaimResponse {
    let response = send(
        context,
        "/api/v1/putaway-claims/next",
        Some(key),
        json!({ "workflow": workflow }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    response_json::<Option<PutawayClaimResponse>>(response)
        .await
        .expect("putaway work is available")
}

async fn claim_by_id(context: &LifecycleContext, task_id: i64, key: &str) -> PutawayClaimResponse {
    let response = send(
        context,
        &format!("/api/v1/putaway-claims/{task_id}"),
        Some(key),
        json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

async fn lifecycle_effects(context: &LifecycleContext, task_id: i64) -> LifecycleEffects {
    let mut tx = tenant_tx(&context.fixture.db, context.access.tenant_id).await;
    let effects = sqlx::query_as(
        r#"
        SELECT task.status,
               task.assigned_user_id,
               task.started_at IS NULL AS started_at_is_null,
               task.lease_expires_at IS NULL AS lease_expires_at_is_null,
               task.lease_expires_at > statement_timestamp() AS lease_is_current,
               task.release_count,
               (
                   SELECT COUNT(*)
                   FROM work_task_progress progress
                   WHERE progress.tenant_id = task.tenant_id
                     AND progress.task_id = task.id
                     AND progress.action = 'putaway_heartbeat'
               ) AS heartbeat_progress_count,
               (
                   SELECT COUNT(*)
                   FROM work_task_progress progress
                   WHERE progress.tenant_id = task.tenant_id
                     AND progress.task_id = task.id
                     AND progress.action = 'putaway_released'
               ) AS release_progress_count,
               (
                   SELECT COUNT(*)
                   FROM command_idempotency_records command
                   WHERE command.tenant_id = task.tenant_id
                     AND command.operation = 'putaway.heartbeat.v1'
                     AND command.result_json->>'task_id' = task.id::TEXT
               ) AS heartbeat_command_count,
               (
                   SELECT COUNT(*)
                   FROM command_idempotency_records command
                   WHERE command.tenant_id = task.tenant_id
                     AND command.operation = 'putaway.release.v1'
                     AND command.result_json->>'task_id' = task.id::TEXT
               ) AS release_command_count,
               (
                   SELECT COUNT(*)
                   FROM inventory_transactions
                   WHERE tenant_id = task.tenant_id
                     AND operation IN (
                         'task.confirm_putaway.v1',
                         'task.confirm_license_plate_putaway.v1'
                     )
                     AND reference_id = task.id
               ) AS inventory_transaction_count,
               (
                   SELECT COUNT(*)
                   FROM inventory_entries entry
                   INNER JOIN inventory_transactions transaction
                     ON transaction.tenant_id = entry.tenant_id
                    AND transaction.id = entry.transaction_id
                   WHERE transaction.tenant_id = task.tenant_id
                     AND transaction.operation IN (
                         'task.confirm_putaway.v1',
                         'task.confirm_license_plate_putaway.v1'
                     )
                     AND transaction.reference_id = task.id
               ) AS inventory_entry_count,
               (
                   SELECT COUNT(*)
                   FROM outbox_events
                   WHERE tenant_id = task.tenant_id
                     AND event_type IN (
                         'inventory.putaway.confirmed',
                         'inventory.license_plate_putaway.confirmed'
                     )
                     AND aggregate_id = task.id::TEXT
               ) AS outbox_count
        FROM work_tasks task
        WHERE task.tenant_id = $1 AND task.id = $2
        "#,
    )
    .bind(context.access.tenant_id.get())
    .bind(task_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    effects
}

async fn progress_audit(
    context: &LifecycleContext,
    task_id: i64,
    action: &str,
) -> (Option<i64>, Value) {
    let mut tx = tenant_tx(&context.fixture.db, context.access.tenant_id).await;
    let (user_id, metadata_json): (Option<i64>, Option<String>) = sqlx::query_as(
        r#"
        SELECT user_id, metadata_json
        FROM work_task_progress
        WHERE tenant_id = $1
          AND task_id = $2
          AND action = $3
        "#,
    )
    .bind(context.access.tenant_id.get())
    .bind(task_id)
    .bind(action)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    let metadata = serde_json::from_str(
        metadata_json
            .as_deref()
            .expect("lifecycle audit includes typed metadata"),
    )
    .unwrap();
    (user_id, metadata)
}

async fn restrict_to_primary_owner(context: &LifecycleContext) {
    update_scope(
        context,
        false,
        vec![context.facility_id],
        false,
        vec![context.inventory_owner_id],
    )
    .await;
}

async fn update_scope(
    context: &LifecycleContext,
    all_facilities: bool,
    facility_ids: Vec<i64>,
    all_inventory_owners: bool,
    inventory_owner_ids: Vec<i64>,
) {
    assert!(repo::tenants::update_user_access_scope(
        &context.fixture.db,
        context.access.tenant_id,
        &UpdateUserAccessScope {
            user_id: context.access.user_id.get(),
            all_facilities,
            facility_ids,
            all_inventory_owners,
            inventory_owner_ids,
        },
    )
    .await
    .unwrap());
}

#[tokio::test]
async fn putaway_lifecycle_is_typed_scoped_replay_safe_and_audited() {
    let context = setup_context("putaway-lifecycle@test.local").await;
    let loose_stock = receive_stock(
        &context,
        context.inventory_owner_id,
        "PUTAWAY-LIFECYCLE-LOOSE",
        false,
    )
    .await;
    let loose_task = create_loose_task(&context, &loose_stock, "lifecycle-loose-create").await;
    let loose_claim = claim_next(&context, "loose", "lifecycle-loose-claim").await;
    assert_eq!(loose_claim.task_id, loose_task);

    let missing_key = send(
        &context,
        &format!("/api/v1/putaway-claims/{loose_task}/heartbeats"),
        None,
        json!({}),
    )
    .await;
    assert_error(
        missing_key,
        StatusCode::BAD_REQUEST,
        ErrorReason::IdempotencyKeyRequired,
    )
    .await;

    let heartbeat = send(
        &context,
        &format!("/api/v1/putaway-claims/{loose_task}/heartbeats"),
        Some("lifecycle-loose-heartbeat"),
        json!({}),
    )
    .await;
    assert_eq!(heartbeat.status(), StatusCode::OK);
    let heartbeat: PutawayClaimHeartbeatResponse = response_json(heartbeat).await;
    assert_eq!(heartbeat.task_id, loose_task);
    assert!(heartbeat.heartbeat_at < heartbeat.lease_expires_at);
    assert!(loose_claim.lease_expires_at < heartbeat.lease_expires_at);
    let after_heartbeat = lifecycle_effects(&context, loose_task).await;
    assert_eq!(after_heartbeat.heartbeat_progress_count, 1);
    assert_eq!(after_heartbeat.heartbeat_command_count, 1);
    assert_eq!(after_heartbeat.lease_is_current, Some(true));
    assert_eq!(after_heartbeat.inventory_transaction_count, 0);
    assert_eq!(after_heartbeat.inventory_entry_count, 0);
    assert_eq!(after_heartbeat.outbox_count, 0);
    let (heartbeat_actor, heartbeat_metadata) =
        progress_audit(&context, loose_task, "putaway_heartbeat").await;
    assert_eq!(heartbeat_actor, Some(context.access.user_id.get()));
    assert_eq!(
        heartbeat_metadata["lease_expires_at"],
        heartbeat.lease_expires_at
    );
    assert!(heartbeat_metadata["previous_lease_expires_at"]
        .as_str()
        .is_some());

    let heartbeat_replay = send(
        &context,
        &format!("/api/v1/putaway-claims/{loose_task}/heartbeats"),
        Some("lifecycle-loose-heartbeat"),
        json!({}),
    )
    .await;
    assert_eq!(heartbeat_replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<PutawayClaimHeartbeatResponse>(heartbeat_replay).await,
        heartbeat
    );
    assert_eq!(
        lifecycle_effects(&context, loose_task).await,
        after_heartbeat
    );

    for (key, body) in [
        (
            "lifecycle-release-other-without-note",
            json!({ "reason": "other" }),
        ),
        (
            "lifecycle-release-untrimmed-note",
            json!({
                "reason": "work_interrupted",
                "note": " untrimmed",
            }),
        ),
        (
            "lifecycle-release-oversized-note",
            json!({
                "reason": "work_interrupted",
                "note": "x".repeat(501),
            }),
        ),
    ] {
        let invalid = send(
            &context,
            &format!("/api/v1/putaway-claims/{loose_task}/releases"),
            Some(key),
            body,
        )
        .await;
        assert_error(
            invalid,
            StatusCode::BAD_REQUEST,
            ErrorReason::InvalidRequest,
        )
        .await;
    }

    let release = send(
        &context,
        &format!("/api/v1/putaway-claims/{loose_task}/releases"),
        Some("lifecycle-loose-release"),
        json!({
            "reason": "destination_blocked",
            "note": "Storage lane is obstructed",
        }),
    )
    .await;
    assert_eq!(release.status(), StatusCode::OK);
    let release: PutawayClaimReleaseResponse = response_json(release).await;
    assert_eq!(release.task_id, loose_task);
    assert_eq!(release.release_count, 1);
    assert_eq!(
        release.reason,
        PutawayClaimReleaseReason::DestinationBlocked
    );
    assert_eq!(release.note.as_deref(), Some("Storage lane is obstructed"));
    let after_release = lifecycle_effects(&context, loose_task).await;
    assert_eq!(after_release.status, "open");
    assert_eq!(after_release.assigned_user_id, None);
    assert!(after_release.started_at_is_null);
    assert!(after_release.lease_expires_at_is_null);
    assert_eq!(after_release.lease_is_current, None);
    assert_eq!(after_release.release_count, 1);
    assert_eq!(after_release.heartbeat_progress_count, 1);
    assert_eq!(after_release.release_progress_count, 1);
    assert_eq!(after_release.heartbeat_command_count, 1);
    assert_eq!(after_release.release_command_count, 1);
    assert_eq!(after_release.inventory_transaction_count, 0);
    assert_eq!(after_release.inventory_entry_count, 0);
    assert_eq!(after_release.outbox_count, 0);
    let (release_actor, release_metadata) =
        progress_audit(&context, loose_task, "putaway_released").await;
    assert_eq!(release_actor, Some(context.access.user_id.get()));
    assert_eq!(
        release_metadata,
        json!({
            "release_count": 1,
            "reason": "destination_blocked",
            "note": "Storage lane is obstructed",
        })
    );

    let release_replay = send(
        &context,
        &format!("/api/v1/putaway-claims/{loose_task}/releases"),
        Some("lifecycle-loose-release"),
        json!({
            "reason": "destination_blocked",
            "note": "Storage lane is obstructed",
        }),
    )
    .await;
    assert_eq!(release_replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<PutawayClaimReleaseResponse>(release_replay).await,
        release
    );
    assert_eq!(lifecycle_effects(&context, loose_task).await, after_release);
    let changed_release = send(
        &context,
        &format!("/api/v1/putaway-claims/{loose_task}/releases"),
        Some("lifecycle-loose-release"),
        json!({
            "reason": "safety_issue",
            "note": "Changed retry body",
        }),
    )
    .await;
    assert_error(
        changed_release,
        StatusCode::CONFLICT,
        ErrorReason::IdempotencyKeyReused,
    )
    .await;
    let heartbeat_after_release = send(
        &context,
        &format!("/api/v1/putaway-claims/{loose_task}/heartbeats"),
        Some("lifecycle-loose-heartbeat"),
        json!({}),
    )
    .await;
    assert_eq!(heartbeat_after_release.status(), StatusCode::OK);
    assert_eq!(
        response_json::<PutawayClaimHeartbeatResponse>(heartbeat_after_release).await,
        heartbeat
    );
    let new_heartbeat_after_release = send(
        &context,
        &format!("/api/v1/putaway-claims/{loose_task}/heartbeats"),
        Some("lifecycle-loose-heartbeat-after-release"),
        json!({}),
    )
    .await;
    assert_error(
        new_heartbeat_after_release,
        StatusCode::CONFLICT,
        ErrorReason::Conflict,
    )
    .await;
    assert_eq!(lifecycle_effects(&context, loose_task).await, after_release);
    let second_release = send(
        &context,
        &format!("/api/v1/putaway-claims/{loose_task}/releases"),
        Some("lifecycle-loose-release-again"),
        json!({ "reason": "work_interrupted" }),
    )
    .await;
    assert_error(second_release, StatusCode::CONFLICT, ErrorReason::Conflict).await;

    let plate_stock = receive_stock(
        &context,
        context.inventory_owner_id,
        "PUTAWAY-LIFECYCLE-PLATE",
        true,
    )
    .await;
    let plate_task = create_plate_task(&context, &plate_stock, "lifecycle-plate-create").await;
    let plate_claim = claim_next(&context, "license_plate", "lifecycle-plate-claim").await;
    assert_eq!(plate_claim.task_id, plate_task);

    let reused_heartbeat_key = send(
        &context,
        &format!("/api/v1/putaway-claims/{plate_task}/heartbeats"),
        Some("lifecycle-loose-heartbeat"),
        json!({}),
    )
    .await;
    assert_error(
        reused_heartbeat_key,
        StatusCode::CONFLICT,
        ErrorReason::IdempotencyKeyReused,
    )
    .await;
    let plate_heartbeat = send(
        &context,
        &format!("/api/v1/putaway-claims/{plate_task}/heartbeats"),
        Some("lifecycle-plate-heartbeat"),
        json!({}),
    )
    .await;
    assert_eq!(plate_heartbeat.status(), StatusCode::OK);
    let plate_release = send(
        &context,
        &format!("/api/v1/putaway-claims/{plate_task}/releases"),
        Some("lifecycle-plate-release"),
        json!({
            "reason": "equipment_unavailable",
            "note": "Reach truck unavailable",
        }),
    )
    .await;
    assert_eq!(plate_release.status(), StatusCode::OK);
    let plate_effects = lifecycle_effects(&context, plate_task).await;
    assert_eq!(plate_effects.status, "open");
    assert_eq!(plate_effects.heartbeat_progress_count, 1);
    assert_eq!(plate_effects.release_progress_count, 1);
    assert_eq!(plate_effects.inventory_transaction_count, 0);

    let facility_denied_stock = receive_stock(
        &context,
        context.inventory_owner_id,
        "PUTAWAY-LIFECYCLE-FACILITY-DENIED",
        false,
    )
    .await;
    let facility_denied_task = create_loose_task(
        &context,
        &facility_denied_stock,
        "lifecycle-facility-denied-create",
    )
    .await;
    update_scope(&context, false, Vec::new(), true, Vec::new()).await;
    for (suffix, key) in [
        ("heartbeats", "lifecycle-facility-denied-heartbeat"),
        ("releases", "lifecycle-facility-denied-release"),
    ] {
        let denied = send(
            &context,
            &format!("/api/v1/putaway-claims/{facility_denied_task}/{suffix}"),
            Some(key),
            lifecycle_body(suffix),
        )
        .await;
        assert_error(denied, StatusCode::NOT_FOUND, ErrorReason::NotFound).await;
    }
    let facility_denied_effects = lifecycle_effects(&context, facility_denied_task).await;
    assert_eq!(facility_denied_effects.heartbeat_progress_count, 0);
    assert_eq!(facility_denied_effects.release_progress_count, 0);
    assert_eq!(facility_denied_effects.heartbeat_command_count, 0);
    assert_eq!(facility_denied_effects.release_command_count, 0);
    update_scope(&context, true, Vec::new(), true, Vec::new()).await;
    assert!(repo::tasks::cancel_task(
        &context.fixture.db,
        context.access.tenant_id,
        facility_denied_task,
        context.access.user_id.get(),
    )
    .await
    .unwrap());

    let denied_stock = receive_stock(
        &context,
        context.denied_inventory_owner_id,
        "PUTAWAY-LIFECYCLE-DENIED",
        false,
    )
    .await;
    let denied_task = create_loose_task(&context, &denied_stock, "lifecycle-denied-create").await;
    restrict_to_primary_owner(&context).await;
    for (suffix, key) in [
        ("heartbeats", "lifecycle-denied-heartbeat"),
        ("releases", "lifecycle-denied-release"),
    ] {
        let denied = send(
            &context,
            &format!("/api/v1/putaway-claims/{denied_task}/{suffix}"),
            Some(key),
            lifecycle_body(suffix),
        )
        .await;
        assert_error(denied, StatusCode::NOT_FOUND, ErrorReason::NotFound).await;
    }
    let denied_effects = lifecycle_effects(&context, denied_task).await;
    assert_eq!(denied_effects.heartbeat_progress_count, 0);
    assert_eq!(denied_effects.release_progress_count, 0);
    assert_eq!(denied_effects.heartbeat_command_count, 0);
    assert_eq!(denied_effects.release_command_count, 0);

    let stale_stock = receive_stock(
        &context,
        context.inventory_owner_id,
        "PUTAWAY-LIFECYCLE-STALE",
        false,
    )
    .await;
    let stale_task = create_loose_task(&context, &stale_stock, "lifecycle-stale-create").await;
    let stale_claim = claim_by_id(&context, stale_task, "lifecycle-stale-claim").await;
    assert_eq!(stale_claim.task_id, stale_task);
    let mut tx = tenant_tx(&context.fixture.db, context.access.tenant_id).await;
    sqlx::query(
        "UPDATE work_tasks SET lease_expires_at = statement_timestamp() - INTERVAL '1 second' WHERE tenant_id = $1 AND id = $2",
    )
    .bind(context.access.tenant_id.get())
    .bind(stale_task)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let before_stale = lifecycle_effects(&context, stale_task).await;
    assert_eq!(before_stale.lease_is_current, Some(false));
    for (suffix, key) in [
        ("heartbeats", "lifecycle-stale-heartbeat"),
        ("releases", "lifecycle-stale-release"),
    ] {
        let stale = send(
            &context,
            &format!("/api/v1/putaway-claims/{stale_task}/{suffix}"),
            Some(key),
            lifecycle_body(suffix),
        )
        .await;
        assert_error(stale, StatusCode::CONFLICT, ErrorReason::Conflict).await;
    }
    assert_eq!(lifecycle_effects(&context, stale_task).await, before_stale);
}

#[tokio::test]
async fn concurrent_putaway_heartbeat_and_terminal_commands_are_serialized() {
    let context = setup_context("putaway-lifecycle-race@test.local").await;
    let stock = receive_stock(
        &context,
        context.inventory_owner_id,
        "PUTAWAY-LIFECYCLE-RACE",
        false,
    )
    .await;
    let task_id = create_loose_task(&context, &stock, "lifecycle-race-create").await;
    let claim = claim_next(&context, "loose", "lifecycle-race-claim").await;
    assert_eq!(claim.task_id, task_id);

    let heartbeat_uri = format!("/api/v1/putaway-claims/{task_id}/heartbeats");
    let (first, second) = tokio::join!(
        send(
            &context,
            &heartbeat_uri,
            Some("lifecycle-race-heartbeat"),
            json!({})
        ),
        send(
            &context,
            &heartbeat_uri,
            Some("lifecycle-race-heartbeat"),
            json!({})
        )
    );
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(
        response_json::<PutawayClaimHeartbeatResponse>(first).await,
        response_json::<PutawayClaimHeartbeatResponse>(second).await
    );
    let after_heartbeats = lifecycle_effects(&context, task_id).await;
    assert_eq!(after_heartbeats.heartbeat_progress_count, 1);
    assert_eq!(after_heartbeats.heartbeat_command_count, 1);

    let release_uri = format!("/api/v1/putaway-claims/{task_id}/releases");
    let confirm_uri = format!("/api/v1/putaway-tasks/{task_id}/confirmations");
    let (release, confirmation) = tokio::join!(
        send(
            &context,
            &release_uri,
            Some("lifecycle-race-release"),
            json!({
                "reason": "safety_issue",
                "note": "Concurrent safety stop",
            })
        ),
        send(
            &context,
            &confirm_uri,
            Some("lifecycle-race-confirm"),
            json!({ "destination_location_barcode": context.destination_barcode })
        )
    );
    let statuses = [release.status(), confirmation.status()];
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
    let effects = lifecycle_effects(&context, task_id).await;
    assert_eq!(
        effects.release_progress_count + effects.inventory_transaction_count,
        1
    );
    assert_eq!(effects.release_command_count + effects.outbox_count, 1);
    assert!(matches!(effects.status.as_str(), "open" | "completed"));

    let release_replay = send(
        &context,
        &release_uri,
        Some("lifecycle-race-release"),
        json!({
            "reason": "safety_issue",
            "note": "Concurrent safety stop",
        }),
    )
    .await;
    if effects.status == "open" {
        assert_eq!(release_replay.status(), StatusCode::OK);
        let release: PutawayClaimReleaseResponse = response_json(release_replay).await;
        assert_eq!(release.task_id, task_id);
    } else {
        assert_error(release_replay, StatusCode::CONFLICT, ErrorReason::Conflict).await;
    }
}
