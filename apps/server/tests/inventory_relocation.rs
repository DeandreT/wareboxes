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
use wareboxes_api_contract::v1::{
    CreateInventoryRelocationTaskResponse, ErrorReason, ErrorResponse,
    InventoryRelocationClaimHeartbeatResponse, InventoryRelocationClaimReleaseResponse,
    InventoryRelocationClaimResponse, InventoryRelocationConfirmationResponse,
    InventoryRelocationResult,
};
use wareboxes_application::CommandContext;
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_core::models::{InboundReceiptExceptionReason, ReceiveExpectedInventoryResult};

const CONCURRENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

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
    app.clone()
        .oneshot(request(
            token,
            tenant_id,
            method,
            uri,
            idempotency_key,
            body,
        ))
        .await
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn command(access: &wareboxes_core::models::TenantAccess, key: &str) -> CommandContext {
    CommandContext {
        tenant_id: access.tenant_id,
        actor_id: access.user_id,
        request_id: format!("request-{key}"),
        idempotency_key: Some(key.to_owned()),
    }
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

async fn grant_wms_role(db: &db::Db, tenant_id: TenantId, user_ids: &[i64], role_name: &str) {
    let permission = repo::permissions::find_by_name(db, tenant_id, "wms")
        .await
        .unwrap()
        .expect("tenant has a WMS permission");
    let role = repo::roles::add_role(
        db,
        tenant_id,
        role_name,
        Some("Inventory relocation operator"),
    )
    .await
    .unwrap();
    assert!(
        repo::roles::add_role_permission(db, tenant_id, role, permission.id)
            .await
            .unwrap()
    );
    for user_id in user_ids {
        assert!(repo::roles::add_role_to_user(db, tenant_id, *user_id, role)
            .await
            .unwrap());
    }
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
async fn receive_plate_stock(
    fixture: &Fixture,
    access: &wareboxes_core::models::TenantAccess,
    inventory_owner_id: i64,
    facility_id: i64,
    receiving_location_id: i64,
    item_id: i64,
    quantity: i64,
    license_plate_id: Option<i64>,
    license_plate_barcode: Option<&str>,
    key: &str,
) -> ReceiveExpectedInventoryResult {
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
        quantity,
        Some(key),
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
            received_qty: quantity,
            rejected_qty: 0,
            missing_qty: 0,
            license_plate_id,
            license_plate_barcode,
            lot: Some(key),
            serial: None,
            expiration: None,
            exception_reason: None::<InboundReceiptExceptionReason>,
            exception_note: None,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn loose_relocation_is_claimed_scanned_atomic_and_replay_safe() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("inventory-relocation@test.local").await;
    let access = default_tenant_for_user(&fixture.db, user.id)
        .await
        .expect("WMS user has tenant access");
    let tenant_id = access.tenant_id;
    let facility_id = fixture.facility(tenant_id, "Relocation DC").await;
    let inventory_owner_id = fixture.inventory_owner(tenant_id, "Relocation Owner").await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let item_id = fixture.item(tenant_id, "Relocation Item", "case").await;
    let source = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id,
                facility_id,
                item_id,
                qty: 10,
                key: "RELOCATION-SOURCE",
            },
        )
        .await;
    let destination_id = fixture
        .location(tenant_id, facility_id, "RELOCATION-A-01")
        .await;
    let incompatible_destination_id = fixture
        .location(tenant_id, facility_id, "RELOCATION-INCOMPATIBLE")
        .await;
    let incompatible_batch_id = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        inventory_owner_id,
        item_id,
        None,
        Some("RELOCATION-INCOMPATIBLE-LOT"),
        None,
        None,
    )
    .await
    .unwrap();
    repo::inventory::receive_inventory(
        &fixture.db,
        tenant_id,
        user.id,
        incompatible_batch_id,
        incompatible_destination_id,
        1,
        None,
        Some("incompatible relocation destination"),
        None,
        None,
        "relocation-incompatible-receipt",
    )
    .await
    .unwrap();
    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let incompatible = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/v1/inventory-relocation-tasks",
        Some("relocation-incompatible-destination"),
        Some(json!({
            "work": {
                "workflow": "loose_balance",
                "source_inventory_balance_id": source.balance_id,
                "quantity": 1
            },
            "destination_location_id": incompatible_destination_id
        })),
    )
    .await;
    assert_eq!(incompatible.status(), StatusCode::CONFLICT);
    let incompatible: ErrorResponse = response_json(incompatible).await;
    assert_eq!(incompatible.reason, ErrorReason::Conflict);
    assert_eq!(
        incompatible.message,
        "location already contains this item with a different lot or expiration"
    );

    let create_body = json!({
        "work": {
            "workflow": "loose_balance",
            "source_inventory_balance_id": source.balance_id,
            "quantity": 6
        },
        "destination_location_id": destination_id,
        "priority": 80,
        "assigned_user_id": user.id,
        "instructions": "Move the directed quantity"
    });
    let created = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/v1/inventory-relocation-tasks",
        Some("relocation-create"),
        Some(create_body.clone()),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    let created: CreateInventoryRelocationTaskResponse = response_json(created).await;

    let replayed_create = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/v1/inventory-relocation-tasks",
        Some("relocation-create"),
        Some(create_body),
    )
    .await;
    assert_eq!(replayed_create.status(), StatusCode::OK);
    assert_eq!(
        response_json::<CreateInventoryRelocationTaskResponse>(replayed_create).await,
        created
    );

    let claim_uri = format!("/api/v1/inventory-relocation-claims/{}", created.task_id);
    let claimed = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        &claim_uri,
        Some("relocation-claim"),
        Some(json!({})),
    )
    .await;
    assert_eq!(claimed.status(), StatusCode::OK);
    let claim: InventoryRelocationClaimResponse = response_json(claimed).await;
    assert_eq!(claim.task_id, created.task_id);
    assert_eq!(claim.source_location.location_id, source.location_id);
    assert_eq!(claim.destination_location.location_id, destination_id);
    assert_eq!(
        claim.destination_location.barcode.as_deref(),
        Some("RELOCATION-A-01")
    );

    let heartbeat_uri = format!(
        "/api/v1/inventory-relocation-claims/{}/heartbeats",
        created.task_id
    );
    let heartbeat = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        &heartbeat_uri,
        Some("relocation-heartbeat"),
        Some(json!({})),
    )
    .await;
    assert_eq!(heartbeat.status(), StatusCode::OK);
    let heartbeat: InventoryRelocationClaimHeartbeatResponse = response_json(heartbeat).await;
    let replayed_heartbeat = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        &heartbeat_uri,
        Some("relocation-heartbeat"),
        Some(json!({})),
    )
    .await;
    assert_eq!(replayed_heartbeat.status(), StatusCode::OK);
    assert_eq!(
        response_json::<InventoryRelocationClaimHeartbeatResponse>(replayed_heartbeat).await,
        heartbeat
    );

    let release_uri = format!(
        "/api/v1/inventory-relocation-claims/{}/releases",
        created.task_id
    );
    let release_body = json!({
        "reason": "work_interrupted",
        "note": "Shift handoff"
    });
    let released = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        &release_uri,
        Some("relocation-release"),
        Some(release_body.clone()),
    )
    .await;
    assert_eq!(released.status(), StatusCode::OK);
    let released: InventoryRelocationClaimReleaseResponse = response_json(released).await;
    assert_eq!(released.release_count, 1);
    let replayed_release = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        &release_uri,
        Some("relocation-release"),
        Some(release_body),
    )
    .await;
    assert_eq!(replayed_release.status(), StatusCode::OK);
    assert_eq!(
        response_json::<InventoryRelocationClaimReleaseResponse>(replayed_release).await,
        released
    );

    let reclaimed = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        &claim_uri,
        Some("relocation-reclaim"),
        Some(json!({})),
    )
    .await;
    assert_eq!(reclaimed.status(), StatusCode::OK);
    assert_eq!(
        response_json::<InventoryRelocationClaimResponse>(reclaimed)
            .await
            .task_id,
        created.task_id
    );

    let current = send(
        &app,
        &token,
        tenant_id,
        Method::GET,
        "/api/v1/inventory-relocation-claims/current",
        None,
        None,
    )
    .await;
    assert_eq!(current.status(), StatusCode::OK);
    assert_eq!(
        response_json::<Option<InventoryRelocationClaimResponse>>(current)
            .await
            .expect("claim remains recoverable")
            .task_id,
        created.task_id
    );

    let confirmation_uri = format!(
        "/api/v1/inventory-relocation-tasks/{}/confirmations",
        created.task_id
    );
    let wrong_scan = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        &confirmation_uri,
        Some("relocation-wrong-scan"),
        Some(json!({"destination_location_barcode": "RELOCATION-WRONG"})),
    )
    .await;
    assert_eq!(wrong_scan.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(wrong_scan).await.reason,
        ErrorReason::Conflict
    );

    let confirmation_body = json!({
        "destination_location_barcode": "RELOCATION-A-01"
    });
    let confirmed = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        &confirmation_uri,
        Some("relocation-confirm"),
        Some(confirmation_body.clone()),
    )
    .await;
    assert_eq!(confirmed.status(), StatusCode::OK);
    let confirmed: InventoryRelocationConfirmationResponse = response_json(confirmed).await;
    assert_eq!(confirmed.task_id, created.task_id);
    assert_eq!(confirmed.source_location_id, source.location_id);
    assert_eq!(confirmed.destination_location_id, destination_id);
    let destination_balance_id = match confirmed.result {
        InventoryRelocationResult::LooseBalance {
            source_inventory_balance_id,
            destination_inventory_balance_id,
            quantity,
            ..
        } => {
            assert_eq!(source_inventory_balance_id, source.balance_id);
            assert_eq!(quantity, 6);
            destination_inventory_balance_id
        }
        InventoryRelocationResult::LicensePlate { .. } => {
            panic!("expected loose relocation result")
        }
    };

    let replayed = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        &confirmation_uri,
        Some("relocation-confirm"),
        Some(confirmation_body),
    )
    .await;
    assert_eq!(replayed.status(), StatusCode::OK);
    assert_eq!(
        response_json::<InventoryRelocationConfirmationResponse>(replayed).await,
        confirmed
    );

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let effects: (i64, i64, i64, i64, i64, String, bool) = sqlx::query_as(
        r#"
        SELECT
            (SELECT qty_on_hand FROM inventory_balances WHERE id = $1),
            (SELECT qty_on_hand FROM inventory_balances WHERE id = $2),
            (
                SELECT COUNT(*)
                FROM inventory_entries
                WHERE transaction_id = $3
            ),
            (
                SELECT COALESCE(SUM(quantity_delta), 0)::BIGINT
                FROM inventory_entries
                WHERE transaction_id = $3
            ),
            (
                SELECT COUNT(*)
                FROM inventory_relocation_results
                WHERE task_id = $4
            ),
            (SELECT status FROM work_tasks WHERE id = $4),
            (
                SELECT closed_at IS NOT NULL
                FROM inventory_relocation_tasks
                WHERE task_id = $4
            )
        "#,
    )
    .bind(source.balance_id)
    .bind(destination_balance_id)
    .bind(confirmed.inventory_transaction_id)
    .bind(created.task_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(effects.0, 4);
    assert_eq!(effects.1, 6);
    assert_eq!(effects.2, 2);
    assert_eq!(effects.3, 0);
    assert_eq!(effects.4, 1);
    assert_eq!(effects.5, "completed");
    assert!(effects.6);
}

