use sqlx::Row;
use wareboxes_application::CommandContext;
use wareboxes_core::models::{
    CycleCountClaimHeartbeat, CycleCountClaimRelease, CycleCountClaimReleaseReason, TenantAccess,
    Timestamp,
};
use wareboxes_domain::InventoryOwnerId;

use crate::db::{bind_tenant_context, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, ScopeBindings};
use crate::repo::idempotency::{require_command_context, PreparedCommand};

use super::{insert_progress_tx, TaskDimensions};

const HEARTBEAT_OPERATION: &str = "cycle_count.heartbeat.v1";
const RELEASE_OPERATION: &str = "cycle_count.release.v1";
const MAX_RELEASE_NOTE_LENGTH: usize = 500;

struct LockedClaim {
    inventory_owner_id: InventoryOwnerId,
    facility_id: i64,
    lease_expires_at: Timestamp,
    task_timeout_seconds: i64,
}

async fn require_claim_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    task_id: i64,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"
        SELECT task_type, facility_id, inventory_owner_id
        FROM work_tasks
        WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL
        FOR SHARE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("cycle count claim"))?;
    let dimensions = TaskDimensions {
        facility_id: row.try_get("facility_id")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
    };
    if row.try_get::<String, _>("task_type")? != "cycle_count_item_location"
        || !dimensions.is_allowed_by(scope)
    {
        return Err(AppError::not_found("cycle count claim"));
    }
    Ok(())
}

async fn lock_active_claim_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    task_id: i64,
    scope: &ScopeBindings,
) -> AppResult<LockedClaim> {
    let row = sqlx::query(
        r#"
        SELECT task.status,
               task.assigned_user_id,
               task.lease_expires_at,
               task.lease_expires_at > statement_timestamp() AS lease_is_current,
               task.task_timeout_seconds,
               task.facility_id,
               task.inventory_owner_id,
               EXISTS (
                   SELECT 1
                   FROM cycle_count_item_location_tasks detail
                   WHERE detail.tenant_id = task.tenant_id
                     AND detail.task_id = task.id
               ) AS detail_exists
        FROM work_tasks task
        WHERE task.tenant_id = $1
          AND task.id = $2
          AND task.task_type = 'cycle_count_item_location'
          AND task.deleted IS NULL
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("cycle count claim"))?;
    let dimensions = TaskDimensions {
        facility_id: row.try_get("facility_id")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
    };
    if !dimensions.is_allowed_by(scope) {
        return Err(AppError::not_found("cycle count claim"));
    }
    if row.try_get::<String, _>("status")? != "in_progress"
        || row.try_get::<Option<i64>, _>("assigned_user_id")? != Some(access.user_id.get())
        || row.try_get::<Option<bool>, _>("lease_is_current")? != Some(true)
        || !row.try_get::<bool, _>("detail_exists")?
    {
        return Err(AppError::conflict("cycle count claim is no longer active"));
    }
    Ok(LockedClaim {
        inventory_owner_id: InventoryOwnerId::new(
            dimensions
                .inventory_owner_id
                .ok_or_else(|| AppError::internal("cycle count task has no inventory owner"))?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: dimensions
            .facility_id
            .ok_or_else(|| AppError::internal("cycle count task has no facility"))?,
        lease_expires_at: row
            .try_get::<Option<Timestamp>, _>("lease_expires_at")?
            .ok_or_else(|| AppError::conflict("cycle count claim has no active lease"))?,
        task_timeout_seconds: row.try_get("task_timeout_seconds")?,
    })
}

fn validate_release_input(
    reason: CycleCountClaimReleaseReason,
    note: Option<&str>,
) -> AppResult<()> {
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
    if reason == CycleCountClaimReleaseReason::Other && note.is_none() {
        return Err(AppError::bad_request(
            "release note is required when reason is other",
        ));
    }
    Ok(())
}

pub async fn heartbeat_cycle_count_claim_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
) -> AppResult<CycleCountClaimHeartbeat> {
    require_command_context(access, command)?;
    if task_id <= 0 {
        return Err(AppError::bad_request(
            "cycle count task ID must be positive",
        ));
    }
    let prepared = PreparedCommand::new(command, HEARTBEAT_OPERATION, &task_id)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, command.actor_id.get()).await?;
    if let Some(heartbeat) = prepared
        .replayed::<CycleCountClaimHeartbeat>(&mut tx)
        .await?
    {
        require_claim_visible_tx(&mut tx, access, task_id, &scope).await?;
        tx.commit().await?;
        return Ok(heartbeat);
    }

    let claim = lock_active_claim_tx(&mut tx, access, task_id, &scope).await?;
    let heartbeat_at = now_iso();
    let lease_expires_at: Timestamp = sqlx::query_scalar(
        r#"
        UPDATE work_tasks
        SET lease_expires_at = $1 + make_interval(secs => $2::INT), modified = $1
        WHERE tenant_id = $3
          AND id = $4
          AND task_type = 'cycle_count_item_location'
          AND status = 'in_progress'
          AND assigned_user_id = $5
          AND lease_expires_at > $1
          AND deleted IS NULL
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
    .ok_or_else(|| AppError::conflict("cycle count claim is no longer active"))?;
    insert_progress_tx(
        &mut tx,
        access.tenant_id,
        task_id,
        None,
        Some(command.actor_id.get()),
        "cycle_count_heartbeat",
        None,
        None,
        None,
        None,
        Some(
            &serde_json::json!({
                "previous_lease_expires_at": claim.lease_expires_at.to_rfc3339(),
                "lease_expires_at": lease_expires_at.to_rfc3339(),
            })
            .to_string(),
        ),
    )
    .await?;
    prepared
        .commit(
            tx,
            CycleCountClaimHeartbeat {
                tenant_id: access.tenant_id,
                task_id,
                inventory_owner_id: claim.inventory_owner_id,
                facility_id: claim.facility_id,
                heartbeat_by: command.actor_id.get(),
                heartbeat_at,
                previous_lease_expires_at: claim.lease_expires_at,
                lease_expires_at,
            },
        )
        .await
}

