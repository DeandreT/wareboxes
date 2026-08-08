use super::*;

pub(super) fn init_test_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("wareboxes_api=debug")
        .with_test_writer()
        .try_init();
}

pub(super) struct InventorySnapshot {
    transactions: i64,
    entries: i64,
    balances: Vec<String>,
}

impl InventorySnapshot {
    pub(super) fn assert_unchanged(&self, current: &Self) {
        assert_eq!(current.transactions, self.transactions);
        assert_eq!(current.entries, self.entries);
        assert_eq!(current.balances, self.balances);
    }
}

pub(super) struct MultiShortageFixture {
    pub(super) app: axum::Router,
    pub(super) access: TenantAccess,
    pub(super) token: String,
    pub(super) reports: Vec<ReportPickShortageResponse>,
}

impl MultiShortageFixture {
    pub(super) async fn new(key: &str, line_quantities: &[(i64, i64)]) -> Self {
        let fixture = Fixture::new().await;
        let operator = fixture.wms_user(&format!("{key}@test.local")).await;
        let access = default_tenant_for_user(&fixture.db, operator.id)
            .await
            .unwrap();
        grant_permissions(
            &fixture.db,
            access.tenant_id,
            operator.id,
            key,
            &["orders", "wms_supervisor"],
        )
        .await;
        let inventory_owner_id = fixture
            .inventory_owner(access.tenant_id, &format!("{key} owner"))
            .await;
        let facility_id = fixture
            .facility(access.tenant_id, &format!("{key} facility"))
            .await;
        fixture
            .assign_owner_to_facility(access.tenant_id, inventory_owner_id, facility_id)
            .await;
        let destination_location_id = execution_location(
            &fixture,
            access.tenant_id,
            facility_id,
            &format!("{key}-PACK"),
        )
        .await;
        let destination_plate_barcode = format!("{key}-TOTE");
        plate_at(
            &fixture,
            access.tenant_id,
            inventory_owner_id,
            facility_id,
            destination_location_id,
            &destination_plate_barcode,
        )
        .await;
        let order_id = fixture
            .order_header(
                access.tenant_id,
                &format!("{key}-ORDER"),
                inventory_owner_id,
            )
            .await;
        for (index, (planned, _)) in line_quantities.iter().copied().enumerate() {
            let item_id = fixture
                .item(access.tenant_id, &format!("{key} item {index}"), "each")
                .await;
            wareboxes_api::repo::items::add_barcode(
                &fixture.db,
                access.tenant_id,
                item_id,
                &format!("{key}-ITEM-{index}"),
                "code128",
                None,
            )
            .await
            .unwrap();
            fixture
                .order_item(access.tenant_id, order_id, item_id, planned)
                .await;
            fixture
                .received_balance(
                    &access,
                    ReceivedBalanceSetup {
                        inventory_owner_id,
                        facility_id,
                        item_id,
                        qty: planned,
                        key: &format!("{key}-SOURCE-{index}"),
                    },
                )
                .await;
        }
        let token = auth::create_session(&fixture.db, operator.id)
            .await
            .unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));
        let allocated = send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &format!("/api/v1/orders/{order_id}/allocation-runs"),
            Some(&format!("{key}-allocate")),
            Some(json!({
                "facility_id": facility_id,
                "expected_revision": 1,
                "strategy": "fefo"
            })),
        )
        .await;
        expect_status(allocated, StatusCode::OK, "allocate multi-shortage order").await;
        let released = send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &format!("/api/v1/orders/{order_id}/releases"),
            Some(&format!("{key}-release")),
            Some(json!({
                "facility_id": facility_id,
                "destination_location_id": destination_location_id,
                "expected_revision": 2
            })),
        )
        .await;
        expect_status(released, StatusCode::OK, "release multi-shortage order").await;

        let mut reports = Vec::with_capacity(line_quantities.len());
        for (index, (planned, picked)) in line_quantities.iter().copied().enumerate() {
            assert!(planned > picked && picked >= 0);
            let claim = send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                "/api/v1/picking-claims/next",
                Some(&format!("{key}-claim-{index}")),
                Some(json!({})),
            )
            .await;
            let claim: PickClaimResponse = response_json::<Option<PickClaimResponse>>(
                expect_status(claim, StatusCode::OK, "claim multi-shortage pick").await,
            )
            .await
            .expect("fixture has another pick task");
            assert_eq!(claim.order_id, order_id);
            assert_eq!(claim.content.planned_quantity, planned);
            let outcome = if picked == 0 {
                json!({"kind": "no_pick"})
            } else {
                json!({
                    "kind": "partial",
                    "picked_quantity": picked,
                    "destination_license_plate_barcode": destination_plate_barcode
                })
            };
            let report = send(
                &app,
                &token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/picking-tasks/{}/contents/{}/short-picks",
                    claim.task_id, claim.content.content_id
                ),
                Some(&format!("{key}-report-{index}")),
                Some(json!({
                    "source_location_barcode": claim.content.source_location_barcode,
                    "source_license_plate_barcode": claim.content.source_license_plate_barcode,
                    "observed_item_barcode": claim.content.item_barcodes[0],
                    "observed_lot": claim.content.lot,
                    "observed_serial": claim.content.serial,
                    "outcome": outcome,
                    "details": {"reason": "insufficient_quantity"}
                })),
            )
            .await;
            reports.push(
                response_json(
                    expect_status(report, StatusCode::OK, "report sibling shortage").await,
                )
                .await,
            );
        }

        Self {
            app,
            access,
            token,
            reports,
        }
    }

    pub(super) async fn accept(
        &self,
        shortage_id: i64,
        key: &str,
        body: Value,
    ) -> axum::response::Response {
        send(
            &self.app,
            &self.token,
            self.access.tenant_id,
            Method::POST,
            &short_ship_path(shortage_id),
            Some(key),
            Some(body),
        )
        .await
    }
}

