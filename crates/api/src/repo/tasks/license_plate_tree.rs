use sqlx::Row;
use wareboxes_domain::TenantId;

use crate::error::{AppError, AppResult};

const MAX_TREE_NODES: usize = wareboxes_domain::MAX_LICENSE_PLATE_HIERARCHY_NODES as usize;

#[derive(Debug)]
pub(in crate::repo) struct LockedLicensePlateTree {
    pub(in crate::repo) inventory_owner_id: i64,
    pub(in crate::repo) facility_id: i64,
    pub(in crate::repo) location_id: i64,
    pub(in crate::repo) barcode: String,
    pub(in crate::repo) plate_ids: Vec<i64>,
}

pub(in crate::repo) async fn lock_root_tree_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    root_id: i64,
) -> AppResult<LockedLicensePlateTree> {
    let scope = sqlx::query(
        r#"
        SELECT inventory_owner_id,facility_id
        FROM license_plates
        WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(root_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("license plate"))?;
    let inventory_owner_id: i64 = scope.try_get("inventory_owner_id")?;
    let facility_id: i64 = scope.try_get("facility_id")?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "license-plate-hierarchy:{}:{}:{}",
            tenant_id.get(),
            inventory_owner_id,
            facility_id
        ))
        .execute(&mut **tx)
        .await?;

    let row_limit = i64::try_from(MAX_TREE_NODES + 1)
        .map_err(|_| AppError::internal("license plate tree limit is out of range"))?;
    let rows = sqlx::query(
        r#"
        WITH RECURSIVE tree AS (
          SELECT plate.id,plate.parent_license_plate_id,plate.inventory_owner_id,
                 plate.facility_id,plate.location_id,plate.barcode,0::INTEGER AS depth
          FROM license_plates plate
          WHERE plate.tenant_id=$1 AND plate.id=$2 AND plate.deleted IS NULL
          UNION ALL
          SELECT child.id,child.parent_license_plate_id,child.inventory_owner_id,
                 child.facility_id,child.location_id,child.barcode,tree.depth+1
          FROM tree JOIN license_plates child
            ON child.tenant_id=$1 AND child.parent_license_plate_id=tree.id
           AND child.deleted IS NULL
          WHERE tree.depth<9
        )
        SELECT plate.id,tree.parent_license_plate_id,tree.inventory_owner_id,
               tree.facility_id,tree.location_id,tree.barcode,tree.depth
        FROM tree JOIN license_plates plate
          ON plate.tenant_id=$1 AND plate.id=tree.id
        ORDER BY plate.id
        LIMIT $3
        FOR UPDATE OF plate
        "#,
    )
    .bind(tenant_id.get())
    .bind(root_id)
    .bind(row_limit)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() > MAX_TREE_NODES {
        return Err(AppError::conflict(
            "license plate hierarchy exceeds the 1000-container movement limit",
        ));
    }
    let root = rows
        .iter()
        .find(|row| row.get::<i64, _>("id") == root_id)
        .ok_or_else(|| AppError::not_found("license plate"))?;
    if root
        .try_get::<Option<i64>, _>("parent_license_plate_id")?
        .is_some()
    {
        return Err(AppError::conflict(
            "nested license plates move with their root container; detach this plate first",
        ));
    }
    let location_id = root
        .try_get::<Option<i64>, _>("location_id")?
        .ok_or_else(|| AppError::conflict("license plate has no current location"))?;
    let barcode = root
        .try_get::<Option<String>, _>("barcode")?
        .filter(|barcode| !barcode.trim().is_empty())
        .ok_or_else(|| AppError::conflict("license plate must have a scannable barcode"))?;
    for row in &rows {
        let depth: i32 = row.try_get("depth")?;
        if depth > i32::from(wareboxes_domain::MAX_LICENSE_PLATE_HIERARCHY_DEPTH)
            || row.try_get::<i64, _>("inventory_owner_id")? != inventory_owner_id
            || row.try_get::<i64, _>("facility_id")? != facility_id
            || row.try_get::<Option<i64>, _>("location_id")? != Some(location_id)
        {
            return Err(AppError::conflict(
                "license plate hierarchy is not a colocated owner/facility tree",
            ));
        }
    }
    Ok(LockedLicensePlateTree {
        inventory_owner_id,
        facility_id,
        location_id,
        barcode,
        plate_ids: rows.iter().map(|row| row.get("id")).collect(),
    })
}

pub(in crate::repo) async fn require_no_active_tree_movement_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    plate_ids: &[i64],
) -> AppResult<()> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
          SELECT 1 FROM license_plate_putaway_tasks
          WHERE tenant_id=$1 AND inventory_owner_id=$2
            AND license_plate_id=ANY($3) AND closed_at IS NULL
          UNION ALL
          SELECT 1 FROM inventory_relocation_tasks
          WHERE tenant_id=$1 AND inventory_owner_id=$2
            AND license_plate_id=ANY($3) AND closed_at IS NULL
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(plate_ids)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Err(AppError::conflict(
            "license plate hierarchy already has active directed putaway work or relocation work",
        ))
    } else {
        Ok(())
    }
}
