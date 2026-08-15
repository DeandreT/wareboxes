//! Immutable operator correction evidence for quarantined order envelopes.

use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::integration::{
    CorrectIntegrationOrderCommand, IntegrationInboxReceipt, IntegrationOrderProcessingResult,
    CORRECT_INTEGRATION_ORDER_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    IntegrationInboxCorrectionId, IntegrationInboxProcessingStatus, NewFulfillmentOrder,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, Db};
use wareboxes_persistence_postgres::idempotency::{insert_result, PostgresPreparedCommandExt};

use super::{
    create_order_for_processing_tx, current_processing_tx, lock_receipt_tx,
    validate_expected_revision, write_outcome_tx, OutcomeIds, OutcomeWrite, ProcessingInput,
    QuarantineReason,
};
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy)]
pub(crate) struct CorrectionInput<'a> {
    pub(crate) command: &'a CorrectIntegrationOrderCommand,
    pub(crate) corrected_payload: &'a [u8],
}

async fn insert_correction_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    receipt: &IntegrationInboxReceipt,
    processing_id: i64,
    input: CorrectionInput<'_>,
) -> AppResult<(IntegrationInboxCorrectionId, wareboxes_domain::Timestamp)> {
    let owner_id = receipt
        .inventory_owner_id
        .ok_or_else(|| AppError::internal("order correction receipt lost owner scope"))?;
    let corrected_at: wareboxes_domain::Timestamp = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?;
    let correction_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO integration_inbox_processing_corrections
            (tenant_id,inventory_owner_id,processing_id,receipt_id,
             expected_revision,resulting_revision,corrected_payload,payload_sha256,
             reason,corrected_by_user_id,corrected_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(owner_id.get())
    .bind(processing_id)
    .bind(receipt.id)
    .bind(input.command.expected_revision().get())
    .bind(input.command.expected_revision().get() + 1)
    .bind(input.corrected_payload)
    .bind(input.command.corrected_payload_sha256().as_slice())
    .bind(input.command.reason().as_str())
    .bind(context.actor_id.get())
    .bind(corrected_at)
    .fetch_one(&mut **tx)
    .await?;
    Ok((
        IntegrationInboxCorrectionId::new(correction_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        corrected_at,
    ))
}

async fn insert_prepared_correction_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    receipt: &IntegrationInboxReceipt,
    input: CorrectionInput<'_>,
) -> AppResult<(super::StoredProcessing, ProcessingInput)> {
    let previous = current_processing_tx(tx, access.tenant_id, receipt.id)
        .await?
        .ok_or_else(|| AppError::conflict("integration inbox receipt is not quarantined"))?;
    validate_expected_revision(Some(&previous), Some(input.command.expected_revision()))?;
    if previous.result.status != IntegrationInboxProcessingStatus::Quarantined {
        return Err(AppError::conflict(
            "processed integration inbox receipt is terminal",
        ));
    }
    let (correction_id, corrected_at) = insert_correction_tx(
        tx,
        access,
        context,
        receipt,
        previous.result.processing_id.get(),
        input,
    )
    .await?;
    Ok((
        previous,
        ProcessingInput {
            payload_sha256: *input.command.corrected_payload_sha256(),
            correction_id: Some(correction_id),
            attempted_at: Some(corrected_at),
        },
    ))
}

pub(crate) async fn correct(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    receipt: &IntegrationInboxReceipt,
    order: &NewFulfillmentOrder,
    input: CorrectionInput<'_>,
) -> AppResult<IntegrationOrderProcessingResult> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    lock_receipt_tx(&mut tx, access, context.actor_id, receipt).await?;
    let prepared =
        PreparedCommand::new_v1(context, CORRECT_INTEGRATION_ORDER_OPERATION, input.command)?;
    if let Some(result) = prepared
        .replayed::<IntegrationOrderProcessingResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    let (previous, processing_input) =
        insert_prepared_correction_tx(&mut tx, access, context, receipt, input).await?;
    let outcome = create_order_for_processing_tx(
        &mut tx,
        access,
        context,
        receipt,
        Some(input.command.expected_revision()),
        order,
    )
    .await?;
    let result = write_outcome_tx(
        &mut tx,
        access,
        OutcomeWrite {
            receipt,
            actor_id: context.actor_id,
            previous: Some(&previous),
            input: processing_input,
            ids: outcome,
            failure: None,
            applied_mappings: &[],
            adapter: None,
        },
    )
    .await?;
    let completed = prepared.completed_result(&result, None)?;
    insert_result(&mut tx, &completed).await?;
    tx.commit().await?;
    Ok(result)
}

pub(crate) async fn quarantine_correction(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    receipt: &IntegrationInboxReceipt,
    input: CorrectionInput<'_>,
    reason: QuarantineReason<'_>,
) -> AppResult<IntegrationOrderProcessingResult> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    lock_receipt_tx(&mut tx, access, context.actor_id, receipt).await?;
    let prepared =
        PreparedCommand::new_v1(context, CORRECT_INTEGRATION_ORDER_OPERATION, input.command)?;
    if let Some(result) = prepared
        .replayed::<IntegrationOrderProcessingResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    let (previous, processing_input) =
        insert_prepared_correction_tx(&mut tx, access, context, receipt, input).await?;
    let result = write_outcome_tx(
        &mut tx,
        access,
        OutcomeWrite {
            receipt,
            actor_id: context.actor_id,
            previous: Some(&previous),
            input: processing_input,
            ids: OutcomeIds {
                order_id: None,
                order_revision: None,
            },
            failure: Some(reason),
            applied_mappings: &[],
            adapter: None,
        },
    )
    .await?;
    let completed = prepared.completed_result(&result, None)?;
    insert_result(&mut tx, &completed).await?;
    tx.commit().await?;
    Ok(result)
}
