use anyhow::Context;
use axum::http::Method;
use serde_json::json;
use wareboxes_api_contract::v1::{
    BillableEventResponse, BillingContractResponse, BillingFinancialExportResponse,
    BillingRateResponse, BillingRunResponse, BillingStorageSnapshotResponse,
    ValueAddedWorkResponse, VendorReturnResponse,
};

use super::support::SeedContext;

pub async fn seed(context: &SeedContext) -> anyhow::Result<()> {
    let approver_token = context.configuration_approver_token().await?;
    let contract: BillingContractResponse = context
        .command(
            Method::POST,
            "/api/v1/billing/contracts",
            "demo-billing-contract-create",
            json!({
                "inventory_owner_id":context.inventory_owner_id,
                "contract_number":"NORTHSTAR-2026",
                "currency":"USD",
                "effective_from":"2025-01-01T00:00:00Z"
            }),
        )
        .await?;
    let _: BillingRateResponse = context
        .command(
            Method::POST,
            &format!("/api/v1/billing/contracts/{}/rates", contract.contract_id),
            "demo-billing-accessorial-rate",
            json!({
                "event_type":"accessorial",
                "unit":"event",
                "currency":"USD",
                "rate_minor":125,
                "minimum_charge_minor":500,
                "effective_from":"2025-01-01T00:00:00Z"
            }),
        )
        .await?;
    let _: BillingRateResponse = context
        .command(
            Method::POST,
            &format!("/api/v1/billing/contracts/{}/rates", contract.contract_id),
            "demo-billing-kit-rate",
            json!({
                "event_type":"kit_unit",
                "unit":"each",
                "currency":"USD",
                "rate_minor":175,
                "minimum_charge_minor":0,
                "effective_from":"2025-01-01T00:00:00Z"
            }),
        )
        .await?;
    let _: BillingRateResponse = context
        .command(
            Method::POST,
            &format!("/api/v1/billing/contracts/{}/rates", contract.contract_id),
            "demo-billing-return-rate",
            json!({
                "event_type":"return_unit",
                "unit":"each",
                "currency":"USD",
                "rate_minor":95,
                "minimum_charge_minor":0,
                "effective_from":"2025-01-01T00:00:00Z"
            }),
        )
        .await?;
    let _: BillingRateResponse = context
        .command(
            Method::POST,
            &format!("/api/v1/billing/contracts/{}/rates", contract.contract_id),
            "demo-billing-pallet-day-rate",
            json!({
                "event_type":"pallet_day",
                "unit":"pallet",
                "currency":"USD",
                "rate_minor":20,
                "minimum_charge_minor":0,
                "effective_from":"2025-01-01T00:00:00Z"
            }),
        )
        .await?;
    let active: BillingContractResponse = context
        .command(
            Method::POST,
            &format!(
                "/api/v1/billing/contracts/{}/activations",
                contract.contract_id
            ),
            "demo-billing-contract-activate",
            json!({"expected_revision":contract.revision}),
        )
        .await?;
    seed_value_added_work(context).await?;
    seed_vendor_returns(context).await?;
    let _: BillableEventResponse = context
        .command(
            Method::POST,
            &format!(
                "/api/v1/billing/contracts/{}/billable-events",
                active.contract_id
            ),
            "demo-billing-accessorial-event",
            json!({
                "facility_id":context.facility_id,
                "event_type":"accessorial",
                "unit":"event",
                "quantity":3,
                "source_reference":"DEMO-SPECIAL-HANDLING-01",
                "description":"Retail compliance sort and special handling",
                "occurred_at":"2026-08-11T14:00:00Z"
            }),
        )
        .await?;
    let _: BillingStorageSnapshotResponse = context
        .command(
            Method::POST,
            &format!(
                "/api/v1/billing/contracts/{}/storage-snapshots",
                active.contract_id
            ),
            "demo-billing-storage-snapshot",
            json!({
                "facility_id":context.facility_id,
                "snapshot_date":"2026-08-10"
            }),
        )
        .await?;
    let run: BillingRunResponse = context
        .command(
            Method::POST,
            &format!(
                "/api/v1/billing/contracts/{}/reconciliation-runs",
                active.contract_id
            ),
            "demo-billing-run-generate",
            json!({
                "facility_id":context.facility_id,
                "period_from":"2026-08-01T00:00:00Z",
                "period_until":"2026-08-12T23:59:59Z"
            }),
        )
        .await?;
    let approved: BillingRunResponse = context
        .command_as(
            &approver_token,
            Method::POST,
            &format!("/api/v1/billing/reconciliation-runs/{}/reviews", run.run_id),
            "demo-billing-run-approve",
            json!({
                "expected_revision":run.revision,
                "decision":"approve",
                "note":"Reconciled against demo service and storage evidence"
            }),
        )
        .await?;
    let export: BillingFinancialExportResponse = context
        .command(
            Method::POST,
            &format!("/api/v1/billing/reconciliation-runs/{}/exports", run.run_id),
            "demo-billing-run-export",
            json!({
                "expected_revision":approved.revision,
                "external_batch_key":"DEMO-ERP-2026-08-NORTHSTAR"
            }),
        )
        .await?;
    if export.line_count == 0 || export.content_sha256.len() != 64 {
        anyhow::bail!("demo billing export is incomplete");
    }
    wareboxes_api::auth::destroy_session(&context.db, &approver_token)
        .await
        .context("destroying demo billing approver session")?;
    println!(
        "seeded approved billing run {} and immutable financial export {}",
        run.run_id, export.export_id
    );
    Ok(())
}

