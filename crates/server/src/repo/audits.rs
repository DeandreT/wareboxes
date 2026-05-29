//! Ported from `app/utils/types/db/audits.ts` (audit waves + location counts).

use sqlx::Row;
use wareboxes_core::models::{AuditLocationCount, AuditWave};

use crate::db::{now_iso, Db};
use crate::error::AppResult;

fn map_wave(row: &sqlx::postgres::PgRow) -> AppResult<AuditWave> {
    Ok(AuditWave {
        id: row.try_get("id")?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
    })
}

pub async fn get_audit_waves(db: &Db, show_deleted: bool) -> AppResult<Vec<AuditWave>> {
    let sql = if show_deleted {
        "SELECT id, created, deleted, name, description FROM audit_waves ORDER BY id DESC"
    } else {
        "SELECT id, created, deleted, name, description FROM audit_waves WHERE deleted IS NULL ORDER BY id DESC"
    };
    let rows = sqlx::query(sql).fetch_all(db).await?;
    rows.iter().map(map_wave).collect()
}

pub async fn add_audit_wave(db: &Db, name: &str, description: Option<&str>) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO audit_waves (created, name, description) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(now_iso())
    .bind(name)
    .bind(description)
    .fetch_one(db)
    .await?;
    Ok(id)
}

pub async fn update_audit_wave(
    db: &Db,
    id: i64,
    name: Option<&str>,
    description: Option<&str>,
) -> AppResult<bool> {
    let res = sqlx::query(
        "UPDATE audit_waves SET name = COALESCE($1, name), description = COALESCE($2, description) WHERE id = $3",
    )
    .bind(name)
    .bind(description)
    .bind(id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_audit_wave_deleted(db: &Db, id: i64, deleted: bool) -> AppResult<bool> {
    let res = sqlx::query("UPDATE audit_waves SET deleted = $1 WHERE id = $2")
        .bind(if deleted { Some(now_iso()) } else { None })
        .bind(id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn get_location_counts(db: &Db, audit_id: i64) -> AppResult<Vec<AuditLocationCount>> {
    let rows = sqlx::query(
        r#"
        SELECT id, created, deleted, started, ended, audit_id, location_id, item_id,
               lot, expiration, serial, on_hand, count, approval_status
        FROM audit_location_counts
        WHERE audit_id = $1 AND deleted IS NULL
        ORDER BY id
        "#,
    )
    .bind(audit_id)
    .fetch_all(db)
    .await?;
    rows.iter()
        .map(|r| {
            Ok(AuditLocationCount {
                id: r.try_get("id")?,
                created: r.try_get("created")?,
                deleted: r.try_get("deleted")?,
                started: r.try_get("started")?,
                ended: r.try_get("ended")?,
                audit_id: r.try_get("audit_id")?,
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
