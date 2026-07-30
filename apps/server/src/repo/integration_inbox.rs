//! Immutable inbound integration receipts and replay-safe deduplication.

use sha2::{Digest, Sha256};
use sqlx::Row;
use wareboxes_core::models::Timestamp;
use wareboxes_domain::{FacilityId, InventoryOwnerId, TenantId};

use crate::db::{bind_tenant_context, Db};
use crate::error::{AppError, AppResult};

const MAX_SOURCE_KEY_CHARACTERS: usize = 200;
const MAX_DEDUPLICATION_KEY_CHARACTERS: usize = 500;
const MAX_CONTENT_TYPE_CHARACTERS: usize = 255;
const MAX_REQUEST_ID_CHARACTERS: usize = 128;
const MAX_RAW_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

pub struct NewIntegrationInboxReceipt<'a> {
    pub tenant_id: TenantId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
    pub source_key: &'a str,
    pub deduplication_key: &'a str,
    pub content_type: &'a str,
    pub raw_payload: &'a [u8],
    pub request_id: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntegrationInboxReadScope {
    pub tenant_id: TenantId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationInboxReceipt {
    pub id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
    pub received_at: Timestamp,
    pub source_key: String,
    pub deduplication_key: String,
    pub content_type: String,
    pub raw_payload: Vec<u8>,
    pub payload_sha256: Vec<u8>,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveIntegrationInboxResult {
    pub receipt: IntegrationInboxReceipt,
    pub replayed: bool,
}

fn validate_key(value: &str, label: &str, maximum_characters: usize) -> AppResult<()> {
    if value.is_empty() || value.trim() != value {
        return Err(AppError::bad_request(format!(
            "{label} must be non-blank and cannot have surrounding whitespace"
        )));
    }
    if value.chars().count() > maximum_characters {
        return Err(AppError::bad_request(format!(
            "{label} cannot exceed {maximum_characters} characters"
        )));
    }
    Ok(())
}

fn map_optional_owner(row: &sqlx::postgres::PgRow) -> AppResult<Option<InventoryOwnerId>> {
    row.try_get::<Option<i64>, _>("inventory_owner_id")?
        .map(InventoryOwnerId::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))
}

fn map_optional_facility(row: &sqlx::postgres::PgRow) -> AppResult<Option<FacilityId>> {
    row.try_get::<Option<i64>, _>("facility_id")?
        .map(FacilityId::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))
}

fn map_receipt(row: &sqlx::postgres::PgRow) -> AppResult<IntegrationInboxReceipt> {
    Ok(IntegrationInboxReceipt {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: map_optional_owner(row)?,
        facility_id: map_optional_facility(row)?,
        received_at: row.try_get("received_at")?,
        source_key: row.try_get("source_key")?,
        deduplication_key: row.try_get("deduplication_key")?,
        content_type: row.try_get("content_type")?,
        raw_payload: row.try_get("raw_payload")?,
        payload_sha256: row.try_get("payload_sha256")?,
        request_id: row.try_get("request_id")?,
    })
}

const RECEIPT_COLUMNS: &str = r#"
    id, tenant_id, inventory_owner_id, facility_id, received_at, source_key,
    deduplication_key, content_type, raw_payload, payload_sha256, request_id
"#;

fn same_envelope(
    inventory_owner_id: Option<InventoryOwnerId>,
    facility_id: Option<FacilityId>,
    content_type: &str,
    existing_payload_sha256: &[u8],
    candidate: &NewIntegrationInboxReceipt<'_>,
    payload_sha256: &[u8],
) -> bool {
    inventory_owner_id == candidate.inventory_owner_id
        && facility_id == candidate.facility_id
        && content_type == candidate.content_type
        && existing_payload_sha256 == payload_sha256
}

