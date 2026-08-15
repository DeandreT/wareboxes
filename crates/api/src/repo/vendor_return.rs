//! Tenant-scoped vendor returns backed by inventory holds and an outbound journal entry.

use serde::Serialize;
use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::vendor_return::{
    CreateVendorReturnCommand, VendorReturnEventReadModel, VendorReturnFilter,
    VendorReturnLifecycleCommand, VendorReturnLineReadModel, VendorReturnPage,
    VendorReturnReadModel, CANCEL_VENDOR_RETURN_OPERATION, CREATE_VENDOR_RETURN_OPERATION,
    RELEASE_VENDOR_RETURN_OPERATION, SHIP_VENDOR_RETURN_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{
    InventoryHoldReason, InventoryStatus, InventoryTransactionType, TenantAccess,
};
use wareboxes_domain::{
    validate_vendor_return_lines, BillableEventId, FacilityId, InventoryBalanceId, InventoryHoldId,
    InventoryOwnerId, ItemBatchId, LicensePlateId, LocationId, TenantId, Timestamp, UserId,
    VendorReturnEventId, VendorReturnId, VendorReturnLineId, VendorReturnQuantity,
    VendorReturnReason, VendorReturnRevision, VendorReturnStatus,
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
const REFERENCE_TYPE: &str = "vendor_return";

#[derive(Debug, Clone)]
struct LockedLine {
    line_id: i64,
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
        Err(AppError::not_found("vendor return"))
    }
}

fn parse_status(value: &str) -> AppResult<VendorReturnStatus> {
    VendorReturnStatus::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored vendor-return status"))
}

fn parse_reason(value: &str) -> AppResult<VendorReturnReason> {
    VendorReturnReason::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored vendor-return reason"))
}

fn parse_inventory_status(value: &str) -> AppResult<InventoryStatus> {
    InventoryStatus::parse(value)
        .ok_or_else(|| AppError::internal("invalid stored inventory status"))
}

async fn lock_return_key_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    vendor_return_id: VendorReturnId,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "vendor-return:{}:{}",
            tenant_id.get(),
            vendor_return_id.get()
        ))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn return_scope_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    vendor_return_id: VendorReturnId,
) -> AppResult<(InventoryOwnerId, FacilityId)> {
    let row = sqlx::query(
        "SELECT inventory_owner_id,facility_id FROM vendor_returns WHERE tenant_id=$1 AND id=$2",
    )
    .bind(tenant_id.get())
    .bind(vendor_return_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("vendor return"))?;
    Ok((
        InventoryOwnerId::new(row.try_get("inventory_owner_id")?).map_err(internal)?,
        FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
    ))
}

