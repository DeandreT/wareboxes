use sqlx::Row;
use wareboxes_application::billing::{
    BillableEventReadModel, BillingChargeReadModel, BillingContractReadModel,
    BillingFinancialExportReadModel, BillingRateReadModel, BillingRunReadModel,
    BillingStorageSnapshotReadModel,
};
use wareboxes_domain::{
    BillableEventId, BillingChargeId, BillingContractId, BillingContractStatus,
    BillingEffectiveWindow, BillingFinancialExportId, BillingQuantity, BillingRateDefinition,
    BillingRateId, BillingReconciliationRunId, BillingRunStatus, BillingStorageSnapshotId,
    CurrencyCode, FacilityId, InventoryOwnerId, TenantId, UserId,
};

use super::{internal, parse_event, parse_unit};
use crate::error::{AppError, AppResult};

fn positive_u64(value: i64, field: &str) -> AppResult<u64> {
    u64::try_from(value).map_err(|_| AppError::internal(format!("invalid stored {field}")))
}

fn optional_user(row: &sqlx::postgres::PgRow, column: &str) -> AppResult<Option<UserId>> {
    row.try_get::<Option<i64>, _>(column)?
        .map(UserId::new)
        .transpose()
        .map_err(internal)
}

fn parse_contract_status(value: &str) -> AppResult<BillingContractStatus> {
    match value {
        "draft" => Ok(BillingContractStatus::Draft),
        "active" => Ok(BillingContractStatus::Active),
        "closed" => Ok(BillingContractStatus::Closed),
        _ => Err(AppError::internal("invalid stored billing contract status")),
    }
}

fn parse_run_status(value: &str) -> AppResult<BillingRunStatus> {
    match value {
        "pending_review" => Ok(BillingRunStatus::PendingReview),
        "approved" => Ok(BillingRunStatus::Approved),
        "rejected" => Ok(BillingRunStatus::Rejected),
        "exported" => Ok(BillingRunStatus::Exported),
        _ => Err(AppError::internal("invalid stored billing run status")),
    }
}

pub(super) fn contract(row: &sqlx::postgres::PgRow) -> AppResult<BillingContractReadModel> {
    Ok(BillingContractReadModel {
        contract_id: BillingContractId::new(row.try_get("id")?).map_err(internal)?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(internal)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        contract_number: row.try_get("contract_number")?,
        currency: row.try_get("currency")?,
        effective_window: BillingEffectiveWindow::new(
            row.try_get("effective_from")?,
            row.try_get("effective_until")?,
        )
        .map_err(internal)?,
        status: parse_contract_status(&row.try_get::<String, _>("status")?)?,
        revision: row.try_get("revision")?,
        created_by: UserId::new(row.try_get("created_by_user_id")?).map_err(internal)?,
        created_at: row.try_get("created_at")?,
        activated_by: optional_user(row, "activated_by_user_id")?,
        activated_at: row.try_get("activated_at")?,
        closed_by: optional_user(row, "closed_by_user_id")?,
        closed_at: row.try_get("closed_at")?,
    })
}

pub(super) fn rate(row: &sqlx::postgres::PgRow) -> AppResult<BillingRateReadModel> {
    let rate_minor = positive_u64(row.try_get("rate_minor")?, "billing rate")?;
    let minimum = positive_u64(row.try_get("minimum_charge_minor")?, "billing minimum")?;
    Ok(BillingRateReadModel {
        rate_id: BillingRateId::new(row.try_get("id")?).map_err(internal)?,
        contract_id: BillingContractId::new(row.try_get("contract_id")?).map_err(internal)?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(internal)?,
        definition: BillingRateDefinition::new(
            parse_event(&row.try_get::<String, _>("event_type")?)?,
            parse_unit(&row.try_get::<String, _>("unit")?)?,
            CurrencyCode::new(row.try_get("currency")?).map_err(internal)?,
            rate_minor,
            minimum,
        )
        .map_err(internal)?,
        effective_window: BillingEffectiveWindow::new(
            row.try_get("effective_from")?,
            row.try_get("effective_until")?,
        )
        .map_err(internal)?,
        revision: row.try_get("revision")?,
        active: row.try_get::<String, _>("status")? == "active",
        created_by: UserId::new(row.try_get("created_by_user_id")?).map_err(internal)?,
        created_at: row.try_get("created_at")?,
    })
}