async fn seed_value_added_work(context: &SeedContext) -> anyhow::Result<()> {
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM value_added_work_orders WHERE tenant_id=$1 AND work_number='WB-DEMO-VAS-0001')",
    )
    .bind(context.tenant_id.get())
    .fetch_one(&context.admin)
    .await?
    {
        return Ok(());
    }
    let balances: Vec<(i64, i64, Option<i64>, i64)> = sqlx::query_as(
        r#"
        SELECT balance.id,balance.location_id,balance.license_plate_id,balance.item_batch_id
        FROM inventory_balances balance
        WHERE balance.tenant_id=$1 AND balance.inventory_owner_id=$2
          AND balance.facility_id=$3 AND balance.status='available'
          AND balance.deleted IS NULL
          AND balance.qty_on_hand-balance.qty_reserved-balance.qty_held>=3
        ORDER BY balance.id
        LIMIT 3
        "#,
    )
    .bind(context.tenant_id.get())
    .bind(context.inventory_owner_id)
    .bind(context.facility_id)
    .fetch_all(&context.admin)
    .await?;
    if balances.len() < 3 {
        anyhow::bail!("value-added demo requires three available inventory identities");
    }
    for sequence in 1_i64..=3 {
        let created: ValueAddedWorkResponse = context
            .command(
                Method::POST,
                "/api/v1/value-added-work",
                &format!("demo-vas-create-{sequence}"),
                json!({
                    "inventory_owner_id":context.inventory_owner_id,
                    "facility_id":context.facility_id,
                    "number":format!("WB-DEMO-VAS-{sequence:04}"),
                    "kind":if sequence==1 { "value_added_service" } else { "kit" },
                    "note":if sequence==1 { "Retail compliance relabel queue" } else { "Build a two-component promotional kit" },
                    "inputs":if sequence==1 {
                        json!([{"inventory_balance_id":balances[0].0,"quantity":1}])
                    } else {
                        json!([
                            {"inventory_balance_id":balances[0].0,"quantity":1},
                            {"inventory_balance_id":balances[1].0,"quantity":1}
                        ])
                    },
                    "outputs":[{
                        "location_id":balances[2].1,
                        "license_plate_id":balances[2].2,
                        "item_batch_id":balances[2].3,
                        "inventory_status":"available",
                        "quantity":1
                    }]
                }),
            )
            .await?;
        if sequence == 1 {
            continue;
        }
        let released: ValueAddedWorkResponse = context
            .command(
                Method::POST,
                &format!("/api/v1/value-added-work/{}/releases", created.work_id),
                &format!("demo-vas-release-{sequence}"),
                json!({
                    "expected_revision":created.revision,
                    "note":"Components scanned and reserved at the VAS station"
                }),
            )
            .await?;
        if sequence == 3 {
            let _: ValueAddedWorkResponse = context
                .command(
                    Method::POST,
                    &format!("/api/v1/value-added-work/{}/completions", created.work_id),
                    "demo-vas-complete-3",
                    json!({
                        "expected_revision":released.revision,
                        "note":"Kit contents and finished quantity verified"
                    }),
                )
                .await?;
        }
    }
    println!("seeded draft, released, and billed value-added work");
    Ok(())
}

