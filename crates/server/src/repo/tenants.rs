use sqlx::Row;
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_core::models::{OwnerScope, SiteScope, TenantAccess, TenantStatus};
use wareboxes_domain::{FacilityId, InventoryOwnerId, TenantId, UserId};

use crate::db::Db;
use crate::error::{AppError, AppResult};

use super::access::ScopeBindings;

fn tenant_access_from_row(row: &sqlx::postgres::PgRow) -> AppResult<TenantAccess> {
    let status: String = row.try_get("status")?;
    let facility_ids = row
        .try_get::<Vec<i64>, _>("facility_ids")?
        .into_iter()
        .map(|id| FacilityId::new(id).map_err(|error| AppError::internal(error.to_string())))
        .collect::<AppResult<Vec<_>>>()?;
    let inventory_owner_ids = row
        .try_get::<Vec<i64>, _>("inventory_owner_ids")?
        .into_iter()
        .map(|id| InventoryOwnerId::new(id).map_err(|error| AppError::internal(error.to_string())))
        .collect::<AppResult<Vec<_>>>()?;
    Ok(TenantAccess {
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        user_id: UserId::new(row.try_get("user_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        slug: row.try_get("slug")?,
        name: row.try_get("name")?,
        status: TenantStatus::parse(&status)
            .ok_or_else(|| AppError::internal(format!("unknown tenant status: {status}")))?,
        is_default: row.try_get("is_default")?,
        site_scope: SiteScope {
            all_facilities: row.try_get("all_facilities")?,
            facility_ids,
        },
        owner_scope: OwnerScope {
            all_inventory_owners: row.try_get("all_inventory_owners")?,
            inventory_owner_ids,
        },
    })
}

pub async fn list_for_user(db: &Db, user_id: i64) -> AppResult<Vec<TenantAccess>> {
    let rows = sqlx::query(
        r#"
        SELECT
            membership.tenant_id,
            membership.user_id,
            membership.is_default,
            membership.all_facilities,
            membership.all_inventory_owners,
            ARRAY(
                SELECT user_facility.facility_id
                FROM user_facilities user_facility
                WHERE user_facility.tenant_id = membership.tenant_id
                  AND user_facility.user_id = membership.user_id
                  AND user_facility.deleted IS NULL
                ORDER BY user_facility.facility_id
            ) AS facility_ids,
            ARRAY(
                SELECT user_owner.inventory_owner_id
                FROM user_inventory_owners user_owner
                WHERE user_owner.tenant_id = membership.tenant_id
                  AND user_owner.user_id = membership.user_id
                  AND user_owner.deleted IS NULL
                ORDER BY user_owner.inventory_owner_id
            ) AS inventory_owner_ids,
            tenant.slug,
            tenant.name,
            tenant.status
        FROM tenant_memberships membership
        JOIN tenants tenant ON tenant.id = membership.tenant_id
        WHERE membership.user_id = $1
          AND membership.deleted IS NULL
          AND tenant.deleted IS NULL
          AND tenant.status = 'active'
        ORDER BY membership.is_default DESC, tenant.name, tenant.id
        "#,
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;

    rows.iter().map(tenant_access_from_row).collect()
}

pub async fn default_for_user(db: &Db, user_id: i64) -> AppResult<Option<TenantAccess>> {
    let row = sqlx::query(
        r#"
        SELECT
            membership.tenant_id,
            membership.user_id,
            membership.is_default,
            membership.all_facilities,
            membership.all_inventory_owners,
            ARRAY(
                SELECT user_facility.facility_id
                FROM user_facilities user_facility
                WHERE user_facility.tenant_id = membership.tenant_id
                  AND user_facility.user_id = membership.user_id
                  AND user_facility.deleted IS NULL
                ORDER BY user_facility.facility_id
            ) AS facility_ids,
            ARRAY(
                SELECT user_owner.inventory_owner_id
                FROM user_inventory_owners user_owner
                WHERE user_owner.tenant_id = membership.tenant_id
                  AND user_owner.user_id = membership.user_id
                  AND user_owner.deleted IS NULL
                ORDER BY user_owner.inventory_owner_id
            ) AS inventory_owner_ids,
            tenant.slug,
            tenant.name,
            tenant.status
        FROM tenant_memberships membership
        JOIN tenants tenant ON tenant.id = membership.tenant_id
        WHERE membership.user_id = $1
          AND membership.deleted IS NULL
          AND tenant.deleted IS NULL
          AND tenant.status = 'active'
        ORDER BY membership.is_default DESC, tenant.name, tenant.id
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;

    row.as_ref().map(tenant_access_from_row).transpose()
}

pub async fn access_for_user(
    db: &Db,
    user_id: i64,
    tenant_id: TenantId,
) -> AppResult<Option<TenantAccess>> {
    let row = sqlx::query(
        r#"
        SELECT
            membership.tenant_id,
            membership.user_id,
            membership.is_default,
            membership.all_facilities,
            membership.all_inventory_owners,
            ARRAY(
                SELECT user_facility.facility_id
                FROM user_facilities user_facility
                WHERE user_facility.tenant_id = membership.tenant_id
                  AND user_facility.user_id = membership.user_id
                  AND user_facility.deleted IS NULL
                ORDER BY user_facility.facility_id
            ) AS facility_ids,
            ARRAY(
                SELECT user_owner.inventory_owner_id
                FROM user_inventory_owners user_owner
                WHERE user_owner.tenant_id = membership.tenant_id
                  AND user_owner.user_id = membership.user_id
                  AND user_owner.deleted IS NULL
                ORDER BY user_owner.inventory_owner_id
            ) AS inventory_owner_ids,
            tenant.slug,
            tenant.name,
            tenant.status
        FROM tenant_memberships membership
        JOIN tenants tenant ON tenant.id = membership.tenant_id
        WHERE membership.user_id = $1
          AND membership.tenant_id = $2
          AND membership.deleted IS NULL
          AND tenant.deleted IS NULL
          AND tenant.status = 'active'
        "#,
    )
    .bind(user_id)
    .bind(tenant_id.get())
    .fetch_optional(db)
    .await?;

    row.as_ref().map(tenant_access_from_row).transpose()
}

fn validate_scope_request(scope: &UpdateUserAccessScope) -> AppResult<()> {
    if scope.all_facilities && !scope.facility_ids.is_empty() {
        return Err(AppError::bad_request(
            "facility_ids must be empty when all_facilities is true",
        ));
    }
    if scope.all_inventory_owners && !scope.inventory_owner_ids.is_empty() {
        return Err(AppError::bad_request(
            "inventory_owner_ids must be empty when all_inventory_owners is true",
        ));
    }

    let mut facility_ids = scope.facility_ids.clone();
    facility_ids.sort_unstable();
    if facility_ids.iter().any(|id| *id <= 0) || facility_ids.windows(2).any(|ids| ids[0] == ids[1])
    {
        return Err(AppError::bad_request(
            "facility_ids must contain unique positive IDs",
        ));
    }

    let mut inventory_owner_ids = scope.inventory_owner_ids.clone();
    inventory_owner_ids.sort_unstable();
    if inventory_owner_ids.iter().any(|id| *id <= 0)
        || inventory_owner_ids.windows(2).any(|ids| ids[0] == ids[1])
    {
        return Err(AppError::bad_request(
            "inventory_owner_ids must contain unique positive IDs",
        ));
    }
    Ok(())
}

/// Atomically replaces an active tenant member's complete resource scope.
pub async fn update_user_access_scope(
    db: &Db,
    tenant_id: TenantId,
    scope: &UpdateUserAccessScope,
) -> AppResult<bool> {
    validate_scope_request(scope)?;
    let mut transaction = db.begin().await?;

    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::TEXT || ':' || $2::TEXT, 0))")
        .bind(tenant_id.get())
        .bind(scope.user_id)
        .execute(&mut *transaction)
        .await?;

    let membership_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id
        FROM tenant_memberships
        WHERE tenant_id = $1 AND user_id = $2 AND deleted IS NULL
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(scope.user_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if membership_id.is_none() {
        transaction.rollback().await?;
        return Ok(false);
    }

    let facility_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM facilities
        WHERE tenant_id = $1 AND id = ANY($2) AND deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(&scope.facility_ids)
    .fetch_one(&mut *transaction)
    .await?;
    let expected_facility_count = i64::try_from(scope.facility_ids.len())
        .map_err(|_| AppError::bad_request("too many facility IDs"))?;
    if facility_count != expected_facility_count {
        return Err(AppError::bad_request(
            "facility_ids contains an unavailable facility",
        ));
    }

    let owner_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM inventory_owners
        WHERE tenant_id = $1 AND id = ANY($2) AND deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(&scope.inventory_owner_ids)
    .fetch_one(&mut *transaction)
    .await?;
    let expected_owner_count = i64::try_from(scope.inventory_owner_ids.len())
        .map_err(|_| AppError::bad_request("too many inventory owner IDs"))?;
    if owner_count != expected_owner_count {
        return Err(AppError::bad_request(
            "inventory_owner_ids contains an unavailable inventory owner",
        ));
    }

    sqlx::query(
        r#"
        UPDATE tenant_memberships
        SET all_facilities = $1, all_inventory_owners = $2
        WHERE tenant_id = $3 AND user_id = $4 AND deleted IS NULL
        "#,
    )
    .bind(scope.all_facilities)
    .bind(scope.all_inventory_owners)
    .bind(tenant_id.get())
    .bind(scope.user_id)
    .execute(&mut *transaction)
    .await?;

    sqlx::query(
        r#"
        UPDATE user_facilities
        SET deleted = CURRENT_TIMESTAMP
        WHERE tenant_id = $1 AND user_id = $2 AND deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(scope.user_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO user_facilities (tenant_id, user_id, facility_id)
        SELECT $1, $2, UNNEST($3::BIGINT[])
        ON CONFLICT (tenant_id, user_id, facility_id) DO UPDATE
        SET created = CURRENT_TIMESTAMP, deleted = NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(scope.user_id)
    .bind(&scope.facility_ids)
    .execute(&mut *transaction)
    .await?;

    sqlx::query(
        r#"
        UPDATE user_inventory_owners
        SET deleted = CURRENT_TIMESTAMP
        WHERE tenant_id = $1 AND user_id = $2 AND deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(scope.user_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO user_inventory_owners
            (tenant_id, created, user_id, inventory_owner_id)
        SELECT $1, CURRENT_TIMESTAMP, $2, UNNEST($3::BIGINT[])
        ON CONFLICT (tenant_id, user_id, inventory_owner_id) DO UPDATE
        SET created = CURRENT_TIMESTAMP, deleted = NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(scope.user_id)
    .bind(&scope.inventory_owner_ids)
    .execute(&mut *transaction)
    .await?;

    super::tasks::release_tasks_outside_scope_tx(
        &mut transaction,
        tenant_id,
        scope.user_id,
        &ScopeBindings {
            all_facilities: scope.all_facilities,
            facility_ids: scope.facility_ids.clone(),
            all_inventory_owners: scope.all_inventory_owners,
            inventory_owner_ids: scope.inventory_owner_ids.clone(),
        },
    )
    .await?;

    transaction.commit().await?;
    Ok(true)
}

pub async fn ensure_default_for_user(
    db: &Db,
    user_id: i64,
    tenant_name: &str,
) -> AppResult<TenantId> {
    let mut transaction = db.begin().await?;

    // Serialize provisioning for a user so concurrent login/registration paths
    // cannot create multiple default memberships.
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('tenant-provision:' || $1::TEXT, 0))",
    )
    .bind(user_id)
    .execute(&mut *transaction)
    .await?;

    let existing: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT tenant_id
        FROM tenant_memberships
        WHERE user_id = $1 AND deleted IS NULL AND is_default
        "#,
    )
    .bind(user_id)
    .fetch_optional(&mut *transaction)
    .await?;

    if let Some(tenant_id) = existing {
        transaction.commit().await?;
        return TenantId::new(tenant_id).map_err(|error| AppError::internal(error.to_string()));
    }

    let slug = format!("tenant-{user_id}");
    let tenant_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO tenants (slug, name)
        VALUES ($1, $2)
        ON CONFLICT (slug) DO UPDATE
        SET deleted = NULL
        RETURNING id
        "#,
    )
    .bind(slug)
    .bind(tenant_name)
    .fetch_one(&mut *transaction)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO tenant_memberships (tenant_id, user_id, is_default)
        VALUES ($1, $2, TRUE)
        ON CONFLICT (tenant_id, user_id) DO UPDATE
        SET deleted = NULL, is_default = TRUE
        "#,
    )
    .bind(tenant_id)
    .bind(user_id)
    .execute(&mut *transaction)
    .await?;

    transaction.commit().await?;
    TenantId::new(tenant_id).map_err(|error| AppError::internal(error.to_string()))
}
