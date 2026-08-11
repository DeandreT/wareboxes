use anyhow::{bail, Context};
use axum::body::to_bytes;
use axum::http::{Method, StatusCode};
use serde_json::json;
use wareboxes_api::repo;
use wareboxes_api_contract::v1::{
    CancelInboundAsnResponse, CancelPurchaseOrderResponse, ConfigureReplenishmentPolicyResponse,
    CreateCycleCountTaskResponse, CreatePurchaseOrderAsnResponse, CreatePurchaseOrderResponse,
    CreatePutawayTaskResponse, IntegrationOrderIntakeResponse,
    IntegrationOrderOwnerMappingResponse, PickWaveResponse, PlaceInventoryHoldResponse,
    PlanInboundAsnLoadResponse, PlanOrderAllocationResponse, PlanReplenishmentResponse,
    ReleasePurchaseOrderResponse,
};

use crate::support::SeedContext;

pub async fn seed(context: &SeedContext) -> anyhow::Result<()> {
    seed_purchase_orders(context).await?;
    seed_inventory_hold(context).await?;
    seed_cycle_count(context).await?;
    seed_putaway(context).await?;
    seed_replenishment(context).await?;
    seed_pick_wave(context).await?;
    seed_integration_monitor(context).await?;
    Ok(())
}

async fn seed_purchase_orders(context: &SeedContext) -> anyhow::Result<()> {
    let states: (bool, bool) = sqlx::query_as(
        r#"
        SELECT EXISTS(SELECT 1 FROM purchase_orders WHERE tenant_id=$1 AND status='draft'),
               EXISTS(SELECT 1 FROM purchase_orders WHERE tenant_id=$1 AND status='released')
        "#,
    )
    .bind(context.tenant_id.get())
    .fetch_one(&context.admin)
    .await?;
    let item_ids = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT owner_item.item_id
        FROM inventory_owner_items owner_item
        INNER JOIN items item
          ON item.tenant_id=owner_item.tenant_id AND item.id=owner_item.item_id
        WHERE owner_item.tenant_id=$1 AND owner_item.inventory_owner_id=$2
          AND owner_item.deleted IS NULL AND item.deleted IS NULL
        ORDER BY owner_item.item_id
        LIMIT 2
        "#,
    )
    .bind(context.tenant_id.get())
    .bind(context.inventory_owner_id)
    .fetch_all(&context.admin)
    .await?;
    if item_ids.len() < 2 {
        bail!("purchase-order demo requires two client-eligible catalog items");
    }
    if states != (true, true) {
        for sequence in 1_i64..=6 {
            let number = format!("WB-DEMO-PO-{sequence:04}");
            let created: CreatePurchaseOrderResponse = context
                .command(
                    Method::POST,
                    "/api/v1/purchase-orders",
                    &format!("demo-purchase-order-{sequence}"),
                    json!({
                        "inventory_owner_id": context.inventory_owner_id,
                        "facility_id": context.facility_id,
                        "number": number,
                        "supplier": if sequence % 2 == 0 { "Northstar Foods" } else { "Cascade Supply Co." },
                        "expected_by": format!("2027-08-{:02}T17:00:00Z", 10 + sequence),
                        "lines": [
                            {"item_id": item_ids[0], "ordered_quantity": 8 + sequence},
                            {"item_id": item_ids[1], "ordered_quantity": 12 + sequence}
                        ]
                    }),
                )
                .await?;
            if sequence <= 3 {
                let _: ReleasePurchaseOrderResponse = context
                    .command(
                        Method::POST,
                        &format!(
                            "/api/v1/purchase-orders/{}/releases",
                            created.purchase_order_id
                        ),
                        &format!("demo-purchase-order-release-{sequence}"),
                        json!({"expected_revision": created.revision}),
                    )
                    .await?;
            }
        }
    }
    let source_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM purchase_order_asn_sources WHERE tenant_id=$1)",
    )
    .bind(context.tenant_id.get())
    .fetch_one(&context.admin)
    .await?;
    if !source_exists {
        let source: (i64, i64, i64, i64, i64) = sqlx::query_as(
            r#"
            SELECT purchase.id,purchase.revision,first_line.id,first_line.ordered_quantity,
                   second_line.id
            FROM purchase_orders purchase
            INNER JOIN purchase_order_lines first_line
              ON first_line.tenant_id=purchase.tenant_id
             AND first_line.purchase_order_id=purchase.id AND first_line.sequence=1
            INNER JOIN purchase_order_lines second_line
              ON second_line.tenant_id=purchase.tenant_id
             AND second_line.purchase_order_id=purchase.id AND second_line.sequence=2
            WHERE purchase.tenant_id=$1 AND purchase.status='released'
            ORDER BY purchase.id
            LIMIT 1
            "#,
        )
        .bind(context.tenant_id.get())
        .fetch_one(&context.admin)
        .await?;
        let second_quantity: i64 = sqlx::query_scalar(
            "SELECT ordered_quantity FROM purchase_order_lines WHERE tenant_id=$1 AND id=$2",
        )
        .bind(context.tenant_id.get())
        .bind(source.4)
        .fetch_one(&context.admin)
        .await?;
        let _: CreatePurchaseOrderAsnResponse = context
            .command(
                Method::POST,
                &format!("/api/v1/purchase-orders/{}/asns", source.0),
                "demo-purchase-order-asn-source",
                json!({
                    "expected_purchase_order_revision": source.1,
                    "number": "WB-DEMO-ASN-FROM-PO-0001",
                    "expected_at": "2027-08-12T14:00:00Z",
                    "lines": [
                        {
                            "purchase_order_line_id": source.2,
                            "expected_quantity": std::cmp::max(source.3 / 2, 1),
                            "lot": "WB-DEMO-PO-LOT-01",
                            "serial": null,
                            "expiration": "2028-08-12T00:00:00Z"
                        },
                        {
                            "purchase_order_line_id": source.4,
                            "expected_quantity": second_quantity,
                            "lot": null,
                            "serial": null,
                            "expiration": null
                        }
                    ]
                }),
            )
            .await?;
    }
    seed_purchase_order_receipt_progress(context).await?;
    seed_cancelled_purchase_order_notice(context).await?;
    seed_cancelled_purchase_order(context, &item_ids).await?;
    println!("seeded draft, released, and cancelled purchase orders with PO-sourced ASN demand");
    Ok(())
}