pub async fn receive(
    db: &Db,
    receipt: &NewIntegrationInboxReceipt<'_>,
) -> AppResult<ReceiveIntegrationInboxResult> {
    validate_key(
        receipt.source_key,
        "integration source key",
        MAX_SOURCE_KEY_CHARACTERS,
    )?;
    validate_key(
        receipt.deduplication_key,
        "integration deduplication key",
        MAX_DEDUPLICATION_KEY_CHARACTERS,
    )?;
    validate_key(
        receipt.content_type,
        "integration content type",
        MAX_CONTENT_TYPE_CHARACTERS,
    )?;
    if let Some(request_id) = receipt.request_id {
        validate_key(
            request_id,
            "integration request ID",
            MAX_REQUEST_ID_CHARACTERS,
        )?;
    }
    if receipt.raw_payload.len() > MAX_RAW_PAYLOAD_BYTES {
        return Err(AppError::bad_request(format!(
            "integration raw payload cannot exceed {MAX_RAW_PAYLOAD_BYTES} bytes"
        )));
    }

    let payload_sha256 = Sha256::digest(receipt.raw_payload).to_vec();
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, receipt.tenant_id).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "integration-inbox:{}:{}:{}",
            receipt.tenant_id, receipt.source_key, receipt.deduplication_key
        ))
        .execute(&mut *tx)
        .await?;

    let existing_key = sqlx::query(
        r#"
        SELECT receipt_id, inventory_owner_id, facility_id, content_type,
               payload_sha256
        FROM integration_inbox_keys
        WHERE tenant_id = $1
          AND source_key = $2
          AND deduplication_key = $3
        "#,
    )
    .bind(receipt.tenant_id.get())
    .bind(receipt.source_key)
    .bind(receipt.deduplication_key)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(key) = existing_key {
        let inventory_owner_id = map_optional_owner(&key)?;
        let facility_id = map_optional_facility(&key)?;
        let content_type: String = key.try_get("content_type")?;
        let existing_payload_sha256: Vec<u8> = key.try_get("payload_sha256")?;
        if !same_envelope(
            inventory_owner_id,
            facility_id,
            &content_type,
            &existing_payload_sha256,
            receipt,
            &payload_sha256,
        ) {
            return Err(AppError::conflict(
                "integration deduplication key was reused with a different payload or scope",
            ));
        }
        let receipt_id: i64 = key.try_get("receipt_id")?;
        let existing = get_in_transaction(&mut tx, receipt.tenant_id, receipt_id)
            .await?
            .ok_or_else(|| {
                AppError::conflict(
                    "the original integration receipt is no longer available for replay",
                )
            })?;
        if existing.raw_payload != receipt.raw_payload {
            return Err(AppError::conflict(
                "integration deduplication key payload hash collision detected",
            ));
        }
        tx.commit().await?;
        return Ok(ReceiveIntegrationInboxResult {
            receipt: existing,
            replayed: true,
        });
    }

    let insert_sql = format!(
        r#"
        INSERT INTO integration_inbox_receipts
            (tenant_id, inventory_owner_id, facility_id, received_at, source_key,
             deduplication_key, content_type, raw_payload, payload_sha256,
             request_id)
        VALUES ($1, $2, $3, clock_timestamp(), $4, $5, $6, $7, $8, $9)
        RETURNING {RECEIPT_COLUMNS}
        "#
    );
    let row = sqlx::query(&insert_sql)
        .bind(receipt.tenant_id.get())
        .bind(receipt.inventory_owner_id.map(InventoryOwnerId::get))
        .bind(receipt.facility_id.map(FacilityId::get))
        .bind(receipt.source_key)
        .bind(receipt.deduplication_key)
        .bind(receipt.content_type)
        .bind(receipt.raw_payload)
        .bind(&payload_sha256)
        .bind(receipt.request_id)
        .fetch_one(&mut *tx)
        .await?;
    let stored = map_receipt(&row)?;

    sqlx::query(
        r#"
        INSERT INTO integration_inbox_keys
            (tenant_id, source_key, deduplication_key, created_at, receipt_id,
             inventory_owner_id, facility_id, content_type, payload_sha256)
        VALUES ($1, $2, $3, clock_timestamp(), $4, $5, $6, $7, $8)
        "#,
    )
    .bind(receipt.tenant_id.get())
    .bind(receipt.source_key)
    .bind(receipt.deduplication_key)
    .bind(stored.id)
    .bind(receipt.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(receipt.facility_id.map(FacilityId::get))
    .bind(receipt.content_type)
    .bind(&payload_sha256)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(ReceiveIntegrationInboxResult {
        receipt: stored,
        replayed: false,
    })
}

async fn get_in_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    receipt_id: i64,
) -> AppResult<Option<IntegrationInboxReceipt>> {
    let sql = format!(
        "SELECT {RECEIPT_COLUMNS} FROM integration_inbox_receipts WHERE tenant_id = $1 AND id = $2"
    );
    sqlx::query(&sql)
        .bind(tenant_id.get())
        .bind(receipt_id)
        .fetch_optional(&mut **tx)
        .await?
        .as_ref()
        .map(map_receipt)
        .transpose()
}

pub async fn get(
    db: &Db,
    scope: IntegrationInboxReadScope,
    receipt_id: i64,
) -> AppResult<Option<IntegrationInboxReceipt>> {
    if receipt_id <= 0 {
        return Err(AppError::bad_request(
            "integration inbox receipt ID must be positive",
        ));
    }
    let sql = format!(
        r#"
        SELECT {RECEIPT_COLUMNS}
        FROM integration_inbox_receipts
        WHERE tenant_id = $1
          AND id = $2
          AND inventory_owner_id IS NOT DISTINCT FROM $3
          AND facility_id IS NOT DISTINCT FROM $4
        "#
    );
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, scope.tenant_id).await?;
    let receipt = sqlx::query(&sql)
        .bind(scope.tenant_id.get())
        .bind(receipt_id)
        .bind(scope.inventory_owner_id.map(InventoryOwnerId::get))
        .bind(scope.facility_id.map(FacilityId::get))
        .fetch_optional(&mut *tx)
        .await?
        .as_ref()
        .map(map_receipt)
        .transpose()?;
    tx.commit().await?;
    Ok(receipt)
}
