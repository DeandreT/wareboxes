mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::json;
use tokio::time::timeout;
use tower::ServiceExt;
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_server::auth::TENANT_ID_HEADER;
use wareboxes_server::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_server::{routes, state::AppState};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
struct Effects {
    transactions: i64,
    entries: i64,
    transitions: i64,
    command_records: i64,
    outbox_events: i64,
}

fn request(
    token: &str,
    tenant_id: TenantId,
    idempotency_key: &str,
    inventory_balance_id: i64,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/inventory/status-changes")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(IDEMPOTENCY_KEY_HEADER, idempotency_key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "inventory_balance_id": inventory_balance_id,
                "qty": 1,
                "to_status": "quarantine",
                "reason": "quality_inspection"
            })
            .to_string(),
        ))
        .unwrap()
}

async fn send(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    idempotency_key: &str,
    inventory_balance_id: i64,
) -> axum::response::Response {
    timeout(
        REQUEST_TIMEOUT,
        app.clone().oneshot(request(
            token,
            tenant_id,
            idempotency_key,
            inventory_balance_id,
        )),
    )
    .await
    .expect("status-change authorization request completes within the bound")
    .unwrap()
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

async fn effects(db: &db::Db, tenant_id: TenantId) -> Effects {
    let mut tx = tenant_tx(db, tenant_id).await;
    let effects = sqlx::query_as(
        r#"
        SELECT
            (
                SELECT COUNT(*)
                FROM inventory_transactions
                WHERE tenant_id = $1 AND transaction_type = 'status_change'
            ) AS transactions,
            (
                SELECT COUNT(*)
                FROM inventory_entries entry
                INNER JOIN inventory_transactions transaction
                    ON transaction.tenant_id = entry.tenant_id
                   AND transaction.inventory_owner_id = entry.inventory_owner_id
                   AND transaction.id = entry.transaction_id
                WHERE entry.tenant_id = $1
                  AND transaction.transaction_type = 'status_change'
            ) AS entries,
            (
                SELECT COUNT(*)
                FROM inventory_status_transitions
                WHERE tenant_id = $1
            ) AS transitions,
            (
                SELECT COUNT(*)
                FROM command_idempotency_records
                WHERE tenant_id = $1 AND operation = 'inventory.status_change.v1'
            ) AS command_records,
            (
                SELECT COUNT(*)
                FROM outbox_events
                WHERE tenant_id = $1
                  AND (
                      event_type = 'inventory.status.changed'
                      OR (
                          event_type = 'inventory.transaction.recorded'
                          AND payload->>'transaction_type' = 'status_change'
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

#[tokio::test]
async fn status_change_http_denials_and_revoked_replays_leave_no_effects() {
    let fixture = Fixture::new().await;
    let administrator = fixture.user("status-scope-admin@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, administrator.id).await;
    let operator = fixture.user("status-scope-operator@test.com").await;
    let unprivileged = fixture.user("status-scope-unprivileged@test.com").await;
    add_membership(&fixture.db, tenant_id, operator.id).await;
    add_membership(&fixture.db, tenant_id, unprivileged.id).await;

    let permission = repo::permissions::add_permission(&fixture.db, tenant_id, "wms", Some("WMS"))
        .await
        .unwrap();
    let role = repo::roles::add_role(
        &fixture.db,
        tenant_id,
        "status-change-operator",
        Some("Status change operator"),
    )
    .await
    .unwrap();
    assert!(
        repo::roles::add_role_permission(&fixture.db, tenant_id, role, permission)
            .await
            .unwrap()
    );
    assert!(
        repo::roles::add_role_to_user(&fixture.db, tenant_id, operator.id, role)
            .await
            .unwrap()
    );

    let allowed_facility = fixture
        .facility(tenant_id, "Status Scope Allowed Facility")
        .await;
    let denied_facility = fixture
        .facility(tenant_id, "Status Scope Denied Facility")
        .await;
    let allowed_owner = fixture
        .inventory_owner(tenant_id, "Status Scope Allowed Owner")
        .await;
    let denied_owner = fixture
        .inventory_owner(tenant_id, "Status Scope Denied Owner")
        .await;
    assert!(repo::inventory_owners::replace_inventory_owner_facilities(
        &fixture.db,
        tenant_id,
        allowed_owner,
        &[allowed_facility, denied_facility],
    )
    .await
    .unwrap());
    fixture
        .assign_owner_to_facility(tenant_id, denied_owner, allowed_facility)
        .await;

    let administrator_access = default_tenant_for_user(&fixture.db, administrator.id)
        .await
        .unwrap();
    let item_id = fixture.item(tenant_id, "Status Scope Item", "each").await;
    let allowed = fixture
        .received_balance(
            &administrator_access,
            ReceivedBalanceSetup {
                inventory_owner_id: allowed_owner,
                facility_id: allowed_facility,
                item_id,
                qty: 10,
                key: "STATUS-SCOPE-ALLOWED",
            },
        )
        .await;
    let owner_denied = fixture
        .received_balance(
            &administrator_access,
            ReceivedBalanceSetup {
                inventory_owner_id: denied_owner,
                facility_id: allowed_facility,
                item_id,
                qty: 10,
                key: "STATUS-SCOPE-OWNER-DENIED",
            },
        )
        .await;
    let facility_denied = fixture
        .received_balance(
            &administrator_access,
            ReceivedBalanceSetup {
                inventory_owner_id: allowed_owner,
                facility_id: denied_facility,
                item_id,
                qty: 10,
                key: "STATUS-SCOPE-FACILITY-DENIED",
            },
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

    let other_administrator = fixture.user("status-scope-other-tenant@test.com").await;
    let other_tenant_id = tenant_for_user(&fixture.db, other_administrator.id).await;
    let other_access = default_tenant_for_user(&fixture.db, other_administrator.id)
        .await
        .unwrap();
    let other_owner = fixture
        .inventory_owner(other_tenant_id, "Status Scope Other Owner")
        .await;
    let other_facility = fixture
        .facility(other_tenant_id, "Status Scope Other Facility")
        .await;
    fixture
        .assign_owner_to_facility(other_tenant_id, other_owner, other_facility)
        .await;
    let other_item = fixture
        .item(other_tenant_id, "Status Scope Other Item", "each")
        .await;
    let other_balance = fixture
        .received_balance(
            &other_access,
            ReceivedBalanceSetup {
                inventory_owner_id: other_owner,
                facility_id: other_facility,
                item_id: other_item,
                qty: 10,
                key: "STATUS-SCOPE-OTHER",
            },
        )
        .await;

    let operator_token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let unprivileged_token = auth::create_session(&fixture.db, unprivileged.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let tenant_before = effects(&fixture.db, tenant_id).await;
    let other_before = effects(&fixture.db, other_tenant_id).await;

    for (key, balance_id) in [
        ("status-scope-owner-denied", owner_denied.balance_id),
        ("status-scope-facility-denied", facility_denied.balance_id),
    ] {
        let response = send(&app, &operator_token, tenant_id, key, balance_id).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let cross_tenant = send(
        &app,
        &operator_token,
        other_tenant_id,
        "status-scope-cross-tenant",
        other_balance.balance_id,
    )
    .await;
    assert_eq!(cross_tenant.status(), StatusCode::FORBIDDEN);
    let permission_denied = send(
        &app,
        &unprivileged_token,
        tenant_id,
        "status-scope-permission-denied",
        allowed.balance_id,
    )
    .await;
    assert_eq!(permission_denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(effects(&fixture.db, tenant_id).await, tenant_before);
    assert_eq!(effects(&fixture.db, other_tenant_id).await, other_before);

    let replay_key = "status-scope-revoked-replay";
    let allowed_response = send(
        &app,
        &operator_token,
        tenant_id,
        replay_key,
        allowed.balance_id,
    )
    .await;
    assert_eq!(allowed_response.status(), StatusCode::OK);
    let effects_after_success = effects(&fixture.db, tenant_id).await;
    assert_eq!(
        effects_after_success.transactions,
        tenant_before.transactions + 1
    );
    assert_eq!(effects_after_success.entries, tenant_before.entries + 2);
    assert_eq!(
        effects_after_success.transitions,
        tenant_before.transitions + 1
    );
    assert_eq!(
        effects_after_success.command_records,
        tenant_before.command_records + 1
    );
    assert_eq!(
        effects_after_success.outbox_events,
        tenant_before.outbox_events + 2
    );

    set_scope(
        &fixture.db,
        tenant_id,
        operator.id,
        vec![allowed_facility],
        Vec::new(),
    )
    .await;
    let owner_revoked_replay = send(
        &app,
        &operator_token,
        tenant_id,
        replay_key,
        allowed.balance_id,
    )
    .await;
    assert_eq!(owner_revoked_replay.status(), StatusCode::FORBIDDEN);
    assert_eq!(effects(&fixture.db, tenant_id).await, effects_after_success);

    set_scope(
        &fixture.db,
        tenant_id,
        operator.id,
        Vec::new(),
        vec![allowed_owner],
    )
    .await;
    let facility_revoked_replay = send(
        &app,
        &operator_token,
        tenant_id,
        replay_key,
        allowed.balance_id,
    )
    .await;
    assert_eq!(facility_revoked_replay.status(), StatusCode::FORBIDDEN);
    assert_eq!(effects(&fixture.db, tenant_id).await, effects_after_success);
    assert_eq!(effects(&fixture.db, other_tenant_id).await, other_before);
}
