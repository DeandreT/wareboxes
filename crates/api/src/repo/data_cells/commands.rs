use sqlx::Row;
use wareboxes_application::data_cell::{
    ChangeDataCellStatusCommand, ChangeDataCellStatusResult, ReconfigureDataCellCommand,
    ReconfigureDataCellResult, RegisterDataCellCommand, RegisterDataCellResult,
    CHANGE_DATA_CELL_STATUS_OPERATION, RECONFIGURE_DATA_CELL_OPERATION,
    REGISTER_DATA_CELL_OPERATION,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{DataCellId, DataCellMode, DataCellStatus};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use super::events::{self, DataCellEvent};
use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};

fn revision_conflict() -> AppError {
    AppError::conflict("data-cell revision does not match expected revision")
}

pub async fn register(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &RegisterDataCellCommand,
) -> AppResult<RegisterDataCellResult> {
    context.require_actor(actor_access.tenant_id, actor_access.user_id)?;
    command
        .mode
        .validate_capacity(command.max_tenants)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let prepared = PreparedCommand::new_v1(context, REGISTER_DATA_CELL_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    crate::repo::tenant_lifecycle::authorize_tx(&mut tx, actor_access, context.actor_id).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("platform-data-cell-key:{}", command.key.as_str()))
        .execute(&mut *tx)
        .await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM data_cells WHERE cell_key=$1)")
            .bind(command.key.as_str())
            .fetch_one(&mut *tx)
            .await?;
    if exists {
        return Err(AppError::conflict("data-cell key already exists"));
    }
    let occurred_at = now_iso();
    let data_cell_id = DataCellId::new(
        sqlx::query_scalar(
            r#"INSERT INTO data_cells
            (cell_key,name,region,residency_code,mode,status,revision,max_tenants,
             created_at,created_by_user_id)
            VALUES($1,$2,$3,$4,$5,'provisioning',1,$6,$7,$8) RETURNING id"#,
        )
        .bind(command.key.as_str())
        .bind(command.name.as_str())
        .bind(command.region.as_str())
        .bind(command.residency.as_str())
        .bind(command.mode.as_str())
        .bind(i64::from(command.max_tenants.get()))
        .bind(occurred_at)
        .bind(context.actor_id.get())
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    let evidence = serde_json::json!({
        "data_cell_id": data_cell_id.get(),
        "key": command.key.as_str(),
        "name": command.name.as_str(),
        "region": command.region.as_str(),
        "residency": command.residency.as_str(),
        "mode": command.mode.as_str(),
        "status": "provisioning",
        "revision": 1,
        "max_tenants": command.max_tenants.get(),
        "actor_user_id": context.actor_id.get(),
        "occurred_at": occurred_at,
    });
    events::record_tx(
        &mut tx,
        actor_access,
        &DataCellEvent {
            data_cell_id,
            action: "registered",
            revision: 1,
            previous_status: None,
            resulting_status: DataCellStatus::Provisioning,
            actor_id: context.actor_id,
            occurred_at,
            reason: None,
            request_id: &context.request_id,
            evidence: &evidence,
        },
    )
    .await?;
    let result = super::query::read_tx(&mut tx, data_cell_id).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn reconfigure(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &ReconfigureDataCellCommand,
) -> AppResult<ReconfigureDataCellResult> {
    context.require_actor(actor_access.tenant_id, actor_access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, RECONFIGURE_DATA_CELL_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    crate::repo::tenant_lifecycle::authorize_tx(&mut tx, actor_access, context.actor_id).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let row = sqlx::query(
        r#"SELECT name,mode,status,revision,max_tenants,
        (SELECT COUNT(*) FROM tenant_cell_placements placement
          WHERE placement.data_cell_id=cell.id) AS placement_count
        FROM data_cells cell WHERE id=$1 FOR UPDATE"#,
    )
    .bind(command.data_cell_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("data cell"))?;
    let revision: i64 = row.try_get("revision")?;
    if revision != command.expected_revision.get() {
        return Err(revision_conflict());
    }
    let mode = DataCellMode::parse(row.try_get("mode")?)
        .ok_or_else(|| AppError::internal("stored data-cell mode is invalid"))?;
    mode.validate_capacity(command.max_tenants)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let placement_count: i64 = row.try_get("placement_count")?;
    if i64::from(command.max_tenants.get()) < placement_count {
        return Err(AppError::conflict(
            "data-cell capacity cannot be lower than its current placements",
        ));
    }
    let current_name: String = row.try_get("name")?;
    let current_capacity: i64 = row.try_get("max_tenants")?;
    if current_name == command.name.as_str()
        && current_capacity == i64::from(command.max_tenants.get())
    {
        return Err(AppError::bad_request(
            "data-cell reconfiguration must change name or capacity",
        ));
    }
    let status = parse_status(row.try_get("status")?)?;
    let next_revision = command
        .expected_revision
        .checked_next()
        .ok_or_else(|| AppError::internal("data-cell revision overflow"))?;
    let occurred_at = now_iso();
    sqlx::query(
        r#"UPDATE data_cells SET name=$2,max_tenants=$3,revision=$4,
        changed_at=$5,changed_by_user_id=$6,change_reason=$7 WHERE id=$1"#,
    )
    .bind(command.data_cell_id.get())
    .bind(command.name.as_str())
    .bind(i64::from(command.max_tenants.get()))
    .bind(next_revision.get())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .bind(command.reason.as_str())
    .execute(&mut *tx)
    .await?;
    let evidence = serde_json::json!({
        "data_cell_id": command.data_cell_id.get(),
        "action": "reconfigured",
        "revision": next_revision.get(),
        "status": status.as_str(),
        "previous_name": current_name,
        "name": command.name.as_str(),
        "previous_max_tenants": current_capacity,
        "max_tenants": command.max_tenants.get(),
        "placement_count": placement_count,
        "reason": command.reason.as_str(),
        "actor_user_id": context.actor_id.get(),
        "occurred_at": occurred_at,
    });
    events::record_tx(
        &mut tx,
        actor_access,
        &DataCellEvent {
            data_cell_id: command.data_cell_id,
            action: "reconfigured",
            revision: next_revision.get(),
            previous_status: Some(status),
            resulting_status: status,
            actor_id: context.actor_id,
            occurred_at,
            reason: Some(command.reason.as_str()),
            request_id: &context.request_id,
            evidence: &evidence,
        },
    )
    .await?;
    let result = super::query::read_tx(&mut tx, command.data_cell_id).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn change_status(
    db: &Db,
    actor_access: &TenantAccess,
    context: &CommandContext,
    command: &ChangeDataCellStatusCommand,
) -> AppResult<ChangeDataCellStatusResult> {
    context.require_actor(actor_access.tenant_id, actor_access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CHANGE_DATA_CELL_STATUS_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    crate::repo::tenant_lifecycle::authorize_tx(&mut tx, actor_access, context.actor_id).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let row = sqlx::query(
        r#"SELECT status,revision,
        (SELECT COUNT(*) FROM tenant_cell_placements placement
          WHERE placement.data_cell_id=cell.id) AS placement_count
        FROM data_cells cell WHERE id=$1 FOR UPDATE"#,
    )
    .bind(command.data_cell_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("data cell"))?;
    let revision: i64 = row.try_get("revision")?;
    if revision != command.expected_revision.get() {
        return Err(revision_conflict());
    }
    let current = parse_status(row.try_get("status")?)?;
    current
        .require_transition(command.status)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let placement_count: i64 = row.try_get("placement_count")?;
    if command.status == DataCellStatus::Retired && placement_count != 0 {
        return Err(AppError::conflict(
            "data cell cannot be retired while tenants remain placed",
        ));
    }
    let next_revision = command
        .expected_revision
        .checked_next()
        .ok_or_else(|| AppError::internal("data-cell revision overflow"))?;
    let occurred_at = now_iso();
    sqlx::query(
        r#"UPDATE data_cells SET status=$2,revision=$3,changed_at=$4,
        changed_by_user_id=$5,change_reason=$6 WHERE id=$1"#,
    )
    .bind(command.data_cell_id.get())
    .bind(command.status.as_str())
    .bind(next_revision.get())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .bind(command.reason.as_str())
    .execute(&mut *tx)
    .await?;
    let action = match (current, command.status) {
        (DataCellStatus::Provisioning, DataCellStatus::Active) => "activated",
        (DataCellStatus::Active, DataCellStatus::Draining) => "draining",
        (DataCellStatus::Draining, DataCellStatus::Active) => "reactivated",
        (DataCellStatus::Draining, DataCellStatus::Retired) => "retired",
        _ => return Err(AppError::internal("unhandled data-cell transition")),
    };
    let evidence = serde_json::json!({
        "data_cell_id": command.data_cell_id.get(),
        "action": action,
        "previous_status": current.as_str(),
        "resulting_status": command.status.as_str(),
        "revision": next_revision.get(),
        "placement_count": placement_count,
        "reason": command.reason.as_str(),
        "actor_user_id": context.actor_id.get(),
        "occurred_at": occurred_at,
    });
    events::record_tx(
        &mut tx,
        actor_access,
        &DataCellEvent {
            data_cell_id: command.data_cell_id,
            action,
            revision: next_revision.get(),
            previous_status: Some(current),
            resulting_status: command.status,
            actor_id: context.actor_id,
            occurred_at,
            reason: Some(command.reason.as_str()),
            request_id: &context.request_id,
            evidence: &evidence,
        },
    )
    .await?;
    let result = super::query::read_tx(&mut tx, command.data_cell_id).await?;
    Ok(prepared.commit(tx, result).await?)
}

fn parse_status(value: String) -> AppResult<DataCellStatus> {
    DataCellStatus::parse(&value)
        .ok_or_else(|| AppError::internal(format!("stored data-cell status is invalid: {value}")))
}
