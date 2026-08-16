use wareboxes_application::automation::{
    AutomationCommandReadModel, AutomationCommandStatus, AutomationDeviceReadModel,
    ChangeAutomationControlCommand, EnqueueAutomationCommand, RegisterAutomationDeviceCommand,
    ResolveAutomationCommand, CHANGE_AUTOMATION_CONTROL_OPERATION,
    ENQUEUE_AUTOMATION_COMMAND_OPERATION, REGISTER_AUTOMATION_DEVICE_OPERATION,
    RESOLVE_AUTOMATION_COMMAND_OPERATION,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    validate_automation_device, validate_automation_message, AutomationCommandId,
    AutomationControlMode, AutomationDeviceId, AutomationHealthState, FacilityId, TenantId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};

use super::events::{
    insert_command_history_tx, insert_outbox_tx, AutomationEvent, CommandHistoryEvent,
};
use super::mapping;
use super::{HEALTH_FRESH_SECONDS, SUPERVISOR_PERMISSION};

pub async fn register_device(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &RegisterAutomationDeviceCommand,
) -> AppResult<AutomationDeviceReadModel> {
    context.require_actor(access.tenant_id, access.user_id)?;
    validate_automation_device(&command.device_key, &command.display_name)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let prepared = PreparedCommand::new_v1(context, REGISTER_AUTOMATION_DEVICE_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    bind_actor_tx(&mut tx, context.actor_id.get()).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        SUPERVISOR_PERMISSION,
    )
    .await?;
    if let Some(result) = prepared
        .replayed::<AutomationDeviceReadModel>(&mut tx)
        .await?
    {
        require_device_visible_tx(&mut tx, access.tenant_id, result.device_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }
    require_facility_tx(&mut tx, access.tenant_id, command.facility_id, &scope).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "automation-device:{}:{}",
            access.tenant_id.get(),
            command.device_key
        ))
        .execute(&mut *tx)
        .await?;
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM automation_devices WHERE tenant_id=$1 AND lower(device_key)=lower($2))",
    )
    .bind(access.tenant_id.get())
    .bind(&command.device_key)
    .fetch_one(&mut *tx)
    .await?
    {
        return Err(AppError::conflict("automation device key already exists"));
    }
    let now = now_iso();
    let row = sqlx::query(&format!(
        r#"INSERT INTO automation_devices
        (tenant_id,facility_id,device_key,device_class,display_name,control_mode,
         control_reason,control_changed_by_user_id,control_changed_at,revision,health,
         registered_by_user_id,registered_at)
        VALUES($1,$2,$3,$4,$5,'disabled','registered disabled',$6,$7,1,'unknown',$6,$7)
        RETURNING {}"#,
        mapping::DEVICE_COLUMNS
    ))
    .bind(access.tenant_id.get())
    .bind(command.facility_id.get())
    .bind(&command.device_key)
    .bind(command.class.as_str())
    .bind(&command.display_name)
    .bind(context.actor_id.get())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    let result = mapping::device(&row)?;
    let event_payload =
        serde_json::to_value(&result).map_err(|error| AppError::internal(error.to_string()))?;
    insert_outbox_tx(
        &mut tx,
        AutomationEvent {
            tenant_id: access.tenant_id,
            facility_id: result.facility_id,
            actor_user_id: context.actor_id.get(),
            aggregate_type: "automation_device",
            aggregate_id: result.device_id.get().to_string(),
            event_type: "automation.device.registered",
            event_key: format!("automation-device:{}:registered", result.device_id.get()),
            payload: &event_payload,
            occurred_at: now,
        },
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn change_control(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ChangeAutomationControlCommand,
) -> AppResult<AutomationDeviceReadModel> {
    context.require_actor(access.tenant_id, access.user_id)?;
    validate_automation_message(&command.reason)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    if command.expected_revision == 0 {
        return Err(AppError::bad_request("expected revision must be positive"));
    }
    if command.target_mode == AutomationControlMode::Automatic && !command.safety_confirmed {
        return Err(AppError::bad_request(
            "resuming automation requires the physical safety confirmation",
        ));
    }
    let prepared = PreparedCommand::new_v1(context, CHANGE_AUTOMATION_CONTROL_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    bind_actor_tx(&mut tx, context.actor_id.get()).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        SUPERVISOR_PERMISSION,
    )
    .await?;
    if let Some(result) = prepared
        .replayed::<AutomationDeviceReadModel>(&mut tx)
        .await?
    {
        require_device_visible_tx(&mut tx, access.tenant_id, result.device_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_device_tx(&mut tx, access.tenant_id, command.device_id, &scope).await?;
    if current.revision != command.expected_revision {
        return Err(AppError::conflict(
            "automation device revision does not match expected revision",
        ));
    }
    if current.control_mode == command.target_mode {
        return Err(AppError::conflict(
            "automation device is already in the requested control mode",
        ));
    }
    if command.target_mode == AutomationControlMode::Automatic {
        let health_is_fresh: bool = sqlx::query_scalar(
            r#"SELECT health IN ('healthy','degraded')
              AND last_heartbeat_at >= CURRENT_TIMESTAMP - make_interval(secs=>$3)
              AND NOT EXISTS(SELECT 1 FROM automation_commands command
                WHERE command.tenant_id=automation_devices.tenant_id
                  AND command.device_id=automation_devices.id
                  AND command.status='manual_review')
              FROM automation_devices WHERE tenant_id=$1 AND id=$2"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.device_id.get())
        .bind(HEALTH_FRESH_SECONDS as i32)
        .fetch_one(&mut *tx)
        .await?;
        if !health_is_fresh {
            return Err(AppError::conflict(
                "automation cannot resume without a fresh healthy heartbeat and reconciled manual-review commands",
            ));
        }
    }
    let now = now_iso();
    let row = sqlx::query(&format!(
        r#"UPDATE automation_devices SET control_mode=$3,control_reason=$4,
        control_changed_by_user_id=$5,control_changed_at=$6,revision=revision+1
        WHERE tenant_id=$1 AND id=$2 RETURNING {}"#,
        mapping::DEVICE_COLUMNS
    ))
    .bind(access.tenant_id.get())
    .bind(command.device_id.get())
    .bind(command.target_mode.as_str())
    .bind(&command.reason)
    .bind(context.actor_id.get())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    let result = mapping::device(&row)?;
    let event_payload =
        serde_json::to_value(&result).map_err(|error| AppError::internal(error.to_string()))?;
    insert_outbox_tx(
        &mut tx,
        AutomationEvent {
            tenant_id: access.tenant_id,
            facility_id: result.facility_id,
            actor_user_id: context.actor_id.get(),
            aggregate_type: "automation_device",
            aggregate_id: result.device_id.get().to_string(),
            event_type: "automation.device.control_changed",
            event_key: format!(
                "automation-device:{}:control:{}",
                result.device_id.get(),
                result.revision
            ),
            payload: &event_payload,
            occurred_at: now,
        },
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn enqueue_command(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &EnqueueAutomationCommand,
) -> AppResult<AutomationCommandReadModel> {
    context.require_actor(access.tenant_id, access.user_id)?;
    validate_automation_message(&command.correlation_id)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    command
        .command
        .validate()
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let prepared = PreparedCommand::new_v1(context, ENQUEUE_AUTOMATION_COMMAND_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    bind_actor_tx(&mut tx, context.actor_id.get()).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        SUPERVISOR_PERMISSION,
    )
    .await?;
    if let Some(result) = prepared
        .replayed::<AutomationCommandReadModel>(&mut tx)
        .await?
    {
        require_device_visible_tx(&mut tx, access.tenant_id, result.device_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }
    let device = lock_device_tx(&mut tx, access.tenant_id, command.device_id, &scope).await?;
    if device.class != command.command.device_class() {
        return Err(AppError::bad_request(
            "automation command class does not match the device",
        ));
    }
    let now = now_iso();
    let ready = device.control_mode == AutomationControlMode::Automatic
        && matches!(
            device.health,
            AutomationHealthState::Healthy | AutomationHealthState::Degraded
        )
        && device.last_heartbeat_at.is_some_and(|heartbeat| {
            heartbeat >= now - chrono::Duration::seconds(HEALTH_FRESH_SECONDS)
        });
    if !ready {
        return Err(AppError::conflict(
            "automation device is not healthy, fresh, and in automatic mode",
        ));
    }
    let unresolved_command_exists: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM automation_commands
        WHERE tenant_id=$1 AND device_id=$2 AND status='manual_review')"#,
    )
    .bind(access.tenant_id.get())
    .bind(device.device_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if unresolved_command_exists {
        return Err(AppError::conflict(
            "automation device has an unresolved manual-review command",
        ));
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "automation-correlation:{}:{}",
            access.tenant_id.get(),
            command.correlation_id
        ))
        .execute(&mut *tx)
        .await?;
    let correlation_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM automation_commands WHERE tenant_id=$1 AND correlation_id=$2)",
    )
    .bind(access.tenant_id.get())
    .bind(&command.correlation_id)
    .fetch_one(&mut *tx)
    .await?;
    if correlation_exists {
        return Err(AppError::conflict(
            "automation command correlation ID already exists",
        ));
    }
    let payload = serde_json::to_value(&command.command)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let row = sqlx::query(&format!(
        r#"WITH inserted AS (
          INSERT INTO automation_commands
          (tenant_id,facility_id,device_id,device_class,correlation_id,recovery_policy,
           command_payload,status,revision,delivery_attempts,requested_by_user_id,requested_at)
          VALUES($1,$2,$3,$4,$5,$6,$7,'queued',1,0,$8,$9)
          RETURNING *)
        SELECT {} FROM inserted command JOIN automation_devices device
          ON device.tenant_id=command.tenant_id AND device.id=command.device_id"#,
        mapping::COMMAND_COLUMNS
    ))
    .bind(access.tenant_id.get())
    .bind(device.facility_id.get())
    .bind(device.device_id.get())
    .bind(device.class.as_str())
    .bind(&command.correlation_id)
    .bind(command.recovery_policy.as_str())
    .bind(payload)
    .bind(context.actor_id.get())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    let result = mapping::command(&row)?;
    let event_payload =
        serde_json::to_value(&result).map_err(|error| AppError::internal(error.to_string()))?;
    insert_outbox_tx(
        &mut tx,
        AutomationEvent {
            tenant_id: access.tenant_id,
            facility_id: result.facility_id,
            actor_user_id: context.actor_id.get(),
            aggregate_type: "automation_command",
            aggregate_id: result.command_id.get().to_string(),
            event_type: "automation.command.enqueued",
            event_key: format!("automation-command:{}:enqueued", result.command_id.get()),
            payload: &event_payload,
            occurred_at: now,
        },
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn resolve_command(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ResolveAutomationCommand,
) -> AppResult<AutomationCommandReadModel> {
    context.require_actor(access.tenant_id, access.user_id)?;
    validate_automation_message(&command.reason)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    if command.expected_revision == 0 {
        return Err(AppError::bad_request("expected revision must be positive"));
    }
    let prepared = PreparedCommand::new_v1(context, RESOLVE_AUTOMATION_COMMAND_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    bind_actor_tx(&mut tx, context.actor_id.get()).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        SUPERVISOR_PERMISSION,
    )
    .await?;
    if let Some(result) = prepared
        .replayed::<AutomationCommandReadModel>(&mut tx)
        .await?
    {
        require_device_visible_tx(&mut tx, access.tenant_id, result.device_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_command_tx(&mut tx, access.tenant_id, command.command_id).await?;
    require_device_visible_tx(&mut tx, access.tenant_id, current.device_id, &scope).await?;
    if current.status != AutomationCommandStatus::ManualReview
        || current.revision != command.expected_revision
    {
        return Err(AppError::conflict(
            "automation command is not at the expected manual-review revision",
        ));
    }
    let now = now_iso();
    sqlx::query(
        r#"UPDATE automation_commands SET status='resolved_manually',revision=revision+1,
        resolved_by_user_id=$3,resolution_outcome=$4,resolution_reason=$5,resolved_at=$6
        WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.command_id.get())
    .bind(context.actor_id.get())
    .bind(command.outcome.as_str())
    .bind(&command.reason)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    insert_command_history_tx(
        &mut tx,
        CommandHistoryEvent {
            tenant_id: access.tenant_id,
            command_id: command.command_id,
            transition: "resolved_manually",
            actor_user_id: context.actor_id.get(),
            service_account_id: None,
            occurred_at: now,
            evidence: serde_json::json!({
                "outcome": command.outcome,
                "reason": command.reason,
            }),
        },
    )
    .await?;
    let result =
        mapping::command(&command_row_tx(&mut tx, access.tenant_id, command.command_id).await?)?;
    let event_payload =
        serde_json::to_value(&result).map_err(|error| AppError::internal(error.to_string()))?;
    insert_outbox_tx(
        &mut tx,
        AutomationEvent {
            tenant_id: access.tenant_id,
            facility_id: result.facility_id,
            actor_user_id: context.actor_id.get(),
            aggregate_type: "automation_command",
            aggregate_id: result.command_id.get().to_string(),
            event_type: "automation.command.resolved_manually",
            event_key: format!(
                "automation-command:{}:{}:resolved-manually",
                result.command_id.get(),
                result.revision
            ),
            payload: &event_payload,
            occurred_at: now,
        },
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub(super) async fn bind_actor_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_user_id: i64,
) -> AppResult<()> {
    sqlx::query("SELECT set_config('wareboxes.actor_user_id',$1,true)")
        .bind(actor_user_id.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(super) async fn require_facility_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: FacilityId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    if !scope.includes_facility(facility_id.get()) {
        return Err(AppError::not_found("automation facility"));
    }
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM facilities WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL)",
    )
    .bind(tenant_id.get())
    .bind(facility_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(AppError::not_found("automation facility"))
    }
}

pub(super) async fn lock_device_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    device_id: AutomationDeviceId,
    scope: &ScopeBindings,
) -> AppResult<AutomationDeviceReadModel> {
    let row = sqlx::query(&format!(
        "SELECT {} FROM automation_devices WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
        mapping::DEVICE_COLUMNS
    ))
    .bind(tenant_id.get())
    .bind(device_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("automation device"))?;
    let device = mapping::device(&row)?;
    if !scope.includes_facility(device.facility_id.get()) {
        return Err(AppError::not_found("automation device"));
    }
    Ok(device)
}

pub(super) async fn require_device_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    device_id: AutomationDeviceId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    lock_device_tx(tx, tenant_id, device_id, scope)
        .await
        .map(|_| ())
}

pub(super) async fn lock_command_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command_id: AutomationCommandId,
) -> AppResult<AutomationCommandReadModel> {
    let row = sqlx::query(&format!(
        r#"SELECT {} FROM automation_commands command
        JOIN automation_devices device ON device.tenant_id=command.tenant_id
          AND device.id=command.device_id
        WHERE command.tenant_id=$1 AND command.id=$2 FOR UPDATE OF command"#,
        mapping::COMMAND_COLUMNS
    ))
    .bind(tenant_id.get())
    .bind(command_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("automation command"))?;
    mapping::command(&row)
}

pub(super) async fn command_row_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command_id: AutomationCommandId,
) -> AppResult<sqlx::postgres::PgRow> {
    sqlx::query(&format!(
        r#"SELECT {} FROM automation_commands command
        JOIN automation_devices device ON device.tenant_id=command.tenant_id
          AND device.id=command.device_id
        WHERE command.tenant_id=$1 AND command.id=$2 FOR UPDATE OF command"#,
        mapping::COMMAND_COLUMNS
    ))
    .bind(tenant_id.get())
    .bind(command_id.get())
    .fetch_one(&mut **tx)
    .await
    .map_err(AppError::from)
}
