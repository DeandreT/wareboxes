use super::*;

#[derive(Debug)]
pub(super) struct ReadyShipment {
    pub(super) order_id: i64,
    pub(super) packing_session_id: i64,
    pub(super) order_revision: i64,
    pub(super) carton_ids: Vec<i64>,
    pub(super) carton_barcodes: Vec<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct CartonSnapshotRow {
    carton_id: i64,
    carton_barcode: String,
    content_count: i64,
    packed_qty: i64,
    weight_g: Option<i64>,
    length_mm: Option<i64>,
    width_mm: Option<i64>,
    height_mm: Option<i64>,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn prepare_ready_shipment(
    fixture: &Fixture,
    app: &axum::Router,
    token: &str,
    access: &wareboxes_core::models::TenantAccess,
    inventory_owner_id: i64,
    facility_id: i64,
    station_id: i64,
    key: &str,
) -> ReadyShipment {
    let order_id = fixture
        .order_header(access.tenant_id, key, inventory_owner_id)
        .await;
    for (index, quantity) in [3_i64, 2].into_iter().enumerate() {
        let item_id = fixture
            .item(access.tenant_id, &format!("{key} item {index}"), "each")
            .await;
        repo::items::add_barcode(
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
            .order_item(access.tenant_id, order_id, item_id, quantity)
            .await;
        fixture
            .received_balance(
                access,
                ReceivedBalanceSetup {
                    inventory_owner_id,
                    facility_id,
                    item_id,
                    qty: quantity + 2,
                    key: &format!("{key}-SOURCE-{index}"),
                },
            )
            .await;
    }

    let allocated = send(
        app,
        token,
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
    expect_status(allocated, StatusCode::OK, "allocate shipping order").await;
    let released = send(
        app,
        token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{order_id}/releases"),
        Some(&format!("{key}-release")),
        Some(json!({
            "facility_id": facility_id,
            "destination_location_id": station_id,
            "expected_revision": 2
        })),
    )
    .await;
    expect_status(released, StatusCode::OK, "release shipping order").await;

    let tote_barcode = format!("{key}-TOTE");
    for index in 0..2 {
        let claim = send(
            app,
            token,
            access.tenant_id,
            Method::POST,
            "/api/v1/picking-claims/next",
            Some(&format!("{key}-claim-{index}")),
            Some(json!({})),
        )
        .await;
        let claim: PickClaimResponse = response_json::<Option<PickClaimResponse>>(
            expect_status(claim, StatusCode::OK, "claim shipping pick").await,
        )
        .await
        .expect("released order has pick work");
        let confirmed = send(
            app,
            token,
            access.tenant_id,
            Method::POST,
            &format!(
                "/api/v1/picking-tasks/{}/contents/{}/confirmations",
                claim.task_id, claim.content.content_id
            ),
            Some(&format!("{key}-pick-{index}")),
            Some(json!({
                "source_location_barcode": claim.content.source_location_barcode,
                "item_barcode": claim.content.item_barcodes[0],
                "destination_license_plate_barcode": tote_barcode
            })),
        )
        .await;
        let _: PickContentConfirmationResponse =
            response_json(expect_status(confirmed, StatusCode::OK, "confirm shipping pick").await)
                .await;
    }

    let opened = send(
        app,
        token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/orders/{order_id}/packing-sessions"),
        Some(&format!("{key}-open-pack")),
        Some(json!({
            "facility_id": facility_id,
            "station_location_id": station_id,
            "expected_revision": 4
        })),
    )
    .await;
    let opened: OpenPackSessionResponse =
        response_json(expect_status(opened, StatusCode::OK, "open shipping pack session").await)
            .await;
    assert_eq!(opened.session.allocations.len(), 2);

    let mut revision = 5;
    let mut carton_ids = Vec::with_capacity(2);
    let mut carton_barcodes = Vec::with_capacity(2);
    for (index, allocation) in opened.session.allocations.iter().enumerate() {
        let carton_barcode = format!("{key}-CARTON-{}", index + 1);
        let created = send(
            app,
            token,
            access.tenant_id,
            Method::POST,
            &format!(
                "/api/v1/packing-sessions/{}/cartons",
                opened.session.session_id
            ),
            Some(&format!("{key}-carton-{index}")),
            Some(json!({
                "carton_barcode": carton_barcode,
                "expected_revision": revision
            })),
        )
        .await;
        let created: CreateCartonResponse =
            response_json(expect_status(created, StatusCode::OK, "create shipping carton").await)
                .await;
        revision = created.revision.get();

        let packed = send(
            app,
            token,
            access.tenant_id,
            Method::POST,
            &format!(
                "/api/v1/packing-sessions/{}/cartons/{}/contents",
                opened.session.session_id, created.carton.carton_id
            ),
            Some(&format!("{key}-pack-{index}")),
            Some(json!({
                "inventory_allocation_id": allocation.inventory_allocation_id,
                "item_barcode": allocation.item_barcodes[0],
                "lot_scan": allocation.lot.as_deref().unwrap(),
                "source_license_plate_barcode": tote_barcode,
                "carton_barcode": carton_barcode,
                "expected_revision": revision
            })),
        )
        .await;
        let packed: PackPickedAllocationResponse =
            response_json(expect_status(packed, StatusCode::OK, "pack shipping carton").await)
                .await;
        revision = packed.revision.get();

        let closed = send(
            app,
            token,
            access.tenant_id,
            Method::POST,
            &format!(
                "/api/v1/packing-sessions/{}/cartons/{}/closures",
                opened.session.session_id, created.carton.carton_id
            ),
            Some(&format!("{key}-close-{index}")),
            Some(json!({
                "carton_barcode": carton_barcode,
                "measurements": {
                    "weight_grams": 1250 + index as i64,
                    "dimensions": {"length_mm": 300, "width_mm": 200, "height_mm": 150}
                },
                "expected_revision": revision
            })),
        )
        .await;
        let closed: CloseCartonResponse =
            response_json(expect_status(closed, StatusCode::OK, "close shipping carton").await)
                .await;
        revision = closed.revision.get();
        carton_ids.push(created.carton.carton_id);
        carton_barcodes.push(carton_barcode);
    }
    assert_eq!(revision, 11);

    ReadyShipment {
        order_id,
        packing_session_id: opened.session.session_id,
        order_revision: revision,
        carton_ids,
        carton_barcodes,
    }
}

pub(super) async fn set_facility_address(
    fixture: &Fixture,
    tenant_id: TenantId,
    facility_id: i64,
    key: &str,
    complete: bool,
) -> i64 {
    let admin = admin_db_for(&fixture.db).await;
    let mut tx = admin.begin().await.unwrap();
    if !complete {
        // Shipping rejects an incomplete legacy fixture even though new origin writes are guarded.
        sqlx::query("SET LOCAL session_replication_role = replica")
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    let address_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO addresses
            (tenant_id, created, name, company, line1, postal_code, country,
             phone, email, state, city)
        VALUES ($1, clock_timestamp(), $2, $3, $4, $5, 'US', $6, $7, 'NV', 'Reno')
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(format!("{key} Shipping"))
    .bind(format!("{key} Warehouse"))
    .bind("100 Dock Way")
    .bind(complete.then_some("89501"))
    .bind("+1-775-555-0100")
    .bind(format!("{key}@test.local"))
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let (previous_address_id, expected_revision, actor_id): (Option<i64>, i64, i64) =
        sqlx::query_as(
            r#"
            SELECT facility.address_id, facility.revision,
                   (SELECT MIN(membership.user_id)
                    FROM tenant_memberships membership
                    WHERE membership.tenant_id = facility.tenant_id)
            FROM facilities facility
            WHERE facility.tenant_id = $1 AND facility.id = $2
            FOR UPDATE OF facility
            "#,
        )
        .bind(tenant_id.get())
        .bind(facility_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE facilities SET address_id = $1, revision = revision + 1 WHERE tenant_id = $2 AND id = $3",
    )
    .bind(address_id)
    .bind(tenant_id.get())
    .bind(facility_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    if complete {
        sqlx::query(
            r#"
            INSERT INTO facility_shipping_origin_configurations (
                tenant_id, facility_id, previous_address_id, address_id,
                configured_by_user_id, configured_at, expected_revision,
                resulting_revision
            ) VALUES ($1, $2, $3, $4, $5, clock_timestamp(), $6, $6 + 1)
            "#,
        )
        .bind(tenant_id.get())
        .bind(facility_id)
        .bind(previous_address_id)
        .bind(address_id)
        .bind(actor_id)
        .bind(expected_revision)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    admin.close().await;
    address_id
}

pub(super) async fn shipping_effect_counts(
    fixture: &Fixture,
    tenant_id: TenantId,
    order_id: i64,
) -> (i64, i64, i64, i64) {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let counts = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM shipments WHERE tenant_id = $1 AND order_id = $2),
               (SELECT COUNT(*) FROM shipment_address_snapshots WHERE tenant_id = $1),
               (SELECT COUNT(*) FROM shipment_cartons WHERE tenant_id = $1),
               (SELECT COUNT(*) FROM command_idempotency_records
                 WHERE tenant_id = $1 AND operation = 'shipping.shipment.create.v1')
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    counts
}

pub(super) async fn assert_shipment_snapshots(
    fixture: &Fixture,
    tenant_id: TenantId,
    shipment_id: i64,
    destination_address_id: i64,
    origin_address_id: i64,
    ready: &ReadyShipment,
) {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let addresses: Vec<(String, i64, String, String, String)> = sqlx::query_as(
        r#"
        SELECT address_role, source_address_id, line1, postal_code, city
        FROM shipment_address_snapshots
        WHERE tenant_id = $1 AND shipment_id = $2
        ORDER BY address_role
        "#,
    )
    .bind(tenant_id.get())
    .bind(shipment_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(addresses.len(), 2);
    assert_eq!(addresses[0].0, "destination");
    assert_eq!(addresses[0].1, destination_address_id);
    assert_eq!(addresses[1].0, "origin");
    assert_eq!(addresses[1].1, origin_address_id);
    assert!(addresses.iter().all(|snapshot| {
        !snapshot.2.is_empty() && !snapshot.3.is_empty() && !snapshot.4.is_empty()
    }));

    let cartons: Vec<CartonSnapshotRow> = sqlx::query_as(
        r#"
            SELECT carton_id, carton_barcode, content_count, packed_qty,
                   weight_g, length_mm, width_mm, height_mm
            FROM shipment_cartons
            WHERE tenant_id = $1 AND shipment_id = $2
            ORDER BY sequence
            "#,
    )
    .bind(tenant_id.get())
    .bind(shipment_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(cartons.len(), 2);
    assert_eq!(
        cartons
            .iter()
            .map(|carton| carton.carton_id)
            .collect::<Vec<_>>(),
        ready.carton_ids
    );
    assert_eq!(
        cartons
            .iter()
            .map(|carton| carton.carton_barcode.clone())
            .collect::<Vec<_>>(),
        ready.carton_barcodes
    );
    assert_eq!(
        cartons
            .iter()
            .map(|carton| carton.content_count)
            .sum::<i64>(),
        2
    );
    assert_eq!(
        cartons.iter().map(|carton| carton.packed_qty).sum::<i64>(),
        5
    );
    for (index, carton) in cartons.iter().enumerate() {
        assert_eq!(carton.weight_g, Some(1250 + index as i64));
        assert_eq!(
            (carton.length_mm, carton.width_mm, carton.height_mm),
            (Some(300), Some(200), Some(150))
        );
    }
}

pub(super) fn create_shipment_body(ready: &ReadyShipment) -> Value {
    json!({
        "packing_session_id": ready.packing_session_id,
        "expected_revision": ready.order_revision
    })
}

pub(super) fn manifest_body(ready: &ReadyShipment, reference: &str, revision: i64) -> Value {
    json!({
        "carrier_code": "UPS",
        "service_code": "GROUND",
        "manifest_reference": reference,
        "carton_tracking_assignments": [
            {"carton_id": ready.carton_ids[0], "tracking_number": format!("TRACK-{}-1", ready.order_id)},
            {"carton_id": ready.carton_ids[1], "tracking_number": format!("TRACK-{}-2", ready.order_id)}
        ],
        "expected_revision": revision
    })
}
