use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbound_load::{
    LoadPackedCartonCommand, MovePackedCartonResult, PackedCartonMovementDetailReadModel,
    PackedCartonMovementReadModel, StagePackedCartonCommand, UnloadPackedCartonCommand,
    UnstagePackedCartonCommand, LOAD_OUTBOUND_LOAD_CARTON_OPERATION,
    STAGE_OUTBOUND_LOAD_CARTON_OPERATION, UNLOAD_OUTBOUND_LOAD_CARTON_OPERATION,
    UNSTAGE_OUTBOUND_LOAD_CARTON_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InventoryStatus, InventoryTransactionType, TenantAccess};
use wareboxes_domain::{
    load_packed_carton, record_outbound_carton_loaded, record_outbound_carton_staged,
    record_outbound_carton_unloaded, record_outbound_carton_unstaged, stage_packed_carton,
    unload_packed_carton, unstage_packed_carton, CartonContentId, CartonId, InventoryAllocationId,
    InventoryBalanceId, LocationId, OutboundLoadCartonId, OutboundLoadRevision,
    PackedCartonMovementId, PackedCartonMovementKind, PackedCartonPositionRevision, TenantId,
    Timestamp, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry};
use crate::repo::inventory_locking;

use super::{
    enqueue_load_event_tx, load_progress_tx, lock_load_tx, positive, progress_read,
    read_model::position_for_carton_tx, LockedLoad,
};

#[derive(Debug)]
struct MoveRequest<'a> {
    load_id: wareboxes_domain::OutboundLoadId,
    carton_id: CartonId,
    expected_load_revision: OutboundLoadRevision,
    expected_position_revision: PackedCartonPositionRevision,
    source_scan: &'a str,
    carton_scan: &'a str,
    destination_scan: &'a str,
}

#[derive(Debug)]
struct LockedCarton {
    id: OutboundLoadCartonId,
    inventory_owner_id: i64,
    facility_id: i64,
    carton_id: CartonId,
    license_plate_id: i64,
    carton_barcode: String,
    load_sequence: i64,
    original_location_id: i64,
    state: String,
    revision: PackedCartonPositionRevision,
    packed_qty: i64,
    content_count: i64,
}

#[derive(Debug)]
struct PositionRow {
    id: i64,
    carton_content_id: i64,
    reservation_id: i64,
    item_batch_id: i64,
    item_id: i64,
    uom: String,
    inventory_status: InventoryStatus,
    packed_qty: i64,
    current_allocation_id: i64,
    current_balance_id: i64,
    current_location_id: i64,
    current_license_plate_id: i64,
    revision: i64,
}

#[derive(Debug)]
struct MovedDetail {
    position_id: i64,
    carton_content_id: i64,
    reservation_id: i64,
    item_batch_id: i64,
    item_id: i64,
    uom: String,
    inventory_status: InventoryStatus,
    quantity: i64,
    source_allocation_id: i64,
    destination_allocation_id: i64,
    source_balance_id: i64,
    destination_balance_id: i64,
}

pub async fn stage_carton(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &StagePackedCartonCommand,
) -> AppResult<MovePackedCartonResult> {
    let request = MoveRequest {
        load_id: command.outbound_load_id,
        carton_id: command.carton_id,
        expected_load_revision: command.expected_load_revision,
        expected_position_revision: command.expected_position_revision,
        source_scan: command.source_location_barcode.as_str(),
        carton_scan: command.carton_barcode.as_str(),
        destination_scan: command.staging_location_barcode.as_str(),
    };
    execute(
        db,
        access,
        context,
        command,
        STAGE_OUTBOUND_LOAD_CARTON_OPERATION,
        PackedCartonMovementKind::Stage,
        request,
    )
    .await
}

pub async fn load_carton(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &LoadPackedCartonCommand,
) -> AppResult<MovePackedCartonResult> {
    let request = MoveRequest {
        load_id: command.outbound_load_id,
        carton_id: command.carton_id,
        expected_load_revision: command.expected_load_revision,
        expected_position_revision: command.expected_position_revision,
        source_scan: command.staging_location_barcode.as_str(),
        carton_scan: command.carton_barcode.as_str(),
        destination_scan: command.trailer_number.as_str(),
    };
    execute(
        db,
        access,
        context,
        command,
        LOAD_OUTBOUND_LOAD_CARTON_OPERATION,
        PackedCartonMovementKind::Load,
        request,
    )
    .await
}