pub(super) fn event(row: &sqlx::postgres::PgRow) -> AppResult<BillableEventReadModel> {
    Ok(BillableEventReadModel {
        event_id: BillableEventId::new(row.try_get("id")?).map_err(internal)?,
        contract_id: BillingContractId::new(row.try_get("contract_id")?).map_err(internal)?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(internal)?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        event_type: parse_event(&row.try_get::<String, _>("event_type")?)?,
        unit: parse_unit(&row.try_get::<String, _>("unit")?)?,
        quantity: BillingQuantity::new(row.try_get("quantity")?)
            .map_err(internal)?
            .get(),
        source_type: row.try_get("source_type")?,
        source_reference: row.try_get("source_reference")?,
        description: row.try_get("description")?,
        occurred_at: row.try_get("occurred_at")?,
        captured_at: row.try_get("captured_at")?,
    })
}

pub(super) fn snapshot(row: &sqlx::postgres::PgRow) -> AppResult<BillingStorageSnapshotReadModel> {
    Ok(BillingStorageSnapshotReadModel {
        snapshot_id: BillingStorageSnapshotId::new(row.try_get("id")?).map_err(internal)?,
        contract_id: BillingContractId::new(row.try_get("contract_id")?).map_err(internal)?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(internal)?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        snapshot_date: row.try_get("snapshot_date")?,
        pallet_count: row.try_get("pallet_count")?,
        unit_count: row.try_get("unit_count")?,
        captured_at: row.try_get("captured_at")?,
    })
}

pub(super) fn charge(row: &sqlx::postgres::PgRow) -> AppResult<BillingChargeReadModel> {
    Ok(BillingChargeReadModel {
        charge_id: BillingChargeId::new(row.try_get("id")?).map_err(internal)?,
        event_id: BillableEventId::new(row.try_get("billable_event_id")?).map_err(internal)?,
        rate_id: row
            .try_get::<Option<i64>, _>("rate_version_id")?
            .map(BillingRateId::new)
            .transpose()
            .map_err(internal)?,
        decision_policy: super::decision_policy::from_charge_row(row)?,
        event_type: parse_event(&row.try_get::<String, _>("event_type")?)?,
        unit: parse_unit(&row.try_get::<String, _>("unit")?)?,
        quantity: positive_u64(row.try_get("quantity")?, "billing charge quantity")?,
        rate_minor: positive_u64(row.try_get("rate_minor")?, "billing charge rate")?,
        minimum_charge_minor: positive_u64(
            row.try_get("minimum_charge_minor")?,
            "billing charge minimum",
        )?,
        gross_minor: positive_u64(row.try_get("gross_minor")?, "billing charge gross")?,
        amount_minor: positive_u64(row.try_get("amount_minor")?, "billing charge amount")?,
        currency: row.try_get("currency")?,
        source_type: row.try_get("source_type")?,
        source_reference: row.try_get("source_reference")?,
        occurred_at: row.try_get("occurred_at")?,
    })
}

pub(super) async fn read_contract_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    contract_id: BillingContractId,
) -> AppResult<BillingContractReadModel> {
    let row = sqlx::query(
        r#"SELECT contract.*,owner.name AS inventory_owner_name
           FROM billing_contracts contract
           JOIN inventory_owners owner ON owner.tenant_id=contract.tenant_id
             AND owner.id=contract.inventory_owner_id
           WHERE contract.tenant_id=$1 AND contract.id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(contract_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("billing contract"))?;
    contract(&row)
}

pub(super) async fn read_rate_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    rate_id: BillingRateId,
) -> AppResult<BillingRateReadModel> {
    let row = sqlx::query("SELECT * FROM billing_rate_versions WHERE tenant_id=$1 AND id=$2")
        .bind(tenant_id.get())
        .bind(rate_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("billing rate"))?;
    rate(&row)
}

