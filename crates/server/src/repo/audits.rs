//! Ported from `app/utils/types/db/audits.ts` (audit waves + location counts).

use sqlx::Row;
use wareboxes_core::models::{AuditLocationCount, AuditWave};
use wareboxes_domain::TenantId;

use crate::db::{now_iso, Db};
use crate::error::{AppError, AppResult};

fn map_wave(row: &sqlx::postgres::PgRow) -> AppResult<AuditWave> {
    Ok(AuditWave {
        id: row.try_get("id")?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        created_by: row.try_get("created_by")?,
    })
}

pub async fn get_audit_waves(
    db: &Db,
    tenant_id: TenantId,
    show_deleted: bool,
) -> AppResult<Vec<AuditWave>> {
    let sql = if show_deleted {
        "SELECT id, tenant_id, created, deleted, name, description, created_by FROM audit_waves WHERE tenant_id = $1 ORDER BY id DESC"
    } else {
        "SELECT id, tenant_id, created, deleted, name, description, created_by FROM audit_waves WHERE tenant_id = $1 AND deleted IS NULL ORDER BY id DESC"
    };
    let rows = sqlx::query(sql).bind(tenant_id.get()).fetch_all(db).await?;
    rows.iter().map(map_wave).collect()
}

pub async fn add_audit_wave(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    name: &str,
    description: Option<&str>,
) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO audit_waves (tenant_id, created, name, description, created_by) VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(tenant_id.get())
    .bind(now_iso())
    .bind(name)
    .bind(description)
    .bind(user_id)
    .fetch_one(db)
    .await?;
    Ok(id)
}

pub async fn update_audit_wave(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
    name: Option<&str>,
    description: Option<&str>,
) -> AppResult<bool> {
    let res = sqlx::query(
        "UPDATE audit_waves SET name = COALESCE($1, name), description = COALESCE($2, description) WHERE tenant_id = $3 AND id = $4",
    )
    .bind(name)
    .bind(description)
    .bind(tenant_id.get())
    .bind(id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_audit_wave_deleted(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
    deleted: bool,
) -> AppResult<bool> {
    let res = sqlx::query("UPDATE audit_waves SET deleted = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(if deleted { Some(now_iso()) } else { None })
        .bind(tenant_id.get())
        .bind(id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn get_location_counts(
    db: &Db,
    tenant_id: TenantId,
    audit_id: i64,
) -> AppResult<Vec<AuditLocationCount>> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, created, deleted, started, ended, audit_id, inventory_owner_id,
               location_id, item_id, lot, expiration, serial, on_hand, count, approval_status
        FROM audit_location_counts
        WHERE tenant_id = $1 AND audit_id = $2 AND deleted IS NULL
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(audit_id)
    .fetch_all(db)
    .await?;
    rows.iter()
        .map(|r| {
            Ok(AuditLocationCount {
                id: r.try_get("id")?,
                tenant_id: TenantId::new(r.try_get("tenant_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                created: r.try_get("created")?,
                deleted: r.try_get("deleted")?,
                started: r.try_get("started")?,
                ended: r.try_get("ended")?,
                audit_id: r.try_get("audit_id")?,
                inventory_owner_id: r.try_get("inventory_owner_id")?,
                location_id: r.try_get("location_id")?,
                item_id: r.try_get("item_id")?,
                lot: r.try_get("lot")?,
                expiration: r.try_get("expiration")?,
                serial: r.try_get("serial")?,
                on_hand: r.try_get("on_hand")?,
                count: r.try_get("count")?,
                approval_status: r.try_get("approval_status")?,
            })
        })
        .collect()
}
