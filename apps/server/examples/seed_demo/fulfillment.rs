use anyhow::{bail, Context};
use axum::http::Method;
use serde_json::json;
use wareboxes_api::repo;
use wareboxes_api_contract::v1::{
    CloseCartonResponse, CreateCartonResponse, CreateShipmentResponse,
    GenerateCartonLabelSetResponse, GeneratePackingSlipResponse, OpenPackSessionResponse,
    PackPickedAllocationResponse, PickClaimResponse, PickContentConfirmationResponse,
    PlanOutboundLoadResponse, RecordManualManifestResponse, ReleaseOutboundLoadResponse,
};

use crate::support::SeedContext;

const PACK_LOCATION: &str = "DEMO-PACK-01";
const OUTBOUND_STAGE: &str = "DEMO-OUT-STAGE-01";

#[derive(Debug)]
struct ReadyShipment {
    order_id: i64,
    packing_session_id: i64,
    order_revision: i64,
    carton_ids: Vec<i64>,
}

pub async fn seed(context: &SeedContext) -> anyhow::Result<()> {
    context.ensure_shipping_origin().await?;
    let packing_location = context
        .location(PACK_LOCATION, "packing", false, false)
        .await?;
    context
        .location(OUTBOUND_STAGE, "staging", false, false)
        .await?;
    context
        .location("DEMO-OUT-DOCK-01", "dock", false, false)
        .await?;

    seed_packing_ready(context, packing_location).await?;
    seed_packing_active(context, packing_location).await?;
    seed_shipment_awaiting_manifest(context, packing_location).await?;
    seed_manifested_shipment(context, packing_location).await?;
    seed_staging_outbound_load(context, packing_location).await?;
    Ok(())
}

async fn seed_packing_ready(context: &SeedContext, packing_location: i64) -> anyhow::Result<()> {
    let key = "WB-DEMO-FLOW-PACK-READY";
    if context.scenario_exists(key).await? {
        return Ok(());
    }
    let _ = prepare_picked_order(context, packing_location, key).await?;
    println!("seeded packing queue scenario: {key}");
    Ok(())
}

async fn seed_packing_active(context: &SeedContext, packing_location: i64) -> anyhow::Result<()> {
    let key = "WB-DEMO-FLOW-PACK-ACTIVE";
    if context.scenario_exists(key).await? {
        return Ok(());
    }
    let order_id = prepare_picked_order(context, packing_location, key).await?;
    let opened = open_pack_session(context, order_id, packing_location, key).await?;
    let _: CreateCartonResponse = context
        .command(
            Method::POST,
            &format!(
                "/api/v1/packing-sessions/{}/cartons",
                opened.session.session_id
            ),
            &format!("{key}-carton-empty"),
            json!({
                "carton_barcode": format!("{key}-CARTON-EMPTY"),
                "expected_revision": opened.session.revision
            }),
        )
        .await?;
    println!("seeded active packing scenario: {key}");
    Ok(())
}

async fn seed_shipment_awaiting_manifest(
    context: &SeedContext,
    packing_location: i64,
) -> anyhow::Result<()> {
    let key = "WB-DEMO-FLOW-SHIP-READY";
    if context.scenario_exists(key).await? {
        ensure_shipment_documents(context, key, false).await?;
        return Ok(());
    }
    let ready = prepare_ready_shipment(context, packing_location, key).await?;
    let _: CreateShipmentResponse = create_shipment(context, &ready, key).await?;
    ensure_shipment_documents(context, key, false).await?;
    println!("seeded shipment awaiting manifest: {key}");
    Ok(())
}

async fn seed_manifested_shipment(
    context: &SeedContext,
    packing_location: i64,
) -> anyhow::Result<()> {
    let key = "WB-DEMO-FLOW-SHIP-MANIFESTED";
    if context.scenario_exists(key).await? {
        ensure_shipment_documents(context, key, true).await?;
        return Ok(());
    }
    let ready = prepare_ready_shipment(context, packing_location, key).await?;
    let shipment = create_shipment(context, &ready, key).await?;
    let _: RecordManualManifestResponse =
        manifest_shipment(context, &ready, &shipment, key).await?;
    ensure_shipment_documents(context, key, true).await?;
    println!("seeded manifested shipment: {key}");
    Ok(())
}

