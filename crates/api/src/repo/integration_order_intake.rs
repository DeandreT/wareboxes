//! Durable standard-order inbox processing and quarantine evidence.

use sqlx::postgres::PgRow;
use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::integration::{
    IntegrationInboxReceipt, IntegrationOrderProcessingResult, ReprocessIntegrationOrderCommand,
    REPROCESS_INTEGRATION_ORDER_OPERATION, STANDARD_ORDER_INTAKE_ADAPTER,
    STANDARD_ORDER_INTAKE_MAPPING_VERSION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    IntegrationInboxProcessingAttemptId, IntegrationInboxProcessingId,
    IntegrationInboxProcessingRevision, IntegrationInboxProcessingStatus, InventoryOwnerId,
    NewFulfillmentOrder, OrderId, OrderRevision, TenantId, UserId,
    MAX_INTEGRATION_PROCESSING_ERROR_CODE_LENGTH, MAX_INTEGRATION_PROCESSING_ERROR_MESSAGE_LENGTH,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, Db};
use wareboxes_persistence_postgres::idempotency::{insert_result, PostgresPreparedCommandExt};

use super::access::{lock_current_scope_tx, require_permission_tx};
use super::order_creation;
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone)]
struct StoredProcessing {
    result: IntegrationOrderProcessingResult,
}

