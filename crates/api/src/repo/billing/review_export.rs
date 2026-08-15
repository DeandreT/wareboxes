use sha2::{Digest, Sha256};
use wareboxes_application::billing::{
    BillingFinancialExportReadModel, BillingReviewDecision, BillingRunReadModel,
    ExportBillingRunCommand, ReviewBillingRunCommand, EXPORT_BILLING_RUN_OPERATION,
    REVIEW_BILLING_RUN_OPERATION,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{validate_review_separation, BillingFinancialExportId, FacilityId};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use super::models::{financial_export, read_run_tx};
use super::{
    enqueue_event_tx, event_name, internal, require_access_actor, require_owner,
    require_record_scope, unit_name, BillingOutboxEvent, PERMISSION,
};
use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};

fn verify_run_replay(scope: &ScopeBindings, result: &BillingRunReadModel) -> AppResult<()> {
    require_record_scope(scope, result.inventory_owner_id, result.facility_id)
}

pub async fn review_run(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ReviewBillingRunCommand,
) -> AppResult<BillingRunReadModel> {
    require_access_actor(access, context)?;
    let note = command.note.as_deref().map(str::trim);
    if note.is_some_and(|note| note.is_empty() || note.len() > 500) {
        return Err(AppError::bad_request(
            "billing review note must be between 1 and 500 characters",
        ));
    }
    if command.decision == BillingReviewDecision::Reject && note.is_none() {
        return Err(AppError::bad_request("billing rejection requires a note"));
    }
    let prepared = PreparedCommand::new_v1(context, REVIEW_BILLING_RUN_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        verify_run_replay(&scope, &result)?;
        tx.commit().await?;
        return Ok(result);
    }
    sqlx::query(
        "SELECT id FROM billing_reconciliation_runs WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(access.tenant_id.get())
    .bind(command.run_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("billing reconciliation run"))?;
    let current = read_run_tx(&mut tx, access.tenant_id, command.run_id).await?;
    verify_run_replay(&scope, &current)?;
    if current.revision != command.expected_revision {
        return Err(AppError::conflict("billing run revision does not match"));
    }
    validate_review_separation(current.generated_by.get(), context.actor_id.get())
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let status = match command.decision {
        BillingReviewDecision::Approve => {
            current
                .status
                .approve()
                .map_err(|error| AppError::conflict(error.to_string()))?;
            if current.event_count == 0
                || current.unmatched_event_count != 0
                || current.event_count != current.charge_count
            {
                return Err(AppError::conflict(
                    "billing run cannot be approved until every billable event has a charge",
                ));
            }
            "approved"
        }
        BillingReviewDecision::Reject => {
            current
                .status
                .reject()
                .map_err(|error| AppError::conflict(error.to_string()))?;
            "rejected"
        }
    };
    let reviewed_at = now_iso();
    let resulting_revision = current
        .revision
        .checked_add(1)
        .ok_or_else(|| AppError::internal("billing run revision overflow"))?;
    sqlx::query(
        r#"INSERT INTO billing_reviews
             (tenant_id,inventory_owner_id,reconciliation_run_id,decision,note,
              reviewed_by_user_id,reviewed_at,resulting_revision)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8)"#,
    )
    .bind(access.tenant_id.get())
    .bind(current.inventory_owner_id.get())
    .bind(command.run_id.get())
    .bind(status)
    .bind(note)
    .bind(context.actor_id.get())
    .bind(reviewed_at)
    .bind(resulting_revision)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"UPDATE billing_reconciliation_runs SET status=$3,revision=$4,
             reviewed_by_user_id=$5,reviewed_at=$6,review_note=$7
           WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.run_id.get())
    .bind(status)
    .bind(resulting_revision)
    .bind(context.actor_id.get())
    .bind(reviewed_at)
    .bind(note)
    .execute(&mut *tx)
    .await?;
    let result = read_run_tx(&mut tx, access.tenant_id, command.run_id).await?;
    enqueue_event_tx(
        &mut tx,
        BillingOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            owner_id: result.inventory_owner_id,
            facility_id: result.facility_id,
            aggregate_type: "reconciliation_run",
            aggregate_id: command.run_id.get(),
            transition: status,
            occurred_at: reviewed_at,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

fn build_export_csv(run: &BillingRunReadModel) -> String {
    let mut csv = String::from(
        "run_id,attempt,contract_number,inventory_owner_id,facility_id,charge_id,event_type,unit,quantity,rate_minor,minimum_charge_minor,amount_minor,currency,source_type,source_reference,occurred_at\n",
    );
    for charge in &run.charges {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            run.run_id.get(),
            run.attempt,
            csv_field(&run.contract_number),
            run.inventory_owner_id.get(),
            run.facility_id.map(FacilityId::get).unwrap_or_default(),
            charge.charge_id.get(),
            event_name(charge.event_type),
            unit_name(charge.unit),
            charge.quantity,
            charge.rate_minor,
            charge.minimum_charge_minor,
            charge.amount_minor,
            charge.currency,
            csv_field(&charge.source_type),
            csv_field(&charge.source_reference),
            charge.occurred_at.to_rfc3339(),
        ));
    }
    csv
}

