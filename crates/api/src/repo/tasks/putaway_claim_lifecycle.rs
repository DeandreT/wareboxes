use sqlx::Row;
use wareboxes_core::models::{
    PutawayClaimHeartbeat, PutawayClaimRelease, PutawayClaimReleaseReason, TenantAccess, Timestamp,
    WorkTaskType,
};
use wareboxes_domain::{CommandContext, InventoryOwnerId};

use crate::db::{bind_tenant_context, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, ScopeBindings};
use crate::repo::idempotency::{require_command_context, PreparedCommand};

use super::{insert_progress_tx, TaskDimensions};

const HEARTBEAT_OPERATION: &str = "putaway.heartbeat.v1";
const RELEASE_OPERATION: &str = "putaway.release.v1";
const MAX_RELEASE_NOTE_LENGTH: usize = 500;

struct LockedPutawayClaim {
    inventory_owner_id: InventoryOwnerId,
    facility_id: i64,
    lease_expires_at: Timestamp,
    task_timeout_seconds: i64,
}

async fn lock_active_putaway_claim_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    task_id: i64,
    scope: &ScopeBindings,
) -> AppResult<LockedPutawayClaim> {
    let row = sqlx::query(
        r#"
        SELECT task.task_type,
               task.status,
               task.assigned_user_id,
               task.lease_expires_at,
               task.lease_expires_at > statement_timestamp() AS lease_is_current,
               task.task_timeout_seconds,
               task.facility_id,
               task.inventory_owner_id,
               CASE
                   WHEN task.task_type = 'putaway' THEN EXISTS (
                       SELECT 1
                       FROM putaway_tasks detail
                       WHERE detail.tenant_id = task.tenant_id
                         AND detail.task_id = task.id
                         AND detail.closed_at IS NULL
                   )
                   WHEN task.task_type = 'license_plate_putaway' THEN EXISTS (
                       SELECT 1
                       FROM license_plate_putaway_tasks detail
                       WHERE detail.tenant_id = task.tenant_id
                         AND detail.task_id = task.id
                         AND detail.closed_at IS NULL
                   )
                   ELSE FALSE
               END AS detail_is_open
        FROM work_tasks task
        WHERE task.tenant_id = $1
          AND task.id = $2
          AND task.deleted IS NULL
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("putaway claim"))?;

    let task_type = WorkTaskType::parse(&row.try_get::<String, _>("task_type")?)
        .ok_or_else(|| AppError::internal("work task has an invalid type"))?;
    let dimensions = TaskDimensions {
        facility_id: row.try_get("facility_id")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
    };
    if !matches!(
        task_type,
        WorkTaskType::Putaway | WorkTaskType::LicensePlatePutaway
    ) || !dimensions.is_allowed_by(scope)
    {
        return Err(AppError::not_found("putaway claim"));
    }
    if row.try_get::<String, _>("status")? != "in_progress"
        || row.try_get::<Option<i64>, _>("assigned_user_id")? != Some(access.user_id.get())
        || row.try_get::<Option<bool>, _>("lease_is_current")? != Some(true)
        || !row.try_get::<bool, _>("detail_is_open")?
    {
        return Err(AppError::conflict("putaway claim is no longer active"));
    }

    let inventory_owner_id = InventoryOwnerId::new(
        dimensions
            .inventory_owner_id
            .ok_or_else(|| AppError::internal("putaway task has no inventory owner"))?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    Ok(LockedPutawayClaim {
        inventory_owner_id,
        facility_id: dimensions
            .facility_id
            .ok_or_else(|| AppError::internal("putaway task has no facility"))?,
        lease_expires_at: row
            .try_get::<Option<Timestamp>, _>("lease_expires_at")?
            .ok_or_else(|| AppError::conflict("putaway claim has no active lease"))?,
        task_timeout_seconds: row.try_get("task_timeout_seconds")?,
    })
}

async fn require_putaway_claim_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    task_id: i64,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"
        SELECT task_type, facility_id, inventory_owner_id
        FROM work_tasks
        WHERE tenant_id = $1
          AND id = $2
          AND deleted IS NULL
        FOR SHARE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("putaway claim"))?;
    let task_type = WorkTaskType::parse(&row.try_get::<String, _>("task_type")?)
        .ok_or_else(|| AppError::internal("work task has an invalid type"))?;
    let dimensions = TaskDimensions {
        facility_id: row.try_get("facility_id")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
    };
    if !matches!(
        task_type,
        WorkTaskType::Putaway | WorkTaskType::LicensePlatePutaway
    ) || !dimensions.is_allowed_by(scope)
    {
        return Err(AppError::not_found("putaway claim"));
    }
    Ok(())
}

fn validate_release_input(reason: PutawayClaimReleaseReason, note: Option<&str>) -> AppResult<()> {
    if let Some(note) = note {
        if note.trim() != note || note.is_empty() {
            return Err(AppError::bad_request(
                "release note must be trimmed and nonempty when provided",
            ));
        }
        if note.chars().count() > MAX_RELEASE_NOTE_LENGTH {
            return Err(AppError::bad_request(format!(
                "release note cannot exceed {MAX_RELEASE_NOTE_LENGTH} characters"
            )));
        }
    }
    if reason == PutawayClaimReleaseReason::Other && note.is_none() {
        return Err(AppError::bad_request(
            "release note is required when reason is other",
        ));
    }
    Ok(())
}

