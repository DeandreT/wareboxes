//! Tenant-scoped value-added work backed by inventory holds and the journal.

use serde::Serialize;
use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::value_added_work::{
    CreateValueAddedWorkCommand, ValueAddedWorkEventReadModel, ValueAddedWorkFilter,
    ValueAddedWorkInputReadModel, ValueAddedWorkLifecycleCommand, ValueAddedWorkOutputReadModel,
    ValueAddedWorkPage, ValueAddedWorkReadModel, CANCEL_VALUE_ADDED_WORK_OPERATION,
    COMPLETE_VALUE_ADDED_WORK_OPERATION, CREATE_VALUE_ADDED_WORK_OPERATION,
    RELEASE_VALUE_ADDED_WORK_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{
    InventoryHoldReason, InventoryStatus, InventoryTransactionType, TenantAccess,
};
use wareboxes_domain::{
    validate_value_added_quantities, validate_value_added_shape, BillableEventId, FacilityId,
    InventoryBalanceId, InventoryHoldId, InventoryOwnerId, ItemBatchId, LicensePlateId, LocationId,
    TenantId, Timestamp, UserId, ValueAddedInventoryStatus, ValueAddedQuantity, ValueAddedRevision,
    ValueAddedWorkEventId, ValueAddedWorkId, ValueAddedWorkInputId, ValueAddedWorkKind,
    ValueAddedWorkOutputId, ValueAddedWorkStatus,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{
    current_scope_tx, lock_current_scope_tx, require_permission_tx, ScopeBindings,
};
use crate::repo::inventory_hold::{
    place_composed_inventory_hold_tx, release_composed_inventory_hold_tx, PlaceInventoryHoldCommand,
};
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry};
use crate::repo::inventory_locking::lock_license_plates;
use crate::repo::orders::next_outbox_sequence_tx;

const PERMISSION: &str = "wms";
const HOLD_REFERENCE_TYPE: &str = "value_added_work_order";

#[derive(Debug, Clone)]
struct LockedInput {
    input_id: i64,
    inventory_balance_id: i64,
    quantity: i64,
    hold_id: Option<i64>,
    location_id: i64,
    license_plate_id: Option<i64>,
    item_batch_id: i64,
    status: InventoryStatus,
    qty_on_hand: i64,
    qty_reserved: i64,
    qty_held: i64,
    active: bool,
}

#[derive(Debug, Clone)]
struct StoredOutput {
    location_id: i64,
    license_plate_id: Option<i64>,
    item_batch_id: i64,
    item_id: i64,
    uom: String,
    status: InventoryStatus,
    quantity: i64,
}

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}

fn require_actor(access: &TenantAccess, context: &CommandContext) -> AppResult<()> {
    context.require_actor(access.tenant_id, access.user_id)?;
    Ok(())
}

fn require_scope(
    scope: &ScopeBindings,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
) -> AppResult<()> {
    if scope.includes_inventory_owner(inventory_owner_id.get())
        && scope.includes_facility(facility_id.get())
    {
        Ok(())
    } else {
        Err(AppError::not_found("value-added work"))
    }
}

fn parse_kind(value: &str) -> AppResult<ValueAddedWorkKind> {
    ValueAddedWorkKind::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored value-added work kind"))
}

fn parse_status(value: &str) -> AppResult<ValueAddedWorkStatus> {
    ValueAddedWorkStatus::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored value-added work status"))
}

fn parse_inventory_status(value: &str) -> AppResult<InventoryStatus> {
    InventoryStatus::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored value-added inventory status"))
}

fn parse_value_added_inventory_status(value: &str) -> AppResult<ValueAddedInventoryStatus> {
    ValueAddedInventoryStatus::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored value-added inventory status"))
}

fn transaction_type(kind: ValueAddedWorkKind) -> InventoryTransactionType {
    match kind {
        ValueAddedWorkKind::Relabel => InventoryTransactionType::Relabel,
        ValueAddedWorkKind::Refurbishment => InventoryTransactionType::Refurbishment,
        ValueAddedWorkKind::Kit => InventoryTransactionType::Kit,
        ValueAddedWorkKind::Dekit => InventoryTransactionType::Dekit,
        ValueAddedWorkKind::Assembly => InventoryTransactionType::Assembly,
        ValueAddedWorkKind::ValueAddedService => InventoryTransactionType::ValueAddedService,
    }
}

fn billable_event_type(kind: ValueAddedWorkKind) -> &'static str {
    match kind {
        ValueAddedWorkKind::Relabel => "relabel_unit",
        ValueAddedWorkKind::Refurbishment => "refurbishment_unit",
        ValueAddedWorkKind::Kit | ValueAddedWorkKind::Dekit => "kit_unit",
        ValueAddedWorkKind::Assembly => "assembly_unit",
        ValueAddedWorkKind::ValueAddedService => "value_added_service_unit",
    }
}

async fn lock_work_key_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    work_id: ValueAddedWorkId,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "value-added-work:{}:{}",
            tenant_id.get(),
            work_id.get()
        ))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn work_scope_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    work_id: ValueAddedWorkId,
) -> AppResult<(InventoryOwnerId, FacilityId)> {
    let row = sqlx::query(
        "SELECT inventory_owner_id,facility_id FROM value_added_work_orders WHERE tenant_id=$1 AND id=$2",
    )
    .bind(tenant_id.get())
    .bind(work_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("value-added work"))?;
    Ok((
        InventoryOwnerId::new(row.try_get("inventory_owner_id")?).map_err(internal)?,
        FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
    ))
}

