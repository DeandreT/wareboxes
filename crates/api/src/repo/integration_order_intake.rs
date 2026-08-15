//! Durable standard-order inbox processing and quarantine evidence.

mod correction;
mod receipt;

pub(crate) use correction::{correct, quarantine_correction, CorrectionInput};
pub(crate) use receipt::{receive_external_order, ExternalOrderReceipt};

use sqlx::postgres::PgRow;
use sqlx::{Acquire, Row};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::integration::{
    IntegrationInboxReceipt, IntegrationOrderEnvelope, IntegrationOrderProcessingResult,
    ReprocessIntegrationOrderCommand, REPROCESS_INTEGRATION_ORDER_OPERATION,
    STANDARD_ORDER_INTAKE_ADAPTER, STANDARD_ORDER_INTAKE_MAPPING_VERSION,
};
use wareboxes_application::{ApplicationError, CommandContext};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    CatalogItemId, ExternalItemKey, ExternalItemUom, FulfillmentOrderDemandLine,
    IntegrationInboxCorrectionId, IntegrationInboxProcessingAttemptId,
    IntegrationInboxProcessingId, IntegrationInboxProcessingRevision,
    IntegrationInboxProcessingStatus, IntegrationOrderItemMappingId,
    IntegrationOrderItemMappingRevision, InventoryOwnerId, NewFulfillmentOrder, OrderId,
    OrderRevision, RequestedUom, TenantId, UserId, MAX_INTEGRATION_PROCESSING_ERROR_CODE_LENGTH,
    MAX_INTEGRATION_PROCESSING_ERROR_MESSAGE_LENGTH,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, Db};
use wareboxes_persistence_postgres::idempotency::{insert_result, PostgresPreparedCommandExt};

use super::access::{lock_current_scope_tx, require_permission_tx};
use super::order_creation;
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AdapterDescriptor {
    pub(crate) key: &'static str,
    pub(crate) mapping_version: i32,
}

pub(crate) const JSON_ORDER_ADAPTER: AdapterDescriptor = AdapterDescriptor {
    key: STANDARD_ORDER_INTAKE_ADAPTER,
    mapping_version: STANDARD_ORDER_INTAKE_MAPPING_VERSION,
};

pub(crate) const X12_940_ORDER_ADAPTER: AdapterDescriptor = AdapterDescriptor {
    key: "x12.940.warehouse_shipping_order",
    mapping_version: 1,
};

fn supported_adapter(key: &str, mapping_version: i32) -> AppResult<AdapterDescriptor> {
    [JSON_ORDER_ADAPTER, X12_940_ORDER_ADAPTER]
        .into_iter()
        .find(|adapter| adapter.key == key && adapter.mapping_version == mapping_version)
        .ok_or_else(|| {
            AppError::conflict(format!(
                "integration receipt uses unsupported adapter {key} version {mapping_version}"
            ))
        })
}

#[derive(Debug, Clone)]
pub(super) struct StoredProcessing {
    result: IntegrationOrderProcessingResult,
}