async fn ensure_shipment_documents(
    context: &SeedContext,
    order_key: &str,
    include_labels: bool,
) -> anyhow::Result<()> {
    let (shipment_id, revision): (i64, i64) = sqlx::query_as(
        r#"
        SELECT shipment.id,shipment.revision
        FROM shipments shipment
        INNER JOIN orders order_header
          ON order_header.tenant_id=shipment.tenant_id AND order_header.id=shipment.order_id
        WHERE shipment.tenant_id=$1 AND order_header.order_key=$2
        ORDER BY shipment.id DESC LIMIT 1
        "#,
    )
    .bind(context.tenant_id.get())
    .bind(order_key)
    .fetch_one(&context.admin)
    .await?;
    let has_packing_slip: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM shipment_documents WHERE tenant_id=$1 AND shipment_id=$2 AND document_type='packing_slip')",
    )
    .bind(context.tenant_id.get())
    .bind(shipment_id)
    .fetch_one(&context.admin)
    .await?;
    if !has_packing_slip {
        let _: GeneratePackingSlipResponse = context
            .command(
                Method::POST,
                &format!("/api/v1/shipments/{shipment_id}/documents/packing-slips"),
                &format!("{order_key}-packing-slip"),
                json!({"expected_shipment_revision":revision}),
            )
            .await?;
    }
    let has_labels: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM shipment_documents WHERE tenant_id=$1 AND shipment_id=$2 AND document_type='carton_label_set')",
    )
    .bind(context.tenant_id.get())
    .bind(shipment_id)
    .fetch_one(&context.admin)
    .await?;
    if include_labels && !has_labels {
        let _: GenerateCartonLabelSetResponse = context
            .command(
                Method::POST,
                &format!("/api/v1/shipments/{shipment_id}/documents/carton-label-sets"),
                &format!("{order_key}-carton-label-set"),
                json!({"expected_shipment_revision":revision}),
            )
            .await?;
    }
    Ok(())
}

async fn seed_staging_outbound_load(
    context: &SeedContext,
    packing_location: i64,
) -> anyhow::Result<()> {
    let key = "WB-DEMO-FLOW-LOAD-STAGING";
    if context.scenario_exists(key).await? {
        return Ok(());
    }
    let ready = prepare_ready_shipment(context, packing_location, key).await?;
    let shipment = create_shipment(context, &ready, key).await?;
    let manifest = manifest_shipment(context, &ready, &shipment, key).await?;
    let staging_location_id = context
        .location(OUTBOUND_STAGE, "staging", false, false)
        .await?;
    let planned: PlanOutboundLoadResponse = context
        .command(
            Method::POST,
            "/api/v1/outbound-loads",
            &format!("{key}-plan-load"),
            json!({
                "facility_id": context.facility_id,
                "load_reference": "WB-DEMO-LOAD-STAGING",
                "carrier_code": "UPS",
                "staging_location_id": staging_location_id,
                "scheduled_departure_at": "2026-08-11T16:00:00Z",
                "shipments": [{
                    "shipment_id": shipment.shipment.shipment_id,
                    "expected_shipment_revision": manifest.revision,
                    "expected_order_revision": shipment.order_revision,
                    "shipment_sequence": 1,
                    "cartons": ready.carton_ids.iter().enumerate().map(|(index, carton_id)| json!({
                        "carton_id": carton_id,
                        "load_sequence": index + 1
                    })).collect::<Vec<_>>()
                }]
            }),
        )
        .await?;
    let _: ReleaseOutboundLoadResponse = context
        .command(
            Method::POST,
            &format!(
                "/api/v1/outbound-loads/{}/releases",
                planned.outbound_load.outbound_load_id
            ),
            &format!("{key}-release-load"),
            json!({"expected_revision": planned.outbound_load.revision}),
        )
        .await?;
    println!("seeded outbound load in staging: {key}");
    Ok(())
}

async fn prepare_picked_order(
    context: &SeedContext,
    packing_location: i64,
    key: &str,
) -> anyhow::Result<i64> {
    context
        .plate_at(&format!("{key}-TOTE"), packing_location)
        .await?;
    let order_id = context.order_header(key).await?;
    for (index, quantity) in [3_i64, 2].into_iter().enumerate() {
        let item_id = context
            .item(&format!("{key} item {}", index + 1), "each")
            .await?;
        repo::items::add_barcode(
            &context.db,
            context.tenant_id,
            item_id,
            &format!("{key}-ITEM-{}", index + 1),
            "code128",
            None,
        )
        .await?;
        context.order_item(order_id, item_id, quantity).await?;
        context
            .received_balance(
                item_id,
                quantity + 4,
                &format!("{key}-SOURCE-{}", index + 1),
            )
            .await?;
    }
    let _: wareboxes_api_contract::v1::PlanOrderAllocationResponse = context
        .command(
            Method::POST,
            &format!("/api/v1/orders/{order_id}/allocation-runs"),
            &format!("{key}-allocate"),
            json!({
                "facility_id": context.facility_id,
                "expected_revision": 1,
                "strategy": "fefo"
            }),
        )
        .await?;
    let _: wareboxes_api_contract::v1::ReleaseOrderResponse = context
        .command(
            Method::POST,
            &format!("/api/v1/orders/{order_id}/releases"),
            &format!("{key}-release"),
            json!({
                "facility_id": context.facility_id,
                "destination_location_id": packing_location,
                "expected_revision": 2
            }),
        )
        .await?;

    for index in 0..2 {
        let claim: Option<PickClaimResponse> = context
            .command(
                Method::POST,
                "/api/v1/picking-claims/next",
                &format!("{key}-claim-{}", index + 1),
                json!({}),
            )
            .await?;
        let claim = claim.ok_or_else(|| anyhow::anyhow!("{key}: no pick work was claimable"))?;
        let _: PickContentConfirmationResponse = context
            .command(
                Method::POST,
                &format!(
                    "/api/v1/picking-tasks/{}/contents/{}/confirmations",
                    claim.task_id, claim.content.content_id
                ),
                &format!("{key}-pick-{}", index + 1),
                json!({
                    "source_location_barcode": claim.content.source_location_barcode,
                    "item_barcode": claim.content.item_barcodes.first().context("pick item barcode")?,
                    "destination_license_plate_barcode": format!("{key}-TOTE")
                }),
            )
            .await?;
    }
    Ok(order_id)
}

