use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::yard::{
    ConfigureYardLocationCommand, CreateYardAppointmentCommand, RegisterYardAssetCommand,
    YardAppointmentLifecycleCommand, YardAppointmentReadModel, YardAssetReadModel,
    YardLocationReadModel, CANCEL_YARD_APPOINTMENT_OPERATION, CONFIGURE_YARD_LOCATION_OPERATION,
    CREATE_YARD_APPOINTMENT_OPERATION, MARK_YARD_APPOINTMENT_NO_SHOW_OPERATION,
    REGISTER_YARD_ASSET_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    YardAppointmentId, YardAppointmentStatus, YardAssetId, YardDirection, YardLocationId,
    YardRevision,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use super::{
    insert_appointment_event_tx, lock_key_tx, validate_owner_facility_tx, NewAppointmentEvent,
};
use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};
use crate::repo::yard::models::{read_appointment_tx, read_asset_tx, read_location_tx};
use crate::repo::yard::{
    asset_kind_name, direction_name, enqueue_event_tx, location_kind_name, require_access_actor,
    require_facility, require_scope, YardOutboxEvent, PERMISSION,
};

pub async fn configure_location(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigureYardLocationCommand,
) -> AppResult<YardLocationReadModel> {
    require_access_actor(access, context)?;
    let prepared = PreparedCommand::new_v1(context, CONFIGURE_YARD_LOCATION_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    require_facility(&scope, command.facility_id)?;
    if let Some(result) = prepared.replayed::<YardLocationReadModel>(&mut tx).await? {
        require_facility(&scope, result.facility_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    lock_key_tx(
        &mut tx,
        format!(
            "yard-location:{}:{}:{}",
            access.tenant_id.get(),
            command.facility_id.get(),
            command.code.as_str()
        ),
    )
    .await?;
    let facility_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM facilities WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL)",
    )
    .bind(access.tenant_id.get())
    .bind(command.facility_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if !facility_exists {
        return Err(AppError::not_found("facility"));
    }
    let duplicate = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM yard_locations WHERE tenant_id=$1 AND facility_id=$2 AND code=$3)",
    )
    .bind(access.tenant_id.get())
    .bind(command.facility_id.get())
    .bind(command.code.as_str())
    .fetch_one(&mut *tx)
    .await?;
    if duplicate {
        return Err(AppError::conflict("yard location code already exists"));
    }
    let now = now_iso();
    let location_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO yard_locations
           (tenant_id,facility_id,code,name,kind,created_by_user_id,created_at)
           VALUES($1,$2,$3,$4,$5,$6,$7) RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.facility_id.get())
    .bind(command.code.as_str())
    .bind(command.name.as_str())
    .bind(location_kind_name(command.kind))
    .bind(context.actor_id.get())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    let result = read_location_tx(
        &mut tx,
        access.tenant_id,
        YardLocationId::new(location_id).map_err(super::super::internal)?,
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        YardOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            owner_id: None,
            facility_id: Some(result.facility_id),
            aggregate_type: "location",
            aggregate_id: location_id,
            transition: "configured",
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn register_asset(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &RegisterYardAssetCommand,
) -> AppResult<YardAssetReadModel> {
    require_access_actor(access, context)?;
    let prepared = PreparedCommand::new_v1(context, REGISTER_YARD_ASSET_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let _scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    if let Some(result) = prepared.replayed::<YardAssetReadModel>(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    lock_key_tx(
        &mut tx,
        format!(
            "yard-asset:{}:{}:{}",
            access.tenant_id.get(),
            asset_kind_name(command.kind),
            command.asset_number.as_str()
        ),
    )
    .await?;
    let duplicate = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM yard_assets WHERE tenant_id=$1 AND kind=$2 AND asset_number=$3)",
    )
    .bind(access.tenant_id.get())
    .bind(asset_kind_name(command.kind))
    .bind(command.asset_number.as_str())
    .fetch_one(&mut *tx)
    .await?;
    if duplicate {
        return Err(AppError::conflict("yard asset already exists"));
    }
    let now = now_iso();
    let asset_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO yard_assets
           (tenant_id,kind,asset_number,carrier,created_by_user_id,created_at)
           VALUES($1,$2,$3,$4,$5,$6) RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(asset_kind_name(command.kind))
    .bind(command.asset_number.as_str())
    .bind(command.carrier.as_str())
    .bind(context.actor_id.get())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    let result = read_asset_tx(
        &mut tx,
        access.tenant_id,
        YardAssetId::new(asset_id).map_err(super::super::internal)?,
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        YardOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            owner_id: None,
            facility_id: None,
            aggregate_type: "asset",
            aggregate_id: asset_id,
            transition: "registered",
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

async fn validate_load_binding_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    command: &CreateYardAppointmentCommand,
) -> AppResult<()> {
    let valid = match command.direction {
        YardDirection::Inbound if command.outbound_load_id.is_some() => false,
        YardDirection::Outbound if command.inbound_load_id.is_some() => false,
        YardDirection::Inbound => match command.inbound_load_id {
            None => true,
            Some(load_id) => {
                sqlx::query_scalar::<_, bool>(
                    r#"SELECT EXISTS(SELECT 1 FROM loads WHERE tenant_id=$1
                       AND inventory_owner_id=$2 AND facility_id=$3 AND id=$4
                       AND type='inbound' AND deleted IS NULL)"#,
                )
                .bind(tenant_id)
                .bind(command.inventory_owner_id.get())
                .bind(command.facility_id.get())
                .bind(load_id.get())
                .fetch_one(&mut **tx)
                .await?
            }
        },
        YardDirection::Outbound => match command.outbound_load_id {
            None => true,
            Some(load_id) => {
                sqlx::query_scalar::<_, bool>(
                    r#"SELECT EXISTS(SELECT 1 FROM outbound_loads outbound
                       JOIN outbound_load_shipments shipment ON shipment.tenant_id=outbound.tenant_id
                         AND shipment.outbound_load_id=outbound.id
                       WHERE outbound.tenant_id=$1 AND outbound.facility_id=$2 AND outbound.id=$3
                         AND shipment.inventory_owner_id=$4)"#,
                )
                .bind(tenant_id)
                .bind(command.facility_id.get())
                .bind(load_id.get())
                .bind(command.inventory_owner_id.get())
                .fetch_one(&mut **tx)
                .await?
            }
        },
    };
    if valid {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "yard appointment load does not match direction and scope",
        ))
    }
}