async fn seed_vendor_returns(context: &SeedContext) -> anyhow::Result<()> {
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM vendor_returns WHERE tenant_id=$1 AND return_number='WB-DEMO-RTV-0001')",
    )
    .bind(context.tenant_id.get())
    .fetch_one(&context.admin)
    .await?
    {
        return Ok(());
    }
    let balances: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT balance.id
        FROM inventory_balances balance
        WHERE balance.tenant_id=$1 AND balance.inventory_owner_id=$2
          AND balance.facility_id=$3 AND balance.status='available'
          AND balance.deleted IS NULL
          AND balance.qty_on_hand-balance.qty_reserved-balance.qty_held>=1
        ORDER BY balance.id
        LIMIT 3
        "#,
    )
    .bind(context.tenant_id.get())
    .bind(context.inventory_owner_id)
    .bind(context.facility_id)
    .fetch_all(&context.admin)
    .await?;
    if balances.len() < 3 {
        anyhow::bail!("vendor-return demo requires three available inventory identities");
    }
    for sequence in 1_i64..=3 {
        let created: VendorReturnResponse = context
            .command(
                Method::POST,
                "/api/v1/vendor-returns",
                &format!("demo-vendor-return-create-{sequence}"),
                json!({
                    "inventory_owner_id":context.inventory_owner_id,
                    "facility_id":context.facility_id,
                    "number":format!("WB-DEMO-RTV-{sequence:04}"),
                    "vendor_name":"Acme Component Supply",
                    "vendor_reference":format!("RGA-DEMO-{sequence:04}"),
                    "note":"Supplier-authorized return with item-level quality evidence",
                    "lines":[{
                        "inventory_balance_id":balances[(sequence-1) as usize],
                        "quantity":1,
                        "reason":if sequence==1 { "overstock" } else if sequence==2 { "defective" } else { "recall" },
                        "note":if sequence==1 { "Excess seasonal stock" } else if sequence==2 { "Failed incoming component test" } else { "Supplier recall DEMO-2026-17" }
                    }]
                }),
            )
            .await?;
        if sequence == 1 {
            continue;
        }
        let released: VendorReturnResponse = context
            .command(
                Method::POST,
                &format!(
                    "/api/v1/vendor-returns/{}/releases",
                    created.vendor_return_id
                ),
                &format!("demo-vendor-return-release-{sequence}"),
                json!({
                    "expected_revision":created.revision,
                    "note":"Stock scanned, isolated, and staged against the vendor RGA"
                }),
            )
            .await?;
        if sequence == 3 {
            let _: VendorReturnResponse = context
                .command(
                    Method::POST,
                    &format!(
                        "/api/v1/vendor-returns/{}/shipments",
                        created.vendor_return_id
                    ),
                    "demo-vendor-return-ship-3",
                    json!({
                        "expected_revision":released.revision,
                        "note":"Carrier receipt and trailer departure independently verified"
                    }),
                )
                .await?;
        }
    }
    println!("seeded draft, reserved, and shipped/billed vendor returns");
    Ok(())
}