pub async fn unload_carton(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &UnloadPackedCartonCommand,
) -> AppResult<MovePackedCartonResult> {
    let request = MoveRequest {
        load_id: command.outbound_load_id,
        carton_id: command.carton_id,
        expected_load_revision: command.expected_load_revision,
        expected_position_revision: command.expected_position_revision,
        source_scan: command.trailer_number.as_str(),
        carton_scan: command.carton_barcode.as_str(),
        destination_scan: command.staging_location_barcode.as_str(),
    };
    execute(
        db,
        access,
        context,
        command,
        UNLOAD_OUTBOUND_LOAD_CARTON_OPERATION,
        PackedCartonMovementKind::Unload,
        request,
    )
    .await
}

pub async fn unstage_carton(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &UnstagePackedCartonCommand,
) -> AppResult<MovePackedCartonResult> {
    let request = MoveRequest {
        load_id: command.outbound_load_id,
        carton_id: command.carton_id,
        expected_load_revision: command.expected_load_revision,
        expected_position_revision: command.expected_position_revision,
        source_scan: command.staging_location_barcode.as_str(),
        carton_scan: command.carton_barcode.as_str(),
        destination_scan: command.return_location_barcode.as_str(),
    };
    execute(
        db,
        access,
        context,
        command,
        UNSTAGE_OUTBOUND_LOAD_CARTON_OPERATION,
        PackedCartonMovementKind::Unstage,
        request,
    )
    .await
}

async fn execute<T: serde::Serialize>(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &T,
    operation: &'static str,
    kind: PackedCartonMovementKind,
    request: MoveRequest<'_>,
) -> AppResult<MovePackedCartonResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, operation, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    if let Some(result) = prepared.replayed::<MovePackedCartonResult>(&mut tx).await? {
        super::require_load_visible_tx(&mut tx, access.tenant_id, result.outbound_load_id, &scope)
            .await?;
        tx.commit().await?;
        return Ok(result);
    }
    let mut load = lock_load_tx(&mut tx, access.tenant_id, request.load_id, &scope).await?;
    let expected_load_revision = load.revision;
    if load.revision != request.expected_load_revision {
        return Err(AppError::conflict("outbound load revision is stale"));
    }
    let carton = lock_carton_tx(&mut tx, access.tenant_id, &load, &request, &scope).await?;
    let positions = lock_positions_and_inventory_tx(&mut tx, access.tenant_id, &carton).await?;
    let (source_location_id, destination_location_id) = validate_move(
        &mut tx,
        access.tenant_id,
        &load,
        &carton,
        &positions,
        kind,
        &request,
    )
    .await?;
    let progress = load_progress_tx(&mut tx, access.tenant_id, &load).await?;
    let transition = match kind {
        PackedCartonMovementKind::Stage => record_outbound_carton_staged(progress),
        PackedCartonMovementKind::Load => record_outbound_carton_loaded(progress),
        PackedCartonMovementKind::Unload => record_outbound_carton_unloaded(progress),
        PackedCartonMovementKind::Unstage => record_outbound_carton_unstaged(progress),
    }
    .map_err(|error| AppError::conflict(error.to_string()))?;
    let moved_at = now_iso();
    let resulting_load_revision = if transition.advances_load_revision {
        reopen_ready_load_tx(&mut tx, access, &load, context.actor_id.get(), moved_at).await?
    } else {
        load.revision
    };
    load.revision = resulting_load_revision;
    load.state = transition.progress.status();
    let movement_id: i64 =
        sqlx::query_scalar("SELECT nextval('packed_carton_move_confirmations_id_seq')")
            .fetch_one(&mut *tx)
            .await?;
    let transaction_id = inventory_journal::begin_transaction(
        &mut tx,
        &JournalCommand {
            tenant_id: access.tenant_id,
            owner_facility: inventory_journal::owner_facility_scope(
                carton.inventory_owner_id,
                carton.facility_id,
            )?,
            actor_user_id: context.actor_id.get(),
            transaction_type: InventoryTransactionType::Move,
            reason: Some("move packed carton for outbound loading"),
            reference_type: Some("outbound_load_carton"),
            reference_id: Some(carton.id.get()),
            correlation_id: Some(&context.request_id),
            operation,
            idempotency_key: Some(prepared.idempotency_key()),
            request_hash: prepared.request_hash(),
        },
    )
    .await?;
    let details = move_inventory_tx(
        &mut tx,
        access.tenant_id,
        &carton,
        &positions,
        source_location_id,
        destination_location_id,
        transaction_id,
        context.actor_id.get(),
        moved_at,
    )
    .await?;
    let resulting_position_revision = carton
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("packed carton position revision overflow"))?;
    update_position_and_carton_tx(
        &mut tx,
        access.tenant_id,
        &load,
        &carton,
        &details,
        kind,
        movement_id,
        resulting_position_revision,
        moved_at,
    )
    .await?;
    insert_movement_evidence_tx(
        &mut tx,
        access.tenant_id,
        &load,
        &carton,
        &details,
        kind,
        movement_id,
        transaction_id,
        source_location_id,
        destination_location_id,
        resulting_position_revision,
        expected_load_revision,
        context.actor_id.get(),
        moved_at,
    )
    .await?;
    let movement = movement_read_model(
        load.id,
        &carton,
        &details,
        kind,
        movement_id,
        transaction_id,
        source_location_id,
        destination_location_id,
        context.actor_id.get(),
        moved_at,
    )?;
    let position =
        position_for_carton_tx(&mut tx, access.tenant_id, carton.carton_id, Some(&scope)).await?;
    let result = MovePackedCartonResult {
        movement,
        position,
        outbound_load_id: load.id,
        load_status: load.state,
        load_revision: load.revision,
        progress: progress_read(transition.progress),
    };
    enqueue_load_event_tx(
        &mut tx,
        super::LoadEvent {
            tenant_id: access.tenant_id,
            facility_id: load.facility_id,
            actor_user_id: context.actor_id.get(),
            load_id: load.id,
            event_type: movement_event(kind),
            event_key: &format!("{}:{}", kind.as_str(), movement_id),
            payload: serde_json::to_value(&result)
                .map_err(|error| AppError::internal(error.to_string()))?,
            occurred_at: moved_at,
        },
    )
    .await?;
    Ok(prepared
        .commit_with_inventory_transaction(tx, result, Some(transaction_id))
        .await?)
}