async fn seed_cancelled_purchase_order(
    context: &SeedContext,
    item_ids: &[i64],
) -> anyhow::Result<()> {
    let existing: Option<(i64, String, i64)> = sqlx::query_as(
        r#"
        SELECT id,status,revision
        FROM purchase_orders
        WHERE tenant_id=$1 AND number='WB-DEMO-PO-CANCELLED-0001'
        "#,
    )
    .bind(context.tenant_id.get())
    .fetch_optional(&context.admin)
    .await?;
    let (purchase_order_id, status, revision) = match existing {
        Some(existing) => existing,
        None => {
            let created: CreatePurchaseOrderResponse = context
                .command(
                    Method::POST,
                    "/api/v1/purchase-orders",
                    "demo-cancelled-purchase-order-create",
                    json!({
                        "inventory_owner_id": context.inventory_owner_id,
                        "facility_id": context.facility_id,
                        "number": "WB-DEMO-PO-CANCELLED-0001",
                        "supplier": "Cancelled Supplier Example",
                        "expected_by": "2027-08-24T17:00:00Z",
                        "lines": [
                            {"item_id": item_ids[0], "ordered_quantity": 6},
                            {"item_id": item_ids[1], "ordered_quantity": 9}
                        ]
                    }),
                )
                .await?;
            (
                created.purchase_order_id,
                "draft".to_owned(),
                created.revision.get(),
            )
        }
    };
    match status.as_str() {
        "draft" | "released" => {
            let _: CancelPurchaseOrderResponse = context
                .command(
                    Method::POST,
                    &format!("/api/v1/purchase-orders/{purchase_order_id}/cancellations"),
                    "demo-cancelled-purchase-order-cancel",
                    json!({
                        "expected_revision": revision,
                        "reason": "demand_cancelled",
                        "note": "Demo demand was withdrawn before receiving began"
                    }),
                )
                .await?;
        }
        "cancelled" => {}
        status => bail!("demo cancellation purchase order has unsupported resume status {status}"),
    }
    Ok(())
}

