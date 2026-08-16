use super::*;
use wareboxes_api_contract::v1::{
    PickExecutionMethod, PickZoneWorkspaceResponse, StorageZoneResponse,
    PRODUCT_DEFAULT_PICK_DECISION_POLICY_HASH,
};

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
        &format!("zone-pick-supervisor-{user_id}"),
        Some("Zone-pick queue supervision"),
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

struct ZoneConfig<'a> {
    facility_id: i64,
    location_id: i64,
    travel_sequence: u32,
    expected_revision: Option<i64>,
    key: &'a str,
}

async fn configure_zone(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    config: ZoneConfig<'_>,
) -> StorageZoneResponse {
    let response = send(
        app,
        token,
        tenant_id,
        Method::POST,
        "/api/v1/storage-zones",
        Some(config.key),
        Some(json!({
            "facility_id": config.facility_id,
            "code": "PICK-A",
            "name": "Primary pick zone",
            "purpose": "pick",
            "travel_sequence": config.travel_sequence,
            "location_ids": [config.location_id],
            "expected_revision": config.expected_revision,
        })),
    )
    .await;
    let response = expect_status(response, StatusCode::OK, "configure pick zone").await;
    response_json(response).await
}

async fn claim_zone(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    zone_id: i64,
    key: &str,
) -> axum::response::Response {
    send(
        app,
        token,
        tenant_id,
        Method::POST,
        &format!("/api/v1/pick-zones/{zone_id}/claims/next"),
        Some(key),
        Some(json!({})),
    )
    .await
}