#[tokio::test]
async fn license_plate_relocation_requires_plate_scan_and_moves_all_contents_atomically() {
    let fixture = Fixture::new().await;
    let user = fixture
        .wms_user("license-plate-relocation@test.local")
        .await;
    let access = default_tenant_for_user(&fixture.db, user.id)
        .await
        .expect("WMS user has tenant access");
    let tenant_id = access.tenant_id;
    let facility_id = fixture.facility(tenant_id, "Plate Relocation DC").await;
    let inventory_owner_id = fixture
        .inventory_owner(tenant_id, "Plate Relocation Owner")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let source_location_id = repo::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some("PLATE-RELOCATION-SOURCE"),
        Some("Plate Relocation Source"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let destination_location_id = fixture
        .location(tenant_id, facility_id, "PLATE-RELOCATION-A-01")
        .await;
    let item_a = fixture.item(tenant_id, "Plate Item A", "case").await;
    let item_b = fixture.item(tenant_id, "Plate Item B", "each").await;
    let plate_barcode = "LP-RELOCATION-001";
    let first = receive_plate_stock(
        &fixture,
        &access,
        inventory_owner_id,
        facility_id,
        source_location_id,
        item_a,
        4,
        None,
        Some(plate_barcode),
        "plate-relocation-a",
    )
    .await;
    let license_plate_id = first
        .license_plate_id
        .expect("container receipt creates a license plate");
    let second = receive_plate_stock(
        &fixture,
        &access,
        inventory_owner_id,
        facility_id,
        source_location_id,
        item_b,
        7,
        Some(license_plate_id),
        None,
        "plate-relocation-b",
    )
    .await;
    assert_ne!(first.inventory_balance_id, second.inventory_balance_id);

    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let created = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/v1/inventory-relocation-tasks",
        Some("plate-relocation-create"),
        Some(json!({
            "work": {
                "workflow": "license_plate",
                "license_plate_id": license_plate_id
            },
            "destination_location_id": destination_location_id,
            "priority": 90,
            "assigned_user_id": user.id
        })),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    let created: CreateInventoryRelocationTaskResponse = response_json(created).await;
    let claim_uri = format!("/api/v1/inventory-relocation-claims/{}", created.task_id);
    let claimed = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        &claim_uri,
        Some("plate-relocation-claim"),
        Some(json!({})),
    )
    .await;
    assert_eq!(claimed.status(), StatusCode::OK);

    let confirmation_uri = format!(
        "/api/v1/inventory-relocation-tasks/{}/confirmations",
        created.task_id
    );
    let missing_plate_scan = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        &confirmation_uri,
        Some("plate-relocation-missing-scan"),
        Some(json!({
            "destination_location_barcode": "PLATE-RELOCATION-A-01"
        })),
    )
    .await;
    assert_eq!(missing_plate_scan.status(), StatusCode::BAD_REQUEST);

    let wrong_plate_scan = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        &confirmation_uri,
        Some("plate-relocation-wrong-scan"),
        Some(json!({
            "destination_location_barcode": "PLATE-RELOCATION-A-01",
            "license_plate_barcode": "LP-WRONG"
        })),
    )
    .await;
    assert_eq!(wrong_plate_scan.status(), StatusCode::CONFLICT);

    let confirmed = send(
        &app,
        &token,
        tenant_id,
        Method::POST,
        &confirmation_uri,
        Some("plate-relocation-confirm"),
        Some(json!({
            "destination_location_barcode": "PLATE-RELOCATION-A-01",
            "license_plate_barcode": plate_barcode
        })),
    )
    .await;
    assert_eq!(confirmed.status(), StatusCode::OK);
    let confirmed: InventoryRelocationConfirmationResponse = response_json(confirmed).await;
    assert_eq!(confirmed.task_id, created.task_id);
    assert_eq!(confirmed.source_location_id, source_location_id);
    assert_eq!(confirmed.destination_location_id, destination_location_id);
    match confirmed.result {
        InventoryRelocationResult::LicensePlate {
            license_plate_id: result_plate_id,
            license_plate_barcode,
            moved_balance_count,
        } => {
            assert_eq!(result_plate_id, license_plate_id);
            assert_eq!(license_plate_barcode, plate_barcode);
            assert_eq!(moved_balance_count, 2);
        }
        InventoryRelocationResult::LooseBalance { .. } => {
            panic!("expected license plate relocation result")
        }
    }

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let effects: (Option<i64>, i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT location_id FROM license_plates WHERE id = $1),
            (
                SELECT COUNT(*)
                FROM inventory_balances
                WHERE license_plate_id = $1
                  AND location_id = $2
                  AND deleted IS NULL
                  AND qty_on_hand > 0
            ),
            (
                SELECT COUNT(*)
                FROM inventory_balances
                WHERE license_plate_id = $1
                  AND location_id = $3
                  AND deleted IS NULL
                  AND qty_on_hand > 0
            ),
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
                FROM inventory_relocation_results
                WHERE task_id = $5
            )
        "#,
    )
    .bind(license_plate_id)
    .bind(source_location_id)
    .bind(destination_location_id)
    .bind(confirmed.inventory_transaction_id)
    .bind(created.task_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(effects.0, Some(destination_location_id));
    assert_eq!(effects.1, 0);
    assert_eq!(effects.2, 2);
    assert_eq!(effects.3, 4);
    assert_eq!(effects.4, 0);
    assert_eq!(effects.5, 1);
}