pub(super) fn short_ship_path(shortage_id: i64) -> String {
    format!("/api/v1/pick-shortages/{shortage_id}/short-ship-dispositions")
}

pub(super) fn short_ship_body(shortage_revision: i64, order_revision: i64) -> Value {
    json!({
        "expected_shortage_revision": shortage_revision,
        "expected_order_revision": order_revision,
        "reason": "inventory_unavailable",
        "note": "Inventory unavailable before the shipping commitment"
    })
}

pub(super) async fn accept_short_ship(
    fixture: &PickShortageFixture,
    shortage_id: i64,
    key: Option<&str>,
    body: Value,
) -> axum::response::Response {
    fixture
        .request(Method::POST, &short_ship_path(shortage_id), key, Some(body))
        .await
}

pub(super) async fn inventory_snapshot(fixture: &PickShortageFixture) -> InventorySnapshot {
    let mut tx = tenant_tx(&fixture.fixture.db, fixture.access.tenant_id).await;
    let transactions = sqlx::query_scalar(
        "SELECT COUNT(*) FROM inventory_transactions WHERE tenant_id = $1 AND inventory_owner_id = $2",
    )
    .bind(fixture.access.tenant_id.get())
    .bind(fixture.inventory_owner_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let entries = sqlx::query_scalar(
        "SELECT COUNT(*) FROM inventory_entries WHERE tenant_id = $1 AND inventory_owner_id = $2 AND facility_id = $3",
    )
    .bind(fixture.access.tenant_id.get())
    .bind(fixture.inventory_owner_id)
    .bind(fixture.facility_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let balances = sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
                   'id', id,
                   'location_id', location_id,
                   'qty_on_hand', qty_on_hand,
                   'qty_reserved', qty_reserved,
                   'qty_held', qty_held,
                   'license_plate_id', license_plate_id,
                   'modified', modified,
                   'is_deleted', deleted IS NOT NULL
               )::TEXT
        FROM inventory_balances
        WHERE tenant_id = $1 AND inventory_owner_id = $2 AND facility_id = $3
        ORDER BY id
        "#,
    )
    .bind(fixture.access.tenant_id.get())
    .bind(fixture.inventory_owner_id)
    .bind(fixture.facility_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    InventorySnapshot {
        transactions,
        entries,
        balances,
    }
}

pub(super) async fn disposition_count(fixture: &PickShortageFixture) -> i64 {
    let mut tx = tenant_tx(&fixture.fixture.db, fixture.access.tenant_id).await;
    let count = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pick_short_ship_dispositions WHERE tenant_id = $1 AND order_id = $2",
    )
    .bind(fixture.access.tenant_id.get())
    .bind(fixture.order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    count
}

pub(super) async fn short_ship_effect_counts(
    fixture: &PickShortageFixture,
) -> (i64, i64, i64, i64) {
    let mut tx = tenant_tx(&fixture.fixture.db, fixture.access.tenant_id).await;
    let counts = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM pick_short_ship_dispositions
                WHERE tenant_id = $1 AND order_id = $2),
               (SELECT COUNT(*) FROM command_idempotency_records
                WHERE tenant_id = $1
                  AND operation = 'picking.shortage.accept_short_ship.v1'),
               (SELECT COUNT(*) FROM outbox_events
                WHERE tenant_id = $1 AND ordering_key = 'order:' || $2::TEXT
                  AND event_type = 'outbound.pick.shortage_short_ship_accepted'),
               (SELECT COUNT(*) FROM order_activity
                WHERE tenant_id = $1 AND order_id = $2
                  AND action LIKE 'accepted % as a short shipment for pick shortage %')
        "#,
    )
    .bind(fixture.access.tenant_id.get())
    .bind(fixture.order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    counts
}