pub async fn export_run(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ExportBillingRunCommand,
) -> AppResult<BillingFinancialExportReadModel> {
    require_access_actor(access, context)?;
    let batch_key = command.external_batch_key.trim();
    if batch_key.is_empty() || batch_key.len() > 120 {
        return Err(AppError::bad_request(
            "financial export batch key must be between 1 and 120 characters",
        ));
    }
    let prepared = PreparedCommand::new_v1(context, EXPORT_BILLING_RUN_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    let replay: Option<BillingFinancialExportReadModel> = prepared.replayed(&mut tx).await?;
    if let Some(result) = replay {
        require_owner(&scope, result.inventory_owner_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    sqlx::query(
        "SELECT id FROM billing_reconciliation_runs WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(access.tenant_id.get())
    .bind(command.run_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("billing reconciliation run"))?;
    let current = read_run_tx(&mut tx, access.tenant_id, command.run_id).await?;
    verify_run_replay(&scope, &current)?;
    if current.revision != command.expected_revision {
        return Err(AppError::conflict("billing run revision does not match"));
    }
    current
        .status
        .export()
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let duplicate = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM billing_financial_exports WHERE tenant_id=$1 AND external_batch_key=$2)",
    )
    .bind(access.tenant_id.get())
    .bind(batch_key)
    .fetch_one(&mut *tx)
    .await?;
    if duplicate {
        return Err(AppError::conflict(
            "financial export batch key already exists",
        ));
    }
    let csv_content = build_export_csv(&current);
    let content_sha256 = hex::encode(Sha256::digest(csv_content.as_bytes()));
    let exported_at = now_iso();
    let resulting_revision = current
        .revision
        .checked_add(1)
        .ok_or_else(|| AppError::internal("billing run revision overflow"))?;
    let export_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO billing_financial_exports
             (tenant_id,inventory_owner_id,reconciliation_run_id,external_batch_key,
              content_sha256,line_count,total_minor,currency,csv_content,exported_by_user_id,
              exported_at,resulting_revision)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(current.inventory_owner_id.get())
    .bind(command.run_id.get())
    .bind(batch_key)
    .bind(&content_sha256)
    .bind(i64::try_from(current.charges.len()).map_err(internal)?)
    .bind(i64::try_from(current.total_minor).map_err(internal)?)
    .bind(&current.currency)
    .bind(&csv_content)
    .bind(context.actor_id.get())
    .bind(exported_at)
    .bind(resulting_revision)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        r#"UPDATE billing_reconciliation_runs SET status='exported',revision=$3,exported_at=$4
           WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.run_id.get())
    .bind(resulting_revision)
    .bind(exported_at)
    .execute(&mut *tx)
    .await?;
    let row = sqlx::query("SELECT * FROM billing_financial_exports WHERE tenant_id=$1 AND id=$2")
        .bind(access.tenant_id.get())
        .bind(export_id)
        .fetch_one(&mut *tx)
        .await?;
    let result = financial_export(&row)?;
    enqueue_event_tx(
        &mut tx,
        BillingOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            owner_id: result.inventory_owner_id,
            facility_id: current.facility_id,
            aggregate_type: "financial_export",
            aggregate_id: BillingFinancialExportId::new(export_id)
                .map_err(internal)?
                .get(),
            transition: "created",
            occurred_at: exported_at,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}
