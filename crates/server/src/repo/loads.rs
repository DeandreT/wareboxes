//! Load planning and receiving workflows. Inbound receiving writes structured
//! load lines, journal entries, and balance projections in one transaction.

use std::collections::HashMap;

use sqlx::Row;
use wareboxes_core::models::{
    InventoryStatus, InventoryTransactionType, Load, LoadActivity, LoadFile, LoadFileCategory,
    LoadLine, LoadLineStatus, LoadNote, LoadStatus, LoadType, ReceiveLoadLineResult, TenantAccess,
    Timestamp,
};
use wareboxes_domain::TenantId;

use crate::db::{now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::ScopeBindings;
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry, JournalStart};
use crate::repo::{inventory, license_plates, orders};

fn parse_load_status(s: &str) -> AppResult<LoadStatus> {
    LoadStatus::parse(s)
        .ok_or_else(|| AppError::internal(format!("invalid load status in database: {s}")))
}

fn parse_load_type(s: &str) -> AppResult<LoadType> {
    LoadType::parse(s)
        .ok_or_else(|| AppError::internal(format!("invalid load type in database: {s}")))
}

fn parse_load_line_status(s: &str) -> AppResult<LoadLineStatus> {
    LoadLineStatus::parse(s)
        .ok_or_else(|| AppError::internal(format!("invalid load line status in database: {s}")))
}

fn parse_load_file_category(s: &str) -> AppResult<LoadFileCategory> {
    LoadFileCategory::parse(s)
        .ok_or_else(|| AppError::internal(format!("invalid load file category in database: {s}")))
}

fn load_line_status(expected: i64, received: i64, rejected: i64, missing: i64) -> LoadLineStatus {
    if received + rejected + missing >= expected {
        if received > 0 {
            LoadLineStatus::Received
        } else if rejected > 0 {
            LoadLineStatus::Rejected
        } else {
            LoadLineStatus::Missing
        }
    } else if received > 0 || rejected > 0 || missing > 0 {
        LoadLineStatus::Partial
    } else {
        LoadLineStatus::Pending
    }
}

fn map_load(row: &sqlx::postgres::PgRow) -> AppResult<Load> {
    Ok(Load {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        facility_id: row.try_get("facility_id")?,
        facility_name: row.try_get("facility_name")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        status: parse_load_status(row.try_get::<String, _>("status")?.as_str())?,
        r#type: parse_load_type(row.try_get::<String, _>("type")?.as_str())?,
        reference_number: row.try_get::<Option<String>, _>("reference_number")?,
        invoice_number: row.try_get::<Option<String>, _>("invoice_number")?,
        carrier: row.try_get::<Option<String>, _>("carrier")?,
        trailer_number: row.try_get::<Option<String>, _>("trailer_number")?,
        seal_number: row.try_get::<Option<String>, _>("seal_number")?,
        dock_door_location_id: row.try_get::<Option<i64>, _>("dock_door_location_id")?,
        expected_time: row.try_get("expected_time")?,
        appointment_time: row.try_get("appointment_time")?,
        actual_time: row.try_get("actual_time")?,
        arrival: row.try_get("arrival")?,
        departure: row.try_get("departure")?,
        rejected: row.try_get("rejected")?,
        receive_completed: row.try_get("receive_completed")?,
        closed: row.try_get("closed")?,
        checked_in_by: row.try_get::<Option<i64>, _>("checked_in_by")?,
        closed_by: row.try_get::<Option<i64>, _>("closed_by")?,
        notes: Vec::new(),
        files: Vec::new(),
        lines: Vec::new(),
        orders: Vec::new(),
        activity: Vec::new(),
    })
}

