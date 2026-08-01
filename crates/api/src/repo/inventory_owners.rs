//! Ported from `app/utils/inventory-owners.ts`.

use std::collections::HashMap;

use sqlx::{Postgres, Row, Transaction};
use wareboxes_application::ApplicationError;
use wareboxes_core::models::{Facility, InventoryOwner};
use wareboxes_domain::{OwnerScope, SiteScope, TenantId};

use crate::db::{begin_tenant_transaction, now_iso, Db};
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
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
) -> AppResult<HashMap<i64, Vec<Facility>>> {
    let rows = sqlx::query(
        r#"
        SELECT aw.inventory_owner_id AS inventory_owner_id,
               w.id AS id, w.tenant_id AS tenant_id, w.created AS created, w.deleted AS deleted,
               w.name AS name, w.address_id AS address_id
        FROM inventory_owner_facilities aw
        INNER JOIN facilities w
            ON w.tenant_id = aw.tenant_id AND w.id = aw.facility_id
        WHERE aw.tenant_id = $1 AND aw.deleted IS NULL AND w.deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .fetch_all(&mut **tx)
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

async fn facilities_by_inventory_owner_in_scope(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: TenantId,
    site_scope: &SiteScope,
) -> AppResult<HashMap<i64, Vec<Facility>>> {
    let facility_ids = site_scope
        .facility_ids
        .iter()
        .map(|id| id.get())
        .collect::<Vec<_>>();
    let rows = sqlx::query(
        r#"
        SELECT owner_facility.inventory_owner_id AS inventory_owner_id,
               facility.id AS id, facility.tenant_id AS tenant_id,
               facility.created AS created, facility.deleted AS deleted,
               facility.name AS name, facility.address_id AS address_id
        FROM inventory_owner_facilities owner_facility
        INNER JOIN facilities facility
            ON facility.tenant_id = owner_facility.tenant_id
           AND facility.id = owner_facility.facility_id
        WHERE owner_facility.tenant_id = $1
          AND owner_facility.deleted IS NULL
          AND facility.deleted IS NULL
          AND ($2 OR facility.id = ANY($3))
        "#,
    )
    .bind(tenant_id.get())
    .bind(site_scope.all_facilities)
    .bind(&facility_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut facilities = HashMap::<i64, Vec<Facility>>::new();
    for row in &rows {
        let inventory_owner_id = row.try_get("inventory_owner_id")?;
        facilities
            .entry(inventory_owner_id)
            .or_default()
            .push(Facility {
                id: row.try_get("id")?,
                tenant_id: TenantId::new(row.try_get("tenant_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                created: row.try_get("created")?,
                deleted: row.try_get("deleted")?,
                name: row.try_get("name")?,
                address_id: row.try_get("address_id")?,
            });
    }
    Ok(facilities)
}

pub async fn get_inventory_owners(
    db: &Db,
    tenant_id: TenantId,
    show_deleted: bool,
) -> AppResult<Vec<InventoryOwner>> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
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
    .fetch_all(&mut *tx)
    .await?;
    let mut wh = facilities_by_inventory_owner(&mut tx, tenant_id).await?;
    let inventory_owners = rows
        .iter()
        .map(|r| {
            let mut a = map_inventory_owner(r)?;
            a.inventory_owner_facilities = wh.remove(&a.id).unwrap_or_default();
            Ok(a)
        })
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(inventory_owners)
}

pub async fn get_inventory_owners_in_scope(
    db: &Db,
    tenant_id: TenantId,
    owner_scope: &OwnerScope,
    site_scope: &SiteScope,
    show_deleted: bool,
) -> AppResult<Vec<InventoryOwner>> {
    let inventory_owner_ids = owner_scope
        .inventory_owner_ids
        .iter()
        .map(|id| id.get())
        .collect::<Vec<_>>();
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, created, deleted, name, email
        FROM inventory_owners
        WHERE tenant_id = $1
          AND ($2 OR deleted IS NULL)
          AND ($3 OR id = ANY($4))
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(show_deleted)
    .bind(owner_scope.all_inventory_owners)
    .bind(&inventory_owner_ids)
    .fetch_all(&mut *tx)
    .await?;
    let mut facilities =
        facilities_by_inventory_owner_in_scope(&mut tx, tenant_id, site_scope).await?;
    let inventory_owners = rows
        .iter()
        .map(|row| {
            let mut inventory_owner = map_inventory_owner(row)?;
            inventory_owner.inventory_owner_facilities =
                facilities.remove(&inventory_owner.id).unwrap_or_default();
            Ok(inventory_owner)
        })
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(inventory_owners)
}

pub async fn active_inventory_owner_exists(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
) -> AppResult<bool> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM inventory_owners WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL)",
    )
    .bind(tenant_id.get())
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(exists)
}

pub async fn add_inventory_owner(
    db: &Db,
    tenant_id: TenantId,
    name: &str,
    email: &str,
) -> AppResult<i64> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO inventory_owners (tenant_id, name, email, created) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(tenant_id.get())
    .bind(name)
    .bind(email)
    .bind(now_iso())
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn replace_inventory_owner_facilities(
    db: &Db,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    facility_ids: &[i64],
) -> AppResult<bool> {
    let mut facility_ids = facility_ids.to_vec();
    facility_ids.sort_unstable();
    if facility_ids.iter().any(|id| *id <= 0) || facility_ids.windows(2).any(|ids| ids[0] == ids[1])
    {
        return Err(crate::error::AppError::bad_request(
            "facility_ids must contain unique positive IDs",
        ));
    }

    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let owner_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id
        FROM inventory_owners
        WHERE tenant_id = $1
          AND id = $2
          AND deleted IS NULL
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .fetch_optional(&mut *tx)
    .await?;
    if owner_id.is_none() {
        tx.rollback().await?;
        return Ok(false);
    }

    let facility_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM facilities
        WHERE tenant_id = $1
          AND id = ANY($2)
          AND deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(&facility_ids)
    .fetch_one(&mut *tx)
    .await?;
    let expected_count = i64::try_from(facility_ids.len())
        .map_err(|_| crate::error::AppError::bad_request("too many facility IDs"))?;
    if facility_count != expected_count {
        return Err(crate::error::AppError::bad_request(
            "facility_ids contains an unavailable facility",
        ));
    }

    sqlx::query(
        r#"
        UPDATE inventory_owner_facilities
        SET deleted = $1
        WHERE tenant_id = $2
          AND inventory_owner_id = $3
          AND deleted IS NULL
          AND NOT (facility_id = ANY($4))
        "#,
    )
    .bind(now_iso())
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(&facility_ids)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO inventory_owner_facilities
            (tenant_id, created, inventory_owner_id, facility_id)
        SELECT $1, $2, $3, UNNEST($4::BIGINT[])
        ON CONFLICT (tenant_id, inventory_owner_id, facility_id) DO UPDATE
        SET created = excluded.created, deleted = NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(now_iso())
    .bind(inventory_owner_id)
    .bind(&facility_ids)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(true)
}

pub async fn update_inventory_owner(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
    name: Option<&str>,
    email: Option<&str>,
) -> AppResult<bool> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let res = sqlx::query(
        "UPDATE inventory_owners SET name = COALESCE($1, name), email = COALESCE($2, email) WHERE tenant_id = $3 AND id = $4",
    )
    .bind(name)
    .bind(email)
    .bind(tenant_id.get())
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}

