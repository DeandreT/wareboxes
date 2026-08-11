use anyhow::{bail, Context};
use axum::http::Method;
use serde_json::json;
use wareboxes_api::{auth, repo};
use wareboxes_api_contract::v1::{
    CancelCrossDockWorkResponse, ConfirmCrossDockWorkResponse, CrossDockClaimResponse,
    CrossDockWorkStatus, PlanCrossDockWorkResponse,
};
use wareboxes_application::inbound_load::StartInboundLoadUnloadingCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InboundReceiptExceptionReason, LoadStatus, LoadType, TenantAccess};
use wareboxes_domain::{InboundLoadId, InboundLoadScanValue};

use crate::support::SeedContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScenarioState {
    PlanningReady,
    Pending,
    Cancelled,
    Completed,
}

struct Scenario {
    sequence: i64,
    order_id: i64,
    order_line_id: i64,
    receipt_transaction_id: i64,
    source_barcode: String,
    destination_barcode: String,
    item_barcode: String,
    lot: String,
}

pub async fn seed(context: &SeedContext) -> anyhow::Result<()> {
    let access = auth::default_tenant_for_session(&context.db, &context.token)
        .await?
        .context("loading demo seed access for cross-docking")?;
    for (sequence, state) in [
        (1, ScenarioState::PlanningReady),
        (2, ScenarioState::Pending),
        (3, ScenarioState::Cancelled),
        (4, ScenarioState::Completed),
    ] {
        seed_scenario(context, &access, sequence, state).await?;
    }
    println!("seeded planning-ready, pending, cancelled, and completed cross-dock workflows");
    Ok(())
}

async fn seed_scenario(
    context: &SeedContext,
    access: &TenantAccess,
    sequence: i64,
    desired: ScenarioState,
) -> anyhow::Result<()> {
    let scenario = ensure_scenario(context, access, sequence).await?;
    if desired == ScenarioState::PlanningReady {
        return Ok(());
    }

    let existing: Option<(i64, String)> = sqlx::query_as(
        r#"
        SELECT work.id,work.status
        FROM cross_dock_tasks detail
        JOIN work_tasks work ON work.tenant_id=detail.tenant_id AND work.id=detail.task_id
        WHERE detail.tenant_id=$1 AND detail.order_id=$2
        ORDER BY work.id DESC LIMIT 1
        "#,
    )
    .bind(context.tenant_id.get())
    .bind(scenario.order_id)
    .fetch_optional(&context.admin)
    .await?;
    if matches!(
        (&existing, desired),
        (Some((_, status)), ScenarioState::Pending) if status == "open"
    ) || matches!(
        (&existing, desired),
        (Some((_, status)), ScenarioState::Cancelled) if status == "cancelled"
    ) || matches!(
        (&existing, desired),
        (Some((_, status)), ScenarioState::Completed) if status == "completed"
    ) {
        return Ok(());
    }

    let (work_id, status) = match existing {
        Some((work_id, status)) if status != "cancelled" => (work_id, status),
        _ => {
            let current_revision: i64 =
                sqlx::query_scalar("SELECT revision FROM orders WHERE tenant_id=$1 AND id=$2")
                    .bind(context.tenant_id.get())
                    .bind(scenario.order_id)
                    .fetch_one(&context.admin)
                    .await?;
            let planned: PlanCrossDockWorkResponse = context
                .command(
                    Method::POST,
                    &format!("/api/v1/orders/{}/cross-dock-tasks", scenario.order_id),
                    &format!("demo-cross-dock-plan-{}-{current_revision}", scenario.sequence),
                    json!({
                        "order_line_id": scenario.order_line_id,
                        "expected_order_revision": current_revision,
                        "source_receipt_inventory_transaction_id": scenario.receipt_transaction_id,
                        "destination_pick_face_location_id": destination_id(context, &scenario).await?,
                        "quantity": 8,
                        "priority": 70 - scenario.sequence,
                        "assigned_user_id": null,
                        "due_at": null,
                        "instructions": "Move received stock directly to the forward pick face"
                    }),
                )
                .await?;
            (planned.work_id, "open".to_owned())
        }
    };

    match desired {
        ScenarioState::PlanningReady | ScenarioState::Pending => {}
        ScenarioState::Cancelled => {
            if status != "open" {
                bail!("cross-dock cancellation demo has unsupported status {status}");
            }
            let revision: i64 =
                sqlx::query_scalar("SELECT revision FROM orders WHERE tenant_id=$1 AND id=$2")
                    .bind(context.tenant_id.get())
                    .bind(scenario.order_id)
                    .fetch_one(&context.admin)
                    .await?;
            let cancelled: CancelCrossDockWorkResponse = context
                .command(
                    Method::POST,
                    &format!("/api/v1/cross-dock-tasks/{work_id}/cancellations"),
                    &format!("demo-cross-dock-cancel-{}", scenario.sequence),
                    json!({
                        "expected_order_revision": revision,
                        "reason": "operational_change",
                        "note": "Retained as cancellation history for training"
                    }),
                )
                .await?;
            if cancelled.status != CrossDockWorkStatus::Cancelled {
                bail!("cross-dock cancellation did not become terminal");
            }
        }
        ScenarioState::Completed => {
            if status == "open" {
                let _: CrossDockClaimResponse = context
                    .command(
                        Method::POST,
                        &format!("/api/v1/cross-dock-claims/{work_id}"),
                        &format!("demo-cross-dock-claim-{}", scenario.sequence),
                        json!({}),
                    )
                    .await?;
            }
            let confirmed: ConfirmCrossDockWorkResponse = context
                .command(
                    Method::POST,
                    &format!("/api/v1/cross-dock-tasks/{work_id}/confirmations"),
                    &format!("demo-cross-dock-confirm-{}", scenario.sequence),
                    json!({
                        "source_receiving_location_barcode": scenario.source_barcode,
                        "item_barcode": scenario.item_barcode,
                        "lot_scan": scenario.lot,
                        "serial_scan": null,
                        "destination_pick_face_barcode": scenario.destination_barcode
                    }),
                )
                .await?;
            if confirmed.status != CrossDockWorkStatus::Completed {
                bail!("cross-dock confirmation did not complete");
            }
        }
    }
    Ok(())
}

