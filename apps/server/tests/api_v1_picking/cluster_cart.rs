use super::*;
use wareboxes_api_contract::v1::{
    PickCartResponse, PickCartStatus, PickClusterResponse, PickClusterStatus,
    PickClusterWorkspaceResponse, PickExecutionMethod, PickRouteMode,
};

struct ClusterOrderSetup<'a> {
    owner_id: i64,
    facility_id: i64,
    destination_id: i64,
    key: &'a str,
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
        &format!("cluster-supervisor-{user_id}"),
        Some("Cluster-cart planning"),
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

async fn released_single_line_order(
    fixture: &Fixture,
    app: &axum::Router,
    token: &str,
    access: &wareboxes_core::models::TenantAccess,
    setup: ClusterOrderSetup<'_>,
) -> AllocatedOrder {
    let order = allocated_order(
        fixture,
        app,
        token,
        access,
        setup.owner_id,
        setup.facility_id,
        setup.key,
        &[2],
        &[2],
    )
    .await;
    let response = release(
        app,
        token,
        access.tenant_id,
        order.order_id,
        Some(&format!("{}-release", setup.key)),
        release_body(setup.facility_id, setup.destination_id, 2),
    )
    .await;
    expect_status(response, StatusCode::OK, "release cluster order").await;
    order
}

async fn confirm_cluster_pick(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    claim: &PickClaimResponse,
    destination_barcode: &str,
    key: &str,
) -> PickContentConfirmationResponse {
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
    let response = expect_status(response, StatusCode::OK, "confirm cluster pick").await;
    response_json(response).await
}

