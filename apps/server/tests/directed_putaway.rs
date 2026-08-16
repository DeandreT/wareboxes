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
    ConfigurationResponse, CreatePutawayTaskResponse, ErrorReason, ErrorResponse,
    ItemStoragePolicyResponse, PutawayCandidatePage, PutawayConfirmationResponse,
    PutawayPolicySource, PutawayWorkPage,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InboundReceiptExceptionReason, InventoryHoldReason};

fn request(
    token: &str,
    tenant_id: TenantId,
    method: Method,
    uri: &str,
    idempotency_key: &str,
    body: Value,
) -> Request<Body> {
    Request::builder()
        .method(method)
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
        .oneshot(request(
            token,
            tenant_id,
            Method::POST,
            uri,
            idempotency_key,
            body,
        ))
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

async fn grant_permission(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    permission_name: &str,
    suffix: &str,
) {
    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        &fixture.db,
        tenant_id,
        permission_name,
        Some(permission_name),
    )
    .await
    .unwrap();
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("putaway-policy-{permission_name}-{suffix}"),
        None,
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        permission,
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_to_user(&fixture.db, tenant_id, user_id, role)
        .await
        .unwrap();
}

async fn transition_configuration(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    configuration: &ConfigurationResponse,
    transition: &str,
    key: &str,
) -> ConfigurationResponse {
    let response = send(
        app,
        token,
        tenant_id,
        &format!(
            "/api/v1/configurations/{}/{}",
            configuration.configuration_id, transition
        ),
        key,
        json!({"expected_revision": configuration.revision.get()}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

fn command(access: &wareboxes_core::models::TenantAccess, key: &str) -> CommandContext {
    CommandContext {
        tenant_id: access.tenant_id,
        actor_id: access.user_id,
        request_id: format!("request-{key}"),
        idempotency_key: Some(key.to_owned()),
    }
}

#[tokio::test]
async fn directed_putaway_is_claimed_scanned_atomic_and_replay_safe() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("directed-putaway@test.local").await;
    let access = default_tenant_for_user(&fixture.db, user.id)
        .await
        .expect("WMS user has tenant access");
    let tenant_id = access.tenant_id;
    grant_permission(&fixture, tenant_id, user.id, "admin", "operator").await;
    let approver = fixture.user("directed-putaway-approver@test.local").await;
    let mut membership_tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships(tenant_id,user_id) VALUES ($1,$2)")
        .bind(tenant_id.get())
        .bind(approver.id)
        .execute(&mut *membership_tx)
        .await
        .unwrap();
    membership_tx.commit().await.unwrap();
    grant_permission(&fixture, tenant_id, approver.id, "admin", "approver").await;
    let facility_id = fixture.facility(tenant_id, "Putaway DC").await;
    let inventory_owner_id = fixture.inventory_owner(tenant_id, "Putaway Owner").await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let receiving_location_id = wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some("PUTAWAY-RECEIVING"),
        Some("Putaway Receiving"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let destination_location_id = wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some("PUTAWAY-A-01"),
        Some("Putaway A-01"),
        "bin",
        true,
        false,
        false,
    )
    .await
    .unwrap();
    let _wrong_destination_location_id = fixture
        .location(tenant_id, facility_id, "PUTAWAY-A-02")
        .await;
    let item_id = fixture.item(tenant_id, "Putaway Item", "case").await;
    let load_id = repo::loads::add_load(
        &fixture.db,
        tenant_id,
        user.id,
        facility_id,
        inventory_owner_id,
        LoadType::Inbound,
        Some("PUTAWAY-INBOUND"),
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
        tenant_id,
        user.id,
        load_id,
        item_id,
        None,
        10,
        Some("PUTAWAY-LOT"),
        None,
        None,
    )
    .await
    .unwrap();
    repo::loads::update_load(
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
    .unwrap();
    start_expected_receipt_unloading(
        &fixture.db,
        &access,
        load_id,
        receiving_location_id,
        "putaway-unloading",
    )
    .await;
    let receipt = repo::inbound_receipt::receive_expected_inventory(
        &fixture.db,
        &access,
        &command(&access, "putaway-receipt"),
        load_line_id,
        &repo::inbound_receipt::ReceiveExpectedInventoryCommand {
            receiving_location_id: Some(receiving_location_id),
            received_qty: 10,
            rejected_qty: 0,
            missing_qty: 0,
            license_plate_id: None,
            license_plate_barcode: None,
            lot: Some("PUTAWAY-LOT"),
            serial: None,
            expiration: None,
            exception_reason: None::<InboundReceiptExceptionReason>,
            exception_note: None,
        },
    )
    .await
    .unwrap();
    let source_inventory_balance_id = receipt
        .inventory_balance_id
        .expect("physical receipt identifies its balance");

    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let approver_token = auth::create_session(&fixture.db, approver.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let strict = send(
        &app,
        &token,
        tenant_id,
        "/api/v1/configurations",
        "putaway-policy-strict-create",
        json!({
            "scope": {
                "level": "owner_facility",
                "inventory_owner_id": inventory_owner_id,
                "facility_id": facility_id
            },
            "effective_from": "2026-01-01T00:00:00Z",
            "effective_until": null,
            "rule": {
                "kind": "putaway",
                "require_zone_compatibility": true,
                "enforce_location_capacity": true,
                "allow_mixed_lots": false
            },
            "expected_revision": null
        }),
    )
    .await;
    assert_eq!(strict.status(), StatusCode::OK);
    let strict: ConfigurationResponse = response_json(strict).await;
    let strict = transition_configuration(
        &app,
        &token,
        tenant_id,
        &strict,
        "submissions",
        "putaway-policy-strict-submit",
    )
    .await;
    let strict = transition_configuration(
        &app,
        &approver_token,
        tenant_id,
        &strict,
        "approvals",
        "putaway-policy-strict-approve",
    )
    .await;
    let strict = transition_configuration(
        &app,
        &token,
        tenant_id,
        &strict,
        "activations",
        "putaway-policy-strict-activate",
    )
    .await;
    let candidates = get(
        &app,
        &token,
        tenant_id,
        "/api/v1/putaway-candidates?limit=100&sort=quantity&direction=desc",
    )
    .await;
    assert_eq!(candidates.status(), StatusCode::OK);
    let candidates: PutawayCandidatePage = response_json(candidates).await;
    assert_eq!(candidates.items.len(), 1);
    assert_eq!(
        candidates.items[0].source_inventory_balance_id,
        Some(source_inventory_balance_id)
    );
    assert_eq!(candidates.items[0].available_quantity, 10);
    assert_eq!(
        candidates.items[0].putaway_policy.source,
        PutawayPolicySource::Configuration
    );
    assert_eq!(
        candidates.items[0].putaway_policy.configuration_id,
        Some(strict.configuration_id)
    );
    assert!(
        candidates.items[0]
            .putaway_policy
            .require_zone_compatibility
    );
    let strict_policy = candidates.items[0].putaway_policy.clone();
    let strict_expectation = strict_policy.expectation();

    let rejected_by_zone_policy = send(
        &app,
        &token,
        tenant_id,
        "/api/v1/putaway-tasks",
        "putaway-strict-zone-rejection",
        json!({
            "source_inventory_balance_id": source_inventory_balance_id,
            "destination_location_id": destination_location_id,
            "quantity": 6,
            "priority": 80,
            "assigned_user_id": user.id,
            "expected_policy": strict_expectation
        }),
    )
    .await;
    assert_eq!(rejected_by_zone_policy.status(), StatusCode::CONFLICT);

    let zone = send(
        &app,
        &token,
        tenant_id,
        "/api/v1/storage-zones",
        "putaway-policy-zone",
        json!({
            "facility_id": facility_id,
            "code": "PUTAWAY-RESERVE",
            "name": "Putaway reserve",
            "purpose": "reserve",
            "travel_sequence": 10,
            "location_ids": [destination_location_id]
        }),
    )
    .await;
    assert_eq!(zone.status(), StatusCode::OK);
    let storage_policy = send(
        &app,
        &token,
        tenant_id,
        "/api/v1/item-storage-policies",
        "putaway-policy-capacity",
        json!({
            "inventory_owner_id": inventory_owner_id,
            "facility_id": facility_id,
            "item_id": item_id,
            "uom": "case",
            "allowed_zone_purposes": ["reserve"],
            "max_quantity_per_location": 5,
            "expected_revision": null
        }),
    )
    .await;
    assert_eq!(storage_policy.status(), StatusCode::OK);
    let storage_policy: ItemStoragePolicyResponse = response_json(storage_policy).await;

    let rejected_by_capacity_policy = send(
        &app,
        &token,
        tenant_id,
        "/api/v1/putaway-tasks",
        "putaway-strict-capacity-rejection",
        json!({
            "source_inventory_balance_id": source_inventory_balance_id,
            "destination_location_id": destination_location_id,
            "quantity": 6,
            "priority": 80,
            "assigned_user_id": user.id,
            "expected_policy": strict_policy.expectation()
        }),
    )
    .await;
    assert_eq!(rejected_by_capacity_policy.status(), StatusCode::CONFLICT);

    let mut tamper_tx = tenant_tx(&fixture.db, tenant_id).await;
    let raw_task_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO work_tasks(
          tenant_id,created,task_type,status,required_permission,priority,title,
          created_by,facility_id,inventory_owner_id)
        VALUES($1,statement_timestamp(),'putaway','open','wms',50,
          'Raw policy bypass attempt',$2,$3,$4)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(user.id)
    .bind(facility_id)
    .bind(inventory_owner_id)
    .fetch_one(&mut *tamper_tx)
    .await
    .unwrap();
    let source_dimensions: (i64, i64, String) = sqlx::query_as(
        "SELECT item_batch_id,item_id,status FROM inventory_balances WHERE tenant_id=$1 AND id=$2",
    )
    .bind(tenant_id.get())
    .bind(source_inventory_balance_id)
    .fetch_one(&mut *tamper_tx)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO putaway_tasks(
          tenant_id,task_id,inventory_owner_id,facility_id,
          source_inventory_balance_id,source_location_id,destination_location_id,
          item_batch_id,item_id,inventory_status,planned_quantity,
          putaway_policy_source,putaway_policy_configuration_id,
          putaway_policy_configuration_revision,putaway_policy_scope_level,
          putaway_policy_scope_owner_id,putaway_policy_scope_facility_id,
          putaway_policy_definition,putaway_policy_hash)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,6,
          'configuration',$11,$12,'owner_facility',$3,$4,$13,$14)
        "#,
    )
    .bind(tenant_id.get())
    .bind(raw_task_id)
    .bind(inventory_owner_id)
    .bind(facility_id)
    .bind(source_inventory_balance_id)
    .bind(receiving_location_id)
    .bind(destination_location_id)
    .bind(source_dimensions.0)
    .bind(source_dimensions.1)
    .bind(source_dimensions.2)
    .bind(strict_policy.configuration_id)
    .bind(strict_policy.configuration_revision)
    .bind(json!({
        "require_zone_compatibility": true,
        "enforce_location_capacity": true,
        "allow_mixed_lots": false
    }))
    .bind(&strict_policy.policy_hash)
    .execute(&mut *tamper_tx)
    .await
    .unwrap();
    assert!(
        tamper_tx.commit().await.is_err(),
        "deferred database policy enforcement must reject a raw over-capacity task"
    );

    let retired_storage_policy = send(
        &app,
        &token,
        tenant_id,
        &format!(
            "/api/v1/item-storage-policies/{}/retirements",
            storage_policy.item_storage_policy_id
        ),
        "putaway-policy-capacity-retire",
        json!({"expected_revision": storage_policy.revision.get()}),
    )
    .await;
    assert_eq!(retired_storage_policy.status(), StatusCode::OK);

    let strict = transition_configuration(
        &app,
        &token,
        tenant_id,
        &strict,
        "retirements",
        "putaway-policy-strict-retire",
    )
    .await;
    let relaxed = send(
        &app,
        &token,
        tenant_id,
        "/api/v1/configurations",
        "putaway-policy-relaxed-create",
        json!({
            "scope": {
                "level": "owner_facility",
                "inventory_owner_id": inventory_owner_id,
                "facility_id": facility_id
            },
            "effective_from": "2026-01-01T00:00:00Z",
            "effective_until": null,
            "rule": {
                "kind": "putaway",
                "require_zone_compatibility": false,
                "enforce_location_capacity": false,
                "allow_mixed_lots": true
            },
            "expected_revision": strict.revision.get()
        }),
    )
    .await;
    assert_eq!(relaxed.status(), StatusCode::OK);
    let relaxed: ConfigurationResponse = response_json(relaxed).await;
    let relaxed = transition_configuration(
        &app,
        &token,
        tenant_id,
        &relaxed,
        "submissions",
        "putaway-policy-relaxed-submit",
    )
    .await;
    let relaxed = transition_configuration(
        &app,
        &approver_token,
        tenant_id,
        &relaxed,
        "approvals",
        "putaway-policy-relaxed-approve",
    )
    .await;
    let relaxed = transition_configuration(
        &app,
        &token,
        tenant_id,
        &relaxed,
        "activations",
        "putaway-policy-relaxed-activate",
    )
    .await;
    let refreshed_candidates = get(
        &app,
        &token,
        tenant_id,
        "/api/v1/putaway-candidates?limit=100",
    )
    .await;
    assert_eq!(refreshed_candidates.status(), StatusCode::OK);
    let refreshed_candidates: PutawayCandidatePage = response_json(refreshed_candidates).await;
    let relaxed_policy = refreshed_candidates.items[0].putaway_policy.clone();
    assert_eq!(
        relaxed_policy.configuration_id,
        Some(relaxed.configuration_id)
    );
    assert!(relaxed_policy.allow_mixed_lots);
    let stale_policy = send(
        &app,
        &token,
        tenant_id,
        "/api/v1/putaway-tasks",
        "putaway-stale-policy",
        json!({
            "source_inventory_balance_id": source_inventory_balance_id,
            "destination_location_id": destination_location_id,
            "quantity": 6,
            "priority": 80,
            "assigned_user_id": user.id,
            "expected_policy": strict_expectation
        }),
    )
    .await;
    assert_eq!(stale_policy.status(), StatusCode::CONFLICT);

    let create_body = json!({
        "source_inventory_balance_id": source_inventory_balance_id,
        "destination_location_id": destination_location_id,
        "quantity": 6,
        "priority": 80,
        "assigned_user_id": user.id,
        "instructions": "Scan the directed storage location",
        "expected_policy": relaxed_policy.expectation()
    });
    let created = send(
        &app,
        &token,
        tenant_id,
        "/api/v1/putaway-tasks",
        "putaway-create",
        create_body.clone(),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    let created: CreatePutawayTaskResponse = response_json(created).await;
    assert!(created.task_id > 0);
    assert_eq!(created.putaway_policy, relaxed_policy);

    let candidates_after_plan = get(
        &app,
        &token,
        tenant_id,
        "/api/v1/putaway-candidates?limit=100",
    )
    .await;
    assert_eq!(candidates_after_plan.status(), StatusCode::OK);
    assert!(response_json::<PutawayCandidatePage>(candidates_after_plan)
        .await
        .items
        .is_empty());
    let work = get(
        &app,
        &token,
        tenant_id,
        "/api/v1/putaway-tasks?limit=100&sort=priority&direction=desc",
    )
    .await;
    assert_eq!(work.status(), StatusCode::OK);
    let work: PutawayWorkPage = response_json(work).await;
    assert_eq!(work.items.len(), 1);
    assert_eq!(work.items[0].task_id, created.task_id);
    assert_eq!(work.items[0].planned_quantity, 6);

    let replayed_create = send(
        &app,
        &token,
        tenant_id,
        "/api/v1/putaway-tasks",
        "putaway-create",
        create_body,
    )
    .await;
    assert_eq!(replayed_create.status(), StatusCode::OK);
    assert_eq!(
        response_json::<CreatePutawayTaskResponse>(replayed_create).await,
        created
    );
    let changed_create = send(
        &app,
        &token,
        tenant_id,
        "/api/v1/putaway-tasks",
        "putaway-create",
        json!({
            "source_inventory_balance_id": source_inventory_balance_id,
            "destination_location_id": destination_location_id,
            "quantity": 5,
            "priority": 80,
            "assigned_user_id": user.id
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
        "putaway-start",
        json!({"task_id": created.task_id}),
    )
    .await;
    assert_eq!(started.status(), StatusCode::OK);
    assert!(response_json::<bool>(started).await);

    let wrong_scan = send(
        &app,
        &token,
        tenant_id,
        &format!("/api/v1/putaway-tasks/{}/confirmations", created.task_id),
        "putaway-wrong-scan",
        json!({
            "destination_location_barcode": "PUTAWAY-A-02",
            "expected_policy": relaxed_policy.expectation()
        }),
    )
    .await;
    let wrong_scan_status = wrong_scan.status();
    let wrong_scan_error = response_json::<ErrorResponse>(wrong_scan).await;
    assert_eq!(
        wrong_scan_status,
        StatusCode::CONFLICT,
        "unexpected putaway scan error: {wrong_scan_error:?}"
    );

    let confirmation_uri = format!("/api/v1/putaway-tasks/{}/confirmations", created.task_id);
    let confirmed = send(
        &app,
        &token,
        tenant_id,
        &confirmation_uri,
        "putaway-confirm",
        json!({
            "destination_location_barcode": "PUTAWAY-A-01",
            "expected_policy": relaxed_policy.expectation()
        }),
    )
    .await;
    let confirmed_status = confirmed.status();
    if confirmed_status != StatusCode::OK {
        let error = response_json::<ErrorResponse>(confirmed).await;
        panic!("putaway confirmation failed with {confirmed_status}: {error:?}");
    }
    let confirmed: PutawayConfirmationResponse = response_json(confirmed).await;
    assert_eq!(confirmed.task_id, created.task_id);
    assert_eq!(
        confirmed.source_inventory_balance_id,
        source_inventory_balance_id
    );
    assert_eq!(confirmed.source_location_id, receiving_location_id);
    assert_eq!(confirmed.destination_location_id, destination_location_id);
    assert_eq!(confirmed.destination_location_barcode, "PUTAWAY-A-01");
    assert_eq!(confirmed.quantity, 6);
    assert_eq!(confirmed.inventory_status, "available");
    assert_eq!(confirmed.putaway_policy, relaxed_policy);

    let completed_work = get(
        &app,
        &token,
        tenant_id,
        "/api/v1/putaway-tasks?limit=100&status=completed&sort=created_at&direction=asc",
    )
    .await;
    assert_eq!(completed_work.status(), StatusCode::OK);
    let completed_work: PutawayWorkPage = response_json(completed_work).await;
    assert_eq!(completed_work.items.len(), 1);
    assert_eq!(completed_work.items[0].task_id, created.task_id);
    assert!(completed_work.items[0].completed_at.is_some());

    let replayed_confirmation = send(
        &app,
        &token,
        tenant_id,
        &confirmation_uri,
        "putaway-confirm",
        json!({
            "destination_location_barcode": "PUTAWAY-A-01",
            "expected_policy": relaxed_policy.expectation()
        }),
    )
    .await;
    assert_eq!(replayed_confirmation.status(), StatusCode::OK);
    assert_eq!(
        response_json::<PutawayConfirmationResponse>(replayed_confirmation).await,
        confirmed
    );

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let effects: (i64, i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT qty_on_hand FROM inventory_balances WHERE id = $1),
            (SELECT qty_on_hand FROM inventory_balances WHERE id = $2),
            (SELECT COUNT(*) FROM putaway_results WHERE task_id = $3),
            (
                SELECT COUNT(*)
                FROM inventory_entries
                WHERE transaction_id = $4
            ),
            (
                SELECT COALESCE(SUM(quantity_delta), 0)::BIGINT
                FROM inventory_entries
                WHERE transaction_id = $4
            ),
            (
                SELECT COUNT(*)
                FROM work_task_progress
                WHERE task_id = $3 AND action = 'putaway_confirmed'
            ),
            (
                SELECT COUNT(*)
                FROM outbox_events
                WHERE event_type = 'inventory.putaway.confirmed'
            ),
            (SELECT COUNT(*) FROM inventory_reconciliation),
            (
                SELECT COUNT(*)
                FROM inventory_projection_changes
                WHERE transaction_id = $4
            )
        "#,
    )
    .bind(source_inventory_balance_id)
    .bind(confirmed.destination_inventory_balance_id)
    .bind(created.task_id)
    .bind(confirmed.inventory_transaction_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let task_state: (String, Option<i64>, Option<i64>, bool) = sqlx::query_as(
        r#"
        SELECT task.status,
               task.completed_by,
               result.inventory_transaction_id,
               detail.closed_at IS NOT NULL
        FROM work_tasks task
        INNER JOIN putaway_tasks detail
          ON detail.tenant_id = task.tenant_id AND detail.task_id = task.id
        INNER JOIN putaway_results result
          ON result.tenant_id = task.tenant_id AND result.task_id = task.id
        WHERE task.tenant_id = $1 AND task.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(created.task_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();

    assert_eq!(effects, (4, 6, 1, 2, 0, 1, 1, 0, 2));
    assert_eq!(
        task_state,
        (
            "completed".to_owned(),
            Some(user.id),
            Some(confirmed.inventory_transaction_id),
            true,
        )
    );

    let blocked_task = send(
        &app,
        &token,
        tenant_id,
        "/api/v1/putaway-tasks",
        "putaway-create-blocked",
        json!({
            "source_inventory_balance_id": source_inventory_balance_id,
            "destination_location_id": destination_location_id,
            "quantity": 4,
            "assigned_user_id": user.id,
            "expected_policy": relaxed_policy.expectation()
        }),
    )
    .await;
    assert_eq!(blocked_task.status(), StatusCode::OK);
    let blocked_task: CreatePutawayTaskResponse = response_json(blocked_task).await;
    repo::inventory::place_inventory_hold(
        &fixture.db,
        &access,
        &command(&access, "putaway-place-hold"),
        &repo::inventory::PlaceInventoryHoldCommand {
            inventory_balance_id: source_inventory_balance_id,
            qty: 1,
            reason: InventoryHoldReason::QualityInspection,
            note: Some("putaway revalidation"),
            reference_type: Some("putaway_task"),
            reference_id: Some(blocked_task.task_id),
        },
    )
    .await
    .unwrap();
    let started = send(
        &app,
        &token,
        tenant_id,
        "/api/tasks/start",
        "putaway-start-blocked",
        json!({"task_id": blocked_task.task_id}),
    )
    .await;
    assert_eq!(started.status(), StatusCode::OK);
    assert!(response_json::<bool>(started).await);
    let blocked_confirmation = send(
        &app,
        &token,
        tenant_id,
        &format!(
            "/api/v1/putaway-tasks/{}/confirmations",
            blocked_task.task_id
        ),
        "putaway-confirm-blocked",
        json!({
            "destination_location_barcode": "PUTAWAY-A-01",
            "expected_policy": relaxed_policy.expectation()
        }),
    )
    .await;
    assert_eq!(blocked_confirmation.status(), StatusCode::CONFLICT);

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let blocked_effects: (i64, i64, i64, String) = sqlx::query_as(
        r#"
        SELECT
            (SELECT qty_on_hand FROM inventory_balances WHERE id = $1),
            (SELECT qty_held FROM inventory_balances WHERE id = $1),
            (SELECT COUNT(*) FROM putaway_results),
            (SELECT status FROM work_tasks WHERE id = $2)
        "#,
    )
    .bind(source_inventory_balance_id)
    .bind(blocked_task.task_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        blocked_effects,
        (4, 1, 1, "in_progress".to_owned()),
        "a new commitment must block stale putaway work without moving stock"
    );

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    assert!(
        sqlx::query("UPDATE putaway_results SET quantity = quantity + 1 WHERE task_id = $1")
            .bind(created.task_id)
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    assert!(
        sqlx::query(
            "UPDATE putaway_tasks SET putaway_policy_hash=repeat('0',64) WHERE task_id=$1",
        )
        .bind(created.task_id)
        .execute(&mut *tx)
        .await
        .is_err(),
        "frozen putaway policy evidence must be immutable"
    );
    tx.rollback().await.unwrap();
}