pub(super) async fn assert_disposition_evidence(
    fixture: &PickShortageFixture,
    report: &ReportPickShortageResponse,
    accepted: &AcceptPickShortageAsShortShipResponse,
) {
    let mut tx = tenant_tx(&fixture.fixture.db, fixture.access.tenant_id).await;
    let row = sqlx::query(
        r#"
        SELECT disposition.inventory_owner_id, disposition.facility_id,
               disposition.order_id, disposition.order_item_id,
               disposition.reservation_id, disposition.pick_shortage_id,
               disposition.accepted_short_qty, disposition.reason_code,
               disposition.note, disposition.expected_shortage_revision,
               disposition.resulting_shortage_revision,
               disposition.expected_order_revision,
               disposition.resulting_order_revision,
               disposition.disposed_by_user_id, disposition.disposed_at,
               (SELECT COUNT(*) FROM command_idempotency_records command
                WHERE command.tenant_id = disposition.tenant_id
                  AND command.operation = 'picking.shortage.accept_short_ship.v1'
                  AND (command.result_json->>'disposition_id')::BIGINT = disposition.id)
                   AS command_count,
               (SELECT COUNT(*) FROM command_idempotency_records command
                WHERE command.tenant_id = disposition.tenant_id
                  AND command.operation = 'picking.shortage.accept_short_ship.v1'
                  AND command.inventory_transaction_id IS NOT NULL)
                   AS command_inventory_count,
               (SELECT COUNT(*) FROM order_activity activity
                WHERE activity.tenant_id = disposition.tenant_id
                  AND activity.order_id = disposition.order_id
                  AND activity.action =
                      'accepted ' || disposition.accepted_short_qty::TEXT ||
                      ' unit(s) as a short shipment for pick shortage ' ||
                      disposition.pick_shortage_id::TEXT) AS activity_count
        FROM pick_short_ship_dispositions disposition
        WHERE disposition.tenant_id = $1 AND disposition.id = $2
        "#,
    )
    .bind(fixture.access.tenant_id.get())
    .bind(accepted.disposition_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let events = sqlx::query(
        r#"
        SELECT event_key, aggregate_type, aggregate_id, ordering_key,
               aggregate_sequence, schema_version, payload,
               inventory_owner_id, facility_id, actor_user_id
        FROM outbox_events
        WHERE tenant_id = $1
          AND event_type = 'outbound.pick.shortage_short_ship_accepted'
          AND aggregate_id = $2
        ORDER BY aggregate_sequence
        "#,
    )
    .bind(fixture.access.tenant_id.get())
    .bind(report.shortage_id.to_string())
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();

    assert_eq!(
        row.get::<i64, _>("inventory_owner_id"),
        fixture.inventory_owner_id
    );
    assert_eq!(row.get::<i64, _>("facility_id"), fixture.facility_id);
    assert_eq!(row.get::<i64, _>("order_id"), fixture.order_id);
    assert_eq!(row.get::<i64, _>("pick_shortage_id"), report.shortage_id);
    assert_eq!(
        row.get::<i64, _>("accepted_short_qty"),
        accepted.accepted_short_quantity
    );
    assert_eq!(row.get::<String, _>("reason_code"), "inventory_unavailable");
    assert_eq!(row.get::<Option<String>, _>("note"), accepted.note.clone());
    assert_eq!(
        row.get::<i64, _>("expected_shortage_revision"),
        report.shortage_revision.get()
    );
    assert_eq!(
        row.get::<i64, _>("resulting_shortage_revision"),
        accepted.shortage_revision.get()
    );
    assert_eq!(
        row.get::<i64, _>("expected_order_revision"),
        report.order_revision.get()
    );
    assert_eq!(
        row.get::<i64, _>("resulting_order_revision"),
        accepted.order_revision.get()
    );
    assert_eq!(
        row.get::<i64, _>("disposed_by_user_id"),
        accepted.resolved_by
    );
    assert_eq!(
        row.get::<wareboxes_domain::Timestamp, _>("disposed_at")
            .timestamp_micros(),
        accepted
            .resolved_at
            .parse::<wareboxes_domain::Timestamp>()
            .unwrap()
            .timestamp_micros()
    );
    assert!(row.get::<i64, _>("order_item_id") > 0);
    assert!(row.get::<i64, _>("reservation_id") > 0);
    assert_eq!(row.get::<i64, _>("command_count"), 1);
    assert_eq!(row.get::<i64, _>("command_inventory_count"), 0);
    assert_eq!(row.get::<i64, _>("activity_count"), 1);

    assert_eq!(events.len(), 1, "short-ship event must be replay safe");
    let event = &events[0];
    assert_eq!(
        event.get::<String, _>("event_key"),
        format!(
            "pick-shortage:{}:short-ship:{}",
            report.shortage_id,
            accepted.shortage_revision.get()
        )
    );
    assert_eq!(event.get::<String, _>("aggregate_type"), "pick_shortage");
    assert_eq!(
        event.get::<String, _>("aggregate_id"),
        report.shortage_id.to_string()
    );
    assert_eq!(
        event.get::<String, _>("ordering_key"),
        format!("order:{}", fixture.order_id)
    );
    assert!(event.get::<i64, _>("aggregate_sequence") > 1);
    assert_eq!(event.get::<i32, _>("schema_version"), 1);
    assert_eq!(
        event.get::<Option<i64>, _>("inventory_owner_id"),
        Some(fixture.inventory_owner_id)
    );
    assert_eq!(
        event.get::<Option<i64>, _>("facility_id"),
        Some(fixture.facility_id)
    );
    assert_eq!(
        event.get::<Option<i64>, _>("actor_user_id"),
        Some(accepted.resolved_by)
    );
    assert_eq!(
        event.get::<Value, _>("payload"),
        json!({
            "disposition_id": accepted.disposition_id,
            "pick_shortage_id": accepted.shortage_id,
            "shortage_revision": accepted.shortage_revision,
            "shortage_resolution": accepted.shortage_resolution,
            "order_id": accepted.order_id,
            "order_line_id": accepted.order_line_id,
            "order_status": accepted.order_status,
            "order_revision": accepted.order_revision,
            "order_ready_to_pack": accepted.order_ready_to_pack,
            "accepted_short_quantity": accepted.accepted_short_quantity,
            "line_demand": accepted.line_demand,
            "order_demand": accepted.order_demand,
            "inventory_hold_id": accepted.inventory_hold_id,
            "reason": accepted.reason,
            "note": accepted.note,
            "resolved_by": accepted.resolved_by,
            "resolved_at": accepted
                .resolved_at
                .parse::<wareboxes_domain::Timestamp>()
                .unwrap(),
        })
    );
}

pub(super) async fn configure_shipping_origin(fixture: &PickShortageFixture, key: &str) {
    grant_permission(
        &fixture.fixture.db,
        fixture.access.tenant_id,
        fixture.access.user_id.get(),
        &format!("{key}-admin"),
        "admin",
    )
    .await;
    let response = fixture
        .request(
            Method::POST,
            &format!(
                "/api/v1/facilities/{}/shipping-origin-configurations",
                fixture.facility_id
            ),
            Some(key),
            Some(json!({
                "expected_revision": 1,
                "name": "Outbound office",
                "company": "Wareboxes Test Facility",
                "line1": "100 Distribution Way",
                "line2": "Dock 4",
                "city": "Reno",
                "state": "NV",
                "postal_code": "89502",
                "country": "US",
                "phone": "+1 775 555 0100",
                "email": "shipping@test.local"
            })),
        )
        .await;
    expect_status(response, StatusCode::OK, "configure shipping origin").await;
}

pub(super) async fn pack_manifest_and_depart(
    fixture: &PickShortageFixture,
    accepted: &AcceptPickShortageAsShortShipResponse,
    key: &str,
) -> ConfirmShipmentDepartureResponse {
    let opened = fixture
        .open_packing_session(accepted.order_revision.get(), &format!("{key}-open-pack"))
        .await;
    let opened: OpenPackSessionResponse =
        response_json(expect_status(opened, StatusCode::OK, "open reduced packing session").await)
            .await;
    assert_eq!(opened.session.allocations.len(), 1);
    assert_eq!(
        opened.session.progress.expected_quantity,
        accepted.order_demand.effective
    );
    let during_packing = accept_short_ship(
        fixture,
        accepted.shortage_id,
        Some(&format!("{key}-reject-during-packing")),
        short_ship_body(
            accepted.shortage_revision.get(),
            opened.session.revision.get(),
        ),
    )
    .await;
    assert_eq!(during_packing.status(), StatusCode::CONFLICT);
    let allocation = &opened.session.allocations[0];
    let carton_barcode = format!("{key}-CARTON");
    let created = fixture
        .request(
            Method::POST,
            &format!(
                "/api/v1/packing-sessions/{}/cartons",
                opened.session.session_id
            ),
            Some(&format!("{key}-carton")),
            Some(json!({
                "carton_barcode": carton_barcode,
                "expected_revision": opened.session.revision
            })),
        )
        .await;
    let created: CreateCartonResponse = response_json(
        expect_status(created, StatusCode::OK, "create reduced shipment carton").await,
    )
    .await;
    let packed = fixture
        .request(
            Method::POST,
            &format!(
                "/api/v1/packing-sessions/{}/cartons/{}/contents",
                opened.session.session_id, created.carton.carton_id
            ),
            Some(&format!("{key}-pack")),
            Some(json!({
                "inventory_allocation_id": allocation.inventory_allocation_id,
                "item_barcode": allocation.item_barcodes[0],
                "lot_scan": allocation.lot.as_deref().unwrap(),
                "source_license_plate_barcode": fixture.destination_plate_barcode,
                "carton_barcode": carton_barcode,
                "expected_revision": created.revision
            })),
        )
        .await;
    let packed: PackPickedAllocationResponse =
        response_json(expect_status(packed, StatusCode::OK, "pack reduced shipment content").await)
            .await;
    let closed = fixture
        .request(
            Method::POST,
            &format!(
                "/api/v1/packing-sessions/{}/cartons/{}/closures",
                opened.session.session_id, created.carton.carton_id
            ),
            Some(&format!("{key}-close")),
            Some(json!({
                "carton_barcode": carton_barcode,
                "measurements": {
                    "weight_grams": 1250,
                    "dimensions": {"length_mm": 300, "width_mm": 200, "height_mm": 150}
                },
                "expected_revision": packed.revision
            })),
        )
        .await;
    let closed: CloseCartonResponse =
        response_json(expect_status(closed, StatusCode::OK, "close reduced shipment carton").await)
            .await;
    assert!(closed.ready_to_manifest);

    configure_shipping_origin(fixture, &format!("{key}-origin")).await;
    let shipment = fixture
        .request(
            Method::POST,
            &format!("/api/v1/orders/{}/shipments", fixture.order_id),
            Some(&format!("{key}-shipment")),
            Some(json!({
                "packing_session_id": opened.session.session_id,
                "expected_revision": closed.revision
            })),
        )
        .await;
    let shipment: CreateShipmentResponse =
        response_json(expect_status(shipment, StatusCode::OK, "create reduced shipment").await)
            .await;
    assert_eq!(shipment.shipment.cartons.len(), 1);
    assert_eq!(
        shipment.shipment.cartons[0].packed_quantity,
        accepted.order_demand.effective
    );
    assert_eq!(
        shipment.shipment.demand.ordered_quantity,
        accepted.order_demand.ordered
    );
    assert_eq!(
        shipment.shipment.demand.shipped_quantity,
        accepted.order_demand.effective
    );
    assert_eq!(
        shipment.shipment.demand.accepted_short_quantity,
        accepted.order_demand.accepted_short
    );

    let after_shipment = accept_short_ship(
        fixture,
        accepted.shortage_id,
        Some(&format!("{key}-reject-after-shipment")),
        short_ship_body(
            accepted.shortage_revision.get(),
            shipment.order_revision.get(),
        ),
    )
    .await;
    assert_eq!(after_shipment.status(), StatusCode::CONFLICT);

    let manifest = fixture
        .request(
            Method::POST,
            &format!(
                "/api/v1/shipments/{}/manifests",
                shipment.shipment.shipment_id
            ),
            Some(&format!("{key}-manifest")),
            Some(json!({
                "carrier_code": "TEST",
                "service_code": "GROUND",
                "manifest_reference": format!("{key}-MANIFEST"),
                "carton_tracking_assignments": [{
                    "carton_id": created.carton.carton_id,
                    "tracking_number": format!("{key}-TRACKING")
                }],
                "expected_revision": shipment.shipment.revision
            })),
        )
        .await;
    let manifest: RecordManualManifestResponse =
        response_json(expect_status(manifest, StatusCode::OK, "manifest reduced shipment").await)
            .await;
    let departed = fixture
        .request(
            Method::POST,
            &format!(
                "/api/v1/shipments/{}/departures",
                shipment.shipment.shipment_id
            ),
            Some(&format!("{key}-depart")),
            Some(json!({
                "scanned_carton_barcodes": [carton_barcode],
                "expected_shipment_revision": manifest.revision,
                "expected_order_revision": shipment.order_revision
            })),
        )
        .await;
    response_json(expect_status(departed, StatusCode::OK, "depart reduced shipment").await).await
}

pub(super) async fn grant_permission(
    db: &wareboxes_persistence_postgres::db::Db,
    tenant_id: TenantId,
    user_id: i64,
    role_name: &str,
    permission_name: &str,
) {
    let permission = match wareboxes_persistence_postgres::permissions::find_by_name(
        db,
        tenant_id,
        permission_name,
    )
    .await
    .unwrap()
    {
        Some(permission) => permission.id,
        None => wareboxes_persistence_postgres::permissions::add_permission(
            db,
            tenant_id,
            permission_name,
            Some(permission_name),
        )
        .await
        .unwrap(),
    };
    let role =
        wareboxes_persistence_postgres::roles::add_role(db, tenant_id, role_name, Some(role_name))
            .await
            .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        db, tenant_id, role, permission,
    )
    .await
    .unwrap());
    assert!(
        wareboxes_persistence_postgres::roles::add_role_to_user(db, tenant_id, user_id, role,)
            .await
            .unwrap()
    );
}

async fn grant_permissions(
    db: &wareboxes_persistence_postgres::db::Db,
    tenant_id: TenantId,
    user_id: i64,
    role_prefix: &str,
    permission_names: &[&str],
) {
    for permission in permission_names {
        grant_permission(
            db,
            tenant_id,
            user_id,
            &format!("{role_prefix}-{permission}"),
            permission,
        )
        .await;
    }
}

async fn execution_location(
    fixture: &Fixture,
    tenant_id: TenantId,
    facility_id: i64,
    barcode: &str,
) -> i64 {
    wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some(barcode),
        Some(barcode),
        "packing",
        true,
        false,
        false,
    )
    .await
    .unwrap()
}

async fn plate_at(
    fixture: &Fixture,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
    location_id: i64,
    barcode: &str,
) {
    let plate_id = wareboxes_api::repo::license_plates::add_license_plate(
        &fixture.db,
        tenant_id,
        inventory_owner_id,
        facility_id,
        Some(barcode),
    )
    .await
    .unwrap();
    let admin = admin_db_for(&fixture.db).await;
    sqlx::query("UPDATE license_plates SET location_id = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(location_id)
        .bind(tenant_id.get())
        .bind(plate_id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