#[derive(Debug, Clone, Copy)]
struct OutcomeIds {
    order_id: Option<OrderId>,
    order_revision: Option<OrderRevision>,
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
            inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            adapter_key: row.try_get("adapter_key")?,
            mapping_version: row.try_get("mapping_version")?,
            status,
            revision: IntegrationInboxProcessingRevision::new(row.try_get("revision")?)
                .map_err(AppError::internal)?,
            attempt_count: row.try_get("attempt_count")?,
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
           attempt.id AS processing_attempt_id
    FROM integration_inbox_processings processing
    INNER JOIN integration_inbox_processing_attempts attempt
        ON attempt.tenant_id=processing.tenant_id
       AND attempt.processing_id=processing.id
       AND attempt.attempt_number=processing.attempt_count
    WHERE processing.tenant_id=$1 AND processing.receipt_id=$2
"#;

async fn current_processing_tx(
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

async fn lock_receipt_tx(
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

async fn write_outcome_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    receipt: &IntegrationInboxReceipt,
    actor_id: UserId,
    previous: Option<&StoredProcessing>,
    outcome_ids: OutcomeIds,
    failure: Option<QuarantineReason<'_>>,
) -> AppResult<IntegrationOrderProcessingResult> {
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
    let attempted_at: wareboxes_domain::Timestamp = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?;
    let processed_at =
        (status == IntegrationInboxProcessingStatus::Processed).then_some(attempted_at);
    let owner_id = receipt
        .inventory_owner_id
        .ok_or_else(|| AppError::internal("order intake receipt lost inventory owner scope"))?;
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
                processed_at=$13
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
        .fetch_one(&mut **tx)
        .await?
    } else {
        sqlx::query_scalar(
            r#"
            INSERT INTO integration_inbox_processings
                (tenant_id,inventory_owner_id,receipt_id,source_key,
                 deduplication_key,payload_sha256,adapter_key,mapping_version,
                 status,revision,attempt_count,order_id,order_revision,error_code,
                 error_message,last_attempted_by_user_id,last_attempted_at,processed_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,1,1,$10,$11,$12,$13,$14,$15,$16)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(owner_id.get())
        .bind(receipt.id)
        .bind(&receipt.source_key)
        .bind(&receipt.deduplication_key)
        .bind(&receipt.payload_sha256)
        .bind(STANDARD_ORDER_INTAKE_ADAPTER)
        .bind(STANDARD_ORDER_INTAKE_MAPPING_VERSION)
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

    let attempt_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO integration_inbox_processing_attempts
            (tenant_id,inventory_owner_id,processing_id,receipt_id,
             attempt_number,previous_revision,resulting_revision,outcome,
             order_id,order_revision,error_code,error_message,
             attempted_by_user_id,attempted_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
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
    .bind(status.as_str())
    .bind(outcome_ids.order_id.map(OrderId::get))
    .bind(outcome_ids.order_revision.map(OrderRevision::get))
    .bind(error_code)
    .bind(error_message)
    .bind(actor_id.get())
    .bind(attempted_at)
    .fetch_one(&mut **tx)
    .await?;

    Ok(IntegrationOrderProcessingResult {
        receipt_id: receipt.id,
        processing_id: IntegrationInboxProcessingId::new(processing_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        processing_attempt_id: IntegrationInboxProcessingAttemptId::new(attempt_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: owner_id,
        adapter_key: STANDARD_ORDER_INTAKE_ADAPTER.into(),
        mapping_version: STANDARD_ORDER_INTAKE_MAPPING_VERSION,
        status,
        revision: IntegrationInboxProcessingRevision::new(revision).map_err(AppError::internal)?,
        attempt_count,
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
    receipt: &IntegrationInboxReceipt,
    expected_revision: Option<IntegrationInboxProcessingRevision>,
    reason: QuarantineReason<'_>,
    reprocess: Option<&ReprocessIntegrationOrderCommand>,
) -> AppResult<IntegrationOrderProcessingResult> {
    let prepared = reprocess
        .map(|command| {
            PreparedCommand::new_v1(context, REPROCESS_INTEGRATION_ORDER_OPERATION, command)
        })
        .transpose()?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    lock_receipt_tx(&mut tx, access, context.actor_id, receipt).await?;
    if let Some(prepared) = &prepared {
        if let Some(result) = prepared
            .replayed::<IntegrationOrderProcessingResult>(&mut tx)
            .await?
        {
            tx.commit().await?;
            return Ok(result);
        }
    }
    let previous = current_processing_tx(&mut tx, access.tenant_id, receipt.id).await?;
    if reprocess.is_none() {
        if let Some(previous) = previous {
            tx.commit().await?;
            return Ok(previous.result);
        }
    }
    validate_expected_revision(previous.as_ref(), expected_revision)?;
    let result = write_outcome_tx(
        &mut tx,
        access,
        receipt,
        context.actor_id,
        previous.as_ref(),
        OutcomeIds {
            order_id: None,
            order_revision: None,
        },
        Some(reason),
    )
    .await?;
    if let Some(prepared) = prepared {
        let completed = prepared.completed_result(&result, None)?;
        insert_result(&mut tx, &completed).await?;
    }
    tx.commit().await?;
    Ok(result)
}

pub async fn process(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    receipt: &IntegrationInboxReceipt,
    order: &NewFulfillmentOrder,
    expected_revision: Option<IntegrationInboxProcessingRevision>,
    reprocess: Option<&ReprocessIntegrationOrderCommand>,
) -> AppResult<IntegrationOrderProcessingResult> {
    let prepared = reprocess
        .map(|command| {
            PreparedCommand::new_v1(context, REPROCESS_INTEGRATION_ORDER_OPERATION, command)
        })
        .transpose()?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    lock_receipt_tx(&mut tx, access, context.actor_id, receipt).await?;
    if let Some(prepared) = &prepared {
        if let Some(result) = prepared
            .replayed::<IntegrationOrderProcessingResult>(&mut tx)
            .await?
        {
            tx.commit().await?;
            return Ok(result);
        }
    }
    let previous = current_processing_tx(&mut tx, access.tenant_id, receipt.id).await?;
    if reprocess.is_none() {
        if let Some(previous) = previous {
            tx.commit().await?;
            return Ok(previous.result);
        }
    }
    validate_expected_revision(previous.as_ref(), expected_revision)?;
    if previous
        .as_ref()
        .is_some_and(|value| value.result.status == IntegrationInboxProcessingStatus::Processed)
    {
        return Err(AppError::conflict(
            "processed integration inbox receipt is terminal",
        ));
    }

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
        order_creation::create_fulfillment_order_tx(&mut tx, access, &order_context, order).await?;
    let order_id = OrderId::new(order_result.order_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let order_revision = OrderRevision::new(order_result.revision)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let result = write_outcome_tx(
        &mut tx,
        access,
        receipt,
        context.actor_id,
        previous.as_ref(),
        OutcomeIds {
            order_id: Some(order_id),
            order_revision: Some(order_revision),
        },
        None,
    )
    .await?;
    if let Some(prepared) = prepared {
        let completed = prepared.completed_result(&result, None)?;
        insert_result(&mut tx, &completed).await?;
    }
    tx.commit().await?;
    Ok(result)
}

fn validate_expected_revision(
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

pub async fn receipt_for_reprocessing(
    db: &Db,
    access: &TenantAccess,
    receipt_id: i64,
) -> AppResult<Option<IntegrationInboxReceipt>> {
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
        SELECT id,tenant_id,inventory_owner_id,facility_id,received_at,
               source_key,deduplication_key,content_type,raw_payload,
               payload_sha256,request_id
        FROM integration_inbox_receipts
        WHERE tenant_id=$1 AND id=$2
          AND inventory_owner_id IS NOT NULL
          AND facility_id IS NULL
          AND ($3 OR inventory_owner_id=ANY($4))
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
            Ok::<_, AppError>(IntegrationInboxReceipt {
                id: row.try_get("id")?,
                tenant_id: TenantId::new(row.try_get("tenant_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                inventory_owner_id: row
                    .try_get::<Option<i64>, _>("inventory_owner_id")?
                    .map(InventoryOwnerId::new)
                    .transpose()
                    .map_err(|error| AppError::internal(error.to_string()))?,
                facility_id: None,
                received_at: row.try_get("received_at")?,
                source_key: row.try_get("source_key")?,
                deduplication_key: row.try_get("deduplication_key")?,
                content_type: row.try_get("content_type")?,
                raw_payload: row.try_get("raw_payload")?,
                payload_sha256: row.try_get("payload_sha256")?,
                request_id: row.try_get("request_id")?,
            })
        })
        .transpose()?;
    tx.commit().await?;
    Ok(receipt)
}