async fn lock_carton_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    load: &LockedLoad,
    request: &MoveRequest<'_>,
    scope: &ScopeBindings,
) -> AppResult<LockedCarton> {
    let row = sqlx::query(
        r#"
        SELECT id,inventory_owner_id,facility_id,carton_id,license_plate_id,
               carton_barcode,load_sequence,original_location_id,state,revision,
               packed_qty,content_count
        FROM outbound_load_cartons
        WHERE tenant_id=$1 AND outbound_load_id=$2 AND carton_id=$3 AND closed_at IS NULL
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(load.id.get())
    .bind(request.carton_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("outbound load carton"))?;
    let owner_id: i64 = row.try_get("inventory_owner_id")?;
    if !scope.includes_inventory_owner(owner_id) {
        return Err(AppError::not_found("outbound load carton"));
    }
    Ok(LockedCarton {
        id: positive(row.try_get("id")?, OutboundLoadCartonId::new)?,
        inventory_owner_id: owner_id,
        facility_id: row.try_get("facility_id")?,
        carton_id: positive(row.try_get("carton_id")?, CartonId::new)?,
        license_plate_id: row.try_get("license_plate_id")?,
        carton_barcode: row.try_get("carton_barcode")?,
        load_sequence: row.try_get("load_sequence")?,
        original_location_id: row.try_get("original_location_id")?,
        state: row.try_get("state")?,
        revision: positive(row.try_get("revision")?, PackedCartonPositionRevision::new)?,
        packed_qty: row.try_get("packed_qty")?,
        content_count: row.try_get("content_count")?,
    })
}

async fn lock_positions_and_inventory_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    carton: &LockedCarton,
) -> AppResult<Vec<PositionRow>> {
    let hints = sqlx::query(
        r#"
        SELECT id,reservation_id,current_inventory_allocation_id,current_inventory_balance_id
        FROM packed_inventory_positions
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND carton_id=$3
          AND state<>'unpacked'
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(carton.inventory_owner_id)
    .bind(carton.carton_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let reservation_ids = hints
        .iter()
        .map(|row| row.try_get::<i64, _>("reservation_id"))
        .collect::<Result<Vec<_>, _>>()?;
    sqlx::query("SELECT id FROM inventory_reservations WHERE tenant_id=$1 AND id=ANY($2) ORDER BY id FOR UPDATE")
        .bind(tenant_id.get()).bind(&reservation_ids).fetch_all(&mut **tx).await?;
    let allocation_ids = hints
        .iter()
        .map(|row| row.try_get::<i64, _>("current_inventory_allocation_id"))
        .collect::<Result<Vec<_>, _>>()?;
    sqlx::query("SELECT id FROM inventory_allocations WHERE tenant_id=$1 AND id=ANY($2) ORDER BY id FOR UPDATE")
        .bind(tenant_id.get()).bind(&allocation_ids).fetch_all(&mut **tx).await?;
    inventory_locking::lock_license_plates(tx, tenant_id, vec![carton.license_plate_id]).await?;
    let balance_ids = hints
        .iter()
        .map(|row| row.try_get::<i64, _>("current_inventory_balance_id"))
        .collect::<Result<Vec<_>, _>>()?;
    sqlx::query("SELECT id FROM inventory_balances WHERE tenant_id=$1 AND id=ANY($2) ORDER BY id FOR UPDATE")
        .bind(tenant_id.get()).bind(&balance_ids).fetch_all(&mut **tx).await?;
    let rows = sqlx::query(
        r#"
        SELECT id,carton_content_id,reservation_id,item_batch_id,item_id,uom,
               inventory_status,packed_qty,current_inventory_allocation_id,
               current_inventory_balance_id,current_location_id,
               current_license_plate_id,revision,state
        FROM packed_inventory_positions
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND carton_id=$3
          AND state<>'unpacked'
        ORDER BY id FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(carton.inventory_owner_id)
    .bind(carton.carton_id.get())
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != usize::try_from(carton.content_count).unwrap_or(usize::MAX) || rows.is_empty()
    {
        return Err(AppError::conflict("packed carton contents changed"));
    }
    let expected_position_state = if carton.state == "planned" {
        "packed"
    } else {
        carton.state.as_str()
    };
    rows.into_iter()
        .map(|row| {
            let status = InventoryStatus::parse(&row.try_get::<String, _>("inventory_status")?)
                .ok_or_else(|| AppError::internal("packed position has an invalid status"))?;
            if row.try_get::<String, _>("state")? != expected_position_state
                || row.try_get::<i64, _>("revision")? != carton.revision.get()
            {
                return Err(AppError::conflict("packed carton position changed"));
            }
            Ok(PositionRow {
                id: row.try_get("id")?,
                carton_content_id: row.try_get("carton_content_id")?,
                reservation_id: row.try_get("reservation_id")?,
                item_batch_id: row.try_get("item_batch_id")?,
                item_id: row.try_get("item_id")?,
                uom: row.try_get("uom")?,
                inventory_status: status,
                packed_qty: row.try_get("packed_qty")?,
                current_allocation_id: row.try_get("current_inventory_allocation_id")?,
                current_balance_id: row.try_get("current_inventory_balance_id")?,
                current_location_id: row.try_get("current_location_id")?,
                current_license_plate_id: row.try_get("current_license_plate_id")?,
                revision: row.try_get("revision")?,
            })
        })
        .collect()
}

async fn validate_move(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    load: &LockedLoad,
    carton: &LockedCarton,
    positions: &[PositionRow],
    kind: PackedCartonMovementKind,
    request: &MoveRequest<'_>,
) -> AppResult<(i64, i64)> {
    if carton.revision != request.expected_position_revision
        || carton.carton_barcode != request.carton_scan
        || positions.iter().any(|position| {
            position.current_license_plate_id != carton.license_plate_id
                || position.revision != carton.revision.get()
        })
    {
        return Err(AppError::conflict("packed carton position changed"));
    }
    let source_location_id = positions[0].current_location_id;
    if positions
        .iter()
        .any(|position| position.current_location_id != source_location_id)
    {
        return Err(AppError::conflict(
            "packed carton contents are split across locations",
        ));
    }
    let source_barcode: String = sqlx::query_scalar(
        "SELECT barcode FROM locations WHERE tenant_id=$1 AND id=$2 AND active AND deleted IS NULL FOR SHARE",
    )
    .bind(tenant_id.get())
    .bind(source_location_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("packed carton source location is unavailable"))?;
    let source_scan_matches = match kind {
        PackedCartonMovementKind::Load | PackedCartonMovementKind::Unload => {
            load.trailer_number.as_deref() == Some(request.source_scan)
                || source_barcode == request.source_scan
        }
        _ => source_barcode == request.source_scan,
    };
    if !source_scan_matches {
        return Err(AppError::bad_request(
            "source scan does not match packed carton position",
        ));
    }
    let destination_location_id = match kind {
        PackedCartonMovementKind::Stage | PackedCartonMovementKind::Unload => {
            require_location_scan_tx(
                tx,
                tenant_id,
                load,
                load.staging_location_id,
                request.destination_scan,
                "staging",
            )
            .await?
        }
        PackedCartonMovementKind::Load => {
            if load.trailer_number.as_deref() != Some(request.destination_scan) {
                return Err(AppError::bad_request(
                    "trailer scan does not match outbound load",
                ));
            }
            load.virtual_trailer_location_id
        }
        PackedCartonMovementKind::Unstage => {
            require_location_scan_tx(
                tx,
                tenant_id,
                load,
                carton.original_location_id,
                request.destination_scan,
                "packing",
            )
            .await?
        }
    };
    if kind == PackedCartonMovementKind::Load {
        let next_sequence: Option<i64> = sqlx::query_scalar(
            r#"
            SELECT MIN(load_sequence)
            FROM outbound_load_cartons
            WHERE tenant_id=$1 AND outbound_load_id=$2 AND state='staged'
            "#,
        )
        .bind(tenant_id.get())
        .bind(load.id.get())
        .fetch_one(&mut **tx)
        .await?;
        if next_sequence != Some(carton.load_sequence) {
            return Err(AppError::conflict(
                "outbound load cartons must be loaded in sequence",
            ));
        }
    }
    let original_location = positive(carton.original_location_id, LocationId::new)?;
    let staging_location = positive(load.staging_location_id, LocationId::new)?;
    let current = super::read_model::carton_state(
        &carton.state,
        Some(load.id),
        carton.original_location_id,
        source_location_id,
        Some(carton.load_sequence),
    )?;
    match kind {
        PackedCartonMovementKind::Stage => {
            stage_packed_carton(current, original_location, load.id, staging_location)
        }
        PackedCartonMovementKind::Load => load_packed_carton(
            current,
            load.id,
            u32::try_from(carton.load_sequence)
                .map_err(|_| AppError::internal("load sequence is invalid"))?,
        ),
        PackedCartonMovementKind::Unload => {
            unload_packed_carton(current, load.id, staging_location)
        }
        PackedCartonMovementKind::Unstage => {
            unstage_packed_carton(current, load.id, original_location)
        }
    }
    .map_err(|error| AppError::conflict(error.to_string()))?;
    if source_location_id == destination_location_id {
        return Err(AppError::conflict(
            "packed carton is already at the destination",
        ));
    }
    Ok((source_location_id, destination_location_id))
}

async fn require_location_scan_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    load: &LockedLoad,
    location_id: i64,
    scan: &str,
    expected_type: &str,
) -> AppResult<i64> {
    let matches: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM locations
            WHERE tenant_id=$1 AND facility_id=$2 AND id=$3 AND barcode=$4
              AND active AND deleted IS NULL AND NOT pickable AND NOT receivable
              AND lower(type)=$5
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(load.facility_id.get())
    .bind(location_id)
    .bind(scan)
    .bind(expected_type)
    .fetch_one(&mut **tx)
    .await?;
    if matches {
        Ok(location_id)
    } else {
        Err(AppError::bad_request(
            "destination scan does not match outbound load",
        ))
    }
}

async fn reopen_ready_load_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    load: &LockedLoad,
    _actor_id: i64,
    _occurred_at: Timestamp,
) -> AppResult<OutboundLoadRevision> {
    let revision = load
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("outbound load revision overflow"))?;
    let updated = sqlx::query(
        r#"
        UPDATE outbound_loads
        SET state='loading',revision=$3,seal_number=NULL,
            ready_to_depart_by_user_id=NULL,ready_to_depart_at=NULL
        WHERE tenant_id=$1 AND id=$2 AND state='ready_to_depart' AND revision=$4
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load.id.get())
    .bind(revision.get())
    .bind(load.revision.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("outbound load changed concurrently"));
    }
    Ok(revision)
}