pub async fn heartbeat_putaway_claim_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
) -> AppResult<PutawayClaimHeartbeat> {
    require_command_context(access, command)?;
    if task_id <= 0 {
        return Err(AppError::bad_request("putaway task ID must be positive"));
    }
    let prepared = PreparedCommand::new(command, HEARTBEAT_OPERATION, &task_id)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, command.actor_id.get()).await?;

    if let Some(heartbeat) = prepared.replayed::<PutawayClaimHeartbeat>(&mut tx).await? {
        require_putaway_claim_visible_tx(&mut tx, access, task_id, &scope).await?;
        tx.commit().await?;
        return Ok(heartbeat);
    }

    let claim = lock_active_putaway_claim_tx(&mut tx, access, task_id, &scope).await?;
    let heartbeat_at = now_iso();
    let lease_expires_at: Timestamp = sqlx::query_scalar(
        r#"
        UPDATE work_tasks
        SET lease_expires_at = $1 + make_interval(secs => $2::INT),
            modified = $1
        WHERE tenant_id = $3
          AND id = $4
          AND deleted IS NULL
          AND task_type IN ('putaway', 'license_plate_putaway')
          AND status = 'in_progress'
          AND assigned_user_id = $5
          AND lease_expires_at > $1
        RETURNING lease_expires_at
        "#,
    )
    .bind(heartbeat_at)
    .bind(claim.task_timeout_seconds)
    .bind(access.tenant_id.get())
    .bind(task_id)
    .bind(command.actor_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::conflict("putaway claim is no longer active"))?;
    let metadata = serde_json::json!({
        "previous_lease_expires_at": claim.lease_expires_at.to_rfc3339(),
        "lease_expires_at": lease_expires_at.to_rfc3339(),
    })
    .to_string();
    insert_progress_tx(
        &mut tx,
        access.tenant_id,
        task_id,
        None,
        Some(command.actor_id.get()),
        "putaway_heartbeat",
        None,
        None,
        None,
        None,
        Some(&metadata),
    )
    .await?;

    let heartbeat = PutawayClaimHeartbeat {
        tenant_id: access.tenant_id,
        task_id,
        inventory_owner_id: claim.inventory_owner_id,
        facility_id: claim.facility_id,
        heartbeat_by: command.actor_id.get(),
        heartbeat_at,
        previous_lease_expires_at: claim.lease_expires_at,
        lease_expires_at,
    };
    prepared.commit(tx, heartbeat).await
}

pub async fn release_putaway_claim_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
    reason: PutawayClaimReleaseReason,
    note: Option<&str>,
) -> AppResult<PutawayClaimRelease> {
    require_command_context(access, command)?;
    if task_id <= 0 {
        return Err(AppError::bad_request("putaway task ID must be positive"));
    }
    validate_release_input(reason, note)?;
    let prepared = PreparedCommand::new(command, RELEASE_OPERATION, &(task_id, reason, note))?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, command.actor_id.get()).await?;

    if let Some(release) = prepared.replayed::<PutawayClaimRelease>(&mut tx).await? {
        require_putaway_claim_visible_tx(&mut tx, access, task_id, &scope).await?;
        tx.commit().await?;
        return Ok(release);
    }

    let claim = lock_active_putaway_claim_tx(&mut tx, access, task_id, &scope).await?;
    let released_at = now_iso();
    let release_count: i64 = sqlx::query_scalar(
        r#"
        UPDATE work_tasks
        SET status = 'open',
            assigned_user_id = NULL,
            started_at = NULL,
            lease_expires_at = NULL,
            last_released_at = $1,
            release_count = release_count + 1,
            modified = $1
        WHERE tenant_id = $2
          AND id = $3
          AND deleted IS NULL
          AND task_type IN ('putaway', 'license_plate_putaway')
          AND status = 'in_progress'
          AND assigned_user_id = $4
          AND lease_expires_at > $1
          AND completed_at IS NULL
        RETURNING release_count
        "#,
    )
    .bind(released_at)
    .bind(access.tenant_id.get())
    .bind(task_id)
    .bind(command.actor_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::conflict("putaway claim is no longer active"))?;
    let metadata = serde_json::json!({
        "release_count": release_count,
        "reason": reason.as_str(),
        "note": note,
    })
    .to_string();
    insert_progress_tx(
        &mut tx,
        access.tenant_id,
        task_id,
        None,
        Some(command.actor_id.get()),
        "putaway_released",
        None,
        None,
        None,
        note,
        Some(&metadata),
    )
    .await?;

    let release = PutawayClaimRelease {
        tenant_id: access.tenant_id,
        task_id,
        inventory_owner_id: claim.inventory_owner_id,
        facility_id: claim.facility_id,
        released_by: command.actor_id.get(),
        released_at,
        release_count,
        reason,
        note: note.map(str::to_owned),
    };
    prepared.commit(tx, release).await
}
