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
    CreatePutawayTaskResponse, ErrorReason, ErrorResponse, PutawayConfirmationResponse,
};
use wareboxes_application::CommandContext;
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_core::models::InboundReceiptExceptionReason;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
struct PutawayEffects {
    tasks: i64,
    details: i64,
    results: i64,
    transactions: i64,
    entries: i64,
    command_records: i64,
    outbox_events: i64,
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
    .expect("putaway authorization request completes within the bound")
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
async fn receive_expected_inventory(
    fixture: &Fixture,
    access: &wareboxes_core::models::TenantAccess,
    actor_user_id: i64,
    facility_id: i64,
    inventory_owner_id: i64,
    receiving_location_id: i64,
    item_id: i64,
    key: &str,
) -> i64 {
    let load_id = repo::loads::add_load(
        &fixture.db,
        access.tenant_id,
        actor_user_id,
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
        actor_user_id,
        load_id,
        item_id,
        None,
        10,
        Some(key),
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
            lot: Some(key),
            serial: None,
            expiration: None,
            exception_reason: None::<InboundReceiptExceptionReason>,
            exception_note: None,
        },
    )
    .await
    .unwrap()
    .inventory_balance_id
    .expect("a physical expected receipt identifies its balance")
}

async fn effects(db: &db::Db, tenant_id: TenantId) -> PutawayEffects {
    let mut tx = tenant_tx(db, tenant_id).await;
    let effects = sqlx::query_as(
        r#"
        SELECT
            (
                SELECT COUNT(*)
                FROM work_tasks
                WHERE tenant_id = $1 AND task_type = 'putaway'
            ) AS tasks,
            (
                SELECT COUNT(*)
                FROM putaway_tasks
                WHERE tenant_id = $1
            ) AS details,
            (
                SELECT COUNT(*)
                FROM putaway_results
                WHERE tenant_id = $1
            ) AS results,
            (
                SELECT COUNT(*)
                FROM inventory_transactions
                WHERE tenant_id = $1
                  AND operation = 'task.confirm_putaway.v2'
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
                      'task.confirm_putaway.v2'
            ) AS entries,
            (
                SELECT COUNT(*)
                FROM command_idempotency_records
                WHERE tenant_id = $1
                  AND operation IN (
                      'task.create_putaway.v2',
                      'task.confirm_putaway.v2'
                  )
            ) AS command_records,
            (
                SELECT COUNT(*)
                FROM outbox_events
                WHERE tenant_id = $1
                  AND (
                      event_type = 'inventory.putaway.confirmed'
                      OR (
                          event_type =
                              'inventory.transaction.recorded'
                          AND payload->>'operation' =
                              'task.confirm_putaway.v2'
                      )
                  )
            ) AS outbox_events
        "#,
    )
    .bind(tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    effects
}

async fn inventory_quantities(
    db: &db::Db,
    tenant_id: TenantId,
    source_inventory_balance_id: i64,
    destination_location_id: i64,
) -> (i64, i64, i64) {
    let mut tx = tenant_tx(db, tenant_id).await;
    let quantities = sqlx::query_as(
        r#"
        SELECT
            (
                SELECT qty_on_hand
                FROM inventory_balances
                WHERE tenant_id = $1 AND id = $2
            ),
            (
                SELECT COALESCE(SUM(qty_on_hand), 0)::BIGINT
                FROM inventory_balances
                WHERE tenant_id = $1
                  AND location_id = $3
                  AND deleted IS NULL
            ),
            (SELECT COUNT(*) FROM inventory_reconciliation)
        "#,
    )
    .bind(tenant_id.get())
    .bind(source_inventory_balance_id)
    .bind(destination_location_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    quantities
}

#[tokio::test]
async fn putaway_scope_denials_replays_and_rls_leave_no_hidden_effects() {
    let fixture = Fixture::new().await;
    let administrator = fixture.wms_user("putaway-scope-admin@test.local").await;
    let administrator_access = default_tenant_for_user(&fixture.db, administrator.id)
        .await
        .unwrap();
    let tenant_id = administrator_access.tenant_id;

    let operator = fixture.user("putaway-scope-operator@test.local").await;
    let foreign_tenant_id = tenant_for_user(&fixture.db, operator.id).await;
    add_membership(&fixture.db, tenant_id, operator.id).await;
    let wms_permission =
        wareboxes_persistence_postgres::permissions::find_by_name(&fixture.db, tenant_id, "wms")
            .await
            .unwrap()
            .unwrap();
    let operator_role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        "putaway-scope-operator",
        Some("Restricted putaway operator"),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        operator_role,
        wms_permission.id,
    )
    .await
    .unwrap());
    assert!(wareboxes_persistence_postgres::roles::add_role_to_user(
        &fixture.db,
        tenant_id,
        operator.id,
        operator_role,
    )
    .await
    .unwrap());

    let allowed_facility = fixture
        .facility(tenant_id, "Putaway Scope Allowed Facility")
        .await;
    let denied_facility = fixture
        .facility(tenant_id, "Putaway Scope Denied Facility")
        .await;
    let allowed_owner = fixture
        .inventory_owner(tenant_id, "Putaway Scope Allowed Owner")
        .await;
    let denied_owner = fixture
        .inventory_owner(tenant_id, "Putaway Scope Denied Owner")
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
        Some("PUTAWAY-SCOPE-RECEIVING"),
        Some("Putaway Scope Receiving"),
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
        Some("PUTAWAY-SCOPE-DENIED-RECEIVING"),
        Some("Putaway Scope Denied Receiving"),
        "dock",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let allowed_destination = fixture
        .location(tenant_id, allowed_facility, "PUTAWAY-SCOPE-DESTINATION")
        .await;
    let denied_destination = fixture
        .location(
            tenant_id,
            denied_facility,
            "PUTAWAY-SCOPE-DENIED-DESTINATION",
        )
        .await;
    let item_id = fixture.item(tenant_id, "Putaway Scope Item", "case").await;
    let allowed_source = receive_expected_inventory(
        &fixture,
        &administrator_access,
        administrator.id,
        allowed_facility,
        allowed_owner,
        allowed_receiving,
        item_id,
        "PUTAWAY-SCOPE-ALLOWED",
    )
    .await;
    let owner_denied_source = receive_expected_inventory(
        &fixture,
        &administrator_access,
        administrator.id,
        allowed_facility,
        denied_owner,
        allowed_receiving,
        item_id,
        "PUTAWAY-SCOPE-OWNER-DENIED",
    )
    .await;
    let facility_denied_source = receive_expected_inventory(
        &fixture,
        &administrator_access,
        administrator.id,
        denied_facility,
        allowed_owner,
        denied_receiving,
        item_id,
        "PUTAWAY-SCOPE-FACILITY-DENIED",
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

    for (key, source_id, destination_id) in [
        (
            "putaway-owner-denied",
            owner_denied_source,
            allowed_destination,
        ),
        (
            "putaway-facility-denied",
            facility_denied_source,
            denied_destination,
        ),
    ] {
        let response = send(
            &app,
            &operator_token,
            tenant_id,
            "/api/v1/putaway-tasks",
            key,
            json!({
                "source_inventory_balance_id": source_id,
                "destination_location_id": destination_id,
                "quantity": 2,
                "assigned_user_id": operator.id,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response_json::<ErrorResponse>(response).await.reason,
            ErrorReason::NotFound
        );
    }
    assert_eq!(effects(&fixture.db, tenant_id).await, before_denials);

    let create = send(
        &app,
        &operator_token,
        tenant_id,
        "/api/v1/putaway-tasks",
        "putaway-scope-create",
        json!({
            "source_inventory_balance_id": allowed_source,
            "destination_location_id": allowed_destination,
            "quantity": 4,
            "assigned_user_id": operator.id,
        }),
    )
    .await;
    assert_eq!(create.status(), StatusCode::OK);
    let task_id = response_json::<CreatePutawayTaskResponse>(create)
        .await
        .task_id;
    let start = send(
        &app,
        &operator_token,
        tenant_id,
        "/api/tasks/start",
        "putaway-scope-start",
        json!({"task_id": task_id}),
    )
    .await;
    assert_eq!(start.status(), StatusCode::OK);
    assert!(response_json::<bool>(start).await);

    set_scope(
        &fixture.db,
        tenant_id,
        operator.id,
        vec![allowed_facility],
        Vec::new(),
    )
    .await;
    let before_denied_confirmation = effects(&fixture.db, tenant_id).await;
    let quantities_before =
        inventory_quantities(&fixture.db, tenant_id, allowed_source, allowed_destination).await;
    let confirmation_uri = format!("/api/v1/putaway-tasks/{task_id}/confirmations");
    let denied_confirmation = send(
        &app,
        &operator_token,
        tenant_id,
        &confirmation_uri,
        "putaway-scope-confirm",
        json!({"destination_location_barcode": "PUTAWAY-SCOPE-DESTINATION"}),
    )
    .await;
    assert_eq!(denied_confirmation.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response_json::<ErrorResponse>(denied_confirmation)
            .await
            .reason,
        ErrorReason::NotFound
    );
    assert_eq!(
        effects(&fixture.db, tenant_id).await,
        before_denied_confirmation
    );
    assert_eq!(
        inventory_quantities(&fixture.db, tenant_id, allowed_source, allowed_destination,).await,
        quantities_before
    );

    set_scope(
        &fixture.db,
        tenant_id,
        operator.id,
        vec![allowed_facility],
        vec![allowed_owner],
    )
    .await;
    let restart = send(
        &app,
        &operator_token,
        tenant_id,
        "/api/tasks/start",
        "putaway-scope-restart",
        json!({"task_id": task_id}),
    )
    .await;
    assert_eq!(restart.status(), StatusCode::OK);
    assert!(response_json::<bool>(restart).await);
    let confirmation = send(
        &app,
        &operator_token,
        tenant_id,
        &confirmation_uri,
        "putaway-scope-confirm",
        json!({"destination_location_barcode": "PUTAWAY-SCOPE-DESTINATION"}),
    )
    .await;
    assert_eq!(confirmation.status(), StatusCode::OK);
    let confirmation = response_json::<PutawayConfirmationResponse>(confirmation).await;
    assert_eq!(confirmation.task_id, task_id);
    let effects_after_confirmation = effects(&fixture.db, tenant_id).await;

    set_scope(
        &fixture.db,
        tenant_id,
        operator.id,
        Vec::new(),
        vec![allowed_owner],
    )
    .await;
    let concealed_replay = send(
        &app,
        &operator_token,
        tenant_id,
        &confirmation_uri,
        "putaway-scope-confirm",
        json!({"destination_location_barcode": "PUTAWAY-SCOPE-DESTINATION"}),
    )
    .await;
    assert_eq!(concealed_replay.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response_json::<ErrorResponse>(concealed_replay)
            .await
            .reason,
        ErrorReason::NotFound
    );
    assert_eq!(
        effects(&fixture.db, tenant_id).await,
        effects_after_confirmation
    );

    let unbound_counts: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT COUNT(*) FROM putaway_tasks),
            (SELECT COUNT(*) FROM putaway_results)
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(unbound_counts, (0, 0));

    let mut foreign_tx = tenant_tx(&fixture.db, foreign_tenant_id).await;
    let foreign_counts: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (
                SELECT COUNT(*)
                FROM putaway_tasks
                WHERE task_id = $1
            ),
            (
                SELECT COUNT(*)
                FROM putaway_results
                WHERE task_id = $1
            )
        "#,
    )
    .bind(task_id)
    .fetch_one(&mut *foreign_tx)
    .await
    .unwrap();
    foreign_tx.rollback().await.unwrap();
    assert_eq!(foreign_counts, (0, 0));

    let result_privileges: (bool, bool, bool, bool, bool, bool, bool) = sqlx::query_as(
        r#"
            SELECT
                has_table_privilege(
                    current_user,
                    'public.putaway_results',
                    'SELECT'
                ),
                has_table_privilege(
                    current_user,
                    'public.putaway_results',
                    'INSERT'
                ),
                has_table_privilege(
                    current_user,
                    'public.putaway_results',
                    'UPDATE'
                ),
                has_table_privilege(
                    current_user,
                    'public.putaway_results',
                    'DELETE'
                ),
                has_table_privilege(
                    current_user,
                    'public.putaway_results',
                    'TRUNCATE'
                ),
                has_table_privilege(
                    current_user,
                    'public.putaway_results',
                    'REFERENCES'
                ),
                has_table_privilege(
                    current_user,
                    'public.putaway_results',
                    'TRIGGER'
                )
            "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(
        result_privileges,
        (true, true, false, false, false, false, false)
    );

    let app_mutation = sqlx::query("UPDATE putaway_results SET quantity = quantity + 1")
        .execute(&fixture.db)
        .await
        .unwrap_err();
    let sqlx::Error::Database(app_mutation) = app_mutation else {
        panic!("putaway result ACL rejection must be a database error");
    };
    assert_eq!(app_mutation.code().as_deref(), Some("42501"));

    let admin_db = admin_db_for(&fixture.db).await;
    let immutable = sqlx::query(
        r#"
            UPDATE putaway_results
            SET quantity = quantity + 1
            WHERE tenant_id = $1 AND task_id = $2
            "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .execute(&admin_db)
    .await
    .unwrap_err();
    let sqlx::Error::Database(immutable) = immutable else {
        panic!("putaway result immutability rejection must be a database error");
    };
    assert_eq!(immutable.code().as_deref(), Some("55000"));
    assert_eq!(immutable.message(), "putaway results are immutable");
    admin_db.close().await;
}
