mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sqlx::Row;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::repo::inventory::CreateInventoryReservationCommand;
use wareboxes_api::request_context::{IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use wareboxes_api::{auth, routes, state::AppState};
use wareboxes_api_contract::v1::{
    CancelCrossDockWorkResponse, ConfirmCrossDockWorkResponse, CrossDockClaimResponse,
    CrossDockPlanningOptionPage, CrossDockWorkPage, CrossDockWorkStatus,
    OrderAllocationReadinessBlocker, OrderAllocationReadinessResponse,
    OrderAllocationReadinessStatus, PlanCrossDockWorkResponse,
};
use wareboxes_application::inbound_load::StartInboundLoadUnloadingCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_core::models::{
    InboundReceiptExceptionReason, LoadStatus, LoadType, ReceiveExpectedInventoryResult,
    TenantAccess,
};
use wareboxes_domain::{InboundLoadId, InboundLoadScanValue};

#[tokio::test]
async fn expected_receipt_cross_docks_into_reserved_pick_face_atomically() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("cross-dock@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .expect("operator tenant access");
    grant_permission(
        &fixture.db,
        &access,
        "cross-dock-supervisor",
        "wms_supervisor",
    )
    .await;
    grant_permission(&fixture.db, &access, "cross-dock-orders", "orders").await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Cross dock owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Cross dock facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let item_id = fixture
        .item(access.tenant_id, "Cross dock item", "case")
        .await;
    wareboxes_api::repo::items::add_barcode(
        &fixture.db,
        access.tenant_id,
        item_id,
        "XD-ITEM-01",
        "code128",
        None,
    )
    .await
    .unwrap();
    let receiving_location_id = wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        access.tenant_id,
        facility_id,
        None,
        Some("XD-RECV-01"),
        Some("Cross-dock receiving"),
        "receiving",
        true,
        false,
        true,
    )
    .await
    .unwrap();
    let destination_location_id = fixture
        .location(access.tenant_id, facility_id, "XD-PICK-01")
        .await;
    let receipt = receive_expected(
        &fixture,
        &access,
        owner_id,
        facility_id,
        receiving_location_id,
        item_id,
        5,
    )
    .await;
    let receipt_transaction_id = receipt
        .inventory_transaction_id
        .expect("physical receipt transaction");
    let order_id = fixture
        .order_header(access.tenant_id, "XD-ORDER-01", owner_id)
        .await;
    let order_item_id = fixture
        .order_item(access.tenant_id, order_id, item_id, 5)
        .await;
    let _reservation = wareboxes_api::repo::inventory::create_inventory_reservation(
        &fixture.db,
        &access,
        &CreateInventoryReservationCommand {
            order_id,
            order_item_id,
            facility_id,
            qty: 5,
            idempotency_key: "cross-dock-reservation",
        },
    )
    .await
    .unwrap();
    let mut revision_tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let revision: i64 =
        sqlx::query_scalar("SELECT revision FROM orders WHERE tenant_id=$1 AND id=$2")
            .bind(access.tenant_id.get())
            .bind(order_id)
            .fetch_one(&mut *revision_tx)
            .await
            .unwrap();
    revision_tx.rollback().await.unwrap();
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let options: CrossDockPlanningOptionPage = json_response(
        expect_status(
            send(
                &app,
                &token,
                &access,
                Method::GET,
                &format!("/api/v1/cross-dock-planning-options?facility_id={facility_id}"),
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "cross-dock planning options",
        )
        .await,
    )
    .await;
    assert_eq!(options.items.len(), 1);
    let option = &options.items[0];
    assert_eq!(option.order_id, order_id);
    assert_eq!(option.order_line_id, order_item_id);
    assert_eq!(
        option.source_receipt_inventory_transaction_id,
        receipt_transaction_id
    );
    assert_eq!(option.maximum_plan_quantity, 5);
    assert_eq!(option.source_receiving_location.barcode, "XD-RECV-01");
    assert!(option
        .destination_pick_faces
        .iter()
        .any(|location| location.location_id == destination_location_id));
    let plan_body = json!({
      "order_line_id":order_item_id,"expected_order_revision":revision,
      "source_receipt_inventory_transaction_id":receipt_transaction_id,
      "destination_pick_face_location_id":destination_location_id,"quantity":5,"priority":25,
      "instructions":"Move directly from receiving to the forward pick face"
    });
    let plan: PlanCrossDockWorkResponse = json_response(
        expect_status(
            send(
                &app,
                &token,
                &access,
                Method::POST,
                &format!("/api/v1/orders/{order_id}/cross-dock-tasks"),
                Some("cross-dock-plan"),
                Some(plan_body.clone()),
            )
            .await,
            StatusCode::OK,
            "plan cross-dock",
        )
        .await,
    )
    .await;
    assert_eq!(plan.quantity, 5);
    assert_eq!(plan.order_revision.get(), revision + 1);
    assert_eq!(plan.status, CrossDockWorkStatus::Pending);
    let replay: PlanCrossDockWorkResponse = json_response(
        expect_status(
            send(
                &app,
                &token,
                &access,
                Method::POST,
                &format!("/api/v1/orders/{order_id}/cross-dock-tasks"),
                Some("cross-dock-plan"),
                Some(plan_body.clone()),
            )
            .await,
            StatusCode::OK,
            "replay cross-dock plan",
        )
        .await,
    )
    .await;
    assert_eq!(replay, plan);
    let readiness: OrderAllocationReadinessResponse = json_response(
        expect_status(
            send(
                &app,
                &token,
                &access,
                Method::GET,
                &format!(
                    "/api/v1/orders/{order_id}/allocation-readiness?facility_id={facility_id}"
                ),
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "cross-dock allocation readiness",
        )
        .await,
    )
    .await;
    assert_eq!(readiness.status, OrderAllocationReadinessStatus::Blocked);
    assert!(readiness
        .blocking_reasons
        .contains(&OrderAllocationReadinessBlocker::CrossDockInProgress));
    assert_eq!(
        send(
            &app,
            &token,
            &access,
            Method::POST,
            &format!("/api/v1/orders/{order_id}/allocation-runs"),
            Some("cross-dock-allocation-blocked"),
            Some(json!({
                "facility_id": facility_id,
                "expected_revision": plan.order_revision,
                "expected_policy": {"source": "product_default", "policy_hash": "6090a99a06ea2e049d7321d5cf2b8f462c6d6e6e2ca527ae87657a7a5fd9d156"}
            })),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(
            &app,
            &token,
            &access,
            Method::POST,
            &format!("/api/v1/orders/{order_id}/cancellations"),
            Some("cross-dock-order-cancel-blocked"),
            Some(json!({
                "expected_revision": plan.order_revision,
                "reason": "client_request",
                "note": null
            })),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let cancellation_body = json!({
        "expected_order_revision": plan.order_revision,
        "reason": "demand_changed",
        "note": "Use a different receiving lane"
    });
    let cancellation: CancelCrossDockWorkResponse = json_response(
        expect_status(
            send(
                &app,
                &token,
                &access,
                Method::POST,
                &format!("/api/v1/cross-dock-tasks/{}/cancellations", plan.work_id),
                Some("cross-dock-cancel"),
                Some(cancellation_body.clone()),
            )
            .await,
            StatusCode::OK,
            "cancel cross-dock work",
        )
        .await,
    )
    .await;
    assert_eq!(cancellation.status, CrossDockWorkStatus::Cancelled);
    assert_eq!(cancellation.quantity, 5);
    let cancellation_replay: CancelCrossDockWorkResponse = json_response(
        expect_status(
            send(
                &app,
                &token,
                &access,
                Method::POST,
                &format!("/api/v1/cross-dock-tasks/{}/cancellations", plan.work_id),
                Some("cross-dock-cancel"),
                Some(cancellation_body),
            )
            .await,
            StatusCode::OK,
            "replay cross-dock cancellation",
        )
        .await,
    )
    .await;
    assert_eq!(cancellation_replay, cancellation);
    assert_eq!(
        send(
            &app,
            &token,
            &access,
            Method::POST,
            &format!("/api/v1/cross-dock-tasks/{}/cancellations", plan.work_id),
            Some("cross-dock-cancel"),
            Some(json!({
                "expected_order_revision": plan.order_revision,
                "reason": "operational_change",
                "note": null
            })),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let replan_body = json!({
      "order_line_id":order_item_id,"expected_order_revision":cancellation.order_revision,
      "source_receipt_inventory_transaction_id":receipt_transaction_id,
      "destination_pick_face_location_id":destination_location_id,"quantity":5,"priority":25,
      "instructions":"Move directly from receiving to the forward pick face"
    });
    let plan: PlanCrossDockWorkResponse = json_response(
        expect_status(
            send(
                &app,
                &token,
                &access,
                Method::POST,
                &format!("/api/v1/orders/{order_id}/cross-dock-tasks"),
                Some("cross-dock-replan"),
                Some(replan_body),
            )
            .await,
            StatusCode::OK,
            "replan cancelled cross-dock work",
        )
        .await,
    )
    .await;
    assert_eq!(plan.previous_order_revision, cancellation.order_revision);
    let page: CrossDockWorkPage = json_response(
        expect_status(
            send(
                &app,
                &token,
                &access,
                Method::GET,
                "/api/v1/cross-dock-queue?status=pending",
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "cross-dock queue",
        )
        .await,
    )
    .await;
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].work_id, plan.work_id);
    let claim: CrossDockClaimResponse = json_response(
        expect_status(
            send(
                &app,
                &token,
                &access,
                Method::POST,
                &format!("/api/v1/cross-dock-claims/{}", plan.work_id),
                Some("cross-dock-claim"),
                Some(json!({})),
            )
            .await,
            StatusCode::OK,
            "claim cross-dock",
        )
        .await,
    )
    .await;
    assert_eq!(claim.quantity, 5);
    assert_eq!(claim.source_receiving_location.barcode, "XD-RECV-01");
    assert_eq!(claim.destination_pick_face.barcode, "XD-PICK-01");
    let scans = json!({"source_receiving_location_barcode":"XD-RECV-01","item_barcode":"XD-ITEM-01",
      "lot_scan":"XD-RECEIPT","serial_scan":null,"destination_pick_face_barcode":"XD-PICK-01"});
    let mut wrong = scans.clone();
    wrong["destination_pick_face_barcode"] = json!("WRONG");
    assert_eq!(
        send(
            &app,
            &token,
            &access,
            Method::POST,
            &format!("/api/v1/cross-dock-tasks/{}/confirmations", plan.work_id),
            Some("cross-dock-wrong"),
            Some(wrong)
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let confirmed: ConfirmCrossDockWorkResponse = json_response(
        expect_status(
            send(
                &app,
                &token,
                &access,
                Method::POST,
                &format!("/api/v1/cross-dock-tasks/{}/confirmations", plan.work_id),
                Some("cross-dock-confirm"),
                Some(scans.clone()),
            )
            .await,
            StatusCode::OK,
            "confirm cross-dock",
        )
        .await,
    )
    .await;
    assert_eq!(confirmed.status, CrossDockWorkStatus::Completed);
    assert_eq!(confirmed.quantity, 5);
    let confirmed_replay: ConfirmCrossDockWorkResponse = json_response(
        expect_status(
            send(
                &app,
                &token,
                &access,
                Method::POST,
                &format!("/api/v1/cross-dock-tasks/{}/confirmations", plan.work_id),
                Some("cross-dock-confirm"),
                Some(scans.clone()),
            )
            .await,
            StatusCode::OK,
            "replay cross-dock confirmation",
        )
        .await,
    )
    .await;
    assert_eq!(confirmed_replay, confirmed);
    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let row=sqlx::query(
      r#"SELECT
        (SELECT qty_on_hand FROM inventory_balances WHERE tenant_id=$1 AND id=$2) source_on_hand,
        destination.qty_on_hand destination_on_hand,destination.qty_reserved destination_reserved,
        allocation.qty allocation_qty,allocation.status allocation_status,
        transaction.transaction_type,transaction.operation,transaction.reference_type,
        (SELECT COUNT(*) FROM inventory_entries entry WHERE entry.tenant_id=$1 AND entry.transaction_id=transaction.id) entry_count,
        (SELECT COALESCE(SUM(quantity_delta),0)::bigint FROM inventory_entries entry WHERE entry.tenant_id=$1 AND entry.transaction_id=transaction.id) net_quantity,
        (SELECT released_at IS NOT NULL FROM loose_inventory_movement_claims WHERE tenant_id=$1 AND work_task_id=$3) source_claim_released
      FROM cross_dock_confirmations confirmation
      JOIN inventory_balances destination ON destination.tenant_id=confirmation.tenant_id AND destination.id=confirmation.destination_inventory_balance_id
      JOIN inventory_allocations allocation ON allocation.tenant_id=confirmation.tenant_id AND allocation.id=confirmation.inventory_allocation_id
      JOIN inventory_transactions transaction ON transaction.tenant_id=confirmation.tenant_id AND transaction.id=confirmation.inventory_transaction_id
      WHERE confirmation.tenant_id=$1 AND confirmation.task_id=$3"#,
    ).bind(access.tenant_id.get()).bind(receipt.inventory_balance_id.unwrap()).bind(plan.work_id)
      .fetch_one(&mut *tx).await.unwrap();
    assert_eq!(row.get::<i64, _>("source_on_hand"), 0);
    assert_eq!(row.get::<i64, _>("destination_on_hand"), 5);
    assert_eq!(row.get::<i64, _>("destination_reserved"), 5);
    assert_eq!(row.get::<i64, _>("allocation_qty"), 5);
    assert_eq!(row.get::<String, _>("allocation_status"), "allocated");
    assert_eq!(row.get::<String, _>("transaction_type"), "move");
    assert_eq!(
        row.get::<String, _>("operation"),
        "inbound.cross_dock.confirm.v1"
    );
    assert_eq!(
        row.get::<Option<String>, _>("reference_type").as_deref(),
        Some("cross_dock_task")
    );
    assert_eq!(row.get::<i64, _>("entry_count"), 2);
    assert_eq!(row.get::<i64, _>("net_quantity"), 0);
    assert!(row.get::<bool, _>("source_claim_released"));
    tx.rollback().await.unwrap();

    wareboxes_api::repo::tenants::update_user_access_scope(
        &fixture.db,
        access.tenant_id,
        &UpdateUserAccessScope {
            user_id: operator.id,
            all_facilities: true,
            facility_ids: vec![],
            all_inventory_owners: false,
            inventory_owner_ids: vec![],
        },
    )
    .await
    .unwrap();
    assert_eq!(
        send(
            &app,
            &token,
            &access,
            Method::POST,
            &format!("/api/v1/cross-dock-tasks/{}/confirmations", plan.work_id),
            Some("cross-dock-confirm"),
            Some(scans.clone()),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    wareboxes_api::repo::tenants::update_user_access_scope(
        &fixture.db,
        access.tenant_id,
        &UpdateUserAccessScope {
            user_id: operator.id,
            all_facilities: false,
            facility_ids: vec![],
            all_inventory_owners: true,
            inventory_owner_ids: vec![],
        },
    )
    .await
    .unwrap();
    assert_eq!(
        send(
            &app,
            &token,
            &access,
            Method::POST,
            &format!("/api/v1/cross-dock-tasks/{}/confirmations", plan.work_id),
            Some("cross-dock-confirm"),
            Some(scans),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

async fn receive_expected(
    fixture: &Fixture,
    access: &TenantAccess,
    owner_id: i64,
    facility_id: i64,
    receiving_location_id: i64,
    item_id: i64,
    quantity: i64,
) -> ReceiveExpectedInventoryResult {
    let load_id = wareboxes_api::repo::loads::add_load(
        &fixture.db,
        access.tenant_id,
        access.user_id.get(),
        facility_id,
        owner_id,
        LoadType::Inbound,
        Some("XD-RECEIPT"),
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
    let line_id = wareboxes_api::repo::loads::add_line(
        &fixture.db,
        access.tenant_id,
        access.user_id.get(),
        load_id,
        item_id,
        None,
        quantity,
        Some("XD-RECEIPT"),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(wareboxes_api::repo::loads::update_load(
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
        None
    )
    .await
    .unwrap());
    let load = wareboxes_api::repo::loads::get_load(&fixture.db, access.tenant_id, load_id, false)
        .await
        .unwrap()
        .unwrap();
    wareboxes_api::repo::inbound_load::start_inbound_load_unloading(
        &fixture.db,
        access,
        &CommandContext {
            tenant_id: access.tenant_id,
            actor_id: access.user_id,
            request_id: "cross-dock-unloading-request".to_owned(),
            idempotency_key: Some("cross-dock-unloading".to_owned()),
        },
        &StartInboundLoadUnloadingCommand::new(
            InboundLoadId::new(load_id).unwrap(),
            InboundLoadScanValue::new(load.execution_barcode).unwrap(),
            InboundLoadScanValue::new("XD-RECV-01").unwrap(),
            None,
            None,
        ),
    )
    .await
    .unwrap();
    wareboxes_api::repo::inbound_receipt::receive_expected_inventory(
        &fixture.db,
        access,
        &CommandContext {
            tenant_id: access.tenant_id,
            actor_id: access.user_id,
            request_id: "cross-dock-receipt-request".to_owned(),
            idempotency_key: Some("cross-dock-receipt".to_owned()),
        },
        line_id,
        &wareboxes_api::repo::inbound_receipt::ReceiveExpectedInventoryCommand {
            receiving_location_id: Some(receiving_location_id),
            received_qty: quantity,
            rejected_qty: 0,
            missing_qty: 0,
            license_plate_id: None,
            license_plate_barcode: None,
            lot: Some("XD-RECEIPT"),
            serial: None,
            expiration: None,
            exception_reason: None::<InboundReceiptExceptionReason>,
            exception_note: None,
        },
    )
    .await
    .unwrap()
}

async fn grant_permission(
    db: &db::Db,
    access: &TenantAccess,
    role_name: &str,
    permission_name: &str,
) {
    let permission = match wareboxes_persistence_postgres::permissions::find_by_name(
        db,
        access.tenant_id,
        permission_name,
    )
    .await
    .unwrap()
    {
        Some(value) => value.id,
        None => wareboxes_persistence_postgres::permissions::add_permission(
            db,
            access.tenant_id,
            permission_name,
            Some(permission_name),
        )
        .await
        .unwrap(),
    };
    let role = wareboxes_persistence_postgres::roles::add_role(
        db,
        access.tenant_id,
        role_name,
        Some(role_name),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        db,
        access.tenant_id,
        role,
        permission
    )
    .await
    .unwrap());
    assert!(wareboxes_persistence_postgres::roles::add_role_to_user(
        db,
        access.tenant_id,
        access.user_id.get(),
        role
    )
    .await
    .unwrap());
}

async fn send(
    app: &axum::Router,
    token: &str,
    access: &TenantAccess,
    method: Method,
    path: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, access.tenant_id.to_string());
    if let Some(key) = key {
        request = request
            .header(IDEMPOTENCY_KEY_HEADER, key)
            .header(REQUEST_ID_HEADER, format!("request-{key}"));
    }
    let body = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    app.clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}
async fn expect_status(
    response: axum::response::Response,
    expected: StatusCode,
    context: &str,
) -> axum::response::Response {
    if response.status() == expected {
        return response;
    }
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    panic!(
        "{context}: expected {expected}, got {status}: {}",
        String::from_utf8_lossy(&bytes)
    )
}
async fn json_response<T: DeserializeOwned>(response: axum::response::Response) -> T {
    let bytes = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
