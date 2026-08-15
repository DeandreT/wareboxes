use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::yard::{
    AssignYardVisitDoorCommand, GateInYardVisitCommand, MoveYardVisitCommand,
    YardDockOperationCommand, YardVisitEventKind, YardVisitLifecycleCommand, YardVisitReadModel,
    ASSIGN_YARD_VISIT_DOOR_OPERATION, COMPLETE_YARD_OPERATION, GATE_IN_YARD_VISIT_OPERATION,
    GATE_OUT_YARD_VISIT_OPERATION, REJECT_YARD_VISIT_OPERATION, SPOT_YARD_VISIT_OPERATION,
    START_YARD_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    calculate_yard_detention, BillableEventId, YardAppointmentStatus, YardFreeMinutes,
    YardLocationKind, YardOperation, YardRevision, YardVisitId, YardVisitStatus,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use super::{
    conflict, insert_appointment_event_tx, insert_visit_event_tx, lock_key_tx, lock_visit_tx,
    next_revision, validate_location_tx, validate_non_door_destination, validate_owner_facility_tx,
    verify_visit_replay, NewAppointmentEvent, NewVisitEvent,
};
use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};
use crate::repo::yard::models::{read_appointment_tx, read_asset_tx, read_visit_tx};
use crate::repo::yard::{
    direction_name, enqueue_event_tx, internal, require_access_actor, require_scope,
    YardOutboxEvent, PERMISSION,
};