pub async fn release_cycle_count_claim_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
    reason: CycleCountClaimReleaseReason,
    note: Option<&str>,
) -> AppResult<CycleCountClaimRelease> {
    require_command_context(access, command)?;
    if task_id <= 0 {
        return Err(AppError::bad_request(
            "cycle count task ID must be positive",
        ));
    }
    validate_release_input(reason, note)?;
    let prepared = PreparedCommand::new(command, RELEASE_OPERATION, &(task_id, reason, note))?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, command.actor_id.get()).await?;
    if let Some(release) = prepared.replayed::<CycleCountClaimRelease>(&mut tx).await? {
        require_claim_visible_tx(&mut tx, access, task_id, &scope).await?;
        tx.commit().await?;
        return Ok(release);
    }

    let claim = lock_active_claim_tx(&mut tx, access, task_id, &scope).await?;
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
          AND task_type = 'cycle_count_item_location'
          AND status = 'in_progress'
          AND assigned_user_id = $4
          AND lease_expires_at > $1
          AND completed_at IS NULL
          AND deleted IS NULL
        RETURNING release_count
        "#,
    )
    .bind(released_at)
    .bind(access.tenant_id.get())
    .bind(task_id)
    .bind(command.actor_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::conflict("cycle count claim is no longer active"))?;
    insert_progress_tx(
        &mut tx,
        access.tenant_id,
        task_id,
        None,
        Some(command.actor_id.get()),
        "cycle_count_released",
        None,
        None,
        None,
        note,
        Some(
            &serde_json::json!({
                "release_count": release_count,
                "reason": reason.as_str(),
                "note": note,
            })
            .to_string(),
        ),
    )
    .await?;
    prepared
        .commit(
            tx,
            CycleCountClaimRelease {
                tenant_id: access.tenant_id,
                task_id,
                inventory_owner_id: claim.inventory_owner_id,
                facility_id: claim.facility_id,
                released_by: command.actor_id.get(),
                released_at,
                release_count,
                reason,
                note: note.map(str::to_owned),
            },
        )
        .await
}
