use rand::distributions::Alphanumeric;
use rand::Rng;
use wareboxes_application::automation::{
    AcknowledgeAutomationCommand, AutomationCommandReadModel, AutomationCommandStatus,
    AutomationDeliveryReadModel, AutomationHeartbeatReadModel, PullAutomationCommands,
    RecordAutomationHeartbeat, ReportAutomationCommand, ACK_AUTOMATION_COMMAND_OPERATION,
    RECORD_AUTOMATION_HEARTBEAT_OPERATION, REPORT_AUTOMATION_COMMAND_OPERATION,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{validate_automation_message, AutomationCommandId, ServiceAccountId};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

use super::commands::{
    bind_actor_tx, command_row_tx, lock_command_tx, lock_device_tx, require_device_visible_tx,
};
use super::events::{
    insert_command_history_tx, insert_outbox_tx, AutomationEvent, CommandHistoryEvent,
};
use super::mapping;
use super::{DELIVERY_LEASE_SECONDS, EDGE_PERMISSION, HEALTH_FRESH_SECONDS};

pub async fn assigned_devices(
    db: &Db,
    access: &TenantAccess,
    facility_id: wareboxes_domain::FacilityId,
) -> AppResult<Vec<wareboxes_application::automation::AutomationDeviceReadModel>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        EDGE_PERMISSION,
    )
    .await?;
    if !scope.includes_facility(facility_id.get()) {
        return Err(AppError::not_found("automation facility"));
    }
    let rows = sqlx::query(&format!(
        r#"SELECT {} FROM automation_devices
        WHERE tenant_id=$1 AND facility_id=$2
        ORDER BY lower(display_name),id"#,
        mapping::DEVICE_COLUMNS
    ))
    .bind(access.tenant_id.get())
    .bind(facility_id.get())
    .fetch_all(&mut *tx)
    .await?;
    let result = rows
        .iter()
        .map(mapping::device)
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(result)
}

