use sqlx::postgres::PgRow;
use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::integration_monitor::{
    DiscardOutboxDeadLetterCommand, DiscardOutboxDeadLetterResult, InboundIntegrationPage,
    InboundIntegrationQuery, InboundIntegrationReceiptReadModel, InboundIntegrationSort,
    IntegrationSortDirection, OutboundDeliveryAttemptReadModel, OutboundDeliveryStatus,
    OutboundIntegrationDetailReadModel, OutboundIntegrationEventReadModel, OutboundIntegrationPage,
    OutboundIntegrationQuery, OutboundIntegrationSort, OutboxDeadLetterDiscardReadModel,
    OutboxDeadLetterReplayReadModel, ReplayOutboxDeadLetterCommand, ReplayOutboxDeadLetterResult,
    DISCARD_OUTBOX_DEAD_LETTER_OPERATION, REPLAY_OUTBOX_DEAD_LETTER_OPERATION,
};
use wareboxes_application::outbox::DeliveryAttemptOutcome;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, OutboxDeadLetterDiscardId, OutboxDeadLetterDiscardReason,
    OutboxDeadLetterReplayId, Timestamp, UserId,
};
use wareboxes_persistence_postgres::db::bind_tenant_context;
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::db::{begin_tenant_transaction, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};

const MAX_PAGE_SIZE: u16 = 1_000;
const MAX_SEARCH_CHARACTERS: usize = 200;
const MAX_SOURCE_CHARACTERS: usize = 200;

fn optional_owner(row: &PgRow) -> AppResult<Option<InventoryOwnerId>> {
    row.try_get::<Option<i64>, _>("inventory_owner_id")?
        .map(InventoryOwnerId::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))
}

fn optional_facility(row: &PgRow) -> AppResult<Option<FacilityId>> {
    row.try_get::<Option<i64>, _>("facility_id")?
        .map(FacilityId::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))
}

fn delivery_status(value: &str) -> AppResult<OutboundDeliveryStatus> {
    match value {
        "pending" => Ok(OutboundDeliveryStatus::Pending),
        "claimed" => Ok(OutboundDeliveryStatus::Claimed),
        "retry_scheduled" => Ok(OutboundDeliveryStatus::RetryScheduled),
        "dead_lettered" => Ok(OutboundDeliveryStatus::DeadLettered),
        "published" => Ok(OutboundDeliveryStatus::Published),
        "discarded" => Ok(OutboundDeliveryStatus::Discarded),
        _ => Err(AppError::internal(format!(
            "database returned invalid outbound delivery status: {value}"
        ))),
    }
}

fn attempt_outcome(value: &str) -> AppResult<DeliveryAttemptOutcome> {
    match value {
        "published" => Ok(DeliveryAttemptOutcome::Published),
        "retry_scheduled" => Ok(DeliveryAttemptOutcome::RetryScheduled),
        "permanent_failure" => Ok(DeliveryAttemptOutcome::PermanentFailure),
        "retry_exhausted" => Ok(DeliveryAttemptOutcome::RetryExhausted),
        "lease_lost" => Ok(DeliveryAttemptOutcome::LeaseLost),
        _ => Err(AppError::internal(format!(
            "database returned invalid delivery attempt outcome: {value}"
        ))),
    }
}

fn map_inbound(row: &PgRow) -> AppResult<InboundIntegrationReceiptReadModel> {
    Ok(InboundIntegrationReceiptReadModel {
        id: row.try_get("id")?,
        inventory_owner_id: optional_owner(row)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: optional_facility(row)?,
        facility_name: row.try_get("facility_name")?,
        received_at: row.try_get("received_at")?,
        source_key: row.try_get("source_key")?,
        deduplication_key: row.try_get("deduplication_key")?,
        content_type: row.try_get("content_type")?,
        payload_bytes: row.try_get("payload_bytes")?,
        payload_sha256: row.try_get("payload_sha256")?,
        request_id: row.try_get("request_id")?,
    })
}

