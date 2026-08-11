//! Atomic, replay-safe planning of inbound loads and their expected contents.

use std::collections::HashSet;

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::inbound_load::{
    ArriveInboundLoadCommand, ArriveInboundLoadResult, ArrivedInboundLoadStatus,
    InboundLoadReceivingStatus, PlanInboundLoadCommand, PlanInboundLoadResult,
    PlannedInboundLoadLineResult, PlannedInboundLoadStatus, StartInboundLoadUnloadingCommand,
    StartInboundLoadUnloadingResult, ARRIVE_INBOUND_LOAD_OPERATION, PLAN_INBOUND_LOAD_OPERATION,
    START_INBOUND_LOAD_UNLOADING_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    validate_inbound_load_arrival, validate_inbound_load_unloading_start, InboundLoadArrivalId,
    InboundLoadId, InboundLoadLineId, InboundLoadPreArrivalStatus, InboundLoadUnloadingStartId,
    LocationId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::{insert_result, PostgresPreparedCommandExt};
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use super::access::{lock_current_scope_tx, require_permission_tx};
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundLoadEntryItem {
    pub item_id: i64,
    pub description: Option<String>,
    pub uom: String,
}

pub async fn inbound_load_entry_items(
    db: &Db,
    access: &TenantAccess,
    inventory_owner_id: i64,
    search: Option<&str>,
    limit: i64,
) -> AppResult<Option<Vec<InboundLoadEntryItem>>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    if !scope.includes_inventory_owner(inventory_owner_id) {
        return Ok(None);
    }
    let rows = sqlx::query(
        r#"
        SELECT item.id, item.description, item.packaging_unit
        FROM inventory_owner_items owner_item
        INNER JOIN items item
          ON item.tenant_id=owner_item.tenant_id AND item.id=owner_item.item_id
        WHERE owner_item.tenant_id=$1
          AND owner_item.inventory_owner_id=$2
          AND owner_item.deleted IS NULL
          AND item.deleted IS NULL
          AND EXISTS (
              SELECT 1 FROM barcodes barcode
              WHERE barcode.tenant_id=item.tenant_id
                AND barcode.item_id=item.id
                AND barcode.deleted IS NULL
                AND NULLIF(BTRIM(barcode.name), '') IS NOT NULL
          )
          AND ($3::TEXT IS NULL OR item.id::TEXT=$3 OR lower(item.description) LIKE lower($3) || '%')
        ORDER BY COALESCE(item.description, ''), item.id
        LIMIT $4
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(inventory_owner_id)
    .bind(search)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    let items = rows
        .iter()
        .map(|row| {
            Ok(InboundLoadEntryItem {
                item_id: row.try_get("id")?,
                description: row.try_get("description")?,
                uom: row.try_get("packaging_unit")?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(Some(items))
}

pub async fn plan_inbound_load(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &PlanInboundLoadCommand,
) -> AppResult<PlanInboundLoadResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, PLAN_INBOUND_LOAD_OPERATION, command)?;
    let plan = command.plan();
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_plan_visible_before_replay(&mut tx, access, &prepared, &scope).await?;

    if let Some(result) = prepared.replayed::<PlanInboundLoadResult>(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }

    if !scope.includes_facility(plan.facility_id().get())
        || !scope.includes_inventory_owner(plan.inventory_owner_id().get())
    {
        return Err(AppError::forbidden());
    }

    lock_reference(&mut tx, access, command).await?;
    lock_plan_resources(&mut tx, access, command).await?;

    let planned_at = now_iso();
    let execution_barcode = super::loads::generated_execution_barcode();
    let load_id = InboundLoadId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO loads
                (tenant_id, created, facility_id, inventory_owner_id, execution_barcode,
                 status, type, reference_number, invoice_number, carrier, trailer_number,
                 seal_number, dock_door_location_id, expected_time, appointment_time,
                 receive_completed)
            VALUES ($1,$2,$3,$4,$5,'planned','inbound',$6,$7,$8,$9,$10,$11,$12,$13,false)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(planned_at)
        .bind(plan.facility_id().get())
        .bind(plan.inventory_owner_id().get())
        .bind(&execution_barcode)
        .bind(plan.reference().as_str())
        .bind(plan.invoice_number())
        .bind(plan.carrier())
        .bind(plan.trailer_number())
        .bind(plan.seal_number())
        .bind(plan.receiving_location_id().get())
        .bind(plan.expected_at())
        .bind(plan.appointment_at())
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;

    let mut lines = Vec::with_capacity(plan.lines().len());
    let mut total_expected_quantity = 0_i64;
    for line in plan.lines() {
        let load_line_id = InboundLoadLineId::new(
            sqlx::query_scalar(
                r#"
                INSERT INTO load_lines
                    (tenant_id, created, load_id, item_id, expected_qty, lot, serial,
                     expiration, status)
                VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'pending')
                RETURNING id
                "#,
            )
            .bind(access.tenant_id.get())
            .bind(planned_at)
            .bind(load_id.get())
            .bind(line.item_id().get())
            .bind(line.expected_quantity().get())
            .bind(line.lot())
            .bind(line.serial())
            .bind(line.expiration())
            .fetch_one(&mut *tx)
            .await?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?;
        total_expected_quantity = total_expected_quantity
            .checked_add(line.expected_quantity().get())
            .ok_or_else(|| AppError::bad_request("total expected quantity exceeds i64"))?;
        lines.push(PlannedInboundLoadLineResult {
            load_line_id,
            item_id: line.item_id().get(),
            expected_quantity: line.expected_quantity().get(),
        });
    }

    sqlx::query(
        r#"
        INSERT INTO load_activity
            (tenant_id, created, load_id, user_id, action, message, metadata_json)
        VALUES ($1,$2,$3,$4,'planned','inbound load and expected contents planned',$5)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(planned_at)
    .bind(load_id.get())
    .bind(context.actor_id.get())
    .bind(
        serde_json::json!({
            "reference": plan.reference().as_str(),
            "line_count": lines.len(),
            "total_expected_quantity": total_expected_quantity,
        })
        .to_string(),
    )
    .execute(&mut *tx)
    .await?;

    enqueue_planned_event(
        &mut tx,
        access,
        context,
        command,
        load_id,
        &execution_barcode,
        total_expected_quantity,
        planned_at,
    )
    .await?;

    let result = PlanInboundLoadResult {
        load_id,
        execution_barcode,
        reference: plan.reference().as_str().to_owned(),
        status: PlannedInboundLoadStatus::Planned,
        lines,
        total_expected_quantity,
        planned_by: context.actor_id,
        planned_at,
    };
    insert_result(&mut tx, &prepared.completed_result(&result, None)?).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn arrive_inbound_load(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ArriveInboundLoadCommand,
) -> AppResult<ArriveInboundLoadResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, ARRIVE_INBOUND_LOAD_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_arrival_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<ArriveInboundLoadResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, facility_id, type, status, execution_barcode,
               dock_door_location_id
        FROM loads
        WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL
          AND ($3 OR facility_id=ANY($4))
          AND ($5 OR inventory_owner_id=ANY($6))
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.load_id().get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("inbound load"))?;
    let inventory_owner_id: i64 = row.try_get("inventory_owner_id")?;
    let facility_id: i64 = row.try_get("facility_id")?;
    let load_type: String = row.try_get("type")?;
    let status: String = row.try_get("status")?;
    let execution_barcode: String = row.try_get("execution_barcode")?;
    let receiving_location_id: Option<i64> = row.try_get("dock_door_location_id")?;
    if load_type != "inbound" {
        return Err(AppError::not_found("inbound load"));
    }
    let previous_status = match status.as_str() {
        "planned" => InboundLoadPreArrivalStatus::Planned,
        "scheduled" => InboundLoadPreArrivalStatus::Scheduled,
        _ => {
            return Err(AppError::conflict(
                "inbound load must be planned or scheduled before arrival",
            ))
        }
    };
    if !execution_barcode.eq_ignore_ascii_case(command.load_scan().as_str()) {
        return Err(AppError::bad_request(
            "load scan does not match inbound load",
        ));
    }
    let receiving_location_id = receiving_location_id
        .ok_or_else(|| AppError::conflict("inbound load has no assigned receiving location"))?;
    let location_barcode: Option<String> = sqlx::query_scalar(
        r#"
        SELECT barcode
        FROM locations
        WHERE tenant_id=$1 AND facility_id=$2 AND id=$3
          AND deleted IS NULL AND active AND receivable
          AND NULLIF(BTRIM(barcode), '') IS NOT NULL
        FOR SHARE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(facility_id)
    .bind(receiving_location_id)
    .fetch_optional(&mut *tx)
    .await?;
    let location_barcode = location_barcode
        .ok_or_else(|| AppError::conflict("assigned receiving location is no longer executable"))?;
    if !location_barcode.eq_ignore_ascii_case(command.receiving_location_scan().as_str()) {
        return Err(AppError::bad_request(
            "receiving location scan does not match the assigned location",
        ));
    }

    let server_time = now_iso();
    let arrived_at = command.arrived_at().copied().unwrap_or(server_time);
    validate_inbound_load_arrival(previous_status, arrived_at, server_time)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let arrival_id = InboundLoadArrivalId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO inbound_load_arrivals
                (tenant_id, inventory_owner_id, facility_id, load_id,
                 receiving_location_id, previous_status, observed_load_barcode,
                 observed_receiving_location_barcode, arrived_by_user_id, arrived_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(inventory_owner_id)
        .bind(facility_id)
        .bind(command.load_id().get())
        .bind(receiving_location_id)
        .bind(match previous_status {
            InboundLoadPreArrivalStatus::Planned => "planned",
            InboundLoadPreArrivalStatus::Scheduled => "scheduled",
        })
        .bind(command.load_scan().as_str())
        .bind(command.receiving_location_scan().as_str())
        .bind(context.actor_id.get())
        .bind(arrived_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;

    let updated = sqlx::query(
        r#"
        UPDATE loads
        SET status='arrived', arrival=$1, checked_in_by=$2
        WHERE tenant_id=$3 AND id=$4 AND status=$5 AND deleted IS NULL
        "#,
    )
    .bind(arrived_at)
    .bind(context.actor_id.get())
    .bind(access.tenant_id.get())
    .bind(command.load_id().get())
    .bind(match previous_status {
        InboundLoadPreArrivalStatus::Planned => "planned",
        InboundLoadPreArrivalStatus::Scheduled => "scheduled",
    })
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "inbound load state changed during arrival",
        ));
    }

    sqlx::query(
        r#"
        INSERT INTO load_activity
            (tenant_id, created, load_id, user_id, action, message, metadata_json)
        VALUES ($1,$2,$3,$4,'arrived','inbound load arrived at receiving location',$5)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(server_time)
    .bind(command.load_id().get())
    .bind(context.actor_id.get())
    .bind(
        serde_json::json!({
            "arrival_id": arrival_id.get(),
            "receiving_location_id": receiving_location_id,
            "previous_status": match previous_status {
                InboundLoadPreArrivalStatus::Planned => "planned",
                InboundLoadPreArrivalStatus::Scheduled => "scheduled",
            },
            "arrived_at": arrived_at,
        })
        .to_string(),
    )
    .execute(&mut *tx)
    .await?;

    let result = ArriveInboundLoadResult {
        arrival_id,
        load_id: command.load_id(),
        previous_status,
        status: ArrivedInboundLoadStatus::Arrived,
        receiving_location_id: LocationId::new(receiving_location_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        arrived_by: context.actor_id,
        arrived_at,
    };
    enqueue_arrived_event(
        &mut tx,
        access,
        context,
        inventory_owner_id,
        facility_id,
        &result,
    )
    .await?;
    insert_result(&mut tx, &prepared.completed_result(&result, None)?).await?;
    tx.commit().await?;
    Ok(result)
}

async fn require_stored_arrival_visible_before_replay(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    prepared: &PreparedCommand,
    scope: &super::access::ScopeBindings,
) -> AppResult<()> {
    let stored_load_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT (result_json->>'load_id')::BIGINT
        FROM command_idempotency_records
        WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(ARRIVE_INBOUND_LOAD_OPERATION)
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(load_id) = stored_load_id else {
        return Ok(());
    };
    let visible: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM loads
            WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL
              AND ($3 OR facility_id=ANY($4))
              AND ($5 OR inventory_owner_id=ANY($6))
        )
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(&mut **tx)
    .await?;
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("inbound load"))
    }
}

async fn enqueue_arrived_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    inventory_owner_id: i64,
    facility_id: i64,
    result: &ArriveInboundLoadResult,
) -> AppResult<()> {
    let ordering_key = format!("inbound-load:{}", result.load_id.get());
    let sequence: i64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE((
            SELECT last_sequence FROM outbox_aggregate_sequences
            WHERE tenant_id=$1 AND ordering_key=$2
        ), 0) + 1
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(&ordering_key)
    .fetch_one(&mut **tx)
    .await?;
    let event_key = format!("inbound-load:{}:arrived", result.load_id.get());
    let aggregate_id = result.load_id.to_string();
    let payload = serde_json::json!({
        "arrival_id": result.arrival_id.get(),
        "load_id": result.load_id.get(),
        "previous_status": match result.previous_status {
            InboundLoadPreArrivalStatus::Planned => "planned",
            InboundLoadPreArrivalStatus::Scheduled => "scheduled",
        },
        "status": "arrived",
        "inventory_owner_id": inventory_owner_id,
        "facility_id": facility_id,
        "receiving_location_id": result.receiving_location_id.get(),
        "arrived_by": result.arrived_by.get(),
        "arrived_at": result.arrived_at,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(
                wareboxes_domain::InventoryOwnerId::new(inventory_owner_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            facility_id: Some(
                wareboxes_domain::FacilityId::new(facility_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            actor_user_id: Some(context.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "inbound_load",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: "inbound.load.arrived",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.arrived_at,
        },
    )
    .await?;
    Ok(())
}

pub async fn start_inbound_load_unloading(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &StartInboundLoadUnloadingCommand,
) -> AppResult<StartInboundLoadUnloadingResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared =
        PreparedCommand::new_v1(context, START_INBOUND_LOAD_UNLOADING_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_unloading_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<StartInboundLoadUnloadingResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, facility_id, type, status, execution_barcode,
               dock_door_location_id, NULLIF(BTRIM(seal_number),'') AS seal_number
        FROM loads
        WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL
          AND ($3 OR facility_id=ANY($4))
          AND ($5 OR inventory_owner_id=ANY($6))
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.load_id().get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("inbound load"))?;
    let inventory_owner_id: i64 = row.try_get("inventory_owner_id")?;
    let facility_id: i64 = row.try_get("facility_id")?;
    let load_type: String = row.try_get("type")?;
    let status: String = row.try_get("status")?;
    let execution_barcode: String = row.try_get("execution_barcode")?;
    let receiving_location_id: Option<i64> = row.try_get("dock_door_location_id")?;
    let expected_seal: Option<String> = row.try_get("seal_number")?;
    if load_type != "inbound" {
        return Err(AppError::not_found("inbound load"));
    }
    if status != "arrived" {
        return Err(AppError::conflict(
            "inbound load must be arrived before unloading begins",
        ));
    }
    if !execution_barcode.eq_ignore_ascii_case(command.load_scan().as_str()) {
        return Err(AppError::bad_request(
            "load scan does not match inbound load",
        ));
    }
    match (expected_seal.as_deref(), command.seal_scan()) {
        (Some(expected), Some(observed)) if expected.eq_ignore_ascii_case(observed.as_str()) => {}
        (None, None) => {}
        (Some(_), None) => return Err(AppError::bad_request("planned seal scan is required")),
        _ => {
            return Err(AppError::bad_request(
                "seal scan does not match planned seal",
            ))
        }
    }
    let receiving_location_id = receiving_location_id
        .ok_or_else(|| AppError::conflict("inbound load has no assigned receiving location"))?;
    let location_barcode: Option<String> = sqlx::query_scalar(
        r#"
        SELECT barcode FROM locations
        WHERE tenant_id=$1 AND facility_id=$2 AND id=$3
          AND deleted IS NULL AND active AND receivable
          AND NULLIF(BTRIM(barcode),'') IS NOT NULL
        FOR SHARE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(facility_id)
    .bind(receiving_location_id)
    .fetch_optional(&mut *tx)
    .await?;
    let location_barcode = location_barcode
        .ok_or_else(|| AppError::conflict("assigned receiving location is no longer executable"))?;
    if !location_barcode.eq_ignore_ascii_case(command.receiving_location_scan().as_str()) {
        return Err(AppError::bad_request(
            "receiving location scan does not match the assigned location",
        ));
    }
    let server_time = now_iso();
    let started_at = command.started_at().copied().unwrap_or(server_time);
    validate_inbound_load_unloading_start(started_at, server_time)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let unloading_start_id = InboundLoadUnloadingStartId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO inbound_load_unloading_starts
                (tenant_id,inventory_owner_id,facility_id,load_id,receiving_location_id,
                 observed_load_barcode,observed_receiving_location_barcode,observed_seal,
                 started_by_user_id,started_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(inventory_owner_id)
        .bind(facility_id)
        .bind(command.load_id().get())
        .bind(receiving_location_id)
        .bind(command.load_scan().as_str())
        .bind(command.receiving_location_scan().as_str())
        .bind(command.seal_scan().map(|scan| scan.as_str()))
        .bind(context.actor_id.get())
        .bind(started_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    let updated = sqlx::query(
        r#"
        UPDATE loads SET status='receiving', actual_time=$1
        WHERE tenant_id=$2 AND id=$3 AND status='arrived' AND deleted IS NULL
        "#,
    )
    .bind(started_at)
    .bind(access.tenant_id.get())
    .bind(command.load_id().get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "inbound load state changed while unloading began",
        ));
    }
    sqlx::query(
        r#"
        INSERT INTO load_activity
            (tenant_id,created,load_id,user_id,action,message,metadata_json)
        VALUES ($1,$2,$3,$4,'unloading_started','inbound unloading started',$5)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(server_time)
    .bind(command.load_id().get())
    .bind(context.actor_id.get())
    .bind(
        serde_json::json!({
            "unloading_start_id": unloading_start_id.get(),
            "receiving_location_id": receiving_location_id,
            "seal_verified": expected_seal.is_some(),
            "started_at": started_at,
        })
        .to_string(),
    )
    .execute(&mut *tx)
    .await?;
    let result = StartInboundLoadUnloadingResult {
        unloading_start_id,
        load_id: command.load_id(),
        status: InboundLoadReceivingStatus::Receiving,
        receiving_location_id: LocationId::new(receiving_location_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        started_by: context.actor_id,
        started_at,
    };
    enqueue_unloading_started_event(
        &mut tx,
        access,
        context,
        inventory_owner_id,
        facility_id,
        expected_seal.is_some(),
        &result,
    )
    .await?;
    insert_result(&mut tx, &prepared.completed_result(&result, None)?).await?;
    tx.commit().await?;
    Ok(result)
}

async fn require_stored_unloading_visible_before_replay(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    prepared: &PreparedCommand,
    scope: &super::access::ScopeBindings,
) -> AppResult<()> {
    let stored_load_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT (result_json->>'load_id')::BIGINT FROM command_idempotency_records
        WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(START_INBOUND_LOAD_UNLOADING_OPERATION)
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(load_id) = stored_load_id else {
        return Ok(());
    };
    let visible: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(SELECT 1 FROM loads
          WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL
            AND ($3 OR facility_id=ANY($4))
            AND ($5 OR inventory_owner_id=ANY($6)))
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(&mut **tx)
    .await?;
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("inbound load"))
    }
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_unloading_started_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    inventory_owner_id: i64,
    facility_id: i64,
    seal_verified: bool,
    result: &StartInboundLoadUnloadingResult,
) -> AppResult<()> {
    let ordering_key = format!("inbound-load:{}", result.load_id.get());
    let sequence: i64 = sqlx::query_scalar(
        "SELECT COALESCE((SELECT last_sequence FROM outbox_aggregate_sequences WHERE tenant_id=$1 AND ordering_key=$2),0)+1",
    )
    .bind(access.tenant_id.get())
    .bind(&ordering_key)
    .fetch_one(&mut **tx)
    .await?;
    let event_key = format!("inbound-load:{}:unloading-started", result.load_id.get());
    let aggregate_id = result.load_id.to_string();
    let payload = serde_json::json!({
        "unloading_start_id": result.unloading_start_id.get(),
        "load_id": result.load_id.get(),
        "status": "receiving",
        "inventory_owner_id": inventory_owner_id,
        "facility_id": facility_id,
        "receiving_location_id": result.receiving_location_id.get(),
        "seal_verified": seal_verified,
        "started_by": result.started_by.get(),
        "started_at": result.started_at,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(
                wareboxes_domain::InventoryOwnerId::new(inventory_owner_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            facility_id: Some(
                wareboxes_domain::FacilityId::new(facility_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            actor_user_id: Some(context.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "inbound_load",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: "inbound.load.unloading_started",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.started_at,
        },
    )
    .await?;
    Ok(())
}

async fn require_stored_plan_visible_before_replay(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    prepared: &PreparedCommand,
    scope: &super::access::ScopeBindings,
) -> AppResult<()> {
    let stored_load_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT (result_json->>'load_id')::BIGINT
        FROM command_idempotency_records
        WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(PLAN_INBOUND_LOAD_OPERATION)
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(load_id) = stored_load_id else {
        return Ok(());
    };
    let visible: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM loads
            WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL
              AND ($3 OR facility_id=ANY($4))
              AND ($5 OR inventory_owner_id=ANY($6))
        )
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(&mut **tx)
    .await?;
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("inbound load"))
    }
}

async fn lock_reference(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    command: &PlanInboundLoadCommand,
) -> AppResult<()> {
    let plan = command.plan();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "inbound-load-reference:{}:{}:{}",
            access.tenant_id,
            plan.inventory_owner_id(),
            plan.reference()
        ))
        .execute(&mut **tx)
        .await?;
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM loads
            WHERE tenant_id=$1 AND inventory_owner_id=$2 AND type='inbound'
              AND reference_number=$3 AND deleted IS NULL
        )
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(plan.inventory_owner_id().get())
    .bind(plan.reference().as_str())
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Err(AppError::conflict(
            "inbound load reference already exists for client",
        ))
    } else {
        Ok(())
    }
}

async fn lock_plan_resources(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    command: &PlanInboundLoadCommand,
) -> AppResult<()> {
    let plan = command.plan();
    let owner_exists: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
            SELECT 1 FROM inventory_owners
            WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL
            FOR SHARE
        )"#,
    )
    .bind(access.tenant_id.get())
    .bind(plan.inventory_owner_id().get())
    .fetch_one(&mut **tx)
    .await?;
    if !owner_exists {
        return Err(AppError::not_found("inventory owner"));
    }

    let location_valid: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
            SELECT 1 FROM locations
            WHERE tenant_id=$1 AND facility_id=$2 AND id=$3
              AND deleted IS NULL AND active AND receivable
              AND NULLIF(BTRIM(barcode), '') IS NOT NULL
            FOR SHARE
        )"#,
    )
    .bind(access.tenant_id.get())
    .bind(plan.facility_id().get())
    .bind(plan.receiving_location_id().get())
    .fetch_one(&mut **tx)
    .await?;
    if !location_valid {
        return Err(AppError::conflict(
            "receiving location must be active, receivable, and barcoded in the selected facility",
        ));
    }

    let requested_ids = plan
        .lines()
        .iter()
        .map(|line| line.item_id().get())
        .collect::<HashSet<_>>();
    let mut requested_ids = requested_ids.into_iter().collect::<Vec<_>>();
    requested_ids.sort_unstable();
    let rows = sqlx::query(
        r#"
        SELECT item.id
        FROM items item
        INNER JOIN inventory_owner_items owner_item
          ON owner_item.tenant_id=item.tenant_id
         AND owner_item.item_id=item.id
         AND owner_item.inventory_owner_id=$2
         AND owner_item.deleted IS NULL
        WHERE item.tenant_id=$1 AND item.id=ANY($3) AND item.deleted IS NULL
          AND EXISTS (
              SELECT 1 FROM barcodes barcode
              WHERE barcode.tenant_id=item.tenant_id
                AND barcode.item_id=item.id
                AND barcode.deleted IS NULL
                AND NULLIF(BTRIM(barcode.name), '') IS NOT NULL
          )
        ORDER BY item.id
        FOR SHARE OF item, owner_item
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(plan.inventory_owner_id().get())
    .bind(&requested_ids)
    .fetch_all(&mut **tx)
    .await?;
    let active_ids = rows
        .iter()
        .map(|row| row.try_get::<i64, _>("id"))
        .collect::<Result<HashSet<_>, _>>()?;
    if active_ids.len() != requested_ids.len() {
        return Err(AppError::conflict(
            "one or more items are inactive, unbarcoded, or not linked to the client",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_planned_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    command: &PlanInboundLoadCommand,
    load_id: InboundLoadId,
    execution_barcode: &str,
    total_expected_quantity: i64,
    planned_at: chrono::DateTime<chrono::Utc>,
) -> AppResult<()> {
    let plan = command.plan();
    let event_key = format!("inbound-load:{}:planned", load_id.get());
    let aggregate_id = load_id.to_string();
    let ordering_key = format!("inbound-load:{}", load_id.get());
    let payload = serde_json::json!({
        "load_id": load_id.get(),
        "execution_barcode": execution_barcode,
        "reference": plan.reference().as_str(),
        "inventory_owner_id": plan.inventory_owner_id().get(),
        "facility_id": plan.facility_id().get(),
        "receiving_location_id": plan.receiving_location_id().get(),
        "status": "planned",
        "line_count": plan.lines().len(),
        "total_expected_quantity": total_expected_quantity,
        "expected_at": plan.expected_at(),
        "appointment_at": plan.appointment_at(),
        "lines": plan.lines().iter().map(|line| serde_json::json!({
            "item_id": line.item_id().get(),
            "expected_quantity": line.expected_quantity().get(),
            "lot": line.lot(),
            "serial": line.serial(),
            "expiration": line.expiration(),
        })).collect::<Vec<_>>(),
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(plan.inventory_owner_id()),
            facility_id: Some(plan.facility_id()),
            actor_user_id: Some(context.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "inbound_load",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: 1,
            event_type: "inbound.load.planned",
            schema_version: 1,
            payload: &payload,
            occurred_at: planned_at,
        },
    )
    .await?;
    Ok(())
}
