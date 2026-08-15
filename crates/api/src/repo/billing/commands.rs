use chrono::{DateTime, Utc};
use sqlx::Row;
use wareboxes_application::billing::{
    BillableEventReadModel, BillingContractLifecycleCommand, BillingContractReadModel,
    BillingRateReadModel, BillingRunReadModel, BillingStorageSnapshotReadModel,
    CaptureBillableEventCommand, CaptureStorageSnapshotCommand, ConfigureBillingRateCommand,
    CreateBillingContractCommand, GenerateBillingRunCommand, ACTIVATE_BILLING_CONTRACT_OPERATION,
    CAPTURE_BILLABLE_EVENT_OPERATION, CAPTURE_STORAGE_SNAPSHOT_OPERATION,
    CLOSE_BILLING_CONTRACT_OPERATION, CONFIGURE_BILLING_RATE_OPERATION,
    CREATE_BILLING_CONTRACT_OPERATION, GENERATE_BILLING_RUN_OPERATION,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    BillableEventId, BillableEventType, BillingContractId, BillingContractStatus, BillingRateId,
    BillingReconciliationRunId, BillingStorageSnapshotId, BillingUnit, FacilityId,
    InventoryOwnerId,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use super::models::{read_contract_tx, read_event_tx, read_rate_tx, read_run_tx, read_snapshot_tx};
use super::{
    enqueue_event_tx, event_name, internal, require_access_actor, require_owner,
    require_record_scope, unit_name, BillingOutboxEvent, PERMISSION,
};
use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};

async fn lock_contract_key_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    contract_id: i64,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("billing-contract:{tenant_id}:{contract_id}"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn validate_owner_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    owner_id: InventoryOwnerId,
) -> AppResult<()> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM inventory_owners WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL)",
    )
    .bind(tenant_id)
    .bind(owner_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(AppError::not_found("inventory owner"))
    }
}

async fn validate_owner_facility_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
) -> AppResult<()> {
    let exists = sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS(
             SELECT 1 FROM inventory_owner_facilities link
             JOIN inventory_owners owner ON owner.tenant_id=link.tenant_id
               AND owner.id=link.inventory_owner_id AND owner.deleted IS NULL
             JOIN facilities facility ON facility.tenant_id=link.tenant_id
               AND facility.id=link.facility_id AND facility.deleted IS NULL
             WHERE link.tenant_id=$1 AND link.inventory_owner_id=$2
               AND link.facility_id=$3 AND link.deleted IS NULL)"#,
    )
    .bind(tenant_id)
    .bind(owner_id.get())
    .bind(facility_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(AppError::not_found("owner-facility assignment"))
    }
}

fn verify_contract_replay(
    scope: &ScopeBindings,
    result: &BillingContractReadModel,
) -> AppResult<()> {
    require_owner(scope, result.inventory_owner_id)
}

fn verify_rate_replay(scope: &ScopeBindings, result: &BillingRateReadModel) -> AppResult<()> {
    require_owner(scope, result.inventory_owner_id)
}

fn verify_event_replay(scope: &ScopeBindings, result: &BillableEventReadModel) -> AppResult<()> {
    require_record_scope(scope, result.inventory_owner_id, Some(result.facility_id))
}

fn verify_snapshot_replay(
    scope: &ScopeBindings,
    result: &BillingStorageSnapshotReadModel,
) -> AppResult<()> {
    require_record_scope(scope, result.inventory_owner_id, Some(result.facility_id))
}

fn verify_run_replay(scope: &ScopeBindings, result: &BillingRunReadModel) -> AppResult<()> {
    require_record_scope(scope, result.inventory_owner_id, result.facility_id)
}