pub async fn create_appointment(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CreateYardAppointmentCommand,
) -> AppResult<YardAppointmentReadModel> {
    require_access_actor(access, context)?;
    let prepared = PreparedCommand::new_v1(context, CREATE_YARD_APPOINTMENT_OPERATION, command)?;
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
    if let Some(result) = prepared
        .replayed::<YardAppointmentReadModel>(&mut tx)
        .await?
    {
        require_scope(&scope, result.inventory_owner_id, result.facility_id)?;
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
    validate_load_binding_tx(&mut tx, access.tenant_id.get(), command).await?;
    lock_key_tx(
        &mut tx,
        format!(
            "yard-appointment:{}:{}:{}",
            access.tenant_id.get(),
            command.facility_id.get(),
            command.appointment_number.as_str()
        ),
    )
    .await?;
    let duplicate = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM yard_appointments WHERE tenant_id=$1 AND facility_id=$2 AND appointment_number=$3)",
    )
    .bind(access.tenant_id.get())
    .bind(command.facility_id.get())
    .bind(command.appointment_number.as_str())
    .fetch_one(&mut *tx)
    .await?;
    if duplicate {
        return Err(AppError::conflict("yard appointment number already exists"));
    }
    let now = now_iso();
    let appointment_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO yard_appointments
           (tenant_id,inventory_owner_id,facility_id,direction,appointment_number,
            scheduled_from,scheduled_until,carrier,expected_asset_kind,expected_asset_number,
            inbound_load_id,outbound_load_id,free_minutes,note,created_by_user_id,created_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16) RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.inventory_owner_id.get())
    .bind(command.facility_id.get())
    .bind(direction_name(command.direction))
    .bind(command.appointment_number.as_str())
    .bind(command.window.scheduled_from)
    .bind(command.window.scheduled_until)
    .bind(command.carrier.as_str())
    .bind(asset_kind_name(command.expected_asset_kind))
    .bind(
        command
            .expected_asset_number
            .as_ref()
            .map(|value| value.as_str()),
    )
    .bind(command.inbound_load_id.map(|id| id.get()))
    .bind(command.outbound_load_id.map(|id| id.get()))
    .bind(i32::try_from(command.free_minutes.get()).map_err(super::super::internal)?)
    .bind(command.note.as_ref().map(|value| value.as_str()))
    .bind(context.actor_id.get())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    let appointment_id = YardAppointmentId::new(appointment_id).map_err(super::super::internal)?;
    insert_appointment_event_tx(
        &mut tx,
        NewAppointmentEvent {
            tenant_id: access.tenant_id,
            owner_id: command.inventory_owner_id,
            facility_id: command.facility_id,
            appointment_id,
            kind: "created",
            from_status: None,
            to_status: YardAppointmentStatus::Scheduled,
            note: command.note.as_ref().map(|value| value.as_str()),
            revision: YardRevision::new(1).map_err(super::super::internal)?,
            actor_id: context.actor_id,
            occurred_at: now,
        },
    )
    .await?;
    let result = read_appointment_tx(&mut tx, access.tenant_id, appointment_id).await?;
    enqueue_event_tx(
        &mut tx,
        YardOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            owner_id: Some(result.inventory_owner_id),
            facility_id: Some(result.facility_id),
            aggregate_type: "appointment",
            aggregate_id: appointment_id.get(),
            transition: "created",
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

#[derive(Debug, Clone, Copy)]
enum AppointmentTransition {
    Cancel,
    NoShow,
}

async fn transition_appointment(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &YardAppointmentLifecycleCommand,
    transition: AppointmentTransition,
) -> AppResult<YardAppointmentReadModel> {
    require_access_actor(access, context)?;
    let (operation, target, kind) = match transition {
        AppointmentTransition::Cancel => (
            CANCEL_YARD_APPOINTMENT_OPERATION,
            YardAppointmentStatus::Cancelled,
            "cancelled",
        ),
        AppointmentTransition::NoShow => (
            MARK_YARD_APPOINTMENT_NO_SHOW_OPERATION,
            YardAppointmentStatus::NoShow,
            "no_show",
        ),
    };
    let prepared = PreparedCommand::new_v1(context, operation, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    if let Some(result) = prepared
        .replayed::<YardAppointmentReadModel>(&mut tx)
        .await?
    {
        require_scope(&scope, result.inventory_owner_id, result.facility_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    lock_key_tx(
        &mut tx,
        format!(
            "yard-appointment:{}:{}",
            access.tenant_id.get(),
            command.appointment_id.get()
        ),
    )
    .await?;
    sqlx::query("SELECT id FROM yard_appointments WHERE tenant_id=$1 AND id=$2 FOR UPDATE")
        .bind(access.tenant_id.get())
        .bind(command.appointment_id.get())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| AppError::not_found("yard appointment"))?;
    let current = read_appointment_tx(&mut tx, access.tenant_id, command.appointment_id).await?;
    require_scope(&scope, current.inventory_owner_id, current.facility_id)?;
    if current.revision != command.expected_revision {
        return Err(AppError::conflict(
            "yard appointment revision does not match",
        ));
    }
    current
        .status
        .transition(target)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let now = now_iso();
    let revision = current.revision.next().map_err(super::super::internal)?;
    sqlx::query(
        r#"UPDATE yard_appointments SET status=$3,revision=$4,updated_by_user_id=$5,updated_at=$6
           WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.appointment_id.get())
    .bind(target.as_str())
    .bind(revision.get())
    .bind(context.actor_id.get())
    .bind(now)
    .execute(&mut *tx)
    .await?;
    insert_appointment_event_tx(
        &mut tx,
        NewAppointmentEvent {
            tenant_id: access.tenant_id,
            owner_id: current.inventory_owner_id,
            facility_id: current.facility_id,
            appointment_id: command.appointment_id,
            kind,
            from_status: Some(current.status),
            to_status: target,
            note: Some(command.note.as_str()),
            revision,
            actor_id: context.actor_id,
            occurred_at: now,
        },
    )
    .await?;
    let result = read_appointment_tx(&mut tx, access.tenant_id, command.appointment_id).await?;
    enqueue_event_tx(
        &mut tx,
        YardOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            owner_id: Some(result.inventory_owner_id),
            facility_id: Some(result.facility_id),
            aggregate_type: "appointment",
            aggregate_id: command.appointment_id.get(),
            transition: kind,
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn cancel_appointment(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &YardAppointmentLifecycleCommand,
) -> AppResult<YardAppointmentReadModel> {
    transition_appointment(db, access, context, command, AppointmentTransition::Cancel).await
}

pub async fn mark_no_show(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &YardAppointmentLifecycleCommand,
) -> AppResult<YardAppointmentReadModel> {
    transition_appointment(db, access, context, command, AppointmentTransition::NoShow).await
}