async fn seed_cancelled_purchase_order_notice(context: &SeedContext) -> anyhow::Result<()> {
    let existing_notice: Option<(i64, i64, String, i64)> = sqlx::query_as(
        r#"
        SELECT asn.id,source.purchase_order_id,asn.status,asn.revision
        FROM inbound_asns asn
        INNER JOIN purchase_order_asn_sources source
          ON source.tenant_id=asn.tenant_id AND source.asn_id=asn.id
        WHERE asn.tenant_id=$1 AND asn.number='WB-DEMO-ASN-CANCELLED-0001'
        "#,
    )
    .bind(context.tenant_id.get())
    .fetch_optional(&context.admin)
    .await?;
    let order: (i64, i64) = if let Some((_, purchase_order_id, _, _)) = &existing_notice {
        sqlx::query_as("SELECT id,revision FROM purchase_orders WHERE tenant_id=$1 AND id=$2")
            .bind(context.tenant_id.get())
            .bind(*purchase_order_id)
            .fetch_one(&context.admin)
            .await?
    } else {
        sqlx::query_as(
            r#"
            SELECT purchase.id,purchase.revision
            FROM purchase_orders purchase
            WHERE purchase.tenant_id=$1 AND purchase.status='released'
              AND NOT EXISTS (
                  SELECT 1 FROM purchase_order_asn_sources source
                  WHERE source.tenant_id=purchase.tenant_id
                    AND source.purchase_order_id=purchase.id)
            ORDER BY purchase.id
            LIMIT 1
            "#,
        )
        .bind(context.tenant_id.get())
        .fetch_one(&context.admin)
        .await?
    };
    let source_lines = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT id,ordered_quantity FROM purchase_order_lines
        WHERE tenant_id=$1 AND purchase_order_id=$2
        ORDER BY sequence,id
        "#,
    )
    .bind(context.tenant_id.get())
    .bind(order.0)
    .fetch_all(&context.admin)
    .await?;
    let line_payload = |lot_prefix: &str| {
        source_lines
            .iter()
            .enumerate()
            .map(|(index, (line_id, quantity))| {
                json!({
                    "purchase_order_line_id": line_id,
                    "expected_quantity": quantity,
                    "lot": (index == 0).then(|| format!("{lot_prefix}-LOT")),
                    "serial": null,
                    "expiration": null
                })
            })
            .collect::<Vec<_>>()
    };
    let cancellation_target = if let Some((asn_id, _, status, revision)) = existing_notice {
        (asn_id, status, revision)
    } else {
        let cancelled_source: CreatePurchaseOrderAsnResponse = context
            .command(
                Method::POST,
                &format!("/api/v1/purchase-orders/{}/asns", order.0),
                "demo-purchase-order-cancelled-asn-source",
                json!({
                    "expected_purchase_order_revision": order.1,
                    "number": "WB-DEMO-ASN-CANCELLED-0001",
                    "expected_at": "2027-08-16T14:00:00Z",
                    "lines": line_payload("WB-DEMO-CANCELLED")
                }),
            )
            .await?;
        (
            cancelled_source.asn_id,
            "open".to_owned(),
            cancelled_source.revision.get(),
        )
    };
    match cancellation_target.1.as_str() {
        "open" => {
            let _: CancelInboundAsnResponse = context
                .command(
                    Method::POST,
                    &format!(
                        "/api/v1/inbound-asns/{}/cancellations",
                        cancellation_target.0
                    ),
                    "demo-purchase-order-cancel-asn",
                    json!({
                            "expected_revision": cancellation_target.2,
                        "reason": "supplier_cancelled",
                        "note": "Supplier replaced the original shipping notice"
                    }),
                )
                .await?;
        }
        "cancelled" => {}
        status => anyhow::bail!("demo cancellation ASN has unsupported resume status {status}"),
    }
    let replacement_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM inbound_asns WHERE tenant_id=$1 AND number='WB-DEMO-ASN-REPLACEMENT-0001')",
    )
    .bind(context.tenant_id.get())
    .fetch_one(&context.admin)
    .await?;
    if !replacement_exists {
        let _: CreatePurchaseOrderAsnResponse = context
            .command(
                Method::POST,
                &format!("/api/v1/purchase-orders/{}/asns", order.0),
                "demo-purchase-order-replacement-asn-source",
                json!({
                    "expected_purchase_order_revision": order.1,
                    "number": "WB-DEMO-ASN-REPLACEMENT-0001",
                    "expected_at": "2027-08-17T14:00:00Z",
                    "lines": line_payload("WB-DEMO-REPLACEMENT")
                }),
            )
            .await?;
    }
    Ok(())
}