#[tokio::test]
async fn cluster_cart_plans_two_orders_claims_exclusively_and_completes_with_slot_evidence() {
    let fixture = Fixture::new().await;
    let supervisor = fixture.wms_user("pick-cluster@test.local").await;
    let access = default_tenant_for_user(&fixture.db, supervisor.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        supervisor.id,
        "pick-cluster-orders",
    )
    .await;
    grant_supervisor(&fixture.db, access.tenant_id, supervisor.id).await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Cluster Cart Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Cluster Cart Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let destination_id =
        staging_location(&fixture, access.tenant_id, facility_id, "CLUSTER-STAGE").await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        destination_id,
        "CLUSTER-CART-01-A",
    )
    .await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        destination_id,
        "CLUSTER-CART-01-B",
    )
    .await;
    let token = auth::create_session(&fixture.db, supervisor.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let first_order = released_single_line_order(
        &fixture,
        &app,
        &token,
        &access,
        ClusterOrderSetup {
            owner_id,
            facility_id,
            destination_id,
            key: "CLUSTER-A",
        },
    )
    .await;
    let second_order = released_single_line_order(
        &fixture,
        &app,
        &token,
        &access,
        ClusterOrderSetup {
            owner_id,
            facility_id,
            destination_id,
            key: "CLUSTER-B",
        },
    )
    .await;

    let cart = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/pick-carts",
        Some("cluster-cart-create"),
        Some(json!({
            "facility_id": facility_id,
            "barcode": "CLUSTER-CART-01",
            "name": "Cluster cart 01",
            "slot_codes": ["A", "B"]
        })),
    )
    .await;
    let cart = response_json::<PickCartResponse>(
        expect_status(cart, StatusCode::OK, "create pick cart").await,
    )
    .await;
    assert_eq!(cart.slots.len(), 2);

    let workspace_path = format!(
        "/api/v1/pick-clusters/workspace?facility_id={facility_id}&inventory_owner_id={owner_id}"
    );
    let workspace = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &workspace_path,
        None,
        None,
    )
    .await;
    let workspace: PickClusterWorkspaceResponse =
        response_json(expect_status(workspace, StatusCode::OK, "load cluster workspace").await)
            .await;
    assert_eq!(workspace.candidates.len(), 2);
    let assignments = workspace
        .candidates
        .iter()
        .map(|candidate| {
            let slot_id = if candidate.order_id == first_order.order_id {
                cart.slots[0].slot_id
            } else {
                assert_eq!(candidate.order_id, second_order.order_id);
                cart.slots[1].slot_id
            };
            json!({"task_id": candidate.task_id, "slot_id": slot_id})
        })
        .collect::<Vec<_>>();
    let plan_body = json!({
        "inventory_owner_id": owner_id,
        "facility_id": facility_id,
        "cart_id": cart.cart_id,
        "assignments": assignments,
    });
    let planned = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/pick-clusters",
        Some("cluster-plan"),
        Some(plan_body.clone()),
    )
    .await;
    let planned: PickClusterResponse =
        response_json(expect_status(planned, StatusCode::OK, "plan pick cluster").await).await;
    assert_eq!(planned.status, PickClusterStatus::Planned);
    assert_eq!(planned.mode, PickRouteMode::ClusterCart);
    assert_eq!(planned.batch_total_quantity, None);
    assert_eq!(planned.order_count, 2);
    assert_eq!(planned.task_count, 2);
    assert_eq!(planned.members[0].sequence, 1);
    assert_eq!(planned.members[1].sequence, 2);

    let replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/pick-clusters",
        Some("cluster-plan"),
        Some(plan_body),
    )
    .await;
    assert_eq!(response_json::<PickClusterResponse>(replay).await, planned);

    let direct = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/picking-claims/{}", planned.members[0].task_id),
        Some("cluster-direct-bypass"),
        Some(json!({})),
    )
    .await;
    assert_eq!(direct.status(), StatusCode::CONFLICT);
    let general = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/picking-claims/next",
        Some("cluster-general-bypass"),
        Some(json!({})),
    )
    .await;
    assert!(response_json::<Option<PickClaimResponse>>(
        expect_status(general, StatusCode::OK, "general queue exclusion").await
    )
    .await
    .is_none());

    let second_operator = add_wms_operator(
        &fixture,
        access.tenant_id,
        "pick-cluster-racer@test.local",
        "pick-cluster-racer",
    )
    .await;
    set_scope(
        &fixture.db,
        access.tenant_id,
        second_operator.id,
        vec![facility_id],
        vec![owner_id],
    )
    .await;
    let second_token = auth::create_session(&fixture.db, second_operator.id)
        .await
        .unwrap();
    let claim_path = format!("/api/v1/pick-clusters/{}/claims/next", planned.cluster_id);
    let first_claim = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &claim_path,
        Some("cluster-claim-first"),
        Some(json!({})),
    );
    let second_claim = send(
        &app,
        &second_token,
        access.tenant_id,
        Method::POST,
        &claim_path,
        Some("cluster-claim-second"),
        Some(json!({})),
    );
    let (first_claim, second_claim) = tokio::join!(first_claim, second_claim);
    let (winner, loser, winner_token) = match (first_claim.status(), second_claim.status()) {
        (StatusCode::OK, StatusCode::CONFLICT) => (first_claim, second_claim, token.as_str()),
        (StatusCode::CONFLICT, StatusCode::OK) => {
            (second_claim, first_claim, second_token.as_str())
        }
        statuses => panic!("expected one cluster claimant, got {statuses:?}"),
    };
    assert_eq!(
        response_json::<ErrorResponse>(loser).await.reason,
        ErrorReason::Conflict
    );
    let first_claim = response_json::<Option<PickClaimResponse>>(winner)
        .await
        .unwrap();
    assert_eq!(
        first_claim.execution.method,
        PickExecutionMethod::ClusterCart
    );
    assert_eq!(first_claim.execution.cluster_id, Some(planned.cluster_id));
    assert_eq!(first_claim.execution.task_count, Some(2));
    assert_eq!(first_claim.execution.batch_total_quantity, None);
    let first_plate = match first_claim.execution.slot_code.as_deref() {
        Some("A") => "CLUSTER-CART-01-A",
        Some("B") => "CLUSTER-CART-01-B",
        other => panic!("unexpected first slot {other:?}"),
    };
    let first_confirmation = confirm_cluster_pick(
        &app,
        winner_token,
        access.tenant_id,
        &first_claim,
        first_plate,
        "cluster-confirm-first",
    )
    .await;
    assert!(first_confirmation.task_completed);

    let second_claim = send(
        &app,
        winner_token,
        access.tenant_id,
        Method::POST,
        &claim_path,
        Some("cluster-claim-next"),
        Some(json!({})),
    )
    .await;
    let second_claim = response_json::<Option<PickClaimResponse>>(
        expect_status(second_claim, StatusCode::OK, "claim second cluster task").await,
    )
    .await
    .unwrap();
    assert_ne!(second_claim.order_id, first_claim.order_id);
    let second_plate = match second_claim.execution.slot_code.as_deref() {
        Some("A") => "CLUSTER-CART-01-A",
        Some("B") => "CLUSTER-CART-01-B",
        other => panic!("unexpected second slot {other:?}"),
    };
    confirm_cluster_pick(
        &app,
        winner_token,
        access.tenant_id,
        &second_claim,
        second_plate,
        "cluster-confirm-second",
    )
    .await;

    let history = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &format!("{workspace_path}&include_history=true"),
        None,
        None,
    )
    .await;
    let history: PickClusterWorkspaceResponse =
        response_json(expect_status(history, StatusCode::OK, "load cluster history").await).await;
    let completed = history
        .clusters
        .iter()
        .find(|cluster| cluster.cluster_id == planned.cluster_id)
        .unwrap();
    assert_eq!(completed.status, PickClusterStatus::Completed);
    assert_eq!(completed.revision, 3);
    assert_eq!(completed.completed_task_count, 2);

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let event_types: Vec<String> = sqlx::query_scalar(
        r#"SELECT event_type FROM outbox_events WHERE tenant_id=$1
        AND aggregate_type='pick_cluster' AND aggregate_id=$2 ORDER BY aggregate_sequence"#,
    )
    .bind(access.tenant_id.get())
    .bind(planned.cluster_id.to_string())
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        event_types,
        vec![
            "outbound.pick_cluster.planned",
            "outbound.pick_cluster.started",
            "outbound.pick_cluster.completed",
        ]
    );

    let hidden_owner = fixture
        .inventory_owner(access.tenant_id, "Hidden Cluster Owner")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, hidden_owner, facility_id)
        .await;
    set_scope(
        &fixture.db,
        access.tenant_id,
        supervisor.id,
        vec![facility_id],
        vec![hidden_owner],
    )
    .await;
    let concealed = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &workspace_path,
        None,
        None,
    )
    .await;
    assert_eq!(concealed.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cancelled_routes_release_tasks_and_cart_status_transitions_are_replay_safe() {
    let fixture = Fixture::new().await;
    let supervisor = fixture.wms_user("pick-cluster-lifecycle@test.local").await;
    let access = default_tenant_for_user(&fixture.db, supervisor.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        supervisor.id,
        "pick-cluster-lifecycle-orders",
    )
    .await;
    grant_supervisor(&fixture.db, access.tenant_id, supervisor.id).await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Cluster Lifecycle Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Cluster Lifecycle Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let destination_id = staging_location(
        &fixture,
        access.tenant_id,
        facility_id,
        "CLUSTER-LIFECYCLE-STAGE",
    )
    .await;
    let token = auth::create_session(&fixture.db, supervisor.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let first_order = released_single_line_order(
        &fixture,
        &app,
        &token,
        &access,
        ClusterOrderSetup {
            owner_id,
            facility_id,
            destination_id,
            key: "CLUSTER-LIFECYCLE-A",
        },
    )
    .await;
    released_single_line_order(
        &fixture,
        &app,
        &token,
        &access,
        ClusterOrderSetup {
            owner_id,
            facility_id,
            destination_id,
            key: "CLUSTER-LIFECYCLE-B",
        },
    )
    .await;

    let cart = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/pick-carts",
        Some("cluster-lifecycle-cart"),
        Some(json!({
            "facility_id": facility_id,
            "barcode": "CLUSTER-LIFECYCLE-CART",
            "name": "Cluster lifecycle cart",
            "slot_codes": ["A", "B"]
        })),
    )
    .await;
    let cart = response_json::<PickCartResponse>(
        expect_status(cart, StatusCode::OK, "create lifecycle cart").await,
    )
    .await;
    let workspace_path = format!(
        "/api/v1/pick-clusters/workspace?facility_id={facility_id}&inventory_owner_id={owner_id}"
    );
    let workspace = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &workspace_path,
        None,
        None,
    )
    .await;
    let workspace: PickClusterWorkspaceResponse =
        response_json(expect_status(workspace, StatusCode::OK, "load lifecycle candidates").await)
            .await;
    assert_eq!(workspace.candidates.len(), 2);
    let assignments = workspace
        .candidates
        .iter()
        .zip(cart.slots.iter())
        .map(|(candidate, slot)| json!({"task_id": candidate.task_id, "slot_id": slot.slot_id}))
        .collect::<Vec<_>>();
    let plan_body = json!({
        "inventory_owner_id": owner_id,
        "facility_id": facility_id,
        "cart_id": cart.cart_id,
        "assignments": assignments,
    });
    let first_plan = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/pick-clusters",
        Some("cluster-lifecycle-plan-one"),
        Some(plan_body.clone()),
    )
    .await;
    let first_plan: PickClusterResponse =
        response_json(expect_status(first_plan, StatusCode::OK, "plan lifecycle cluster").await)
            .await;

    let mut revision_tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let order_revision: i64 =
        sqlx::query_scalar("SELECT revision FROM orders WHERE tenant_id=$1 AND id=$2")
            .bind(access.tenant_id.get())
            .bind(first_order.order_id)
            .fetch_one(&mut *revision_tx)
            .await
            .unwrap();
    revision_tx.rollback().await.unwrap();
    let active_route_order_cancel = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{}/cancellations", first_order.order_id),
        Some("cluster-lifecycle-order-cancel"),
        Some(json!({
            "expected_revision": order_revision,
            "reason": "client_request",
            "note": "Cannot silently shrink a clustered route"
        })),
    )
    .await;
    assert_eq!(active_route_order_cancel.status(), StatusCode::CONFLICT);

    let mut raw_cancel_tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let raw_cancel_error = sqlx::query(
        "UPDATE pick_tasks SET status='cancelled',completed_at=statement_timestamp() WHERE tenant_id=$1 AND id=$2",
    )
    .bind(access.tenant_id.get())
    .bind(first_plan.members[0].task_id)
    .execute(&mut *raw_cancel_tx)
    .await
    .unwrap_err();
    let raw_cancel_database_error = raw_cancel_error.as_database_error().unwrap();
    assert_eq!(
        raw_cancel_database_error.code().as_deref(),
        Some("23514"),
        "{}",
        raw_cancel_database_error.message()
    );
    raw_cancel_tx.rollback().await.unwrap();

    let busy_cart = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/pick-carts/{}/status-changes", cart.cart_id),
        Some("cluster-lifecycle-busy-cart"),
        Some(json!({"expected_revision": 1, "status": "out_of_service"})),
    )
    .await;
    assert_eq!(busy_cart.status(), StatusCode::CONFLICT);

    let cancellation_body = json!({
        "expected_revision": first_plan.revision,
        "note": "Route withdrawn for cart inspection"
    });
    let cancel_path = format!(
        "/api/v1/pick-clusters/{}/cancellations",
        first_plan.cluster_id
    );
    let cancelled = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &cancel_path,
        Some("cluster-lifecycle-cancel-one"),
        Some(cancellation_body.clone()),
    )
    .await;
    let cancelled: PickClusterResponse =
        response_json(expect_status(cancelled, StatusCode::OK, "cancel planned cluster").await)
            .await;
    assert_eq!(cancelled.status, PickClusterStatus::Cancelled);
    assert_eq!(cancelled.revision, 2);
    let replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &cancel_path,
        Some("cluster-lifecycle-cancel-one"),
        Some(cancellation_body),
    )
    .await;
    assert_eq!(
        response_json::<PickClusterResponse>(replay).await,
        cancelled
    );
    let changed_replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &cancel_path,
        Some("cluster-lifecycle-cancel-one"),
        Some(json!({
            "expected_revision": first_plan.revision,
            "note": "Different cancellation command"
        })),
    )
    .await;
    assert_eq!(changed_replay.status(), StatusCode::CONFLICT);

    let after_cancel = send(
        &app,
        &token,
        access.tenant_id,
        Method::GET,
        &workspace_path,
        None,
        None,
    )
    .await;
    let after_cancel: PickClusterWorkspaceResponse = response_json(
        expect_status(
            after_cancel,
            StatusCode::OK,
            "reload released cluster tasks",
        )
        .await,
    )
    .await;
    assert_eq!(after_cancel.candidates.len(), 2);
    assert!(after_cancel.clusters.is_empty());

    let out_of_service_body = json!({"expected_revision": 1, "status": "out_of_service"});
    let cart_status_path = format!("/api/v1/pick-carts/{}/status-changes", cart.cart_id);
    let out_of_service = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &cart_status_path,
        Some("cluster-lifecycle-cart-oos"),
        Some(out_of_service_body.clone()),
    )
    .await;
    let out_of_service: PickCartResponse =
        response_json(expect_status(out_of_service, StatusCode::OK, "disable released cart").await)
            .await;
    assert_eq!(out_of_service.status, PickCartStatus::OutOfService);
    let status_replay = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &cart_status_path,
        Some("cluster-lifecycle-cart-oos"),
        Some(out_of_service_body),
    )
    .await;
    assert_eq!(
        response_json::<PickCartResponse>(status_replay).await,
        out_of_service
    );

    let unavailable_plan = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/pick-clusters",
        Some("cluster-lifecycle-plan-unavailable"),
        Some(plan_body.clone()),
    )
    .await;
    assert_eq!(unavailable_plan.status(), StatusCode::CONFLICT);
    let active = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &cart_status_path,
        Some("cluster-lifecycle-cart-active"),
        Some(json!({"expected_revision": 2, "status": "active"})),
    )
    .await;
    let active: PickCartResponse =
        response_json(expect_status(active, StatusCode::OK, "restore cluster cart").await).await;
    assert_eq!(active.status, PickCartStatus::Active);

    let second_plan = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        "/api/v1/pick-clusters",
        Some("cluster-lifecycle-plan-two"),
        Some(plan_body),
    )
    .await;
    let second_plan: PickClusterResponse = response_json(
        expect_status(second_plan, StatusCode::OK, "replan released cluster tasks").await,
    )
    .await;
    assert_ne!(second_plan.cluster_id, first_plan.cluster_id);
    let cancelled_again = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/pick-clusters/{}/cancellations",
            second_plan.cluster_id
        ),
        Some("cluster-lifecycle-cancel-two"),
        Some(json!({
            "expected_revision": second_plan.revision,
            "note": "Retire cart after inspection"
        })),
    )
    .await;
    expect_status(cancelled_again, StatusCode::OK, "cancel replanned route").await;
    let retired = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &cart_status_path,
        Some("cluster-lifecycle-cart-retired"),
        Some(json!({"expected_revision": 3, "status": "retired"})),
    )
    .await;
    let retired: PickCartResponse =
        response_json(expect_status(retired, StatusCode::OK, "retire released cart").await).await;
    assert_eq!(retired.status, PickCartStatus::Retired);
    assert_eq!(retired.revision, 4);
    let reactivate_retired = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &cart_status_path,
        Some("cluster-lifecycle-cart-reactivate-retired"),
        Some(json!({"expected_revision": 4, "status": "active"})),
    )
    .await;
    assert_eq!(reactivate_retired.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn database_rejects_carts_without_slots_and_clusters_without_members() {
    let fixture = Fixture::new().await;
    let supervisor = fixture.wms_user("pick-cluster-db@test.local").await;
    let access = default_tenant_for_user(&fixture.db, supervisor.id)
        .await
        .unwrap();
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Cluster DB Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Cluster DB Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;

    let mut empty_cart_tx = tenant_tx(&fixture.db, access.tenant_id).await;
    sqlx::query(
        r#"INSERT INTO pick_carts(
          tenant_id,facility_id,barcode,name,status,revision,created_by_user_id,created_at)
        VALUES($1,$2,'EMPTY-CART','Empty cart','active',1,$3,statement_timestamp())"#,
    )
    .bind(access.tenant_id.get())
    .bind(facility_id)
    .bind(supervisor.id)
    .execute(&mut *empty_cart_tx)
    .await
    .unwrap();
    let empty_cart_error = empty_cart_tx.commit().await.unwrap_err();
    assert_eq!(
        empty_cart_error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23514")
    );

    let mut cart_tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let cart_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO pick_carts(
          tenant_id,facility_id,barcode,name,status,revision,created_by_user_id,created_at)
        VALUES($1,$2,'VALID-CART','Valid cart','active',1,$3,statement_timestamp())
        RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(facility_id)
    .bind(supervisor.id)
    .fetch_one(&mut *cart_tx)
    .await
    .unwrap();
    for (code, sequence) in [("A", 1_i64), ("B", 2_i64)] {
        sqlx::query(
            r#"INSERT INTO pick_cart_slots(
              tenant_id,facility_id,cart_id,code,sequence,created_at)
            VALUES($1,$2,$3,$4,$5,statement_timestamp())"#,
        )
        .bind(access.tenant_id.get())
        .bind(facility_id)
        .bind(cart_id)
        .bind(code)
        .bind(sequence)
        .execute(&mut *cart_tx)
        .await
        .unwrap();
    }
    cart_tx.commit().await.unwrap();

    let mut empty_cluster_tx = tenant_tx(&fixture.db, access.tenant_id).await;
    sqlx::query(
        r#"INSERT INTO pick_clusters(
          tenant_id,inventory_owner_id,facility_id,cart_id,mode,status,revision,
          task_count,order_count,planned_by_user_id,planned_at)
        VALUES($1,$2,$3,$4,'cluster_cart','planned',1,2,2,$5,statement_timestamp())"#,
    )
    .bind(access.tenant_id.get())
    .bind(owner_id)
    .bind(facility_id)
    .bind(cart_id)
    .bind(supervisor.id)
    .execute(&mut *empty_cluster_tx)
    .await
    .unwrap();
    let empty_cluster_error = empty_cluster_tx.commit().await.unwrap_err();
    assert_eq!(
        empty_cluster_error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23514")
    );
}
