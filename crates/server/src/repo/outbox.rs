//! Transactional domain-event outbox and lease-based worker delivery primitives.

use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
use wareboxes_core::models::Timestamp;
use wareboxes_domain::{FacilityId, InventoryOwnerId, TenantId};

use crate::db::{bind_tenant_context, Db};
use crate::error::{AppError, AppResult};

pub struct NewOutboxEvent<'a> {
    pub tenant_id: TenantId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
    pub actor_user_id: Option<i64>,
    pub event_key: &'a str,
    pub aggregate_type: &'a str,
    pub aggregate_id: &'a str,
    pub ordering_key: &'a str,
    pub aggregate_sequence: i64,
    pub event_type: &'a str,
    pub schema_version: i32,
    pub payload: &'a Value,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutboxEvent {
    pub id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
    pub actor_user_id: Option<i64>,
    pub created: Timestamp,
    pub event_key: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub ordering_key: String,
    pub aggregate_sequence: i64,
    pub event_type: String,
    pub schema_version: i32,
    pub payload: Value,
    pub occurred_at: Timestamp,
    pub available_at: Timestamp,
    pub claimed_at: Option<Timestamp>,
    pub claimed_by: Option<String>,
    pub lease_expires_at: Option<Timestamp>,
    pub claim_version: i64,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub dead_lettered_at: Option<Timestamp>,
    pub replay_count: i32,
    pub discarded_at: Option<Timestamp>,
    pub discard_reason: Option<String>,
    pub discarded_by_user_id: Option<i64>,
    pub published_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryFailureClass {
    Retryable,
    Permanent,
}

