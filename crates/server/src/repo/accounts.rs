//! Ported from `app/utils/accounts.ts`.

use std::collections::HashMap;

use sqlx::Row;
use wareboxes_core::models::{Account, Warehouse};
use wareboxes_core::CoreError;

use crate::db::{now_iso, Db};
use crate::error::{AppError, AppResult};

fn map_account(row: &sqlx::postgres::PgRow) -> AppResult<Account> {
    Ok(Account {
        id: row.try_get("id")?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        name: row.try_get("name")?,
        email: row.try_get("email")?,
        account_warehouses: Vec::new(),
    })
}

async fn warehouses_by_account(db: &Db) -> AppResult<HashMap<i64, Vec<Warehouse>>> {
    let rows = sqlx::query(
        r#"
        SELECT aw.account_id AS account_id,
               w.id AS id, w.created AS created, w.deleted AS deleted,
               w.name AS name, w.address_id AS address_id
        FROM account_warehouses aw
        INNER JOIN warehouses w ON w.id = aw.warehouse_id
        WHERE aw.deleted IS NULL AND w.deleted IS NULL
        "#,
    )
    .fetch_all(db)
    .await?;
    let mut map: HashMap<i64, Vec<Warehouse>> = HashMap::new();
    for r in &rows {
        let acc = r.try_get("account_id")?;
        map.entry(acc).or_default().push(Warehouse {
            id: r.try_get("id")?,
            created: r.try_get("created")?,
            deleted: r.try_get("deleted")?,
            name: r.try_get("name")?,
            address_id: r.try_get("address_id")?,
        });
    }
    Ok(map)
}

pub async fn get_accounts(db: &Db, show_deleted: bool) -> AppResult<Vec<Account>> {
    let sql = if show_deleted {
        "SELECT id, created, deleted, name, email FROM accounts ORDER BY id"
    } else {
        "SELECT id, created, deleted, name, email FROM accounts WHERE deleted IS NULL ORDER BY id"
    };
    let rows = sqlx::query(sql).fetch_all(db).await?;
    let mut wh = warehouses_by_account(db).await?;
    rows.iter()
        .map(|r| {
            let mut a = map_account(r)?;
            a.account_warehouses = wh.remove(&a.id).unwrap_or_default();
            Ok(a)
        })
        .collect()
}

pub async fn active_account_exists(db: &Db, id: i64) -> AppResult<bool> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM accounts WHERE id = $1 AND deleted IS NULL)",
    )
    .bind(id)
    .fetch_one(db)
    .await?;
    Ok(exists)
}

pub async fn add_account(db: &Db, name: &str, email: &str) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO accounts (name, email, created) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(name)
    .bind(email)
    .bind(now_iso())
    .fetch_one(db)
    .await?;
    Ok(id)
}

pub async fn update_account(
    db: &Db,
    id: i64,
    name: Option<&str>,
    email: Option<&str>,
) -> AppResult<bool> {
    let res = sqlx::query(
        "UPDATE accounts SET name = COALESCE($1, name), email = COALESCE($2, email) WHERE id = $3",
    )
    .bind(name)
    .bind(email)
    .bind(id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// `deleteAccount` — refuses if the account still has orders that are not
/// shipped or cancelled.
pub async fn delete_account(db: &Db, id: i64) -> AppResult<bool> {
    let open: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM orders
        WHERE account_id = $1 AND status NOT IN ('shipped', 'cancelled')
        "#,
    )
    .bind(id)
    .fetch_one(db)
    .await?;
    if open > 0 {
        return Err(AppError::Core(CoreError::Conflict(
            "Account has orders that are not shipped or cancelled".into(),
        )));
    }
    let res = sqlx::query("UPDATE accounts SET deleted = $1 WHERE id = $2")
        .bind(now_iso())
        .bind(id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn restore_account(db: &Db, id: i64) -> AppResult<bool> {
    let res = sqlx::query("UPDATE accounts SET deleted = NULL WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}
