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
use wareboxes_core::dto::{ChangeInventoryStatusResult, ErrorCode, ErrorResponse};
use wareboxes_core::models::{
    InventoryHoldReason, InventoryStatus, InventoryStatusChangeReason, InventoryTransactionType,
    TenantAccess,
};
use wareboxes_domain::CommandContext;

const OPERATION_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
struct StatusChangeEffects {
    transactions: i64,
    entries: i64,
    transitions: i64,
    outbox_events: i64,
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct TransitionAudit {
    transaction_id: i64,
    source_balance_id: i64,
    destination_balance_id: i64,
    from_status: String,
    to_status: String,
    qty: i64,
    reason_code: String,
    reason_note: Option<String>,
    reference_type: Option<String>,
    reference_id: Option<i64>,
    created_by: i64,
}

fn command_context(access: &TenantAccess, key: &str) -> CommandContext {
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
    idempotency_key: Option<&str>,
    body: Value,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri("/api/inventory/status-changes")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(idempotency_key) = idempotency_key {
        request = request.header(IDEMPOTENCY_KEY_HEADER, idempotency_key);
    }
    request.body(Body::from(body.to_string())).unwrap()
}

async fn send(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    idempotency_key: Option<&str>,
    body: Value,
) -> axum::response::Response {
    timeout(
        OPERATION_TIMEOUT,
        app.clone()
            .oneshot(request(token, tenant_id, idempotency_key, body)),
    )
    .await
    .expect("status-change HTTP request completes within the bound")
    .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn status_change_effects(db: &db::Db, tenant_id: TenantId) -> StatusChangeEffects {
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
                   AND transaction.inventory_owner_id =
                       entry.inventory_owner_id
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
                FROM outbox_events
                WHERE tenant_id = $1
                  AND (
                      (
                          event_type = 'inventory.transaction.recorded'
                          AND payload->>'transaction_type' = 'status_change'
                      )
                      OR event_type = 'inventory.status.changed'
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

async fn balance_quantities(
    db: &db::Db,
    tenant_id: TenantId,
    balance_id: i64,
) -> (String, i64, i64, i64) {
    let mut tx = tenant_tx(db, tenant_id).await;
    let quantities = sqlx::query_as(
        r#"
        SELECT status, qty_on_hand, qty_reserved, qty_held
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
    quantities
}

async fn transition_audit(
    db: &db::Db,
    tenant_id: TenantId,
    transaction_id: i64,
) -> TransitionAudit {
    let mut tx = tenant_tx(db, tenant_id).await;
    let audit = sqlx::query_as(
        r#"
        SELECT transaction_id, source_balance_id, destination_balance_id,
               from_status, to_status, qty, reason_code, reason_note,
               reference_type, reference_id, created_by
        FROM inventory_status_transitions
        WHERE tenant_id = $1 AND transaction_id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(transaction_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    audit
}

#[tokio::test]
async fn status_changes_round_trip_replay_and_preserve_committed_inventory() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("inventory-status-change@test.local").await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let tenant_id = access.tenant_id;
    let owner_id = fixture
        .inventory_owner(tenant_id, "Status Change Owner")
        .await;
    let facility_id = fixture.facility(tenant_id, "Status Change DC").await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;
    let item_id = fixture.item(tenant_id, "Status Change Item", "each").await;
    let received = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id,
                qty: 20,
                key: "STATUS-CHANGE",
            },
        )
        .await;

    let order_id = fixture
        .order(tenant_id, "STATUS-CHANGE-ORDER", owner_id)
        .await;
    fixture
        .allocated_reservation(
            tenant_id,
            user.id,
            order_id,
            received.balance_id,
            4,
            "status-change-allocation",
        )
        .await;
    repo::inventory::place_inventory_hold(
        &fixture.db,
        &access,
        &command_context(&access, "status-change-hold"),
        &repo::inventory::PlaceInventoryHoldCommand {
            inventory_balance_id: received.balance_id,
            qty: 3,
            reason: InventoryHoldReason::QualityInspection,
            note: Some("committed hold remains on available stock"),
            reference_type: None,
            reference_id: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        balance_quantities(&fixture.db, tenant_id, received.balance_id).await,
        ("available".into(), 20, 4, 3)
    );

    let token = auth::create_session(&fixture.db, user.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let missing_header_before = status_change_effects(&fixture.db, tenant_id).await;
    let missing_header = send(
        &app,
        &token,
        tenant_id,
        None,
        json!({
            "inventory_balance_id": received.balance_id,
            "qty": 1,
            "to_status": "quarantine",
            "reason": "quality_inspection"
        }),
    )
    .await;
    assert_eq!(missing_header.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json::<ErrorResponse>(missing_header).await.code,
        ErrorCode::IdempotencyKeyRequired
    );
    assert_eq!(
        status_change_effects(&fixture.db, tenant_id).await,
        missing_header_before
    );

    let over_capacity_before = status_change_effects(&fixture.db, tenant_id).await;
    let over_capacity = timeout(
        OPERATION_TIMEOUT,
        repo::inventory::change_inventory_status(
            &fixture.db,
            &access,
            &command_context(&access, "status-change-over-capacity"),
            &repo::inventory::ChangeInventoryStatusCommand {
                inventory_balance_id: received.balance_id,
                qty: 14,
                to_status: InventoryStatus::Quarantine,
                reason: InventoryStatusChangeReason::QualityInspection,
                note: None,
                reference_type: None,
                reference_id: None,
            },
        ),
    )
    .await
    .expect("over-capacity status change completes within the bound")
    .unwrap_err();
    assert!(matches!(
        over_capacity,
        AppError::Core(CoreError::Conflict(_))
    ));
    assert_eq!(
        status_change_effects(&fixture.db, tenant_id).await,
        over_capacity_before
    );
    assert_eq!(
        balance_quantities(&fixture.db, tenant_id, received.balance_id).await,
        ("available".into(), 20, 4, 3)
    );

    let first_context = command_context(&access, "status-change-first");
    let first_command = repo::inventory::ChangeInventoryStatusCommand {
        inventory_balance_id: received.balance_id,
        qty: 5,
        to_status: InventoryStatus::Quarantine,
        reason: InventoryStatusChangeReason::QualityInspection,
        note: Some("awaiting quality inspection"),
        reference_type: Some("inspection"),
        reference_id: Some(9001),
    };
    let first = timeout(
        OPERATION_TIMEOUT,
        repo::inventory::change_inventory_status(
            &fixture.db,
            &access,
            &first_context,
            &first_command,
        ),
    )
    .await
    .expect("first status change completes within the bound")
    .unwrap();
    assert_eq!(first.source_inventory_balance_id, received.balance_id);
    assert_eq!(first.qty, 5);
    assert_eq!(first.from_status, InventoryStatus::Available);
    assert_eq!(first.to_status, InventoryStatus::Quarantine);
    assert_eq!(
        balance_quantities(&fixture.db, tenant_id, received.balance_id).await,
        ("available".into(), 15, 4, 3)
    );
    assert_eq!(
        balance_quantities(&fixture.db, tenant_id, first.target_inventory_balance_id).await,
        ("quarantine".into(), 5, 0, 0)
    );

    let effects_after_first = status_change_effects(&fixture.db, tenant_id).await;
    assert_eq!(
        timeout(
            OPERATION_TIMEOUT,
            repo::inventory::change_inventory_status(
                &fixture.db,
                &access,
                &first_context,
                &first_command,
            ),
        )
        .await
        .expect("exact replay completes within the bound")
        .unwrap(),
        first
    );
    assert_eq!(
        status_change_effects(&fixture.db, tenant_id).await,
        effects_after_first
    );
    let changed_request = timeout(
        OPERATION_TIMEOUT,
        repo::inventory::change_inventory_status(
            &fixture.db,
            &access,
            &first_context,
            &repo::inventory::ChangeInventoryStatusCommand {
                qty: 6,
                ..first_command
            },
        ),
    )
    .await
    .expect("changed replay completes within the bound")
    .unwrap_err();
    assert!(matches!(
        changed_request,
        AppError::Core(CoreError::IdempotencyKeyReused)
    ));
    assert_eq!(
        status_change_effects(&fixture.db, tenant_id).await,
        effects_after_first
    );

    let transactions = repo::inventory::get_transactions(&fixture.db, tenant_id)
        .await
        .unwrap();
    let journal = transactions
        .iter()
        .find(|transaction| transaction.id == first.inventory_transaction_id)
        .unwrap();
    assert_eq!(
        journal.transaction_type,
        InventoryTransactionType::StatusChange
    );
    assert_eq!(journal.reason.as_deref(), Some("quality_inspection"));
    assert_eq!(journal.reference_type.as_deref(), Some("inspection"));
    assert_eq!(journal.reference_id, Some(9001));
    assert_eq!(journal.operation, "inventory.status_change.v1");
    assert_eq!(journal.entries.len(), 2);
    assert_eq!(
        journal
            .entries
            .iter()
            .map(|entry| (entry.status, entry.quantity_delta))
            .collect::<Vec<_>>(),
        vec![
            (InventoryStatus::Available, -5),
            (InventoryStatus::Quarantine, 5),
        ]
    );
    assert_eq!(
        transition_audit(&fixture.db, tenant_id, first.inventory_transaction_id).await,
        TransitionAudit {
            transaction_id: first.inventory_transaction_id,
            source_balance_id: received.balance_id,
            destination_balance_id: first.target_inventory_balance_id,
            from_status: "available".into(),
            to_status: "quarantine".into(),
            qty: 5,
            reason_code: "quality_inspection".into(),
            reason_note: Some("awaiting quality inspection".into()),
            reference_type: Some("inspection".into()),
            reference_id: Some(9001),
            created_by: user.id,
        }
    );

    let first_ordering_key = format!("inventory-transaction:{}", first.inventory_transaction_id);
    let first_events =
        wareboxes_persistence_postgres::outbox::get_events(&fixture.db, tenant_id, None, 100)
            .await
            .unwrap()
            .into_iter()
            .filter(|event| event.ordering_key == first_ordering_key)
            .collect::<Vec<_>>();
    assert_eq!(first_events.len(), 2);
    assert_eq!(
        first_events
            .iter()
            .map(|event| (event.aggregate_sequence, event.event_type.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (1, "inventory.transaction.recorded"),
            (2, "inventory.status.changed"),
        ]
    );
    assert_eq!(
        first_events[1].payload["inventory_transaction_id"],
        first.inventory_transaction_id
    );
    assert_eq!(
        first_events[1].payload["source_inventory_balance_id"],
        received.balance_id
    );
    assert_eq!(
        first_events[1].payload["target_inventory_balance_id"],
        first.target_inventory_balance_id
    );
    assert_eq!(first_events[1].payload["quantity"], 5);
    assert_eq!(first_events[1].payload["from_status"], "available");
    assert_eq!(first_events[1].payload["to_status"], "quarantine");

    let second = timeout(
        OPERATION_TIMEOUT,
        repo::inventory::change_inventory_status(
            &fixture.db,
            &access,
            &command_context(&access, "status-change-second"),
            &repo::inventory::ChangeInventoryStatusCommand {
                inventory_balance_id: received.balance_id,
                qty: 3,
                to_status: InventoryStatus::Quarantine,
                reason: InventoryStatusChangeReason::DamageSuspected,
                note: Some("additional units require inspection"),
                reference_type: None,
                reference_id: None,
            },
        ),
    )
    .await
    .expect("target-merge status change completes within the bound")
    .unwrap();
    assert_eq!(
        second.target_inventory_balance_id,
        first.target_inventory_balance_id
    );
    assert_eq!(
        balance_quantities(&fixture.db, tenant_id, received.balance_id).await,
        ("available".into(), 12, 4, 3)
    );
    assert_eq!(
        balance_quantities(&fixture.db, tenant_id, first.target_inventory_balance_id).await,
        ("quarantine".into(), 8, 0, 0)
    );

    let released_response = send(
        &app,
        &token,
        tenant_id,
        Some("status-change-release"),
        json!({
            "inventory_balance_id": first.target_inventory_balance_id,
            "qty": 8,
            "to_status": "available",
            "reason": "inspection_passed",
            "note": "inspection completed",
            "reference_type": "inspection",
            "reference_id": 9002
        }),
    )
    .await;
    assert_eq!(released_response.status(), StatusCode::OK);
    let released: ChangeInventoryStatusResult = response_json(released_response).await;
    assert_eq!(
        released.source_inventory_balance_id,
        first.target_inventory_balance_id
    );
    assert_eq!(released.target_inventory_balance_id, received.balance_id);
    assert_eq!(released.qty, 8);
    assert_eq!(released.from_status, InventoryStatus::Quarantine);
    assert_eq!(released.to_status, InventoryStatus::Available);
    assert_eq!(
        balance_quantities(&fixture.db, tenant_id, received.balance_id).await,
        ("available".into(), 20, 4, 3)
    );
    assert_eq!(
        balance_quantities(&fixture.db, tenant_id, first.target_inventory_balance_id).await,
        ("quarantine".into(), 0, 0, 0)
    );

    let final_effects = status_change_effects(&fixture.db, tenant_id).await;
    assert_eq!(final_effects.transactions, 3);
    assert_eq!(final_effects.entries, 6);
    assert_eq!(final_effects.transitions, 3);
    assert_eq!(final_effects.outbox_events, 6);
    assert!(
        repo::inventory::get_reconciliation_issues(&fixture.db, tenant_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        repo::inventory::get_inventory_hold_reconciliation_issues_in_scope(&fixture.db, &access)
            .await
            .unwrap()
            .is_empty()
    );
}