async fn ensure_scenario(
    context: &SeedContext,
    access: &TenantAccess,
    sequence: i64,
) -> anyhow::Result<Scenario> {
    let source_barcode = format!("WB-DEMO-XD-RECV-{sequence:02}");
    let destination_barcode = format!("WB-DEMO-XD-PICK-{sequence:02}");
    let item_barcode = format!("WB-DEMO-XD-ITEM-{sequence:02}");
    let lot = format!("WB-DEMO-XD-LOT-{sequence:02}");
    let load_reference = format!("WB-DEMO-XD-LOAD-{sequence:02}");
    let order_key = format!("WB-DEMO-XD-ORDER-{sequence:02}");
    let source_location_id = context
        .location(&source_barcode, "receiving", false, true)
        .await?;
    let _destination_location_id = context
        .location(&destination_barcode, "pick", true, false)
        .await?;
    let item_id = match sqlx::query_scalar::<_, i64>(
        "SELECT id FROM items WHERE tenant_id=$1 AND description=$2 AND deleted IS NULL",
    )
    .bind(context.tenant_id.get())
    .bind(format!("Demo cross-dock item {sequence}"))
    .fetch_optional(&context.admin)
    .await?
    {
        Some(id) => id,
        None => {
            context
                .item(&format!("Demo cross-dock item {sequence}"), "case")
                .await?
        }
    };
    let barcode_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM barcodes WHERE tenant_id=$1 AND item_id=$2 AND name=$3 AND deleted IS NULL)",
    )
    .bind(context.tenant_id.get())
    .bind(item_id)
    .bind(&item_barcode)
    .fetch_one(&context.admin)
    .await?;
    if !barcode_exists {
        repo::items::add_barcode(
            &context.db,
            context.tenant_id,
            item_id,
            &item_barcode,
            "code128",
            None,
        )
        .await?;
    }
    let (load_id, load_line_id) =
        ensure_receiving_load(context, item_id, source_location_id, &load_reference, &lot).await?;
    let receipt_transaction_id = ensure_receipt(
        context,
        access,
        load_id,
        load_line_id,
        source_location_id,
        &source_barcode,
        &lot,
        sequence,
    )
    .await?;
    let order_id = match sqlx::query_scalar::<_, i64>(
        "SELECT id FROM orders WHERE tenant_id=$1 AND order_key=$2 AND deleted IS NULL",
    )
    .bind(context.tenant_id.get())
    .bind(&order_key)
    .fetch_optional(&context.admin)
    .await?
    {
        Some(id) => id,
        None => context.order_header(&order_key).await?,
    };
    let order_line_id = match sqlx::query_scalar::<_, i64>(
        "SELECT id FROM order_items WHERE tenant_id=$1 AND order_id=$2 AND item_id=$3 AND deleted IS NULL",
    )
    .bind(context.tenant_id.get())
    .bind(order_id)
    .bind(item_id)
    .fetch_optional(&context.admin)
    .await?
    {
        Some(id) => id,
        None => context.order_item(order_id, item_id, 8).await?,
    };
    let reservation_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM inventory_reservations WHERE tenant_id=$1 AND order_item_id=$2 AND facility_id=$3 AND status='active' AND deleted IS NULL)",
    )
    .bind(context.tenant_id.get())
    .bind(order_line_id)
    .bind(context.facility_id)
    .fetch_one(&context.admin)
    .await?;
    if !reservation_exists {
        repo::inventory::create_inventory_reservation(
            &context.db,
            access,
            &repo::inventory::CreateInventoryReservationCommand {
                order_id,
                order_item_id: order_line_id,
                facility_id: context.facility_id,
                qty: 8,
                idempotency_key: &format!("demo-cross-dock-reservation-{sequence}"),
            },
        )
        .await?;
    }
    Ok(Scenario {
        sequence,
        order_id,
        order_line_id,
        receipt_transaction_id,
        source_barcode,
        destination_barcode,
        item_barcode,
        lot,
    })
}

