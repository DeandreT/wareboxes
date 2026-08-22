//! Platform data-cell registry, capacity, residency, and placement reads.

mod commands;
mod events;
mod query;

pub use commands::{change_status, reconfigure, register};
pub use query::{by_id, event_page, page};

use sqlx::Row;
use wareboxes_domain::{DataCellId, DataResidencyCode};

use crate::error::{AppError, AppResult};

pub(crate) async fn require_available_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    data_cell_id: DataCellId,
    residency_requirement: &DataResidencyCode,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"SELECT cell.status,cell.mode,cell.max_tenants,cell.residency_code,
        (SELECT COUNT(*) FROM tenant_cell_placements placement
          WHERE placement.data_cell_id=cell.id) AS placement_count,
        (SELECT COUNT(*) FROM tenant_cell_moves move
          WHERE (move.target_data_cell_id=cell.id
              AND move.status IN ('planned','copying','frozen','validated'))
            OR (move.source_data_cell_id=cell.id AND move.status='cut_over'))
          AS reserved_move_count
        FROM data_cells cell WHERE cell.id=$1 FOR UPDATE"#,
    )
    .bind(data_cell_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("data cell"))?;
    let status: String = row.try_get("status")?;
    let mode: String = row.try_get("mode")?;
    let max_tenants: i64 = row.try_get("max_tenants")?;
    let placement_count: i64 = row.try_get("placement_count")?;
    let reserved_move_count: i64 = row.try_get("reserved_move_count")?;
    let residency = DataResidencyCode::new(row.try_get::<String, _>("residency_code")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    if status != "active" {
        return Err(AppError::conflict(
            "data cell is not accepting tenant placements",
        ));
    }
    if placement_count + reserved_move_count >= max_tenants
        || (mode == "dedicated" && placement_count + reserved_move_count != 0)
    {
        return Err(AppError::conflict(
            "data cell has no available tenant capacity",
        ));
    }
    if !residency_requirement.allows(&residency) {
        return Err(AppError::bad_request(
            "data cell does not satisfy the tenant residency requirement",
        ));
    }
    Ok(())
}
