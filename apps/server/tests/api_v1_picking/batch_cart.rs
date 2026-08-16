use super::*;
use wareboxes_api_contract::v1::{
    PickCartResponse, PickClusterResponse, PickClusterWorkspaceResponse, PickExecutionMethod,
    PickRouteMode,
};

struct BatchOrderSetup<'a> {
    owner_id: i64,
    facility_id: i64,
    destination_id: i64,
    item_id: i64,
    key: &'a str,
    quantity: i64,
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
        &format!("batch-cart-supervisor-{user_id}"),
        Some("Batch-cart planning"),
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

async fn allocate_and_release(
    fixture: &Fixture,
    app: &axum::Router,
    token: &str,
    access: &wareboxes_core::models::TenantAccess,
    setup: BatchOrderSetup<'_>,
) -> i64 {
    let order_id = fixture
        .order_header(access.tenant_id, setup.key, setup.owner_id)
        .await;
    fixture
        .order_item(access.tenant_id, order_id, setup.item_id, setup.quantity)
        .await;
    let allocation = send(
        app,
        token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{order_id}/allocation-runs"),
        Some(&format!("{}-allocate", setup.key)),
        Some(
            serde_json::to_value(PlanOrderAllocationRequest {
                facility_id: setup.facility_id,
                expected_revision: Revision::new(1).unwrap(),
                expected_policy: AllocationPolicyReference::product_default(),
            })
            .unwrap(),
        ),
    )
    .await;
    expect_status(allocation, StatusCode::OK, "allocate batch order").await;
    let release = release(
        app,
        token,
        access.tenant_id,
        order_id,
        Some(&format!("{}-release", setup.key)),
        release_body(setup.facility_id, setup.destination_id, 2),
    )
    .await;
    expect_status(release, StatusCode::OK, "release batch order").await;
    order_id
}

async fn confirm(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    claim: &PickClaimResponse,
    destination_barcode: &str,
    key: &str,
) {
    let response = send(
        app,
        token,
        tenant_id,
        Method::POST,
        &format!(
            "/api/v1/picking-tasks/{}/contents/{}/confirmations",
            claim.task_id, claim.content.content_id
        ),
        Some(key),
        Some(json!({
            "source_location_barcode": claim.content.source_location_barcode,
            "item_barcode": claim.content.item_barcodes[0],
            "destination_license_plate_barcode": destination_barcode,
        })),
    )
    .await;
    expect_status(response, StatusCode::OK, "confirm batch pick").await;
}

