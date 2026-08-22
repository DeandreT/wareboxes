use sqlx::Row;
use wareboxes_application::data_cell::{
    DataCellCursor, DataCellEventCursor, DataCellEventPage, DataCellEventPageQuery,
    DataCellEventReadModel, DataCellPage, DataCellPageQuery, DataCellReadModel,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{DataCellId, DataCellMode, DataCellRevision, DataCellStatus, UserId};

use crate::db::{begin_tenant_transaction, Db};
use crate::error::{AppError, AppResult};

fn parse_status(value: String) -> AppResult<DataCellStatus> {
    DataCellStatus::parse(&value)
        .ok_or_else(|| AppError::internal(format!("stored data-cell status is invalid: {value}")))
}

fn parse_mode(value: String) -> AppResult<DataCellMode> {
    DataCellMode::parse(&value)
        .ok_or_else(|| AppError::internal(format!("stored data-cell mode is invalid: {value}")))
}

fn map_row(row: &sqlx::postgres::PgRow) -> AppResult<DataCellReadModel> {
    let max_tenants: i64 = row.try_get("max_tenants")?;
    Ok(DataCellReadModel {
        data_cell_id: DataCellId::new(row.try_get("data_cell_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        key: row.try_get("cell_key")?,
        name: row.try_get("name")?,
        region: row.try_get("region")?,
        residency: row.try_get("residency_code")?,
        mode: parse_mode(row.try_get("mode")?)?,
        status: parse_status(row.try_get("status")?)?,
        revision: DataCellRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        max_tenants: u32::try_from(max_tenants)
            .map_err(|_| AppError::internal("stored data-cell capacity is invalid"))?,
        placement_count: row.try_get("placement_count")?,
        reserved_inbound_move_count: row.try_get("reserved_inbound_move_count")?,
        reserved_rollback_move_count: row.try_get("reserved_rollback_move_count")?,
        created_at: row.try_get("created_at")?,
        created_by: row
            .try_get::<Option<i64>, _>("created_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        changed_at: row.try_get("changed_at")?,
        changed_by: row
            .try_get::<Option<i64>, _>("changed_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        change_reason: row.try_get("change_reason")?,
    })
}

pub(super) async fn read_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    data_cell_id: DataCellId,
) -> AppResult<DataCellReadModel> {
    let row = sqlx::query(
        r#"SELECT cell.id AS data_cell_id,cell.cell_key,cell.name,cell.region,
        cell.residency_code,cell.mode,cell.status,cell.revision,cell.max_tenants,
        cell.created_at,cell.created_by_user_id,cell.changed_at,
        cell.changed_by_user_id,cell.change_reason,
        (SELECT COUNT(*) FROM tenant_cell_placements placement
          WHERE placement.data_cell_id=cell.id) AS placement_count,
        (SELECT COUNT(*) FROM tenant_cell_moves move
          WHERE move.target_data_cell_id=cell.id
            AND move.status IN ('planned','copying','frozen','validated'))
          AS reserved_inbound_move_count,
        (SELECT COUNT(*) FROM tenant_cell_moves move
          WHERE move.source_data_cell_id=cell.id AND move.status='cut_over')
          AS reserved_rollback_move_count
        FROM data_cells cell WHERE cell.id=$1"#,
    )
    .bind(data_cell_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("data cell"))?;
    map_row(&row)
}

pub async fn by_id(
    db: &Db,
    actor_access: &TenantAccess,
    data_cell_id: DataCellId,
) -> AppResult<DataCellReadModel> {
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    crate::repo::tenant_lifecycle::authorize_tx(&mut tx, actor_access, actor_access.user_id)
        .await?;
    let result = read_tx(&mut tx, data_cell_id).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn page(
    db: &Db,
    actor_access: &TenantAccess,
    query: &DataCellPageQuery,
) -> AppResult<DataCellPage> {
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    crate::repo::tenant_lifecycle::authorize_tx(&mut tx, actor_access, actor_access.user_id)
        .await?;
    let rows = sqlx::query(
        r#"SELECT cell.id AS data_cell_id,cell.cell_key,cell.name,cell.region,
        cell.residency_code,cell.mode,cell.status,cell.revision,cell.max_tenants,
        cell.created_at,cell.created_by_user_id,cell.changed_at,
        cell.changed_by_user_id,cell.change_reason,
        (SELECT COUNT(*) FROM tenant_cell_placements placement
          WHERE placement.data_cell_id=cell.id) AS placement_count,
        (SELECT COUNT(*) FROM tenant_cell_moves move
          WHERE move.target_data_cell_id=cell.id
            AND move.status IN ('planned','copying','frozen','validated'))
          AS reserved_inbound_move_count,
        (SELECT COUNT(*) FROM tenant_cell_moves move
          WHERE move.source_data_cell_id=cell.id AND move.status='cut_over')
          AS reserved_rollback_move_count
        FROM data_cells cell
        WHERE ($1::TEXT IS NULL OR cell.status=$1)
          AND ($2::TEXT IS NULL OR cell.region=$2)
          AND ($3::TIMESTAMPTZ IS NULL OR (cell.created_at,cell.id)<($3,$4))
        ORDER BY cell.created_at DESC,cell.id DESC LIMIT $5"#,
    )
    .bind(query.status.map(DataCellStatus::as_str))
    .bind(query.region.as_deref())
    .bind(query.cursor.map(|cursor| cursor.after_created_at))
    .bind(query.cursor.map(|cursor| cursor.after_data_cell_id.get()))
    .bind(i64::from(query.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let mut items = rows.iter().map(map_row).collect::<AppResult<Vec<_>>>()?;
    let next_cursor = if items.len() > usize::from(query.limit) {
        items.pop();
        items.last().map(|cell| DataCellCursor {
            after_created_at: cell.created_at,
            after_data_cell_id: cell.data_cell_id,
        })
    } else {
        None
    };
    tx.commit().await?;
    Ok(DataCellPage { items, next_cursor })
}

pub async fn event_page(
    db: &Db,
    actor_access: &TenantAccess,
    query: &DataCellEventPageQuery,
) -> AppResult<DataCellEventPage> {
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    crate::repo::tenant_lifecycle::authorize_tx(&mut tx, actor_access, actor_access.user_id)
        .await?;
    read_tx(&mut tx, query.data_cell_id).await?;
    let rows = sqlx::query(
        r#"SELECT id AS event_id,data_cell_id,action,cell_revision,
        previous_status,resulting_status,actor_user_id,occurred_at,reason,evidence
        FROM data_cell_events WHERE data_cell_id=$1
          AND ($2::TIMESTAMPTZ IS NULL OR (occurred_at,id)<($2,$3))
        ORDER BY occurred_at DESC,id DESC LIMIT $4"#,
    )
    .bind(query.data_cell_id.get())
    .bind(query.cursor.map(|cursor| cursor.after_occurred_at))
    .bind(query.cursor.map(|cursor| cursor.after_event_id))
    .bind(i64::from(query.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let mut items = rows
        .iter()
        .map(|row| {
            Ok(DataCellEventReadModel {
                event_id: row.try_get("event_id")?,
                data_cell_id: DataCellId::new(row.try_get("data_cell_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                action: row.try_get("action")?,
                cell_revision: DataCellRevision::new(row.try_get("cell_revision")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                previous_status: row
                    .try_get::<Option<String>, _>("previous_status")?
                    .map(parse_status)
                    .transpose()?,
                resulting_status: parse_status(row.try_get("resulting_status")?)?,
                actor_id: row
                    .try_get::<Option<i64>, _>("actor_user_id")?
                    .map(UserId::new)
                    .transpose()
                    .map_err(|error| AppError::internal(error.to_string()))?,
                occurred_at: row.try_get("occurred_at")?,
                reason: row.try_get("reason")?,
                evidence: row.try_get("evidence")?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = if items.len() > usize::from(query.limit) {
        items.pop();
        items.last().map(|event| DataCellEventCursor {
            after_occurred_at: event.occurred_at,
            after_event_id: event.event_id,
        })
    } else {
        None
    };
    tx.commit().await?;
    Ok(DataCellEventPage { items, next_cursor })
}