#[tokio::test]
async fn relocation_routes_conceal_cross_scope_and_cross_tenant_identifiers() {
    let fixture = Fixture::new().await;
    let administrator = fixture.wms_user("relocation-scope-admin@test.local").await;
    let administrator_access = default_tenant_for_user(&fixture.db, administrator.id)
        .await
        .expect("administrator has tenant access");
    let tenant_id = administrator_access.tenant_id;
    let operator = fixture.user("relocation-scope-operator@test.local").await;
    add_membership(&fixture.db, tenant_id, operator.id).await;
    grant_wms_role(
        &fixture.db,
        tenant_id,
        &[operator.id],
        "relocation-scope-operator",
    )
    .await;

    let allowed_facility = fixture
        .facility(tenant_id, "Relocation Scope Allowed Facility")
        .await;
    let denied_facility = fixture
        .facility(tenant_id, "Relocation Scope Denied Facility")
        .await;
    let allowed_owner = fixture
        .inventory_owner(tenant_id, "Relocation Scope Allowed Owner")
        .await;
    let denied_owner = fixture
        .inventory_owner(tenant_id, "Relocation Scope Denied Owner")
        .await;
    for (owner_id, facility_id) in [
        (allowed_owner, allowed_facility),
        (denied_owner, allowed_facility),
        (allowed_owner, denied_facility),
    ] {
        fixture
            .assign_owner_to_facility(tenant_id, owner_id, facility_id)
            .await;
    }
    let item_id = fixture
        .item(tenant_id, "Relocation Scope Item", "each")
        .await;
    let owner_denied_source = fixture
        .received_balance(
            &administrator_access,
            ReceivedBalanceSetup {
                inventory_owner_id: denied_owner,
                facility_id: allowed_facility,
                item_id,
                qty: 10,
                key: "RELOCATION-SCOPE-OWNER-DENIED-SOURCE",
            },
        )
        .await;
    let facility_denied_source = fixture
        .received_balance(
            &administrator_access,
            ReceivedBalanceSetup {
                inventory_owner_id: allowed_owner,
                facility_id: denied_facility,
                item_id,
                qty: 10,
                key: "RELOCATION-SCOPE-FACILITY-DENIED-SOURCE",
            },
        )
        .await;
    let allowed_destination = fixture
        .location(
            tenant_id,
            allowed_facility,
            "RELOCATION-SCOPE-ALLOWED-DESTINATION",
        )
        .await;
    let denied_destination = fixture
        .location(
            tenant_id,
            denied_facility,
            "RELOCATION-SCOPE-DENIED-DESTINATION",
        )
        .await;
    set_scope(
        &fixture.db,
        tenant_id,
        operator.id,
        vec![allowed_facility],
        vec![allowed_owner],
    )
    .await;

    let foreign_administrator = fixture
        .wms_user("relocation-scope-foreign@test.local")
        .await;
    let foreign_access = default_tenant_for_user(&fixture.db, foreign_administrator.id)
        .await
        .expect("foreign administrator has tenant access");
    let foreign_tenant_id = foreign_access.tenant_id;
    let foreign_facility = fixture
        .facility(foreign_tenant_id, "Relocation Scope Foreign Facility")
        .await;
    let foreign_owner = fixture
        .inventory_owner(foreign_tenant_id, "Relocation Scope Foreign Owner")
        .await;
    fixture
        .assign_owner_to_facility(foreign_tenant_id, foreign_owner, foreign_facility)
        .await;
    let foreign_item = fixture
        .item(foreign_tenant_id, "Relocation Scope Foreign Item", "each")
        .await;
    let foreign_source = fixture
        .received_balance(
            &foreign_access,
            ReceivedBalanceSetup {
                inventory_owner_id: foreign_owner,
                facility_id: foreign_facility,
                item_id: foreign_item,
                qty: 10,
                key: "RELOCATION-SCOPE-FOREIGN-SOURCE",
            },
        )
        .await;
    let foreign_destination = fixture
        .location(
            foreign_tenant_id,
            foreign_facility,
            "RELOCATION-SCOPE-FOREIGN-DESTINATION",
        )
        .await;

    let operator_token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let administrator_token = auth::create_session(&fixture.db, administrator.id)
        .await
        .unwrap();
    let foreign_token = auth::create_session(&fixture.db, foreign_administrator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let before_denied_creates: (i64, i64) = {
        let mut tx = tenant_tx(&fixture.db, tenant_id).await;
        let counts = sqlx::query_as(
            r#"
            SELECT
                (
                    SELECT COUNT(*)
                    FROM work_tasks
                    WHERE tenant_id = $1
                      AND task_type = 'inventory_relocation'
                ),
                (
                    SELECT COUNT(*)
                    FROM inventory_relocation_tasks
                    WHERE tenant_id = $1
                )
            "#,
        )
        .bind(tenant_id.get())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        counts
    };
    for (key, source_id, destination_id) in [
        (
            "relocation-owner-scope-denied",
            owner_denied_source.balance_id,
            allowed_destination,
        ),
        (
            "relocation-facility-scope-denied",
            facility_denied_source.balance_id,
            denied_destination,
        ),
        (
            "relocation-cross-tenant-source-guessed",
            foreign_source.balance_id,
            allowed_destination,
        ),
    ] {
        let response = send(
            &app,
            &operator_token,
            tenant_id,
            Method::POST,
            "/api/v1/inventory-relocation-tasks",
            Some(key),
            Some(json!({
                "work": {
                    "workflow": "loose_balance",
                    "source_inventory_balance_id": source_id,
                    "quantity": 2
                },
                "destination_location_id": destination_id
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{key}");
        assert_eq!(
            response_json::<ErrorResponse>(response).await.reason,
            ErrorReason::NotFound,
            "{key}"
        );
    }
    let after_denied_creates: (i64, i64) = {
        let mut tx = tenant_tx(&fixture.db, tenant_id).await;
        let counts = sqlx::query_as(
            r#"
            SELECT
                (
                    SELECT COUNT(*)
                    FROM work_tasks
                    WHERE tenant_id = $1
                      AND task_type = 'inventory_relocation'
                ),
                (
                    SELECT COUNT(*)
                    FROM inventory_relocation_tasks
                    WHERE tenant_id = $1
                )
            "#,
        )
        .bind(tenant_id.get())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        counts
    };
    assert_eq!(after_denied_creates, before_denied_creates);

    let mut concealed_task_ids = Vec::new();
    for (key, source_id, destination_id) in [
        (
            "relocation-owner-scope-hidden-task",
            owner_denied_source.balance_id,
            allowed_destination,
        ),
        (
            "relocation-facility-scope-hidden-task",
            facility_denied_source.balance_id,
            denied_destination,
        ),
    ] {
        let response = send(
            &app,
            &administrator_token,
            tenant_id,
            Method::POST,
            "/api/v1/inventory-relocation-tasks",
            Some(key),
            Some(json!({
                "work": {
                    "workflow": "loose_balance",
                    "source_inventory_balance_id": source_id,
                    "quantity": 2
                },
                "destination_location_id": destination_id
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{key}");
        concealed_task_ids.push(
            response_json::<CreateInventoryRelocationTaskResponse>(response)
                .await
                .task_id,
        );
    }
    let foreign_task = send(
        &app,
        &foreign_token,
        foreign_tenant_id,
        Method::POST,
        "/api/v1/inventory-relocation-tasks",
        Some("relocation-foreign-hidden-task"),
        Some(json!({
            "work": {
                "workflow": "loose_balance",
                "source_inventory_balance_id": foreign_source.balance_id,
                "quantity": 2
            },
            "destination_location_id": foreign_destination
        })),
    )
    .await;
    assert_eq!(foreign_task.status(), StatusCode::OK);
    let foreign_task_id = response_json::<CreateInventoryRelocationTaskResponse>(foreign_task)
        .await
        .task_id;
    concealed_task_ids.push(foreign_task_id);

    let scoped_counts_before: (i64, i64, i64) = {
        let mut tx = tenant_tx(&fixture.db, tenant_id).await;
        let counts = sqlx::query_as(
            r#"
            SELECT
                (
                    SELECT COUNT(*)
                    FROM work_tasks
                    WHERE tenant_id = $1
                      AND task_type = 'inventory_relocation'
                ),
                (
                    SELECT COUNT(*)
                    FROM inventory_relocation_results
                    WHERE tenant_id = $1
                ),
                (
                    SELECT COUNT(*)
                    FROM inventory_transactions
                    WHERE tenant_id = $1
                      AND operation = 'task.confirm_inventory_relocation.v1'
                )
            "#,
        )
        .bind(tenant_id.get())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        counts
    };

    for (index, task_id) in concealed_task_ids.iter().copied().enumerate() {
        let claim_uri = format!("/api/v1/inventory-relocation-claims/{task_id}");
        let claim = send(
            &app,
            &operator_token,
            tenant_id,
            Method::POST,
            &claim_uri,
            Some(&format!("relocation-hidden-claim-{index}")),
            Some(json!({})),
        )
        .await;
        assert_eq!(claim.status(), StatusCode::NOT_FOUND, "task {task_id}");
        assert_eq!(
            response_json::<ErrorResponse>(claim).await.reason,
            ErrorReason::NotFound,
            "task {task_id}"
        );

        let confirmation_uri =
            format!("/api/v1/inventory-relocation-tasks/{task_id}/confirmations");
        let confirmation = send(
            &app,
            &operator_token,
            tenant_id,
            Method::POST,
            &confirmation_uri,
            Some(&format!("relocation-hidden-confirm-{index}")),
            Some(json!({
                "destination_location_barcode": "RELOCATION-SCOPE-ALLOWED-DESTINATION"
            })),
        )
        .await;
        assert_eq!(
            confirmation.status(),
            StatusCode::NOT_FOUND,
            "task {task_id}"
        );
        assert_eq!(
            response_json::<ErrorResponse>(confirmation).await.reason,
            ErrorReason::NotFound,
            "task {task_id}"
        );
    }

    let cross_tenant_claim = send(
        &app,
        &operator_token,
        foreign_tenant_id,
        Method::POST,
        &format!("/api/v1/inventory-relocation-claims/{foreign_task_id}"),
        Some("relocation-cross-tenant-task"),
        Some(json!({})),
    )
    .await;
    assert_eq!(cross_tenant_claim.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response_json::<ErrorResponse>(cross_tenant_claim)
            .await
            .reason,
        ErrorReason::Forbidden
    );

    let scoped_counts_after: (i64, i64, i64) = {
        let mut tx = tenant_tx(&fixture.db, tenant_id).await;
        let counts = sqlx::query_as(
            r#"
            SELECT
                (
                    SELECT COUNT(*)
                    FROM work_tasks
                    WHERE tenant_id = $1
                      AND task_type = 'inventory_relocation'
                ),
                (
                    SELECT COUNT(*)
                    FROM inventory_relocation_results
                    WHERE tenant_id = $1
                ),
                (
                    SELECT COUNT(*)
                    FROM inventory_transactions
                    WHERE tenant_id = $1
                      AND operation = 'task.confirm_inventory_relocation.v1'
                )
            "#,
        )
        .bind(tenant_id.get())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        counts
    };
    assert_eq!(scoped_counts_after, scoped_counts_before);

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let untouched_hidden_tasks: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM work_tasks
        WHERE tenant_id = $1
          AND id = ANY($2)
          AND status = 'open'
          AND assigned_user_id IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(&concealed_task_ids[..2])
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(untouched_hidden_tasks, 2);
}

#[tokio::test]
async fn concurrent_create_claim_and_confirm_conserve_inventory_and_work_ownership() {
    let fixture = Fixture::new().await;
    let administrator = fixture.wms_user("relocation-race-admin@test.local").await;
    let administrator_access = default_tenant_for_user(&fixture.db, administrator.id)
        .await
        .expect("administrator has tenant access");
    let tenant_id = administrator_access.tenant_id;
    let worker_one = fixture.user("relocation-race-worker-one@test.local").await;
    let worker_two = fixture.user("relocation-race-worker-two@test.local").await;
    add_membership(&fixture.db, tenant_id, worker_one.id).await;
    add_membership(&fixture.db, tenant_id, worker_two.id).await;
    grant_wms_role(
        &fixture.db,
        tenant_id,
        &[worker_one.id, worker_two.id],
        "relocation-race-workers",
    )
    .await;

    let facility_id = fixture
        .facility(tenant_id, "Relocation Race Facility")
        .await;
    let inventory_owner_id = fixture
        .inventory_owner(tenant_id, "Relocation Race Owner")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    for user_id in [worker_one.id, worker_two.id] {
        set_scope(
            &fixture.db,
            tenant_id,
            user_id,
            vec![facility_id],
            vec![inventory_owner_id],
        )
        .await;
    }
    let item_id = fixture
        .item(tenant_id, "Relocation Race Item", "each")
        .await;
    let source = fixture
        .received_balance(
            &administrator_access,
            ReceivedBalanceSetup {
                inventory_owner_id,
                facility_id,
                item_id,
                qty: 10,
                key: "RELOCATION-RACE-SOURCE",
            },
        )
        .await;
    let destination_id = fixture
        .location(tenant_id, facility_id, "RELOCATION-RACE-DESTINATION")
        .await;
    let administrator_token = auth::create_session(&fixture.db, administrator.id)
        .await
        .unwrap();
    let worker_one_token = auth::create_session(&fixture.db, worker_one.id)
        .await
        .unwrap();
    let worker_two_token = auth::create_session(&fixture.db, worker_two.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let create_body = json!({
        "work": {
            "workflow": "loose_balance",
            "source_inventory_balance_id": source.balance_id,
            "quantity": 4
        },
        "destination_location_id": destination_id
    });

    let (first_create, second_create) = timeout(CONCURRENT_REQUEST_TIMEOUT, async {
        tokio::join!(
            send(
                &app,
                &administrator_token,
                tenant_id,
                Method::POST,
                "/api/v1/inventory-relocation-tasks",
                Some("relocation-race-create-one"),
                Some(create_body.clone()),
            ),
            send(
                &app,
                &administrator_token,
                tenant_id,
                Method::POST,
                "/api/v1/inventory-relocation-tasks",
                Some("relocation-race-create-two"),
                Some(create_body),
            )
        )
    })
    .await
    .expect("concurrent relocation creation completes within the bound");
    let first_create_won = first_create.status() == StatusCode::OK;
    assert_ne!(
        first_create_won,
        second_create.status() == StatusCode::OK,
        "exactly one independent command can create movement work for a source"
    );
    let (created, rejected) = if first_create_won {
        (first_create, second_create)
    } else {
        (second_create, first_create)
    };
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(rejected).await.reason,
        ErrorReason::Conflict
    );
    let task_id = response_json::<CreateInventoryRelocationTaskResponse>(created)
        .await
        .task_id;

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let created_work: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (
                SELECT COUNT(*)
                FROM work_tasks
                WHERE tenant_id = $1
                  AND task_type = 'inventory_relocation'
            ),
            (
                SELECT COUNT(*)
                FROM inventory_relocation_tasks
                WHERE tenant_id = $1
                  AND source_inventory_balance_id = $2
                  AND closed_at IS NULL
            )
        "#,
    )
    .bind(tenant_id.get())
    .bind(source.balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(created_work, (1, 1));

    let claim_uri = format!("/api/v1/inventory-relocation-claims/{task_id}");
    let (first_claim, second_claim) = timeout(CONCURRENT_REQUEST_TIMEOUT, async {
        tokio::join!(
            send(
                &app,
                &worker_one_token,
                tenant_id,
                Method::POST,
                &claim_uri,
                Some("relocation-race-claim-one"),
                Some(json!({})),
            ),
            send(
                &app,
                &worker_two_token,
                tenant_id,
                Method::POST,
                &claim_uri,
                Some("relocation-race-claim-two"),
                Some(json!({})),
            )
        )
    })
    .await
    .expect("concurrent relocation claims complete within the bound");
    let first_claim_won = first_claim.status() == StatusCode::OK;
    assert_ne!(
        first_claim_won,
        second_claim.status() == StatusCode::OK,
        "exactly one operator can own a relocation claim"
    );
    let (claimed, rejected_claim, winner_token, winner_id) = if first_claim_won {
        (
            first_claim,
            second_claim,
            worker_one_token.as_str(),
            worker_one.id,
        )
    } else {
        (
            second_claim,
            first_claim,
            worker_two_token.as_str(),
            worker_two.id,
        )
    };
    assert_eq!(rejected_claim.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(rejected_claim).await.reason,
        ErrorReason::Conflict
    );
    assert_eq!(
        response_json::<InventoryRelocationClaimResponse>(claimed)
            .await
            .task_id,
        task_id
    );

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let ownership: (String, Option<i64>, i64) = sqlx::query_as(
        r#"
        SELECT
            task.status,
            task.assigned_user_id,
            (
                SELECT COUNT(*)
                FROM work_task_progress progress
                WHERE progress.tenant_id = task.tenant_id
                  AND progress.task_id = task.id
                  AND progress.action = 'started'
            )
        FROM work_tasks task
        WHERE task.tenant_id = $1 AND task.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(ownership, ("in_progress".to_owned(), Some(winner_id), 1));

    let confirmation_uri = format!("/api/v1/inventory-relocation-tasks/{task_id}/confirmations");
    let confirmation_body = json!({"destination_location_barcode": "RELOCATION-RACE-DESTINATION"});
    let (first_confirmation, second_confirmation) = timeout(CONCURRENT_REQUEST_TIMEOUT, async {
        tokio::join!(
            send(
                &app,
                winner_token,
                tenant_id,
                Method::POST,
                &confirmation_uri,
                Some("relocation-race-confirm"),
                Some(confirmation_body.clone()),
            ),
            send(
                &app,
                winner_token,
                tenant_id,
                Method::POST,
                &confirmation_uri,
                Some("relocation-race-confirm"),
                Some(confirmation_body),
            )
        )
    })
    .await
    .expect("duplicate concurrent confirmations complete within the bound");
    assert_eq!(first_confirmation.status(), StatusCode::OK);
    assert_eq!(second_confirmation.status(), StatusCode::OK);
    let first_confirmation: InventoryRelocationConfirmationResponse =
        response_json(first_confirmation).await;
    let second_confirmation: InventoryRelocationConfirmationResponse =
        response_json(second_confirmation).await;
    assert_eq!(second_confirmation, first_confirmation);
    let inventory_transaction_id = first_confirmation.inventory_transaction_id;
    let destination_balance_id = match first_confirmation.result {
        InventoryRelocationResult::LooseBalance {
            source_inventory_balance_id,
            destination_inventory_balance_id,
            quantity,
            ..
        } => {
            assert_eq!(source_inventory_balance_id, source.balance_id);
            assert_eq!(quantity, 4);
            destination_inventory_balance_id
        }
        InventoryRelocationResult::LicensePlate { .. } => {
            panic!("expected loose relocation result")
        }
    };

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let effects: (i64, i64, i64, i64, i64, i64, i64, String, bool) = sqlx::query_as(
        r#"
        SELECT
            (SELECT qty_on_hand FROM inventory_balances WHERE id = $1),
            (SELECT qty_on_hand FROM inventory_balances WHERE id = $2),
            (
                SELECT COUNT(*)
                FROM inventory_transactions
                WHERE tenant_id = $3
                  AND operation = 'task.confirm_inventory_relocation.v1'
            ),
            (
                SELECT COUNT(*)
                FROM inventory_entries
                WHERE tenant_id = $3
                  AND transaction_id = $4
            ),
            (
                SELECT COALESCE(SUM(quantity_delta), 0)::BIGINT
                FROM inventory_entries
                WHERE tenant_id = $3
                  AND transaction_id = $4
            ),
            (
                SELECT COUNT(*)
                FROM inventory_relocation_results
                WHERE tenant_id = $3
                  AND task_id = $5
            ),
            (
                SELECT COUNT(*)
                FROM command_idempotency_records
                WHERE tenant_id = $3
                  AND operation = 'task.confirm_inventory_relocation.v1'
                  AND idempotency_key = 'relocation-race-confirm'
            ),
            (SELECT status FROM work_tasks WHERE tenant_id = $3 AND id = $5),
            (
                SELECT closed_at IS NOT NULL
                FROM inventory_relocation_tasks
                WHERE tenant_id = $3 AND task_id = $5
            )
        "#,
    )
    .bind(source.balance_id)
    .bind(destination_balance_id)
    .bind(tenant_id.get())
    .bind(inventory_transaction_id)
    .bind(task_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(effects.0, 6);
    assert_eq!(effects.1, 4);
    assert_eq!(effects.2, 1);
    assert_eq!(effects.3, 2);
    assert_eq!(effects.4, 0);
    assert_eq!(effects.5, 1);
    assert_eq!(effects.6, 1);
    assert_eq!(effects.7, "completed");
    assert!(effects.8);
}