#[tokio::test]
async fn homogeneous_multi_order_work_is_a_frozen_replay_safe_batch_cart_route() {
    let fixture = Fixture::new().await;
    let supervisor = fixture.wms_user("batch-cart@test.local").await;
    let access = default_tenant_for_user(&fixture.db, supervisor.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        supervisor.id,
        "batch-cart-orders",
    )
    .await;
    grant_supervisor(&fixture.db, access.tenant_id, supervisor.id).await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Batch Cart Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Batch Cart Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let destination_id =
        staging_location(&fixture, access.tenant_id, facility_id, "BATCH-STAGE").await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        destination_id,
        "BATCH-CART-A",
    )
    .await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        destination_id,
        "BATCH-CART-B",
    )
    .await;
    let item_id = fixture
        .item(access.tenant_id, "Batch-picked item", "each")
        .await;
    repo::items::add_barcode(
        &fixture.db,
        access.tenant_id,
        item_id,
        "BATCH-ITEM",
        "code128",
        None,
    )
    .await
    .unwrap();
    let source = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id,
                qty: 5,
                key: "BATCH-SOURCE",
            },
        )
        .await;
    let token = auth::create_session(&fixture.db, supervisor.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let first_order = allocate_and_release(
        &fixture,
        &app,
        &token,
        &access,
        BatchOrderSetup {
            owner_id,
            facility_id,
            destination_id,
            item_id,
            key: "BATCH-A",
            quantity: 2,
        },
    )
    .await;
    let second_order = allocate_and_release(
        &fixture,
        &app,
        &token,
        &access,
        BatchOrderSetup {
            owner_id,
            facility_id,
            destination_id,
            item_id,
            key: "BATCH-B",
            quantity: 3,
        },
    )
    .await;

    let cart: PickCartResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                "/api/v1/pick-carts",
                Some("batch-cart-create"),
                Some(json!({
                    "facility_id": facility_id,
                    "barcode": "BATCH-CART",
                    "name": "Batch cart",
                    "slot_codes": ["A", "B"]
                })),
            )
            .await,
            StatusCode::OK,
            "create batch cart",
        )
        .await,
    )
    .await;
    let workspace_path = format!(
        "/api/v1/pick-clusters/workspace?facility_id={facility_id}&inventory_owner_id={owner_id}"
    );
    let workspace: PickClusterWorkspaceResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::GET,
                &workspace_path,
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "load batch candidates",
        )
        .await,
    )
    .await;
    assert_eq!(workspace.candidates.len(), 2);
    let first = &workspace.candidates[0];
    assert!(workspace.candidates.iter().all(|candidate| {
        candidate.source_inventory_balance_id == source.balance_id
            && candidate.source_location_id == source.location_id
            && candidate.source_location_id == first.source_location_id
            && candidate.item_batch_id == first.item_batch_id
            && candidate.uom == first.uom
            && candidate.inventory_status == first.inventory_status
    }));
    let assignments = workspace
        .candidates
        .iter()
        .map(|candidate| {
            let slot_id = if candidate.order_id == first_order {
                cart.slots[0].slot_id
            } else {
                assert_eq!(candidate.order_id, second_order);
                cart.slots[1].slot_id
            };
            json!({"task_id": candidate.task_id, "slot_id": slot_id})
        })
        .collect::<Vec<_>>();

    let mut forged = tenant_tx(&fixture.db, access.tenant_id).await;
    let forged_cluster_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO pick_clusters(
          tenant_id,inventory_owner_id,facility_id,cart_id,mode,
          batch_source_inventory_balance_id,batch_source_location_id,
          batch_item_batch_id,batch_uom,
          batch_inventory_status,batch_total_quantity,status,revision,
          task_count,order_count,planned_by_user_id,planned_at)
        VALUES($1,$2,$3,$4,'batch_cart',$5,$6,$7,$8,$9,999,'planned',1,2,2,$10,
          statement_timestamp()) RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(owner_id)
    .bind(facility_id)
    .bind(cart.cart_id)
    .bind(first.source_inventory_balance_id)
    .bind(first.source_location_id)
    .bind(first.item_batch_id)
    .bind(&first.uom)
    .bind(&first.inventory_status)
    .bind(supervisor.id)
    .fetch_one(&mut *forged)
    .await
    .unwrap();
    for (sequence, candidate) in workspace.candidates.iter().enumerate() {
        let slot_id = if candidate.order_id == first_order {
            cart.slots[0].slot_id
        } else {
            cart.slots[1].slot_id
        };
        sqlx::query(
            r#"INSERT INTO pick_cluster_orders(
              tenant_id,inventory_owner_id,facility_id,cluster_id,cart_id,order_id,slot_id)
            VALUES($1,$2,$3,$4,$5,$6,$7)"#,
        )
        .bind(access.tenant_id.get())
        .bind(owner_id)
        .bind(facility_id)
        .bind(forged_cluster_id)
        .bind(cart.cart_id)
        .bind(candidate.order_id)
        .bind(slot_id)
        .execute(&mut *forged)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO pick_cluster_members(
              tenant_id,inventory_owner_id,facility_id,cluster_id,cart_id,
              order_id,slot_id,task_id,sequence,created_at)
            VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,statement_timestamp())"#,
        )
        .bind(access.tenant_id.get())
        .bind(owner_id)
        .bind(facility_id)
        .bind(forged_cluster_id)
        .bind(cart.cart_id)
        .bind(candidate.order_id)
        .bind(slot_id)
        .bind(candidate.task_id)
        .bind(i64::try_from(sequence + 1).unwrap())
        .execute(&mut *forged)
        .await
        .unwrap();
    }
    let forged_error = forged.commit().await.unwrap_err();
    assert_eq!(
        forged_error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23514")
    );

    let plan_body = json!({
        "inventory_owner_id": owner_id,
        "facility_id": facility_id,
        "cart_id": cart.cart_id,
        "assignments": assignments,
    });
    let planned: PickClusterResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                "/api/v1/pick-clusters",
                Some("batch-plan"),
                Some(plan_body.clone()),
            )
            .await,
            StatusCode::OK,
            "plan batch cart",
        )
        .await,
    )
    .await;
    assert_eq!(planned.mode, PickRouteMode::BatchCart);
    assert_eq!(
        planned.batch_source_inventory_balance_id,
        Some(first.source_inventory_balance_id)
    );
    assert_eq!(planned.batch_source_location_id, Some(source.location_id));
    assert_eq!(
        planned.batch_source_location_barcode.as_deref(),
        Some("BATCH-SOURCE")
    );
    assert_eq!(planned.batch_item_batch_id, Some(first.item_batch_id));
    assert_eq!(planned.batch_uom.as_deref(), Some("each"));
    assert_eq!(planned.batch_inventory_status.as_deref(), Some("available"));
    assert_eq!(planned.batch_total_quantity, Some(5));
    let replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/pick-clusters",
        Some("batch-plan"),
        Some(plan_body.clone()),
    )
    .await;
    assert_eq!(response_json::<PickClusterResponse>(replay).await, planned);
    let conflict = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/pick-clusters",
        Some("batch-plan"),
        Some(json!({
            "inventory_owner_id": owner_id,
            "facility_id": facility_id,
            "cart_id": cart.cart_id,
            "assignments": [plan_body["assignments"][1].clone(), plan_body["assignments"][0].clone()],
        })),
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let claim_path = format!("/api/v1/pick-clusters/{}/claims/next", planned.cluster_id);
    let first_claim = response_json::<Option<PickClaimResponse>>(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &claim_path,
                Some("batch-claim-one"),
                Some(json!({})),
            )
            .await,
            StatusCode::OK,
            "claim first batch task",
        )
        .await,
    )
    .await
    .unwrap();
    assert_eq!(first_claim.execution.method, PickExecutionMethod::BatchCart);
    assert_eq!(first_claim.execution.batch_total_quantity, Some(5));
    assert_eq!(first_claim.content.source_location_id, source.location_id);
    let first_destination = if first_claim.execution.slot_code.as_deref() == Some("A") {
        "BATCH-CART-A"
    } else {
        "BATCH-CART-B"
    };
    confirm(
        &app,
        &token,
        access.tenant_id,
        &first_claim,
        first_destination,
        "batch-confirm-one",
    )
    .await;
    let second_claim = response_json::<Option<PickClaimResponse>>(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &claim_path,
                Some("batch-claim-two"),
                Some(json!({})),
            )
            .await,
            StatusCode::OK,
            "claim second batch task",
        )
        .await,
    )
    .await
    .unwrap();
    assert_eq!(
        second_claim.execution.method,
        PickExecutionMethod::BatchCart
    );
    assert_eq!(second_claim.execution.batch_total_quantity, Some(5));
    assert_eq!(second_claim.content.source_location_id, source.location_id);
    let second_destination = if second_claim.execution.slot_code.as_deref() == Some("A") {
        "BATCH-CART-A"
    } else {
        "BATCH-CART-B"
    };
    confirm(
        &app,
        &token,
        access.tenant_id,
        &second_claim,
        second_destination,
        "batch-confirm-two",
    )
    .await;

    let mut evidence = tenant_tx(&fixture.db, access.tenant_id).await;
    let methods: Vec<String> = sqlx::query_scalar(
        r#"SELECT payload->>'execution_method' FROM outbox_events
        WHERE tenant_id=$1 AND event_type='outbound.pick.confirmed'
          AND aggregate_id=ANY($2) ORDER BY aggregate_id"#,
    )
    .bind(access.tenant_id.get())
    .bind(
        planned
            .members
            .iter()
            .map(|member| member.task_id.to_string())
            .collect::<Vec<_>>(),
    )
    .fetch_all(&mut *evidence)
    .await
    .unwrap();
    let planned_payload: serde_json::Value = sqlx::query_scalar(
        r#"SELECT payload FROM outbox_events WHERE tenant_id=$1
        AND event_type='outbound.pick_cluster.planned' AND aggregate_id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(planned.cluster_id.to_string())
    .fetch_one(&mut *evidence)
    .await
    .unwrap();
    evidence.rollback().await.unwrap();
    assert_eq!(methods, vec!["batch_cart", "batch_cart"]);
    assert_eq!(planned_payload["mode"], "batch_cart");
    assert_eq!(planned_payload["batch_total_quantity"], 5);
}
