use super::*;
use wareboxes_api_contract::v1::{PickExecutionMethod, ReversePickConfirmationResponse};
use wareboxes_application::CommandContext;
use wareboxes_core::models::InboundReceiptExceptionReason;

#[tokio::test]
async fn full_pallet_pick_moves_one_physical_plate_and_conserves_inventory() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("pallet-pick@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "pallet-pick-orders",
    )
    .await;
    grant_supervisor(&fixture.db, access.tenant_id, operator.id).await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Pallet Pick Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Pallet Pick Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let source_location_id = wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        access.tenant_id,
        facility_id,
        None,
        Some("PALLET-PICK-SOURCE"),
        Some("Pallet receiving source"),
        "dock",
        true,
        true,
        true,
    )
    .await
    .unwrap();
    let destination_location_id =
        staging_location(&fixture, access.tenant_id, facility_id, "PALLET-PICK-STAGE").await;
    let source_plate_id = plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        source_location_id,
        "PALLET-PICK-LP",
    )
    .await;
    let item_id = fixture
        .item(access.tenant_id, "Full pallet item", "case")
        .await;
    repo::items::add_barcode(
        &fixture.db,
        access.tenant_id,
        item_id,
        "PALLET-PICK-ITEM",
        "code128",
        None,
    )
    .await
    .unwrap();
    let source_balance_id = receive_into_plate(
        &fixture,
        &access,
        owner_id,
        facility_id,
        source_location_id,
        source_plate_id,
        item_id,
        12,
        "complete",
    )
    .await;
    let order_id = fixture
        .order_header(access.tenant_id, "PALLET-PICK-ORDER", owner_id)
        .await;
    fixture
        .order_item(access.tenant_id, order_id, item_id, 12)
        .await;

    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let allocation = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{order_id}/allocation-runs"),
        Some("pallet-pick-allocation"),
        Some(
            serde_json::to_value(PlanOrderAllocationRequest {
                facility_id,
                expected_revision: Revision::new(1).unwrap(),
                expected_policy: AllocationPolicyReference::product_default(),
            })
            .unwrap(),
        ),
    )
    .await;
    expect_status(allocation, StatusCode::OK, "allocate pallet").await;
    let released = release(
        &app,
        &token,
        access.tenant_id,
        order_id,
        Some("pallet-pick-release"),
        release_body(facility_id, destination_location_id, 2),
    )
    .await;
    expect_status(released, StatusCode::OK, "release pallet pick").await;

    let claim = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/picking-claims/next",
        Some("pallet-pick-claim"),
        Some(json!({})),
    )
    .await;
    let claim = response_json::<Option<PickClaimResponse>>(
        expect_status(claim, StatusCode::OK, "claim pallet pick").await,
    )
    .await
    .unwrap();
    assert_eq!(claim.execution.method, PickExecutionMethod::Pallet);
    assert_eq!(claim.content.source_license_plate_id, Some(source_plate_id));
    assert_eq!(
        claim.content.source_license_plate_barcode.as_deref(),
        Some("PALLET-PICK-LP")
    );
    assert!(claim.suggested_destination_license_plate_barcode.is_none());

    let confirmation_body = json!({
        "source_location_barcode": "PALLET-PICK-SOURCE",
        "item_barcode": "PALLET-PICK-ITEM",
        "source_license_plate_barcode": "PALLET-PICK-LP",
        "destination_license_plate_barcode": "PALLET-PICK-LP"
    });
    let confirmation_path = format!(
        "/api/v1/picking-tasks/{}/contents/{}/confirmations",
        claim.task_id, claim.content.content_id
    );
    let confirmation = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &confirmation_path,
        Some("pallet-pick-confirm"),
        Some(confirmation_body.clone()),
    )
    .await;
    let confirmation: PickContentConfirmationResponse =
        response_json(expect_status(confirmation, StatusCode::OK, "confirm pallet pick").await)
            .await;
    assert_eq!(confirmation.source_license_plate_id, Some(source_plate_id));
    assert_eq!(confirmation.destination_license_plate_id, source_plate_id);
    assert_ne!(
        confirmation.source_inventory_balance_id,
        confirmation.destination_inventory_balance_id
    );
    assert_eq!(confirmation.picked_quantity, 12);
    assert!(confirmation.task_completed);

    let replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &confirmation_path,
        Some("pallet-pick-confirm"),
        Some(confirmation_body),
    )
    .await;
    assert_eq!(
        response_json::<PickContentConfirmationResponse>(replay).await,
        confirmation
    );

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let source_quantity: i64 = sqlx::query_scalar(
        "SELECT qty_on_hand FROM inventory_balances WHERE tenant_id=$1 AND id=$2",
    )
    .bind(access.tenant_id.get())
    .bind(source_balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let destination: (i64, i64, i64) = sqlx::query_as(
        r#"SELECT location_id,license_plate_id,qty_on_hand FROM inventory_balances
        WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(confirmation.destination_inventory_balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let plate_location: i64 =
        sqlx::query_scalar("SELECT location_id FROM license_plates WHERE tenant_id=$1 AND id=$2")
            .bind(access.tenant_id.get())
            .bind(source_plate_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let bound_order_id: i64 = sqlx::query_scalar(
        r#"SELECT order_id FROM outbound_order_containers
        WHERE tenant_id=$1 AND license_plate_id=$2 AND released_at IS NULL"#,
    )
    .bind(access.tenant_id.get())
    .bind(source_plate_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let journal_total: i64 = sqlx::query_scalar(
        r#"SELECT COALESCE(SUM(quantity_delta),0)::bigint FROM inventory_entries
        WHERE tenant_id=$1 AND transaction_id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(confirmation.inventory_transaction_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let event_evidence: (String, String) = sqlx::query_as(
        r#"SELECT payload->>'uom',payload->>'execution_method' FROM outbox_events
        WHERE tenant_id=$1 AND event_type='outbound.pick.confirmed'
          AND aggregate_id=$2 ORDER BY id DESC LIMIT 1"#,
    )
    .bind(access.tenant_id.get())
    .bind(claim.task_id.to_string())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();

    assert_eq!(source_quantity, 0);
    assert_eq!(destination, (destination_location_id, source_plate_id, 12));
    assert_eq!(plate_location, destination_location_id);
    assert_eq!(bound_order_id, order_id);
    assert_eq!(journal_total, 0);
    assert_eq!(event_evidence, ("case".into(), "pallet".into()));

    let reversal = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/pick-confirmations/{}/reversals",
            confirmation.result_id
        ),
        Some("pallet-pick-reversal"),
        Some(json!({
            "expected_order_revision": confirmation.order_revision.get(),
            "staged_location_barcode": "PALLET-PICK-STAGE",
            "staged_license_plate_barcode": "PALLET-PICK-LP",
            "item_barcode": "PALLET-PICK-ITEM",
            "lot_scan": "PALLET-PICK-LOT-complete",
            "return_location_barcode": "PALLET-PICK-SOURCE",
            "return_license_plate_barcode": "PALLET-PICK-LP",
            "reason": "mis_pick",
            "note": "Return the intact pallet"
        })),
    )
    .await;
    let reversal: ReversePickConfirmationResponse =
        response_json(expect_status(reversal, StatusCode::OK, "reverse full pallet pick").await)
            .await;
    assert_eq!(reversal.source_license_plate_id, Some(source_plate_id));
    assert_eq!(reversal.staged_license_plate_id, source_plate_id);
    assert_eq!(reversal.reversed_quantity, 12);

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let reversed_source: (i64, i64, i64) = sqlx::query_as(
        r#"SELECT location_id,qty_on_hand,qty_reserved FROM inventory_balances
        WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(source_balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let reversed_staged: i64 = sqlx::query_scalar(
        "SELECT qty_on_hand FROM inventory_balances WHERE tenant_id=$1 AND id=$2",
    )
    .bind(access.tenant_id.get())
    .bind(confirmation.destination_inventory_balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let returned_plate_location: i64 =
        sqlx::query_scalar("SELECT location_id FROM license_plates WHERE tenant_id=$1 AND id=$2")
            .bind(access.tenant_id.get())
            .bind(source_plate_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let reversal_journal_total: i64 = sqlx::query_scalar(
        r#"SELECT COALESCE(SUM(quantity_delta),0)::bigint FROM inventory_entries
        WHERE tenant_id=$1 AND transaction_id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(reversal.inventory_transaction_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(reversed_source, (source_location_id, 12, 12));
    assert_eq!(reversed_staged, 0);
    assert_eq!(returned_plate_location, source_location_id);
    assert_eq!(reversal_journal_total, 0);

    let reclaimed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/picking-claims/{}", claim.task_id),
        Some("pallet-pick-reclaim"),
        Some(json!({})),
    )
    .await;
    let reclaimed: PickClaimResponse =
        response_json(expect_status(reclaimed, StatusCode::OK, "reclaim full pallet").await).await;
    assert_eq!(reclaimed.execution.method, PickExecutionMethod::Pallet);
    assert!(reclaimed
        .suggested_destination_license_plate_barcode
        .is_none());
    let repick = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/picking-tasks/{}/contents/{}/confirmations",
            reclaimed.task_id, reclaimed.content.content_id
        ),
        Some("pallet-pick-reconfirm"),
        Some(json!({
            "source_location_barcode": "PALLET-PICK-SOURCE",
            "item_barcode": "PALLET-PICK-ITEM",
            "source_license_plate_barcode": "PALLET-PICK-LP",
            "destination_license_plate_barcode": "PALLET-PICK-LP"
        })),
    )
    .await;
    let repick: PickContentConfirmationResponse = response_json(
        expect_status(repick, StatusCode::OK, "confirm full pallet after reversal").await,
    )
    .await;
    assert_ne!(repick.result_id, confirmation.result_id);
    assert_eq!(repick.destination_license_plate_id, source_plate_id);
}

#[tokio::test]
async fn partial_plate_inventory_remains_case_work_and_cannot_move_as_a_pallet() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("partial-pallet-pick@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "partial-pallet-pick-orders",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Partial Pallet Pick Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Partial Pallet Pick Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let source_location_id = wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        access.tenant_id,
        facility_id,
        None,
        Some("PARTIAL-PALLET-SOURCE"),
        Some("Partial pallet source"),
        "dock",
        true,
        true,
        true,
    )
    .await
    .unwrap();
    let destination_location_id = staging_location(
        &fixture,
        access.tenant_id,
        facility_id,
        "PARTIAL-PALLET-STAGE",
    )
    .await;
    let source_plate_id = plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        source_location_id,
        "PARTIAL-PALLET-LP",
    )
    .await;
    let item_id = fixture
        .item(access.tenant_id, "Partial pallet item", "case")
        .await;
    repo::items::add_barcode(
        &fixture.db,
        access.tenant_id,
        item_id,
        "PARTIAL-PALLET-ITEM",
        "code128",
        None,
    )
    .await
    .unwrap();
    let source_balance_id = receive_into_plate(
        &fixture,
        &access,
        owner_id,
        facility_id,
        source_location_id,
        source_plate_id,
        item_id,
        12,
        "partial",
    )
    .await;
    let order_id = fixture
        .order_header(access.tenant_id, "PARTIAL-PALLET-ORDER", owner_id)
        .await;
    fixture
        .order_item(access.tenant_id, order_id, item_id, 6)
        .await;

    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let allocation = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{order_id}/allocation-runs"),
        Some("partial-pallet-allocation"),
        Some(
            serde_json::to_value(PlanOrderAllocationRequest {
                facility_id,
                expected_revision: Revision::new(1).unwrap(),
                expected_policy: AllocationPolicyReference::product_default(),
            })
            .unwrap(),
        ),
    )
    .await;
    expect_status(allocation, StatusCode::OK, "allocate partial pallet").await;
    let released = release(
        &app,
        &token,
        access.tenant_id,
        order_id,
        Some("partial-pallet-release"),
        release_body(facility_id, destination_location_id, 2),
    )
    .await;
    expect_status(released, StatusCode::OK, "release partial pallet").await;

    let claim = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/picking-claims/next",
        Some("partial-pallet-claim"),
        Some(json!({})),
    )
    .await;
    let claim = response_json::<Option<PickClaimResponse>>(
        expect_status(claim, StatusCode::OK, "claim partial pallet").await,
    )
    .await
    .unwrap();
    assert_eq!(claim.execution.method, PickExecutionMethod::Case);

    let rejected = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/picking-tasks/{}/contents/{}/confirmations",
            claim.task_id, claim.content.content_id
        ),
        Some("partial-pallet-confirm"),
        Some(json!({
            "source_location_barcode": "PARTIAL-PALLET-SOURCE",
            "item_barcode": "PARTIAL-PALLET-ITEM",
            "source_license_plate_barcode": "PARTIAL-PALLET-LP",
            "destination_license_plate_barcode": "PARTIAL-PALLET-LP"
        })),
    )
    .await;
    let rejected = expect_status(
        rejected,
        StatusCode::CONFLICT,
        "reject partial pallet movement",
    )
    .await;
    assert_eq!(
        response_json::<ErrorResponse>(rejected).await.reason,
        ErrorReason::Conflict
    );

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let source: (i64, i64, i64) = sqlx::query_as(
        r#"SELECT location_id,qty_on_hand,qty_reserved FROM inventory_balances
        WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(source_balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let plate_location: i64 =
        sqlx::query_scalar("SELECT location_id FROM license_plates WHERE tenant_id=$1 AND id=$2")
            .bind(access.tenant_id.get())
            .bind(source_plate_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let confirmations: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pick_confirmations WHERE tenant_id=$1 AND task_id=$2",
    )
    .bind(access.tenant_id.get())
    .bind(claim.task_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(source, (source_location_id, 12, 6));
    assert_eq!(plate_location, source_location_id);
    assert_eq!(confirmations, 0);
}

#[allow(clippy::too_many_arguments)]
async fn receive_into_plate(
    fixture: &Fixture,
    access: &wareboxes_core::models::TenantAccess,
    inventory_owner_id: i64,
    facility_id: i64,
    source_location_id: i64,
    source_plate_id: i64,
    item_id: i64,
    quantity: i64,
    key: &str,
) -> i64 {
    let load_reference = format!("PALLET-PICK-LOAD-{key}");
    let lot = format!("PALLET-PICK-LOT-{key}");
    let unload_key = format!("pallet-pick-unload-{key}");
    let receive_key = format!("pallet-pick-receive-{key}");
    let load_id = repo::loads::add_load(
        &fixture.db,
        access.tenant_id,
        access.user_id.get(),
        facility_id,
        inventory_owner_id,
        LoadType::Inbound,
        Some(&load_reference),
        None,
        None,
        None,
        None,
        Some(source_location_id),
        None,
        None,
    )
    .await
    .unwrap();
    let line_id = repo::loads::add_line(
        &fixture.db,
        access.tenant_id,
        access.user_id.get(),
        load_id,
        item_id,
        None,
        quantity,
        Some(&lot),
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
    start_expected_receipt_unloading(
        &fixture.db,
        access,
        load_id,
        source_location_id,
        &unload_key,
    )
    .await;
    repo::inbound_receipt::receive_expected_inventory(
        &fixture.db,
        access,
        &CommandContext {
            tenant_id: access.tenant_id,
            actor_id: access.user_id,
            request_id: format!("request-{receive_key}"),
            idempotency_key: Some(receive_key),
        },
        line_id,
        &repo::inbound_receipt::ReceiveExpectedInventoryCommand {
            receiving_location_id: Some(source_location_id),
            received_qty: quantity,
            rejected_qty: 0,
            missing_qty: 0,
            license_plate_id: Some(source_plate_id),
            license_plate_barcode: None,
            lot: Some(&lot),
            serial: None,
            expiration: None,
            exception_reason: None::<InboundReceiptExceptionReason>,
            exception_note: None,
        },
    )
    .await
    .unwrap()
    .inventory_balance_id
    .unwrap()
}

async fn grant_supervisor(db: &db::Db, tenant_id: TenantId, user_id: i64) {
    let permission_id = match wareboxes_persistence_postgres::permissions::find_by_name(
        db,
        tenant_id,
        "wms_supervisor",
    )
    .await
    .unwrap()
    {
        Some(permission) => permission.id,
        None => wareboxes_persistence_postgres::permissions::add_permission(
            db,
            tenant_id,
            "wms_supervisor",
            Some("WMS supervisor"),
        )
        .await
        .unwrap(),
    };
    let role = wareboxes_persistence_postgres::roles::add_role(
        db,
        tenant_id,
        &format!("pallet-pick-supervisor-{user_id}"),
        Some("Reverse full pallet picks"),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        db,
        tenant_id,
        role,
        permission_id,
    )
    .await
    .unwrap());
    assert!(
        wareboxes_persistence_postgres::roles::add_role_to_user(db, tenant_id, user_id, role,)
            .await
            .unwrap()
    );
}
