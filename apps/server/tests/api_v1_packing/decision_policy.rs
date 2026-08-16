use super::super::*;
use wareboxes_api_contract::v1::{
    ConfigurationLifecycleRequest, ConfigurationResponse, ConfigurationScope,
    CreateConfigurationRequest, DecisionRule, PackDecisionPolicySource,
};

async fn grant_permission(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    role_name: &str,
    permission_name: &str,
) {
    let permission_id = match wareboxes_persistence_postgres::permissions::find_by_name(
        &fixture.db,
        tenant_id,
        permission_name,
    )
    .await
    .unwrap()
    {
        Some(permission) => permission.id,
        None => wareboxes_persistence_postgres::permissions::add_permission(
            &fixture.db,
            tenant_id,
            permission_name,
            Some("Pack policy acceptance permission"),
        )
        .await
        .unwrap(),
    };
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        role_name,
        Some("Pack policy acceptance role"),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        permission_id,
    )
    .await
    .unwrap());
    assert!(wareboxes_persistence_postgres::roles::add_role_to_user(
        &fixture.db,
        tenant_id,
        user_id,
        role,
    )
    .await
    .unwrap());
}

async fn add_admin_approver(fixture: &Fixture, tenant_id: TenantId) -> String {
    let user = fixture.user("pack-policy-approver@test.local").await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships(tenant_id,user_id) VALUES ($1,$2)")
        .bind(tenant_id.get())
        .bind(user.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    grant_permission(fixture, tenant_id, user.id, "pack-policy-approver", "admin").await;
    auth::create_session(&fixture.db, user.id).await.unwrap()
}

async fn transition(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    configuration: &ConfigurationResponse,
    transition: &str,
    key: &str,
) -> ConfigurationResponse {
    let response = send(
        app,
        token,
        tenant_id,
        Method::POST,
        &format!(
            "/api/v1/configurations/{}/{transition}",
            configuration.configuration_id
        ),
        Some(key),
        Some(
            serde_json::to_value(ConfigurationLifecycleRequest {
                expected_revision: configuration.revision,
            })
            .unwrap(),
        ),
    )
    .await;
    response_json(expect_status(response, StatusCode::OK, transition).await).await
}

async fn activate_pack_policy(
    app: &axum::Router,
    creator_token: &str,
    approver_token: &str,
    tenant_id: TenantId,
    owner_id: i64,
    facility_id: i64,
) -> ConfigurationResponse {
    let response = send(
        app,
        creator_token,
        tenant_id,
        Method::POST,
        "/api/v1/configurations",
        Some("pack-policy-create"),
        Some(
            serde_json::to_value(CreateConfigurationRequest {
                scope: ConfigurationScope::OwnerFacility {
                    inventory_owner_id: owner_id,
                    facility_id,
                },
                effective_from: "2026-01-01T00:00:00Z".into(),
                effective_until: None,
                rule: DecisionRule::Pack {
                    require_station_scan: true,
                    require_weight: true,
                    allow_mixed_orders: false,
                },
                expected_revision: None,
            })
            .unwrap(),
        ),
    )
    .await;
    let created: ConfigurationResponse =
        response_json(expect_status(response, StatusCode::OK, "create Pack policy").await).await;
    let submitted = transition(
        app,
        creator_token,
        tenant_id,
        &created,
        "submissions",
        "pack-policy-submit",
    )
    .await;
    let approved = transition(
        app,
        approver_token,
        tenant_id,
        &submitted,
        "approvals",
        "pack-policy-approve",
    )
    .await;
    transition(
        app,
        creator_token,
        tenant_id,
        &approved,
        "activations",
        "pack-policy-activate",
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn ready_order(
    fixture: &Fixture,
    app: &axum::Router,
    token: &str,
    access: &wareboxes_core::models::TenantAccess,
    owner_id: i64,
    facility_id: i64,
    station_id: i64,
    key: &str,
    tote_barcode: &str,
) -> PreparedOrder {
    plate_at(
        fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        station_id,
        tote_barcode,
    )
    .await;
    let order = prepare_order(
        fixture,
        app,
        token,
        access,
        owner_id,
        facility_id,
        key,
        &[2],
    )
    .await;
    release_order(
        app,
        token,
        access.tenant_id,
        order.order_id,
        facility_id,
        station_id,
        &format!("{key}-release"),
    )
    .await;
    pick_order(app, token, access.tenant_id, tote_barcode, 1, key).await;
    order
}

#[tokio::test]
async fn effective_pack_policy_controls_station_weight_and_shared_station_admission() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("pack-policy-operator@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(
        &fixture.db,
        access.tenant_id,
        operator.id,
        "pack-policy-orders",
    )
    .await;
    grant_permission(
        &fixture,
        access.tenant_id,
        operator.id,
        "pack-policy-creator",
        "admin",
    )
    .await;
    let approver_token = add_admin_approver(&fixture, access.tenant_id).await;
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Pack Policy Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Pack Policy Facility")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let station_id = execution_location(
        &fixture,
        access.tenant_id,
        facility_id,
        "PACK-POLICY-STATION",
        "packing",
    )
    .await;
    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let configuration = activate_pack_policy(
        &app,
        &token,
        &approver_token,
        access.tenant_id,
        owner_id,
        facility_id,
    )
    .await;
    let first = ready_order(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        station_id,
        "PACK-POLICY-FIRST",
        "PACK-POLICY-TOTE-1",
    )
    .await;
    let second = ready_order(
        &fixture,
        &app,
        &token,
        &access,
        owner_id,
        facility_id,
        station_id,
        "PACK-POLICY-SECOND",
        "PACK-POLICY-TOTE-2",
    )
    .await;

    for (key, station_scan) in [
        ("pack-policy-no-station", None),
        ("pack-policy-wrong-station", Some("WRONG-STATION")),
    ] {
        let response = send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &format!("/api/v1/orders/{}/packing-sessions", first.order_id),
            Some(key),
            Some(json!({
                "facility_id": facility_id,
                "station_location_id": station_id,
                "station_location_barcode": station_scan,
                "expected_revision": 4
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{key}");
    }

    let open_body = json!({
        "facility_id": facility_id,
        "station_location_id": station_id,
        "station_location_barcode": "PACK-POLICY-STATION",
        "expected_revision": 4
    });
    let opened: OpenPackSessionResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/orders/{}/packing-sessions", first.order_id),
                Some("pack-policy-open"),
                Some(open_body.clone()),
            )
            .await,
            StatusCode::OK,
            "open policy-controlled session",
        )
        .await,
    )
    .await;
    assert_eq!(
        opened.session.pack_policy.source,
        PackDecisionPolicySource::Configuration
    );
    assert_eq!(
        opened.session.pack_policy.configuration_id,
        Some(configuration.configuration_id)
    );
    assert!(opened.session.pack_policy.require_station_scan);
    assert!(opened.session.pack_policy.require_weight);
    assert!(!opened.session.pack_policy.allow_mixed_orders);
    assert!(opened.session.station_scan_verified);

    let replay: OpenPackSessionResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/orders/{}/packing-sessions", first.order_id),
                Some("pack-policy-open"),
                Some(open_body),
            )
            .await,
            StatusCode::OK,
            "replay policy-controlled session",
        )
        .await,
    )
    .await;
    assert_eq!(replay, opened);

    let occupied = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{}/packing-sessions", second.order_id),
        Some("pack-policy-second-open"),
        Some(json!({
            "facility_id": facility_id,
            "station_location_id": station_id,
            "station_location_barcode": "PACK-POLICY-STATION",
            "expected_revision": 4
        })),
    )
    .await;
    assert_eq!(occupied.status(), StatusCode::CONFLICT);

    let carton = create_carton(
        &app,
        &token,
        access.tenant_id,
        opened.session.session_id,
        "PACK-POLICY-CARTON",
        5,
        "pack-policy-carton",
    )
    .await;
    let allocation = &opened.session.allocations[0];
    let packed = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/packing-sessions/{}/cartons/{}/contents",
            opened.session.session_id, carton.carton.carton_id
        ),
        Some("pack-policy-pack"),
        Some(controlled_pack_body(
            allocation.inventory_allocation_id,
            &allocation.item_barcodes[0],
            allocation.lot.as_deref(),
            allocation.serial.as_deref(),
            "PACK-POLICY-TOTE-1",
            "PACK-POLICY-CARTON",
            6,
        )),
    )
    .await;
    let packed: PackPickedAllocationResponse =
        response_json(expect_status(packed, StatusCode::OK, "pack policy carton content").await)
            .await;

    let mut raw_tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let forged_close = sqlx::query(
        r#"UPDATE cartons SET state='closed',closed_by_user_id=$1,
           closed_at=clock_timestamp() WHERE tenant_id=$2 AND id=$3"#,
    )
    .bind(operator.id)
    .bind(access.tenant_id.get())
    .bind(carton.carton.carton_id)
    .execute(&mut *raw_tx)
    .await;
    assert!(forged_close.is_err());
    raw_tx.rollback().await.unwrap();

    let missing_weight = send(
        &app,
        &token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/packing-sessions/{}/cartons/{}/closures",
            opened.session.session_id, carton.carton.carton_id
        ),
        Some("pack-policy-close-no-weight"),
        Some(json!({
            "carton_barcode": "PACK-POLICY-CARTON",
            "measurements": {},
            "expected_revision": packed.revision
        })),
    )
    .await;
    assert_eq!(missing_weight.status(), StatusCode::BAD_REQUEST);

    let closed: CloseCartonResponse = response_json(
        expect_status(
            send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/packing-sessions/{}/cartons/{}/closures",
                    opened.session.session_id, carton.carton.carton_id
                ),
                Some("pack-policy-close"),
                Some(json!({
                    "carton_barcode": "PACK-POLICY-CARTON",
                    "measurements": {"weight_grams": 900},
                    "expected_revision": packed.revision
                })),
            )
            .await,
            StatusCode::OK,
            "close weighted policy carton",
        )
        .await,
    )
    .await;
    assert_eq!(closed.pack_policy, opened.session.pack_policy);

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let evidence: (i64, i64, bool, i64) = sqlx::query_as(
        r#"SELECT pack_configuration_id,pack_configuration_revision,station_scan_verified,
          (SELECT COUNT(*) FROM outbox_events event
           WHERE event.tenant_id=$1 AND event.event_type='packing.session_opened'
             AND event.payload->'pack_policy'->>'configuration_id'=$3::text)
        FROM packing_sessions WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(opened.session.session_id)
    .bind(configuration.configuration_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        evidence,
        (
            configuration.configuration_id,
            configuration.revision.get(),
            true,
            1
        )
    );
    let tamper = sqlx::query(
        "UPDATE packing_sessions SET require_weight=false WHERE tenant_id=$1 AND id=$2",
    )
    .bind(access.tenant_id.get())
    .bind(opened.session.session_id)
    .execute(&mut *tx)
    .await;
    assert!(tamper.is_err());
    tx.rollback().await.unwrap();
}