async fn seed_purchase_order_receipt_progress(context: &SeedContext) -> anyhow::Result<()> {
    let source: Option<(i64, i64, String)> = sqlx::query_as(
        r#"
        SELECT asn.id,asn.revision,asn.status
        FROM inbound_asns asn
        WHERE asn.tenant_id=$1 AND asn.number='WB-DEMO-ASN-FROM-PO-0001'
        "#,
    )
    .bind(context.tenant_id.get())
    .fetch_optional(&context.admin)
    .await?;
    let Some((asn_id, revision, status)) = source else {
        return Ok(());
    };
    if status != "open" {
        seed_purchase_order_follow_up_asn(context, asn_id).await?;
        return Ok(());
    }

    let receiving_barcode = "WB-DEMO-PO-RECV-01";
    let receiving_location_id = context
        .location(receiving_barcode, "dock", false, true)
        .await?;
    let plan: PlanInboundAsnLoadResponse = context
        .command(
            Method::POST,
            &format!("/api/v1/inbound-asns/{asn_id}/load-plans"),
            "demo-purchase-order-asn-load-plan",
            json!({
                "expected_revision": revision,
                "receiving_location_id": receiving_location_id,
                "carrier": "Cascade Inbound Freight",
                "trailer_number": "WB-DEMO-PO-TRL-01",
                "seal_number": "WB-DEMO-PO-SEAL-01"
            }),
        )
        .await?;
    let _: serde_json::Value = context
        .command(
            Method::POST,
            &format!("/api/v1/inbound-loads/{}/arrivals", plan.load_id),
            "demo-purchase-order-asn-arrival",
            json!({
                "load_scan": plan.execution_barcode,
                "receiving_location_scan": receiving_barcode,
                "arrived_at": null
            }),
        )
        .await?;
    let _: serde_json::Value = context
        .command(
            Method::POST,
            &format!("/api/v1/inbound-loads/{}/unloading-starts", plan.load_id),
            "demo-purchase-order-asn-unloading-start",
            json!({
                "load_scan": plan.execution_barcode,
                "receiving_location_scan": receiving_barcode,
                "seal_scan": "WB-DEMO-PO-SEAL-01",
                "started_at": null
            }),
        )
        .await?;

    for (index, line) in plan.lines.iter().enumerate() {
        if index == 0 {
            let barcode =
                ensure_seed_item_barcode(context, line.item_id, "WB-DEMO-PO-ITEM").await?;
            let quantity = std::cmp::max(line.expected_quantity / 2, 1);
            let _: serde_json::Value = context
                .command(
                    Method::POST,
                    &format!(
                        "/api/v1/expected-receiving/lines/{}/confirmations",
                        line.load_line_id
                    ),
                    "demo-purchase-order-asn-receive-partial",
                    json!({
                        "disposition": "received",
                        "item_barcode": barcode,
                        "receiving_location_barcode": receiving_barcode,
                        "quantity": quantity,
                        "license_plate_barcode": "WB-DEMO-PO-RECEIPT-LP-01",
                        "lot": "WB-DEMO-PO-LOT-01",
                        "serial": null,
                        "expiration": "2028-08-12T00:00:00Z"
                    }),
                )
                .await?;
        } else {
            let _: serde_json::Value = context
                .command(
                    Method::POST,
                    &format!(
                        "/api/v1/expected-receiving/lines/{}/confirmations",
                        line.load_line_id
                    ),
                    "demo-purchase-order-asn-reject-partial",
                    json!({
                        "disposition": "rejected",
                        "item_barcode": ensure_seed_item_barcode(context, line.item_id, "WB-DEMO-PO-ITEM").await?,
                        "quantity": 1,
                        "reason": "damaged",
                        "note": "Visible demo receiving exception"
                    }),
                )
                .await?;
        }
    }
    seed_purchase_order_follow_up_asn(context, asn_id).await?;
    Ok(())
}

