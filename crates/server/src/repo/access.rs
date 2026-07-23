use sqlx::Row;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{FacilityId, InventoryOwnerId, TenantId};

use crate::db::{bind_tenant_context, Db};
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

pub(crate) async fn lock_user_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    user_id: i64,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::TEXT || ':' || $2::TEXT, 0))")
        .bind(tenant_id.get())
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) async fn current_scope_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    user_id: i64,
) -> AppResult<ScopeBindings> {
    let row = sqlx::query(
        r#"
        SELECT
            membership.all_facilities,
            ARRAY(
                SELECT user_facility.facility_id
                FROM user_facilities user_facility
                WHERE user_facility.tenant_id = membership.tenant_id
                  AND user_facility.user_id = membership.user_id
                  AND user_facility.deleted IS NULL
                ORDER BY user_facility.facility_id
            ) AS facility_ids,
            membership.all_inventory_owners,
            ARRAY(
                SELECT user_owner.inventory_owner_id
                FROM user_inventory_owners user_owner
                WHERE user_owner.tenant_id = membership.tenant_id
                  AND user_owner.user_id = membership.user_id
                  AND user_owner.deleted IS NULL
                ORDER BY user_owner.inventory_owner_id
            ) AS inventory_owner_ids
        FROM tenant_memberships membership
        WHERE membership.tenant_id = $1
          AND membership.user_id = $2
          AND membership.deleted IS NULL
        FOR SHARE OF membership
        "#,
    )
    .bind(tenant_id.get())
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;

    let Some(row) = row else {
        return Ok(ScopeBindings {
            all_facilities: false,
            facility_ids: Vec::new(),
            all_inventory_owners: false,
            inventory_owner_ids: Vec::new(),
        });
    };
    Ok(ScopeBindings {
        all_facilities: row.try_get("all_facilities")?,
        facility_ids: row.try_get("facility_ids")?,
        all_inventory_owners: row.try_get("all_inventory_owners")?,
        inventory_owner_ids: row.try_get("inventory_owner_ids")?,
    })
}

pub(crate) async fn lock_current_scope_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    user_id: i64,
) -> AppResult<ScopeBindings> {
    lock_user_tx(tx, tenant_id, user_id).await?;
    current_scope_tx(tx, tenant_id, user_id).await
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

pub async fn item_batch_owner(
    db: &Db,
    access: &TenantAccess,
    item_batch_id: i64,
    include_deleted: bool,
) -> AppResult<Option<InventoryOwnerId>> {
    let scope = ScopeBindings::for_access(access);
    let owner_id = sqlx::query_scalar(
        r#"
        SELECT inventory_owner_id
        FROM item_batches
        WHERE tenant_id = $1
          AND id = $2
          AND ($3 OR deleted IS NULL)
          AND ($4 OR inventory_owner_id = ANY($5))
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(item_batch_id)
    .bind(include_deleted)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(db)
    .await?;
    owner_id
        .map(|id| InventoryOwnerId::new(id).map_err(|error| AppError::internal(error.to_string())))
        .transpose()
}

pub async fn license_plate_dimensions(
    db: &Db,
    access: &TenantAccess,
    license_plate_id: i64,
    include_deleted: bool,
) -> AppResult<Option<OperationalDimensions>> {
    let scope = ScopeBindings::for_access(access);
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let row = sqlx::query(
        r#"
        SELECT facility_id, inventory_owner_id
        FROM license_plates
        WHERE tenant_id = $1
          AND id = $2
          AND ($3 OR deleted IS NULL)
          AND ($4 OR facility_id = ANY($5))
          AND ($6 OR inventory_owner_id = ANY($7))
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(license_plate_id)
    .bind(include_deleted)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?;
    let dimensions = row.as_ref().map(dimensions_from_row).transpose()?;
    tx.commit().await?;
    Ok(dimensions)
}

pub async fn inventory_balance_dimensions(
    db: &Db,
    access: &TenantAccess,
    inventory_balance_id: i64,
    include_deleted: bool,
) -> AppResult<Option<OperationalDimensions>> {
    inventory_dimensions(
        db,
        access,
        InventoryRecord::Balance(inventory_balance_id),
        include_deleted,
    )
    .await
}

pub async fn inventory_position_dimensions(
    db: &Db,
    access: &TenantAccess,
    item_batch_id: i64,
    location_id: i64,
    status: &str,
) -> AppResult<Option<OperationalDimensions>> {
    let scope = ScopeBindings::for_access(access);
    let row = sqlx::query(
        r#"
        SELECT facility_id, inventory_owner_id
        FROM inventory_balances
        WHERE tenant_id = $1
          AND item_batch_id = $2
          AND location_id = $3
          AND status = $4
          AND license_plate_id IS NULL
          AND deleted IS NULL
          AND ($5 OR facility_id = ANY($6))
          AND ($7 OR inventory_owner_id = ANY($8))
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(item_batch_id)
    .bind(location_id)
    .bind(status)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(db)
    .await?;
    row.as_ref().map(dimensions_from_row).transpose()
}

pub async fn inventory_reservation_dimensions(
    db: &Db,
    access: &TenantAccess,
    reservation_id: i64,
    include_deleted: bool,
) -> AppResult<Option<OperationalDimensions>> {
    inventory_dimensions(
        db,
        access,
        InventoryRecord::Reservation(reservation_id),
        include_deleted,
    )
    .await
}

#[derive(Debug, Clone, Copy)]
enum InventoryRecord {
    Balance(i64),
    Reservation(i64),
}

async fn inventory_dimensions(
    db: &Db,
    access: &TenantAccess,
    record: InventoryRecord,
    include_deleted: bool,
) -> AppResult<Option<OperationalDimensions>> {
    let scope = ScopeBindings::for_access(access);
    let (sql, id) = match record {
        InventoryRecord::Balance(id) => (
            r#"
            SELECT facility_id, inventory_owner_id
            FROM inventory_balances
            WHERE tenant_id = $1
              AND id = $2
              AND ($3 OR deleted IS NULL)
              AND ($4 OR facility_id = ANY($5))
              AND ($6 OR inventory_owner_id = ANY($7))
            "#,
            id,
        ),
        InventoryRecord::Reservation(id) => (
            r#"
            SELECT facility_id, inventory_owner_id
            FROM inventory_reservations
            WHERE tenant_id = $1
              AND id = $2
              AND ($3 OR deleted IS NULL)
              AND ($4 OR facility_id = ANY($5))
              AND ($6 OR inventory_owner_id = ANY($7))
            "#,
            id,
        ),
    };
    let row = sqlx::query(sql)
        .bind(access.tenant_id.get())
        .bind(id)
        .bind(include_deleted)
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .fetch_optional(db)
        .await?;
    row.as_ref().map(dimensions_from_row).transpose()
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
