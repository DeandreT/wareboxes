use sqlx::Row;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{FacilityId, InventoryOwnerId};

use crate::db::Db;
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone)]
pub struct ScopeBindings {
    pub all_facilities: bool,
    pub facility_ids: Vec<i64>,
    pub all_inventory_owners: bool,
    pub inventory_owner_ids: Vec<i64>,
}

impl ScopeBindings {
    pub fn unrestricted() -> Self {
        Self {
            all_facilities: true,
            facility_ids: Vec::new(),
            all_inventory_owners: true,
            inventory_owner_ids: Vec::new(),
        }
    }

    pub fn for_access(access: &TenantAccess) -> Self {
        Self {
            all_facilities: access.site_scope.all_facilities,
            facility_ids: access
                .site_scope
                .facility_ids
                .iter()
                .map(|id| id.get())
                .collect(),
            all_inventory_owners: access.owner_scope.all_inventory_owners,
            inventory_owner_ids: access
                .owner_scope
                .inventory_owner_ids
                .iter()
                .map(|id| id.get())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationalDimensions {
    pub facility_id: FacilityId,
    pub inventory_owner_id: InventoryOwnerId,
}

fn dimensions_from_row(row: &sqlx::postgres::PgRow) -> AppResult<OperationalDimensions> {
    Ok(OperationalDimensions {
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
    })
}

pub async fn order_is_accessible(
    db: &Db,
    access: &TenantAccess,
    order_id: i64,
    include_deleted: bool,
) -> AppResult<bool> {
    let scope = ScopeBindings::for_access(access);
    let exists = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM orders
            WHERE tenant_id = $1
              AND id = $2
              AND ($3 OR deleted IS NULL)
              AND ($4 OR inventory_owner_id = ANY($5))
        )
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(order_id)
    .bind(include_deleted)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(db)
    .await?;
    Ok(exists)
}

pub async fn load_dimensions(
    db: &Db,
    access: &TenantAccess,
    load_id: i64,
    include_deleted: bool,
) -> AppResult<Option<OperationalDimensions>> {
    let scope = ScopeBindings::for_access(access);
    let row = sqlx::query(
        r#"
        SELECT facility_id, inventory_owner_id
        FROM loads
        WHERE tenant_id = $1
          AND id = $2
          AND ($3 OR deleted IS NULL)
          AND ($4 OR facility_id = ANY($5))
          AND ($6 OR inventory_owner_id = ANY($7))
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load_id)
    .bind(include_deleted)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(db)
    .await?;
    row.as_ref().map(dimensions_from_row).transpose()
}

pub async fn load_line_dimensions(
    db: &Db,
    access: &TenantAccess,
    load_line_id: i64,
) -> AppResult<Option<OperationalDimensions>> {
    load_child_dimensions(db, access, LoadChild::Line, load_line_id).await
}

pub async fn load_note_dimensions(
    db: &Db,
    access: &TenantAccess,
    load_note_id: i64,
) -> AppResult<Option<OperationalDimensions>> {
    load_child_dimensions(db, access, LoadChild::Note, load_note_id).await
}

pub async fn load_file_dimensions(
    db: &Db,
    access: &TenantAccess,
    load_file_id: i64,
) -> AppResult<Option<OperationalDimensions>> {
    load_child_dimensions(db, access, LoadChild::File, load_file_id).await
}

#[derive(Debug, Clone, Copy)]
enum LoadChild {
    Line,
    Note,
    File,
}

async fn load_child_dimensions(
    db: &Db,
    access: &TenantAccess,
    child: LoadChild,
    child_id: i64,
) -> AppResult<Option<OperationalDimensions>> {
    let scope = ScopeBindings::for_access(access);
    let sql = match child {
        LoadChild::Line => {
            r#"
            SELECT load.facility_id, load.inventory_owner_id
            FROM load_lines child
            INNER JOIN loads load
                ON load.tenant_id = child.tenant_id AND load.id = child.load_id
            WHERE child.tenant_id = $1
              AND child.id = $2
              AND child.deleted IS NULL
              AND load.deleted IS NULL
              AND ($3 OR load.facility_id = ANY($4))
              AND ($5 OR load.inventory_owner_id = ANY($6))
            "#
        }
        LoadChild::Note => {
            r#"
            SELECT load.facility_id, load.inventory_owner_id
            FROM load_notes child
            INNER JOIN loads load
                ON load.tenant_id = child.tenant_id AND load.id = child.load_id
            WHERE child.tenant_id = $1
              AND child.id = $2
              AND child.deleted IS NULL
              AND load.deleted IS NULL
              AND ($3 OR load.facility_id = ANY($4))
              AND ($5 OR load.inventory_owner_id = ANY($6))
            "#
        }
        LoadChild::File => {
            r#"
            SELECT load.facility_id, load.inventory_owner_id
            FROM load_files child
            INNER JOIN loads load
                ON load.tenant_id = child.tenant_id AND load.id = child.load_id
            WHERE child.tenant_id = $1
              AND child.id = $2
              AND child.deleted IS NULL
              AND load.deleted IS NULL
              AND ($3 OR load.facility_id = ANY($4))
              AND ($5 OR load.inventory_owner_id = ANY($6))
            "#
        }
    };
    let row = sqlx::query(sql)
        .bind(access.tenant_id.get())
        .bind(child_id)
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .fetch_optional(db)
        .await?;
    row.as_ref().map(dimensions_from_row).transpose()
}