async fn read_return_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    vendor_return_id: VendorReturnId,
) -> AppResult<VendorReturnReadModel> {
    let row = sqlx::query(
        r#"SELECT vendor_return.*,owner.name AS inventory_owner_name,
                  facility.name AS facility_name
           FROM vendor_returns vendor_return
           JOIN inventory_owners owner ON owner.tenant_id=vendor_return.tenant_id
             AND owner.id=vendor_return.inventory_owner_id
           JOIN facilities facility ON facility.tenant_id=vendor_return.tenant_id
             AND facility.id=vendor_return.facility_id
           WHERE vendor_return.tenant_id=$1 AND vendor_return.id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(vendor_return_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("vendor return"))?;
    let line_rows = sqlx::query(
        r#"SELECT line.*,balance.location_id,balance.license_plate_id,balance.item_batch_id,
                  balance.item_id,balance.uom,balance.status AS inventory_status,
                  COALESCE(location.barcode,location.name,'Location #'||location.id::TEXT)
                    AS location_code,plate.barcode AS license_plate_number,
                  item.description AS item_description,batch.lot,batch.serial
           FROM vendor_return_lines line
           JOIN inventory_balances balance ON balance.tenant_id=line.tenant_id
             AND balance.inventory_owner_id=line.inventory_owner_id
             AND balance.id=line.inventory_balance_id
           JOIN locations location ON location.tenant_id=balance.tenant_id
             AND location.facility_id=balance.facility_id AND location.id=balance.location_id
           JOIN item_batches batch ON batch.tenant_id=balance.tenant_id
             AND batch.inventory_owner_id=balance.inventory_owner_id
             AND batch.id=balance.item_batch_id
           JOIN items item ON item.tenant_id=batch.tenant_id AND item.id=batch.item_id
           LEFT JOIN license_plates plate ON plate.tenant_id=balance.tenant_id
             AND plate.inventory_owner_id=balance.inventory_owner_id
             AND plate.facility_id=balance.facility_id AND plate.id=balance.license_plate_id
           WHERE line.tenant_id=$1 AND line.vendor_return_id=$2 ORDER BY line.id"#,
    )
    .bind(tenant_id.get())
    .bind(vendor_return_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let lines = line_rows
        .iter()
        .map(|line| {
            Ok(VendorReturnLineReadModel {
                line_id: VendorReturnLineId::new(line.try_get("id")?).map_err(internal)?,
                inventory_balance_id: InventoryBalanceId::new(
                    line.try_get("inventory_balance_id")?,
                )
                .map_err(internal)?,
                location_id: LocationId::new(line.try_get("location_id")?).map_err(internal)?,
                location_code: line.try_get("location_code")?,
                license_plate_id: line
                    .try_get::<Option<i64>, _>("license_plate_id")?
                    .map(LicensePlateId::new)
                    .transpose()
                    .map_err(internal)?,
                license_plate_number: line.try_get("license_plate_number")?,
                item_batch_id: ItemBatchId::new(line.try_get("item_batch_id")?)
                    .map_err(internal)?,
                item_id: line.try_get("item_id")?,
                item_description: line.try_get("item_description")?,
                uom: line.try_get("uom")?,
                lot: line.try_get("lot")?,
                serial: line.try_get("serial")?,
                inventory_status: line.try_get("inventory_status")?,
                quantity: VendorReturnQuantity::new(line.try_get("quantity")?).map_err(internal)?,
                reason: parse_reason(&line.try_get::<String, _>("reason")?)?,
                note: line.try_get("note")?,
                hold_id: line
                    .try_get::<Option<i64>, _>("inventory_hold_id")?
                    .map(InventoryHoldId::new)
                    .transpose()
                    .map_err(internal)?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let event_rows = sqlx::query(
        r#"SELECT * FROM vendor_return_events WHERE tenant_id=$1
           AND vendor_return_id=$2 ORDER BY resulting_revision"#,
    )
    .bind(tenant_id.get())
    .bind(vendor_return_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let events = event_rows
        .iter()
        .map(|event| {
            Ok(VendorReturnEventReadModel {
                event_id: VendorReturnEventId::new(event.try_get("id")?).map_err(internal)?,
                from_status: event
                    .try_get::<Option<String>, _>("from_status")?
                    .map(|value| parse_status(&value))
                    .transpose()?,
                to_status: parse_status(&event.try_get::<String, _>("to_status")?)?,
                note: event.try_get("note")?,
                resulting_revision: VendorReturnRevision::new(event.try_get("resulting_revision")?)
                    .map_err(internal)?,
                actor_id: UserId::new(event.try_get("actor_user_id")?).map_err(internal)?,
                occurred_at: event.try_get("occurred_at")?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    Ok(VendorReturnReadModel {
        vendor_return_id,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(internal)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        facility_name: row.try_get("facility_name")?,
        number: row.try_get("return_number")?,
        vendor_name: row.try_get("vendor_name")?,
        vendor_reference: row.try_get("vendor_reference")?,
        status: parse_status(&row.try_get::<String, _>("status")?)?,
        revision: VendorReturnRevision::new(row.try_get("revision")?).map_err(internal)?,
        note: row.try_get("note")?,
        lines,
        shipment_inventory_transaction_id: row.try_get("shipment_inventory_transaction_id")?,
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
        shipped_by: row
            .try_get::<Option<i64>, _>("shipped_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(internal)?,
        shipped_at: row.try_get("shipped_at")?,
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
    value: &VendorReturnReadModel,
    from_status: Option<VendorReturnStatus>,
    note: Option<&str>,
    actor_id: UserId,
    occurred_at: Timestamp,
) -> AppResult<()> {
    sqlx::query(
        r#"INSERT INTO vendor_return_events
           (tenant_id,inventory_owner_id,facility_id,vendor_return_id,from_status,to_status,
            note,resulting_revision,actor_user_id,occurred_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)"#,
    )
    .bind(tenant_id.get())
    .bind(value.inventory_owner_id.get())
    .bind(value.facility_id.get())
    .bind(value.vendor_return_id.get())
    .bind(from_status.map(VendorReturnStatus::as_str))
    .bind(value.status.as_str())
    .bind(note)
    .bind(value.revision.get())
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
    value: &VendorReturnReadModel,
    transition: &str,
    payload: &T,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let ordering_key = format!("vendor-return:{}", value.vendor_return_id.get());
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let event_key = format!("{ordering_key}:{transition}:{sequence}");
    let aggregate_id = value.vendor_return_id.get().to_string();
    let event_type = format!("vendor_return.{transition}");
    let payload = serde_json::to_value(payload).map_err(internal)?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(value.inventory_owner_id),
            facility_id: Some(value.facility_id),
            actor_user_id: Some(actor_id.get()),
            event_key: &event_key,
            aggregate_type: REFERENCE_TYPE,
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

pub async fn create(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CreateVendorReturnCommand,
) -> AppResult<VendorReturnReadModel> {
    require_actor(access, context)?;
    let line_ids = command
        .lines
        .iter()
        .map(|line| line.inventory_balance_id.get())
        .collect::<Vec<_>>();
    validate_vendor_return_lines(&line_ids)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    if command
        .lines
        .iter()
        .any(|line| line.reason == VendorReturnReason::Other && line.note.is_none())
    {
        return Err(AppError::bad_request(
            "vendor-return lines with reason other require a note",
        ));
    }
    let prepared = PreparedCommand::new_v1(context, CREATE_VENDOR_RETURN_OPERATION, command)?;
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
    if let Some(result) = prepared.replayed::<VendorReturnReadModel>(&mut tx).await? {
        require_scope(&scope, result.inventory_owner_id, result.facility_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "vendor-return-number:{}:{}:{}:{}",
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
        r#"SELECT EXISTS(SELECT 1 FROM vendor_returns WHERE tenant_id=$1
           AND inventory_owner_id=$2 AND facility_id=$3 AND return_number=$4)"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.inventory_owner_id.get())
    .bind(command.facility_id.get())
    .bind(command.number.as_str())
    .fetch_one(&mut *tx)
    .await?;
    if duplicate {
        return Err(AppError::conflict(
            "vendor-return number already exists in this owner and facility",
        ));
    }
    let mut sorted_ids = line_ids.clone();
    sorted_ids.sort_unstable();
    let visible = sqlx::query_scalar::<_, i64>(
        r#"SELECT balance.id FROM inventory_balances balance
           WHERE balance.tenant_id=$1 AND balance.inventory_owner_id=$2
             AND balance.facility_id=$3 AND balance.id=ANY($4)
             AND balance.deleted IS NULL ORDER BY balance.id FOR SHARE"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.inventory_owner_id.get())
    .bind(command.facility_id.get())
    .bind(&sorted_ids)
    .fetch_all(&mut *tx)
    .await?;
    if visible != sorted_ids {
        return Err(AppError::not_found("vendor-return inventory balance"));
    }
    let now = now_iso();
    let vendor_return_id = VendorReturnId::new(
        sqlx::query_scalar::<_, i64>(
            r#"INSERT INTO vendor_returns
               (tenant_id,inventory_owner_id,facility_id,return_number,vendor_name,
                vendor_reference,note,created_by_user_id,created_at)
               VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING id"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.inventory_owner_id.get())
        .bind(command.facility_id.get())
        .bind(command.number.as_str())
        .bind(command.vendor_name.as_str())
        .bind(
            command
                .vendor_reference
                .as_ref()
                .map(|value| value.as_str()),
        )
        .bind(command.note.as_ref().map(|value| value.as_str()))
        .bind(context.actor_id.get())
        .bind(now)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(internal)?;
    for line in &command.lines {
        sqlx::query(
            r#"INSERT INTO vendor_return_lines
               (tenant_id,inventory_owner_id,facility_id,vendor_return_id,
                inventory_balance_id,quantity,reason,note)
               VALUES($1,$2,$3,$4,$5,$6,$7,$8)"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.inventory_owner_id.get())
        .bind(command.facility_id.get())
        .bind(vendor_return_id.get())
        .bind(line.inventory_balance_id.get())
        .bind(line.quantity.get())
        .bind(line.reason.as_str())
        .bind(line.note.as_ref().map(|value| value.as_str()))
        .execute(&mut *tx)
        .await?;
    }
    let result = read_return_tx(&mut tx, access.tenant_id, vendor_return_id).await?;
    insert_event_tx(
        &mut tx,
        access.tenant_id,
        &result,
        None,
        result.note.as_deref(),
        context.actor_id,
        now,
    )
    .await?;
    let result = read_return_tx(&mut tx, access.tenant_id, vendor_return_id).await?;
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

async fn lock_return_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    vendor_return_id: VendorReturnId,
) -> AppResult<VendorReturnReadModel> {
    lock_return_key_tx(tx, tenant_id, vendor_return_id).await?;
    sqlx::query("SELECT id FROM vendor_returns WHERE tenant_id=$1 AND id=$2 FOR UPDATE")
        .bind(tenant_id.get())
        .bind(vendor_return_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("vendor return"))?;
    read_return_tx(tx, tenant_id, vendor_return_id).await
}

async fn lock_lines_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    vendor_return_id: VendorReturnId,
) -> AppResult<Vec<LockedLine>> {
    let rows = sqlx::query(
        r#"SELECT line.id AS line_id,line.inventory_balance_id,line.quantity,
                  line.inventory_hold_id,balance.location_id,balance.license_plate_id,
                  balance.item_batch_id,balance.status,balance.qty_on_hand,
                  balance.qty_reserved,balance.qty_held,
                  (balance.deleted IS NULL AND location.deleted IS NULL
                    AND batch.deleted IS NULL AND (balance.license_plate_id IS NULL
                      OR plate.deleted IS NULL)) AS active
           FROM vendor_return_lines line
           JOIN inventory_balances balance ON balance.tenant_id=line.tenant_id
             AND balance.inventory_owner_id=line.inventory_owner_id
             AND balance.id=line.inventory_balance_id
           JOIN locations location ON location.tenant_id=balance.tenant_id
             AND location.facility_id=balance.facility_id AND location.id=balance.location_id
           JOIN item_batches batch ON batch.tenant_id=balance.tenant_id
             AND batch.inventory_owner_id=balance.inventory_owner_id
             AND batch.id=balance.item_batch_id
           LEFT JOIN license_plates plate ON plate.tenant_id=balance.tenant_id
             AND plate.inventory_owner_id=balance.inventory_owner_id
             AND plate.facility_id=balance.facility_id AND plate.id=balance.license_plate_id
           WHERE line.tenant_id=$1 AND line.vendor_return_id=$2
           ORDER BY balance.id,line.id FOR UPDATE OF balance"#,
    )
    .bind(tenant_id.get())
    .bind(vendor_return_id.get())
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(LockedLine {
                line_id: row.try_get("line_id")?,
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

async fn lock_return_plates_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    vendor_return_id: VendorReturnId,
) -> AppResult<()> {
    let rows = sqlx::query(
        r#"SELECT balance.license_plate_id FROM vendor_return_lines line
           JOIN inventory_balances balance ON balance.tenant_id=line.tenant_id
             AND balance.inventory_owner_id=line.inventory_owner_id
             AND balance.id=line.inventory_balance_id
           WHERE line.tenant_id=$1 AND line.vendor_return_id=$2
           ORDER BY line.inventory_balance_id"#,
    )
    .bind(tenant_id.get())
    .bind(vendor_return_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let ids = rows
        .iter()
        .filter_map(|row| {
            row.try_get::<Option<i64>, _>("license_plate_id")
                .transpose()
        })
        .collect::<Result<Vec<_>, _>>()?;
    lock_license_plates(tx, tenant_id, ids).await?;
    Ok(())
}

async fn lifecycle_context_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    command: &VendorReturnLifecycleCommand,
) -> AppResult<(ScopeBindings, VendorReturnReadModel)> {
    let scope = lock_current_scope_tx(tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(tx, access.tenant_id, context.actor_id.get(), PERMISSION).await?;
    let value = lock_return_tx(tx, access.tenant_id, command.vendor_return_id).await?;
    require_scope(&scope, value.inventory_owner_id, value.facility_id)?;
    Ok((scope, value))
}

struct LifecycleEffects {
    inventory_transaction_id: Option<i64>,
    billable_event_id: Option<i64>,
}

async fn update_lifecycle_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    value: &VendorReturnReadModel,
    target: VendorReturnStatus,
    actor_id: UserId,
    occurred_at: Timestamp,
    effects: LifecycleEffects,
) -> AppResult<()> {
    let revision = value.revision.next().map_err(internal)?;
    let updated = match target {
        VendorReturnStatus::Released => {
            sqlx::query(
                r#"UPDATE vendor_returns SET status='released',revision=$1,
               released_by_user_id=$2,released_at=$3 WHERE tenant_id=$4 AND id=$5
               AND status='draft' AND revision=$6"#,
            )
            .bind(revision.get())
            .bind(actor_id.get())
            .bind(occurred_at)
            .bind(tenant_id.get())
            .bind(value.vendor_return_id.get())
            .bind(value.revision.get())
            .execute(&mut **tx)
            .await?
        }
        VendorReturnStatus::Shipped => {
            sqlx::query(
                r#"UPDATE vendor_returns SET status='shipped',revision=$1,
               shipped_by_user_id=$2,shipped_at=$3,shipment_inventory_transaction_id=$4,
               billable_event_id=$5 WHERE tenant_id=$6 AND id=$7
               AND status='released' AND revision=$8"#,
            )
            .bind(revision.get())
            .bind(actor_id.get())
            .bind(occurred_at)
            .bind(effects.inventory_transaction_id)
            .bind(effects.billable_event_id)
            .bind(tenant_id.get())
            .bind(value.vendor_return_id.get())
            .bind(value.revision.get())
            .execute(&mut **tx)
            .await?
        }
        VendorReturnStatus::Cancelled => {
            sqlx::query(
                r#"UPDATE vendor_returns SET status='cancelled',revision=$1,
               cancelled_by_user_id=$2,cancelled_at=$3 WHERE tenant_id=$4 AND id=$5
               AND status=$6 AND revision=$7"#,
            )
            .bind(revision.get())
            .bind(actor_id.get())
            .bind(occurred_at)
            .bind(tenant_id.get())
            .bind(value.vendor_return_id.get())
            .bind(value.status.as_str())
            .bind(value.revision.get())
            .execute(&mut **tx)
            .await?
        }
        VendorReturnStatus::Draft => {
            return Err(AppError::internal(
                "vendor return cannot transition back to draft",
            ));
        }
    };
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "vendor return changed during lifecycle command",
        ));
    }
    Ok(())
}

pub async fn release(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &VendorReturnLifecycleCommand,
) -> AppResult<VendorReturnReadModel> {
    require_actor(access, context)?;
    let prepared = PreparedCommand::new_v1(context, RELEASE_VENDOR_RETURN_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let (scope, value) = lifecycle_context_tx(&mut tx, access, context, command).await?;
    if let Some(result) = prepared.replayed::<VendorReturnReadModel>(&mut tx).await? {
        require_scope(&scope, result.inventory_owner_id, result.facility_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    if value.revision != command.expected_revision {
        return Err(AppError::conflict(
            "vendor return changed; refresh before releasing it",
        ));
    }
    value
        .status
        .require_transition_to(VendorReturnStatus::Released)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    lock_return_plates_tx(&mut tx, access.tenant_id, value.vendor_return_id).await?;
    let lines = lock_lines_tx(&mut tx, access.tenant_id, value.vendor_return_id).await?;
    if lines
        .iter()
        .any(|line| !line.active || line.hold_id.is_some())
    {
        return Err(AppError::conflict(
            "vendor-return stock is no longer available for release",
        ));
    }
    let now = now_iso();
    for line in &lines {
        let hold_id = place_composed_inventory_hold_tx(
            &mut tx,
            access.tenant_id,
            context.actor_id.get(),
            now,
            &PlaceInventoryHoldCommand {
                inventory_balance_id: line.inventory_balance_id,
                qty: line.quantity,
                reason: InventoryHoldReason::Other,
                note: Some(command.note.as_str()),
                reference_type: Some(REFERENCE_TYPE),
                reference_id: Some(value.vendor_return_id.get()),
            },
        )
        .await?;
        let updated = sqlx::query(
            r#"UPDATE vendor_return_lines SET inventory_hold_id=$1
               WHERE tenant_id=$2 AND id=$3 AND inventory_hold_id IS NULL"#,
        )
        .bind(hold_id)
        .bind(access.tenant_id.get())
        .bind(line.line_id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(AppError::conflict(
                "vendor-return line changed while reserving inventory",
            ));
        }
    }
    update_lifecycle_tx(
        &mut tx,
        access.tenant_id,
        &value,
        VendorReturnStatus::Released,
        context.actor_id,
        now,
        LifecycleEffects {
            inventory_transaction_id: None,
            billable_event_id: None,
        },
    )
    .await?;
    let result = read_return_tx(&mut tx, access.tenant_id, value.vendor_return_id).await?;
    insert_event_tx(
        &mut tx,
        access.tenant_id,
        &result,
        Some(value.status),
        Some(command.note.as_str()),
        context.actor_id,
        now,
    )
    .await?;
    let result = read_return_tx(&mut tx, access.tenant_id, value.vendor_return_id).await?;
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

async fn decrement_line_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    line: &LockedLine,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"UPDATE inventory_balances SET qty_on_hand=qty_on_hand-$1,modified=$2
           WHERE tenant_id=$3 AND id=$4 AND deleted IS NULL AND status=$5
             AND qty_on_hand=$6 AND qty_reserved=$7 AND qty_held=$8
             AND qty_on_hand-qty_reserved-qty_held >= $1"#,
    )
    .bind(line.quantity)
    .bind(occurred_at)
    .bind(tenant_id.get())
    .bind(line.inventory_balance_id)
    .bind(line.status.as_str())
    .bind(line.qty_on_hand)
    .bind(line.qty_reserved)
    .bind(line.qty_held - line.quantity)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "vendor-return inventory changed during shipment",
        ));
    }
    Ok(())
}

async fn capture_billable_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_id: UserId,
    value: &VendorReturnReadModel,
    quantity: i64,
    occurred_at: Timestamp,
) -> AppResult<Option<i64>> {
    let contract_id = sqlx::query_scalar::<_, i64>(
        r#"SELECT id FROM billing_contracts WHERE tenant_id=$1 AND inventory_owner_id=$2
           AND status='active' AND effective_from<=$3
           AND (effective_until IS NULL OR effective_until>$3)
           ORDER BY effective_from DESC,id DESC LIMIT 1 FOR SHARE"#,
    )
    .bind(tenant_id.get())
    .bind(value.inventory_owner_id.get())
    .bind(occurred_at)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(contract_id) = contract_id else {
        return Ok(None);
    };
    Ok(Some(
        sqlx::query_scalar::<_, i64>(
            r#"INSERT INTO billable_events
               (tenant_id,inventory_owner_id,facility_id,contract_id,event_type,unit,quantity,
                source_type,source_reference,description,occurred_at,captured_by_user_id,captured_at)
               VALUES($1,$2,$3,$4,'return_unit','each',$5,'vendor_return',$6,$7,$8,$9,$8)
               RETURNING id"#,
        )
        .bind(tenant_id.get())
        .bind(value.inventory_owner_id.get())
        .bind(value.facility_id.get())
        .bind(contract_id)
        .bind(quantity)
        .bind(value.vendor_return_id.get().to_string())
        .bind(format!(
            "Return {} to {}",
            value.number, value.vendor_name
        ))
        .bind(occurred_at)
        .bind(actor_id.get())
        .fetch_one(&mut **tx)
        .await?,
    ))
}

pub async fn ship(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &VendorReturnLifecycleCommand,
) -> AppResult<VendorReturnReadModel> {
    require_actor(access, context)?;
    let prepared = PreparedCommand::new_v1(context, SHIP_VENDOR_RETURN_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let (scope, value) = lifecycle_context_tx(&mut tx, access, context, command).await?;
    if let Some(result) = prepared.replayed::<VendorReturnReadModel>(&mut tx).await? {
        require_scope(&scope, result.inventory_owner_id, result.facility_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    if value.revision != command.expected_revision {
        return Err(AppError::conflict(
            "vendor return changed; refresh before shipping it",
        ));
    }
    value
        .status
        .require_transition_to(VendorReturnStatus::Shipped)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    lock_return_plates_tx(&mut tx, access.tenant_id, value.vendor_return_id).await?;
    let lines = lock_lines_tx(&mut tx, access.tenant_id, value.vendor_return_id).await?;
    if lines.iter().any(|line| {
        !line.active
            || line.hold_id.is_none()
            || line.qty_held < line.quantity
            || line.qty_on_hand - line.qty_reserved < line.quantity
    }) {
        return Err(AppError::conflict(
            "vendor-return inventory is no longer executable",
        ));
    }
    let occurred_at = now_iso();
    let owner_facility = inventory_journal::owner_facility_scope(
        value.inventory_owner_id.get(),
        value.facility_id.get(),
    )?;
    let transaction_id = inventory_journal::begin_batched_transaction_at(
        &mut tx,
        &JournalCommand {
            tenant_id: access.tenant_id,
            owner_facility,
            actor_user_id: context.actor_id.get(),
            transaction_type: InventoryTransactionType::ReturnToVendor,
            reason: Some(command.note.as_str()),
            reference_type: Some(REFERENCE_TYPE),
            reference_id: Some(value.vendor_return_id.get()),
            correlation_id: Some(&context.request_id),
            operation: SHIP_VENDOR_RETURN_OPERATION,
            idempotency_key: Some(prepared.idempotency_key()),
            request_hash: prepared.request_hash(),
        },
        occurred_at,
    )
    .await?;
    let quantity = lines.iter().try_fold(0_i64, |total, line| {
        total
            .checked_add(line.quantity)
            .ok_or_else(|| AppError::internal("vendor-return quantity exceeds i64"))
    })?;
    for line in &lines {
        release_composed_inventory_hold_tx(
            &mut tx,
            access.tenant_id,
            context.actor_id.get(),
            occurred_at,
            line.hold_id
                .ok_or_else(|| AppError::internal("released vendor-return line has no hold"))?,
            REFERENCE_TYPE,
            value.vendor_return_id.get(),
        )
        .await?;
        decrement_line_tx(&mut tx, access.tenant_id, line, occurred_at).await?;
        inventory_journal::append_entry(
            &mut tx,
            access.tenant_id,
            owner_facility,
            transaction_id,
            &JournalEntry {
                location_id: line.location_id,
                license_plate_id: line.license_plate_id,
                item_batch_id: line.item_batch_id,
                status: line.status,
                quantity_delta: -line.quantity,
            },
        )
        .await?;
    }
    let billable_event_id = capture_billable_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        &value,
        quantity,
        occurred_at,
    )
    .await?;
    update_lifecycle_tx(
        &mut tx,
        access.tenant_id,
        &value,
        VendorReturnStatus::Shipped,
        context.actor_id,
        occurred_at,
        LifecycleEffects {
            inventory_transaction_id: Some(transaction_id),
            billable_event_id,
        },
    )
    .await?;
    let result = read_return_tx(&mut tx, access.tenant_id, value.vendor_return_id).await?;
    insert_event_tx(
        &mut tx,
        access.tenant_id,
        &result,
        Some(value.status),
        Some(command.note.as_str()),
        context.actor_id,
        occurred_at,
    )
    .await?;
    let result = read_return_tx(&mut tx, access.tenant_id, value.vendor_return_id).await?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        &result,
        "shipped",
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
    command: &VendorReturnLifecycleCommand,
) -> AppResult<VendorReturnReadModel> {
    require_actor(access, context)?;
    let prepared = PreparedCommand::new_v1(context, CANCEL_VENDOR_RETURN_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let (scope, value) = lifecycle_context_tx(&mut tx, access, context, command).await?;
    if let Some(result) = prepared.replayed::<VendorReturnReadModel>(&mut tx).await? {
        require_scope(&scope, result.inventory_owner_id, result.facility_id)?;
        tx.commit().await?;
        return Ok(result);
    }
    if value.revision != command.expected_revision {
        return Err(AppError::conflict(
            "vendor return changed; refresh before cancelling it",
        ));
    }
    value
        .status
        .require_transition_to(VendorReturnStatus::Cancelled)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let now = now_iso();
    if value.status == VendorReturnStatus::Released {
        lock_return_plates_tx(&mut tx, access.tenant_id, value.vendor_return_id).await?;
        let lines = lock_lines_tx(&mut tx, access.tenant_id, value.vendor_return_id).await?;
        for line in &lines {
            release_composed_inventory_hold_tx(
                &mut tx,
                access.tenant_id,
                context.actor_id.get(),
                now,
                line.hold_id
                    .ok_or_else(|| AppError::internal("released vendor-return line has no hold"))?,
                REFERENCE_TYPE,
                value.vendor_return_id.get(),
            )
            .await?;
        }
    }
    update_lifecycle_tx(
        &mut tx,
        access.tenant_id,
        &value,
        VendorReturnStatus::Cancelled,
        context.actor_id,
        now,
        LifecycleEffects {
            inventory_transaction_id: None,
            billable_event_id: None,
        },
    )
    .await?;
    let result = read_return_tx(&mut tx, access.tenant_id, value.vendor_return_id).await?;
    insert_event_tx(
        &mut tx,
        access.tenant_id,
        &result,
        Some(value.status),
        Some(command.note.as_str()),
        context.actor_id,
        now,
    )
    .await?;
    let result = read_return_tx(&mut tx, access.tenant_id, value.vendor_return_id).await?;
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
    vendor_return_id: VendorReturnId,
) -> AppResult<VendorReturnReadModel> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), PERMISSION).await?;
    let (owner_id, facility_id) =
        return_scope_tx(&mut tx, access.tenant_id, vendor_return_id).await?;
    require_scope(&scope, owner_id, facility_id)?;
    let result = read_return_tx(&mut tx, access.tenant_id, vendor_return_id).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn list(
    db: &Db,
    access: &TenantAccess,
    filter: &VendorReturnFilter,
) -> AppResult<VendorReturnPage> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), PERMISSION).await?;
    if filter
        .inventory_owner_id
        .is_some_and(|id| !scope.includes_inventory_owner(id.get()))
        || filter
            .facility_id
            .is_some_and(|id| !scope.includes_facility(id.get()))
    {
        return Err(AppError::not_found("vendor return"));
    }
    let ids = sqlx::query_scalar::<_, i64>(
        r#"SELECT vendor_return.id FROM vendor_returns vendor_return
           WHERE vendor_return.tenant_id=$1
             AND ($2::BIGINT IS NULL OR vendor_return.inventory_owner_id=$2)
             AND ($3::BIGINT IS NULL OR vendor_return.facility_id=$3)
             AND ($4::TEXT IS NULL OR vendor_return.status=$4)
             AND ($5::BIGINT IS NULL OR vendor_return.id<$5)
             AND ($6 OR vendor_return.inventory_owner_id=ANY($7))
             AND ($8 OR vendor_return.facility_id=ANY($9))
           ORDER BY vendor_return.id DESC LIMIT $10"#,
    )
    .bind(access.tenant_id.get())
    .bind(filter.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(filter.facility_id.map(FacilityId::get))
    .bind(filter.status.map(VendorReturnStatus::as_str))
    .bind(filter.before_id.map(VendorReturnId::get))
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(i64::from(filter.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let has_more = ids.len() > filter.limit as usize;
    let visible = ids
        .iter()
        .take(filter.limit as usize)
        .copied()
        .collect::<Vec<_>>();
    let mut items = Vec::with_capacity(visible.len());
    for id in &visible {
        items.push(
            read_return_tx(
                &mut tx,
                access.tenant_id,
                VendorReturnId::new(*id).map_err(internal)?,
            )
            .await?,
        );
    }
    tx.commit().await?;
    Ok(VendorReturnPage {
        items,
        next_before_id: has_more
            .then(|| visible.last().copied())
            .flatten()
            .map(VendorReturnId::new)
            .transpose()
            .map_err(internal)?,
    })
}
