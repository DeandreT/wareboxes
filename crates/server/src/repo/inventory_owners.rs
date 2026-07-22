//! Ported from `app/utils/inventory-owners.ts`.

use std::collections::HashMap;

use sqlx::Row;
use wareboxes_core::models::{Facility, InventoryOwner};
use wareboxes_core::CoreError;
use wareboxes_domain::TenantId;

use crate::db::{now_iso, Db};
use crate::error::{AppError, AppResult};

fn map_inventory_owner(row: &sqlx::postgres::PgRow) -> AppResult<InventoryOwner> {
    Ok(InventoryOwner {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        name: row.try_get("name")?,
        email: row.try_get("email")?,
        inventory_owner_facilities: Vec::new(),
    })
}

async fn facilities_by_inventory_owner(
    db: &Db,
    tenant_id: TenantId,
) -> AppResult<HashMap<i64, Vec<Facility>>> {
    let rows = sqlx::query(
        r#"
        SELECT aw.inventory_owner_id AS inventory_owner_id,
               w.id AS id, w.tenant_id AS tenant_id, w.created AS created, w.deleted AS deleted,
               w.name AS name, w.address_id AS address_id
        FROM inventory_owner_facilities aw
        INNER JOIN facilities w ON w.id = aw.facility_id
        WHERE aw.tenant_id = $1 AND aw.deleted IS NULL AND w.deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .fetch_all(db)
    .await?;
    let mut map: HashMap<i64, Vec<Facility>> = HashMap::new();
    for r in &rows {
        let acc = r.try_get("inventory_owner_id")?;
        map.entry(acc).or_default().push(Facility {
            id: r.try_get("id")?,
            tenant_id: TenantId::new(r.try_get("tenant_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            created: r.try_get("created")?,
            deleted: r.try_get("deleted")?,
            name: r.try_get("name")?,
            address_id: r.try_get("address_id")?,
        });
    }
    Ok(map)
}

pub async fn get_inventory_owners(
    db: &Db,
    tenant_id: TenantId,
    show_deleted: bool,
) -> AppResult<Vec<InventoryOwner>> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, created, deleted, name, email
        FROM inventory_owners
        WHERE tenant_id = $1 AND ($2 OR deleted IS NULL)
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(show_deleted)
    .fetch_all(db)
    .await?;
    let mut wh = facilities_by_inventory_owner(db, tenant_id).await?;
    rows.iter()
        .map(|r| {
            let mut a = map_inventory_owner(r)?;
            a.inventory_owner_facilities = wh.remove(&a.id).unwrap_or_default();
            Ok(a)
        })
        .collect()
}

pub async fn active_inventory_owner_exists(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
) -> AppResult<bool> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM inventory_owners WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL)",
    )
    .bind(tenant_id.get())
    .bind(id)
    .fetch_one(db)
    .await?;
    Ok(exists)
}

pub async fn add_inventory_owner(
    db: &Db,
    tenant_id: TenantId,
    name: &str,
    email: &str,
) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO inventory_owners (tenant_id, name, email, created) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(tenant_id.get())
    .bind(name)
    .bind(email)
    .bind(now_iso())
    .fetch_one(db)
    .await?;
    Ok(id)
}

pub async fn update_inventory_owner(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
    name: Option<&str>,
    email: Option<&str>,
) -> AppResult<bool> {
    let res = sqlx::query(
        "UPDATE inventory_owners SET name = COALESCE($1, name), email = COALESCE($2, email) WHERE tenant_id = $3 AND id = $4",
    )
    .bind(name)
    .bind(email)
    .bind(tenant_id.get())
    .bind(id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Refuses if the inventory owner still has orders that are not
/// shipped or cancelled.
pub async fn delete_inventory_owner(db: &Db, tenant_id: TenantId, id: i64) -> AppResult<bool> {
    let open: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM orders
        WHERE tenant_id = $2
          AND inventory_owner_id = $1
          AND status NOT IN ('shipped', 'cancelled')
        "#,
    )
    .bind(id)
    .bind(tenant_id.get())
    .fetch_one(db)
    .await?;
    if open > 0 {
        return Err(AppError::Core(CoreError::Conflict(
            "Inventory owner has orders that are not shipped or cancelled".into(),
        )));
    }
    let res =
        sqlx::query("UPDATE inventory_owners SET deleted = $1 WHERE tenant_id = $2 AND id = $3")
            .bind(now_iso())
            .bind(tenant_id.get())
            .bind(id)
            .execute(db)
            .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn restore_inventory_owner(db: &Db, tenant_id: TenantId, id: i64) -> AppResult<bool> {
    let res =
        sqlx::query("UPDATE inventory_owners SET deleted = NULL WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id.get())
            .bind(id)
            .execute(db)
            .await?;
    Ok(res.rows_affected() > 0)
}
