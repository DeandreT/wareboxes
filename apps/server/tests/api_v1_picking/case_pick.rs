use super::*;
use wareboxes_api_contract::v1::PickExecutionMethod;

#[tokio::test]
async fn case_pick_is_explicit_replay_safe_and_conserves_case_inventory() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("case-pick@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "case-pick-orders",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Case Pick Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Case Pick Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let destination_id =
        staging_location(&fixture, access.tenant_id, facility_id, "CASE-PICK-STAGE").await;
    let destination_plate_id = plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        destination_id,
        "CASE-PICK-TOTE",
    )
    .await;
    let item_id = fixture
        .item(access.tenant_id, "Sealed case item", "case")
        .await;
    repo::items::add_barcode(
        &fixture.db,
        access.tenant_id,
        item_id,
        "CASE-PICK-ITEM",
        "code128",
        None,
    )
    .await
    .unwrap();
    let order_id = fixture
        .order_header(access.tenant_id, "CASE-PICK-ORDER", owner_id)
        .await;
    fixture
        .order_item(access.tenant_id, order_id, item_id, 3)
        .await;
    let source = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id,
                qty: 5,
                key: "CASE-PICK-SOURCE",
            },
        )
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
        Some("case-pick-allocation"),
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
    expect_status(allocation, StatusCode::OK, "allocate cases").await;
    let released = release(
        &app,
        &token,
        access.tenant_id,
        order_id,
        Some("case-pick-release"),
        release_body(facility_id, destination_id, 2),
    )
    .await;
    expect_status(released, StatusCode::OK, "release case pick").await;

    let claim = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/picking-claims/next",
        Some("case-pick-claim"),
        Some(json!({})),
    )
    .await;
    let claim = response_json::<Option<PickClaimResponse>>(
        expect_status(claim, StatusCode::OK, "claim case pick").await,
    )
    .await
    .unwrap();
    assert_eq!(claim.execution.method, PickExecutionMethod::Case);
    assert_eq!(claim.content.uom, "case");
    assert_eq!(claim.content.planned_quantity, 3);
    assert!(claim.execution.cluster_id.is_none());
    assert!(claim.execution.cart_barcode.is_none());

    let confirmation_body = json!({
        "source_location_barcode": claim.content.source_location_barcode,
        "item_barcode": claim.content.item_barcodes[0],
        "destination_license_plate_barcode": "CASE-PICK-TOTE"
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
        Some("case-pick-confirm"),
        Some(confirmation_body.clone()),
    )
    .await;
    let confirmation: PickContentConfirmationResponse =
        response_json(expect_status(confirmation, StatusCode::OK, "confirm case pick").await).await;
    assert_eq!(confirmation.picked_quantity, 3);
    assert!(confirmation.task_completed);
    let replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &confirmation_path,
        Some("case-pick-confirm"),
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
    .bind(source.balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let destination: (i64, String) = sqlx::query_as(
        r#"SELECT qty_on_hand,uom FROM inventory_balances
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
          AND license_plate_id=$4 AND item_id=$5 AND deleted IS NULL"#,
    )
    .bind(access.tenant_id.get())
    .bind(owner_id)
    .bind(facility_id)
    .bind(destination_plate_id)
    .bind(item_id)
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
    assert_eq!(source_quantity, 2);
    assert_eq!(destination, (3, "case".into()));
    assert_eq!(event_evidence, ("case".into(), "case".into()));
}