fn map_file(r: &sqlx::postgres::PgRow) -> AppResult<LoadFile> {
    Ok(LoadFile {
        id: r.try_get("id")?,
        tenant_id: TenantId::new(r.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: r.try_get("created")?,
        deleted: r.try_get("deleted")?,
        load_id: r.try_get("load_id")?,
        original_name: r.try_get("original_name")?,
        name: r.try_get("name")?,
        path: r.try_get("path")?,
        content_type: r.try_get::<Option<String>, _>("content_type")?,
        category: parse_load_file_category(r.try_get::<String, _>("category")?.as_str())?,
    })
}

fn map_note(r: &sqlx::postgres::PgRow) -> AppResult<LoadNote> {
    Ok(LoadNote {
        id: r.try_get("id")?,
        tenant_id: TenantId::new(r.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: r.try_get("created")?,
        deleted: r.try_get("deleted")?,
        load_id: r.try_get("load_id")?,
        note: r.try_get("note")?,
    })
}

fn map_line(r: &sqlx::postgres::PgRow) -> AppResult<LoadLine> {
    Ok(LoadLine {
        id: r.try_get("id")?,
        tenant_id: TenantId::new(r.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: r.try_get("created")?,
        deleted: r.try_get("deleted")?,
        load_id: r.try_get("load_id")?,
        item_id: r.try_get("item_id")?,
        sku_id: r.try_get::<Option<i64>, _>("sku_id")?,
        expected_qty: r.try_get("expected_qty")?,
        received_qty: r.try_get("received_qty")?,
        rejected_qty: r.try_get("rejected_qty")?,
        missing_qty: r.try_get("missing_qty")?,
        missing_confirmed_by: r.try_get::<Option<i64>, _>("missing_confirmed_by")?,
        missing_confirmed_at: r.try_get("missing_confirmed_at")?,
        lot: r.try_get::<Option<String>, _>("lot")?,
        serial: r.try_get::<Option<String>, _>("serial")?,
        expiration: r.try_get("expiration")?,
        status: parse_load_line_status(r.try_get::<String, _>("status")?.as_str())?,
    })
}

fn map_activity(r: &sqlx::postgres::PgRow) -> AppResult<LoadActivity> {
    Ok(LoadActivity {
        id: r.try_get("id")?,
        tenant_id: TenantId::new(r.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: r.try_get("created")?,
        deleted: r.try_get("deleted")?,
        load_id: r.try_get("load_id")?,
        user_id: r.try_get::<Option<i64>, _>("user_id")?,
        action: r.try_get("action")?,
        message: r.try_get::<Option<String>, _>("message")?,
        metadata_json: r.try_get::<Option<String>, _>("metadata_json")?,
    })
}

pub async fn get_loads(
    db: &Db,
    tenant_id: TenantId,
    show_deleted: bool,
    show_deleted_notes: bool,
) -> AppResult<Vec<Load>> {
    let rows = sqlx::query(
        r#"
        SELECT l.id, l.tenant_id, l.created, l.deleted, l.facility_id, facility.name AS facility_name,
               l.inventory_owner_id, a.name AS inventory_owner_name, l.status, l.type, l.reference_number,
               l.invoice_number, l.carrier, l.trailer_number, l.seal_number, l.dock_door_location_id,
               l.expected_time, l.appointment_time, l.actual_time, l.arrival, l.departure, l.rejected,
               l.receive_completed, l.closed, l.checked_in_by, l.closed_by
        FROM loads l
        INNER JOIN facilities facility
            ON facility.tenant_id = l.tenant_id AND facility.id = l.facility_id
        INNER JOIN inventory_owners a
            ON a.tenant_id = l.tenant_id AND a.id = l.inventory_owner_id
        WHERE l.tenant_id = $1 AND ($2 OR l.deleted IS NULL)
        ORDER BY l.id DESC
        "#,
    )
    .bind(tenant_id.get())
    .bind(show_deleted)
    .fetch_all(db)
    .await?;

    let note_rows = sqlx::query(
        "SELECT id, tenant_id, created, deleted, load_id, note FROM load_notes WHERE tenant_id = $1 AND ($2 OR deleted IS NULL) ORDER BY id",
    )
    .bind(tenant_id.get())
    .bind(show_deleted_notes)
    .fetch_all(db)
    .await?;
    let mut notes: HashMap<i64, Vec<LoadNote>> = HashMap::new();
    for r in &note_rows {
        let note = map_note(r)?;
        notes.entry(note.load_id).or_default().push(note);
    }

    let file_rows = sqlx::query(
        "SELECT id, tenant_id, created, deleted, load_id, original_name, name, path, content_type, category FROM load_files WHERE tenant_id = $1 AND deleted IS NULL",
    )
    .bind(tenant_id.get())
    .fetch_all(db)
    .await?;
    let mut files: HashMap<i64, Vec<LoadFile>> = HashMap::new();
    for r in &file_rows {
        let file = map_file(r)?;
        files.entry(file.load_id).or_default().push(file);
    }

    let line_rows = sqlx::query(
        r#"
        SELECT id, tenant_id, created, deleted, load_id, item_id, sku_id, expected_qty, received_qty,
               rejected_qty, missing_qty, missing_confirmed_by, missing_confirmed_at, lot, serial, expiration, status
        FROM load_lines
        WHERE tenant_id = $1 AND deleted IS NULL
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .fetch_all(db)
    .await?;
    let mut lines: HashMap<i64, Vec<LoadLine>> = HashMap::new();
    for r in &line_rows {
        let line = map_line(r)?;
        lines.entry(line.load_id).or_default().push(line);
    }

    let mut orders = orders::orders_by_load(db, tenant_id).await?;

    let activity_rows = sqlx::query(
        "SELECT id, tenant_id, created, deleted, load_id, user_id, action, message, metadata_json FROM load_activity WHERE tenant_id = $1 AND deleted IS NULL ORDER BY id",
    )
    .bind(tenant_id.get())
    .fetch_all(db)
    .await?;
    let mut activity: HashMap<i64, Vec<LoadActivity>> = HashMap::new();
    for r in &activity_rows {
        let event = map_activity(r)?;
        activity.entry(event.load_id).or_default().push(event);
    }

    rows.iter()
        .map(|r| {
            let mut l = map_load(r)?;
            l.notes = notes.remove(&l.id).unwrap_or_default();
            l.files = files.remove(&l.id).unwrap_or_default();
            l.lines = lines.remove(&l.id).unwrap_or_default();
            l.orders = orders.remove(&l.id).unwrap_or_default();
            l.activity = activity.remove(&l.id).unwrap_or_default();
            Ok(l)
        })
        .collect()
}

pub async fn get_load_summaries(
    db: &Db,
    tenant_id: TenantId,
    show_deleted: bool,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<Load>> {
    get_load_summaries_with_scope(
        db,
        tenant_id,
        &ScopeBindings::unrestricted(),
        show_deleted,
        limit,
        offset,
    )
    .await
}

pub async fn get_load_summaries_in_scope(
    db: &Db,
    access: &TenantAccess,
    show_deleted: bool,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<Load>> {
    let scope = ScopeBindings::for_access(access);
    get_load_summaries_with_scope(db, access.tenant_id, &scope, show_deleted, limit, offset).await
}

async fn get_load_summaries_with_scope(
    db: &Db,
    tenant_id: TenantId,
    scope: &ScopeBindings,
    show_deleted: bool,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<Load>> {
    let rows = sqlx::query(
        r#"
        SELECT l.id, l.tenant_id, l.created, l.deleted, l.facility_id, facility.name AS facility_name,
               l.inventory_owner_id, a.name AS inventory_owner_name, l.status, l.type, l.reference_number,
               l.invoice_number, l.carrier, l.trailer_number, l.seal_number, l.dock_door_location_id,
               l.expected_time, l.appointment_time, l.actual_time, l.arrival, l.departure, l.rejected,
               l.receive_completed, l.closed, l.checked_in_by, l.closed_by
        FROM loads l
        INNER JOIN facilities facility
            ON facility.tenant_id = l.tenant_id AND facility.id = l.facility_id
        INNER JOIN inventory_owners a
            ON a.tenant_id = l.tenant_id AND a.id = l.inventory_owner_id
        WHERE l.tenant_id = $1
          AND ($2 OR l.deleted IS NULL)
          AND ($3 OR l.facility_id = ANY($4))
          AND ($5 OR l.inventory_owner_id = ANY($6))
        ORDER BY l.created DESC, l.id DESC
        LIMIT $7 OFFSET $8
        "#,
    )
        .bind(tenant_id.get())
        .bind(show_deleted)
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .bind(limit)
        .bind(offset)
        .fetch_all(db)
        .await?;
    let load_ids = rows
        .iter()
        .map(|r| r.try_get("id"))
        .collect::<Result<Vec<i64>, _>>()?;
    if load_ids.is_empty() {
        return Ok(Vec::new());
    }

    let line_rows = sqlx::query(
        r#"
        SELECT id, tenant_id, created, deleted, load_id, item_id, sku_id, expected_qty, received_qty,
               rejected_qty, missing_qty, missing_confirmed_by, missing_confirmed_at, lot, serial, expiration, status
        FROM load_lines
        WHERE tenant_id = $1 AND deleted IS NULL AND load_id = ANY($2)
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(&load_ids)
    .fetch_all(db)
    .await?;
    let mut lines: HashMap<i64, Vec<LoadLine>> = HashMap::new();
    for r in &line_rows {
        let line = map_line(r)?;
        lines.entry(line.load_id).or_default().push(line);
    }

    rows.iter()
        .map(|r| {
            let mut l = map_load(r)?;
            l.lines = lines.remove(&l.id).unwrap_or_default();
            Ok(l)
        })
        .collect()
}

pub async fn get_load(
    db: &Db,
    tenant_id: TenantId,
    load_id: i64,
    show_deleted_notes: bool,
) -> AppResult<Option<Load>> {
    get_load_with_scope(
        db,
        tenant_id,
        &ScopeBindings::unrestricted(),
        load_id,
        show_deleted_notes,
    )
    .await
}

pub async fn get_load_in_scope(
    db: &Db,
    access: &TenantAccess,
    load_id: i64,
    show_deleted_notes: bool,
) -> AppResult<Option<Load>> {
    let scope = ScopeBindings::for_access(access);
    get_load_with_scope(db, access.tenant_id, &scope, load_id, show_deleted_notes).await
}

async fn get_load_with_scope(
    db: &Db,
    tenant_id: TenantId,
    scope: &ScopeBindings,
    load_id: i64,
    show_deleted_notes: bool,
) -> AppResult<Option<Load>> {
    let row = sqlx::query(
        r#"
        SELECT l.id, l.tenant_id, l.created, l.deleted, l.facility_id, facility.name AS facility_name,
               l.inventory_owner_id, a.name AS inventory_owner_name, l.status, l.type, l.reference_number,
               l.invoice_number, l.carrier, l.trailer_number, l.seal_number, l.dock_door_location_id,
               l.expected_time, l.appointment_time, l.actual_time, l.arrival, l.departure, l.rejected,
               l.receive_completed, l.closed, l.checked_in_by, l.closed_by
        FROM loads l
        INNER JOIN facilities facility
            ON facility.tenant_id = l.tenant_id AND facility.id = l.facility_id
        INNER JOIN inventory_owners a
            ON a.tenant_id = l.tenant_id AND a.id = l.inventory_owner_id
        WHERE l.tenant_id = $1
          AND l.id = $2
          AND l.deleted IS NULL
          AND ($3 OR l.facility_id = ANY($4))
          AND ($5 OR l.inventory_owner_id = ANY($6))
        "#,
    )
    .bind(tenant_id.get())
    .bind(load_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(db)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let mut load = map_load(&row)?;

    let note_rows = sqlx::query(
        "SELECT id, tenant_id, created, deleted, load_id, note FROM load_notes WHERE tenant_id = $1 AND load_id = $2 AND ($3 OR deleted IS NULL) ORDER BY id",
    )
    .bind(tenant_id.get())
    .bind(load_id)
    .bind(show_deleted_notes)
    .fetch_all(db)
    .await?;
    for r in &note_rows {
        load.notes.push(map_note(r)?);
    }

    let file_rows = sqlx::query(
        "SELECT id, tenant_id, created, deleted, load_id, original_name, name, path, content_type, category FROM load_files WHERE tenant_id = $1 AND load_id = $2 AND deleted IS NULL ORDER BY id",
    )
    .bind(tenant_id.get())
    .bind(load_id)
    .fetch_all(db)
    .await?;
    for r in &file_rows {
        load.files.push(map_file(r)?);
    }

    let line_rows = sqlx::query(
        r#"
        SELECT id, tenant_id, created, deleted, load_id, item_id, sku_id, expected_qty, received_qty,
               rejected_qty, missing_qty, missing_confirmed_by, missing_confirmed_at, lot, serial, expiration, status
        FROM load_lines
        WHERE tenant_id = $1 AND load_id = $2 AND deleted IS NULL
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(load_id)
    .fetch_all(db)
    .await?;
    for r in &line_rows {
        load.lines.push(map_line(r)?);
    }

    load.orders = orders::orders_for_load(db, tenant_id, load_id).await?;

    let activity_rows = sqlx::query(
        "SELECT id, tenant_id, created, deleted, load_id, user_id, action, message, metadata_json FROM load_activity WHERE tenant_id = $1 AND load_id = $2 AND deleted IS NULL ORDER BY id",
    )
    .bind(tenant_id.get())
    .bind(load_id)
    .fetch_all(db)
    .await?;
    for r in &activity_rows {
        load.activity.push(map_activity(r)?);
    }

    Ok(Some(load))
}

pub async fn active_load_exists(db: &Db, tenant_id: TenantId, id: i64) -> AppResult<bool> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM loads WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL)",
    )
    .bind(tenant_id.get())
    .bind(id)
    .fetch_one(db)
    .await?;
    Ok(exists)
}

#[allow(clippy::too_many_arguments)]
pub async fn add_load(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    facility_id: i64,
    inventory_owner_id: i64,
    load_type: LoadType,
    reference_number: Option<&str>,
    invoice_number: Option<&str>,
    carrier: Option<&str>,
    trailer_number: Option<&str>,
    seal_number: Option<&str>,
    dock_door_location_id: Option<i64>,
    expected_time: Option<Timestamp>,
    appointment_time: Option<Timestamp>,
) -> AppResult<i64> {
    let now = now_iso();
    let mut tx = db.begin().await?;
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO loads
            (tenant_id, created, facility_id, inventory_owner_id, status, type, reference_number, invoice_number, carrier,
             trailer_number, seal_number, dock_door_location_id, expected_time, appointment_time,
             receive_completed)
        VALUES ($1, $2, $3, $4, 'planned', $5, $6, $7, $8, $9, $10, $11, $12, $13, false)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(now)
    .bind(facility_id)
    .bind(inventory_owner_id)
    .bind(load_type.as_str())
    .bind(reference_number)
    .bind(invoice_number)
    .bind(carrier)
    .bind(trailer_number)
    .bind(seal_number)
    .bind(dock_door_location_id)
    .bind(expected_time)
    .bind(appointment_time)
    .fetch_one(&mut *tx)
    .await?;

    insert_activity_tx(
        &mut tx,
        tenant_id,
        id,
        Some(user_id),
        "created",
        Some("load created"),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}

#[allow(clippy::too_many_arguments)]
pub async fn update_load(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    id: i64,
    status: Option<LoadStatus>,
    load_type: Option<LoadType>,
    reference_number: Option<&str>,
    invoice_number: Option<&str>,
    carrier: Option<&str>,
    trailer_number: Option<&str>,
    seal_number: Option<&str>,
    dock_door_location_id: Option<i64>,
    expected_time: Option<Timestamp>,
    appointment_time: Option<Timestamp>,
    mut actual_time: Option<Timestamp>,
    mut arrival: Option<Timestamp>,
    departure: Option<Timestamp>,
    mut rejected: Option<Timestamp>,
    receive_completed: Option<bool>,
    mut closed: Option<Timestamp>,
) -> AppResult<bool> {
    let mut tx = db.begin().await?;
    let current: Option<String> = sqlx::query_scalar(
        "SELECT status FROM loads WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL FOR UPDATE",
    )
    .bind(tenant_id.get())
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(current) = current else {
        return Ok(false);
    };

    let current = parse_load_status(&current)?;
    if current.is_terminal() {
        return Err(AppError::conflict("cannot update a terminal load"));
    }
    let next_status = match status {
        Some(s) => {
            if s == LoadStatus::Closed {
                ensure_load_resolved_tx(&mut tx, tenant_id, id).await?;
            }
            if !current.can_transition_to(s) {
                return Err(AppError::bad_request(format!(
                    "invalid load status transition from {current} to {s}"
                )));
            }
            Some(s)
        }
        None => None,
    };
    let now = now_iso();
    match next_status {
        Some(LoadStatus::Arrived) => {
            arrival.get_or_insert(now);
        }
        Some(LoadStatus::Receiving) => {
            actual_time.get_or_insert(now);
        }
        Some(LoadStatus::Rejected) => {
            rejected.get_or_insert(now);
        }
        Some(LoadStatus::Closed) => {
            closed.get_or_insert(now);
        }
        _ => {}
    };

    let closed_by = if next_status == Some(LoadStatus::Closed) || closed.is_some() {
        Some(user_id)
    } else {
        None
    };
    let checked_in_by = if next_status == Some(LoadStatus::Arrived) || arrival.is_some() {
        Some(user_id)
    } else {
        None
    };

    let res = sqlx::query(
        r#"
        UPDATE loads SET
            status = COALESCE($1, status),
            type = COALESCE($2, type),
            reference_number = COALESCE($3, reference_number),
            invoice_number = COALESCE($4, invoice_number),
            carrier = COALESCE($5, carrier),
            trailer_number = COALESCE($6, trailer_number),
            seal_number = COALESCE($7, seal_number),
            dock_door_location_id = COALESCE($8, dock_door_location_id),
            expected_time = COALESCE($9, expected_time),
            appointment_time = COALESCE($10, appointment_time),
            actual_time = COALESCE($11, actual_time),
            arrival = COALESCE($12, arrival),
            departure = COALESCE($13, departure),
            rejected = COALESCE($14, rejected),
            receive_completed = COALESCE($15, receive_completed),
            closed = COALESCE($16, closed),
            checked_in_by = COALESCE($17, checked_in_by),
            closed_by = COALESCE($18, closed_by)
        WHERE tenant_id = $19 AND id = $20 AND deleted IS NULL
        "#,
    )
    .bind(next_status.map(|s| s.as_str()))
    .bind(load_type.map(|t| t.as_str()))
    .bind(reference_number)
    .bind(invoice_number)
    .bind(carrier)
    .bind(trailer_number)
    .bind(seal_number)
    .bind(dock_door_location_id)
    .bind(expected_time)
    .bind(appointment_time)
    .bind(actual_time)
    .bind(arrival)
    .bind(departure)
    .bind(rejected)
    .bind(receive_completed)
    .bind(closed)
    .bind(checked_in_by)
    .bind(closed_by)
    .bind(tenant_id.get())
    .bind(id)
    .execute(&mut *tx)
    .await?;

    if res.rows_affected() > 0 {
        insert_activity_tx(
            &mut tx,
            tenant_id,
            id,
            Some(user_id),
            "updated",
            Some("load updated"),
            None,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_load_deleted(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    id: i64,
    deleted: bool,
) -> AppResult<bool> {
    let mut tx = db.begin().await?;
    let res = sqlx::query(
        r#"
        UPDATE loads SET deleted = $1
        WHERE tenant_id = $2 AND id = $3
          AND (($4 AND deleted IS NULL) OR (NOT $4 AND deleted IS NOT NULL))
        "#,
    )
    .bind(if deleted { Some(now_iso()) } else { None })
    .bind(tenant_id.get())
    .bind(id)
    .bind(deleted)
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() > 0 {
        insert_activity_tx(
            &mut tx,
            tenant_id,
            id,
            Some(user_id),
            if deleted { "deleted" } else { "restored" },
            Some(if deleted {
                "load deleted"
            } else {
                "load restored"
            }),
            None,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}

pub async fn add_note(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    load_id: i64,
    note: &str,
) -> AppResult<i64> {
    let mut tx = db.begin().await?;
    let id: Option<i64> = sqlx::query_scalar(
        r#"
        INSERT INTO load_notes (tenant_id, created, load_id, note)
        SELECT $1, $2, load.id, $4
        FROM loads load
        WHERE load.tenant_id = $1 AND load.id = $3 AND load.deleted IS NULL
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(now_iso())
    .bind(load_id)
    .bind(note)
    .fetch_optional(&mut *tx)
    .await?;
    let id = id.ok_or_else(|| AppError::not_found("load"))?;
    insert_activity_tx(
        &mut tx,
        tenant_id,
        load_id,
        Some(user_id),
        "note_added",
        Some(note),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn set_load_note_deleted(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    note_id: i64,
    deleted: bool,
) -> AppResult<bool> {
    let mut tx = db.begin().await?;
    let load_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT load_id FROM load_notes
        WHERE tenant_id = $1 AND id = $2
          AND (($3 AND deleted IS NULL) OR (NOT $3 AND deleted IS NOT NULL))
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(note_id)
    .bind(deleted)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    let Some(load_id) = load_id else {
        return Ok(false);
    };

    let res = sqlx::query("UPDATE load_notes SET deleted = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(if deleted { Some(now_iso()) } else { None })
        .bind(tenant_id.get())
        .bind(note_id)
        .execute(&mut *tx)
        .await?;
    if res.rows_affected() > 0 {
        insert_activity_tx(
            &mut tx,
            tenant_id,
            load_id,
            Some(user_id),
            if deleted {
                "note_deleted"
            } else {
                "note_restored"
            },
            Some(&format!("load note {note_id}")),
            None,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}

#[allow(clippy::too_many_arguments)]
pub async fn add_line(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    load_id: i64,
    item_id: i64,
    sku_id: Option<i64>,
    expected_qty: i64,
    lot: Option<&str>,
    serial: Option<&str>,
    expiration: Option<Timestamp>,
) -> AppResult<i64> {
    if expected_qty <= 0 {
        return Err(AppError::bad_request("expected quantity must be positive"));
    }
    let mut tx = db.begin().await?;
    let load_status: Option<String> = sqlx::query_scalar(
        r#"
        SELECT load.status
        FROM loads load
        INNER JOIN items item
            ON item.tenant_id = load.tenant_id AND item.id = $3 AND item.deleted IS NULL
        WHERE load.tenant_id = $1 AND load.id = $2 AND load.deleted IS NULL
          AND ($4::BIGINT IS NULL OR EXISTS (
              SELECT 1 FROM skus sku
              WHERE sku.tenant_id = $1 AND sku.item_id = item.id
                AND sku.id = $4 AND sku.deleted IS NULL
          ))
        FOR UPDATE OF load
        "#,
    )
    .bind(tenant_id.get())
    .bind(load_id)
    .bind(item_id)
    .bind(sku_id)
    .fetch_optional(&mut *tx)
    .await?;
    let load_status = load_status
        .ok_or_else(|| AppError::not_found("load, item, or SKU in the selected tenant"))?;
    let load_status = parse_load_status(&load_status)?;
    if load_status.is_terminal() {
        return Err(AppError::conflict("cannot add lines to a terminal load"));
    }
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO load_lines
            (tenant_id, created, load_id, item_id, sku_id, expected_qty, lot, serial, expiration, status)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'pending')
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(now_iso())
    .bind(load_id)
    .bind(item_id)
    .bind(sku_id)
    .bind(expected_qty)
    .bind(lot)
    .bind(serial)
    .bind(expiration)
    .fetch_one(&mut *tx)
    .await?;
    insert_activity_tx(
        &mut tx,
        tenant_id,
        load_id,
        Some(user_id),
        "line_added",
        Some("load line added"),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}

#[allow(clippy::too_many_arguments)]
pub async fn receive_line(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    load_line_id: i64,
    to_location_id: i64,
    received_qty: i64,
    rejected_qty: i64,
    missing_qty: i64,
    license_plate_id: Option<i64>,
    license_plate_barcode: Option<&str>,
    lot: Option<&str>,
    serial: Option<&str>,
    expiration: Option<Timestamp>,
    reason: Option<&str>,
    idempotency_key: &str,
) -> AppResult<ReceiveLoadLineResult> {
    if received_qty < 0 || rejected_qty < 0 || missing_qty < 0 {
        return Err(AppError::bad_request(
            "received, rejected, and missing quantities cannot be negative",
        ));
    }
    if received_qty + rejected_qty + missing_qty == 0 {
        return Err(AppError::bad_request(
            "received, rejected, or missing quantity is required",
        ));
    }
    let now = now_iso();
    let mut tx = db.begin().await?;

    let request_hash = inventory_journal::request_hash(&(
        load_line_id,
        to_location_id,
        received_qty,
        rejected_qty,
        missing_qty,
        license_plate_id,
        license_plate_barcode,
        lot,
        serial,
        &expiration,
        reason,
    ))?;
    if let Some(result) = inventory_journal::replayed_result::<ReceiveLoadLineResult>(
        &mut tx,
        tenant_id,
        "receive_load_line",
        Some(idempotency_key),
        &request_hash,
    )
    .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let row = sqlx::query(
        r#"
        SELECT ll.id, ll.load_id, ll.item_id, ll.expected_qty, ll.received_qty, ll.rejected_qty,
               ll.missing_qty,
               COALESCE($1, ll.lot) AS lot, COALESCE($2, ll.serial) AS serial,
               COALESCE($3, ll.expiration) AS expiration, l.status AS load_status,
               l.facility_id AS load_facility_id, l.inventory_owner_id,
               owner.tenant_id, item.packaging_unit AS uom
        FROM load_lines ll
        INNER JOIN loads l ON l.tenant_id = ll.tenant_id AND l.id = ll.load_id
        INNER JOIN inventory_owners owner
            ON owner.tenant_id = l.tenant_id AND owner.id = l.inventory_owner_id
        INNER JOIN items item ON item.id = ll.item_id AND item.tenant_id = owner.tenant_id
        WHERE ll.tenant_id = $5 AND ll.id = $4
          AND ll.deleted IS NULL AND l.deleted IS NULL
        FOR UPDATE OF ll, l
        "#,
    )
    .bind(lot)
    .bind(serial)
    .bind(expiration)
    .bind(load_line_id)
    .bind(tenant_id.get())
    .fetch_one(&mut *tx)
    .await?;

    let load_id: i64 = row.try_get("load_id")?;
    let tenant_id = TenantId::new(row.try_get("tenant_id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let inventory_owner_id: i64 = row.try_get("inventory_owner_id")?;
    let load_facility_id: i64 = row.try_get("load_facility_id")?;
    let item_id: i64 = row.try_get("item_id")?;
    let uom: String = row.try_get("uom")?;
    let expected: i64 = row.try_get("expected_qty")?;
    let prior_received: i64 = row.try_get("received_qty")?;
    let prior_rejected: i64 = row.try_get("rejected_qty")?;
    let prior_missing: i64 = row.try_get("missing_qty")?;
    let load_status = parse_load_status(row.try_get::<String, _>("load_status")?.as_str())?;
    let final_lot = row.try_get::<Option<String>, _>("lot")?;
    let final_serial = row.try_get::<Option<String>, _>("serial")?;
    let final_expiration: Option<Timestamp> = row.try_get("expiration")?;

    if load_status.is_terminal() || load_status == LoadStatus::Rejected {
        return Err(AppError::conflict("cannot receive against a terminal load"));
    }
    if !matches!(load_status, LoadStatus::Arrived | LoadStatus::Receiving) {
        return Err(AppError::conflict(
            "load must be arrived before receiving can begin",
        ));
    }
    let new_received = prior_received + received_qty;
    let new_rejected = prior_rejected + rejected_qty;
    let new_missing = prior_missing + missing_qty;
    if new_received + new_rejected + new_missing > expected {
        return Err(AppError::conflict(
            "cannot receive, reject, or mark missing more than expected quantity",
        ));
    }

    let line_status = load_line_status(expected, new_received, new_rejected, new_missing);
    let missing_confirmed_by = if missing_qty > 0 { Some(user_id) } else { None };
    let missing_confirmed_at = if missing_qty > 0 { Some(now) } else { None };
    sqlx::query(
        r#"
        UPDATE load_lines
        SET received_qty = $1,
            rejected_qty = $2,
            missing_qty = $3,
            missing_confirmed_by = COALESCE($4, missing_confirmed_by),
            missing_confirmed_at = COALESCE($5, missing_confirmed_at),
            lot = COALESCE($6, lot),
            serial = COALESCE($7, serial),
            expiration = COALESCE($8, expiration),
            status = $9
        WHERE tenant_id = $10 AND id = $11
        "#,
    )
    .bind(new_received)
    .bind(new_rejected)
    .bind(new_missing)
    .bind(missing_confirmed_by)
    .bind(missing_confirmed_at)
    .bind(lot)
    .bind(serial)
    .bind(expiration)
    .bind(line_status.as_str())
    .bind(tenant_id.get())
    .bind(load_line_id)
    .execute(&mut *tx)
    .await?;

    let mut inventory_transaction_id = None;
    let resolved_license_plate_id = license_plates::find_or_create_license_plate_tx(
        &mut tx,
        tenant_id,
        inventory_owner_id,
        license_plate_barcode,
        license_plate_id,
        to_location_id,
    )
    .await?;

    if received_qty > 0 {
        sqlx::query(
            r#"
            INSERT INTO inventory_owner_items
                (tenant_id, created, inventory_owner_id, item_id)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (tenant_id, inventory_owner_id, item_id)
            DO UPDATE SET deleted = NULL
            "#,
        )
        .bind(tenant_id.get())
        .bind(now)
        .bind(inventory_owner_id)
        .bind(item_id)
        .execute(&mut *tx)
        .await?;

        let batch_id: i64 = sqlx::query_scalar(
            "INSERT INTO item_batches (tenant_id, inventory_owner_id, created, item_id, uom, load_id, lot, serial, expiration) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
        )
        .bind(tenant_id.get())
        .bind(inventory_owner_id)
        .bind(now)
        .bind(item_id)
        .bind(&uom)
        .bind(load_id)
        .bind(final_lot.as_deref())
        .bind(final_serial.as_deref())
        .bind(final_expiration)
        .fetch_one(&mut *tx)
        .await?;

        inventory::ensure_location_accepts_batch_tx(
            &mut tx,
            tenant_id,
            inventory_owner_id,
            to_location_id,
            batch_id,
        )
        .await?;

        let facility_id: Option<i64> = sqlx::query_scalar(
            r#"
            SELECT facility_id
            FROM locations
            WHERE tenant_id = $1
              AND id = $2
              AND facility_id = $3
              AND deleted IS NULL
              AND active
              AND receivable
            "#,
        )
        .bind(tenant_id.get())
        .bind(to_location_id)
        .bind(load_facility_id)
        .fetch_optional(&mut *tx)
        .await?;
        let facility_id = facility_id.ok_or_else(|| {
            AppError::bad_request("receiving location must be active in the load facility")
        })?;

        let transaction_id = match inventory_journal::begin_transaction(
            &mut tx,
            &JournalCommand {
                tenant_id,
                inventory_owner_id,
                actor_user_id: user_id,
                transaction_type: InventoryTransactionType::Receive,
                reason,
                reference_type: Some("load"),
                reference_id: Some(load_id),
                correlation_id: None,
                operation: "receive_load_line",
                idempotency_key: Some(idempotency_key),
                request_hash: &request_hash,
                record_idempotency: false,
            },
        )
        .await?
        {
            JournalStart::New(id) => id,
            JournalStart::Replay(_) => {
                return Err(AppError::internal(
                    "load receipt journal unexpectedly replayed",
                ));
            }
        };
        inventory_transaction_id = Some(transaction_id);

        if let Some(license_plate_id) = resolved_license_plate_id {
            sqlx::query(
                r#"
                INSERT INTO inventory_balances
                    (tenant_id, inventory_owner_id, created, modified, facility_id,
                     location_id, license_plate_id, item_batch_id, item_id, uom,
                     status, qty_on_hand, qty_reserved)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'available', $11, 0)
                ON CONFLICT (tenant_id, inventory_owner_id, location_id, license_plate_id, item_batch_id, uom, status)
                    WHERE license_plate_id IS NOT NULL DO UPDATE
                SET qty_on_hand = inventory_balances.qty_on_hand + excluded.qty_on_hand,
                    modified = excluded.modified,
                    deleted = NULL
                "#,
            )
            .bind(tenant_id.get())
            .bind(inventory_owner_id)
            .bind(now)
            .bind(now)
            .bind(facility_id)
            .bind(to_location_id)
            .bind(license_plate_id)
            .bind(batch_id)
            .bind(item_id)
            .bind(&uom)
            .bind(received_qty)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query(
                r#"
                INSERT INTO inventory_balances
                    (tenant_id, inventory_owner_id, created, modified, facility_id,
                     location_id, license_plate_id, item_batch_id, item_id, uom,
                     status, qty_on_hand, qty_reserved)
                VALUES ($1, $2, $3, $4, $5, $6, NULL, $7, $8, $9, 'available', $10, 0)
                ON CONFLICT (tenant_id, inventory_owner_id, location_id, item_batch_id, uom, status)
                    WHERE license_plate_id IS NULL DO UPDATE
                SET qty_on_hand = inventory_balances.qty_on_hand + excluded.qty_on_hand,
                    modified = excluded.modified,
                    deleted = NULL
                "#,
            )
            .bind(tenant_id.get())
            .bind(inventory_owner_id)
            .bind(now)
            .bind(now)
            .bind(facility_id)
            .bind(to_location_id)
            .bind(batch_id)
            .bind(item_id)
            .bind(&uom)
            .bind(received_qty)
            .execute(&mut *tx)
            .await?;
        }

        inventory_journal::append_entry(
            &mut tx,
            tenant_id,
            inventory_owner_id,
            transaction_id,
            &JournalEntry {
                facility_id,
                location_id: to_location_id,
                license_plate_id: resolved_license_plate_id,
                item_batch_id: batch_id,
                status: InventoryStatus::Available,
                quantity_delta: received_qty,
            },
        )
        .await?;
    }

    let open_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM load_lines WHERE tenant_id = $1 AND load_id = $2 AND deleted IS NULL AND status IN ('pending', 'partial')",
    )
    .bind(tenant_id.get())
    .bind(load_id)
    .fetch_one(&mut *tx)
    .await?;
    let next_load_status = if open_count == 0 {
        LoadStatus::Received
    } else {
        LoadStatus::Receiving
    };
    let receive_completed = open_count == 0;
    sqlx::query(
        "UPDATE loads SET status = $1, receive_completed = $2, actual_time = COALESCE(actual_time, $3) WHERE tenant_id = $4 AND id = $5",
    )
    .bind(next_load_status.as_str())
    .bind(receive_completed)
    .bind(now)
    .bind(tenant_id.get())
    .bind(load_id)
    .execute(&mut *tx)
    .await?;

    insert_activity_tx(
        &mut tx,
        tenant_id,
        load_id,
        Some(user_id),
        "line_received",
        Some("load line received"),
        Some(&format!(
            r#"{{"load_line_id":{},"received_qty":{},"rejected_qty":{}}}"#,
            load_line_id, received_qty, rejected_qty
        )),
    )
    .await?;
    let result = ReceiveLoadLineResult {
        load_line_id,
        inventory_transaction_id,
    };
    inventory_journal::record_command_result(
        &mut tx,
        tenant_id,
        "receive_load_line",
        idempotency_key,
        &request_hash,
        &result,
        inventory_transaction_id,
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}

async fn ensure_load_resolved_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    load_id: i64,
) -> AppResult<()> {
    let open_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM load_lines WHERE tenant_id = $1 AND load_id = $2 AND deleted IS NULL AND status IN ('pending', 'partial')",
    )
    .bind(tenant_id.get())
    .bind(load_id)
    .fetch_one(&mut **tx)
    .await?;
    if open_count > 0 {
        return Err(AppError::conflict(
            "cannot close a load with unresolved lines",
        ));
    }
    Ok(())
}

async fn insert_activity_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    load_id: i64,
    user_id: Option<i64>,
    action: &str,
    message: Option<&str>,
    metadata_json: Option<&str>,
) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO load_activity (tenant_id, created, load_id, user_id, action, message, metadata_json) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(tenant_id.get())
    .bind(now_iso())
    .bind(load_id)
    .bind(user_id)
    .bind(action)
    .bind(message)
    .bind(metadata_json)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

/// Record an uploaded document/image against a load. The bytes themselves are
/// written to disk by the route handler; this stores the metadata row.
#[allow(clippy::too_many_arguments)]
pub async fn add_file(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    load_id: i64,
    original_name: &str,
    stored_name: &str,
    path: &str,
    content_type: Option<&str>,
    category: LoadFileCategory,
) -> AppResult<i64> {
    let mut tx = db.begin().await?;
    let id: Option<i64> = sqlx::query_scalar(
        r#"
        INSERT INTO load_files
            (tenant_id, created, load_id, original_name, name, path, content_type, category)
        SELECT $1, $2, load.id, $4, $5, $6, $7, $8
        FROM loads load
        WHERE load.tenant_id = $1 AND load.id = $3 AND load.deleted IS NULL
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(now_iso())
    .bind(load_id)
    .bind(original_name)
    .bind(stored_name)
    .bind(path)
    .bind(content_type)
    .bind(category.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    let id = id.ok_or_else(|| AppError::not_found("load"))?;
    insert_activity_tx(
        &mut tx,
        tenant_id,
        load_id,
        Some(user_id),
        "file_added",
        Some(original_name),
        Some(&format!(
            r#"{{"file_id":{},"category":"{}"}}"#,
            id,
            category.as_str()
        )),
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn get_file(db: &Db, tenant_id: TenantId, file_id: i64) -> AppResult<Option<LoadFile>> {
    let row = sqlx::query(
        "SELECT id, tenant_id, created, deleted, load_id, original_name, name, path, content_type, category FROM load_files WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id.get())
    .bind(file_id)
    .fetch_optional(db)
    .await?;
    row.as_ref().map(map_file).transpose()
}

pub async fn delete_file(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    file_id: i64,
) -> AppResult<bool> {
    let mut tx = db.begin().await?;
    let load_id: Option<i64> = sqlx::query_scalar(
        "SELECT load_id FROM load_files WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL FOR UPDATE",
    )
    .bind(tenant_id.get())
    .bind(file_id)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    let Some(load_id) = load_id else {
        return Ok(false);
    };

    let res = sqlx::query("UPDATE load_files SET deleted = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(now_iso())
        .bind(tenant_id.get())
        .bind(file_id)
        .execute(&mut *tx)
        .await?;
    if res.rows_affected() > 0 {
        insert_activity_tx(
            &mut tx,
            tenant_id,
            load_id,
            Some(user_id),
            "file_deleted",
            Some(&format!("load file {file_id}")),
            None,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}