async fn seed_purchase_order_follow_up_asn(
    context: &SeedContext,
    source_asn_id: i64,
) -> anyhow::Result<()> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM inbound_asns WHERE tenant_id=$1 AND number='WB-DEMO-ASN-FROM-PO-0002')",
    )
    .bind(context.tenant_id.get())
    .fetch_one(&context.admin)
    .await?;
    if exists {
        return Ok(());
    }
    let order: (i64, i64) = sqlx::query_as(
        r#"
        SELECT source.purchase_order_id,purchase.revision
        FROM purchase_order_asn_sources source
        INNER JOIN purchase_orders purchase
          ON purchase.tenant_id=source.tenant_id AND purchase.id=source.purchase_order_id
        WHERE source.tenant_id=$1 AND source.asn_id=$2
        "#,
    )
    .bind(context.tenant_id.get())
    .bind(source_asn_id)
    .fetch_one(&context.admin)
    .await?;
    let lines = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT purchase_order_line_id,
               ordered_quantity-received_quantity-active_inbound_quantity AS available_quantity
        FROM purchase_order_line_inbound_progress
        WHERE tenant_id=$1 AND purchase_order_id=$2
          AND ordered_quantity-received_quantity-active_inbound_quantity > 0
        ORDER BY purchase_order_line_id
        "#,
    )
    .bind(context.tenant_id.get())
    .bind(order.0)
    .fetch_all(&context.admin)
    .await?;
    if lines.is_empty() {
        return Ok(());
    }
    let lines = lines
        .into_iter()
        .map(|(line_id, quantity)| {
            json!({
                "purchase_order_line_id": line_id,
                "expected_quantity": quantity,
                "lot": null,
                "serial": null,
                "expiration": null
            })
        })
        .collect::<Vec<_>>();
    let _: CreatePurchaseOrderAsnResponse = context
        .command(
            Method::POST,
            &format!("/api/v1/purchase-orders/{}/asns", order.0),
            "demo-purchase-order-follow-up-asn",
            json!({
                "expected_purchase_order_revision": order.1,
                "number": "WB-DEMO-ASN-FROM-PO-0002",
                "expected_at": "2027-08-15T14:00:00Z",
                "lines": lines
            }),
        )
        .await?;
    Ok(())
}

async fn ensure_seed_item_barcode(
    context: &SeedContext,
    item_id: i64,
    prefix: &str,
) -> anyhow::Result<String> {
    if let Some(barcode) = sqlx::query_scalar::<_, String>(
        "SELECT name FROM barcodes WHERE tenant_id=$1 AND item_id=$2 AND deleted IS NULL ORDER BY id LIMIT 1",
    )
    .bind(context.tenant_id.get())
    .bind(item_id)
    .fetch_optional(&context.admin)
    .await?
    {
        return Ok(barcode);
    }
    let barcode = format!("{prefix}-{item_id}");
    repo::items::add_barcode(
        &context.db,
        context.tenant_id,
        item_id,
        &barcode,
        "code128",
        Some("Demo receiving barcode"),
    )
    .await?;
    Ok(barcode)
}

async fn seed_inventory_hold(context: &SeedContext) -> anyhow::Result<()> {
    if table_has_key(
        context,
        "command_idempotency_records",
        "idempotency_key='demo-hold-place'",
    )
    .await?
    {
        return Ok(());
    }
    let item_id = context.item("Demo hold inspection item", "case").await?;
    let balance = context
        .received_balance(item_id, 18, "DEMO-HOLD-SOURCE")
        .await?;
    let _: PlaceInventoryHoldResponse = context
        .command(
            Method::POST,
            "/api/v1/inventory/holds",
            "demo-hold-place",
            json!({
                "inventory_balance_id": balance.balance_id,
                "quantity": 4,
                "reason": "quality_inspection",
                "note": "Awaiting inbound quality review",
                "reference_type": "demo_seed",
                "reference_id": balance.balance_id
            }),
        )
        .await?;
    println!("seeded active inventory hold");
    Ok(())
}

async fn seed_cycle_count(context: &SeedContext) -> anyhow::Result<()> {
    if table_has_key(
        context,
        "command_idempotency_records",
        "idempotency_key='demo-cycle-count-create'",
    )
    .await?
    {
        return Ok(());
    }
    let item_id = context.item("Demo cycle count item", "each").await?;
    repo::items::add_barcode(
        &context.db,
        context.tenant_id,
        item_id,
        "DEMO-COUNT-ITEM",
        "code128",
        None,
    )
    .await?;
    let balance = context
        .received_balance(item_id, 27, "DEMO-COUNT-SOURCE")
        .await?;
    let _: CreateCycleCountTaskResponse = context
        .command(
            Method::POST,
            "/api/v1/cycle-count-tasks",
            "demo-cycle-count-create",
            json!({
                "inventory_balance_id": balance.balance_id,
                "note": "Demo blind count awaiting assignment"
            }),
        )
        .await?;
    println!("seeded pending cycle count");
    Ok(())
}