#[tokio::test]
async fn zone_queue_claims_are_scoped_concurrent_recoverable_and_frozen() {
    let fixture = Fixture::new().await;
    let supervisor = fixture.wms_user("zone-pick-supervisor@test.local").await;
    let access = default_tenant_for_user(&fixture.db, supervisor.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        supervisor.id,
        "zone-pick-orders",
    )
    .await;
    grant_supervisor(&fixture.db, access.tenant_id, supervisor.id).await;
    let second = add_wms_operator(
        &fixture,
        access.tenant_id,
        "zone-pick-second@test.local",
        "zone-pick-second-role",
    )
    .await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Zone Pick Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Zone Pick Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    set_scope(
        &fixture.db,
        access.tenant_id,
        supervisor.id,
        vec![facility_id],
        vec![owner_id],
    )
    .await;
    set_scope(
        &fixture.db,
        access.tenant_id,
        second.id,
        vec![facility_id],
        vec![owner_id],
    )
    .await;
    let destination_id =
        staging_location(&fixture, access.tenant_id, facility_id, "ZONE-STAGE").await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        destination_id,
        "ZONE-TOTE",
    )
    .await;
    let supervisor_token = auth::create_session(&fixture.db, supervisor.id)
        .await
        .unwrap();
    let second_token = auth::create_session(&fixture.db, second.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let order = allocated_order(
        &fixture,
        &app,
        &supervisor_token,
        &access,
        owner_id,
        facility_id,
        "ZONE-PICK",
        &[4],
        &[4],
    )
    .await;
    let released = release(
        &app,
        &supervisor_token,
        access.tenant_id,
        order.order_id,
        Some("zone-pick-release"),
        release_body(facility_id, destination_id, 2),
    )
    .await;
    expect_status(released, StatusCode::OK, "release zone pick order").await;
    let zone_v1 = configure_zone(
        &app,
        &supervisor_token,
        access.tenant_id,
        ZoneConfig {
            facility_id,
            location_id: order.source_location_ids[0],
            travel_sequence: 10,
            expected_revision: None,
            key: "zone-pick-config-v1",
        },
    )
    .await;

    let workspace = send(
        &app,
        &supervisor_token,
        access.tenant_id,
        Method::GET,
        &format!(
            "/api/v1/pick-zones/workspace?inventory_owner_id={owner_id}&facility_id={facility_id}"
        ),
        None,
        None,
    )
    .await;
    let workspace = expect_status(workspace, StatusCode::OK, "read zone workspace").await;
    let workspace: PickZoneWorkspaceResponse = response_json(workspace).await;
    assert_eq!(workspace.queues.len(), 1);
    assert_eq!(workspace.queues[0].storage_zone_id, zone_v1.storage_zone_id);
    assert_eq!(workspace.queues[0].open_task_count, 1);
    assert_eq!(workspace.queues[0].active_task_count, 0);

    let general = send(
        &app,
        &supervisor_token,
        access.tenant_id,
        Method::POST,
        "/api/v1/picking-claims/next",
        Some("zone-pick-general-bypass"),
        Some(json!({})),
    )
    .await;
    let general = expect_status(general, StatusCode::OK, "general queue skips zoned work").await;
    assert!(response_json::<Option<PickClaimResponse>>(general)
        .await
        .is_none());

    let mut task_tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let task_id: i64 =
        sqlx::query_scalar("SELECT id FROM pick_tasks WHERE tenant_id=$1 AND order_id=$2")
            .bind(access.tenant_id.get())
            .bind(order.order_id)
            .fetch_one(&mut *task_tx)
            .await
            .unwrap();
    task_tx.rollback().await.unwrap();
    let app_db = app_db_for(&fixture.db).await;
    let mut bypass = tenant_tx(&app_db, access.tenant_id).await;
    sqlx::query("SELECT set_config('wareboxes.actor_user_id',$1,true)")
        .bind(supervisor.id.to_string())
        .execute(&mut *bypass)
        .await
        .unwrap();
    sqlx::query(
        r#"UPDATE pick_tasks SET status='in_progress',assigned_user_id=$1,
        claimed_at=statement_timestamp(),lease_expires_at=statement_timestamp()+INTERVAL '15 minutes',
        pick_policy_source='product_default',require_source_location_scan=true,
        require_item_scan=true,require_destination_container_scan=true,pick_policy_hash=$2
        WHERE tenant_id=$3 AND id=$4"#,
    )
    .bind(supervisor.id)
    .bind(PRODUCT_DEFAULT_PICK_DECISION_POLICY_HASH)
    .bind(access.tenant_id.get())
    .bind(task_id)
    .execute(&mut *bypass)
    .await
    .unwrap();
    assert!(bypass.commit().await.is_err(), "raw zone bypass must fail");

    let first = claim_zone(
        &app,
        &supervisor_token,
        access.tenant_id,
        zone_v1.storage_zone_id,
        "zone-pick-race-a",
    );
    let second_claim = claim_zone(
        &app,
        &second_token,
        access.tenant_id,
        zone_v1.storage_zone_id,
        "zone-pick-race-b",
    );
    let (first, second_claim) = tokio::join!(first, second_claim);
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second_claim.status(), StatusCode::OK);
    let first: Option<PickClaimResponse> = response_json(first).await;
    let second_claim: Option<PickClaimResponse> = response_json(second_claim).await;
    let (winner_token, winner_key, winner, next_token) = match (first, second_claim) {
        (Some(claim), None) => (
            supervisor_token.as_str(),
            "zone-pick-race-a",
            claim,
            second_token.as_str(),
        ),
        (None, Some(claim)) => (
            second_token.as_str(),
            "zone-pick-race-b",
            claim,
            supervisor_token.as_str(),
        ),
        state => panic!("one zone claimant must win: {state:?}"),
    };
    assert_eq!(winner.execution.method, PickExecutionMethod::Zone);
    assert_eq!(
        winner.execution.storage_zone_id,
        Some(zone_v1.storage_zone_id)
    );
    assert_eq!(winner.execution.storage_zone_revision, Some(1));
    assert_eq!(winner.execution.storage_zone_travel_sequence, Some(10));

    let replay = claim_zone(
        &app,
        winner_token,
        access.tenant_id,
        zone_v1.storage_zone_id,
        winner_key,
    )
    .await;
    let replay = expect_status(replay, StatusCode::OK, "replay zone claim").await;
    assert_eq!(
        response_json::<Option<PickClaimResponse>>(replay).await,
        Some(winner.clone())
    );

    let active_reconfiguration = send(
        &app,
        &supervisor_token,
        access.tenant_id,
        Method::POST,
        "/api/v1/storage-zones",
        Some("zone-pick-active-reconfiguration"),
        Some(json!({
            "facility_id": facility_id,
            "code": "PICK-A",
            "name": "Primary pick zone",
            "purpose": "pick",
            "travel_sequence": 20,
            "location_ids": [order.source_location_ids[0]],
            "expected_revision": 1,
        })),
    )
    .await;
    assert_eq!(active_reconfiguration.status(), StatusCode::CONFLICT);

    let released = send(
        &app,
        winner_token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/picking-claims/{}/releases", winner.task_id),
        Some("zone-pick-handoff-release"),
        Some(json!({"reason":"work_interrupted","note":"Shift handoff"})),
    )
    .await;
    expect_status(released, StatusCode::OK, "release zone handoff").await;

    let zone_v2 = configure_zone(
        &app,
        &supervisor_token,
        access.tenant_id,
        ZoneConfig {
            facility_id,
            location_id: order.source_location_ids[0],
            travel_sequence: 20,
            expected_revision: Some(1),
            key: "zone-pick-config-v2",
        },
    )
    .await;
    assert_ne!(zone_v2.storage_zone_id, zone_v1.storage_zone_id);
    assert_eq!(zone_v2.revision.get(), 2);

    let stale_zone = claim_zone(
        &app,
        next_token,
        access.tenant_id,
        zone_v1.storage_zone_id,
        "zone-pick-stale-zone",
    )
    .await;
    assert_eq!(stale_zone.status(), StatusCode::NOT_FOUND);
    let changed_replay = claim_zone(
        &app,
        winner_token,
        access.tenant_id,
        zone_v2.storage_zone_id,
        winner_key,
    )
    .await;
    assert_eq!(changed_replay.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed_replay).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let next = claim_zone(
        &app,
        next_token,
        access.tenant_id,
        zone_v2.storage_zone_id,
        "zone-pick-handoff-claim",
    )
    .await;
    let next = expect_status(next, StatusCode::OK, "claim reconfigured zone").await;
    let next = response_json::<Option<PickClaimResponse>>(next)
        .await
        .expect("released task should be available for handoff");
    assert_eq!(next.execution.method, PickExecutionMethod::Zone);
    assert_eq!(
        next.execution.storage_zone_id,
        Some(zone_v2.storage_zone_id)
    );
    assert_eq!(next.execution.storage_zone_revision, Some(2));
    assert_eq!(next.execution.storage_zone_travel_sequence, Some(20));

    let confirmation = send(
        &app,
        next_token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/picking-tasks/{}/contents/{}/confirmations",
            next.task_id, next.content.content_id
        ),
        Some("zone-pick-confirm"),
        Some(json!({
            "source_location_barcode": next.content.source_location_barcode,
            "item_barcode": next.content.item_barcodes[0],
            "destination_license_plate_barcode": "ZONE-TOTE",
        })),
    )
    .await;
    expect_status(confirmation, StatusCode::OK, "confirm zone pick").await;

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let evidence: Vec<(i64, i64, i64, i64, String)> = sqlx::query_as(
        r#"SELECT id,storage_zone_id,storage_zone_revision,
        storage_zone_travel_sequence,storage_zone_code
        FROM pick_zone_claims WHERE tenant_id=$1 AND task_id=$2 ORDER BY claimed_at,id"#,
    )
    .bind(access.tenant_id.get())
    .bind(next.task_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(evidence.len(), 2);
    assert_eq!(evidence[0].1, zone_v1.storage_zone_id);
    assert_eq!(evidence[0].2, 1);
    assert_eq!(evidence[1].1, zone_v2.storage_zone_id);
    assert_eq!(evidence[1].2, 2);
    let methods: Vec<String> = sqlx::query_scalar(
        r#"SELECT payload->>'execution_method' FROM outbox_events
        WHERE tenant_id=$1 AND aggregate_type='pick_task'
          AND event_type='outbound.pick.confirmed' AND aggregate_id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(next.task_id.to_string())
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(methods, vec!["zone"]);
    let claim_events: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM outbox_events WHERE tenant_id=$1
        AND event_type='outbound.pick_zone.claimed' AND aggregate_type='pick_zone_claim'"#,
    )
    .bind(access.tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(claim_events, 2);
    tx.rollback().await.unwrap();

    let mut tamper = tenant_tx(&app_db, access.tenant_id).await;
    let tamper_result = sqlx::query(
        "UPDATE pick_zone_claims SET storage_zone_code=storage_zone_code WHERE tenant_id=$1",
    )
    .bind(access.tenant_id.get())
    .execute(&mut *tamper)
    .await;
    assert!(
        tamper_result.is_err(),
        "zone claim evidence must be immutable"
    );
    tamper.rollback().await.unwrap();
    app_db.close().await;

    set_scope(
        &fixture.db,
        access.tenant_id,
        supervisor.id,
        Vec::new(),
        Vec::new(),
    )
    .await;
    let concealed_workspace = send(
        &app,
        &supervisor_token,
        access.tenant_id,
        Method::GET,
        &format!(
            "/api/v1/pick-zones/workspace?inventory_owner_id={owner_id}&facility_id={facility_id}"
        ),
        None,
        None,
    )
    .await;
    assert_eq!(concealed_workspace.status(), StatusCode::NOT_FOUND);
    let concealed_claim = claim_zone(
        &app,
        &supervisor_token,
        access.tenant_id,
        zone_v2.storage_zone_id,
        "zone-pick-concealed",
    )
    .await;
    assert_eq!(concealed_claim.status(), StatusCode::NOT_FOUND);
}