#[derive(Debug, Clone)]
pub(crate) struct ReprocessingEnvelope {
    pub(crate) receipt: IntegrationInboxReceipt,
    pub(crate) input_payload: Vec<u8>,
    pub(crate) input_payload_sha256: [u8; 32],
    pub(crate) correction_id: Option<IntegrationInboxCorrectionId>,
    pub(crate) adapter: AdapterDescriptor,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProcessingInput {
    pub(crate) payload_sha256: [u8; 32],
    pub(crate) correction_id: Option<IntegrationInboxCorrectionId>,
    pub(crate) attempted_at: Option<wareboxes_domain::Timestamp>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProcessingRequest<'a> {
    receipt: &'a IntegrationInboxReceipt,
    expected_revision: Option<IntegrationInboxProcessingRevision>,
    input: ProcessingInput,
    reprocess: Option<&'a ReprocessIntegrationOrderCommand>,
    adapter: AdapterDescriptor,
}

impl<'a> ProcessingRequest<'a> {
    pub(crate) fn new(
        receipt: &'a IntegrationInboxReceipt,
        expected_revision: Option<IntegrationInboxProcessingRevision>,
        input: ProcessingInput,
        reprocess: Option<&'a ReprocessIntegrationOrderCommand>,
        adapter: AdapterDescriptor,
    ) -> Self {
        Self {
            receipt,
            expected_revision,
            input,
            reprocess,
            adapter,
        }
    }
}

impl ProcessingInput {
    pub(crate) fn retained(receipt: &IntegrationInboxReceipt) -> AppResult<Self> {
        Ok(Self {
            payload_sha256: receipt
                .payload_sha256
                .as_slice()
                .try_into()
                .map_err(|_| AppError::internal("integration inbox payload hash is invalid"))?,
            correction_id: None,
            attempted_at: None,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct OutcomeIds {
    order_id: Option<OrderId>,
    order_revision: Option<OrderRevision>,
}

pub(super) struct OutcomeWrite<'a> {
    receipt: &'a IntegrationInboxReceipt,
    actor_id: UserId,
    previous: Option<&'a StoredProcessing>,
    input: ProcessingInput,
    ids: OutcomeIds,
    failure: Option<QuarantineReason<'a>>,
    applied_mappings: &'a [AppliedMapping],
    adapter: Option<AdapterDescriptor>,
}

#[derive(Debug, Clone)]
pub(super) struct AppliedMapping {
    line_key: String,
    mapping_id: IntegrationOrderItemMappingId,
    mapping_revision: IntegrationOrderItemMappingRevision,
    source_key: String,
    external_item_key: ExternalItemKey,
    external_uom: ExternalItemUom,
    item_id: CatalogItemId,
    requested_uom: RequestedUom,
}

#[derive(Debug, Clone)]
pub(crate) struct QuarantineReason<'a> {
    pub(crate) code: &'a str,
    pub(crate) message: &'a str,
}

fn processing_status(value: &str) -> AppResult<IntegrationInboxProcessingStatus> {
    IntegrationInboxProcessingStatus::parse(value).ok_or_else(|| {
        AppError::internal(format!(
            "database returned invalid integration inbox processing status: {value}"
        ))
    })
}

fn map_processing(row: &PgRow) -> AppResult<StoredProcessing> {
    let status = processing_status(row.try_get::<String, _>("status")?.as_str())?;
    let order_id = row
        .try_get::<Option<i64>, _>("order_id")?
        .map(OrderId::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;
    let order_revision = row
        .try_get::<Option<i64>, _>("order_revision")?
        .map(OrderRevision::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;
    Ok(StoredProcessing {
        result: IntegrationOrderProcessingResult {
            receipt_id: row.try_get("receipt_id")?,
            processing_id: IntegrationInboxProcessingId::new(row.try_get("processing_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            processing_attempt_id: IntegrationInboxProcessingAttemptId::new(
                row.try_get("processing_attempt_id")?,
            )
            .map_err(|error| AppError::internal(error.to_string()))?,
            correction_id: row
                .try_get::<Option<i64>, _>("last_correction_id")?
                .map(IntegrationInboxCorrectionId::new)
                .transpose()
                .map_err(|error| AppError::internal(error.to_string()))?,
            input_payload_sha256: row
                .try_get::<Vec<u8>, _>("last_input_payload_sha256")?
                .try_into()
                .map_err(|_| AppError::internal("integration processing input hash is invalid"))?,
            inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            adapter_key: row.try_get("adapter_key")?,
            mapping_version: row.try_get("mapping_version")?,
            status,
            revision: IntegrationInboxProcessingRevision::new(row.try_get("revision")?)
                .map_err(AppError::internal)?,
            attempt_count: row.try_get("attempt_count")?,
            applied_mapping_count: row.try_get("applied_mapping_count")?,
            order_id,
            order_revision,
            error_code: row.try_get("error_code")?,
            error_message: row.try_get("error_message")?,
            attempted_by: UserId::new(row.try_get("last_attempted_by_user_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            attempted_at: row.try_get("last_attempted_at")?,
            processed_at: row.try_get("processed_at")?,
        },
    })
}

const PROCESSING_SELECT: &str = r#"
    SELECT processing.id AS processing_id, processing.receipt_id,
           processing.inventory_owner_id, processing.adapter_key,
           processing.mapping_version, processing.status, processing.revision,
           processing.attempt_count, processing.order_id,
           processing.order_revision, processing.error_code,
           processing.error_message, processing.last_attempted_by_user_id,
           processing.last_attempted_at, processing.processed_at,
           processing.last_input_payload_sha256,processing.last_correction_id,
           attempt.id AS processing_attempt_id,attempt.applied_mapping_count
    FROM integration_inbox_processings processing
    INNER JOIN integration_inbox_processing_attempts attempt
        ON attempt.tenant_id=processing.tenant_id
       AND attempt.processing_id=processing.id
       AND attempt.attempt_number=processing.attempt_count
    WHERE processing.tenant_id=$1 AND processing.receipt_id=$2
"#;

pub(super) async fn current_processing_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    receipt_id: i64,
) -> AppResult<Option<StoredProcessing>> {
    sqlx::query(PROCESSING_SELECT)
        .bind(tenant_id.get())
        .bind(receipt_id)
        .fetch_optional(&mut **tx)
        .await?
        .as_ref()
        .map(map_processing)
        .transpose()
}

pub(super) async fn lock_receipt_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    actor_id: UserId,
    receipt: &IntegrationInboxReceipt,
) -> AppResult<()> {
    let scope = lock_current_scope_tx(tx, access.tenant_id, actor_id.get()).await?;
    require_permission_tx(tx, access.tenant_id, actor_id.get(), "orders").await?;
    let owner_id = receipt
        .inventory_owner_id
        .ok_or_else(|| AppError::conflict("order intake receipt has no inventory owner scope"))?;
    if !scope.includes_inventory_owner(owner_id.get()) {
        return Err(AppError::not_found("integration inbox receipt"));
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "integration-inbox-processing:{}:{}",
            access.tenant_id, receipt.id
        ))
        .execute(&mut **tx)
        .await?;
    let locked = sqlx::query(
        r#"
        SELECT source_key,deduplication_key,payload_sha256,content_type,facility_id
        FROM integration_inbox_receipts
        WHERE tenant_id=$1 AND id=$2 AND inventory_owner_id=$3
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(receipt.id)
    .bind(owner_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(locked) = locked else {
        return Err(AppError::not_found("integration inbox receipt"));
    };
    let matches = locked.try_get::<String, _>("source_key")? == receipt.source_key
        && locked.try_get::<String, _>("deduplication_key")? == receipt.deduplication_key
        && locked.try_get::<Vec<u8>, _>("payload_sha256")? == receipt.payload_sha256
        && locked.try_get::<String, _>("content_type")? == receipt.content_type
        && locked.try_get::<Option<i64>, _>("facility_id")?.is_none();
    if !matches {
        return Err(AppError::conflict(
            "order intake receipt envelope changed during processing",
        ));
    }
    Ok(())
}

fn validate_failure(failure: &QuarantineReason<'_>) -> AppResult<()> {
    if failure.code.is_empty()
        || failure.code.trim() != failure.code
        || failure.code.chars().count() > MAX_INTEGRATION_PROCESSING_ERROR_CODE_LENGTH
        || failure.message.is_empty()
        || failure.message.trim() != failure.message
        || failure.message.chars().any(char::is_control)
        || failure.message.chars().count() > MAX_INTEGRATION_PROCESSING_ERROR_MESSAGE_LENGTH
    {
        return Err(AppError::internal(
            "integration processing failure diagnostic is invalid",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_applied_mappings_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    receipt: &IntegrationInboxReceipt,
    processing_id: i64,
    processing_attempt_id: i64,
    attempt_number: i32,
    mappings: &[AppliedMapping],
) -> AppResult<()> {
    let owner_id = receipt
        .inventory_owner_id
        .ok_or_else(|| AppError::internal("mapped order receipt lost owner scope"))?;
    for mapping in mappings {
        sqlx::query(
            r#"
            INSERT INTO integration_inbox_processing_attempt_mappings
                (tenant_id,inventory_owner_id,processing_id,processing_attempt_id,
                 receipt_id,attempt_number,line_key,mapping_id,mapping_revision,
                 source_key,external_item_key,external_uom,item_id,requested_uom)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(owner_id.get())
        .bind(processing_id)
        .bind(processing_attempt_id)
        .bind(receipt.id)
        .bind(attempt_number)
        .bind(&mapping.line_key)
        .bind(mapping.mapping_id.get())
        .bind(mapping.mapping_revision.get())
        .bind(&mapping.source_key)
        .bind(mapping.external_item_key.as_str())
        .bind(mapping.external_uom.as_str())
        .bind(mapping.item_id.get())
        .bind(mapping.requested_uom.as_str())
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

pub(super) async fn write_outcome_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    outcome: OutcomeWrite<'_>,
) -> AppResult<IntegrationOrderProcessingResult> {
    let OutcomeWrite {
        receipt,
        actor_id,
        previous,
        input,
        ids: outcome_ids,
        failure,
        applied_mappings,
        adapter,
    } = outcome;
    if let Some(failure) = &failure {
        validate_failure(failure)?;
    }
    let status = if failure.is_some() {
        IntegrationInboxProcessingStatus::Quarantined
    } else {
        IntegrationInboxProcessingStatus::Processed
    };
    let previous_revision = previous.map(|value| value.result.revision.get());
    let revision = previous_revision.map_or(1, |value| value + 1);
    let attempt_count = previous.map_or(1, |value| value.result.attempt_count + 1);
    let attempted_at = if let Some(attempted_at) = input.attempted_at {
        attempted_at
    } else {
        sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut **tx)
            .await?
    };
    let processed_at =
        (status == IntegrationInboxProcessingStatus::Processed).then_some(attempted_at);
    let owner_id = receipt
        .inventory_owner_id
        .ok_or_else(|| AppError::internal("order intake receipt lost inventory owner scope"))?;
    let (adapter_key, mapping_version) = match previous {
        Some(previous) => {
            if adapter.is_some_and(|adapter| {
                adapter.key != previous.result.adapter_key
                    || adapter.mapping_version != previous.result.mapping_version
            }) {
                return Err(AppError::conflict(
                    "integration receipt adapter changed between processing attempts",
                ));
            }
            (
                previous.result.adapter_key.as_str(),
                previous.result.mapping_version,
            )
        }
        None => {
            let adapter = adapter.ok_or_else(|| {
                AppError::internal("initial integration processing has no adapter identity")
            })?;
            (adapter.key, adapter.mapping_version)
        }
    };
    let (error_code, error_message) = failure
        .as_ref()
        .map(|failure| (Some(failure.code), Some(failure.message)))
        .unwrap_or((None, None));

    let processing_id: i64 = if let Some(previous) = previous {
        sqlx::query_scalar(
            r#"
            UPDATE integration_inbox_processings
            SET status=$4,revision=$5,attempt_count=$6,order_id=$7,
                order_revision=$8,error_code=$9,error_message=$10,
                last_attempted_by_user_id=$11,last_attempted_at=$12,
                processed_at=$13,last_input_payload_sha256=$14,last_correction_id=$15
            WHERE tenant_id=$1 AND inventory_owner_id=$2 AND id=$3
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(owner_id.get())
        .bind(previous.result.processing_id.get())
        .bind(status.as_str())
        .bind(revision)
        .bind(attempt_count)
        .bind(outcome_ids.order_id.map(OrderId::get))
        .bind(outcome_ids.order_revision.map(OrderRevision::get))
        .bind(error_code)
        .bind(error_message)
        .bind(actor_id.get())
        .bind(attempted_at)
        .bind(processed_at)
        .bind(input.payload_sha256.as_slice())
        .bind(input.correction_id.map(IntegrationInboxCorrectionId::get))
        .fetch_one(&mut **tx)
        .await?
    } else {
        sqlx::query_scalar(
            r#"
            INSERT INTO integration_inbox_processings
                (tenant_id,inventory_owner_id,receipt_id,source_key,
                 deduplication_key,payload_sha256,last_input_payload_sha256,
                 last_correction_id,adapter_key,mapping_version,
                 status,revision,attempt_count,order_id,order_revision,error_code,
                 error_message,last_attempted_by_user_id,last_attempted_at,processed_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,NULL,$8,$9,$10,1,1,$11,$12,$13,$14,$15,$16,$17)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(owner_id.get())
        .bind(receipt.id)
        .bind(&receipt.source_key)
        .bind(&receipt.deduplication_key)
        .bind(&receipt.payload_sha256)
        .bind(input.payload_sha256.as_slice())
        .bind(adapter_key)
        .bind(mapping_version)
        .bind(status.as_str())
        .bind(outcome_ids.order_id.map(OrderId::get))
        .bind(outcome_ids.order_revision.map(OrderRevision::get))
        .bind(error_code)
        .bind(error_message)
        .bind(actor_id.get())
        .bind(attempted_at)
        .bind(processed_at)
        .fetch_one(&mut **tx)
        .await?
    };

    let attempt_id: i64 =
        sqlx::query_scalar(
            r#"
        INSERT INTO integration_inbox_processing_attempts
            (tenant_id,inventory_owner_id,processing_id,receipt_id,
             attempt_number,previous_revision,resulting_revision,input_payload_sha256,
             correction_id,outcome,
             order_id,order_revision,error_code,error_message,
             attempted_by_user_id,attempted_at,applied_mapping_count)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
        RETURNING id
        "#,
        )
        .bind(access.tenant_id.get())
        .bind(owner_id.get())
        .bind(processing_id)
        .bind(receipt.id)
        .bind(attempt_count)
        .bind(previous_revision)
        .bind(revision)
        .bind(input.payload_sha256.as_slice())
        .bind(input.correction_id.map(IntegrationInboxCorrectionId::get))
        .bind(status.as_str())
        .bind(outcome_ids.order_id.map(OrderId::get))
        .bind(outcome_ids.order_revision.map(OrderRevision::get))
        .bind(error_code)
        .bind(error_message)
        .bind(actor_id.get())
        .bind(attempted_at)
        .bind(i32::try_from(applied_mappings.len()).map_err(|_| {
            AppError::bad_request("integration order contains too many mapped lines")
        })?)
        .fetch_one(&mut **tx)
        .await?;

    insert_applied_mappings_tx(
        tx,
        access,
        receipt,
        processing_id,
        attempt_id,
        attempt_count,
        applied_mappings,
    )
    .await?;

    Ok(IntegrationOrderProcessingResult {
        receipt_id: receipt.id,
        processing_id: IntegrationInboxProcessingId::new(processing_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        processing_attempt_id: IntegrationInboxProcessingAttemptId::new(attempt_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        correction_id: input.correction_id,
        input_payload_sha256: input.payload_sha256,
        inventory_owner_id: owner_id,
        adapter_key: adapter_key.into(),
        mapping_version,
        status,
        revision: IntegrationInboxProcessingRevision::new(revision).map_err(AppError::internal)?,
        attempt_count,
        applied_mapping_count: i32::try_from(applied_mappings.len())
            .map_err(|_| AppError::internal("applied mapping count overflow"))?,
        order_id: outcome_ids.order_id,
        order_revision: outcome_ids.order_revision,
        error_code: error_code.map(str::to_owned),
        error_message: error_message.map(str::to_owned),
        attempted_by: actor_id,
        attempted_at,
        processed_at,
    })
}

pub async fn current_processing(
    db: &Db,
    access: &TenantAccess,
    receipt: &IntegrationInboxReceipt,
) -> AppResult<Option<IntegrationOrderProcessingResult>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    lock_receipt_tx(&mut tx, access, access.user_id, receipt).await?;
    let result = current_processing_tx(&mut tx, access.tenant_id, receipt.id)
        .await?
        .map(|stored| stored.result);
    tx.commit().await?;
    Ok(result)
}

pub(crate) async fn quarantine(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    request: ProcessingRequest<'_>,
    reason: QuarantineReason<'_>,
) -> AppResult<IntegrationOrderProcessingResult> {
    let prepared = request
        .reprocess
        .map(|command| {
            PreparedCommand::new_v1(context, REPROCESS_INTEGRATION_ORDER_OPERATION, command)
        })
        .transpose()?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    lock_receipt_tx(&mut tx, access, context.actor_id, request.receipt).await?;
    if let Some(prepared) = &prepared {
        if let Some(result) = prepared
            .replayed::<IntegrationOrderProcessingResult>(&mut tx)
            .await?
        {
            tx.commit().await?;
            return Ok(result);
        }
    }
    let previous = current_processing_tx(&mut tx, access.tenant_id, request.receipt.id).await?;
    if request.reprocess.is_none() {
        if let Some(previous) = previous {
            tx.commit().await?;
            return Ok(previous.result);
        }
    }
    validate_expected_revision(previous.as_ref(), request.expected_revision)?;
    let result = write_outcome_tx(
        &mut tx,
        access,
        OutcomeWrite {
            receipt: request.receipt,
            actor_id: context.actor_id,
            previous: previous.as_ref(),
            input: request.input,
            ids: OutcomeIds {
                order_id: None,
                order_revision: None,
            },
            failure: Some(reason),
            applied_mappings: &[],
            adapter: Some(request.adapter),
        },
    )
    .await?;
    if let Some(prepared) = prepared {
        let completed = prepared.completed_result(&result, None)?;
        insert_result(&mut tx, &completed).await?;
    }
    tx.commit().await?;
    Ok(result)
}

async fn resolve_order_mappings_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    receipt: &IntegrationInboxReceipt,
    envelope: &IntegrationOrderEnvelope,
) -> AppResult<Result<Vec<AppliedMapping>, String>> {
    let owner_id = receipt
        .inventory_owner_id
        .ok_or_else(|| AppError::internal("mapped order receipt lost owner scope"))?;
    if envelope.inventory_owner_id != owner_id {
        return Err(AppError::not_found("integration inbox receipt"));
    }
    let mut lock_keys = envelope
        .lines
        .iter()
        .map(|line| {
            super::integration_mapping::natural_lock_key(
                access.tenant_id,
                owner_id,
                &receipt.source_key,
                line.external_item_key.as_str(),
                line.external_uom.as_str(),
            )
        })
        .collect::<Vec<_>>();
    lock_keys.sort_unstable();
    lock_keys.dedup();
    for key in lock_keys {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(key)
            .execute(&mut **tx)
            .await?;
    }

    let mut mappings = Vec::with_capacity(envelope.lines.len());
    for line in &envelope.lines {
        let row = sqlx::query(
            r#"
            SELECT mapping.id,mapping.revision,mapping.item_id,mapping.requested_uom
            FROM integration_order_item_mappings mapping
            JOIN inventory_owner_items owner_item
              ON owner_item.tenant_id=mapping.tenant_id
             AND owner_item.inventory_owner_id=mapping.inventory_owner_id
             AND owner_item.item_id=mapping.item_id
            JOIN items item
              ON item.tenant_id=owner_item.tenant_id AND item.id=owner_item.item_id
            WHERE mapping.tenant_id=$1 AND mapping.inventory_owner_id=$2
              AND mapping.source_key=$3 AND mapping.external_item_key=$4
              AND mapping.external_uom=$5 AND mapping.effective_to IS NULL
              AND owner_item.deleted IS NULL AND item.deleted IS NULL
              AND item.packaging_unit=mapping.requested_uom
            FOR SHARE OF mapping,owner_item,item
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(owner_id.get())
        .bind(&receipt.source_key)
        .bind(line.external_item_key.as_str())
        .bind(line.external_uom.as_str())
        .fetch_optional(&mut **tx)
        .await?;
        let Some(row) = row else {
            return Ok(Err(format!(
                "no active item mapping for line {} ({} / {})",
                line.line_key, line.external_item_key, line.external_uom
            )));
        };
        mappings.push(AppliedMapping {
            line_key: line.line_key.as_str().to_owned(),
            mapping_id: IntegrationOrderItemMappingId::new(row.try_get("id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            mapping_revision: IntegrationOrderItemMappingRevision::new(row.try_get("revision")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            source_key: receipt.source_key.clone(),
            external_item_key: line.external_item_key.clone(),
            external_uom: line.external_uom.clone(),
            item_id: CatalogItemId::new(row.try_get("item_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            requested_uom: RequestedUom::new(row.try_get::<String, _>("requested_uom")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
        });
    }
    Ok(Ok(mappings))
}

fn mapped_order(
    envelope: &IntegrationOrderEnvelope,
    mappings: &[AppliedMapping],
) -> AppResult<NewFulfillmentOrder> {
    let lines = envelope
        .lines
        .iter()
        .zip(mappings)
        .map(|(line, mapping)| {
            FulfillmentOrderDemandLine::new(
                line.line_key.clone(),
                mapping.item_id,
                line.quantity,
                mapping.requested_uom.clone(),
            )
        })
        .collect();
    NewFulfillmentOrder::new(
        envelope.inventory_owner_id,
        envelope.order_key.clone(),
        envelope.rush,
        envelope.ship_by,
        envelope.destination.clone(),
        lines,
    )
    .map_err(|error| AppError::bad_request(error.to_string()))
}

fn quarantinable_message(error: &AppError) -> Option<String> {
    let message = match error.public_application_error() {
        ApplicationError::NotFound(resource) => format!("not found: {resource}"),
        ApplicationError::Validation(details) => details
            .into_iter()
            .map(|detail| format!("{}: {}", detail.field, detail.message))
            .collect::<Vec<_>>()
            .join("; "),
        ApplicationError::Conflict(message) | ApplicationError::InvalidRequest(message) => message,
        _ => return None,
    };
    let clean = message
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(MAX_INTEGRATION_PROCESSING_ERROR_MESSAGE_LENGTH)
        .collect::<String>();
    Some(if clean.trim().is_empty() {
        "order intake was rejected by current business rules".into()
    } else {
        clean.trim().to_owned()
    })
}

pub(crate) async fn process_external(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    request: ProcessingRequest<'_>,
    envelope: &IntegrationOrderEnvelope,
) -> AppResult<IntegrationOrderProcessingResult> {
    let prepared = request
        .reprocess
        .map(|command| {
            PreparedCommand::new_v1(context, REPROCESS_INTEGRATION_ORDER_OPERATION, command)
        })
        .transpose()?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    lock_receipt_tx(&mut tx, access, context.actor_id, request.receipt).await?;
    if let Some(prepared) = &prepared {
        if let Some(result) = prepared
            .replayed::<IntegrationOrderProcessingResult>(&mut tx)
            .await?
        {
            tx.commit().await?;
            return Ok(result);
        }
    }
    let previous = current_processing_tx(&mut tx, access.tenant_id, request.receipt.id).await?;
    if request.reprocess.is_none() {
        if let Some(previous) = previous {
            tx.commit().await?;
            return Ok(previous.result);
        }
    }
    validate_expected_revision(previous.as_ref(), request.expected_revision)?;
    if previous
        .as_ref()
        .is_some_and(|value| value.result.status == IntegrationInboxProcessingStatus::Processed)
    {
        return Err(AppError::conflict(
            "processed integration inbox receipt is terminal",
        ));
    }

    let mappings =
        match resolve_order_mappings_tx(&mut tx, access, request.receipt, envelope).await? {
            Ok(mappings) => mappings,
            Err(message) => {
                let result = write_outcome_tx(
                    &mut tx,
                    access,
                    OutcomeWrite {
                        receipt: request.receipt,
                        actor_id: context.actor_id,
                        previous: previous.as_ref(),
                        input: request.input,
                        ids: OutcomeIds {
                            order_id: None,
                            order_revision: None,
                        },
                        failure: Some(QuarantineReason {
                            code: "item_mapping_not_found",
                            message: &message,
                        }),
                        applied_mappings: &[],
                        adapter: Some(request.adapter),
                    },
                )
                .await?;
                if let Some(prepared) = prepared {
                    let completed = prepared.completed_result(&result, None)?;
                    insert_result(&mut tx, &completed).await?;
                }
                tx.commit().await?;
                return Ok(result);
            }
        };
    let order = mapped_order(envelope, &mappings)?;
    let mut savepoint = tx.begin().await?;
    let creation = create_order_for_processing_tx(
        &mut savepoint,
        access,
        context,
        request.receipt,
        request.expected_revision,
        &order,
    )
    .await;
    let (outcome_ids, failure_message) = match creation {
        Ok(ids) => {
            savepoint.commit().await?;
            (ids, None)
        }
        Err(error) => {
            savepoint.rollback().await?;
            let Some(message) = quarantinable_message(&error) else {
                return Err(error);
            };
            (
                OutcomeIds {
                    order_id: None,
                    order_revision: None,
                },
                Some(message),
            )
        }
    };
    let result = write_outcome_tx(
        &mut tx,
        access,
        OutcomeWrite {
            receipt: request.receipt,
            actor_id: context.actor_id,
            previous: previous.as_ref(),
            input: request.input,
            ids: outcome_ids,
            failure: failure_message.as_deref().map(|message| QuarantineReason {
                code: "business_rejected",
                message,
            }),
            applied_mappings: &mappings,
            adapter: Some(request.adapter),
        },
    )
    .await?;
    if let Some(prepared) = prepared {
        let completed = prepared.completed_result(&result, None)?;
        insert_result(&mut tx, &completed).await?;
    }
    tx.commit().await?;
    Ok(result)
}

pub(crate) async fn process_internal(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    request: ProcessingRequest<'_>,
    order: &NewFulfillmentOrder,
) -> AppResult<IntegrationOrderProcessingResult> {
    let prepared = request
        .reprocess
        .map(|command| {
            PreparedCommand::new_v1(context, REPROCESS_INTEGRATION_ORDER_OPERATION, command)
        })
        .transpose()?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    lock_receipt_tx(&mut tx, access, context.actor_id, request.receipt).await?;
    if let Some(prepared) = &prepared {
        if let Some(result) = prepared
            .replayed::<IntegrationOrderProcessingResult>(&mut tx)
            .await?
        {
            tx.commit().await?;
            return Ok(result);
        }
    }
    let previous = current_processing_tx(&mut tx, access.tenant_id, request.receipt.id).await?;
    validate_expected_revision(previous.as_ref(), request.expected_revision)?;
    if previous
        .as_ref()
        .is_some_and(|value| value.result.status == IntegrationInboxProcessingStatus::Processed)
    {
        return Err(AppError::conflict(
            "processed integration inbox receipt is terminal",
        ));
    }
    let outcome_ids = create_order_for_processing_tx(
        &mut tx,
        access,
        context,
        request.receipt,
        request.expected_revision,
        order,
    )
    .await?;
    let result = write_outcome_tx(
        &mut tx,
        access,
        OutcomeWrite {
            receipt: request.receipt,
            actor_id: context.actor_id,
            previous: previous.as_ref(),
            input: request.input,
            ids: outcome_ids,
            failure: None,
            applied_mappings: &[],
            adapter: Some(request.adapter),
        },
    )
    .await?;
    if let Some(prepared) = prepared {
        let completed = prepared.completed_result(&result, None)?;
        insert_result(&mut tx, &completed).await?;
    }
    tx.commit().await?;
    Ok(result)
}

pub(super) async fn create_order_for_processing_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    receipt: &IntegrationInboxReceipt,
    expected_revision: Option<IntegrationInboxProcessingRevision>,
    order: &NewFulfillmentOrder,
) -> AppResult<OutcomeIds> {
    let order_context = CommandContext {
        tenant_id: context.tenant_id,
        actor_id: context.actor_id,
        request_id: context.request_id.clone(),
        idempotency_key: Some(format!(
            "integration-order:{}:{}",
            receipt.id,
            expected_revision.map_or(1, |revision| revision.get() + 1)
        )),
    };
    let order_result =
        order_creation::create_fulfillment_order_tx(tx, access, &order_context, order).await?;
    let order_id = OrderId::new(order_result.order_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let order_revision = OrderRevision::new(order_result.revision)
        .map_err(|error| AppError::internal(error.to_string()))?;
    Ok(OutcomeIds {
        order_id: Some(order_id),
        order_revision: Some(order_revision),
    })
}

pub(super) fn validate_expected_revision(
    previous: Option<&StoredProcessing>,
    expected: Option<IntegrationInboxProcessingRevision>,
) -> AppResult<()> {
    match (previous, expected) {
        (None, None) => Ok(()),
        (Some(previous), Some(expected)) if previous.result.revision == expected => Ok(()),
        (None, Some(_)) => Err(AppError::conflict(
            "integration inbox receipt has no processing state to reprocess",
        )),
        (Some(_), None) => Err(AppError::conflict(
            "integration inbox processing already exists",
        )),
        (Some(_), Some(_)) => Err(AppError::conflict(
            "integration inbox processing revision is stale",
        )),
    }
}

pub(crate) async fn receipt_for_reprocessing(
    db: &Db,
    access: &TenantAccess,
    receipt_id: i64,
) -> AppResult<Option<ReprocessingEnvelope>> {
    if receipt_id <= 0 {
        return Err(AppError::bad_request(
            "integration inbox receipt ID must be positive",
        ));
    }
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "orders").await?;
    let row = sqlx::query(
        r#"
        SELECT receipt.id,receipt.tenant_id,receipt.inventory_owner_id,
               receipt.facility_id,receipt.received_at,receipt.source_key,
               receipt.deduplication_key,receipt.content_type,receipt.raw_payload,
               receipt.payload_sha256,receipt.request_id,
               receipt.external_inventory_owner_key,receipt.owner_mapping_id,
               receipt.owner_mapping_revision,
               COALESCE(correction.corrected_payload,receipt.raw_payload) AS input_payload,
               COALESCE(correction.payload_sha256,receipt.payload_sha256) AS input_payload_sha256,
               processing.last_correction_id,processing.adapter_key,
               processing.mapping_version
        FROM integration_inbox_receipts receipt
        LEFT JOIN integration_inbox_processings processing
          ON processing.tenant_id=receipt.tenant_id AND processing.receipt_id=receipt.id
        LEFT JOIN integration_inbox_processing_corrections correction
          ON correction.tenant_id=processing.tenant_id
         AND correction.id=processing.last_correction_id
        WHERE receipt.tenant_id=$1 AND receipt.id=$2
          AND receipt.inventory_owner_id IS NOT NULL
          AND receipt.facility_id IS NULL
          AND ($3 OR receipt.inventory_owner_id=ANY($4))
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(receipt_id)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?;
    let receipt = row
        .map(|row| {
            let receipt = IntegrationInboxReceipt {
                id: row.try_get("id")?,
                tenant_id: TenantId::new(row.try_get("tenant_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                inventory_owner_id: row
                    .try_get::<Option<i64>, _>("inventory_owner_id")?
                    .map(InventoryOwnerId::new)
                    .transpose()
                    .map_err(|error| AppError::internal(error.to_string()))?,
                facility_id: None,
                owner_mapping: match (
                    row.try_get::<Option<String>, _>("external_inventory_owner_key")?,
                    row.try_get::<Option<i64>, _>("owner_mapping_id")?,
                    row.try_get::<Option<i64>, _>("owner_mapping_revision")?,
                ) {
                    (None, None, None) => None,
                    (Some(external_key), Some(mapping_id), Some(mapping_revision)) => Some(
                        wareboxes_application::integration::IntegrationInboxOwnerMappingEvidence {
                            external_inventory_owner_key:
                                wareboxes_domain::ExternalInventoryOwnerKey::new(external_key)
                                    .map_err(|error| AppError::internal(error.to_string()))?,
                            mapping_id: wareboxes_domain::IntegrationOrderOwnerMappingId::new(
                                mapping_id,
                            )
                            .map_err(|error| AppError::internal(error.to_string()))?,
                            mapping_revision:
                                wareboxes_domain::IntegrationOrderOwnerMappingRevision::new(
                                    mapping_revision,
                                )
                                .map_err(|error| AppError::internal(error.to_string()))?,
                        },
                    ),
                    _ => {
                        return Err(AppError::internal(
                            "integration inbox owner mapping evidence is incomplete",
                        ));
                    }
                },
                received_at: row.try_get("received_at")?,
                source_key: row.try_get("source_key")?,
                deduplication_key: row.try_get("deduplication_key")?,
                content_type: row.try_get("content_type")?,
                raw_payload: row.try_get("raw_payload")?,
                payload_sha256: row.try_get("payload_sha256")?,
                request_id: row.try_get("request_id")?,
            };
            let adapter_key = row
                .try_get::<Option<String>, _>("adapter_key")?
                .ok_or_else(|| AppError::conflict("integration inbox receipt has no processing"))?;
            let mapping_version = row
                .try_get::<Option<i32>, _>("mapping_version")?
                .ok_or_else(|| AppError::conflict("integration inbox receipt has no processing"))?;
            Ok::<_, AppError>(ReprocessingEnvelope {
                receipt,
                input_payload: row.try_get("input_payload")?,
                input_payload_sha256: row
                    .try_get::<Vec<u8>, _>("input_payload_sha256")?
                    .try_into()
                    .map_err(|_| {
                        AppError::internal("integration processing input hash is invalid")
                    })?,
                correction_id: row
                    .try_get::<Option<i64>, _>("last_correction_id")?
                    .map(IntegrationInboxCorrectionId::new)
                    .transpose()
                    .map_err(|error| AppError::internal(error.to_string()))?,
                adapter: supported_adapter(&adapter_key, mapping_version)?,
            })
        })
        .transpose()?;
    tx.commit().await?;
    Ok(receipt)
}
