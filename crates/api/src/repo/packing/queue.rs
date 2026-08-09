use sqlx::Row;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{OrderId, OrderRevision, PackSessionId, Timestamp};
use wareboxes_persistence_postgres::db::{begin_tenant_transaction, Db};

use crate::error::{AppError, AppResult};
use crate::repo::access::current_scope_tx;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackingQueueCursor {
    pub facility_id: Option<i64>,
    pub rush_rank: i16,
    pub ship_by: Option<Timestamp>,
    pub order_id: OrderId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackingQueueSession {
    pub session_id: PackSessionId,
    pub station_location_id: i64,
    pub station_location_barcode: String,
    pub station_location_name: Option<String>,
    pub state: String,
    pub started_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackingQueueEntry {
    pub order_id: OrderId,
    pub order_key: String,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub order_status: String,
    pub revision: OrderRevision,
    pub rush: bool,
    pub ship_by: Option<Timestamp>,
    pub session: Option<PackingQueueSession>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackingQueuePage {
    pub items: Vec<PackingQueueEntry>,
    pub next_cursor: Option<PackingQueueCursor>,
}

pub async fn packing_queue(
    db: &Db,
    access: &TenantAccess,
    facility_id: Option<i64>,
    after: Option<&PackingQueueCursor>,
    limit: u16,
) -> AppResult<PackingQueuePage> {
    if facility_id.is_some_and(|id| id <= 0)
        || after.is_some_and(|cursor| cursor.facility_id != facility_id)
    {
        return Err(AppError::bad_request(
            "packing queue cursor does not match its facility filter",
        ));
    }
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    let after_rush_rank = after.map(|cursor| cursor.rush_rank);
    let after_ship_by = after.and_then(|cursor| cursor.ship_by);
    let after_order_id = after.map(|cursor| cursor.order_id.get());
    let fetch_limit = i64::from(limit) + 1;

    let rows = sqlx::query(
        r#"
        SELECT orders.id AS order_id,
               orders.order_key,
               orders.inventory_owner_id,
               inventory_owner.name AS inventory_owner_name,
               release.facility_id,
               COALESCE(facility.name, '') AS facility_name,
               orders.status AS order_status,
               orders.revision,
               orders.rush,
               orders.ship_by,
               session.id AS session_id,
               session.packing_location_id AS station_location_id,
               station.barcode AS station_location_barcode,
               station.name AS station_location_name,
               session.state AS session_state,
               session.started_at AS session_started_at
        FROM orders
        INNER JOIN order_releases release
          ON release.tenant_id = orders.tenant_id
         AND release.inventory_owner_id = orders.inventory_owner_id
         AND release.order_id = orders.id
        INNER JOIN inventory_owners inventory_owner
          ON inventory_owner.tenant_id = orders.tenant_id
         AND inventory_owner.id = orders.inventory_owner_id
         AND inventory_owner.deleted IS NULL
        INNER JOIN facilities facility
          ON facility.tenant_id = release.tenant_id
         AND facility.id = release.facility_id
         AND facility.deleted IS NULL
        LEFT JOIN packing_sessions session
          ON session.tenant_id = orders.tenant_id
         AND session.inventory_owner_id = orders.inventory_owner_id
         AND session.order_id = orders.id
         AND session.order_release_id = release.id
         AND session.facility_id = release.facility_id
         AND session.state <> 'abandoned'
        LEFT JOIN locations station
          ON station.tenant_id = session.tenant_id
         AND station.facility_id = session.facility_id
         AND station.id = session.packing_location_id
         AND station.deleted IS NULL
        WHERE orders.tenant_id = $1
          AND orders.deleted IS NULL
          AND orders.status IN ('awaiting packing', 'packing')
          AND ($2 OR release.facility_id = ANY($3))
          AND ($4 OR orders.inventory_owner_id = ANY($5))
          AND ($6::BIGINT IS NULL OR release.facility_id = $6)
          AND (
              $7::SMALLINT IS NULL
              OR (
                  CASE WHEN orders.rush THEN 0 ELSE 1 END,
                  COALESCE(orders.ship_by, 'infinity'::TIMESTAMPTZ),
                  orders.id
              ) > (
                  $7,
                  COALESCE($8::TIMESTAMPTZ, 'infinity'::TIMESTAMPTZ),
                  $9
              )
          )
        ORDER BY orders.rush DESC, orders.ship_by ASC NULLS LAST, orders.id ASC
        LIMIT $10
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(facility_id)
    .bind(after_rush_rank)
    .bind(after_ship_by)
    .bind(after_order_id)
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await?;

    let mut items = rows
        .into_iter()
        .map(entry_from_row)
        .collect::<AppResult<Vec<_>>>()?;
    let has_more = items.len() > usize::from(limit);
    if has_more {
        items.pop();
    }
    let next_cursor = has_more
        .then(|| items.last().map(cursor_for_entry))
        .flatten()
        .map(|mut cursor| {
            cursor.facility_id = facility_id;
            cursor
        });
    tx.commit().await?;
    Ok(PackingQueuePage { items, next_cursor })
}

fn entry_from_row(row: sqlx::postgres::PgRow) -> AppResult<PackingQueueEntry> {
    let order_status: String = row.try_get("order_status")?;
    let session_id: Option<i64> = row.try_get("session_id")?;
    let session = match session_id {
        Some(session_id) => Some(PackingQueueSession {
            session_id: PackSessionId::new(session_id)
                .map_err(|error| AppError::internal(error.to_string()))?,
            station_location_id: required_positive(&row, "station_location_id")?,
            station_location_barcode: row
                .try_get::<Option<String>, _>("station_location_barcode")?
                .ok_or_else(|| AppError::internal("packing station has no active barcode"))?,
            station_location_name: row.try_get("station_location_name")?,
            state: row
                .try_get::<Option<String>, _>("session_state")?
                .ok_or_else(|| AppError::internal("packing session has no state"))?,
            started_at: row
                .try_get::<Option<Timestamp>, _>("session_started_at")?
                .ok_or_else(|| AppError::internal("packing session has no start timestamp"))?,
        }),
        None => None,
    };
    match (order_status.as_str(), session.as_ref()) {
        ("awaiting packing", None) => {}
        ("packing", Some(session)) if session.state == "open" => {}
        _ => {
            return Err(AppError::internal(
                "packing queue order and session state are inconsistent",
            ));
        }
    }

    Ok(PackingQueueEntry {
        order_id: OrderId::new(row.try_get("order_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_key: row.try_get("order_key")?,
        inventory_owner_id: required_positive(&row, "inventory_owner_id")?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: required_positive(&row, "facility_id")?,
        facility_name: row.try_get("facility_name")?,
        order_status,
        revision: OrderRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        rush: row.try_get("rush")?,
        ship_by: row.try_get("ship_by")?,
        session,
    })
}

fn required_positive(row: &sqlx::postgres::PgRow, column: &str) -> AppResult<i64> {
    let value: Option<i64> = row.try_get(column)?;
    value
        .filter(|value| *value > 0)
        .ok_or_else(|| AppError::internal(format!("packing queue has invalid {column}")))
}

fn cursor_for_entry(entry: &PackingQueueEntry) -> PackingQueueCursor {
    PackingQueueCursor {
        facility_id: None,
        rush_rank: if entry.rush { 0 } else { 1 },
        ship_by: entry.ship_by,
        order_id: entry.order_id,
    }
}