#[allow(clippy::too_many_arguments)]
async fn move_inventory_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    carton: &LockedCarton,
    positions: &[PositionRow],
    source_location_id: i64,
    destination_location_id: i64,
    transaction_id: i64,
    actor_id: i64,
    moved_at: Timestamp,
) -> AppResult<Vec<MovedDetail>> {
    let owner_facility =
        inventory_journal::owner_facility_scope(carton.inventory_owner_id, carton.facility_id)?;
    let mut details = Vec::with_capacity(positions.len());
    for position in positions {
        let fulfilled = sqlx::query(
            r#"
            UPDATE inventory_allocations
            SET status='fulfilled',modified=$1,deleted=$1
            WHERE tenant_id=$2 AND inventory_owner_id=$3 AND id=$4
              AND status='allocated' AND deleted IS NULL AND execution_stage='packed'
              AND qty=$5 AND inventory_balance_id=$6 AND location_id=$7
            "#,
        )
        .bind(moved_at)
        .bind(tenant_id.get())
        .bind(carton.inventory_owner_id)
        .bind(position.current_allocation_id)
        .bind(position.packed_qty)
        .bind(position.current_balance_id)
        .bind(source_location_id)
        .execute(&mut **tx)
        .await?;
        if fulfilled.rows_affected() != 1 {
            return Err(AppError::conflict(
                "packed allocation changed during movement",
            ));
        }
        let decremented = sqlx::query(
            "UPDATE inventory_balances SET qty_on_hand=qty_on_hand-$1,modified=$2 WHERE tenant_id=$3 AND id=$4 AND deleted IS NULL AND qty_on_hand>=qty_reserved+qty_held+$1",
        )
        .bind(position.packed_qty).bind(moved_at).bind(tenant_id.get()).bind(position.current_balance_id)
        .execute(&mut **tx).await?;
        if decremented.rows_affected() != 1 {
            return Err(AppError::conflict(
                "packed inventory balance changed during movement",
            ));
        }
        let destination_balance_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO inventory_balances (
                tenant_id,inventory_owner_id,created,modified,facility_id,location_id,
                license_plate_id,item_batch_id,item_id,uom,status,qty_on_hand,qty_reserved,qty_held
            ) VALUES ($1,$2,$3,$3,$4,$5,$6,$7,$8,$9,$10,$11,0,0)
            ON CONFLICT (tenant_id,inventory_owner_id,location_id,license_plate_id,item_batch_id,uom,status)
                WHERE license_plate_id IS NOT NULL
            DO UPDATE SET qty_on_hand=inventory_balances.qty_on_hand+excluded.qty_on_hand,
                          modified=excluded.modified,deleted=NULL
            RETURNING id
            "#,
        )
        .bind(tenant_id.get()).bind(carton.inventory_owner_id).bind(moved_at)
        .bind(carton.facility_id).bind(destination_location_id).bind(carton.license_plate_id)
        .bind(position.item_batch_id).bind(position.item_id).bind(&position.uom)
        .bind(position.inventory_status.as_str()).bind(position.packed_qty)
        .fetch_one(&mut **tx).await?;
        let destination_allocation_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO inventory_allocations (
                tenant_id,inventory_owner_id,created,created_by,reservation_id,
                inventory_balance_id,facility_id,location_id,license_plate_id,item_batch_id,
                item_id,uom,inventory_status,allocation_run_id,qty,status,execution_stage
            ) SELECT tenant_id,inventory_owner_id,$1,$2,reservation_id,$3,facility_id,$4,
                     license_plate_id,item_batch_id,item_id,uom,inventory_status,
                     allocation_run_id,qty,'allocated','packed'
              FROM inventory_allocations
              WHERE tenant_id=$5 AND inventory_owner_id=$6 AND id=$7
                AND status='fulfilled' AND deleted=$1
            RETURNING id
            "#,
        )
        .bind(moved_at)
        .bind(actor_id)
        .bind(destination_balance_id)
        .bind(destination_location_id)
        .bind(tenant_id.get())
        .bind(carton.inventory_owner_id)
        .bind(position.current_allocation_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::conflict("packed allocation could not be replaced"))?;
        for (location_id, quantity_delta) in [
            (source_location_id, -position.packed_qty),
            (destination_location_id, position.packed_qty),
        ] {
            inventory_journal::append_entry(
                tx,
                tenant_id,
                owner_facility,
                transaction_id,
                &JournalEntry {
                    location_id,
                    license_plate_id: Some(carton.license_plate_id),
                    item_batch_id: position.item_batch_id,
                    status: position.inventory_status,
                    quantity_delta,
                },
            )
            .await?;
        }
        details.push(MovedDetail {
            position_id: position.id,
            carton_content_id: position.carton_content_id,
            reservation_id: position.reservation_id,
            item_batch_id: position.item_batch_id,
            item_id: position.item_id,
            uom: position.uom.clone(),
            inventory_status: position.inventory_status,
            quantity: position.packed_qty,
            source_allocation_id: position.current_allocation_id,
            destination_allocation_id,
            source_balance_id: position.current_balance_id,
            destination_balance_id,
        });
    }
    let moved = sqlx::query(
        "UPDATE license_plates SET location_id=$1 WHERE tenant_id=$2 AND id=$3 AND location_id=$4 AND deleted IS NULL",
    )
    .bind(destination_location_id)
    .bind(tenant_id.get())
    .bind(carton.license_plate_id)
    .bind(source_location_id)
    .execute(&mut **tx).await?;
    if moved.rows_affected() != 1 {
        return Err(AppError::conflict(
            "carton license plate moved concurrently",
        ));
    }
    Ok(details)
}

