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
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_core::models::InboundReceiptExceptionReason;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const DETAIL_TABLES: [&str; 3] = [
    "license_plate_putaway_tasks",
    "license_plate_putaway_task_contents",
    "license_plate_putaway_results",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
struct PutawayEffects {
    tasks: i64,
    details: i64,
    contents: i64,
    results: i64,
    transactions: i64,
    entries: i64,
    command_records: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct TablePrivileges {
    table_name: String,
    can_select: bool,
    can_insert: bool,
    can_update: bool,
    can_delete: bool,
    can_truncate: bool,
    can_reference: bool,
    can_trigger: bool,
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
        REQUEST_TIMEOUT,
        app.clone()
            .oneshot(request(token, tenant_id, uri, idempotency_key, body)),
    )
    .await
    .expect("license plate putaway authorization request completes within the bound")
    .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
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
async fn receive_license_plate(
    fixture: &Fixture,
    access: &wareboxes_core::models::TenantAccess,
    actor_user_id: i64,
    facility_id: i64,
    inventory_owner_id: i64,
    receiving_location_id: i64,
    item_id: i64,
    barcode: &str,
) -> i64 {
    let load_id = repo::loads::add_load(
        &fixture.db,
        access.tenant_id,
        actor_user_id,
        facility_id,
        inventory_owner_id,
        LoadType::Inbound,
        Some(barcode),
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
        actor_user_id,
        load_id,
        item_id,
        None,
        10,
        Some(barcode),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(repo::loads::update_load(
        &fixture.db,
        access.tenant_id,
        actor_user_id,
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
        &format!("{barcode}-unloading"),
    )
    .await;
    repo::inbound_receipt::receive_expected_inventory(
        &fixture.db,
        access,
        &command(access, &format!("{barcode}-receipt")),
        load_line_id,
        &repo::inbound_receipt::ReceiveExpectedInventoryCommand {
            receiving_location_id: Some(receiving_location_id),
            received_qty: 10,
            rejected_qty: 0,
            missing_qty: 0,
            license_plate_id: None,
            license_plate_barcode: Some(barcode),
            lot: Some(barcode),
            serial: None,
            expiration: None,
            exception_reason: None::<InboundReceiptExceptionReason>,
            exception_note: None,
        },
    )
    .await
    .unwrap()
    .license_plate_id
    .expect("a containerized receipt identifies its license plate")
}

async fn effects(db: &db::Db, tenant_id: TenantId) -> PutawayEffects {
    let mut tx = tenant_tx(db, tenant_id).await;
    let effects = sqlx::query_as(
        r#"
        SELECT
            (
                SELECT COUNT(*)
                FROM work_tasks
                WHERE tenant_id = $1
                  AND task_type = 'license_plate_putaway'
            ) AS tasks,
            (
                SELECT COUNT(*)
                FROM license_plate_putaway_tasks
                WHERE tenant_id = $1
            ) AS details,
            (
                SELECT COUNT(*)
                FROM license_plate_putaway_task_contents
                WHERE tenant_id = $1
            ) AS contents,
            (
                SELECT COUNT(*)
                FROM license_plate_putaway_results
                WHERE tenant_id = $1
            ) AS results,
            (
                SELECT COUNT(*)
                FROM inventory_transactions
                WHERE tenant_id = $1
                  AND operation = 'task.confirm_license_plate_putaway.v1'
            ) AS transactions,
            (
                SELECT COUNT(*)
                FROM inventory_entries entry
                INNER JOIN inventory_transactions transaction
                    ON transaction.tenant_id = entry.tenant_id
                   AND transaction.inventory_owner_id =
                       entry.inventory_owner_id
                   AND transaction.id = entry.transaction_id
                WHERE entry.tenant_id = $1
                  AND transaction.operation =
                      'task.confirm_license_plate_putaway.v1'
            ) AS entries,
            (
                SELECT COUNT(*)
                FROM command_idempotency_records
                WHERE tenant_id = $1
                  AND operation IN (
                      'task.create_license_plate_putaway.v1',
                      'task.confirm_license_plate_putaway.v1'
                  )
            ) AS command_records
        "#,
    )
    .bind(tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    effects
}

async fn inventory_snapshot(
    db: &db::Db,
    tenant_id: TenantId,
    license_plate_id: i64,
    source_location_id: i64,
    destination_location_id: i64,
) -> (Option<i64>, i64, i64, i64) {
    let mut tx = tenant_tx(db, tenant_id).await;
    let snapshot = sqlx::query_as(
        r#"
        SELECT
            (
                SELECT location_id
                FROM license_plates
                WHERE tenant_id = $1 AND id = $2
            ),
            (
                SELECT COUNT(*)
                FROM inventory_balances
                WHERE tenant_id = $1
                  AND license_plate_id = $2
                  AND location_id = $3
                  AND deleted IS NULL
                  AND qty_on_hand > 0
            ),
            (
                SELECT COUNT(*)
                FROM inventory_balances
                WHERE tenant_id = $1
                  AND license_plate_id = $2
                  AND location_id = $4
                  AND deleted IS NULL
                  AND qty_on_hand > 0
            ),
            (
                SELECT COALESCE(SUM(qty_on_hand), 0)::BIGINT
                FROM inventory_balances
                WHERE tenant_id = $1
                  AND license_plate_id = $2
                  AND deleted IS NULL
            )
        "#,
    )
    .bind(tenant_id.get())
    .bind(license_plate_id)
    .bind(source_location_id)
    .bind(destination_location_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    snapshot
}

async fn assert_not_found(response: axum::response::Response) {
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response_json::<ErrorResponse>(response).await.reason,
        ErrorReason::NotFound
    );
}

async fn assert_exact_runtime_privileges(db: &db::Db) {
    let privileges: Vec<TablePrivileges> = sqlx::query_as(
        r#"
        SELECT
            table_name,
            has_table_privilege(
                current_user,
                'public.' || table_name,
                'SELECT'
            ) AS can_select,
            has_table_privilege(
                current_user,
                'public.' || table_name,
                'INSERT'
            ) AS can_insert,
            has_table_privilege(
                current_user,
                'public.' || table_name,
                'UPDATE'
            ) AS can_update,
            has_table_privilege(
                current_user,
                'public.' || table_name,
                'DELETE'
            ) AS can_delete,
            has_table_privilege(
                current_user,
                'public.' || table_name,
                'TRUNCATE'
            ) AS can_truncate,
            has_table_privilege(
                current_user,
                'public.' || table_name,
                'REFERENCES'
            ) AS can_reference,
            has_table_privilege(
                current_user,
                'public.' || table_name,
                'TRIGGER'
            ) AS can_trigger
        FROM unnest($1::TEXT[]) WITH ORDINALITY AS tables(table_name, ordinal)
        ORDER BY ordinal
        "#,
    )
    .bind(DETAIL_TABLES.as_slice())
    .fetch_all(db)
    .await
    .unwrap();
    assert_eq!(
        privileges,
        DETAIL_TABLES
            .iter()
            .map(|table_name| TablePrivileges {
                table_name: (*table_name).to_owned(),
                can_select: true,
                can_insert: true,
                can_update: false,
                can_delete: false,
                can_truncate: false,
                can_reference: false,
                can_trigger: false,
            })
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn license_plate_putaway_scopes_rls_and_acl_fail_closed() {
    let fixture = Fixture::new().await;
    let administrator = fixture
        .wms_user("license-plate-putaway-scope-admin@test.local")
        .await;
    let administrator_access = default_tenant_for_user(&fixture.db, administrator.id)
        .await
        .unwrap();
    let tenant_id = administrator_access.tenant_id;

    let operator = fixture
        .user("license-plate-putaway-scope-operator@test.local")
        .await;
    let foreign_tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    add_membership(&fixture.db, tenant_id, operator.id).await;
    let wms_permission =
        wareboxes_persistence_postgres::permissions::find_by_name(&fixture.db, tenant_id, "wms")
            .await
            .unwrap()
            .unwrap();
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        "license-plate-putaway-scope-operator",
        Some("Restricted license plate putaway operator"),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        wms_permission.id,
    )
    .await
    .unwrap());
    assert!(wareboxes_persistence_postgres::roles::add_role_to_user(
        &fixture.db,
        tenant_id,
        operator.id,
        role,
    )
    .await
    .unwrap());

    let allowed_facility = fixture
        .facility(tenant_id, "License Plate Putaway Scope Allowed Facility")
        .await;
    let denied_facility = fixture
        .facility(tenant_id, "License Plate Putaway Scope Denied Facility")
        .await;
    let allowed_owner = fixture
        .inventory_owner(tenant_id, "License Plate Putaway Scope Allowed Owner")
        .await;
    let denied_owner = fixture
        .inventory_owner(tenant_id, "License Plate Putaway Scope Denied Owner")
        .await;
    for (owner_id, facility_id) in [
        (allowed_owner, allowed_facility),
        (allowed_owner, denied_facility),
        (denied_owner, allowed_facility),
    ] {
        fixture
            .assign_owner_to_facility(tenant_id, owner_id, facility_id)
            .await;
    }

    let allowed_receiving = wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        allowed_facility,
        None,
        Some("LP-PUTAWAY-SCOPE-RECEIVING"),
        Some("License Plate Putaway Scope Receiving"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let denied_receiving = wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        denied_facility,
        None,
        Some("LP-PUTAWAY-SCOPE-DENIED-RECEIVING"),
        Some("License Plate Putaway Scope Denied Receiving"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let allowed_destination_barcode = "LP-PUTAWAY-SCOPE-DESTINATION";
    let allowed_destination = fixture
        .location(tenant_id, allowed_facility, allowed_destination_barcode)
        .await;
    let denied_destination = fixture
        .location(
            tenant_id,
            denied_facility,
            "LP-PUTAWAY-SCOPE-DENIED-DESTINATION",
        )
        .await;
    let item_id = fixture
        .item(tenant_id, "License Plate Putaway Scope Item", "case")
        .await;
    let allowed_barcode = "LP-PUTAWAY-SCOPE-ALLOWED";
    let allowed_plate = receive_license_plate(
        &fixture,
        &administrator_access,
        administrator.id,
        allowed_facility,
        allowed_owner,
        allowed_receiving,
        item_id,
        allowed_barcode,
    )
    .await;
    let owner_denied_plate = receive_license_plate(
        &fixture,
        &administrator_access,
        administrator.id,
        allowed_facility,
        denied_owner,
        allowed_receiving,
        item_id,
        "LP-PUTAWAY-SCOPE-OWNER-DENIED",
    )
    .await;
    let facility_denied_plate = receive_license_plate(
        &fixture,
        &administrator_access,
        administrator.id,
        denied_facility,
        allowed_owner,
        denied_receiving,
        item_id,
        "LP-PUTAWAY-SCOPE-FACILITY-DENIED",
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
    let operator_token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let before_denials = effects(&fixture.db, tenant_id).await;

    for (key, license_plate_id, destination_location_id) in [
        (
            "license-plate-putaway-owner-denied",
            owner_denied_plate,
            allowed_destination,
        ),
        (
            "license-plate-putaway-facility-denied",
            facility_denied_plate,
            denied_destination,
        ),
    ] {
        assert_not_found(
            send(
                &app,
                &operator_token,
                tenant_id,
                "/api/v1/license-plate-putaway-tasks",
                key,
                json!({
                    "license_plate_id": license_plate_id,
                    "destination_location_id": destination_location_id,
                    "assigned_user_id": operator.id,
                }),
            )
            .await,
        )
        .await;
    }
    assert_eq!(effects(&fixture.db, tenant_id).await, before_denials);

    let created = send(
        &app,
        &operator_token,
        tenant_id,
        "/api/v1/license-plate-putaway-tasks",
        "license-plate-putaway-scope-create",
        json!({
            "license_plate_id": allowed_plate,
            "destination_location_id": allowed_destination,
            "assigned_user_id": operator.id,
        }),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    let created: Value = response_json(created).await;
    let task_id = created["task_id"]
        .as_i64()
        .expect("create response identifies the task");

    let started = send(
        &app,
        &operator_token,
        tenant_id,
        "/api/tasks/start",
        "license-plate-putaway-scope-start",
        json!({"task_id": task_id}),
    )
    .await;
    assert_eq!(started.status(), StatusCode::OK);
    assert!(response_json::<bool>(started).await);

    set_scope(
        &fixture.db,
        tenant_id,
        operator.id,
        vec![allowed_facility],
        Vec::new(),
    )
    .await;
    let confirmation_uri = format!("/api/v1/license-plate-putaway-tasks/{task_id}/confirmations");
    let before_denied_confirmation = effects(&fixture.db, tenant_id).await;
    let inventory_before_denied_confirmation = inventory_snapshot(
        &fixture.db,
        tenant_id,
        allowed_plate,
        allowed_receiving,
        allowed_destination,
    )
    .await;
    assert_not_found(
        send(
            &app,
            &operator_token,
            tenant_id,
            &confirmation_uri,
            "license-plate-putaway-scope-confirm",
            json!({
                "license_plate_barcode": allowed_barcode,
                "destination_location_barcode": allowed_destination_barcode,
            }),
        )
        .await,
    )
    .await;
    assert_eq!(
        effects(&fixture.db, tenant_id).await,
        before_denied_confirmation
    );
    assert_eq!(
        inventory_snapshot(
            &fixture.db,
            tenant_id,
            allowed_plate,
            allowed_receiving,
            allowed_destination,
        )
        .await,
        inventory_before_denied_confirmation
    );

    set_scope(
        &fixture.db,
        tenant_id,
        operator.id,
        vec![allowed_facility],
        vec![allowed_owner],
    )
    .await;
    let reclaimed = send(
        &app,
        &operator_token,
        tenant_id,
        "/api/tasks/start",
        "license-plate-putaway-scope-reclaim",
        json!({"task_id": task_id}),
    )
    .await;
    assert_eq!(reclaimed.status(), StatusCode::OK);
    assert!(response_json::<bool>(reclaimed).await);

    let confirmed = send(
        &app,
        &operator_token,
        tenant_id,
        &confirmation_uri,
        "license-plate-putaway-scope-confirm",
        json!({
            "license_plate_barcode": allowed_barcode,
            "destination_location_barcode": allowed_destination_barcode,
        }),
    )
    .await;
    assert_eq!(confirmed.status(), StatusCode::OK);
    let confirmed: Value = response_json(confirmed).await;
    assert_eq!(confirmed["task_id"].as_i64(), Some(task_id));
    let effects_after_confirmation = effects(&fixture.db, tenant_id).await;
    let inventory_after_confirmation = inventory_snapshot(
        &fixture.db,
        tenant_id,
        allowed_plate,
        allowed_receiving,
        allowed_destination,
    )
    .await;
    assert_eq!(
        inventory_after_confirmation,
        (Some(allowed_destination), 0, 1, 10)
    );

    set_scope(
        &fixture.db,
        tenant_id,
        operator.id,
        Vec::new(),
        vec![allowed_owner],
    )
    .await;
    assert_not_found(
        send(
            &app,
            &operator_token,
            tenant_id,
            &confirmation_uri,
            "license-plate-putaway-scope-confirm",
            json!({
                "license_plate_barcode": allowed_barcode,
                "destination_location_barcode": allowed_destination_barcode,
            }),
        )
        .await,
    )
    .await;
    assert_eq!(
        effects(&fixture.db, tenant_id).await,
        effects_after_confirmation
    );
    assert_eq!(
        inventory_snapshot(
            &fixture.db,
            tenant_id,
            allowed_plate,
            allowed_receiving,
            allowed_destination,
        )
        .await,
        inventory_after_confirmation
    );

    let unbound_counts: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT COUNT(*) FROM license_plate_putaway_tasks),
            (SELECT COUNT(*) FROM license_plate_putaway_task_contents),
            (SELECT COUNT(*) FROM license_plate_putaway_results)
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(unbound_counts, (0, 0, 0));

    let mut foreign_tx = tenant_tx(&fixture.db, foreign_tenant_id).await;
    let foreign_counts: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (
                SELECT COUNT(*)
                FROM license_plate_putaway_tasks
                WHERE task_id = $1
            ),
            (
                SELECT COUNT(*)
                FROM license_plate_putaway_task_contents
                WHERE task_id = $1
            ),
            (
                SELECT COUNT(*)
                FROM license_plate_putaway_results
                WHERE task_id = $1
            )
        "#,
    )
    .bind(task_id)
    .fetch_one(&mut *foreign_tx)
    .await
    .unwrap();
    foreign_tx.rollback().await.unwrap();
    assert_eq!(foreign_counts, (0, 0, 0));

    assert_exact_runtime_privileges(&fixture.db).await;
    let app_mutation =
        sqlx::query("UPDATE license_plate_putaway_results SET tenant_id = tenant_id")
            .execute(&fixture.db)
            .await
            .unwrap_err();
    let sqlx::Error::Database(app_mutation) = app_mutation else {
        panic!("result ACL rejection must be a database error");
    };
    assert_eq!(app_mutation.code().as_deref(), Some("42501"));

    let admin_db = admin_db_for(&fixture.db).await;
    let immutable = sqlx::query(
        r#"
        UPDATE license_plate_putaway_results
        SET tenant_id = tenant_id
        WHERE tenant_id = $1 AND task_id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .execute(&admin_db)
    .await
    .unwrap_err();
    let sqlx::Error::Database(immutable) = immutable else {
        panic!("result immutability rejection must be a database error");
    };
    assert_eq!(immutable.code().as_deref(), Some("55000"));
    assert_eq!(
        immutable.message(),
        "license plate putaway results are immutable"
    );
    admin_db.close().await;
}