async fn seed_putaway(context: &SeedContext) -> anyhow::Result<()> {
    if table_has_key(
        context,
        "command_idempotency_records",
        "idempotency_key='demo-putaway-create'",
    )
    .await?
    {
        return Ok(());
    }
    let receiving_id = context
        .location("DEMO-PUTAWAY-RECV", "staging", false, true)
        .await?;
    let destination_id = context
        .location("DEMO-PUTAWAY-A-01", "rack", true, false)
        .await?;
    let item_id = context.item("Demo putaway item", "case").await?;
    let batch_id = repo::inventory::add_item_batch(
        &context.db,
        context.tenant_id,
        context.inventory_owner_id,
        item_id,
        None,
        Some("DEMO-PUTAWAY-LOT"),
        None,
        None,
    )
    .await?;
    repo::inventory::receive_inventory(
        &context.db,
        context.tenant_id,
        context.user_id,
        batch_id,
        receiving_id,
        24,
        None,
        Some("demo putaway receipt"),
        None,
        None,
        "demo-putaway-receive",
    )
    .await?;
    let source_balance_id: i64 = sqlx::query_scalar(
        "SELECT id FROM inventory_balances WHERE tenant_id=$1 AND item_batch_id=$2 AND location_id=$3 AND deleted IS NULL",
    )
    .bind(context.tenant_id.get())
    .bind(batch_id)
    .bind(receiving_id)
    .fetch_one(&context.admin)
    .await?;
    let _: CreatePutawayTaskResponse = context
        .command(
            Method::POST,
            "/api/v1/putaway-tasks",
            "demo-putaway-create",
            json!({
                "source_inventory_balance_id": source_balance_id,
                "destination_location_id": destination_id,
                "quantity": 16,
                "priority": 80,
                "assigned_user_id": null,
                "instructions": "Scan the directed pick-face location"
            }),
        )
        .await?;
    println!("seeded pending directed putaway");
    Ok(())
}

async fn seed_replenishment(context: &SeedContext) -> anyhow::Result<()> {
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM replenishment_policies WHERE tenant_id=$1 AND effective_to IS NULL)",
    )
    .bind(context.tenant_id.get())
    .fetch_one(&context.admin)
    .await?
    {
        return Ok(());
    }
    let pick_face = context
        .location("DEMO-REPLEN-PICK-01", "pick", true, false)
        .await?;
    let reserve = context
        .location("DEMO-REPLEN-RESERVE-01", "reserve", false, false)
        .await?;
    let item_id = context.item("Demo replenishment item", "case").await?;
    repo::items::add_barcode(
        &context.db,
        context.tenant_id,
        item_id,
        "DEMO-REPLEN-ITEM",
        "code128",
        None,
    )
    .await?;
    let batch_id = repo::inventory::add_item_batch(
        &context.db,
        context.tenant_id,
        context.inventory_owner_id,
        item_id,
        None,
        Some("DEMO-REPLEN-LOT"),
        None,
        None,
    )
    .await?;
    repo::inventory::receive_inventory(
        &context.db,
        context.tenant_id,
        context.user_id,
        batch_id,
        reserve,
        30,
        None,
        Some("demo reserve stock"),
        None,
        None,
        "demo-replen-receive",
    )
    .await?;
    let configured: ConfigureReplenishmentPolicyResponse = context
        .command(
            Method::POST,
            "/api/v1/replenishment-policies",
            "demo-replen-policy",
            json!({
                "inventory_owner_id": context.inventory_owner_id,
                "facility_id": context.facility_id,
                "item_id": item_id,
                "uom": "case",
                "pick_face_location_id": pick_face,
                "minimum_quantity": 4,
                "target_quantity": 12,
                "reserve_source_location_ids": [reserve],
                "expected_revision": null
            }),
        )
        .await?;
    let _: PlanReplenishmentResponse = context
        .command(
            Method::POST,
            &format!(
                "/api/v1/replenishment-policies/{}/plan-runs",
                configured.policy_id
            ),
            "demo-replen-plan",
            json!({"expected_policy_revision": configured.revision}),
        )
        .await?;
    println!("seeded replenishment policy and open work");
    Ok(())
}