async fn open_pack_session(
    context: &SeedContext,
    order_id: i64,
    packing_location: i64,
    key: &str,
) -> anyhow::Result<OpenPackSessionResponse> {
    context
        .command(
            Method::POST,
            &format!("/api/v1/orders/{order_id}/packing-sessions"),
            &format!("{key}-open-pack"),
            json!({
                "facility_id": context.facility_id,
                "station_location_id": packing_location,
                "expected_revision": 4
            }),
        )
        .await
}

async fn prepare_ready_shipment(
    context: &SeedContext,
    packing_location: i64,
    key: &str,
) -> anyhow::Result<ReadyShipment> {
    let order_id = prepare_picked_order(context, packing_location, key).await?;
    let opened = open_pack_session(context, order_id, packing_location, key).await?;
    if opened.session.allocations.len() != 2 {
        bail!("{key}: expected two packing allocations");
    }
    let mut revision = opened.session.revision.get();
    let mut carton_ids = Vec::new();
    for (index, allocation) in opened.session.allocations.iter().enumerate() {
        let carton_barcode = format!("{key}-CARTON-{}", index + 1);
        let created: CreateCartonResponse = context
            .command(
                Method::POST,
                &format!(
                    "/api/v1/packing-sessions/{}/cartons",
                    opened.session.session_id
                ),
                &format!("{key}-carton-{}", index + 1),
                json!({
                    "carton_barcode": carton_barcode,
                    "expected_revision": revision
                }),
            )
            .await?;
        revision = created.revision.get();
        let packed: PackPickedAllocationResponse = context
            .command(
                Method::POST,
                &format!(
                    "/api/v1/packing-sessions/{}/cartons/{}/contents",
                    opened.session.session_id, created.carton.carton_id
                ),
                &format!("{key}-pack-{}", index + 1),
                json!({
                    "inventory_allocation_id": allocation.inventory_allocation_id,
                    "item_barcode": allocation.item_barcodes.first().context("pack item barcode")?,
                    "lot_scan": allocation.lot.as_deref().context("pack lot")?,
                    "source_license_plate_barcode": format!("{key}-TOTE"),
                    "carton_barcode": carton_barcode,
                    "expected_revision": revision
                }),
            )
            .await?;
        revision = packed.revision.get();
        let closed: CloseCartonResponse = context
            .command(
                Method::POST,
                &format!(
                    "/api/v1/packing-sessions/{}/cartons/{}/closures",
                    opened.session.session_id, created.carton.carton_id
                ),
                &format!("{key}-close-{}", index + 1),
                json!({
                    "carton_barcode": carton_barcode,
                    "measurements": {
                        "weight_grams": 1250 + index,
                        "dimensions": {"length_mm": 300, "width_mm": 200, "height_mm": 150}
                    },
                    "expected_revision": revision
                }),
            )
            .await?;
        revision = closed.revision.get();
        carton_ids.push(created.carton.carton_id);
    }
    Ok(ReadyShipment {
        order_id,
        packing_session_id: opened.session.session_id,
        order_revision: revision,
        carton_ids,
    })
}

async fn create_shipment(
    context: &SeedContext,
    ready: &ReadyShipment,
    key: &str,
) -> anyhow::Result<CreateShipmentResponse> {
    context
        .command(
            Method::POST,
            &format!("/api/v1/orders/{}/shipments", ready.order_id),
            &format!("{key}-create-shipment"),
            json!({
                "packing_session_id": ready.packing_session_id,
                "expected_revision": ready.order_revision
            }),
        )
        .await
}

async fn manifest_shipment(
    context: &SeedContext,
    ready: &ReadyShipment,
    shipment: &CreateShipmentResponse,
    key: &str,
) -> anyhow::Result<RecordManualManifestResponse> {
    context
        .command(
            Method::POST,
            &format!(
                "/api/v1/shipments/{}/manifests",
                shipment.shipment.shipment_id
            ),
            &format!("{key}-manifest"),
            json!({
                "carrier_code": "UPS",
                "service_code": "GROUND",
                "manifest_reference": format!("{key}-MANIFEST"),
                "carton_tracking_assignments": ready.carton_ids.iter().enumerate().map(|(index, carton_id)| json!({
                    "carton_id": carton_id,
                    "tracking_number": format!("WB{}{:02}", ready.order_id, index + 1)
                })).collect::<Vec<_>>(),
                "expected_revision": shipment.shipment.revision
            }),
        )
        .await
}