pub(super) async fn read_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    event_id: BillableEventId,
) -> AppResult<BillableEventReadModel> {
    let row = sqlx::query("SELECT * FROM billable_events WHERE tenant_id=$1 AND id=$2")
        .bind(tenant_id.get())
        .bind(event_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("billable event"))?;
    event(&row)
}

pub(super) async fn read_snapshot_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    snapshot_id: BillingStorageSnapshotId,
) -> AppResult<BillingStorageSnapshotReadModel> {
    let row = sqlx::query("SELECT * FROM billing_storage_snapshots WHERE tenant_id=$1 AND id=$2")
        .bind(tenant_id.get())
        .bind(snapshot_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("billing storage snapshot"))?;
    snapshot(&row)
}

pub(super) async fn read_run_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    run_id: BillingReconciliationRunId,
) -> AppResult<BillingRunReadModel> {
    let row = sqlx::query(
        r#"SELECT run.*,owner.name AS inventory_owner_name,
                  contract.contract_number
           FROM billing_reconciliation_runs run
           JOIN inventory_owners owner ON owner.tenant_id=run.tenant_id
             AND owner.id=run.inventory_owner_id
           JOIN billing_contracts contract ON contract.tenant_id=run.tenant_id
             AND contract.id=run.contract_id
           WHERE run.tenant_id=$1 AND run.id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(run_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("billing reconciliation run"))?;
    let charge_rows = sqlx::query(
        "SELECT * FROM billing_charges WHERE tenant_id=$1 AND reconciliation_run_id=$2 ORDER BY id",
    )
    .bind(tenant_id.get())
    .bind(run_id.get())
    .fetch_all(&mut **tx)
    .await?;
    Ok(BillingRunReadModel {
        run_id,
        contract_id: BillingContractId::new(row.try_get("contract_id")?).map_err(internal)?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(internal)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        contract_number: row.try_get("contract_number")?,
        facility_id: row
            .try_get::<Option<i64>, _>("facility_id")?
            .map(FacilityId::new)
            .transpose()
            .map_err(internal)?,
        attempt: row.try_get("attempt")?,
        supersedes_run_id: row
            .try_get::<Option<i64>, _>("supersedes_run_id")?
            .map(BillingReconciliationRunId::new)
            .transpose()
            .map_err(internal)?,
        period_from: row.try_get("period_from")?,
        period_until: row.try_get("period_until")?,
        status: parse_run_status(&row.try_get::<String, _>("status")?)?,
        revision: row.try_get("revision")?,
        event_count: row.try_get("event_count")?,
        charge_count: row.try_get("charge_count")?,
        unmatched_event_count: row.try_get("unmatched_event_count")?,
        total_minor: positive_u64(row.try_get("total_minor")?, "billing total")?,
        currency: row.try_get("currency")?,
        generated_by: UserId::new(row.try_get("generated_by_user_id")?).map_err(internal)?,
        generated_at: row.try_get("generated_at")?,
        reviewed_by: optional_user(&row, "reviewed_by_user_id")?,
        reviewed_at: row.try_get("reviewed_at")?,
        review_note: row.try_get("review_note")?,
        exported_at: row.try_get("exported_at")?,
        charges: charge_rows
            .iter()
            .map(charge)
            .collect::<AppResult<Vec<_>>>()?,
    })
}

pub(super) fn financial_export(
    row: &sqlx::postgres::PgRow,
) -> AppResult<BillingFinancialExportReadModel> {
    Ok(BillingFinancialExportReadModel {
        export_id: BillingFinancialExportId::new(row.try_get("id")?).map_err(internal)?,
        run_id: BillingReconciliationRunId::new(row.try_get("reconciliation_run_id")?)
            .map_err(internal)?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(internal)?,
        external_batch_key: row.try_get("external_batch_key")?,
        content_sha256: row.try_get("content_sha256")?,
        line_count: row.try_get("line_count")?,
        total_minor: positive_u64(row.try_get("total_minor")?, "billing export total")?,
        currency: row.try_get("currency")?,
        csv_content: row.try_get("csv_content")?,
        exported_by: UserId::new(row.try_get("exported_by_user_id")?).map_err(internal)?,
        exported_at: row.try_get("exported_at")?,
        resulting_revision: row.try_get("resulting_revision")?,
    })
}