pub async fn pull_commands(
    db: &Db,
    access: &TenantAccess,
    service_account_id: ServiceAccountId,
    context: &CommandContext,
    command: &PullAutomationCommands,
) -> AppResult<Vec<AutomationDeliveryReadModel>> {
    context.require_actor(access.tenant_id, access.user_id)?;
    validate_automation_message(&command.agent_instance)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    if !(1..=100).contains(&command.limit) {
        return Err(AppError::bad_request(
            "automation command pull limit must be between 1 and 100",
        ));
    }
    let prepared = PreparedCommand::new_v1(context, "automation.command.pull.v1", command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    bind_actor_tx(&mut tx, context.actor_id.get()).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        EDGE_PERMISSION,
    )
    .await?;
    if !scope.includes_facility(command.facility_id.get()) {
        return Err(AppError::not_found("automation facility"));
    }
    if let Some(result) = prepared
        .replayed::<Vec<AutomationDeliveryReadModel>>(&mut tx)
        .await?
    {
        for delivery in &result {
            require_device_visible_tx(
                &mut tx,
                access.tenant_id,
                delivery.command.device_id,
                &scope,
            )
            .await?;
            if delivery.command.assigned_service_account_id != Some(service_account_id) {
                return Err(AppError::not_found("automation command delivery"));
            }
        }
        tx.commit().await?;
        return Ok(result);
    }
    let now = now_iso();
    let ids = sqlx::query_scalar::<_, i64>(
        r#"SELECT command.id FROM automation_commands command
        JOIN automation_devices device ON device.tenant_id=command.tenant_id
          AND device.id=command.device_id
        WHERE command.tenant_id=$1 AND command.facility_id=$2
          AND device.control_mode='automatic'
          AND device.health IN ('healthy','degraded')
          AND device.last_heartbeat_at >= $3 - make_interval(secs=>$6)
          AND EXISTS(SELECT 1 FROM automation_heartbeats heartbeat
            WHERE heartbeat.tenant_id=command.tenant_id
              AND heartbeat.device_id=command.device_id
              AND heartbeat.service_account_id=$4
              AND heartbeat.agent_instance=$7
              AND heartbeat.control_mode='automatic'
              AND heartbeat.id=(SELECT max(latest.id) FROM automation_heartbeats latest
                WHERE latest.tenant_id=heartbeat.tenant_id
                  AND latest.device_id=heartbeat.device_id))
          AND NOT EXISTS(SELECT 1 FROM automation_commands held
            WHERE held.tenant_id=command.tenant_id AND held.device_id=command.device_id
              AND held.status='manual_review')
          AND ((command.status='queued' AND command.assigned_service_account_id IS NULL)
            OR (command.status='delivered'
              AND command.assigned_service_account_id=$4
              AND command.delivery_expires_at <= $3))
        ORDER BY command.requested_at,command.id
        LIMIT $5 FOR UPDATE OF command,device SKIP LOCKED"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.facility_id.get())
    .bind(now)
    .bind(service_account_id.get())
    .bind(i64::from(command.limit))
    .bind(HEALTH_FRESH_SECONDS as i32)
    .bind(&command.agent_instance)
    .fetch_all(&mut *tx)
    .await?;
    let mut result = Vec::with_capacity(ids.len());
    for command_id in ids {
        let typed_command_id = AutomationCommandId::new(command_id)
            .map_err(|error| AppError::internal(error.to_string()))?;
        let delivery_token = random_delivery_token();
        let delivery_expires_at = now + chrono::Duration::seconds(DELIVERY_LEASE_SECONDS);
        sqlx::query(
            r#"UPDATE automation_commands SET status='delivered',revision=revision+1,
            delivery_attempts=delivery_attempts+1,assigned_service_account_id=$3,
            agent_instance=$4,delivery_token=$5,delivered_at=$6,delivery_expires_at=$7
            WHERE tenant_id=$1 AND id=$2"#,
        )
        .bind(access.tenant_id.get())
        .bind(command_id)
        .bind(service_account_id.get())
        .bind(&command.agent_instance)
        .bind(&delivery_token)
        .bind(now)
        .bind(delivery_expires_at)
        .execute(&mut *tx)
        .await?;
        insert_command_history_tx(
            &mut tx,
            CommandHistoryEvent {
                tenant_id: access.tenant_id,
                command_id: typed_command_id,
                transition: "delivered",
                actor_user_id: context.actor_id.get(),
                service_account_id: Some(service_account_id.get()),
                occurred_at: now,
                evidence: serde_json::json!({
                    "agent_instance": command.agent_instance,
                    "delivery_expires_at": delivery_expires_at,
                }),
            },
        )
        .await?;
        let row = command_row_tx(&mut tx, access.tenant_id, typed_command_id).await?;
        result.push(AutomationDeliveryReadModel {
            command: mapping::command(&row)?,
            delivery_token,
            delivery_expires_at,
        });
    }
    Ok(prepared.commit(tx, result).await?)
}

pub async fn acknowledge_command(
    db: &Db,
    access: &TenantAccess,
    service_account_id: ServiceAccountId,
    context: &CommandContext,
    command: &AcknowledgeAutomationCommand,
) -> AppResult<AutomationCommandReadModel> {
    context.require_actor(access.tenant_id, access.user_id)?;
    validate_automation_message(&command.delivery_token)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    if command.expected_revision == 0 {
        return Err(AppError::bad_request("expected revision must be positive"));
    }
    let prepared = PreparedCommand::new_v1(context, ACK_AUTOMATION_COMMAND_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    bind_actor_tx(&mut tx, context.actor_id.get()).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        EDGE_PERMISSION,
    )
    .await?;
    if let Some(result) = prepared
        .replayed::<AutomationCommandReadModel>(&mut tx)
        .await?
    {
        require_assigned_visible_tx(
            &mut tx,
            access.tenant_id.get(),
            &result,
            service_account_id,
            &scope,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_command_tx(&mut tx, access.tenant_id, command.command_id).await?;
    require_assigned_visible_tx(
        &mut tx,
        access.tenant_id.get(),
        &current,
        service_account_id,
        &scope,
    )
    .await?;
    lock_device_tx(&mut tx, access.tenant_id, current.device_id, &scope).await?;
    if device_has_manual_review_tx(&mut tx, access.tenant_id.get(), current.device_id.get()).await?
    {
        return Err(AppError::conflict(
            "automation device has an unresolved manual-review command",
        ));
    }
    if current.status != AutomationCommandStatus::Delivered
        || current.revision != command.expected_revision
    {
        return Err(AppError::conflict(
            "automation delivery status or revision changed",
        ));
    }
    let token_matches: bool = sqlx::query_scalar(
        "SELECT delivery_token=$3 FROM automation_commands WHERE tenant_id=$1 AND id=$2",
    )
    .bind(access.tenant_id.get())
    .bind(command.command_id.get())
    .bind(&command.delivery_token)
    .fetch_one(&mut *tx)
    .await?;
    if !token_matches {
        return Err(AppError::conflict("automation delivery token changed"));
    }
    let now = now_iso();
    sqlx::query(
        r#"UPDATE automation_commands SET status='accepted',revision=revision+1,
        accepted_at=$3,delivery_expires_at=NULL
        WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.command_id.get())
    .bind(now)
    .execute(&mut *tx)
    .await?;
    insert_command_history_tx(
        &mut tx,
        CommandHistoryEvent {
            tenant_id: access.tenant_id,
            command_id: command.command_id,
            transition: "accepted",
            actor_user_id: context.actor_id.get(),
            service_account_id: Some(service_account_id.get()),
            occurred_at: now,
            evidence: serde_json::json!({"durably_persisted": true}),
        },
    )
    .await?;
    let result =
        mapping::command(&command_row_tx(&mut tx, access.tenant_id, command.command_id).await?)?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn report_command(
    db: &Db,
    access: &TenantAccess,
    service_account_id: ServiceAccountId,
    context: &CommandContext,
    command: &ReportAutomationCommand,
) -> AppResult<AutomationCommandReadModel> {
    context.require_actor(access.tenant_id, access.user_id)?;
    validate_report_shape(command)?;
    let prepared = PreparedCommand::new_v1(context, REPORT_AUTOMATION_COMMAND_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    bind_actor_tx(&mut tx, context.actor_id.get()).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        EDGE_PERMISSION,
    )
    .await?;
    if let Some(result) = prepared
        .replayed::<AutomationCommandReadModel>(&mut tx)
        .await?
    {
        require_assigned_visible_tx(
            &mut tx,
            access.tenant_id.get(),
            &result,
            service_account_id,
            &scope,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_command_tx(&mut tx, access.tenant_id, command.command_id).await?;
    require_assigned_visible_tx(
        &mut tx,
        access.tenant_id.get(),
        &current,
        service_account_id,
        &scope,
    )
    .await?;
    lock_device_tx(&mut tx, access.tenant_id, current.device_id, &scope).await?;
    if current.status != AutomationCommandStatus::Accepted
        || current.revision != command.expected_revision
    {
        return Err(AppError::conflict(
            "automation command status or revision changed",
        ));
    }
    if let Some(result) = &command.result {
        result
            .validate_for(&current.command)
            .map_err(|error| AppError::bad_request(error.to_string()))?;
    }
    let now = now_iso();
    if command.occurred_at < current.accepted_at.unwrap_or(current.requested_at)
        || command.occurred_at > now + chrono::Duration::minutes(1)
    {
        return Err(AppError::bad_request(
            "automation result time is outside the accepted command window",
        ));
    }
    let result_payload = command
        .result
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;
    sqlx::query(
        r#"UPDATE automation_commands SET status=$3,revision=revision+1,
        result_payload=$4,error_code=$5,error_message=$6,completed_at=$7
        WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.command_id.get())
    .bind(command.status.as_str())
    .bind(result_payload)
    .bind(&command.error_code)
    .bind(&command.error_message)
    .bind(command.occurred_at)
    .execute(&mut *tx)
    .await?;
    insert_command_history_tx(
        &mut tx,
        CommandHistoryEvent {
            tenant_id: access.tenant_id,
            command_id: command.command_id,
            transition: command.status.as_str(),
            actor_user_id: context.actor_id.get(),
            service_account_id: Some(service_account_id.get()),
            occurred_at: command.occurred_at,
            evidence: serde_json::json!({
                "result": command.result,
                "error_code": command.error_code,
                "error_message": command.error_message,
            }),
        },
    )
    .await?;
    let result =
        mapping::command(&command_row_tx(&mut tx, access.tenant_id, command.command_id).await?)?;
    let event_payload =
        serde_json::to_value(&result).map_err(|error| AppError::internal(error.to_string()))?;
    let event_type = format!("automation.command.{}", command.status.as_str());
    insert_outbox_tx(
        &mut tx,
        AutomationEvent {
            tenant_id: access.tenant_id,
            facility_id: result.facility_id,
            actor_user_id: context.actor_id.get(),
            aggregate_type: "automation_command",
            aggregate_id: result.command_id.get().to_string(),
            event_type: &event_type,
            event_key: format!(
                "automation-command:{}:{}:{}",
                result.command_id.get(),
                result.revision,
                command.status.as_str()
            ),
            payload: &event_payload,
            occurred_at: command.occurred_at,
        },
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn record_heartbeat(
    db: &Db,
    access: &TenantAccess,
    service_account_id: ServiceAccountId,
    context: &CommandContext,
    command: &RecordAutomationHeartbeat,
) -> AppResult<AutomationHeartbeatReadModel> {
    context.require_actor(access.tenant_id, access.user_id)?;
    validate_automation_message(&command.agent_instance)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    if let Some(message) = &command.message {
        validate_automation_message(message)
            .map_err(|error| AppError::bad_request(error.to_string()))?;
    }
    let prepared =
        PreparedCommand::new_v1(context, RECORD_AUTOMATION_HEARTBEAT_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    bind_actor_tx(&mut tx, context.actor_id.get()).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        EDGE_PERMISSION,
    )
    .await?;
    if let Some(result) = prepared
        .replayed::<AutomationHeartbeatReadModel>(&mut tx)
        .await?
    {
        require_device_visible_tx(&mut tx, access.tenant_id, result.device_id, &scope).await?;
        if result.service_account_id != service_account_id {
            return Err(AppError::not_found("automation heartbeat"));
        }
        tx.commit().await?;
        return Ok(result);
    }
    let device = lock_device_tx(&mut tx, access.tenant_id, command.device_id, &scope).await?;
    let received_at = now_iso();
    if command.observed_at < received_at - chrono::Duration::minutes(5)
        || command.observed_at > received_at + chrono::Duration::minutes(1)
    {
        return Err(AppError::bad_request(
            "automation heartbeat time is outside the accepted clock-skew window",
        ));
    }
    if device
        .last_heartbeat_at
        .is_some_and(|latest| command.observed_at <= latest)
    {
        return Err(AppError::conflict(
            "automation heartbeat is not newer than the device projection",
        ));
    }
    let row = sqlx::query(
        r#"INSERT INTO automation_heartbeats
        (tenant_id,facility_id,device_id,service_account_id,agent_instance,health,
         control_mode,message,queued_commands,manual_review_commands,observed_at,received_at)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,CURRENT_TIMESTAMP)
        RETURNING id,device_id,service_account_id,agent_instance,health,control_mode,
          message,queued_commands,manual_review_commands,observed_at,received_at"#,
    )
    .bind(access.tenant_id.get())
    .bind(device.facility_id.get())
    .bind(device.device_id.get())
    .bind(service_account_id.get())
    .bind(&command.agent_instance)
    .bind(command.health.as_str())
    .bind(command.control_mode.as_str())
    .bind(&command.message)
    .bind(
        i32::try_from(command.queued_commands)
            .map_err(|_| AppError::bad_request("queued automation command count is too large"))?,
    )
    .bind(i32::try_from(command.manual_review_commands).map_err(|_| {
        AppError::bad_request("manual-review automation command count is too large")
    })?)
    .bind(command.observed_at)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        r#"UPDATE automation_devices SET health=$3,health_message=$4,last_heartbeat_at=$5
        WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(device.device_id.get())
    .bind(command.health.as_str())
    .bind(&command.message)
    .bind(command.observed_at)
    .execute(&mut *tx)
    .await?;
    let result = mapping::heartbeat(&row)?;
    Ok(prepared.commit(tx, result).await?)
}

fn validate_report_shape(command: &ReportAutomationCommand) -> AppResult<()> {
    if command.expected_revision == 0 {
        return Err(AppError::bad_request("expected revision must be positive"));
    }
    match command.status {
        AutomationCommandStatus::Succeeded
            if command.result.is_some()
                && command.error_code.is_none()
                && command.error_message.is_none() => {}
        AutomationCommandStatus::Failed | AutomationCommandStatus::ManualReview
            if command.result.is_none()
                && command
                    .error_message
                    .as_deref()
                    .is_some_and(|value| !value.is_empty()) =>
        {
            if let Some(code) = &command.error_code {
                validate_automation_message(code)
                    .map_err(|error| AppError::bad_request(error.to_string()))?;
            }
            if let Some(message) = &command.error_message {
                validate_automation_message(message)
                    .map_err(|error| AppError::bad_request(error.to_string()))?;
            }
        }
        _ => {
            return Err(AppError::bad_request(
                "automation report must be succeeded with a result, or failed/manual review with an error",
            ));
        }
    }
    Ok(())
}

async fn require_assigned_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    command: &AutomationCommandReadModel,
    service_account_id: ServiceAccountId,
    scope: &crate::repo::access::ScopeBindings,
) -> AppResult<()> {
    require_device_visible_tx(tx, command.tenant_id, command.device_id, scope).await?;
    if command.tenant_id.get() != tenant_id
        || command.assigned_service_account_id != Some(service_account_id)
    {
        return Err(AppError::not_found("automation command"));
    }
    Ok(())
}

fn random_delivery_token() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(48)
        .map(char::from)
        .collect()
}

async fn device_has_manual_review_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    device_id: i64,
) -> AppResult<bool> {
    sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM automation_commands
        WHERE tenant_id=$1 AND device_id=$2 AND status='manual_review')"#,
    )
    .bind(tenant_id)
    .bind(device_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(AppError::from)
}
