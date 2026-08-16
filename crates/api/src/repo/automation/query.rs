use wareboxes_application::automation::{AutomationWorkspaceFilter, AutomationWorkspaceReadModel};
use wareboxes_core::models::TenantAccess;
use wareboxes_persistence_postgres::db::{bind_tenant_context, Db};

use crate::error::{AppError, AppResult};
use crate::repo::access::{current_scope_tx, require_permission_tx};

use super::mapping;
use super::{MAX_WORKSPACE_ROWS, SUPERVISOR_PERMISSION};

pub async fn workspace(
    db: &Db,
    access: &TenantAccess,
    filter: &AutomationWorkspaceFilter,
) -> AppResult<AutomationWorkspaceReadModel> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        SUPERVISOR_PERMISSION,
    )
    .await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    if let Some(facility_id) = filter.facility_id {
        if !scope.includes_facility(facility_id.get()) {
            return Err(AppError::not_found("automation workspace"));
        }
    }
    let facility_ids = &scope.facility_ids;
    let limit = MAX_WORKSPACE_ROWS + 1;
    let device_rows = sqlx::query(&format!(
        r#"SELECT {} FROM automation_devices
        WHERE tenant_id=$1
          AND ($2::bigint IS NULL OR facility_id=$2)
          AND ($3 OR facility_id=ANY($4))
        ORDER BY facility_id,lower(display_name),id LIMIT $5"#,
        mapping::DEVICE_COLUMNS
    ))
    .bind(access.tenant_id.get())
    .bind(filter.facility_id.map(|id| id.get()))
    .bind(scope.all_facilities)
    .bind(facility_ids)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    let command_rows = sqlx::query(&format!(
        r#"SELECT {} FROM automation_commands command
        JOIN automation_devices device ON device.tenant_id=command.tenant_id
          AND device.id=command.device_id
        WHERE command.tenant_id=$1
          AND ($2::bigint IS NULL OR command.facility_id=$2)
          AND ($3 OR command.facility_id=ANY($4))
          AND ($5 OR command.status NOT IN
            ('succeeded','failed','resolved_manually','cancelled'))
        ORDER BY command.requested_at DESC,command.id DESC LIMIT $6"#,
        mapping::COMMAND_COLUMNS
    ))
    .bind(access.tenant_id.get())
    .bind(filter.facility_id.map(|id| id.get()))
    .bind(scope.all_facilities)
    .bind(facility_ids)
    .bind(filter.include_history)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    let heartbeat_rows = sqlx::query(
        r#"SELECT heartbeat.id,heartbeat.device_id,heartbeat.service_account_id,
        heartbeat.agent_instance,heartbeat.health,heartbeat.control_mode,heartbeat.message,
        heartbeat.queued_commands,heartbeat.manual_review_commands,heartbeat.observed_at,
        heartbeat.received_at
        FROM automation_heartbeats heartbeat
        WHERE heartbeat.tenant_id=$1
          AND ($2::bigint IS NULL OR heartbeat.facility_id=$2)
          AND ($3 OR heartbeat.facility_id=ANY($4))
          AND ($5 OR heartbeat.id IN (
            SELECT max(latest.id) FROM automation_heartbeats latest
            WHERE latest.tenant_id=$1 GROUP BY latest.device_id))
        ORDER BY heartbeat.observed_at DESC,heartbeat.id DESC LIMIT $6"#,
    )
    .bind(access.tenant_id.get())
    .bind(filter.facility_id.map(|id| id.get()))
    .bind(scope.all_facilities)
    .bind(facility_ids)
    .bind(filter.include_history)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    let truncated = device_rows.len() > MAX_WORKSPACE_ROWS as usize
        || command_rows.len() > MAX_WORKSPACE_ROWS as usize
        || heartbeat_rows.len() > MAX_WORKSPACE_ROWS as usize;
    let devices = device_rows
        .iter()
        .take(MAX_WORKSPACE_ROWS as usize)
        .map(mapping::device)
        .collect::<AppResult<Vec<_>>>()?;
    let commands = command_rows
        .iter()
        .take(MAX_WORKSPACE_ROWS as usize)
        .map(mapping::command)
        .collect::<AppResult<Vec<_>>>()?;
    let heartbeats = heartbeat_rows
        .iter()
        .take(MAX_WORKSPACE_ROWS as usize)
        .map(mapping::heartbeat)
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(AutomationWorkspaceReadModel {
        devices,
        commands,
        heartbeats,
        truncated,
    })
}