fn map_outbound(row: &PgRow) -> AppResult<OutboundIntegrationEventReadModel> {
    Ok(OutboundIntegrationEventReadModel {
        id: row.try_get("id")?,
        inventory_owner_id: optional_owner(row)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: optional_facility(row)?,
        facility_name: row.try_get("facility_name")?,
        created_at: row.try_get("created_at")?,
        occurred_at: row.try_get("occurred_at")?,
        available_at: row.try_get("available_at")?,
        event_key: row.try_get("event_key")?,
        event_type: row.try_get("event_type")?,
        aggregate_type: row.try_get("aggregate_type")?,
        aggregate_id: row.try_get("aggregate_id")?,
        aggregate_sequence: row.try_get("aggregate_sequence")?,
        schema_version: row.try_get("schema_version")?,
        status: delivery_status(row.try_get::<String, _>("delivery_status")?.as_str())?,
        attempts: row.try_get("attempts")?,
        replay_count: row.try_get("replay_count")?,
        claimed_by: row.try_get("claimed_by")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        last_error: row.try_get("last_error")?,
        published_at: row.try_get("published_at")?,
        dead_lettered_at: row.try_get("dead_lettered_at")?,
        discarded_at: row.try_get("discarded_at")?,
    })
}

fn map_attempt(row: &PgRow) -> AppResult<OutboundDeliveryAttemptReadModel> {
    let outcome = row
        .try_get::<Option<String>, _>("outcome")?
        .as_deref()
        .map(attempt_outcome)
        .transpose()?;
    Ok(OutboundDeliveryAttemptReadModel {
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

fn map_replay(row: &PgRow) -> AppResult<OutboxDeadLetterReplayReadModel> {
    Ok(OutboxDeadLetterReplayReadModel {
        replay_id: OutboxDeadLetterReplayId::new(row.try_get("replay_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        previous_replay_count: row.try_get("previous_replay_count")?,
        replay_count: row.try_get("resulting_replay_count")?,
        previous_attempts: row.try_get("previous_attempts")?,
        last_error: row.try_get("last_error_snapshot")?,
        replayed_by: UserId::new(row.try_get("replayed_by_user_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        replayed_by_name: row.try_get("replayed_by_name")?,
        replayed_at: row.try_get("replayed_at")?,
    })
}

fn map_discard(row: &PgRow) -> AppResult<OutboxDeadLetterDiscardReadModel> {
    Ok(OutboxDeadLetterDiscardReadModel {
        discard_id: OutboxDeadLetterDiscardId::new(row.try_get("discard_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        replay_count: row.try_get("replay_count")?,
        previous_attempts: row.try_get("previous_attempts")?,
        last_error: row.try_get("last_error_snapshot")?,
        reason: OutboxDeadLetterDiscardReason::new(row.try_get::<String, _>("discard_reason")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        discarded_by: UserId::new(row.try_get("discarded_by_user_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        discarded_by_name: row.try_get("discarded_by_name")?,
        discarded_at: row.try_get("discarded_at")?,
    })
}

fn validate_text(value: Option<&str>, label: &str, maximum: usize) -> AppResult<()> {
    if let Some(value) = value {
        if value.trim().is_empty() || value != value.trim() {
            return Err(AppError::bad_request(format!(
                "{label} must be non-blank and cannot have surrounding whitespace"
            )));
        }
        if value.chars().count() > maximum {
            return Err(AppError::bad_request(format!(
                "{label} cannot exceed {maximum} characters"
            )));
        }
    }
    Ok(())
}

fn validate_page(offset: u64, limit: u16) -> AppResult<i64> {
    if !(1..=MAX_PAGE_SIZE).contains(&limit) {
        return Err(AppError::bad_request(
            "integration monitor page limit must be between 1 and 1000",
        ));
    }
    i64::try_from(offset)
        .map_err(|_| AppError::bad_request("integration monitor cursor is out of range"))
}

fn inbound_order(query: &InboundIntegrationQuery) -> &'static str {
    match (query.sort, query.direction) {
        (InboundIntegrationSort::ReceivedAt, IntegrationSortDirection::Ascending) => {
            "received_at ASC, id ASC"
        }
        (InboundIntegrationSort::ReceivedAt, IntegrationSortDirection::Descending) => {
            "received_at DESC, id DESC"
        }
        (InboundIntegrationSort::Source, IntegrationSortDirection::Ascending) => {
            "LOWER(source_key) ASC, received_at DESC, id DESC"
        }
        (InboundIntegrationSort::Source, IntegrationSortDirection::Descending) => {
            "LOWER(source_key) DESC, received_at DESC, id DESC"
        }
        (InboundIntegrationSort::PayloadSize, IntegrationSortDirection::Ascending) => {
            "payload_bytes ASC, received_at DESC, id DESC"
        }
        (InboundIntegrationSort::PayloadSize, IntegrationSortDirection::Descending) => {
            "payload_bytes DESC, received_at DESC, id DESC"
        }
    }
}

fn outbound_order(query: &OutboundIntegrationQuery) -> &'static str {
    match (query.sort, query.direction) {
        (OutboundIntegrationSort::CreatedAt, IntegrationSortDirection::Ascending) => {
            "created_at ASC, id ASC"
        }
        (OutboundIntegrationSort::CreatedAt, IntegrationSortDirection::Descending) => {
            "created_at DESC, id DESC"
        }
        (OutboundIntegrationSort::EventType, IntegrationSortDirection::Ascending) => {
            "LOWER(event_type) ASC, created_at DESC, id DESC"
        }
        (OutboundIntegrationSort::EventType, IntegrationSortDirection::Descending) => {
            "LOWER(event_type) DESC, created_at DESC, id DESC"
        }
        (OutboundIntegrationSort::Status, IntegrationSortDirection::Ascending) => {
            "delivery_status ASC, created_at DESC, id DESC"
        }
        (OutboundIntegrationSort::Status, IntegrationSortDirection::Descending) => {
            "delivery_status DESC, created_at DESC, id DESC"
        }
        (OutboundIntegrationSort::Attempts, IntegrationSortDirection::Ascending) => {
            "attempts ASC, created_at DESC, id DESC"
        }
        (OutboundIntegrationSort::Attempts, IntegrationSortDirection::Descending) => {
            "attempts DESC, created_at DESC, id DESC"
        }
    }
}

fn status_value(status: Option<OutboundDeliveryStatus>) -> Option<&'static str> {
    status.map(|status| match status {
        OutboundDeliveryStatus::Pending => "pending",
        OutboundDeliveryStatus::Claimed => "claimed",
        OutboundDeliveryStatus::RetryScheduled => "retry_scheduled",
        OutboundDeliveryStatus::DeadLettered => "dead_lettered",
        OutboundDeliveryStatus::Published => "published",
        OutboundDeliveryStatus::Discarded => "discarded",
    })
}

async fn current_admin_scope(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
) -> AppResult<ScopeBindings> {
    let scope = lock_current_scope_tx(tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(tx, access.tenant_id, access.user_id.get(), "admin").await?;
    Ok(scope)
}

pub async fn inbound_page(
    db: &Db,
    access: &TenantAccess,
    query: &InboundIntegrationQuery,
) -> AppResult<InboundIntegrationPage> {
    validate_text(
        query.search.as_deref(),
        "integration search",
        MAX_SEARCH_CHARACTERS,
    )?;
    validate_text(
        query.source_key.as_deref(),
        "integration source key",
        MAX_SOURCE_CHARACTERS,
    )?;
    let offset = validate_page(query.offset, query.limit)?;
    let fetch_limit = i64::from(query.limit) + 1;
    let order = inbound_order(query);
    let sql = format!(
        r#"
        SELECT receipt.id, receipt.inventory_owner_id, owner.name AS inventory_owner_name,
               receipt.facility_id, facility.name AS facility_name, receipt.received_at,
               receipt.source_key, receipt.deduplication_key, receipt.content_type,
               octet_length(receipt.raw_payload)::BIGINT AS payload_bytes,
               encode(receipt.payload_sha256, 'hex') AS payload_sha256,
               receipt.request_id
        FROM integration_inbox_receipts receipt
        LEFT JOIN inventory_owners owner
          ON owner.tenant_id=receipt.tenant_id AND owner.id=receipt.inventory_owner_id
        LEFT JOIN facilities facility
          ON facility.tenant_id=receipt.tenant_id AND facility.id=receipt.facility_id
        WHERE receipt.tenant_id=$1
          AND (receipt.facility_id IS NULL OR $2 OR receipt.facility_id=ANY($3))
          AND (receipt.inventory_owner_id IS NULL OR $4 OR receipt.inventory_owner_id=ANY($5))
          AND ($6::BIGINT IS NULL OR receipt.facility_id=$6)
          AND ($7::BIGINT IS NULL OR receipt.inventory_owner_id=$7)
          AND ($8::TEXT IS NULL OR receipt.source_key=$8)
          AND ($9::TEXT IS NULL OR receipt.source_key ILIKE '%' || $9 || '%'
               OR receipt.deduplication_key ILIKE '%' || $9 || '%'
               OR COALESCE(receipt.request_id, '') ILIKE '%' || $9 || '%')
        ORDER BY {order}
        LIMIT $10 OFFSET $11
        "#
    );
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = current_admin_scope(&mut tx, access).await?;
    let rows = sqlx::query(&sql)
        .bind(access.tenant_id.get())
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .bind(query.facility_id.map(FacilityId::get))
        .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
        .bind(query.source_key.as_deref())
        .bind(query.search.as_deref())
        .bind(fetch_limit)
        .bind(offset)
        .fetch_all(&mut *tx)
        .await?;
    let has_more = rows.len() > usize::from(query.limit);
    let items = rows
        .iter()
        .take(usize::from(query.limit))
        .map(map_inbound)
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(InboundIntegrationPage {
        items,
        next_offset: has_more.then(|| query.offset + u64::from(query.limit)),
    })
}

const OUTBOUND_SELECT: &str = r#"
    event.id, event.inventory_owner_id, owner.name AS inventory_owner_name,
    event.facility_id, facility.name AS facility_name, event.created AS created_at,
    event.occurred_at, event.available_at, event.event_key, event.event_type,
    event.aggregate_type, event.aggregate_id, event.aggregate_sequence,
    event.schema_version, event.attempts, event.replay_count, event.claimed_by,
    event.lease_expires_at, event.last_error, event.published_at,
    event.dead_lettered_at, event.discarded_at,
    CASE
      WHEN event.discarded_at IS NOT NULL THEN 'discarded'
      WHEN event.published_at IS NOT NULL THEN 'published'
      WHEN event.dead_lettered_at IS NOT NULL THEN 'dead_lettered'
      WHEN event.claimed_at IS NOT NULL AND event.lease_expires_at > clock_timestamp()
        THEN 'claimed'
      WHEN event.last_error IS NOT NULL AND event.available_at > clock_timestamp()
        THEN 'retry_scheduled'
      ELSE 'pending'
    END AS delivery_status
"#;

pub async fn outbound_page(
    db: &Db,
    access: &TenantAccess,
    query: &OutboundIntegrationQuery,
) -> AppResult<OutboundIntegrationPage> {
    validate_text(
        query.search.as_deref(),
        "integration search",
        MAX_SEARCH_CHARACTERS,
    )?;
    validate_text(
        query.event_type.as_deref(),
        "event type",
        MAX_SEARCH_CHARACTERS,
    )?;
    let offset = validate_page(query.offset, query.limit)?;
    let fetch_limit = i64::from(query.limit) + 1;
    let order = outbound_order(query);
    let sql = format!(
        r#"
        WITH scoped AS (
          SELECT {OUTBOUND_SELECT}
          FROM outbox_events event
          LEFT JOIN inventory_owners owner
            ON owner.tenant_id=event.tenant_id AND owner.id=event.inventory_owner_id
          LEFT JOIN facilities facility
            ON facility.tenant_id=event.tenant_id AND facility.id=event.facility_id
          WHERE event.tenant_id=$1
            AND (event.facility_id IS NULL OR $2 OR event.facility_id=ANY($3))
            AND (event.inventory_owner_id IS NULL OR $4 OR event.inventory_owner_id=ANY($5))
            AND ($6::BIGINT IS NULL OR event.facility_id=$6)
            AND ($7::BIGINT IS NULL OR event.inventory_owner_id=$7)
            AND ($8::TEXT IS NULL OR event.event_type=$8)
            AND ($9::TEXT IS NULL OR event.event_type ILIKE '%' || $9 || '%'
                 OR event.event_key ILIKE '%' || $9 || '%'
                 OR event.aggregate_type ILIKE '%' || $9 || '%'
                 OR event.aggregate_id ILIKE '%' || $9 || '%')
        )
        SELECT * FROM scoped
        WHERE ($10::TEXT IS NULL OR delivery_status=$10)
        ORDER BY {order}
        LIMIT $11 OFFSET $12
        "#
    );
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = current_admin_scope(&mut tx, access).await?;
    let rows = sqlx::query(&sql)
        .bind(access.tenant_id.get())
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .bind(query.facility_id.map(FacilityId::get))
        .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
        .bind(query.event_type.as_deref())
        .bind(query.search.as_deref())
        .bind(status_value(query.status))
        .bind(fetch_limit)
        .bind(offset)
        .fetch_all(&mut *tx)
        .await?;
    let has_more = rows.len() > usize::from(query.limit);
    let items = rows
        .iter()
        .take(usize::from(query.limit))
        .map(map_outbound)
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(OutboundIntegrationPage {
        items,
        next_offset: has_more.then(|| query.offset + u64::from(query.limit)),
    })
}

pub async fn outbound_detail(
    db: &Db,
    access: &TenantAccess,
    event_id: i64,
) -> AppResult<Option<OutboundIntegrationDetailReadModel>> {
    if event_id <= 0 {
        return Err(AppError::bad_request("outbox event ID must be positive"));
    }
    let sql = format!(
        r#"
        SELECT {OUTBOUND_SELECT}, event.payload::TEXT AS payload_json
        FROM outbox_events event
        LEFT JOIN inventory_owners owner
          ON owner.tenant_id=event.tenant_id AND owner.id=event.inventory_owner_id
        LEFT JOIN facilities facility
          ON facility.tenant_id=event.tenant_id AND facility.id=event.facility_id
        WHERE event.tenant_id=$1 AND event.id=$2
          AND (event.facility_id IS NULL OR $3 OR event.facility_id=ANY($4))
          AND (event.inventory_owner_id IS NULL OR $5 OR event.inventory_owner_id=ANY($6))
        "#
    );
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = current_admin_scope(&mut tx, access).await?;
    let row = sqlx::query(&sql)
        .bind(access.tenant_id.get())
        .bind(event_id)
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let event = map_outbound(&row)?;
    let payload_json: String = row.try_get("payload_json")?;
    let payload = serde_json::from_str(&payload_json)
        .map_err(|error| AppError::internal(format!("decoding outbox payload: {error}")))?;
    let attempt_rows = sqlx::query(
        r#"
        SELECT attempt.claim_version, attempt.replay_count, attempt.attempt_number,
               attempt.worker_id, attempt.publisher_name, attempt.claimed_at,
               attempt.lease_expires_at, result.outcome, result.completed_at,
               result.error, result.retry_after_seconds
        FROM outbox_delivery_attempts attempt
        LEFT JOIN outbox_delivery_attempt_results result
          ON result.tenant_id=attempt.tenant_id
         AND result.outbox_event_id=attempt.outbox_event_id
         AND result.claim_version=attempt.claim_version
        WHERE attempt.tenant_id=$1 AND attempt.outbox_event_id=$2
        ORDER BY attempt.claim_version DESC
        LIMIT 100
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(event_id)
    .fetch_all(&mut *tx)
    .await?;
    let attempts = attempt_rows
        .iter()
        .map(map_attempt)
        .collect::<AppResult<Vec<_>>>()?;
    let replay_rows = sqlx::query(
        r#"
        SELECT replay.id AS replay_id,replay.previous_replay_count,
               replay.resulting_replay_count,replay.previous_attempts,
               replay.last_error_snapshot,replay.replayed_by_user_id,replay.replayed_at,
               COALESCE(NULLIF(BTRIM(CONCAT_WS(' ',actor.first_name,actor.last_name)),''),
                        NULLIF(actor.nick_name,''),actor.email) AS replayed_by_name
        FROM outbox_dead_letter_replays replay
        JOIN users actor ON actor.id=replay.replayed_by_user_id
        WHERE replay.tenant_id=$1 AND replay.outbox_event_id=$2
        ORDER BY replay.resulting_replay_count DESC
        LIMIT 100
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(event_id)
    .fetch_all(&mut *tx)
    .await?;
    let replays = replay_rows
        .iter()
        .map(map_replay)
        .collect::<AppResult<Vec<_>>>()?;
    let discard = sqlx::query(
        r#"
        SELECT discard.id AS discard_id,discard.replay_count,discard.previous_attempts,
               discard.last_error_snapshot,discard.discard_reason,
               discard.discarded_by_user_id,discard.discarded_at,
               COALESCE(NULLIF(BTRIM(CONCAT_WS(' ',actor.first_name,actor.last_name)),''),
                        NULLIF(actor.nick_name,''),actor.email) AS discarded_by_name
        FROM outbox_dead_letter_discards discard
        JOIN users actor ON actor.id=discard.discarded_by_user_id
        WHERE discard.tenant_id=$1 AND discard.outbox_event_id=$2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(event_id)
    .fetch_optional(&mut *tx)
    .await?
    .as_ref()
    .map(map_discard)
    .transpose()?;
    tx.commit().await?;
    Ok(Some(OutboundIntegrationDetailReadModel {
        event,
        payload,
        attempts,
        replays,
        discard,
    }))
}

#[derive(Debug)]
struct ReplayTarget {
    inventory_owner_id: Option<i64>,
    facility_id: Option<i64>,
    event_key: String,
    event_type: String,
    replay_count: i32,
    attempts: i32,
    last_error: String,
}

fn require_target_scope(scope: &ScopeBindings, target: &ReplayTarget) -> AppResult<()> {
    if target
        .inventory_owner_id
        .is_some_and(|owner_id| !scope.includes_inventory_owner(owner_id))
        || target
            .facility_id
            .is_some_and(|facility_id| !scope.includes_facility(facility_id))
    {
        return Err(AppError::not_found("outbound integration event"));
    }
    Ok(())
}

async fn require_event_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    event_id: i64,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let visible = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM outbox_events event
            WHERE event.tenant_id=$1 AND event.id=$2
              AND (event.inventory_owner_id IS NULL OR $3 OR event.inventory_owner_id=ANY($4))
              AND (event.facility_id IS NULL OR $5 OR event.facility_id=ANY($6)))
        "#,
    )
    .bind(tenant_id)
    .bind(event_id)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .fetch_one(&mut **tx)
    .await?;
    if !visible {
        return Err(AppError::not_found("outbound integration event"));
    }
    Ok(())
}

async fn lock_replay_target_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    event_id: i64,
    scope: &ScopeBindings,
) -> AppResult<ReplayTarget> {
    let row = sqlx::query(
        r#"
        SELECT event.inventory_owner_id,event.facility_id,event.event_key,event.event_type,
               event.replay_count,event.attempts,event.last_error,event.dead_lettered_at,
               event.published_at,event.discarded_at,event.claimed_at
        FROM outbox_events event
        WHERE event.tenant_id=$1 AND event.id=$2
          AND (event.inventory_owner_id IS NULL OR $3 OR event.inventory_owner_id=ANY($4))
          AND (event.facility_id IS NULL OR $5 OR event.facility_id=ANY($6))
        FOR UPDATE
        "#,
    )
    .bind(tenant_id)
    .bind(event_id)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("outbound integration event"))?;
    if row
        .try_get::<Option<Timestamp>, _>("dead_lettered_at")?
        .is_none()
        || row
            .try_get::<Option<Timestamp>, _>("published_at")?
            .is_some()
        || row
            .try_get::<Option<Timestamp>, _>("discarded_at")?
            .is_some()
        || row.try_get::<Option<Timestamp>, _>("claimed_at")?.is_some()
    {
        return Err(AppError::conflict(
            "outbound integration event is not dead lettered",
        ));
    }
    let last_error = row
        .try_get::<Option<String>, _>("last_error")?
        .ok_or_else(|| AppError::conflict("dead-letter failure evidence is missing"))?;
    let target = ReplayTarget {
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        facility_id: row.try_get("facility_id")?,
        event_key: row.try_get("event_key")?,
        event_type: row.try_get("event_type")?,
        replay_count: row.try_get("replay_count")?,
        attempts: row.try_get("attempts")?,
        last_error,
    };
    require_target_scope(scope, &target)?;
    Ok(target)
}

pub async fn replay_dead_letter(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: ReplayOutboxDeadLetterCommand,
) -> AppResult<ReplayOutboxDeadLetterResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, REPLAY_OUTBOX_DEAD_LETTER_OPERATION, &command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "admin").await?;
    require_event_visible_tx(&mut tx, access.tenant_id.get(), command.event_id(), &scope).await?;
    if let Some(result) = prepared
        .replayed::<ReplayOutboxDeadLetterResult>(&mut tx)
        .await?
    {
        require_event_visible_tx(&mut tx, access.tenant_id.get(), result.event_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }

    let target =
        lock_replay_target_tx(&mut tx, access.tenant_id.get(), command.event_id(), &scope).await?;
    if target.replay_count != command.expected_replay_count() {
        return Err(AppError::conflict(
            "outbound integration event replay generation is stale",
        ));
    }
    if target.attempts <= 0 {
        return Err(AppError::conflict(
            "dead-letter event has no completed delivery attempt",
        ));
    }
    let row = sqlx::query(
        r#"
        INSERT INTO outbox_dead_letter_replays(
            tenant_id,outbox_event_id,inventory_owner_id,facility_id,event_key,event_type,
            previous_replay_count,resulting_replay_count,previous_attempts,last_error_snapshot,
            replayed_by_user_id,replayed_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$7+1,$8,$9,$10,clock_timestamp())
        RETURNING id,replayed_at
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.event_id())
    .bind(target.inventory_owner_id)
    .bind(target.facility_id)
    .bind(&target.event_key)
    .bind(&target.event_type)
    .bind(target.replay_count)
    .bind(target.attempts)
    .bind(&target.last_error)
    .bind(context.actor_id.get())
    .fetch_one(&mut *tx)
    .await?;
    let replay_id = OutboxDeadLetterReplayId::new(row.try_get("id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let replayed_at: Timestamp = row.try_get("replayed_at")?;
    let replay_count = target
        .replay_count
        .checked_add(1)
        .ok_or_else(|| AppError::conflict("outbound replay generation is exhausted"))?;
    let updated = sqlx::query(
        r#"
        UPDATE outbox_events
        SET available_at=$1,attempts=0,last_error=NULL,dead_lettered_at=NULL,
            replay_count=$2,claimed_at=NULL,claimed_by=NULL,lease_expires_at=NULL
        WHERE tenant_id=$3 AND id=$4 AND replay_count=$5
        "#,
    )
    .bind(replayed_at)
    .bind(replay_count)
    .bind(access.tenant_id.get())
    .bind(command.event_id())
    .bind(target.replay_count)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "outbound integration event changed during replay",
        ));
    }
    let result = ReplayOutboxDeadLetterResult {
        replay_id,
        event_id: command.event_id(),
        event_key: target.event_key,
        event_type: target.event_type,
        previous_replay_count: target.replay_count,
        replay_count,
        previous_attempts: target.attempts,
        status: OutboundDeliveryStatus::Pending,
        replayed_by: UserId::new(context.actor_id.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        replayed_at,
    };
    Ok(prepared.commit(tx, result).await?)
}

pub async fn discard_dead_letter(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: DiscardOutboxDeadLetterCommand,
) -> AppResult<DiscardOutboxDeadLetterResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared =
        PreparedCommand::new_v1(context, DISCARD_OUTBOX_DEAD_LETTER_OPERATION, &command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "admin").await?;
    require_event_visible_tx(&mut tx, access.tenant_id.get(), command.event_id(), &scope).await?;
    if let Some(result) = prepared
        .replayed::<DiscardOutboxDeadLetterResult>(&mut tx)
        .await?
    {
        require_event_visible_tx(&mut tx, access.tenant_id.get(), result.event_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }

    let target =
        lock_replay_target_tx(&mut tx, access.tenant_id.get(), command.event_id(), &scope).await?;
    if target.replay_count != command.expected_replay_count() {
        return Err(AppError::conflict(
            "outbound integration event replay generation is stale",
        ));
    }
    if target.attempts <= 0 {
        return Err(AppError::conflict(
            "dead-letter event has no completed delivery attempt",
        ));
    }
    let row = sqlx::query(
        r#"
        INSERT INTO outbox_dead_letter_discards(
            tenant_id,outbox_event_id,inventory_owner_id,facility_id,event_key,event_type,
            replay_count,previous_attempts,last_error_snapshot,discard_reason,
            discarded_by_user_id,discarded_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,clock_timestamp())
        RETURNING id,discarded_at
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.event_id())
    .bind(target.inventory_owner_id)
    .bind(target.facility_id)
    .bind(&target.event_key)
    .bind(&target.event_type)
    .bind(target.replay_count)
    .bind(target.attempts)
    .bind(&target.last_error)
    .bind(command.reason().as_str())
    .bind(context.actor_id.get())
    .fetch_one(&mut *tx)
    .await?;
    let discard_id = OutboxDeadLetterDiscardId::new(row.try_get("id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let discarded_at: Timestamp = row.try_get("discarded_at")?;
    let updated = sqlx::query(
        r#"
        UPDATE outbox_events
        SET discarded_at=$1,discard_reason=$2,discarded_by_user_id=$3
        WHERE tenant_id=$4 AND id=$5 AND replay_count=$6
        "#,
    )
    .bind(discarded_at)
    .bind(command.reason().as_str())
    .bind(context.actor_id.get())
    .bind(access.tenant_id.get())
    .bind(command.event_id())
    .bind(target.replay_count)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "outbound integration event changed during discard",
        ));
    }
    let result = DiscardOutboxDeadLetterResult {
        discard_id,
        event_id: command.event_id(),
        event_key: target.event_key,
        event_type: target.event_type,
        replay_count: target.replay_count,
        previous_attempts: target.attempts,
        reason: command.reason().clone(),
        status: OutboundDeliveryStatus::Discarded,
        discarded_by: UserId::new(context.actor_id.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        discarded_at,
    };
    Ok(prepared.commit(tx, result).await?)
}