async fn destination_id(context: &SeedContext, scenario: &Scenario) -> anyhow::Result<i64> {
    sqlx::query_scalar(
        "SELECT id FROM locations WHERE tenant_id=$1 AND barcode=$2 AND deleted IS NULL",
    )
    .bind(context.tenant_id.get())
    .bind(&scenario.destination_barcode)
    .fetch_one(&context.admin)
    .await
    .map_err(Into::into)
}

async fn ensure_receiving_load(
    context: &SeedContext,
    item_id: i64,
    source_location_id: i64,
    load_reference: &str,
    lot: &str,
) -> anyhow::Result<(i64, i64)> {
    let load_id = match sqlx::query_scalar::<_, i64>(
        "SELECT id FROM loads WHERE tenant_id=$1 AND reference_number=$2 AND deleted IS NULL",
    )
    .bind(context.tenant_id.get())
    .bind(load_reference)
    .fetch_optional(&context.admin)
    .await?
    {
        Some(id) => id,
        None => {
            repo::loads::add_load(
                &context.db,
                context.tenant_id,
                context.user_id,
                context.facility_id,
                context.inventory_owner_id,
                LoadType::Inbound,
                Some(load_reference),
                None,
                None,
                None,
                None,
                Some(source_location_id),
                None,
                None,
            )
            .await?
        }
    };
    let line_id = match sqlx::query_scalar::<_, i64>(
        "SELECT id FROM load_lines WHERE tenant_id=$1 AND load_id=$2 AND item_id=$3 ORDER BY id LIMIT 1",
    )
    .bind(context.tenant_id.get())
    .bind(load_id)
    .bind(item_id)
    .fetch_optional(&context.admin)
    .await?
    {
        Some(id) => id,
        None => repo::loads::add_line(
            &context.db,
            context.tenant_id,
            context.user_id,
            load_id,
            item_id,
            None,
            8,
            Some(lot),
            None,
            None,
        )
        .await?,
    };
    Ok((load_id, line_id))
}

async fn ensure_receipt(
    context: &SeedContext,
    access: &TenantAccess,
    load_id: i64,
    load_line_id: i64,
    source_location_id: i64,
    source_barcode: &str,
    lot: &str,
    sequence: i64,
) -> anyhow::Result<i64> {
    if let Some(id) = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM inventory_transactions WHERE tenant_id=$1 AND operation='inbound.receive_expected_inventory.v1' AND reference_type='load_line' AND reference_id=$2 ORDER BY id LIMIT 1",
    )
    .bind(context.tenant_id.get())
    .bind(load_line_id)
    .fetch_optional(&context.admin)
    .await?
    {
        return Ok(id);
    }
    let load = repo::loads::get_load(&context.db, context.tenant_id, load_id, false)
        .await?
        .context("loading demo cross-dock inbound load")?;
    if load.status == LoadStatus::Planned {
        let updated = repo::loads::update_load(
            &context.db,
            context.tenant_id,
            context.user_id,
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
        .await?;
        if !updated {
            bail!("demo cross-dock inbound load could not be marked arrived");
        }
    }
    let load = repo::loads::get_load(&context.db, context.tenant_id, load_id, false)
        .await?
        .context("reloading demo cross-dock inbound load")?;
    if load.status == LoadStatus::Arrived {
        repo::inbound_load::start_inbound_load_unloading(
            &context.db,
            access,
            &CommandContext {
                tenant_id: context.tenant_id,
                actor_id: access.user_id,
                request_id: format!("demo-cross-dock-unload-{sequence}"),
                idempotency_key: Some(format!("demo-cross-dock-unload-{sequence}")),
            },
            &StartInboundLoadUnloadingCommand::new(
                InboundLoadId::new(load_id)?,
                InboundLoadScanValue::new(load.execution_barcode)?,
                InboundLoadScanValue::new(source_barcode)?,
                None,
                None,
            ),
        )
        .await?;
    }
    let result = repo::inbound_receipt::receive_expected_inventory(
        &context.db,
        access,
        &CommandContext {
            tenant_id: context.tenant_id,
            actor_id: access.user_id,
            request_id: format!("demo-cross-dock-receipt-{sequence}"),
            idempotency_key: Some(format!("demo-cross-dock-receipt-{sequence}")),
        },
        load_line_id,
        &repo::inbound_receipt::ReceiveExpectedInventoryCommand {
            receiving_location_id: Some(source_location_id),
            received_qty: 8,
            rejected_qty: 0,
            missing_qty: 0,
            license_plate_id: None,
            license_plate_barcode: None,
            lot: Some(lot),
            serial: None,
            expiration: None,
            exception_reason: None::<InboundReceiptExceptionReason>,
            exception_note: None,
        },
    )
    .await?;
    result
        .inventory_transaction_id
        .context("demo cross-dock receipt did not post inventory")
}