pub async fn create_contract(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CreateBillingContractCommand,
) -> AppResult<BillingContractReadModel> {
    require_access_actor(access, context)?;
    let prepared = PreparedCommand::new_v1(context, CREATE_BILLING_CONTRACT_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    require_owner(&scope, command.inventory_owner_id)?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        verify_contract_replay(&scope, &result)?;
        tx.commit().await?;
        return Ok(result);
    }
    validate_owner_tx(&mut tx, access.tenant_id.get(), command.inventory_owner_id).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "billing-contract-number:{}:{}:{}",
            access.tenant_id.get(),
            command.inventory_owner_id.get(),
            command.contract_number.as_str()
        ))
        .execute(&mut *tx)
        .await?;
    let duplicate = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM billing_contracts WHERE tenant_id=$1 AND inventory_owner_id=$2 AND contract_number=$3)",
    )
    .bind(access.tenant_id.get())
    .bind(command.inventory_owner_id.get())
    .bind(command.contract_number.as_str())
    .fetch_one(&mut *tx)
    .await?;
    if duplicate {
        return Err(AppError::conflict("billing contract number already exists"));
    }
    let created_at = now_iso();
    let contract_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO billing_contracts
             (tenant_id,inventory_owner_id,contract_number,currency,effective_from,
              effective_until,created_by_user_id,created_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.inventory_owner_id.get())
    .bind(command.contract_number.as_str())
    .bind(command.currency.as_str())
    .bind(command.effective_window.effective_from)
    .bind(command.effective_window.effective_until)
    .bind(context.actor_id.get())
    .bind(created_at)
    .fetch_one(&mut *tx)
    .await?;
    let result = read_contract_tx(
        &mut tx,
        access.tenant_id,
        BillingContractId::new(contract_id).map_err(internal)?,
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        BillingOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            owner_id: result.inventory_owner_id,
            facility_id: None,
            aggregate_type: "contract",
            aggregate_id: contract_id,
            transition: "created",
            occurred_at: created_at,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

#[derive(Debug, Clone, Copy)]
enum ContractTransition {
    Activate,
    Close,
}

async fn transition_contract(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &BillingContractLifecycleCommand,
    transition: ContractTransition,
) -> AppResult<BillingContractReadModel> {
    require_access_actor(access, context)?;
    let operation = match transition {
        ContractTransition::Activate => ACTIVATE_BILLING_CONTRACT_OPERATION,
        ContractTransition::Close => CLOSE_BILLING_CONTRACT_OPERATION,
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
    if let Some(result) = prepared.replayed(&mut tx).await? {
        verify_contract_replay(&scope, &result)?;
        tx.commit().await?;
        return Ok(result);
    }
    lock_contract_key_tx(&mut tx, access.tenant_id.get(), command.contract_id.get()).await?;
    let current = read_contract_tx(&mut tx, access.tenant_id, command.contract_id).await?;
    require_owner(&scope, current.inventory_owner_id)?;
    if current.revision != command.expected_revision {
        return Err(AppError::conflict(
            "billing contract revision does not match",
        ));
    }
    let now = now_iso();
    match transition {
        ContractTransition::Activate => {
            current
                .status
                .activate()
                .map_err(|error| AppError::conflict(error.to_string()))?;
            let rate_count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM billing_rate_versions WHERE tenant_id=$1 AND contract_id=$2 AND status='active'",
            )
            .bind(access.tenant_id.get())
            .bind(command.contract_id.get())
            .fetch_one(&mut *tx)
            .await?;
            if rate_count == 0 {
                return Err(AppError::conflict(
                    "billing contract requires at least one active rate before activation",
                ));
            }
            sqlx::query(
                r#"UPDATE billing_contracts SET status='active',revision=revision+1,
                     activated_by_user_id=$3,activated_at=$4 WHERE tenant_id=$1 AND id=$2"#,
            )
            .bind(access.tenant_id.get())
            .bind(command.contract_id.get())
            .bind(context.actor_id.get())
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        ContractTransition::Close => {
            current
                .status
                .close()
                .map_err(|error| AppError::conflict(error.to_string()))?;
            let pending: bool = sqlx::query_scalar(
                r#"SELECT EXISTS(SELECT 1 FROM billing_reconciliation_runs
                   WHERE tenant_id=$1 AND contract_id=$2 AND status IN ('pending_review','approved'))"#,
            )
            .bind(access.tenant_id.get())
            .bind(command.contract_id.get())
            .fetch_one(&mut *tx)
            .await?;
            if pending {
                return Err(AppError::conflict(
                    "billing contract has reconciliation runs awaiting review or export",
                ));
            }
            sqlx::query(
                r#"UPDATE billing_contracts SET status='closed',revision=revision+1,
                     closed_by_user_id=$3,closed_at=$4 WHERE tenant_id=$1 AND id=$2"#,
            )
            .bind(access.tenant_id.get())
            .bind(command.contract_id.get())
            .bind(context.actor_id.get())
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
    }
    let result = read_contract_tx(&mut tx, access.tenant_id, command.contract_id).await?;
    let transition_name = match transition {
        ContractTransition::Activate => "activated",
        ContractTransition::Close => "closed",
    };
    enqueue_event_tx(
        &mut tx,
        BillingOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            owner_id: result.inventory_owner_id,
            facility_id: None,
            aggregate_type: "contract",
            aggregate_id: command.contract_id.get(),
            transition: transition_name,
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn activate_contract(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &BillingContractLifecycleCommand,
) -> AppResult<BillingContractReadModel> {
    transition_contract(db, access, context, command, ContractTransition::Activate).await
}

pub async fn close_contract(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &BillingContractLifecycleCommand,
) -> AppResult<BillingContractReadModel> {
    transition_contract(db, access, context, command, ContractTransition::Close).await
}

pub async fn configure_rate(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigureBillingRateCommand,
) -> AppResult<BillingRateReadModel> {
    require_access_actor(access, context)?;
    let prepared = PreparedCommand::new_v1(context, CONFIGURE_BILLING_RATE_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        verify_rate_replay(&scope, &result)?;
        tx.commit().await?;
        return Ok(result);
    }
    lock_contract_key_tx(&mut tx, access.tenant_id.get(), command.contract_id.get()).await?;
    let contract = read_contract_tx(&mut tx, access.tenant_id, command.contract_id).await?;
    require_owner(&scope, contract.inventory_owner_id)?;
    if contract.status == BillingContractStatus::Closed {
        return Err(AppError::conflict(
            "closed billing contract cannot accept rates",
        ));
    }
    if command.definition.currency.as_str() != contract.currency {
        return Err(AppError::conflict(
            "billing rate currency must match its contract",
        ));
    }
    if command.effective_window.effective_from < contract.effective_window.effective_from
        || contract
            .effective_window
            .effective_until
            .is_some_and(|until| {
                command
                    .effective_window
                    .effective_until
                    .is_none_or(|rate_until| rate_until > until)
            })
    {
        return Err(AppError::conflict(
            "billing rate effective window must be contained by its contract",
        ));
    }
    let latest: Option<i64> = sqlx::query_scalar(
        r#"SELECT max(revision) FROM billing_rate_versions
           WHERE tenant_id=$1 AND contract_id=$2 AND event_type=$3"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.contract_id.get())
    .bind(event_name(command.definition.event_type))
    .fetch_one(&mut *tx)
    .await?;
    let revision = match (latest, command.expected_revision) {
        (None, None) => 1,
        (Some(latest), Some(expected)) if latest == expected => latest
            .checked_add(1)
            .ok_or_else(|| AppError::internal("billing rate revision overflow"))?,
        (None, Some(_)) => return Err(AppError::conflict("billing rate has no prior revision")),
        (Some(_), None) => {
            return Err(AppError::conflict(
                "expected_revision is required to revise a billing rate",
            ));
        }
        (Some(_), Some(_)) => {
            return Err(AppError::conflict("billing rate revision does not match"));
        }
    };
    let now = now_iso();
    sqlx::query(
        r#"UPDATE billing_rate_versions SET status='retired',retired_by_user_id=$6,retired_at=$7
           WHERE tenant_id=$1 AND contract_id=$2 AND event_type=$3 AND status='active'
             AND tstzrange(effective_from,effective_until,'[)') && tstzrange($4,$5,'[)')"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.contract_id.get())
    .bind(event_name(command.definition.event_type))
    .bind(command.effective_window.effective_from)
    .bind(command.effective_window.effective_until)
    .bind(context.actor_id.get())
    .bind(now)
    .execute(&mut *tx)
    .await?;
    let rate_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO billing_rate_versions
             (tenant_id,inventory_owner_id,contract_id,event_type,unit,currency,rate_minor,
              minimum_charge_minor,effective_from,effective_until,revision,created_by_user_id,created_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(contract.inventory_owner_id.get())
    .bind(command.contract_id.get())
    .bind(event_name(command.definition.event_type))
    .bind(unit_name(command.definition.unit))
    .bind(command.definition.currency.as_str())
    .bind(i64::try_from(command.definition.rate_minor).map_err(internal)?)
    .bind(i64::try_from(command.definition.minimum_charge_minor).map_err(internal)?)
    .bind(command.effective_window.effective_from)
    .bind(command.effective_window.effective_until)
    .bind(revision)
    .bind(context.actor_id.get())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    let result = read_rate_tx(
        &mut tx,
        access.tenant_id,
        BillingRateId::new(rate_id).map_err(internal)?,
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        BillingOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            owner_id: result.inventory_owner_id,
            facility_id: None,
            aggregate_type: "rate",
            aggregate_id: rate_id,
            transition: "configured",
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

fn validate_manual_event(event_type: BillableEventType, unit: BillingUnit) -> AppResult<()> {
    let valid = match event_type {
        BillableEventType::RelabelUnit
        | BillableEventType::RefurbishmentUnit
        | BillableEventType::KitUnit
        | BillableEventType::AssemblyUnit
        | BillableEventType::ValueAddedServiceUnit => {
            matches!(
                unit,
                BillingUnit::Each | BillingUnit::Case | BillingUnit::Pallet
            )
        }
        BillableEventType::Accessorial => unit == BillingUnit::Event,
        BillableEventType::DetentionHour => unit == BillingUnit::Hour,
        BillableEventType::ReceiptLine
        | BillableEventType::ReceivedUnit
        | BillableEventType::PalletDay
        | BillableEventType::PickLine
        | BillableEventType::PickedUnit
        | BillableEventType::PackedCarton
        | BillableEventType::ShippedUnit
        | BillableEventType::ReturnUnit => false,
    };
    if valid {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "event type is operational or uses an incompatible billing unit",
        ))
    }
}

pub async fn capture_billable_event(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CaptureBillableEventCommand,
) -> AppResult<BillableEventReadModel> {
    require_access_actor(access, context)?;
    validate_manual_event(command.event_type, command.unit)?;
    let description = command.description.trim();
    let reference = command.source_reference.trim();
    if description.is_empty() || description.len() > 500 {
        return Err(AppError::bad_request(
            "billing event description must be between 1 and 500 characters",
        ));
    }
    if reference.is_empty() || reference.len() > 160 {
        return Err(AppError::bad_request(
            "billing event source reference must be between 1 and 160 characters",
        ));
    }
    let prepared = PreparedCommand::new_v1(context, CAPTURE_BILLABLE_EVENT_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        verify_event_replay(&scope, &result)?;
        tx.commit().await?;
        return Ok(result);
    }
    lock_contract_key_tx(&mut tx, access.tenant_id.get(), command.contract_id.get()).await?;
    let contract = read_contract_tx(&mut tx, access.tenant_id, command.contract_id).await?;
    require_record_scope(
        &scope,
        contract.inventory_owner_id,
        Some(command.facility_id),
    )?;
    validate_owner_facility_tx(
        &mut tx,
        access.tenant_id.get(),
        contract.inventory_owner_id,
        command.facility_id,
    )
    .await?;
    if contract.status != BillingContractStatus::Active
        || !contract.effective_window.includes(command.occurred_at)
    {
        return Err(AppError::conflict(
            "billable event must occur within an active contract window",
        ));
    }
    let duplicate = sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS(SELECT 1 FROM billable_events WHERE tenant_id=$1 AND contract_id=$2
             AND event_type=$3 AND source_type='manual_service' AND source_reference=$4)"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.contract_id.get())
    .bind(event_name(command.event_type))
    .bind(reference)
    .fetch_one(&mut *tx)
    .await?;
    if duplicate {
        return Err(AppError::conflict("billable event source already exists"));
    }
    let captured_at = now_iso();
    let event_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO billable_events
             (tenant_id,inventory_owner_id,facility_id,contract_id,event_type,unit,quantity,
              source_type,source_reference,description,occurred_at,captured_by_user_id,captured_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,'manual_service',$8,$9,$10,$11,$12) RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(contract.inventory_owner_id.get())
    .bind(command.facility_id.get())
    .bind(command.contract_id.get())
    .bind(event_name(command.event_type))
    .bind(unit_name(command.unit))
    .bind(i64::try_from(command.quantity.get()).map_err(internal)?)
    .bind(reference)
    .bind(description)
    .bind(command.occurred_at)
    .bind(context.actor_id.get())
    .bind(captured_at)
    .fetch_one(&mut *tx)
    .await?;
    let result = read_event_tx(
        &mut tx,
        access.tenant_id,
        BillableEventId::new(event_id).map_err(internal)?,
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        BillingOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            owner_id: result.inventory_owner_id,
            facility_id: Some(result.facility_id),
            aggregate_type: "event",
            aggregate_id: event_id,
            transition: "captured",
            occurred_at: captured_at,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn capture_storage_snapshot(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CaptureStorageSnapshotCommand,
) -> AppResult<BillingStorageSnapshotReadModel> {
    require_access_actor(access, context)?;
    let prepared = PreparedCommand::new_v1(context, CAPTURE_STORAGE_SNAPSHOT_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        verify_snapshot_replay(&scope, &result)?;
        tx.commit().await?;
        return Ok(result);
    }
    lock_contract_key_tx(&mut tx, access.tenant_id.get(), command.contract_id.get()).await?;
    let contract = read_contract_tx(&mut tx, access.tenant_id, command.contract_id).await?;
    require_record_scope(
        &scope,
        contract.inventory_owner_id,
        Some(command.facility_id),
    )?;
    validate_owner_facility_tx(
        &mut tx,
        access.tenant_id.get(),
        contract.inventory_owner_id,
        command.facility_id,
    )
    .await?;
    let occurred_at = DateTime::<Utc>::from_naive_utc_and_offset(
        command
            .snapshot_date
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| AppError::bad_request("invalid storage snapshot date"))?,
        Utc,
    );
    if contract.status != BillingContractStatus::Active
        || !contract.effective_window.includes(occurred_at)
    {
        return Err(AppError::conflict(
            "storage snapshot must fall within an active contract window",
        ));
    }
    let duplicate = sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS(SELECT 1 FROM billing_storage_snapshots
           WHERE tenant_id=$1 AND contract_id=$2 AND facility_id=$3 AND snapshot_date=$4)"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.contract_id.get())
    .bind(command.facility_id.get())
    .bind(command.snapshot_date)
    .fetch_one(&mut *tx)
    .await?;
    if duplicate {
        return Err(AppError::conflict(
            "storage snapshot already exists for this date",
        ));
    }
    let counts = sqlx::query(
        r#"SELECT count(DISTINCT license_plate_id) FILTER (WHERE license_plate_id IS NOT NULL)::BIGINT
                  AS pallet_count,
                  COALESCE(sum(qty_on_hand),0)::BIGINT AS unit_count
           FROM inventory_balances WHERE tenant_id=$1 AND inventory_owner_id=$2
             AND facility_id=$3 AND deleted IS NULL AND qty_on_hand>0"#,
    )
    .bind(access.tenant_id.get())
    .bind(contract.inventory_owner_id.get())
    .bind(command.facility_id.get())
    .fetch_one(&mut *tx)
    .await?;
    let pallet_count: i64 = counts.try_get("pallet_count")?;
    let unit_count: i64 = counts.try_get("unit_count")?;
    let captured_at = now_iso();
    let snapshot_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO billing_storage_snapshots
             (tenant_id,inventory_owner_id,facility_id,contract_id,snapshot_date,
              pallet_count,unit_count,captured_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(contract.inventory_owner_id.get())
    .bind(command.facility_id.get())
    .bind(command.contract_id.get())
    .bind(command.snapshot_date)
    .bind(pallet_count)
    .bind(unit_count)
    .bind(captured_at)
    .fetch_one(&mut *tx)
    .await?;
    if pallet_count > 0 {
        sqlx::query(
            r#"INSERT INTO billable_events
                 (tenant_id,inventory_owner_id,facility_id,contract_id,event_type,unit,quantity,
                  source_type,source_reference,description,occurred_at,captured_by_user_id,captured_at)
               VALUES($1,$2,$3,$4,'pallet_day','pallet',$5,'storage_snapshot',$6,
                      'Daily occupied pallet positions',$7,$8,$9)"#,
        )
        .bind(access.tenant_id.get())
        .bind(contract.inventory_owner_id.get())
        .bind(command.facility_id.get())
        .bind(command.contract_id.get())
        .bind(pallet_count)
        .bind(format!("{snapshot_id}"))
        .bind(occurred_at)
        .bind(context.actor_id.get())
        .bind(captured_at)
        .execute(&mut *tx)
        .await?;
    }
    let result = read_snapshot_tx(
        &mut tx,
        access.tenant_id,
        BillingStorageSnapshotId::new(snapshot_id).map_err(internal)?,
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        BillingOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            owner_id: result.inventory_owner_id,
            facility_id: Some(result.facility_id),
            aggregate_type: "storage_snapshot",
            aggregate_id: snapshot_id,
            transition: "captured",
            occurred_at: captured_at,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

#[derive(Debug, Clone, Copy)]
struct OperationalEventWindow {
    tenant_id: i64,
    owner_id: InventoryOwnerId,
    contract_id: BillingContractId,
    facility_id: Option<FacilityId>,
    period_from: wareboxes_domain::Timestamp,
    period_until: wareboxes_domain::Timestamp,
    captured_at: wareboxes_domain::Timestamp,
}

async fn capture_operational_events_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    window: OperationalEventWindow,
) -> AppResult<()> {
    let OperationalEventWindow {
        tenant_id,
        owner_id,
        contract_id,
        facility_id,
        period_from,
        period_until,
        captured_at,
    } = window;
    // One quantity event per receipt transaction/facility. Customer-return ASNs are
    // classified separately so rate cards can distinguish reverse logistics.
    sqlx::query(
        r#"WITH received AS (
             SELECT transaction.id,entry.facility_id,transaction.created,
                    sum(entry.quantity_delta) FILTER (WHERE entry.quantity_delta>0)::BIGINT AS quantity,
                    bool_or(customer_return.id IS NOT NULL) AS is_return
             FROM inventory_transactions transaction
             JOIN inventory_entries entry ON entry.tenant_id=transaction.tenant_id
               AND entry.inventory_owner_id=transaction.inventory_owner_id
               AND entry.transaction_id=transaction.id
             LEFT JOIN inbound_asn_load_plan_lines plan_line
               ON transaction.reference_type='load_line'
              AND plan_line.tenant_id=transaction.tenant_id
              AND plan_line.load_line_id=transaction.reference_id
             LEFT JOIN customer_returns customer_return
               ON customer_return.tenant_id=plan_line.tenant_id
              AND customer_return.inbound_asn_id=plan_line.asn_id
             WHERE transaction.tenant_id=$1 AND transaction.inventory_owner_id=$2
               AND transaction.transaction_type='receive'
               AND transaction.created>=$4 AND transaction.created<$5
               AND ($6::BIGINT IS NULL OR entry.facility_id=$6)
             GROUP BY transaction.id,entry.facility_id,transaction.created)
           INSERT INTO billable_events
             (tenant_id,inventory_owner_id,facility_id,contract_id,event_type,unit,quantity,
              source_type,source_reference,description,occurred_at,captured_at)
           SELECT $1,$2,facility_id,$3,
                  CASE WHEN is_return THEN 'return_unit' ELSE 'received_unit' END,
                  'each',quantity,'inventory_transaction',id::TEXT||':'||facility_id::TEXT,
                  CASE WHEN is_return THEN 'Customer return received' ELSE 'Inbound units received' END,
                  created,$7 FROM received WHERE quantity>0
           ON CONFLICT(tenant_id,contract_id,event_type,source_type,source_reference) DO NOTHING"#,
    )
    .bind(tenant_id)
    .bind(owner_id.get())
    .bind(contract_id.get())
    .bind(period_from)
    .bind(period_until)
    .bind(facility_id.map(FacilityId::get))
    .bind(captured_at)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"WITH received AS (
             SELECT transaction.id,entry.facility_id,transaction.created
             FROM inventory_transactions transaction
             JOIN inventory_entries entry ON entry.tenant_id=transaction.tenant_id
               AND entry.inventory_owner_id=transaction.inventory_owner_id
               AND entry.transaction_id=transaction.id AND entry.quantity_delta>0
             WHERE transaction.tenant_id=$1 AND transaction.inventory_owner_id=$2
               AND transaction.transaction_type='receive'
               AND transaction.created>=$4 AND transaction.created<$5
               AND ($6::BIGINT IS NULL OR entry.facility_id=$6)
             GROUP BY transaction.id,entry.facility_id,transaction.created)
           INSERT INTO billable_events
             (tenant_id,inventory_owner_id,facility_id,contract_id,event_type,unit,quantity,
              source_type,source_reference,description,occurred_at,captured_at)
           SELECT $1,$2,facility_id,$3,'receipt_line','event',1,
                  'inventory_transaction',id::TEXT||':'||facility_id::TEXT,
                  'Inbound receipt transaction',created,$7 FROM received
           ON CONFLICT(tenant_id,contract_id,event_type,source_type,source_reference) DO NOTHING"#,
    )
    .bind(tenant_id)
    .bind(owner_id.get())
    .bind(contract_id.get())
    .bind(period_from)
    .bind(period_until)
    .bind(facility_id.map(FacilityId::get))
    .bind(captured_at)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"INSERT INTO billable_events
             (tenant_id,inventory_owner_id,facility_id,contract_id,event_type,unit,quantity,
              source_type,source_reference,description,occurred_at,captured_at)
           SELECT tenant_id,inventory_owner_id,facility_id,$3,'picked_unit','each',picked_qty,
                  'pick_confirmation',id::TEXT,'Picked units',confirmed_at,$7
           FROM pick_confirmations WHERE tenant_id=$1 AND inventory_owner_id=$2
             AND confirmed_at>=$4 AND confirmed_at<$5
             AND ($6::BIGINT IS NULL OR facility_id=$6)
           ON CONFLICT(tenant_id,contract_id,event_type,source_type,source_reference) DO NOTHING"#,
    )
    .bind(tenant_id)
    .bind(owner_id.get())
    .bind(contract_id.get())
    .bind(period_from)
    .bind(period_until)
    .bind(facility_id.map(FacilityId::get))
    .bind(captured_at)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"INSERT INTO billable_events
             (tenant_id,inventory_owner_id,facility_id,contract_id,event_type,unit,quantity,
              source_type,source_reference,description,occurred_at,captured_at)
           SELECT tenant_id,inventory_owner_id,facility_id,$3,'pick_line','event',1,
                  'pick_confirmation',id::TEXT,'Pick line confirmed',confirmed_at,$7
           FROM pick_confirmations WHERE tenant_id=$1 AND inventory_owner_id=$2
             AND confirmed_at>=$4 AND confirmed_at<$5
             AND ($6::BIGINT IS NULL OR facility_id=$6)
           ON CONFLICT(tenant_id,contract_id,event_type,source_type,source_reference) DO NOTHING"#,
    )
    .bind(tenant_id)
    .bind(owner_id.get())
    .bind(contract_id.get())
    .bind(period_from)
    .bind(period_until)
    .bind(facility_id.map(FacilityId::get))
    .bind(captured_at)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"INSERT INTO billable_events
             (tenant_id,inventory_owner_id,facility_id,contract_id,event_type,unit,quantity,
              source_type,source_reference,description,occurred_at,captured_at)
           SELECT tenant_id,inventory_owner_id,facility_id,$3,'packed_carton','carton',1,
                  'carton',id::TEXT,'Carton packed and closed',closed_at,$7
           FROM cartons WHERE tenant_id=$1 AND inventory_owner_id=$2 AND state='closed'
             AND closed_at>=$4 AND closed_at<$5
             AND ($6::BIGINT IS NULL OR facility_id=$6)
           ON CONFLICT(tenant_id,contract_id,event_type,source_type,source_reference) DO NOTHING"#,
    )
    .bind(tenant_id)
    .bind(owner_id.get())
    .bind(contract_id.get())
    .bind(period_from)
    .bind(period_until)
    .bind(facility_id.map(FacilityId::get))
    .bind(captured_at)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"INSERT INTO billable_events
             (tenant_id,inventory_owner_id,facility_id,contract_id,event_type,unit,quantity,
              source_type,source_reference,description,occurred_at,captured_at)
           SELECT tenant_id,inventory_owner_id,facility_id,$3,'shipped_unit','each',departed_qty,
                  'shipment',id::TEXT,'Units departed facility',departed_at,$7
           FROM shipments WHERE tenant_id=$1 AND inventory_owner_id=$2 AND state='departed'
             AND departed_at>=$4 AND departed_at<$5 AND departed_qty>0
             AND ($6::BIGINT IS NULL OR facility_id=$6)
           ON CONFLICT(tenant_id,contract_id,event_type,source_type,source_reference) DO NOTHING"#,
    )
    .bind(tenant_id)
    .bind(owner_id.get())
    .bind(contract_id.get())
    .bind(period_from)
    .bind(period_until)
    .bind(facility_id.map(FacilityId::get))
    .bind(captured_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn generate_run(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &GenerateBillingRunCommand,
) -> AppResult<BillingRunReadModel> {
    require_access_actor(access, context)?;
    if command.period_until <= command.period_from {
        return Err(AppError::bad_request(
            "billing period_until must be later than period_from",
        ));
    }
    let prepared = PreparedCommand::new_v1(context, GENERATE_BILLING_RUN_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        verify_run_replay(&scope, &result)?;
        tx.commit().await?;
        return Ok(result);
    }
    lock_contract_key_tx(&mut tx, access.tenant_id.get(), command.contract_id.get()).await?;
    let contract = read_contract_tx(&mut tx, access.tenant_id, command.contract_id).await?;
    require_record_scope(&scope, contract.inventory_owner_id, command.facility_id)?;
    if command.facility_id.is_none() && !scope.all_facilities {
        return Err(AppError::not_found("billing record"));
    }
    if let Some(facility_id) = command.facility_id {
        validate_owner_facility_tx(
            &mut tx,
            access.tenant_id.get(),
            contract.inventory_owner_id,
            facility_id,
        )
        .await?;
    }
    let now = now_iso();
    if command.period_until > now {
        return Err(AppError::conflict(
            "billing period cannot end in the future",
        ));
    }
    if contract.status != BillingContractStatus::Active
        || command.period_from < contract.effective_window.effective_from
        || contract
            .effective_window
            .effective_until
            .is_some_and(|until| command.period_until > until)
    {
        return Err(AppError::conflict(
            "billing period must be contained by an active contract window",
        ));
    }
    let latest = sqlx::query(
        r#"SELECT id,attempt,status FROM billing_reconciliation_runs
           WHERE tenant_id=$1 AND contract_id=$2 AND facility_id IS NOT DISTINCT FROM $3
             AND period_from=$4 AND period_until=$5
           ORDER BY attempt DESC LIMIT 1 FOR UPDATE"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.contract_id.get())
    .bind(command.facility_id.map(FacilityId::get))
    .bind(command.period_from)
    .bind(command.period_until)
    .fetch_optional(&mut *tx)
    .await?;
    let (attempt, supersedes_run_id) = if let Some(row) = latest {
        if row.try_get::<String, _>("status")? != "rejected" {
            return Err(AppError::conflict(
                "billing period already has a non-rejected reconciliation run",
            ));
        }
        let prior_attempt: i64 = row.try_get("attempt")?;
        (
            prior_attempt
                .checked_add(1)
                .ok_or_else(|| AppError::internal("billing run attempt overflow"))?,
            Some(row.try_get::<i64, _>("id")?),
        )
    } else {
        (1, None)
    };

    capture_operational_events_tx(
        &mut tx,
        OperationalEventWindow {
            tenant_id: access.tenant_id.get(),
            owner_id: contract.inventory_owner_id,
            contract_id: command.contract_id,
            facility_id: command.facility_id,
            period_from: command.period_from,
            period_until: command.period_until,
            captured_at: now,
        },
    )
    .await?;

    let stats = sqlx::query(
        r#"WITH eligible AS (
             SELECT event.*,rate.id AS rate_id,rate.rate_minor,rate.minimum_charge_minor
             FROM billable_events event
             LEFT JOIN LATERAL (
               SELECT candidate.id,candidate.rate_minor,candidate.minimum_charge_minor
               FROM billing_rate_versions candidate
               WHERE candidate.tenant_id=event.tenant_id
                 AND candidate.contract_id=event.contract_id
                 AND candidate.event_type=event.event_type AND candidate.unit=event.unit
                 AND candidate.currency=$7
                 AND candidate.effective_from<=event.occurred_at
                 AND (candidate.effective_until IS NULL OR candidate.effective_until>event.occurred_at)
               ORDER BY candidate.revision DESC,candidate.id DESC LIMIT 1
             ) rate ON true
             WHERE event.tenant_id=$1 AND event.contract_id=$2
               AND event.occurred_at>=$3 AND event.occurred_at<$4
               AND event.captured_at<=$6
               AND ($5::BIGINT IS NULL OR event.facility_id=$5)
               AND NOT EXISTS(
                 SELECT 1 FROM billing_charges prior_charge
                 JOIN billing_reconciliation_runs prior_run
                   ON prior_run.tenant_id=prior_charge.tenant_id
                  AND prior_run.id=prior_charge.reconciliation_run_id
                 WHERE prior_charge.tenant_id=event.tenant_id
                   AND prior_charge.billable_event_id=event.id
                   AND prior_run.status<>'rejected'))
           SELECT count(*)::BIGINT AS event_count,
                  count(rate_id)::BIGINT AS charge_count,
                  (count(*)-count(rate_id))::BIGINT AS unmatched_event_count,
                  COALESCE(sum(GREATEST(rate_minor::NUMERIC*quantity,
                                        minimum_charge_minor::NUMERIC)) FILTER
                           (WHERE rate_id IS NOT NULL),0)::TEXT AS total_minor
           FROM eligible"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.contract_id.get())
    .bind(command.period_from)
    .bind(command.period_until)
    .bind(command.facility_id.map(FacilityId::get))
    .bind(now)
    .bind(&contract.currency)
    .fetch_one(&mut *tx)
    .await?;
    let event_count: i64 = stats.try_get("event_count")?;
    let charge_count: i64 = stats.try_get("charge_count")?;
    let unmatched_event_count: i64 = stats.try_get("unmatched_event_count")?;
    let total_text: String = stats.try_get("total_minor")?;
    let total_u128 = total_text
        .parse::<u128>()
        .map_err(|error| AppError::internal(format!("invalid billing total: {error}")))?;
    let total_minor = i64::try_from(total_u128)
        .map_err(|_| AppError::conflict("billing total exceeds supported financial range"))?;
    let run_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO billing_reconciliation_runs
             (tenant_id,inventory_owner_id,contract_id,facility_id,attempt,supersedes_run_id,
              period_from,period_until,event_count,charge_count,unmatched_event_count,total_minor,
              currency,generated_by_user_id,generated_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15) RETURNING id"#,
    )
    .bind(access.tenant_id.get())
    .bind(contract.inventory_owner_id.get())
    .bind(command.contract_id.get())
    .bind(command.facility_id.map(FacilityId::get))
    .bind(attempt)
    .bind(supersedes_run_id)
    .bind(command.period_from)
    .bind(command.period_until)
    .bind(event_count)
    .bind(charge_count)
    .bind(unmatched_event_count)
    .bind(total_minor)
    .bind(&contract.currency)
    .bind(context.actor_id.get())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        r#"WITH eligible AS (
             SELECT event.*,rate.id AS rate_id,rate.rate_minor,rate.minimum_charge_minor
             FROM billable_events event
             JOIN LATERAL (
               SELECT candidate.id,candidate.rate_minor,candidate.minimum_charge_minor
               FROM billing_rate_versions candidate
               WHERE candidate.tenant_id=event.tenant_id
                 AND candidate.contract_id=event.contract_id
                 AND candidate.event_type=event.event_type AND candidate.unit=event.unit
                 AND candidate.currency=$8
                 AND candidate.effective_from<=event.occurred_at
                 AND (candidate.effective_until IS NULL OR candidate.effective_until>event.occurred_at)
               ORDER BY candidate.revision DESC,candidate.id DESC LIMIT 1
             ) rate ON true
             WHERE event.tenant_id=$1 AND event.contract_id=$2
               AND event.occurred_at>=$3 AND event.occurred_at<$4
               AND event.captured_at<=$6
               AND ($5::BIGINT IS NULL OR event.facility_id=$5)
               AND NOT EXISTS(
                 SELECT 1 FROM billing_charges prior_charge
                 JOIN billing_reconciliation_runs prior_run
                   ON prior_run.tenant_id=prior_charge.tenant_id
                  AND prior_run.id=prior_charge.reconciliation_run_id
                 WHERE prior_charge.tenant_id=event.tenant_id
                   AND prior_charge.billable_event_id=event.id
                   AND prior_run.status<>'rejected'))
           INSERT INTO billing_charges
             (tenant_id,inventory_owner_id,facility_id,contract_id,reconciliation_run_id,
              billable_event_id,rate_version_id,event_type,unit,quantity,rate_minor,
              minimum_charge_minor,gross_minor,amount_minor,currency,source_type,
              source_reference,occurred_at,created_at)
           SELECT tenant_id,inventory_owner_id,facility_id,contract_id,$7,id,rate_id,event_type,
                  unit,quantity,rate_minor,minimum_charge_minor,
                  (rate_minor::NUMERIC*quantity)::BIGINT,
                  GREATEST(rate_minor::NUMERIC*quantity,minimum_charge_minor::NUMERIC)::BIGINT,
                  $8,source_type,source_reference,occurred_at,$6 FROM eligible ORDER BY id"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.contract_id.get())
    .bind(command.period_from)
    .bind(command.period_until)
    .bind(command.facility_id.map(FacilityId::get))
    .bind(now)
    .bind(run_id)
    .bind(&contract.currency)
    .execute(&mut *tx)
    .await?;
    let result = read_run_tx(
        &mut tx,
        access.tenant_id,
        BillingReconciliationRunId::new(run_id).map_err(internal)?,
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        BillingOutboxEvent {
            tenant_id: access.tenant_id,
            actor_id: context.actor_id,
            owner_id: result.inventory_owner_id,
            facility_id: result.facility_id,
            aggregate_type: "reconciliation_run",
            aggregate_id: run_id,
            transition: "generated",
            occurred_at: now,
        },
        &result,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}