async fn read_work_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    work_id: ValueAddedWorkId,
) -> AppResult<ValueAddedWorkReadModel> {
    let row = sqlx::query(
        r#"SELECT work.*,owner.name AS inventory_owner_name,facility.name AS facility_name
           FROM value_added_work_orders work
           JOIN inventory_owners owner ON owner.tenant_id=work.tenant_id
             AND owner.id=work.inventory_owner_id
           JOIN facilities facility ON facility.tenant_id=work.tenant_id
             AND facility.id=work.facility_id
           WHERE work.tenant_id=$1 AND work.id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(work_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("value-added work"))?;

    let input_rows = sqlx::query(
        r#"SELECT input.*,balance.location_id,balance.license_plate_id,balance.item_batch_id,
                  balance.item_id,balance.uom,balance.status AS inventory_status,
                  COALESCE(location.barcode,location.name,'Location #'||location.id::TEXT)
                    AS location_code,plate.barcode AS license_plate_number,
                  item.description AS item_description,batch.lot,batch.serial
           FROM value_added_work_inputs input
           JOIN inventory_balances balance ON balance.tenant_id=input.tenant_id
             AND balance.inventory_owner_id=input.inventory_owner_id
             AND balance.id=input.inventory_balance_id
           JOIN locations location ON location.tenant_id=balance.tenant_id
             AND location.facility_id=balance.facility_id AND location.id=balance.location_id
           JOIN item_batches batch ON batch.tenant_id=balance.tenant_id
             AND batch.inventory_owner_id=balance.inventory_owner_id
             AND batch.id=balance.item_batch_id
           JOIN items item ON item.tenant_id=batch.tenant_id AND item.id=batch.item_id
           LEFT JOIN license_plates plate ON plate.tenant_id=balance.tenant_id
             AND plate.inventory_owner_id=balance.inventory_owner_id
             AND plate.facility_id=balance.facility_id AND plate.id=balance.license_plate_id
           WHERE input.tenant_id=$1 AND input.work_id=$2 ORDER BY input.id"#,
    )
    .bind(tenant_id.get())
    .bind(work_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let inputs = input_rows
        .iter()
        .map(|input| {
            Ok(ValueAddedWorkInputReadModel {
                input_id: ValueAddedWorkInputId::new(input.try_get("id")?).map_err(internal)?,
                inventory_balance_id: InventoryBalanceId::new(
                    input.try_get("inventory_balance_id")?,
                )
                .map_err(internal)?,
                location_id: LocationId::new(input.try_get("location_id")?).map_err(internal)?,
                location_code: input.try_get("location_code")?,
                license_plate_id: input
                    .try_get::<Option<i64>, _>("license_plate_id")?
                    .map(LicensePlateId::new)
                    .transpose()
                    .map_err(internal)?,
                license_plate_number: input.try_get("license_plate_number")?,
                item_batch_id: ItemBatchId::new(input.try_get("item_batch_id")?)
                    .map_err(internal)?,
                item_id: input.try_get("item_id")?,
                item_description: input.try_get("item_description")?,
                uom: input.try_get("uom")?,
                lot: input.try_get("lot")?,
                serial: input.try_get("serial")?,
                inventory_status: parse_value_added_inventory_status(
                    &input.try_get::<String, _>("inventory_status")?,
                )?,
                quantity: ValueAddedQuantity::new(input.try_get("quantity")?).map_err(internal)?,
                hold_id: input
                    .try_get::<Option<i64>, _>("inventory_hold_id")?
                    .map(InventoryHoldId::new)
                    .transpose()
                    .map_err(internal)?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;

    let output_rows = sqlx::query(
        r#"SELECT output.*,batch.item_id,batch.uom,batch.lot,batch.serial,
                  COALESCE(location.barcode,location.name,'Location #'||location.id::TEXT)
                    AS location_code,plate.barcode AS license_plate_number,
                  item.description AS item_description
           FROM value_added_work_outputs output
           JOIN locations location ON location.tenant_id=output.tenant_id
             AND location.facility_id=output.facility_id AND location.id=output.location_id
           JOIN item_batches batch ON batch.tenant_id=output.tenant_id
             AND batch.inventory_owner_id=output.inventory_owner_id
             AND batch.id=output.item_batch_id
           JOIN items item ON item.tenant_id=batch.tenant_id AND item.id=batch.item_id
           LEFT JOIN license_plates plate ON plate.tenant_id=output.tenant_id
             AND plate.inventory_owner_id=output.inventory_owner_id
             AND plate.facility_id=output.facility_id AND plate.id=output.license_plate_id
           WHERE output.tenant_id=$1 AND output.work_id=$2 ORDER BY output.id"#,
    )
    .bind(tenant_id.get())
    .bind(work_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let outputs = output_rows
        .iter()
        .map(|output| {
            Ok(ValueAddedWorkOutputReadModel {
                output_id: ValueAddedWorkOutputId::new(output.try_get("id")?).map_err(internal)?,
                location_id: LocationId::new(output.try_get("location_id")?).map_err(internal)?,
                location_code: output.try_get("location_code")?,
                license_plate_id: output
                    .try_get::<Option<i64>, _>("license_plate_id")?
                    .map(LicensePlateId::new)
                    .transpose()
                    .map_err(internal)?,
                license_plate_number: output.try_get("license_plate_number")?,
                item_batch_id: ItemBatchId::new(output.try_get("item_batch_id")?)
                    .map_err(internal)?,
                item_id: output.try_get("item_id")?,
                item_description: output.try_get("item_description")?,
                uom: output.try_get("uom")?,
                lot: output.try_get("lot")?,
                serial: output.try_get("serial")?,
                inventory_status: parse_value_added_inventory_status(
                    &output.try_get::<String, _>("inventory_status")?,
                )?,
                quantity: ValueAddedQuantity::new(output.try_get("quantity")?).map_err(internal)?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;

    let event_rows = sqlx::query(
        "SELECT * FROM value_added_work_events WHERE tenant_id=$1 AND work_id=$2 ORDER BY resulting_revision",
    )
    .bind(tenant_id.get())
    .bind(work_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let events = event_rows
        .iter()
        .map(|event| {
            Ok(ValueAddedWorkEventReadModel {
                event_id: ValueAddedWorkEventId::new(event.try_get("id")?).map_err(internal)?,
                from_status: event
                    .try_get::<Option<String>, _>("from_status")?
                    .map(|value| parse_status(&value))
                    .transpose()?,
                to_status: parse_status(&event.try_get::<String, _>("to_status")?)?,
                note: event.try_get("note")?,
                resulting_revision: ValueAddedRevision::new(event.try_get("resulting_revision")?)
                    .map_err(internal)?,
                actor_id: UserId::new(event.try_get("actor_user_id")?).map_err(internal)?,
                occurred_at: event.try_get("occurred_at")?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;

    Ok(ValueAddedWorkReadModel {
        work_id,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(internal)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        facility_name: row.try_get("facility_name")?,
        number: row.try_get("work_number")?,
        kind: parse_kind(&row.try_get::<String, _>("kind")?)?,
        status: parse_status(&row.try_get::<String, _>("status")?)?,
        revision: ValueAddedRevision::new(row.try_get("revision")?).map_err(internal)?,
        note: row.try_get("note")?,
        inputs,
        outputs,
        completion_inventory_transaction_id: row.try_get("completion_inventory_transaction_id")?,
        billable_event_id: row
            .try_get::<Option<i64>, _>("billable_event_id")?
            .map(BillableEventId::new)
            .transpose()
            .map_err(internal)?,
        created_by: UserId::new(row.try_get("created_by_user_id")?).map_err(internal)?,
        created_at: row.try_get("created_at")?,
        released_by: row
            .try_get::<Option<i64>, _>("released_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(internal)?,
        released_at: row.try_get("released_at")?,
        completed_by: row
            .try_get::<Option<i64>, _>("completed_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(internal)?,
        completed_at: row.try_get("completed_at")?,
        cancelled_by: row
            .try_get::<Option<i64>, _>("cancelled_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(internal)?,
        cancelled_at: row.try_get("cancelled_at")?,
        events,
    })
}

async fn insert_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    work: &ValueAddedWorkReadModel,
    from_status: Option<ValueAddedWorkStatus>,
    note: Option<&str>,
    actor_id: UserId,
    occurred_at: Timestamp,
) -> AppResult<()> {
    sqlx::query(
        r#"INSERT INTO value_added_work_events
           (tenant_id,inventory_owner_id,facility_id,work_id,from_status,to_status,note,
            resulting_revision,actor_user_id,occurred_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)"#,
    )
    .bind(tenant_id.get())
    .bind(work.inventory_owner_id.get())
    .bind(work.facility_id.get())
    .bind(work.work_id.get())
    .bind(from_status.map(ValueAddedWorkStatus::as_str))
    .bind(work.status.as_str())
    .bind(note)
    .bind(work.revision.get())
    .bind(actor_id.get())
    .bind(occurred_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn enqueue_event_tx<T: Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_id: UserId,
    work: &ValueAddedWorkReadModel,
    transition: &str,
    payload: &T,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let ordering_key = format!("value-added-work:{}", work.work_id.get());
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let event_key = format!("{ordering_key}:{transition}:{sequence}");
    let event_type = format!("value_added_work.{transition}");
    let aggregate_id = work.work_id.get().to_string();
    let payload = serde_json::to_value(payload).map_err(internal)?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(work.inventory_owner_id),
            facility_id: Some(work.facility_id),
            actor_user_id: Some(actor_id.get()),
            event_key: &event_key,
            aggregate_type: "value_added_work_order",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: &event_type,
            schema_version: 1,
            payload: &payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}

async fn validate_recipe_references_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command: &CreateValueAddedWorkCommand,
) -> AppResult<()> {
    let mut input_ids = command
        .inputs
        .iter()
        .map(|input| input.inventory_balance_id.get())
        .collect::<Vec<_>>();
    input_ids.sort_unstable();
    let valid_input_ids = sqlx::query_scalar::<_, i64>(
        r#"SELECT balance.id FROM inventory_balances balance
           WHERE balance.tenant_id=$1 AND balance.inventory_owner_id=$2
             AND balance.facility_id=$3 AND balance.id=ANY($4)
             AND balance.deleted IS NULL ORDER BY balance.id FOR SHARE"#,
    )
    .bind(tenant_id.get())
    .bind(command.inventory_owner_id.get())
    .bind(command.facility_id.get())
    .bind(&input_ids)
    .fetch_all(&mut **tx)
    .await?;
    if valid_input_ids != input_ids {
        return Err(AppError::not_found("input inventory balance"));
    }

    for output in &command.outputs {
        let valid = sqlx::query_scalar::<_, bool>(
            r#"SELECT EXISTS(
              SELECT 1 FROM locations location
              JOIN item_batches batch ON batch.tenant_id=location.tenant_id
                AND batch.inventory_owner_id=$2 AND batch.id=$5 AND batch.deleted IS NULL
              LEFT JOIN license_plates plate ON plate.tenant_id=location.tenant_id
                AND plate.inventory_owner_id=$2 AND plate.facility_id=location.facility_id
                AND plate.id=$4 AND plate.deleted IS NULL
              WHERE location.tenant_id=$1 AND location.facility_id=$3 AND location.id=$6
                AND location.deleted IS NULL AND ($4::BIGINT IS NULL OR plate.id IS NOT NULL))"#,
        )
        .bind(tenant_id.get())
        .bind(command.inventory_owner_id.get())
        .bind(command.facility_id.get())
        .bind(output.license_plate_id.map(LicensePlateId::get))
        .bind(output.item_batch_id.get())
        .bind(output.location_id.get())
        .fetch_one(&mut **tx)
        .await?;
        if !valid {
            return Err(AppError::not_found("value-added output identity"));
        }
    }
    Ok(())
}

pub async fn create(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CreateValueAddedWorkCommand,
) -> AppResult<ValueAddedWorkReadModel> {
    require_actor(access, context)?;
    let input_ids = command
        .inputs
        .iter()
        .map(|input| input.inventory_balance_id.get())
        .collect::<Vec<_>>();
    validate_value_added_shape(command.kind, &input_ids, command.outputs.len())
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let input_total = command.inputs.iter().try_fold(0_i64, |total, input| {
        total
            .checked_add(input.quantity.get())
            .ok_or_else(|| AppError::bad_request("value-added input quantity exceeds i64"))
    })?;
    let output_total = command.outputs.iter().try_fold(0_i64, |total, output| {
        total
            .checked_add(output.quantity.get())
            .ok_or_else(|| AppError::bad_request("value-added output quantity exceeds i64"))
    })?;
    validate_value_added_quantities(command.kind, input_total, output_total)
        .map_err(|error| AppError::bad_request(error.to_string()))?;

    let prepared = PreparedCommand::new_v1(context, CREATE_VALUE_ADDED_WORK_OPERATION, command)?;
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
        .replayed::<ValueAddedWorkReadModel>(&mut tx)
        .await?
    {
        require_scope(&scope, result.inventory_owner_id, result.facility_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "value-added-number:{}:{}:{}:{}",
            access.tenant_id.get(),
            command.inventory_owner_id.get(),
            command.facility_id.get(),
            command.number.as_str()
        ))
        .execute(&mut *tx)
        .await?;
    let assignment = sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS(SELECT 1 FROM inventory_owner_facilities assignment
           JOIN inventory_owners owner ON owner.tenant_id=assignment.tenant_id
             AND owner.id=assignment.inventory_owner_id AND owner.deleted IS NULL
           JOIN facilities facility ON facility.tenant_id=assignment.tenant_id
             AND facility.id=assignment.facility_id AND facility.deleted IS NULL
           WHERE assignment.tenant_id=$1 AND assignment.inventory_owner_id=$2
             AND assignment.facility_id=$3 AND assignment.deleted IS NULL)"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.inventory_owner_id.get())
    .bind(command.facility_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if !assignment {
        return Err(AppError::not_found("owner-facility assignment"));
    }
    let duplicate = sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS(SELECT 1 FROM value_added_work_orders WHERE tenant_id=$1
           AND inventory_owner_id=$2 AND facility_id=$3 AND work_number=$4)"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.inventory_owner_id.get())
    .bind(command.facility_id.get())
    .bind(command.number.as_str())
    .fetch_one(&mut *tx)
    .await?;
    if duplicate {
        return Err(AppError::conflict(
            "value-added work number already exists in this owner and facility",
        ));
    }
    validate_recipe_references_tx(&mut tx, access.tenant_id, command).await?;
    let now = now_iso();
    let work_id = ValueAddedWorkId::new(
        sqlx::query_scalar::<_, i64>(
            r#"INSERT INTO value_added_work_orders
               (tenant_id,inventory_owner_id,facility_id,work_number,kind,note,
                created_by_user_id,created_at)
               VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.inventory_owner_id.get())
        .bind(command.facility_id.get())
        .bind(command.number.as_str())
        .bind(command.kind.as_str())
        .bind(command.note.as_ref().map(|note| note.as_str()))
        .bind(context.actor_id.get())
        .bind(now)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(internal)?;
    for input in &command.inputs {
        sqlx::query(
            r#"INSERT INTO value_added_work_inputs
               (tenant_id,inventory_owner_id,facility_id,work_id,inventory_balance_id,quantity)
               VALUES($1,$2,$3,$4,$5,$6)"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.inventory_owner_id.get())
        .bind(command.facility_id.get())
        .bind(work_id.get())
        .bind(input.inventory_balance_id.get())
        .bind(input.quantity.get())
        .execute(&mut *tx)
        .await?;
    }
    for output in &command.outputs {
        sqlx::query(
            r#"INSERT INTO value_added_work_outputs
               (tenant_id,inventory_owner_id,facility_id,work_id,location_id,license_plate_id,
                item_batch_id,inventory_status,quantity)
               VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.inventory_owner_id.get())
        .bind(command.facility_id.get())
        .bind(work_id.get())
        .bind(output.location_id.get())
        .bind(output.license_plate_id.map(LicensePlateId::get))
        .bind(output.item_batch_id.get())
        .bind(output.inventory_status.as_str())
        .bind(output.quantity.get())
        .execute(&mut *tx)
        .await?;
    }
    let result = read_work_tx(&mut tx, access.tenant_id, work_id).await?;
    insert_event_tx(
        &mut tx,
        access.tenant_id,
        &result,
        None,
        command.note.as_ref().map(|note| note.as_str()),
        context.actor_id,
        now,
    )
    .await?;
    let result = read_work_tx(&mut tx, access.tenant_id, work_id).await?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        &result,
        "created",
        &result,
        now,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

async fn lock_work_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    work_id: ValueAddedWorkId,
) -> AppResult<ValueAddedWorkReadModel> {
    lock_work_key_tx(tx, tenant_id, work_id).await?;
    sqlx::query("SELECT id FROM value_added_work_orders WHERE tenant_id=$1 AND id=$2 FOR UPDATE")
        .bind(tenant_id.get())
        .bind(work_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("value-added work"))?;
    read_work_tx(tx, tenant_id, work_id).await
}

async fn lock_inputs_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    work_id: ValueAddedWorkId,
) -> AppResult<Vec<LockedInput>> {
    let rows = sqlx::query(
        r#"SELECT input.id AS input_id,input.inventory_balance_id,input.quantity,
                  input.inventory_hold_id,balance.location_id,balance.license_plate_id,
                  balance.item_batch_id,balance.status,balance.qty_on_hand,
                  balance.qty_reserved,balance.qty_held,balance.deleted IS NULL AS active
           FROM value_added_work_inputs input
           JOIN inventory_balances balance ON balance.tenant_id=input.tenant_id
             AND balance.inventory_owner_id=input.inventory_owner_id
             AND balance.id=input.inventory_balance_id
           WHERE input.tenant_id=$1 AND input.work_id=$2
           ORDER BY input.inventory_balance_id FOR UPDATE OF input,balance"#,
    )
    .bind(tenant_id.get())
    .bind(work_id.get())
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(LockedInput {
                input_id: row.try_get("input_id")?,
                inventory_balance_id: row.try_get("inventory_balance_id")?,
                quantity: row.try_get("quantity")?,
                hold_id: row.try_get("inventory_hold_id")?,
                location_id: row.try_get("location_id")?,
                license_plate_id: row.try_get("license_plate_id")?,
                item_batch_id: row.try_get("item_batch_id")?,
                status: parse_inventory_status(&row.try_get::<String, _>("status")?)?,
                qty_on_hand: row.try_get("qty_on_hand")?,
                qty_reserved: row.try_get("qty_reserved")?,
                qty_held: row.try_get("qty_held")?,
                active: row.try_get("active")?,
            })
        })
        .collect()
}

async fn stored_outputs_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    work_id: ValueAddedWorkId,
) -> AppResult<Vec<StoredOutput>> {
    let rows = sqlx::query(
        r#"SELECT output.id AS output_id,output.location_id,output.license_plate_id,
                  output.item_batch_id,batch.item_id,batch.uom,output.inventory_status,
                  output.quantity
           FROM value_added_work_outputs output
           JOIN item_batches batch ON batch.tenant_id=output.tenant_id
             AND batch.inventory_owner_id=output.inventory_owner_id
             AND batch.id=output.item_batch_id AND batch.deleted IS NULL
           WHERE output.tenant_id=$1 AND output.work_id=$2
           ORDER BY output.location_id,output.license_plate_id NULLS FIRST,
                    output.item_batch_id,output.inventory_status,output.id
           FOR SHARE OF batch"#,
    )
    .bind(tenant_id.get())
    .bind(work_id.get())
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(StoredOutput {
                location_id: row.try_get("location_id")?,
                license_plate_id: row.try_get("license_plate_id")?,
                item_batch_id: row.try_get("item_batch_id")?,
                item_id: row.try_get("item_id")?,
                uom: row.try_get("uom")?,
                status: parse_inventory_status(&row.try_get::<String, _>("inventory_status")?)?,
                quantity: row.try_get("quantity")?,
            })
        })
        .collect()
}

struct LifecycleEffects {
    inventory_transaction_id: Option<i64>,
    billable_event_id: Option<i64>,
}

async fn update_lifecycle_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    work: &ValueAddedWorkReadModel,
    target: ValueAddedWorkStatus,
    actor_id: UserId,
    occurred_at: Timestamp,
    effects: LifecycleEffects,
) -> AppResult<()> {
    let revision = work.revision.next().map_err(internal)?;
    let updated = match target {
        ValueAddedWorkStatus::Released => {
            sqlx::query(
                r#"UPDATE value_added_work_orders SET status='released',revision=$1,
               released_by_user_id=$2,released_at=$3 WHERE tenant_id=$4 AND id=$5
               AND status='draft' AND revision=$6"#,
            )
            .bind(revision.get())
            .bind(actor_id.get())
            .bind(occurred_at)
            .bind(tenant_id.get())
            .bind(work.work_id.get())
            .bind(work.revision.get())
            .execute(&mut **tx)
            .await?
        }
        ValueAddedWorkStatus::Completed => {
            sqlx::query(
                r#"UPDATE value_added_work_orders SET status='completed',revision=$1,
               completed_by_user_id=$2,completed_at=$3,
               completion_inventory_transaction_id=$4,billable_event_id=$5
               WHERE tenant_id=$6 AND id=$7 AND status='released' AND revision=$8"#,
            )
            .bind(revision.get())
            .bind(actor_id.get())
            .bind(occurred_at)
            .bind(effects.inventory_transaction_id)
            .bind(effects.billable_event_id)
            .bind(tenant_id.get())
            .bind(work.work_id.get())
            .bind(work.revision.get())
            .execute(&mut **tx)
            .await?
        }
        ValueAddedWorkStatus::Cancelled => {
            sqlx::query(
                r#"UPDATE value_added_work_orders SET status='cancelled',revision=$1,
               cancelled_by_user_id=$2,cancelled_at=$3 WHERE tenant_id=$4 AND id=$5
               AND status=$6 AND revision=$7"#,
            )
            .bind(revision.get())
            .bind(actor_id.get())
            .bind(occurred_at)
            .bind(tenant_id.get())
            .bind(work.work_id.get())
            .bind(work.status.as_str())
            .bind(work.revision.get())
            .execute(&mut **tx)
            .await?
        }
        ValueAddedWorkStatus::Draft => {
            return Err(AppError::internal(
                "value-added work cannot transition back to draft",
            ));
        }
    };
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "value-added work changed during lifecycle command",
        ));
    }
    Ok(())
}

async fn lifecycle_context_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ValueAddedWorkLifecycleCommand,
) -> AppResult<(ScopeBindings, ValueAddedWorkReadModel)> {
    let scope = lock_current_scope_tx(tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(tx, access.tenant_id, context.actor_id.get(), PERMISSION).await?;
    let work = lock_work_tx(tx, access.tenant_id, command.work_id).await?;
    require_scope(&scope, work.inventory_owner_id, work.facility_id)?;
    Ok((scope, work))
}

pub async fn release(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ValueAddedWorkLifecycleCommand,
) -> AppResult<ValueAddedWorkReadModel> {
    require_actor(access, context)?;
    let prepared = PreparedCommand::new_v1(context, RELEASE_VALUE_ADDED_WORK_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let (scope, work) = lifecycle_context_tx(&mut tx, access, context, command).await?;
    if let Some(result) = prepared
        .replayed::<ValueAddedWorkReadModel>(&mut tx)
        .await?
    {
        require_scope(&scope, result.inventory_owner_id, result.facility_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    if work.revision != command.expected_revision {
        return Err(AppError::conflict(
            "value-added work changed; refresh before releasing it",
        ));
    }
    work.status
        .require_transition_to(ValueAddedWorkStatus::Released)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let input_hints = sqlx::query(
        r#"SELECT input.inventory_balance_id,balance.license_plate_id
           FROM value_added_work_inputs input
           JOIN inventory_balances balance ON balance.tenant_id=input.tenant_id
             AND balance.inventory_owner_id=input.inventory_owner_id
             AND balance.id=input.inventory_balance_id
           WHERE input.tenant_id=$1 AND input.work_id=$2 ORDER BY input.inventory_balance_id"#,
    )
    .bind(access.tenant_id.get())
    .bind(work.work_id.get())
    .fetch_all(&mut *tx)
    .await?;
    let plate_ids = input_hints
        .iter()
        .filter_map(|row| {
            row.try_get::<Option<i64>, _>("license_plate_id")
                .transpose()
        })
        .collect::<Result<Vec<_>, _>>()?;
    lock_license_plates(&mut tx, access.tenant_id, plate_ids).await?;
    let inputs = lock_inputs_tx(&mut tx, access.tenant_id, work.work_id).await?;
    if inputs
        .iter()
        .any(|input| !input.active || input.hold_id.is_some())
    {
        return Err(AppError::conflict(
            "value-added input is no longer available for release",
        ));
    }
    let now = now_iso();
    for input in &inputs {
        let hold_id = place_composed_inventory_hold_tx(
            &mut tx,
            access.tenant_id,
            context.actor_id.get(),
            now,
            &PlaceInventoryHoldCommand {
                inventory_balance_id: input.inventory_balance_id,
                qty: input.quantity,
                reason: InventoryHoldReason::Other,
                note: Some(command.note.as_str()),
                reference_type: Some(HOLD_REFERENCE_TYPE),
                reference_id: Some(work.work_id.get()),
            },
        )
        .await?;
        let updated = sqlx::query(
            r#"UPDATE value_added_work_inputs SET inventory_hold_id=$1
               WHERE tenant_id=$2 AND id=$3 AND inventory_hold_id IS NULL"#,
        )
        .bind(hold_id)
        .bind(access.tenant_id.get())
        .bind(input.input_id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(AppError::conflict(
                "value-added input changed while reserving inventory",
            ));
        }
    }
    update_lifecycle_tx(
        &mut tx,
        access.tenant_id,
        &work,
        ValueAddedWorkStatus::Released,
        context.actor_id,
        now,
        LifecycleEffects {
            inventory_transaction_id: None,
            billable_event_id: None,
        },
    )
    .await?;
    let result = read_work_tx(&mut tx, access.tenant_id, work.work_id).await?;
    insert_event_tx(
        &mut tx,
        access.tenant_id,
        &result,
        Some(work.status),
        Some(command.note.as_str()),
        context.actor_id,
        now,
    )
    .await?;
    let result = read_work_tx(&mut tx, access.tenant_id, work.work_id).await?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        &result,
        "released",
        &result,
        now,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

async fn decrement_input_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    input: &LockedInput,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"UPDATE inventory_balances SET qty_on_hand=qty_on_hand-$1,modified=$2
           WHERE tenant_id=$3 AND id=$4 AND deleted IS NULL AND status=$5
             AND qty_on_hand=$6 AND qty_reserved=$7 AND qty_held=$8
             AND qty_on_hand-qty_reserved-qty_held >= $1"#,
    )
    .bind(input.quantity)
    .bind(occurred_at)
    .bind(tenant_id.get())
    .bind(input.inventory_balance_id)
    .bind(input.status.as_str())
    .bind(input.qty_on_hand)
    .bind(input.qty_reserved)
    .bind(input.qty_held - input.quantity)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "value-added input inventory changed during completion",
        ));
    }
    Ok(())
}

async fn increment_output_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    output: &StoredOutput,
    occurred_at: Timestamp,
) -> AppResult<i64> {
    let id = if output.license_plate_id.is_some() {
        sqlx::query_scalar(
            r#"INSERT INTO inventory_balances
               (tenant_id,inventory_owner_id,created,modified,facility_id,location_id,
                license_plate_id,item_batch_id,item_id,uom,status,qty_on_hand,qty_reserved,qty_held)
               VALUES($1,$2,$3,$3,$4,$5,$6,$7,$8,$9,$10,$11,0,0)
               ON CONFLICT(tenant_id,inventory_owner_id,location_id,license_plate_id,
                 item_batch_id,uom,status) WHERE license_plate_id IS NOT NULL DO UPDATE
               SET qty_on_hand=inventory_balances.qty_on_hand+excluded.qty_on_hand,
                   modified=excluded.modified,deleted=NULL RETURNING id"#,
        )
        .bind(tenant_id.get())
        .bind(owner_id.get())
        .bind(occurred_at)
        .bind(facility_id.get())
        .bind(output.location_id)
        .bind(output.license_plate_id)
        .bind(output.item_batch_id)
        .bind(output.item_id)
        .bind(&output.uom)
        .bind(output.status.as_str())
        .bind(output.quantity)
        .fetch_one(&mut **tx)
        .await?
    } else {
        sqlx::query_scalar(
            r#"INSERT INTO inventory_balances
               (tenant_id,inventory_owner_id,created,modified,facility_id,location_id,
                license_plate_id,item_batch_id,item_id,uom,status,qty_on_hand,qty_reserved,qty_held)
               VALUES($1,$2,$3,$3,$4,$5,NULL,$6,$7,$8,$9,$10,0,0)
               ON CONFLICT(tenant_id,inventory_owner_id,location_id,item_batch_id,uom,status)
                 WHERE license_plate_id IS NULL DO UPDATE
               SET qty_on_hand=inventory_balances.qty_on_hand+excluded.qty_on_hand,
                   modified=excluded.modified,deleted=NULL RETURNING id"#,
        )
        .bind(tenant_id.get())
        .bind(owner_id.get())
        .bind(occurred_at)
        .bind(facility_id.get())
        .bind(output.location_id)
        .bind(output.item_batch_id)
        .bind(output.item_id)
        .bind(&output.uom)
        .bind(output.status.as_str())
        .bind(output.quantity)
        .fetch_one(&mut **tx)
        .await?
    };
    Ok(id)
}

async fn capture_billable_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_id: UserId,
    work: &ValueAddedWorkReadModel,
    inputs: &[LockedInput],
    outputs: &[StoredOutput],
    occurred_at: Timestamp,
) -> AppResult<Option<i64>> {
    let contract_id = sqlx::query_scalar::<_, i64>(
        r#"SELECT id FROM billing_contracts WHERE tenant_id=$1 AND inventory_owner_id=$2
           AND status='active' AND effective_from<=$3
           AND (effective_until IS NULL OR effective_until>$3)
           ORDER BY effective_from DESC,id DESC LIMIT 1 FOR SHARE"#,
    )
    .bind(tenant_id.get())
    .bind(work.inventory_owner_id.get())
    .bind(occurred_at)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(contract_id) = contract_id else {
        return Ok(None);
    };
    if work.kind != ValueAddedWorkKind::Dekit {
        return capture_output_billable_event_tx(
            tx,
            tenant_id,
            actor_id,
            work,
            contract_id,
            outputs,
            occurred_at,
        )
        .await;
    }
    let quantity = inputs
        .iter()
        .map(|line| line.quantity)
        .try_fold(0_i64, |total, value| total.checked_add(value))
        .ok_or_else(|| AppError::internal("value-added billable quantity exceeds i64"))?;
    insert_billable_event_tx(
        tx,
        tenant_id,
        actor_id,
        work,
        contract_id,
        quantity,
        occurred_at,
    )
    .await
}

async fn capture_output_billable_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_id: UserId,
    work: &ValueAddedWorkReadModel,
    contract_id: i64,
    outputs: &[StoredOutput],
    occurred_at: Timestamp,
) -> AppResult<Option<i64>> {
    let quantity = outputs
        .iter()
        .try_fold(0_i64, |total, line| total.checked_add(line.quantity))
        .ok_or_else(|| AppError::internal("value-added billable quantity exceeds i64"))?;
    insert_billable_event_tx(
        tx,
        tenant_id,
        actor_id,
        work,
        contract_id,
        quantity,
        occurred_at,
    )
    .await
}

async fn insert_billable_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_id: UserId,
    work: &ValueAddedWorkReadModel,
    contract_id: i64,
    quantity: i64,
    occurred_at: Timestamp,
) -> AppResult<Option<i64>> {
    let event_id = sqlx::query_scalar::<_, i64>(
        r#"INSERT INTO billable_events
           (tenant_id,inventory_owner_id,facility_id,contract_id,event_type,unit,quantity,
            source_type,source_reference,description,occurred_at,captured_by_user_id,captured_at)
           VALUES($1,$2,$3,$4,$5,'each',$6,'value_added_work_order',$7,$8,$9,$10,$9)
           RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(work.inventory_owner_id.get())
    .bind(work.facility_id.get())
    .bind(contract_id)
    .bind(billable_event_type(work.kind))
    .bind(quantity)
    .bind(work.work_id.get().to_string())
    .bind(format!("{} {}", work.kind.as_str(), work.number))
    .bind(occurred_at)
    .bind(actor_id.get())
    .fetch_one(&mut **tx)
    .await?;
    Ok(Some(event_id))
}

pub async fn complete(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ValueAddedWorkLifecycleCommand,
) -> AppResult<ValueAddedWorkReadModel> {
    require_actor(access, context)?;
    let prepared = PreparedCommand::new_v1(context, COMPLETE_VALUE_ADDED_WORK_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let (scope, work) = lifecycle_context_tx(&mut tx, access, context, command).await?;
    if let Some(result) = prepared
        .replayed::<ValueAddedWorkReadModel>(&mut tx)
        .await?
    {
        require_scope(&scope, result.inventory_owner_id, result.facility_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    if work.revision != command.expected_revision {
        return Err(AppError::conflict(
            "value-added work changed; refresh before completing it",
        ));
    }
    work.status
        .require_transition_to(ValueAddedWorkStatus::Completed)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let outputs = stored_outputs_tx(&mut tx, access.tenant_id, work.work_id).await?;
    let mut plate_ids = outputs
        .iter()
        .filter_map(|output| output.license_plate_id)
        .collect::<Vec<_>>();
    let input_hints = sqlx::query(
        r#"SELECT balance.license_plate_id FROM value_added_work_inputs input
           JOIN inventory_balances balance ON balance.tenant_id=input.tenant_id
             AND balance.inventory_owner_id=input.inventory_owner_id
             AND balance.id=input.inventory_balance_id
           WHERE input.tenant_id=$1 AND input.work_id=$2 ORDER BY input.inventory_balance_id"#,
    )
    .bind(access.tenant_id.get())
    .bind(work.work_id.get())
    .fetch_all(&mut *tx)
    .await?;
    for row in input_hints {
        if let Some(plate_id) = row.try_get::<Option<i64>, _>("license_plate_id")? {
            plate_ids.push(plate_id);
        }
    }
    lock_license_plates(&mut tx, access.tenant_id, plate_ids).await?;
    let inputs = lock_inputs_tx(&mut tx, access.tenant_id, work.work_id).await?;
    if inputs.iter().any(|input| {
        !input.active
            || input.hold_id.is_none()
            || input.qty_held < input.quantity
            || input.qty_on_hand - input.qty_reserved < input.quantity
    }) {
        return Err(AppError::conflict(
            "value-added input inventory is no longer executable",
        ));
    }
    let occurred_at = now_iso();
    let owner_facility = inventory_journal::owner_facility_scope(
        work.inventory_owner_id.get(),
        work.facility_id.get(),
    )?;
    let transaction_id = inventory_journal::begin_batched_transaction_at(
        &mut tx,
        &JournalCommand {
            tenant_id: access.tenant_id,
            owner_facility,
            actor_user_id: context.actor_id.get(),
            transaction_type: transaction_type(work.kind),
            reason: Some(command.note.as_str()),
            reference_type: Some(HOLD_REFERENCE_TYPE),
            reference_id: Some(work.work_id.get()),
            correlation_id: Some(&context.request_id),
            operation: COMPLETE_VALUE_ADDED_WORK_OPERATION,
            idempotency_key: Some(prepared.idempotency_key()),
            request_hash: prepared.request_hash(),
        },
        occurred_at,
    )
    .await?;
    for input in &inputs {
        release_composed_inventory_hold_tx(
            &mut tx,
            access.tenant_id,
            context.actor_id.get(),
            occurred_at,
            input
                .hold_id
                .ok_or_else(|| AppError::internal("released work input has no hold"))?,
            HOLD_REFERENCE_TYPE,
            work.work_id.get(),
        )
        .await?;
        decrement_input_tx(&mut tx, access.tenant_id, input, occurred_at).await?;
    }
    for output in &outputs {
        increment_output_tx(
            &mut tx,
            access.tenant_id,
            work.inventory_owner_id,
            work.facility_id,
            output,
            occurred_at,
        )
        .await?;
    }
    for input in &inputs {
        inventory_journal::append_entry(
            &mut tx,
            access.tenant_id,
            owner_facility,
            transaction_id,
            &JournalEntry {
                location_id: input.location_id,
                license_plate_id: input.license_plate_id,
                item_batch_id: input.item_batch_id,
                status: input.status,
                quantity_delta: -input.quantity,
            },
        )
        .await?;
    }
    for output in &outputs {
        inventory_journal::append_entry(
            &mut tx,
            access.tenant_id,
            owner_facility,
            transaction_id,
            &JournalEntry {
                location_id: output.location_id,
                license_plate_id: output.license_plate_id,
                item_batch_id: output.item_batch_id,
                status: output.status,
                quantity_delta: output.quantity,
            },
        )
        .await?;
    }
    let billable_event_id = capture_billable_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        &work,
        &inputs,
        &outputs,
        occurred_at,
    )
    .await?;
    update_lifecycle_tx(
        &mut tx,
        access.tenant_id,
        &work,
        ValueAddedWorkStatus::Completed,
        context.actor_id,
        occurred_at,
        LifecycleEffects {
            inventory_transaction_id: Some(transaction_id),
            billable_event_id,
        },
    )
    .await?;
    let result = read_work_tx(&mut tx, access.tenant_id, work.work_id).await?;
    insert_event_tx(
        &mut tx,
        access.tenant_id,
        &result,
        Some(work.status),
        Some(command.note.as_str()),
        context.actor_id,
        occurred_at,
    )
    .await?;
    let result = read_work_tx(&mut tx, access.tenant_id, work.work_id).await?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        &result,
        "completed",
        &result,
        occurred_at,
    )
    .await?;
    Ok(prepared
        .commit_with_inventory_transaction(tx, result, Some(transaction_id))
        .await?)
}

pub async fn cancel(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ValueAddedWorkLifecycleCommand,
) -> AppResult<ValueAddedWorkReadModel> {
    require_actor(access, context)?;
    let prepared = PreparedCommand::new_v1(context, CANCEL_VALUE_ADDED_WORK_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let (scope, work) = lifecycle_context_tx(&mut tx, access, context, command).await?;
    if let Some(result) = prepared
        .replayed::<ValueAddedWorkReadModel>(&mut tx)
        .await?
    {
        require_scope(&scope, result.inventory_owner_id, result.facility_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    if work.revision != command.expected_revision {
        return Err(AppError::conflict(
            "value-added work changed; refresh before cancelling it",
        ));
    }
    work.status
        .require_transition_to(ValueAddedWorkStatus::Cancelled)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let now = now_iso();
    if work.status == ValueAddedWorkStatus::Released {
        let inputs = lock_inputs_tx(&mut tx, access.tenant_id, work.work_id).await?;
        for input in &inputs {
            release_composed_inventory_hold_tx(
                &mut tx,
                access.tenant_id,
                context.actor_id.get(),
                now,
                input
                    .hold_id
                    .ok_or_else(|| AppError::internal("released work input has no hold"))?,
                HOLD_REFERENCE_TYPE,
                work.work_id.get(),
            )
            .await?;
        }
    }
    update_lifecycle_tx(
        &mut tx,
        access.tenant_id,
        &work,
        ValueAddedWorkStatus::Cancelled,
        context.actor_id,
        now,
        LifecycleEffects {
            inventory_transaction_id: None,
            billable_event_id: None,
        },
    )
    .await?;
    let result = read_work_tx(&mut tx, access.tenant_id, work.work_id).await?;
    insert_event_tx(
        &mut tx,
        access.tenant_id,
        &result,
        Some(work.status),
        Some(command.note.as_str()),
        context.actor_id,
        now,
    )
    .await?;
    let result = read_work_tx(&mut tx, access.tenant_id, work.work_id).await?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        &result,
        "cancelled",
        &result,
        now,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn get(
    db: &Db,
    access: &TenantAccess,
    work_id: ValueAddedWorkId,
) -> AppResult<ValueAddedWorkReadModel> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), PERMISSION).await?;
    let (owner_id, facility_id) = work_scope_tx(&mut tx, access.tenant_id, work_id).await?;
    require_scope(&scope, owner_id, facility_id)?;
    let result = read_work_tx(&mut tx, access.tenant_id, work_id).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn list(
    db: &Db,
    access: &TenantAccess,
    filter: &ValueAddedWorkFilter,
) -> AppResult<ValueAddedWorkPage> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), PERMISSION).await?;
    if let (Some(owner_id), Some(facility_id)) = (filter.inventory_owner_id, filter.facility_id) {
        require_scope(&scope, owner_id, facility_id)?;
    } else {
        if filter
            .inventory_owner_id
            .is_some_and(|id| !scope.includes_inventory_owner(id.get()))
            || filter
                .facility_id
                .is_some_and(|id| !scope.includes_facility(id.get()))
        {
            return Err(AppError::not_found("value-added work"));
        }
    }
    let fetch_limit = i64::from(filter.limit) + 1;
    let ids = sqlx::query_scalar::<_, i64>(
        r#"SELECT work.id FROM value_added_work_orders work
           WHERE work.tenant_id=$1
             AND ($2::BIGINT IS NULL OR work.inventory_owner_id=$2)
             AND ($3::BIGINT IS NULL OR work.facility_id=$3)
             AND ($4::TEXT IS NULL OR work.status=$4)
             AND ($5::BIGINT IS NULL OR work.id<$5)
             AND ($6 OR work.inventory_owner_id=ANY($7))
             AND ($8 OR work.facility_id=ANY($9))
           ORDER BY work.id DESC LIMIT $10"#,
    )
    .bind(access.tenant_id.get())
    .bind(filter.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(filter.facility_id.map(FacilityId::get))
    .bind(filter.status.map(ValueAddedWorkStatus::as_str))
    .bind(filter.before_id.map(ValueAddedWorkId::get))
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await?;
    let has_more = ids.len() > filter.limit as usize;
    let visible_ids = ids
        .iter()
        .take(filter.limit as usize)
        .copied()
        .collect::<Vec<_>>();
    let mut items = Vec::with_capacity(visible_ids.len());
    for id in &visible_ids {
        items.push(
            read_work_tx(
                &mut tx,
                access.tenant_id,
                ValueAddedWorkId::new(*id).map_err(internal)?,
            )
            .await?,
        );
    }
    let next_before_id = if has_more {
        visible_ids
            .last()
            .copied()
            .map(ValueAddedWorkId::new)
            .transpose()
            .map_err(internal)?
    } else {
        None
    };
    tx.commit().await?;
    Ok(ValueAddedWorkPage {
        items,
        next_before_id,
    })
}