async fn seed_pick_wave(context: &SeedContext) -> anyhow::Result<()> {
    let marker = "WB-DEMO-WAVE-ORDER-A";
    if context.scenario_exists(marker).await? {
        return Ok(());
    }
    let destination = context
        .location("DEMO-WAVE-STAGE", "staging", false, false)
        .await?;
    let first = allocated_wave_order(context, marker, 4).await?;
    let second = allocated_wave_order(context, "WB-DEMO-WAVE-ORDER-B", 7).await?;
    let _: PickWaveResponse = context
        .command(
            Method::POST,
            "/api/v1/pick-waves",
            "demo-wave-plan",
            json!({
                "facility_id": context.facility_id,
                "destination_location_id": destination,
                "name": "Demo priority parcel wave",
                "orders": [
                    {"order_id": first.0, "expected_revision": first.1, "sequence": 1},
                    {"order_id": second.0, "expected_revision": second.1, "sequence": 2}
                ]
            }),
        )
        .await?;
    println!("seeded planned pick wave");
    Ok(())
}

async fn allocated_wave_order(
    context: &SeedContext,
    key: &str,
    quantity: i64,
) -> anyhow::Result<(i64, i64)> {
    let item_id = context.item(&format!("{key} item"), "each").await?;
    repo::items::add_barcode(
        &context.db,
        context.tenant_id,
        item_id,
        &format!("{key}-ITEM"),
        "code128",
        None,
    )
    .await?;
    let order_id = context.order_header(key).await?;
    context.order_item(order_id, item_id, quantity).await?;
    context
        .received_balance(item_id, quantity, &format!("{key}-SOURCE"))
        .await?;
    let result: PlanOrderAllocationResponse = context
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
    Ok((order_id, result.revision.get()))
}

async fn seed_integration_monitor(context: &SeedContext) -> anyhow::Result<()> {
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM integration_inbox_receipts WHERE tenant_id=$1 AND source_key='demo-partner')",
    )
    .bind(context.tenant_id.get())
    .fetch_one(&context.admin)
    .await?
    {
        return Ok(());
    }
    let _: IntegrationOrderOwnerMappingResponse = context
        .command(
            Method::POST,
            "/api/v1/integration-order-owner-mappings",
            "demo-integration-owner-map",
            json!({
                "source_key": "demo-partner",
                "external_inventory_owner_key": "NORTHSTAR",
                "inventory_owner_id": context.inventory_owner_id,
                "expected_revision": null
            }),
        )
        .await?;
    let response = context
        .send(
            Method::POST,
            "/api/v1/integrations/order-intake/demo-partner/inventory-owners/NORTHSTAR/orders",
            Some("demo-integration-message-1"),
            Some(json!({
                "order_key": "WB-DEMO-INTEGRATION-ORDER-1",
                "rush": true,
                "ship_by": "2026-08-12T18:00:00Z",
                "destination": {
                    "recipient_name": "Demo Integration Receiving",
                    "company": "Northstar Retail",
                    "phone": null,
                    "email": "receiving@example.test",
                    "line1": "500 Partner Lane",
                    "line2": null,
                    "city": "Portland",
                    "region": "OR",
                    "postal_code": "97205",
                    "country": "US"
                },
                "lines": [{
                    "line_key": "1",
                    "external_item_key": "UNMAPPED-DEMO-SKU",
                    "external_uom": "CS",
                    "quantity": 6
                }]
            })),
        )
        .await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 512 * 1024).await?;
    if status != StatusCode::ACCEPTED {
        bail!(
            "demo integration intake: expected 202, got {status}: {}",
            String::from_utf8_lossy(&bytes)
        );
    }
    let result: IntegrationOrderIntakeResponse =
        serde_json::from_slice(&bytes).context("decoding demo integration intake response")?;
    println!("seeded integration monitor receipt: {:?}", result.status);
    Ok(())
}

async fn table_has_key(
    context: &SeedContext,
    table: &str,
    predicate: &str,
) -> anyhow::Result<bool> {
    let sql = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE tenant_id=$1 AND {predicate})");
    Ok(sqlx::query_scalar::<_, bool>(&sql)
        .bind(context.tenant_id.get())
        .fetch_one(&context.admin)
        .await?)
}
