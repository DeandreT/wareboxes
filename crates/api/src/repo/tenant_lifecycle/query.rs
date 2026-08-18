use sqlx::Row;
use wareboxes_application::tenant_lifecycle::{
    TenantLifecycleCursor, TenantLifecycleEventCursor, TenantLifecycleEventPage,
    TenantLifecycleEventPageQuery, TenantLifecycleEventReadModel, TenantLifecyclePage,
    TenantLifecyclePageQuery, TenantLifecycleReadModel,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    DataCellId, DataCellMode, DataCellPlacementRevision, TenantId, TenantRevision, TenantStatus,
    UserId,
};

use crate::db::{begin_tenant_transaction, Db};
use crate::error::{AppError, AppResult};

fn parse_status(value: String) -> AppResult<TenantStatus> {
    TenantStatus::parse(&value)
        .ok_or_else(|| AppError::internal(format!("stored tenant status is invalid: {value}")))
}

fn map_row(row: &sqlx::postgres::PgRow) -> AppResult<TenantLifecycleReadModel> {
    Ok(TenantLifecycleReadModel {
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        slug: row.try_get("slug")?,
        name: row.try_get("name")?,
        status: parse_status(row.try_get("status")?)?,
        revision: TenantRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created_at: row.try_get("created_at")?,
        created_by: row
            .try_get::<Option<i64>, _>("created_by")?
            .map(UserId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        initial_admin_user_id: row
            .try_get::<Option<i64>, _>("initial_admin_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        initial_admin_email: row.try_get("initial_admin_email")?,
        status_changed_at: row.try_get("status_changed_at")?,
        status_changed_by: row
            .try_get::<Option<i64>, _>("status_changed_by")?
            .map(UserId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        status_reason: row.try_get("status_reason")?,
        active_member_count: row.try_get("active_member_count")?,
        active_facility_count: row.try_get("active_facility_count")?,
        active_inventory_owner_count: row.try_get("active_inventory_owner_count")?,
        active_service_account_count: row.try_get("active_service_account_count")?,
        data_cell_id: DataCellId::new(row.try_get("data_cell_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        data_cell_key: row.try_get("data_cell_key")?,
        data_cell_name: row.try_get("data_cell_name")?,
        data_cell_region: row.try_get("data_cell_region")?,
        data_cell_residency: row.try_get("data_cell_residency")?,
        data_cell_mode: DataCellMode::parse(&row.try_get::<String, _>("data_cell_mode")?)
            .ok_or_else(|| AppError::internal("stored data-cell mode is invalid"))?,
        placement_revision: DataCellPlacementRevision::new(row.try_get("placement_revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        residency_requirement: row.try_get("residency_requirement")?,
    })
}

pub(super) async fn read_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
) -> AppResult<TenantLifecycleReadModel> {
    super::bind_platform_tenant_tx(tx, tenant_id).await?;
    let row = sqlx::query(
        r#"SELECT tenant.id AS tenant_id,tenant.slug,tenant.name,tenant.status,
        tenant.revision,tenant.created AS created_at,tenant.created_by_user_id AS created_by,
        tenant.initial_admin_user_id,administrator.email AS initial_admin_email,
        tenant.status_changed_at,tenant.status_changed_by_user_id AS status_changed_by,
        tenant.status_reason,
        placement.data_cell_id,cell.cell_key AS data_cell_key,
        cell.name AS data_cell_name,cell.region AS data_cell_region,
        cell.residency_code AS data_cell_residency,cell.mode AS data_cell_mode,
        placement.revision AS placement_revision,placement.residency_requirement,
        (SELECT COUNT(*) FROM tenant_memberships membership
          WHERE membership.tenant_id=tenant.id AND membership.deleted IS NULL
            AND NOT membership.support_managed)
          AS active_member_count,
        (SELECT COUNT(*) FROM facilities facility
          WHERE facility.tenant_id=tenant.id AND facility.deleted IS NULL)
          AS active_facility_count,
        (SELECT COUNT(*) FROM inventory_owners owner
          WHERE owner.tenant_id=tenant.id AND owner.deleted IS NULL)
          AS active_inventory_owner_count,
        (SELECT COUNT(*) FROM service_accounts account
          WHERE account.tenant_id=tenant.id AND account.status='active')
          AS active_service_account_count
        FROM tenants tenant
        JOIN tenant_cell_placements placement ON placement.tenant_id=tenant.id
        JOIN data_cells cell ON cell.id=placement.data_cell_id
        LEFT JOIN users administrator ON administrator.id=tenant.initial_admin_user_id
        WHERE tenant.id=$1 AND tenant.deleted IS NULL"#,
    )
    .bind(tenant_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("tenant"))?;
    map_row(&row)
}

pub async fn by_id(
    db: &Db,
    actor_access: &TenantAccess,
    tenant_id: TenantId,
) -> AppResult<TenantLifecycleReadModel> {
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    super::authorize_tx(&mut tx, actor_access, actor_access.user_id).await?;
    let result = read_tx(&mut tx, tenant_id).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn page(
    db: &Db,
    actor_access: &TenantAccess,
    query: &TenantLifecyclePageQuery,
) -> AppResult<TenantLifecyclePage> {
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    super::authorize_tx(&mut tx, actor_access, actor_access.user_id).await?;
    let rows = sqlx::query(
        r#"SELECT tenant.id,tenant.created FROM tenants tenant
        WHERE tenant.deleted IS NULL
          AND ($1::TEXT IS NULL OR tenant.status=$1)
          AND ($2::TEXT IS NULL OR tenant.slug ILIKE '%'||$2||'%'
            OR tenant.name ILIKE '%'||$2||'%')
          AND ($3::TIMESTAMPTZ IS NULL OR (tenant.created,tenant.id)<($3,$4))
        ORDER BY tenant.created DESC,tenant.id DESC LIMIT $5"#,
    )
    .bind(query.status.map(|status| status.as_str()))
    .bind(query.search.as_deref())
    .bind(query.cursor.map(|cursor| cursor.after_created_at))
    .bind(query.cursor.map(|cursor| cursor.after_tenant_id.get()))
    .bind(i64::from(query.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let mut identities = rows
        .iter()
        .map(|row| {
            Ok((
                TenantId::new(row.try_get("id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                row.try_get("created")?,
            ))
        })
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = if identities.len() > usize::from(query.limit) {
        identities.pop();
        identities
            .last()
            .map(|(tenant_id, created_at)| TenantLifecycleCursor {
                after_created_at: *created_at,
                after_tenant_id: *tenant_id,
            })
    } else {
        None
    };
    let mut items = Vec::with_capacity(identities.len());
    for (tenant_id, _) in identities {
        items.push(read_tx(&mut tx, tenant_id).await?);
    }
    tx.commit().await?;
    Ok(TenantLifecyclePage { items, next_cursor })
}

fn map_event(row: &sqlx::postgres::PgRow) -> AppResult<TenantLifecycleEventReadModel> {
    Ok(TenantLifecycleEventReadModel {
        event_id: row.try_get("event_id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        action: row.try_get("action")?,
        previous_status: row
            .try_get::<Option<String>, _>("previous_status")?
            .map(parse_status)
            .transpose()?,
        resulting_status: parse_status(row.try_get("resulting_status")?)?,
        tenant_revision: TenantRevision::new(row.try_get("tenant_revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        actor_id: UserId::new(row.try_get("actor_user_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        occurred_at: row.try_get("occurred_at")?,
        reason: row.try_get("reason")?,
        revoked_session_count: row.try_get("revoked_session_count")?,
        revoked_credential_count: row.try_get("revoked_credential_count")?,
        evidence: row.try_get("evidence")?,
    })
}

pub async fn event_page(
    db: &Db,
    actor_access: &TenantAccess,
    query: &TenantLifecycleEventPageQuery,
) -> AppResult<TenantLifecycleEventPage> {
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    super::authorize_tx(&mut tx, actor_access, actor_access.user_id).await?;
    read_tx(&mut tx, query.tenant_id).await?;
    let rows = sqlx::query(
        r#"SELECT id AS event_id,tenant_id,action,previous_status,resulting_status,
        tenant_revision,actor_user_id,occurred_at,reason,revoked_session_count,
        revoked_credential_count,evidence
        FROM tenant_lifecycle_events WHERE tenant_id=$1
          AND ($2::TIMESTAMPTZ IS NULL OR (occurred_at,id)<($2,$3))
        ORDER BY occurred_at DESC,id DESC LIMIT $4"#,
    )
    .bind(query.tenant_id.get())
    .bind(query.cursor.map(|cursor| cursor.after_occurred_at))
    .bind(query.cursor.map(|cursor| cursor.after_event_id))
    .bind(i64::from(query.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let mut items = rows.iter().map(map_event).collect::<AppResult<Vec<_>>>()?;
    let next_cursor = if items.len() > usize::from(query.limit) {
        items.pop();
        items.last().map(|event| TenantLifecycleEventCursor {
            after_occurred_at: event.occurred_at,
            after_event_id: event.event_id,
        })
    } else {
        None
    };
    tx.commit().await?;
    Ok(TenantLifecycleEventPage { items, next_cursor })
}