pub async fn update_inventory_owner_in_scope(
    db: &Db,
    tenant_id: TenantId,
    owner_scope: &OwnerScope,
    id: i64,
    name: Option<&str>,
    email: Option<&str>,
) -> AppResult<bool> {
    let inventory_owner_ids = owner_scope
        .inventory_owner_ids
        .iter()
        .map(|owner_id| owner_id.get())
        .collect::<Vec<_>>();
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let result = sqlx::query(
        r#"
        UPDATE inventory_owners
        SET name = COALESCE($1, name), email = COALESCE($2, email)
        WHERE tenant_id = $3
          AND id = $4
          AND ($5 OR id = ANY($6))
        "#,
    )
    .bind(name)
    .bind(email)
    .bind(tenant_id.get())
    .bind(id)
    .bind(owner_scope.all_inventory_owners)
    .bind(&inventory_owner_ids)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.rows_affected() > 0)
}

/// Refuses if the inventory owner still has orders that are not
/// shipped or cancelled.
pub async fn delete_inventory_owner(db: &Db, tenant_id: TenantId, id: i64) -> AppResult<bool> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
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
    .fetch_one(&mut *tx)
    .await?;
    if open > 0 {
        return Err(AppError::Application(ApplicationError::Conflict(
            "Inventory owner has orders that are not shipped or cancelled".into(),
        )));
    }
    let res =
        sqlx::query("UPDATE inventory_owners SET deleted = $1 WHERE tenant_id = $2 AND id = $3")
            .bind(now_iso())
            .bind(tenant_id.get())
            .bind(id)
            .execute(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}

pub async fn delete_inventory_owner_in_scope(
    db: &Db,
    tenant_id: TenantId,
    owner_scope: &OwnerScope,
    id: i64,
) -> AppResult<bool> {
    if !active_inventory_owner_exists_in_scope(db, tenant_id, owner_scope, id).await? {
        return Ok(false);
    }
    delete_inventory_owner(db, tenant_id, id).await
}

pub async fn restore_inventory_owner(db: &Db, tenant_id: TenantId, id: i64) -> AppResult<bool> {
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let res =
        sqlx::query("UPDATE inventory_owners SET deleted = NULL WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id.get())
            .bind(id)
            .execute(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}

pub async fn restore_inventory_owner_in_scope(
    db: &Db,
    tenant_id: TenantId,
    owner_scope: &OwnerScope,
    id: i64,
) -> AppResult<bool> {
    let inventory_owner_ids = owner_scope
        .inventory_owner_ids
        .iter()
        .map(|owner_id| owner_id.get())
        .collect::<Vec<_>>();
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let result = sqlx::query(
        r#"
        UPDATE inventory_owners
        SET deleted = NULL
        WHERE tenant_id = $1
          AND id = $2
          AND ($3 OR id = ANY($4))
        "#,
    )
    .bind(tenant_id.get())
    .bind(id)
    .bind(owner_scope.all_inventory_owners)
    .bind(&inventory_owner_ids)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.rows_affected() > 0)
}

pub async fn active_inventory_owner_exists_in_scope(
    db: &Db,
    tenant_id: TenantId,
    owner_scope: &OwnerScope,
    id: i64,
) -> AppResult<bool> {
    let inventory_owner_ids = owner_scope
        .inventory_owner_ids
        .iter()
        .map(|owner_id| owner_id.get())
        .collect::<Vec<_>>();
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM inventory_owners
            WHERE tenant_id = $1
              AND id = $2
              AND deleted IS NULL
              AND ($3 OR id = ANY($4))
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(id)
    .bind(owner_scope.all_inventory_owners)
    .bind(&inventory_owner_ids)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(exists)
}