#[allow(clippy::too_many_arguments)]
async fn update_position_and_carton_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    load: &LockedLoad,
    carton: &LockedCarton,
    details: &[MovedDetail],
    kind: PackedCartonMovementKind,
    movement_id: i64,
    resulting_revision: PackedCartonPositionRevision,
    moved_at: Timestamp,
) -> AppResult<()> {
    let (
        position_state,
        carton_state,
        outbound_load_id,
        outbound_load_carton_id,
        load_sequence,
        location_id,
    ) = match kind {
        PackedCartonMovementKind::Stage | PackedCartonMovementKind::Unload => (
            "staged",
            "staged",
            Some(load.id.get()),
            Some(carton.id.get()),
            Some(carton.load_sequence),
            load.staging_location_id,
        ),
        PackedCartonMovementKind::Load => (
            "loaded",
            "loaded",
            Some(load.id.get()),
            Some(carton.id.get()),
            Some(carton.load_sequence),
            load.virtual_trailer_location_id,
        ),
        PackedCartonMovementKind::Unstage => (
            "packed",
            "planned",
            None,
            None,
            None,
            carton.original_location_id,
        ),
    };
    for detail in details {
        let updated = sqlx::query(
            r#"
            UPDATE packed_inventory_positions
            SET state=$1,outbound_load_id=$2,outbound_load_carton_id=$3,load_sequence=$4,
                current_inventory_allocation_id=$5,current_inventory_balance_id=$6,
                current_location_id=$7,current_license_plate_id=$8,revision=$9,positioned_at=$10
            WHERE tenant_id=$11 AND id=$12 AND revision=$13
            "#,
        )
        .bind(position_state)
        .bind(outbound_load_id)
        .bind(outbound_load_carton_id)
        .bind(load_sequence)
        .bind(detail.destination_allocation_id)
        .bind(detail.destination_balance_id)
        .bind(location_id)
        .bind(carton.license_plate_id)
        .bind(resulting_revision.get())
        .bind(moved_at)
        .bind(tenant_id.get())
        .bind(detail.position_id)
        .bind(carton.revision.get())
        .execute(&mut **tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(AppError::conflict("packed position changed concurrently"));
        }
    }
    let (staged_at, loaded_at) = match kind {
        PackedCartonMovementKind::Stage => (Some(moved_at), None),
        PackedCartonMovementKind::Load => (None, Some(moved_at)),
        PackedCartonMovementKind::Unload => (Some(moved_at), None),
        PackedCartonMovementKind::Unstage => (None, None),
    };
    let updated = sqlx::query(
        r#"
        UPDATE outbound_load_cartons
        SET state=$1,revision=$2,last_move_confirmation_id=$3,
            staged_at=$4,loaded_at=$5
        WHERE tenant_id=$6 AND id=$7 AND revision=$8
        "#,
    )
    .bind(carton_state)
    .bind(resulting_revision.get())
    .bind(movement_id)
    .bind(staged_at)
    .bind(loaded_at)
    .bind(tenant_id.get())
    .bind(carton.id.get())
    .bind(carton.revision.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "outbound load carton changed concurrently",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_movement_evidence_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    load: &LockedLoad,
    carton: &LockedCarton,
    details: &[MovedDetail],
    kind: PackedCartonMovementKind,
    movement_id: i64,
    transaction_id: i64,
    source_location_id: i64,
    destination_location_id: i64,
    resulting_revision: PackedCartonPositionRevision,
    expected_load_revision: OutboundLoadRevision,
    actor_id: i64,
    moved_at: Timestamp,
) -> AppResult<()> {
    let detail_count = i64::try_from(details.len())
        .map_err(|_| AppError::internal("movement detail count exceeds i64"))?;
    sqlx::query(
        r#"
        INSERT INTO packed_carton_move_confirmations (
            id,tenant_id,inventory_owner_id,facility_id,outbound_load_id,outbound_load_carton_id,
            carton_id,license_plate_id,movement_kind,inventory_transaction_id,
            expected_position_revision,resulting_position_revision,expected_load_revision,
            resulting_load_revision,source_location_id,destination_location_id,
            detail_count,moved_qty,moved_by_user_id,moved_at
        ) OVERRIDING SYSTEM VALUE
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)
        "#,
    )
    .bind(movement_id)
    .bind(tenant_id.get())
    .bind(carton.inventory_owner_id)
    .bind(carton.facility_id)
    .bind(load.id.get())
    .bind(carton.id.get())
    .bind(carton.carton_id.get())
    .bind(carton.license_plate_id)
    .bind(kind.as_str())
    .bind(transaction_id)
    .bind(carton.revision.get())
    .bind(resulting_revision.get())
    .bind(expected_load_revision.get())
    .bind(load.revision.get())
    .bind(source_location_id)
    .bind(destination_location_id)
    .bind(detail_count)
    .bind(carton.packed_qty)
    .bind(actor_id)
    .bind(moved_at)
    .execute(&mut **tx)
    .await?;
    for detail in details {
        sqlx::query(
            r#"
            INSERT INTO packed_carton_move_details (
                tenant_id,inventory_owner_id,facility_id,move_confirmation_id,carton_id,
                packed_position_id,carton_content_id,reservation_id,item_batch_id,item_id,uom,
                inventory_status,source_inventory_allocation_id,destination_inventory_allocation_id,
                source_inventory_balance_id,destination_inventory_balance_id,quantity
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
            "#,
        )
        .bind(tenant_id.get())
        .bind(carton.inventory_owner_id)
        .bind(carton.facility_id)
        .bind(movement_id)
        .bind(carton.carton_id.get())
        .bind(detail.position_id)
        .bind(detail.carton_content_id)
        .bind(detail.reservation_id)
        .bind(detail.item_batch_id)
        .bind(detail.item_id)
        .bind(&detail.uom)
        .bind(detail.inventory_status.as_str())
        .bind(detail.source_allocation_id)
        .bind(detail.destination_allocation_id)
        .bind(detail.source_balance_id)
        .bind(detail.destination_balance_id)
        .bind(detail.quantity)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn movement_read_model(
    load_id: wareboxes_domain::OutboundLoadId,
    carton: &LockedCarton,
    details: &[MovedDetail],
    kind: PackedCartonMovementKind,
    movement_id: i64,
    transaction_id: i64,
    source_location_id: i64,
    destination_location_id: i64,
    actor_id: i64,
    moved_at: Timestamp,
) -> AppResult<PackedCartonMovementReadModel> {
    Ok(PackedCartonMovementReadModel {
        movement_id: positive(movement_id, PackedCartonMovementId::new)?,
        outbound_load_id: load_id,
        outbound_load_carton_id: carton.id,
        carton_id: carton.carton_id,
        kind,
        inventory_transaction_id: transaction_id,
        source_location_id: positive(source_location_id, LocationId::new)?,
        destination_location_id: positive(destination_location_id, LocationId::new)?,
        quantity: carton.packed_qty,
        details: details
            .iter()
            .map(|detail| {
                Ok(PackedCartonMovementDetailReadModel {
                    carton_content_id: positive(detail.carton_content_id, CartonContentId::new)?,
                    source_inventory_allocation_id: positive(
                        detail.source_allocation_id,
                        InventoryAllocationId::new,
                    )?,
                    destination_inventory_allocation_id: positive(
                        detail.destination_allocation_id,
                        InventoryAllocationId::new,
                    )?,
                    source_inventory_balance_id: positive(
                        detail.source_balance_id,
                        InventoryBalanceId::new,
                    )?,
                    destination_inventory_balance_id: positive(
                        detail.destination_balance_id,
                        InventoryBalanceId::new,
                    )?,
                    quantity: detail.quantity,
                })
            })
            .collect::<AppResult<Vec<_>>>()?,
        moved_by: positive(actor_id, UserId::new)?,
        moved_at,
    })
}

fn movement_event(kind: PackedCartonMovementKind) -> &'static str {
    match kind {
        PackedCartonMovementKind::Stage => "outbound.load.carton_staged",
        PackedCartonMovementKind::Load => "outbound.load.carton_loaded",
        PackedCartonMovementKind::Unload => "outbound.load.carton_unloaded",
        PackedCartonMovementKind::Unstage => "outbound.load.carton_unstaged",
    }
}
