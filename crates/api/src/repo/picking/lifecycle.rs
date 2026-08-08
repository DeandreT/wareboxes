use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::picking::{
    HeartbeatPickClaimCommand, PickClaimHeartbeatResult, PickClaimReleaseResult,
    ReleasePickClaimCommand,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{PickClaimReleaseReason, PickTaskId, TenantId, Timestamp};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};

use super::claim::require_task_visible_tx;
use super::{HEARTBEAT_OPERATION, MAX_RELEASE_NOTE_LENGTH, RELEASE_OPERATION};

#[derive(Debug)]
struct ActiveClaim {
    lease_expires_at: Timestamp,
    task_timeout_seconds: i64,
}

pub async fn heartbeat(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: HeartbeatPickClaimCommand,
) -> AppResult<PickClaimHeartbeatResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, HEARTBEAT_OPERATION, &command.task_id)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;

    if let Some(result) = prepared
        .replayed::<PickClaimHeartbeatResult>(&mut tx)
        .await?
    {
        require_task_visible_tx(&mut tx, access.tenant_id, command.task_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }

    let active = lock_active_claim_tx(
        &mut tx,
        access.tenant_id,
        command.task_id,
        context.actor_id.get(),
        &scope,
    )
    .await?;
    let heartbeat_at = now_iso();
    if active.lease_expires_at <= heartbeat_at {
        return Err(AppError::conflict("pick claim has expired"));
    }
    let lease_expires_at = heartbeat_at + chrono::Duration::seconds(active.task_timeout_seconds);
    let updated = sqlx::query(
        r#"
        UPDATE pick_tasks SET lease_expires_at = $1
        WHERE tenant_id = $2 AND id = $3 AND status = 'in_progress'
          AND assigned_user_id = $4 AND lease_expires_at > statement_timestamp()
        "#,
    )
    .bind(lease_expires_at)
    .bind(access.tenant_id.get())
    .bind(command.task_id.get())
    .bind(context.actor_id.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("pick claim expired during heartbeat"));
    }
    let result = PickClaimHeartbeatResult {
        task_id: command.task_id,
        heartbeat_at,
        lease_expires_at,
    };
    Ok(prepared.commit(tx, result).await?)
}

pub async fn release_claim(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: ReleasePickClaimCommand,
) -> AppResult<PickClaimReleaseResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let note = validate_note(command.reason, command.note.as_deref())?;
    let fingerprint = (
        command.task_id,
        release_reason_value(command.reason),
        note.as_deref(),
    );
    let prepared = PreparedCommand::new_v1(context, RELEASE_OPERATION, &fingerprint)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;

    if let Some(result) = prepared.replayed::<PickClaimReleaseResult>(&mut tx).await? {
        require_task_visible_tx(&mut tx, access.tenant_id, command.task_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }

    let active = lock_active_claim_tx(
        &mut tx,
        access.tenant_id,
        command.task_id,
        context.actor_id.get(),
        &scope,
    )
    .await?;
    let released_at = now_iso();
    if active.lease_expires_at <= released_at {
        return Err(AppError::conflict("pick claim has expired"));
    }
    let row = sqlx::query(
        r#"
        UPDATE pick_tasks
        SET status = 'open', assigned_user_id = NULL, claimed_at = NULL,
            lease_expires_at = NULL, last_released_at = $1,
            last_release_reason = $2, last_release_note = $3,
            release_count = release_count + 1
        WHERE tenant_id = $4 AND id = $5 AND status = 'in_progress'
          AND assigned_user_id = $6 AND lease_expires_at > statement_timestamp()
        RETURNING release_count
        "#,
    )
    .bind(released_at)
    .bind(release_reason_value(command.reason))
    .bind(note.as_deref())
    .bind(access.tenant_id.get())
    .bind(command.task_id.get())
    .bind(context.actor_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::conflict("pick claim expired during release"))?;
    let result = PickClaimReleaseResult {
        task_id: command.task_id,
        released_at,
        release_count: row.try_get("release_count")?,
        reason: command.reason,
        note,
    };
    Ok(prepared.commit(tx, result).await?)
}

async fn lock_active_claim_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    task_id: PickTaskId,
    actor_user_id: i64,
    scope: &ScopeBindings,
) -> AppResult<ActiveClaim> {
    let row = sqlx::query(
        r#"
        SELECT facility_id, inventory_owner_id, lease_expires_at,
               task_timeout_seconds
        FROM pick_tasks
        WHERE tenant_id = $1 AND id = $2 AND status = 'in_progress'
          AND assigned_user_id = $3
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id.get())
    .bind(actor_user_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("pick claim is not active for this operator"))?;
    if !scope.includes_facility(row.try_get("facility_id")?)
        || !scope.includes_inventory_owner(row.try_get("inventory_owner_id")?)
    {
        return Err(AppError::not_found("pick task"));
    }
    Ok(ActiveClaim {
        lease_expires_at: row.try_get("lease_expires_at")?,
        task_timeout_seconds: row.try_get("task_timeout_seconds")?,
    })
}

fn validate_note(reason: PickClaimReleaseReason, note: Option<&str>) -> AppResult<Option<String>> {
    let note = match note {
        Some(note)
            if note.trim() == note
                && !note.is_empty()
                && note.chars().count() <= MAX_RELEASE_NOTE_LENGTH =>
        {
            Some(note.to_owned())
        }
        Some(_) => {
            return Err(AppError::bad_request(format!(
                "release note must be trimmed, nonempty, and at most {MAX_RELEASE_NOTE_LENGTH} characters"
            )));
        }
        None => None,
    };
    if reason == PickClaimReleaseReason::Other && note.is_none() {
        return Err(AppError::bad_request(
            "release note is required when reason is other",
        ));
    }
    Ok(note)
}

pub(super) const fn release_reason_value(reason: PickClaimReleaseReason) -> &'static str {
    match reason {
        PickClaimReleaseReason::WorkInterrupted => "work_interrupted",
        PickClaimReleaseReason::EquipmentUnavailable => "equipment_unavailable",
        PickClaimReleaseReason::SourceBlocked => "source_blocked",
        PickClaimReleaseReason::InventoryDiscrepancy => "inventory_discrepancy",
        PickClaimReleaseReason::SafetyIssue => "safety_issue",
        PickClaimReleaseReason::Other => "other",
    }
}