impl DeliveryFailureClass {
    fn as_database_value(self) -> &'static str {
        match self {
            Self::Retryable => "retryable",
            Self::Permanent => "permanent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryAttemptOutcome {
    Published,
    RetryScheduled,
    PermanentFailure,
    RetryExhausted,
    LeaseLost,
}

impl DeliveryAttemptOutcome {
    fn from_database_value(value: &str) -> AppResult<Self> {
        match value {
            "published" => Ok(Self::Published),
            "retry_scheduled" => Ok(Self::RetryScheduled),
            "permanent_failure" => Ok(Self::PermanentFailure),
            "retry_exhausted" => Ok(Self::RetryExhausted),
            "lease_lost" => Ok(Self::LeaseLost),
            _ => Err(AppError::internal(format!(
                "database returned an invalid delivery attempt outcome: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAttempt {
    pub tenant_id: TenantId,
    pub outbox_event_id: i64,
    pub event_key: String,
    pub event_type: String,
    pub claim_version: i64,
    pub replay_count: i32,
    pub attempt_number: i32,
    pub worker_id: String,
    pub publisher_name: String,
    pub claimed_at: Timestamp,
    pub lease_expires_at: Timestamp,
    pub outcome: Option<DeliveryAttemptOutcome>,
    pub completed_at: Option<Timestamp>,
    pub error: Option<String>,
    pub retry_after_seconds: Option<i64>,
}

fn required_text(value: &str, label: &str) -> AppResult<()> {
    if value.trim().is_empty() {
        Err(AppError::bad_request(format!("{label} cannot be blank")))
    } else {
        Ok(())
    }
}

fn bounded_text(value: &str, label: &str, maximum_characters: usize) -> AppResult<()> {
    required_text(value, label)?;
    if value.chars().count() > maximum_characters {
        return Err(AppError::bad_request(format!(
            "{label} cannot exceed {maximum_characters} characters"
        )));
    }
    Ok(())
}

fn map_event(row: &sqlx::postgres::PgRow) -> AppResult<OutboxEvent> {
    let payload_json: String = row.try_get("payload_json")?;
    let payload = serde_json::from_str(&payload_json)
        .map_err(|error| AppError::internal(format!("decoding outbox payload: {error}")))?;
    let inventory_owner_id = row
        .try_get::<Option<i64>, _>("inventory_owner_id")?
        .map(InventoryOwnerId::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;
    let facility_id = row
        .try_get::<Option<i64>, _>("facility_id")?
        .map(FacilityId::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;

    Ok(OutboxEvent {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id,
        facility_id,
        actor_user_id: row.try_get("actor_user_id")?,
        created: row.try_get("created")?,
        event_key: row.try_get("event_key")?,
        aggregate_type: row.try_get("aggregate_type")?,
        aggregate_id: row.try_get("aggregate_id")?,
        ordering_key: row.try_get("ordering_key")?,
        aggregate_sequence: row.try_get("aggregate_sequence")?,
        event_type: row.try_get("event_type")?,
        schema_version: row.try_get("schema_version")?,
        payload,
        occurred_at: row.try_get("occurred_at")?,
        available_at: row.try_get("available_at")?,
        claimed_at: row.try_get("claimed_at")?,
        claimed_by: row.try_get("claimed_by")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        claim_version: row.try_get("claim_version")?,
        attempts: row.try_get("attempts")?,
        last_error: row.try_get("last_error")?,
        dead_lettered_at: row.try_get("dead_lettered_at")?,
        replay_count: row.try_get("replay_count")?,
        discarded_at: row.try_get("discarded_at")?,
        discard_reason: row.try_get("discard_reason")?,
        discarded_by_user_id: row.try_get("discarded_by_user_id")?,
        published_at: row.try_get("published_at")?,
    })
}

fn map_delivery_attempt(row: &sqlx::postgres::PgRow) -> AppResult<DeliveryAttempt> {
    let outcome = row
        .try_get::<Option<String>, _>("outcome")?
        .as_deref()
        .map(DeliveryAttemptOutcome::from_database_value)
        .transpose()?;
    Ok(DeliveryAttempt {
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        outbox_event_id: row.try_get("outbox_event_id")?,
        event_key: row.try_get("event_key")?,
        event_type: row.try_get("event_type")?,
        claim_version: row.try_get("claim_version")?,
        replay_count: row.try_get("replay_count")?,
        attempt_number: row.try_get("attempt_number")?,
        worker_id: row.try_get("worker_id")?,
        publisher_name: row.try_get("publisher_name")?,
        claimed_at: row.try_get("claimed_at")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        outcome,
        completed_at: row.try_get("completed_at")?,
        error: row.try_get("error")?,
        retry_after_seconds: row.try_get("retry_after_seconds")?,
    })
}

const EVENT_COLUMNS: &str = r#"
    id, tenant_id, inventory_owner_id, facility_id, actor_user_id, created,
    event_key, aggregate_type, aggregate_id, ordering_key, aggregate_sequence,
    event_type, schema_version,
    payload::TEXT AS payload_json, occurred_at, available_at, claimed_at,
    claimed_by, lease_expires_at, claim_version, attempts, last_error,
    dead_lettered_at, replay_count, discarded_at, discard_reason,
    discarded_by_user_id, published_at
"#;

const CLAIMED_EVENT_COLUMNS: &str = r#"
    event.id AS id,
    event.tenant_id AS tenant_id,
    event.inventory_owner_id AS inventory_owner_id,
    event.facility_id AS facility_id,
    event.actor_user_id AS actor_user_id,
    event.created AS created,
    event.event_key AS event_key,
    event.aggregate_type AS aggregate_type,
    event.aggregate_id AS aggregate_id,
    event.ordering_key AS ordering_key,
    event.aggregate_sequence AS aggregate_sequence,
    event.event_type AS event_type,
    event.schema_version AS schema_version,
    event.payload::TEXT AS payload_json,
    event.occurred_at AS occurred_at,
    event.available_at AS available_at,
    event.claimed_at AS claimed_at,
    event.claimed_by AS claimed_by,
    event.lease_expires_at AS lease_expires_at,
    event.claim_version AS claim_version,
    event.attempts AS attempts,
    event.last_error AS last_error,
    event.dead_lettered_at AS dead_lettered_at,
    event.replay_count AS replay_count,
    event.discarded_at AS discarded_at,
    event.discard_reason AS discard_reason,
    event.discarded_by_user_id AS discarded_by_user_id,
    event.published_at AS published_at
"#;

pub async fn enqueue(
    tx: &mut Transaction<'_, Postgres>,
    event: &NewOutboxEvent<'_>,
) -> AppResult<i64> {
    required_text(event.event_key, "event key")?;
    required_text(event.aggregate_type, "aggregate type")?;
    required_text(event.aggregate_id, "aggregate ID")?;
    required_text(event.ordering_key, "ordering key")?;
    required_text(event.event_type, "event type")?;
    if event.aggregate_sequence <= 0 {
        return Err(AppError::bad_request("aggregate sequence must be positive"));
    }
    if event.schema_version <= 0 {
        return Err(AppError::bad_request("schema version must be positive"));
    }
    if !event.payload.is_object() {
        return Err(AppError::bad_request("outbox payload must be an object"));
    }
    let payload = serde_json::to_string(event.payload)
        .map_err(|error| AppError::internal(format!("encoding outbox payload: {error}")))?;
    bind_tenant_context(tx, event.tenant_id).await?;

    sqlx::query(
        r#"
        INSERT INTO outbox_event_keys (tenant_id, event_key, created)
        VALUES ($1, $2, clock_timestamp())
        "#,
    )
    .bind(event.tenant_id.get())
    .bind(event.event_key)
    .execute(&mut **tx)
    .await?;

    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "outbox-sequence:{}:{}",
            event.tenant_id, event.ordering_key
        ))
        .execute(&mut **tx)
        .await?;
    let current_sequence: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT last_sequence
        FROM outbox_aggregate_sequences
        WHERE tenant_id = $1 AND ordering_key = $2
        FOR UPDATE
        "#,
    )
    .bind(event.tenant_id.get())
    .bind(event.ordering_key)
    .fetch_optional(&mut **tx)
    .await?;
    let expected_sequence = current_sequence.map_or(1, |sequence| sequence + 1);
    if event.aggregate_sequence != expected_sequence {
        return Err(AppError::conflict(
            "outbox aggregate sequence must be contiguous",
        ));
    }
    sqlx::query(
        r#"
        INSERT INTO outbox_aggregate_sequences
            (tenant_id, ordering_key, last_sequence, updated)
        VALUES ($1, $2, $3, clock_timestamp())
        ON CONFLICT (tenant_id, ordering_key) DO UPDATE
        SET last_sequence = EXCLUDED.last_sequence,
            updated = clock_timestamp()
        "#,
    )
    .bind(event.tenant_id.get())
    .bind(event.ordering_key)
    .bind(event.aggregate_sequence)
    .execute(&mut **tx)
    .await?;

    let id = sqlx::query_scalar(
        r#"
        INSERT INTO outbox_events
            (tenant_id, inventory_owner_id, facility_id, actor_user_id, created,
             event_key, aggregate_type, aggregate_id, ordering_key,
             aggregate_sequence, event_type, schema_version, payload, occurred_at,
             available_at)
        VALUES ($1, $2, $3, $4, clock_timestamp(), $5, $6, $7, $8, $9,
                $10, $11, $12::JSONB, $13, clock_timestamp())
        RETURNING id
        "#,
    )
    .bind(event.tenant_id.get())
    .bind(event.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(event.facility_id.map(FacilityId::get))
    .bind(event.actor_user_id)
    .bind(event.event_key)
    .bind(event.aggregate_type)
    .bind(event.aggregate_id)
    .bind(event.ordering_key)
    .bind(event.aggregate_sequence)
    .bind(event.event_type)
    .bind(event.schema_version)
    .bind(payload)
    .bind(event.occurred_at)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

pub async fn get_events(
    db: &Db,
    tenant_id: TenantId,
    after_id: Option<i64>,
    limit: i64,
) -> AppResult<Vec<OutboxEvent>> {
    if !(1..=1_000).contains(&limit) {
        return Err(AppError::bad_request(
            "outbox history limit must be between 1 and 1000",
        ));
    }
    let sql = format!(
        "SELECT {EVENT_COLUMNS} FROM outbox_events WHERE tenant_id = $1 AND id > $2 ORDER BY id LIMIT $3"
    );
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let rows = sqlx::query(&sql)
        .bind(tenant_id.get())
        .bind(after_id.unwrap_or(0))
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?;
    let events = rows.iter().map(map_event).collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(events)
}

pub async fn get_delivery_attempts(
    db: &Db,
    tenant_id: TenantId,
    outbox_event_id: i64,
    limit: i64,
) -> AppResult<Vec<DeliveryAttempt>> {
    if outbox_event_id <= 0 {
        return Err(AppError::bad_request("outbox event ID must be positive"));
    }
    if !(1..=1_000).contains(&limit) {
        return Err(AppError::bad_request(
            "delivery attempt history limit must be between 1 and 1000",
        ));
    }
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT attempt.tenant_id,
               attempt.outbox_event_id,
               attempt.event_key,
               attempt.event_type,
               attempt.claim_version,
               attempt.replay_count,
               attempt.attempt_number,
               attempt.worker_id,
               attempt.publisher_name,
               attempt.claimed_at,
               attempt.lease_expires_at,
               result.outcome,
               result.completed_at,
               result.error,
               result.retry_after_seconds
        FROM outbox_delivery_attempts attempt
        LEFT JOIN outbox_delivery_attempt_results result
          ON result.tenant_id = attempt.tenant_id
         AND result.outbox_event_id = attempt.outbox_event_id
         AND result.claim_version = attempt.claim_version
        WHERE attempt.tenant_id = $1
          AND attempt.outbox_event_id = $2
        ORDER BY attempt.claim_version
        LIMIT $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(outbox_event_id)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    let attempts = rows
        .iter()
        .map(map_delivery_attempt)
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(attempts)
}

pub async fn claim_events(
    db: &Db,
    tenant_id: TenantId,
    worker_id: &str,
    publisher_name: &str,
    batch_size: i64,
    lease_seconds: i64,
) -> AppResult<Vec<OutboxEvent>> {
    bounded_text(worker_id, "worker ID", 200)?;
    bounded_text(publisher_name, "publisher name", 200)?;
    if !(1..=1_000).contains(&batch_size) {
        return Err(AppError::bad_request(
            "outbox batch size must be between 1 and 1000",
        ));
    }
    if lease_seconds <= 0 {
        return Err(AppError::bad_request("outbox claim lease must be positive"));
    }

    let sql = format!(
        r#"
        WITH candidates AS (
            SELECT event.id
            FROM outbox_events event
            WHERE event.tenant_id = $1
              AND event.published_at IS NULL
              AND event.dead_lettered_at IS NULL
              AND event.discarded_at IS NULL
              AND event.available_at <= clock_timestamp()
              AND (
                  event.claimed_at IS NULL
                  OR event.lease_expires_at <= clock_timestamp()
              )
              AND NOT EXISTS (
                  SELECT 1
                  FROM outbox_events predecessor
                  WHERE predecessor.tenant_id = event.tenant_id
                    AND predecessor.ordering_key = event.ordering_key
                    AND predecessor.aggregate_sequence < event.aggregate_sequence
                    AND predecessor.published_at IS NULL
                    AND predecessor.discarded_at IS NULL
              )
            ORDER BY event.available_at, event.id
            FOR UPDATE OF event SKIP LOCKED
            LIMIT $3
        )
        UPDATE outbox_events event
        SET claimed_at = clock_timestamp(),
            claimed_by = $4,
            lease_expires_at = clock_timestamp() + ($2::BIGINT * INTERVAL '1 second'),
            claim_version = event.claim_version + 1,
            attempts = event.attempts + 1
        FROM candidates
        WHERE event.id = candidates.id
        RETURNING {CLAIMED_EVENT_COLUMNS}
        "#,
    );
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let rows = sqlx::query(&sql)
        .bind(tenant_id.get())
        .bind(lease_seconds)
        .bind(batch_size)
        .bind(worker_id)
        .fetch_all(&mut *tx)
        .await?;
    let mut events = rows.iter().map(map_event).collect::<AppResult<Vec<_>>>()?;
    events.sort_by_key(|event| event.id);
    if !events.is_empty() {
        let event_ids = events.iter().map(|event| event.id).collect::<Vec<_>>();
        let result = sqlx::query(
            r#"
            INSERT INTO outbox_delivery_attempts
                (tenant_id, outbox_event_id, event_key, event_type,
                 claim_version, replay_count, attempt_number, worker_id,
                 publisher_name, claimed_at, lease_expires_at)
            SELECT event.tenant_id,
                   event.id,
                   event.event_key,
                   event.event_type,
                   event.claim_version,
                   event.replay_count,
                   event.attempts,
                   event.claimed_by,
                   $4,
                   event.claimed_at,
                   event.lease_expires_at
            FROM outbox_events event
            WHERE event.tenant_id = $1
              AND event.claimed_by = $2
              AND event.id = ANY($3)
            "#,
        )
        .bind(tenant_id.get())
        .bind(worker_id)
        .bind(&event_ids)
        .bind(publisher_name)
        .execute(&mut *tx)
        .await?;
        let expected_rows = u64::try_from(events.len())
            .map_err(|_| AppError::internal("claimed event count exceeds u64"))?;
        if result.rows_affected() != expected_rows {
            return Err(AppError::internal(
                "delivery attempt journal did not record every claimed event",
            ));
        }
    }
    tx.commit().await?;
    Ok(events)
}

struct RecordDeliveryResult<'a> {
    tenant_id: TenantId,
    event_id: i64,
    worker_id: &'a str,
    claim_version: i64,
    outcome: &'a str,
    error: Option<&'a str>,
    retry_after_seconds: Option<i64>,
}

async fn record_delivery_result(
    tx: &mut Transaction<'_, Postgres>,
    result: &RecordDeliveryResult<'_>,
) -> AppResult<u64> {
    let result = sqlx::query(
        r#"
        INSERT INTO outbox_delivery_attempt_results
            (tenant_id, outbox_event_id, claim_version, outcome, completed_at,
             error, retry_after_seconds)
        SELECT attempt.tenant_id,
               attempt.outbox_event_id,
               attempt.claim_version,
               $5,
               clock_timestamp(),
               $6,
               $7
        FROM outbox_delivery_attempts attempt
        WHERE attempt.tenant_id = $1
          AND attempt.outbox_event_id = $2
          AND attempt.worker_id = $3
          AND attempt.claim_version = $4
        ON CONFLICT (tenant_id, outbox_event_id, claim_version) DO NOTHING
        "#,
    )
    .bind(result.tenant_id.get())
    .bind(result.event_id)
    .bind(result.worker_id)
    .bind(result.claim_version)
    .bind(result.outcome)
    .bind(result.error)
    .bind(result.retry_after_seconds)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected())
}

pub async fn mark_published(
    db: &Db,
    tenant_id: TenantId,
    event_id: i64,
    worker_id: &str,
    claim_version: i64,
) -> AppResult<bool> {
    bounded_text(worker_id, "worker ID", 200)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let result = sqlx::query(
        r#"
        UPDATE outbox_events
        SET published_at = clock_timestamp(), claimed_at = NULL, claimed_by = NULL,
            lease_expires_at = NULL, last_error = NULL
        WHERE tenant_id = $1
          AND id = $2
          AND claimed_by = $3
          AND claim_version = $4
          AND dead_lettered_at IS NULL
          AND discarded_at IS NULL
          AND published_at IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(event_id)
    .bind(worker_id)
    .bind(claim_version)
    .execute(&mut *tx)
    .await?;
    let published = result.rows_affected() == 1;
    let journal_rows = record_delivery_result(
        &mut tx,
        &RecordDeliveryResult {
            tenant_id,
            event_id,
            worker_id,
            claim_version,
            outcome: if published { "published" } else { "lease_lost" },
            error: None,
            retry_after_seconds: None,
        },
    )
    .await?;
    if published && journal_rows != 1 {
        return Err(AppError::internal(
            "published event is missing its delivery attempt result",
        ));
    }
    tx.commit().await?;
    Ok(published)
}

pub struct FailOutboxEvent<'a> {
    pub tenant_id: TenantId,
    pub event_id: i64,
    pub worker_id: &'a str,
    pub claim_version: i64,
    pub failure_class: DeliveryFailureClass,
    pub error: &'a str,
    pub retry_after_seconds: i64,
    pub max_attempts: i32,
}

pub async fn mark_failed(db: &Db, failure: &FailOutboxEvent<'_>) -> AppResult<bool> {
    bounded_text(failure.worker_id, "worker ID", 200)?;
    bounded_text(failure.error, "delivery error", 4_000)?;
    if failure.retry_after_seconds < 0 {
        return Err(AppError::bad_request(
            "outbox retry delay cannot be negative",
        ));
    }
    if failure.max_attempts <= 0 {
        return Err(AppError::bad_request(
            "outbox maximum attempts must be positive",
        ));
    }
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, failure.tenant_id).await?;
    let result: Option<i32> = sqlx::query_scalar(
        r#"
        UPDATE outbox_events
        SET available_at = CASE
                WHEN $1 = 'permanent' OR attempts >= $2 THEN available_at
                ELSE clock_timestamp() + ($3::BIGINT * INTERVAL '1 second')
            END,
            claimed_at = NULL,
            claimed_by = NULL,
            lease_expires_at = NULL,
            last_error = $4,
            dead_lettered_at = CASE
                WHEN $1 = 'permanent' OR attempts >= $2
                    THEN clock_timestamp()
                ELSE NULL
            END
        WHERE tenant_id = $5
          AND id = $6
          AND claimed_by = $7
          AND claim_version = $8
          AND dead_lettered_at IS NULL
          AND discarded_at IS NULL
          AND published_at IS NULL
        RETURNING attempts
        "#,
    )
    .bind(failure.failure_class.as_database_value())
    .bind(failure.max_attempts)
    .bind(failure.retry_after_seconds)
    .bind(failure.error.trim())
    .bind(failure.tenant_id.get())
    .bind(failure.event_id)
    .bind(failure.worker_id)
    .bind(failure.claim_version)
    .fetch_optional(&mut *tx)
    .await?;
    let outcome = match (failure.failure_class, result) {
        (_, None) => "lease_lost",
        (DeliveryFailureClass::Permanent, Some(_)) => "permanent_failure",
        (DeliveryFailureClass::Retryable, Some(attempts)) if attempts >= failure.max_attempts => {
            "retry_exhausted"
        }
        (DeliveryFailureClass::Retryable, Some(_)) => "retry_scheduled",
    };
    let retry_after_seconds = (outcome == "retry_scheduled").then_some(failure.retry_after_seconds);
    let journal_rows = record_delivery_result(
        &mut tx,
        &RecordDeliveryResult {
            tenant_id: failure.tenant_id,
            event_id: failure.event_id,
            worker_id: failure.worker_id,
            claim_version: failure.claim_version,
            outcome,
            error: Some(failure.error.trim()),
            retry_after_seconds,
        },
    )
    .await?;
    if result.is_some() && journal_rows != 1 {
        return Err(AppError::internal(
            "failed event is missing its delivery attempt result",
        ));
    }
    tx.commit().await?;
    Ok(result.is_some())
}

pub async fn replay_dead_letter(db: &Db, tenant_id: TenantId, event_id: i64) -> AppResult<bool> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let result = sqlx::query(
        r#"
        UPDATE outbox_events
        SET available_at = clock_timestamp(), attempts = 0, last_error = NULL,
            dead_lettered_at = NULL, replay_count = replay_count + 1
        WHERE tenant_id = $1
          AND id = $2
          AND dead_lettered_at IS NOT NULL
          AND discarded_at IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(event_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.rows_affected() == 1)
}

pub async fn discard_dead_letter(
    db: &Db,
    tenant_id: TenantId,
    event_id: i64,
    user_id: i64,
    reason: &str,
) -> AppResult<bool> {
    required_text(reason, "discard reason")?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let result = sqlx::query(
        r#"
        UPDATE outbox_events
        SET discarded_at = clock_timestamp(), discard_reason = $1,
            discarded_by_user_id = $2
        WHERE tenant_id = $3
          AND id = $4
          AND dead_lettered_at IS NOT NULL
          AND discarded_at IS NULL
          AND published_at IS NULL
        "#,
    )
    .bind(reason.trim())
    .bind(user_id)
    .bind(tenant_id.get())
    .bind(event_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.rows_affected() == 1)
}

pub async fn purge_published(
    db: &Db,
    tenant_id: TenantId,
    retention_seconds: i64,
    batch_size: i64,
) -> AppResult<u64> {
    if retention_seconds < 0 {
        return Err(AppError::bad_request(
            "outbox retention period cannot be negative",
        ));
    }
    if !(1..=10_000).contains(&batch_size) {
        return Err(AppError::bad_request(
            "outbox purge batch size must be between 1 and 10000",
        ));
    }
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, tenant_id).await?;
    let result = sqlx::query(
        r#"
        WITH expired AS (
            SELECT id
            FROM outbox_events
            WHERE tenant_id = $1
              AND COALESCE(published_at, discarded_at) IS NOT NULL
              AND COALESCE(published_at, discarded_at)
                  <= clock_timestamp() - ($2::BIGINT * INTERVAL '1 second')
            ORDER BY COALESCE(published_at, discarded_at), id
            FOR UPDATE SKIP LOCKED
            LIMIT $3
        )
        DELETE FROM outbox_events event
        USING expired
        WHERE event.tenant_id = $1
          AND event.id = expired.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(retention_seconds)
    .bind(batch_size)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.rows_affected())
}
