use wareboxes_application::automation::{
    AutomationCommandReadModel, AutomationDeviceReadModel, PackingScaleCommandContext,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{AutomationCommandId, AutomationDeviceId, CartonId, PackSessionId};
use wareboxes_persistence_postgres::db::{bind_tenant_context, Db};

use crate::error::{AppError, AppResult};
use crate::repo::access::{current_scope_tx, require_permission_tx};
use crate::repo::automation::mapping;

pub async fn packing_scale_devices(
    db: &Db,
    access: &TenantAccess,
    session_id: PackSessionId,
) -> AppResult<Vec<AutomationDeviceReadModel>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    let facility_id: i64 = sqlx::query_scalar(
        r#"SELECT facility_id FROM packing_sessions
           WHERE tenant_id=$1 AND id=$2 AND state='open'
             AND ($3 OR facility_id=ANY($4))
             AND ($5 OR inventory_owner_id=ANY($6))"#,
    )
    .bind(access.tenant_id.get())
    .bind(session_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("packing session"))?;
    let rows = sqlx::query(&format!(
        r#"SELECT {} FROM automation_devices
           WHERE tenant_id=$1 AND facility_id=$2 AND device_class='scale'
             AND control_mode='automatic' AND health IN ('healthy','degraded')
             AND last_heartbeat_at>=CURRENT_TIMESTAMP-INTERVAL '2 minutes'
             AND EXISTS(SELECT 1 FROM automation_heartbeats heartbeat
               WHERE heartbeat.tenant_id=automation_devices.tenant_id
                 AND heartbeat.device_id=automation_devices.id
                 AND heartbeat.observed_at=automation_devices.last_heartbeat_at
                 AND heartbeat.control_mode='automatic')
           ORDER BY lower(display_name),id"#,
        mapping::DEVICE_COLUMNS
    ))
    .bind(access.tenant_id.get())
    .bind(facility_id)
    .fetch_all(&mut *tx)
    .await?;
    let result = rows
        .iter()
        .map(mapping::device)
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(result)
}

pub async fn require_packing_scale_device(
    db: &Db,
    access: &TenantAccess,
    session_id: PackSessionId,
    carton_id: CartonId,
    device_id: AutomationDeviceId,
) -> AppResult<PackingScaleCommandContext> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    let row: (i64, i64) = sqlx::query_as(
        r#"SELECT session.inventory_owner_id,carton.reopen_count
           FROM packing_sessions session
           INNER JOIN cartons carton
             ON carton.tenant_id=session.tenant_id
            AND carton.inventory_owner_id=session.inventory_owner_id
            AND carton.facility_id=session.facility_id
            AND carton.packing_session_id=session.id
            AND carton.id=$3 AND carton.state='open'
           INNER JOIN automation_devices device
             ON device.tenant_id=session.tenant_id AND device.facility_id=session.facility_id
            AND device.id=$4 AND device.device_class='scale'
            AND device.control_mode='automatic' AND device.health IN ('healthy','degraded')
            AND device.last_heartbeat_at>=CURRENT_TIMESTAMP-INTERVAL '2 minutes'
           WHERE session.tenant_id=$1 AND session.id=$2 AND session.state='open'
             AND ($5 OR session.facility_id=ANY($6))
             AND ($7 OR session.inventory_owner_id=ANY($8))
             AND EXISTS(SELECT 1 FROM automation_heartbeats heartbeat
               WHERE heartbeat.tenant_id=device.tenant_id AND heartbeat.device_id=device.id
                 AND heartbeat.observed_at=device.last_heartbeat_at
                 AND heartbeat.control_mode='automatic')"#,
    )
    .bind(access.tenant_id.get())
    .bind(session_id.get())
    .bind(carton_id.get())
    .bind(device_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("packing scale device"))?;
    tx.commit().await?;
    Ok(PackingScaleCommandContext {
        inventory_owner_id: wareboxes_domain::InventoryOwnerId::new(row.0)
            .map_err(|error| AppError::internal(error.to_string()))?,
        session_id,
        carton_id,
        carton_reopen_count: row.1,
    })
}

pub async fn packing_scale_reading(
    db: &Db,
    access: &TenantAccess,
    session_id: PackSessionId,
    command_id: AutomationCommandId,
) -> AppResult<AutomationCommandReadModel> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    let row = sqlx::query(&format!(
        r#"SELECT {} FROM automation_commands command
           INNER JOIN automation_devices device
             ON device.tenant_id=command.tenant_id AND device.id=command.device_id
           INNER JOIN packing_sessions session
             ON session.tenant_id=command.tenant_id AND session.facility_id=command.facility_id
           WHERE command.tenant_id=$1 AND command.id=$2 AND session.id=$3
             AND command.device_class='scale'
             AND command.command_payload->'command'->>'operation'='read_stable_weight'
             AND command.packing_session_id=session.id
             AND ($4 OR session.facility_id=ANY($5))
             AND ($6 OR session.inventory_owner_id=ANY($7))"#,
        mapping::COMMAND_COLUMNS
    ))
    .bind(access.tenant_id.get())
    .bind(command_id.get())
    .bind(session_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("packing scale reading"))?;
    let result = mapping::command(&row)?;
    tx.commit().await?;
    Ok(result)
}