pub async fn gate_in(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &GateInYardVisitCommand,
) -> AppResult<YardVisitReadModel> {
    require_access_actor(access, context)?;
    let prepared = PreparedCommand::new_v1(context, GATE_IN_YARD_VISIT_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    require_scope(&scope, command.inventory_owner_id, command.facility_id)?;
    if let Some(result) = prepared.replayed::<YardVisitReadModel>(&mut tx).await? {
        verify_visit_replay(&scope, &result)?;
        tx.commit().await?;
        return Ok(result);
    }
    validate_owner_facility_tx(
        &mut tx,
        access.tenant_id,
        command.inventory_owner_id,
        command.facility_id,
    )
    .await?;
    let asset = read_asset_tx(&mut tx, access.tenant_id, command.asset_id).await?;
    if !asset.active {
        return Err(AppError::conflict("yard asset is inactive"));
    }
    validate_location_tx(
        &mut tx,
        access.tenant_id,
        command.facility_id,
        command.gate_location_id,
        Some(YardLocationKind::Gate),
    )
    .await?;
    lock_key_tx(
        &mut tx,
        format!(
            "yard-active-asset:{}:{}",
            access.tenant_id.get(),
            command.asset_id.get()
        ),
    )
    .await?;
    let active_asset = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM yard_visits WHERE tenant_id=$1 AND asset_id=$2 AND status<>'gated_out')",
    )
    .bind(access.tenant_id.get())
    .bind(command.asset_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if active_asset {
        return Err(AppError::conflict("yard asset already has an active visit"));
    }
    let appointment = if let Some(appointment_id) = command.appointment_id {
        lock_key_tx(
            &mut tx,
            format!(
                "yard-appointment:{}:{}",
                access.tenant_id.get(),
                appointment_id.get()
            ),
        )
        .await?;
        sqlx::query("SELECT id FROM yard_appointments WHERE tenant_id=$1 AND id=$2 FOR UPDATE")
            .bind(access.tenant_id.get())
            .bind(appointment_id.get())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| AppError::not_found("yard appointment"))?;
        let appointment = read_appointment_tx(&mut tx, access.tenant_id, appointment_id).await?;
        require_scope(
            &scope,
            appointment.inventory_owner_id,
            appointment.facility_id,
        )?;
        if appointment.status != YardAppointmentStatus::Scheduled
            || appointment.inventory_owner_id != command.inventory_owner_id
            || appointment.facility_id != command.facility_id
            || appointment.direction != command.direction
            || appointment.expected_asset_kind != asset.kind
            || appointment
                .expected_asset_number
                .as_deref()
                .is_some_and(|number| number != asset.asset_number)
            || appointment.carrier != asset.carrier
        {
            return Err(AppError::conflict(
                "yard visit does not match the scheduled appointment",
            ));
        }
        Some(appointment)
    } else {
        None
    };
    let now = now_iso();
    let visit_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO yard_visits
           (tenant_id,inventory_owner_id,facility_id,appointment_id,direction,asset_id,
            driver_name,current_location_id,inbound_load_id,outbound_load_id,
            gated_in_by_user_id,gated_in_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.inventory_owner_id.get())
    .bind(command.facility_id.get())
    .bind(command.appointment_id.map(|id| id.get()))
    .bind(direction_name(command.direction))
    .bind(command.asset_id.get())
    .bind(command.driver_name.as_str())
    .bind(command.gate_location_id.get())
    .bind(
        appointment
            .as_ref()
            .and_then(|value| value.inbound_load_id)
            .map(|id| id.get()),
    )
    .bind(
        appointment
            .as_ref()
            .and_then(|value| value.outbound_load_id)
            .map(|id| id.get()),
    )
    .bind(context.actor_id.get())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    let visit_id = YardVisitId::new(visit_id).map_err(internal)?;
    let initial_revision = YardRevision::new(1).map_err(internal)?;
    insert_visit_event_tx(
        &mut tx,
        NewVisitEvent {
            tenant_id: access.tenant_id,
            owner_id: command.inventory_owner_id,
            facility_id: command.facility_id,
            visit_id,
            kind: YardVisitEventKind::GatedIn,
            from_status: None,
            to_status: YardVisitStatus::GatedIn,
            from_location_id: None,
            to_location_id: Some(command.gate_location_id),
            operation: None,
            note: command.note.as_ref().map(|value| value.as_str()),
            revision: initial_revision,
            actor_id: context.actor_id,
            occurred_at: now,
        },
    )
    .await?;
    if let Some(appointment) = &appointment {
        let revision = next_revision(appointment.revision)?;
        appointment
            .status
            .transition(YardAppointmentStatus::CheckedIn)
            .map_err(conflict)?;
        sqlx::query(
            r#"UPDATE yard_appointments SET status='checked_in',revision=$3,visit_id=$4,
               updated_by_user_id=$5,updated_at=$6 WHERE tenant_id=$1 AND id=$2"#,
        )
        .bind(access.tenant_id.get())
        .bind(appointment.appointment_id.get())
        .bind(revision.get())
        .bind(visit_id.get())
        .bind(context.actor_id.get())
        .bind(now)
        .execute(&mut *tx)
        .await?;
        insert_appointment_event_tx(
            &mut tx,
            NewAppointmentEvent {
                tenant_id: access.tenant_id,
                owner_id: appointment.inventory_owner_id,
                facility_id: appointment.facility_id,
                appointment_id: appointment.appointment_id,
                kind: "checked_in",
                from_status: Some(appointment.status),
                to_status: YardAppointmentStatus::CheckedIn,
                note: command.note.as_ref().map(|value| value.as_str()),
                revision,
                actor_id: context.actor_id,
                occurred_at: now,
            },
        )
        .await?;
        let updated =
            read_appointment_tx(&mut tx, access.tenant_id, appointment.appointment_id).await?;
        enqueue_event_tx(
            &mut tx,
            YardOutboxEvent {
                tenant_id: access.tenant_id,
                actor_id: context.actor_id,
                owner_id: Some(updated.inventory_owner_id),
                facility_id: Some(updated.facility_id),
                aggregate_type: "appointment",
                aggregate_id: updated.appointment_id.get(),
                transition: "checked_in",
                occurred_at: now,
            },
            &updated,
        )
        .await?;
    }
    let result = read_visit_tx(&mut tx, access.tenant_id, visit_id).await?;
    enqueue_event_tx(
        &mut tx,
        YardOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            owner_id: Some(result.inventory_owner_id),
            facility_id: Some(result.facility_id),
            aggregate_type: "visit",
            aggregate_id: visit_id.get(),
            transition: "gated_in",
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn spot_visit(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &MoveYardVisitCommand,
) -> AppResult<YardVisitReadModel> {
    require_access_actor(access, context)?;
    let prepared = PreparedCommand::new_v1(context, SPOT_YARD_VISIT_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    if let Some(result) = prepared.replayed::<YardVisitReadModel>(&mut tx).await? {
        verify_visit_replay(&scope, &result)?;
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_visit_tx(&mut tx, access.tenant_id, command.visit_id).await?;
    verify_visit_replay(&scope, &current)?;
    if current.revision != command.expected_revision {
        return Err(AppError::conflict("yard visit revision does not match"));
    }
    let target = current.status.spot().map_err(conflict)?;
    let destination = validate_location_tx(
        &mut tx,
        access.tenant_id,
        current.facility_id,
        command.destination_location_id,
        None,
    )
    .await?;
    validate_non_door_destination(destination.kind)?;
    let occupied = sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS(SELECT 1 FROM yard_visits WHERE tenant_id=$1 AND facility_id=$2
           AND current_location_id=$3 AND id<>$4
           AND status IN ('in_yard','at_door','loading','unloading','ready_to_depart'))"#,
    )
    .bind(access.tenant_id.get())
    .bind(current.facility_id.get())
    .bind(command.destination_location_id.get())
    .bind(command.visit_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if occupied {
        return Err(AppError::conflict("yard destination is occupied"));
    }
    let now = now_iso();
    let revision = next_revision(current.revision)?;
    sqlx::query(
        r#"UPDATE yard_visits SET status=$3,revision=$4,current_location_id=$5,
           dock_door_location_id=NULL WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.visit_id.get())
    .bind(target.as_str())
    .bind(revision.get())
    .bind(command.destination_location_id.get())
    .execute(&mut *tx)
    .await?;
    insert_visit_event_tx(
        &mut tx,
        NewVisitEvent {
            tenant_id: access.tenant_id,
            owner_id: current.inventory_owner_id,
            facility_id: current.facility_id,
            visit_id: command.visit_id,
            kind: YardVisitEventKind::Spotted,
            from_status: Some(current.status),
            to_status: target,
            from_location_id: current.current_location_id,
            to_location_id: Some(command.destination_location_id),
            operation: None,
            note: Some(command.note.as_str()),
            revision,
            actor_id: context.actor_id,
            occurred_at: now,
        },
    )
    .await?;
    finish_visit_command(
        tx,
        access,
        context,
        prepared,
        command.visit_id,
        "spotted",
        now,
    )
    .await
}

pub async fn assign_door(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &AssignYardVisitDoorCommand,
) -> AppResult<YardVisitReadModel> {
    require_access_actor(access, context)?;
    let prepared = PreparedCommand::new_v1(context, ASSIGN_YARD_VISIT_DOOR_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    if let Some(result) = prepared.replayed::<YardVisitReadModel>(&mut tx).await? {
        verify_visit_replay(&scope, &result)?;
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_visit_tx(&mut tx, access.tenant_id, command.visit_id).await?;
    verify_visit_replay(&scope, &current)?;
    if current.revision != command.expected_revision {
        return Err(AppError::conflict("yard visit revision does not match"));
    }
    let target = current.status.assign_door().map_err(conflict)?;
    validate_location_tx(
        &mut tx,
        access.tenant_id,
        current.facility_id,
        command.door_location_id,
        Some(YardLocationKind::DockDoor),
    )
    .await?;
    let occupied = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM yard_visits WHERE tenant_id=$1 AND facility_id=$2 AND dock_door_location_id=$3 AND id<>$4 AND status<>'gated_out')",
    )
    .bind(access.tenant_id.get())
    .bind(current.facility_id.get())
    .bind(command.door_location_id.get())
    .bind(command.visit_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if occupied {
        return Err(AppError::conflict("yard dock door is occupied"));
    }
    let now = now_iso();
    let revision = next_revision(current.revision)?;
    sqlx::query(
        r#"UPDATE yard_visits SET status=$3,revision=$4,current_location_id=$5,
           dock_door_location_id=$5 WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.visit_id.get())
    .bind(target.as_str())
    .bind(revision.get())
    .bind(command.door_location_id.get())
    .execute(&mut *tx)
    .await?;
    insert_visit_event_tx(
        &mut tx,
        NewVisitEvent {
            tenant_id: access.tenant_id,
            owner_id: current.inventory_owner_id,
            facility_id: current.facility_id,
            visit_id: command.visit_id,
            kind: YardVisitEventKind::DoorAssigned,
            from_status: Some(current.status),
            to_status: target,
            from_location_id: current.current_location_id,
            to_location_id: Some(command.door_location_id),
            operation: None,
            note: Some(command.note.as_str()),
            revision,
            actor_id: context.actor_id,
            occurred_at: now,
        },
    )
    .await?;
    finish_visit_command(
        tx,
        access,
        context,
        prepared,
        command.visit_id,
        "door_assigned",
        now,
    )
    .await
}

#[derive(Debug, Clone, Copy)]
enum DockTransition {
    Start,
    Complete,
}

async fn transition_operation(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &YardDockOperationCommand,
    transition: DockTransition,
) -> AppResult<YardVisitReadModel> {
    require_access_actor(access, context)?;
    let (operation_name, event_kind, outbox_transition) = match transition {
        DockTransition::Start => (
            START_YARD_OPERATION,
            YardVisitEventKind::OperationStarted,
            "operation_started",
        ),
        DockTransition::Complete => (
            COMPLETE_YARD_OPERATION,
            YardVisitEventKind::OperationCompleted,
            "operation_completed",
        ),
    };
    let prepared = PreparedCommand::new_v1(context, operation_name, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    if let Some(result) = prepared.replayed::<YardVisitReadModel>(&mut tx).await? {
        verify_visit_replay(&scope, &result)?;
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_visit_tx(&mut tx, access.tenant_id, command.visit_id).await?;
    verify_visit_replay(&scope, &current)?;
    if current.revision != command.expected_revision {
        return Err(AppError::conflict("yard visit revision does not match"));
    }
    let target = match transition {
        DockTransition::Start => current
            .status
            .begin_operation(current.direction, command.operation)
            .map_err(conflict)?,
        DockTransition::Complete => {
            let expected_active = match command.operation {
                YardOperation::Loading => YardVisitStatus::Loading,
                YardOperation::Unloading => YardVisitStatus::Unloading,
            };
            if current.status != expected_active {
                return Err(AppError::conflict(
                    "yard operation does not match active visit work",
                ));
            }
            current.status.complete_operation().map_err(conflict)?
        }
    };
    let now = now_iso();
    let revision = next_revision(current.revision)?;
    match transition {
        DockTransition::Start => {
            sqlx::query(
                "UPDATE yard_visits SET status=$3,revision=$4,operation_started_at=$5 WHERE tenant_id=$1 AND id=$2",
            )
            .bind(access.tenant_id.get())
            .bind(command.visit_id.get())
            .bind(target.as_str())
            .bind(revision.get())
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        DockTransition::Complete => {
            sqlx::query(
                "UPDATE yard_visits SET status=$3,revision=$4,operation_completed_at=$5 WHERE tenant_id=$1 AND id=$2",
            )
            .bind(access.tenant_id.get())
            .bind(command.visit_id.get())
            .bind(target.as_str())
            .bind(revision.get())
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
    }
    insert_visit_event_tx(
        &mut tx,
        NewVisitEvent {
            tenant_id: access.tenant_id,
            owner_id: current.inventory_owner_id,
            facility_id: current.facility_id,
            visit_id: command.visit_id,
            kind: event_kind,
            from_status: Some(current.status),
            to_status: target,
            from_location_id: current.current_location_id,
            to_location_id: current.current_location_id,
            operation: Some(command.operation),
            note: Some(command.note.as_str()),
            revision,
            actor_id: context.actor_id,
            occurred_at: now,
        },
    )
    .await?;
    finish_visit_command(
        tx,
        access,
        context,
        prepared,
        command.visit_id,
        outbox_transition,
        now,
    )
    .await
}

pub async fn start_operation(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &YardDockOperationCommand,
) -> AppResult<YardVisitReadModel> {
    transition_operation(db, access, context, command, DockTransition::Start).await
}

pub async fn complete_operation(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &YardDockOperationCommand,
) -> AppResult<YardVisitReadModel> {
    transition_operation(db, access, context, command, DockTransition::Complete).await
}

pub async fn reject_visit(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &YardVisitLifecycleCommand,
) -> AppResult<YardVisitReadModel> {
    require_access_actor(access, context)?;
    let prepared = PreparedCommand::new_v1(context, REJECT_YARD_VISIT_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    if let Some(result) = prepared.replayed::<YardVisitReadModel>(&mut tx).await? {
        verify_visit_replay(&scope, &result)?;
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_visit_tx(&mut tx, access.tenant_id, command.visit_id).await?;
    verify_visit_replay(&scope, &current)?;
    if current.revision != command.expected_revision {
        return Err(AppError::conflict("yard visit revision does not match"));
    }
    let target = current.status.reject().map_err(conflict)?;
    let now = now_iso();
    let revision = next_revision(current.revision)?;
    sqlx::query(
        "UPDATE yard_visits SET status=$3,revision=$4,rejected_at=$5 WHERE tenant_id=$1 AND id=$2",
    )
    .bind(access.tenant_id.get())
    .bind(command.visit_id.get())
    .bind(target.as_str())
    .bind(revision.get())
    .bind(now)
    .execute(&mut *tx)
    .await?;
    insert_visit_event_tx(
        &mut tx,
        NewVisitEvent {
            tenant_id: access.tenant_id,
            owner_id: current.inventory_owner_id,
            facility_id: current.facility_id,
            visit_id: command.visit_id,
            kind: YardVisitEventKind::Rejected,
            from_status: Some(current.status),
            to_status: target,
            from_location_id: current.current_location_id,
            to_location_id: current.current_location_id,
            operation: None,
            note: Some(command.note.as_str()),
            revision,
            actor_id: context.actor_id,
            occurred_at: now,
        },
    )
    .await?;
    finish_visit_command(
        tx,
        access,
        context,
        prepared,
        command.visit_id,
        "rejected",
        now,
    )
    .await
}

async fn capture_detention_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    current: &YardVisitReadModel,
    billable_hours: u64,
    occurred_at: chrono::DateTime<chrono::Utc>,
) -> AppResult<Option<BillableEventId>> {
    if billable_hours == 0 {
        return Ok(None);
    }
    let contract_id = sqlx::query_scalar::<_, i64>(
        r#"SELECT id FROM billing_contracts WHERE tenant_id=$1 AND inventory_owner_id=$2
           AND status='active' AND effective_from<=$3
           AND (effective_until IS NULL OR effective_until>$3)
           ORDER BY effective_from DESC,id DESC LIMIT 1 FOR SHARE"#,
    )
    .bind(access.tenant_id.get())
    .bind(current.inventory_owner_id.get())
    .bind(occurred_at)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(contract_id) = contract_id else {
        return Ok(None);
    };
    let quantity = i64::try_from(billable_hours)
        .map_err(|_| AppError::internal("yard detention quantity overflow"))?;
    let event_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO billable_events
           (tenant_id,inventory_owner_id,facility_id,contract_id,event_type,unit,quantity,
            source_type,source_reference,description,occurred_at,captured_by_user_id,captured_at)
           VALUES($1,$2,$3,$4,'detention_hour','hour',$5,'yard_detention',$6,
             'Yard detention beyond contracted free time',$7,$8,$7) RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(current.inventory_owner_id.get())
    .bind(current.facility_id.get())
    .bind(contract_id)
    .bind(quantity)
    .bind(current.visit_id.get().to_string())
    .bind(occurred_at)
    .bind(context.actor_id.get())
    .fetch_one(&mut **tx)
    .await?;
    Ok(Some(BillableEventId::new(event_id).map_err(internal)?))
}

pub async fn gate_out(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &YardVisitLifecycleCommand,
) -> AppResult<YardVisitReadModel> {
    require_access_actor(access, context)?;
    let prepared = PreparedCommand::new_v1(context, GATE_OUT_YARD_VISIT_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    if let Some(result) = prepared.replayed::<YardVisitReadModel>(&mut tx).await? {
        verify_visit_replay(&scope, &result)?;
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_visit_tx(&mut tx, access.tenant_id, command.visit_id).await?;
    verify_visit_replay(&scope, &current)?;
    if current.revision != command.expected_revision {
        return Err(AppError::conflict("yard visit revision does not match"));
    }
    let target = current.status.gate_out().map_err(conflict)?;
    let now = now_iso();
    let revision = next_revision(current.revision)?;
    let free_minutes = if let Some(appointment_id) = current.appointment_id {
        read_appointment_tx(&mut tx, access.tenant_id, appointment_id)
            .await?
            .free_minutes
    } else {
        YardFreeMinutes::new(0).map_err(internal)?
    };
    let detention =
        calculate_yard_detention(current.gated_in_at, now, free_minutes).map_err(internal)?;
    sqlx::query(
        r#"UPDATE yard_visits SET status=$3,revision=$4,current_location_id=NULL,
           dock_door_location_id=NULL,gated_out_at=$5 WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.visit_id.get())
    .bind(target.as_str())
    .bind(revision.get())
    .bind(now)
    .execute(&mut *tx)
    .await?;
    insert_visit_event_tx(
        &mut tx,
        NewVisitEvent {
            tenant_id: access.tenant_id,
            owner_id: current.inventory_owner_id,
            facility_id: current.facility_id,
            visit_id: command.visit_id,
            kind: YardVisitEventKind::GatedOut,
            from_status: Some(current.status),
            to_status: target,
            from_location_id: current.current_location_id,
            to_location_id: None,
            operation: None,
            note: Some(command.note.as_str()),
            revision,
            actor_id: context.actor_id,
            occurred_at: now,
        },
    )
    .await?;
    let billable_event_id = capture_detention_event_tx(
        &mut tx,
        access,
        context,
        &current,
        detention.billable_hours,
        now,
    )
    .await?;
    sqlx::query(
        r#"INSERT INTO yard_detention_records
           (tenant_id,inventory_owner_id,facility_id,visit_id,total_minutes,free_minutes,
            detention_minutes,billable_hours,billable_event_id,calculated_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)"#,
    )
    .bind(access.tenant_id.get())
    .bind(current.inventory_owner_id.get())
    .bind(current.facility_id.get())
    .bind(command.visit_id.get())
    .bind(i64::try_from(detention.total_minutes).map_err(internal)?)
    .bind(i32::try_from(detention.free_minutes).map_err(internal)?)
    .bind(i64::try_from(detention.detention_minutes).map_err(internal)?)
    .bind(i64::try_from(detention.billable_hours).map_err(internal)?)
    .bind(billable_event_id.map(|id| id.get()))
    .bind(now)
    .execute(&mut *tx)
    .await?;
    if let Some(appointment_id) = current.appointment_id {
        sqlx::query("SELECT id FROM yard_appointments WHERE tenant_id=$1 AND id=$2 FOR UPDATE")
            .bind(access.tenant_id.get())
            .bind(appointment_id.get())
            .fetch_one(&mut *tx)
            .await?;
        let appointment = read_appointment_tx(&mut tx, access.tenant_id, appointment_id).await?;
        let appointment_revision = next_revision(appointment.revision)?;
        appointment
            .status
            .transition(YardAppointmentStatus::Completed)
            .map_err(conflict)?;
        sqlx::query(
            r#"UPDATE yard_appointments SET status='completed',revision=$3,
               updated_by_user_id=$4,updated_at=$5 WHERE tenant_id=$1 AND id=$2"#,
        )
        .bind(access.tenant_id.get())
        .bind(appointment_id.get())
        .bind(appointment_revision.get())
        .bind(context.actor_id.get())
        .bind(now)
        .execute(&mut *tx)
        .await?;
        insert_appointment_event_tx(
            &mut tx,
            NewAppointmentEvent {
                tenant_id: access.tenant_id,
                owner_id: appointment.inventory_owner_id,
                facility_id: appointment.facility_id,
                appointment_id,
                kind: "completed",
                from_status: Some(appointment.status),
                to_status: YardAppointmentStatus::Completed,
                note: Some(command.note.as_str()),
                revision: appointment_revision,
                actor_id: context.actor_id,
                occurred_at: now,
            },
        )
        .await?;
        let updated = read_appointment_tx(&mut tx, access.tenant_id, appointment_id).await?;
        enqueue_event_tx(
            &mut tx,
            YardOutboxEvent {
                tenant_id: access.tenant_id,
                actor_id: context.actor_id,
                owner_id: Some(updated.inventory_owner_id),
                facility_id: Some(updated.facility_id),
                aggregate_type: "appointment",
                aggregate_id: updated.appointment_id.get(),
                transition: "completed",
                occurred_at: now,
            },
            &updated,
        )
        .await?;
    }
    finish_visit_command(
        tx,
        access,
        context,
        prepared,
        command.visit_id,
        "gated_out",
        now,
    )
    .await
}

async fn finish_visit_command(
    mut tx: sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    prepared: PreparedCommand,
    visit_id: YardVisitId,
    transition: &str,
    occurred_at: chrono::DateTime<chrono::Utc>,
) -> AppResult<YardVisitReadModel> {
    let result = read_visit_tx(&mut tx, access.tenant_id, visit_id).await?;
    enqueue_event_tx(
        &mut tx,
        YardOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            owner_id: Some(result.inventory_owner_id),
            facility_id: Some(result.facility_id),
            aggregate_type: "visit",
            aggregate_id: visit_id.get(),
            transition,
            occurred_at,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}
