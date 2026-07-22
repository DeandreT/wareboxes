//! Ported from `app/utils/roles.ts`. The original computed parent/child role
//! closures and inherited permissions with recursive CTEs + `json_agg`; here
//! we load flat rows and build the closures in Rust so the SQL stays portable.

use std::collections::{HashMap, HashSet};

use sqlx::Row;
use wareboxes_core::models::{Permission, Role};
use wareboxes_core::CoreError;

use crate::db::{now_iso, Db};
use crate::error::{AppError, AppResult};

fn map_role(row: &sqlx::postgres::PgRow) -> AppResult<Role> {
    Ok(Role {
        id: row.try_get("id")?,
        created: row.try_get("created")?,
        deleted: row.try_get("deleted")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        parent_id: row.try_get("parent_id")?,
        parent_roles: Vec::new(),
        child_roles: Vec::new(),
        role_permissions: Vec::new(),
    })
}

/// Every non-deleted role (used as the graph for closure computation).
async fn load_all(db: &Db) -> AppResult<Vec<Role>> {
    let rows = sqlx::query(
        "SELECT id, created, deleted, name, description, parent_id FROM roles WHERE deleted IS NULL",
    )
    .fetch_all(db)
    .await?;
    rows.iter().map(map_role).collect()
}

fn ancestors(id: i64, parent_of: &HashMap<i64, Option<i64>>) -> Vec<i64> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut cur = parent_of.get(&id).copied().flatten();
    while let Some(p) = cur {
        if !seen.insert(p) {
            break; // cycle guard
        }
        out.push(p);
        cur = parent_of.get(&p).copied().flatten();
    }
    out
}

fn descendants(id: i64, children_of: &HashMap<i64, Vec<i64>>) -> Vec<i64> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut stack = children_of.get(&id).cloned().unwrap_or_default();
    while let Some(c) = stack.pop() {
        if !seen.insert(c) {
            continue;
        }
        out.push(c);
        if let Some(cc) = children_of.get(&c) {
            stack.extend(cc.iter().copied());
        }
    }
    out
}

async fn role_permission_map(db: &Db) -> AppResult<HashMap<i64, Vec<Permission>>> {
    let rows = sqlx::query(
        r#"
        SELECT rp.role_id AS role_id,
               p.id AS id, p.created AS created, p.deleted AS deleted,
               UPPER(p.name) AS name, p.description AS description
        FROM role_permissions rp
        INNER JOIN permissions p ON p.id = rp.permission_id
        WHERE rp.deleted IS NULL AND p.deleted IS NULL
        "#,
    )
    .fetch_all(db)
    .await?;
    let mut map: HashMap<i64, Vec<Permission>> = HashMap::new();
    for r in &rows {
        let role_id = r.try_get("role_id")?;
        map.entry(role_id).or_default().push(Permission {
            id: r.try_get("id")?,
            created: r.try_get("created")?,
            deleted: r.try_get("deleted")?,
            name: r.try_get("name")?,
            description: r.try_get("description")?,
        });
    }
    Ok(map)
}

/// `getRoles(showDeleted, showSelfRole)` — roles enriched with their parent
/// closure, child closure and inherited permissions (self + ancestors).
pub async fn get_roles(db: &Db, show_deleted: bool, show_self_role: bool) -> AppResult<Vec<Role>> {
    let all = load_all(db).await?;
    let by_id: HashMap<i64, Role> = all.iter().map(|r| (r.id, r.clone())).collect();
    let parent_of: HashMap<i64, Option<i64>> = all.iter().map(|r| (r.id, r.parent_id)).collect();
    let mut children_of: HashMap<i64, Vec<i64>> = HashMap::new();
    for r in &all {
        if let Some(p) = r.parent_id {
            children_of.entry(p).or_default().push(r.id);
        }
    }
    let perm_map = role_permission_map(db).await?;

    // Base set is read separately so show_deleted is honoured for the rows
    // themselves while closures still resolve against all live roles.
    let base_sql = "SELECT id, created, deleted, name, description, parent_id FROM roles";
    let base_rows = sqlx::query(base_sql).fetch_all(db).await?;
    let mut roles: Vec<Role> = base_rows.iter().map(map_role).collect::<AppResult<_>>()?;

    roles.retain(|r| {
        (show_deleted || r.deleted.is_none())
            && (show_self_role || r.description.as_deref() != Some(Role::SELF_ROLE_DESCRIPTION))
    });
    roles.sort_by_key(|role| std::cmp::Reverse(role.created));

    for role in &mut roles {
        let anc = ancestors(role.id, &parent_of);
        role.parent_roles = anc.iter().filter_map(|id| by_id.get(id).cloned()).collect();
        role.child_roles = descendants(role.id, &children_of)
            .iter()
            .filter_map(|id| by_id.get(id).cloned())
            .collect();

        let mut perms: Vec<Permission> = Vec::new();
        let mut seen = HashSet::new();
        for rid in std::iter::once(role.id).chain(anc) {
            if let Some(ps) = perm_map.get(&rid) {
                for p in ps {
                    if seen.insert(p.id) {
                        perms.push(p.clone());
                    }
                }
            }
        }
        role.role_permissions = perms;
    }
    Ok(roles)
}

pub async fn get_role(db: &Db, id: i64) -> AppResult<Option<Role>> {
    Ok(get_roles(db, true, true)
        .await?
        .into_iter()
        .find(|r| r.id == id))
}

pub async fn add_role(db: &Db, name: &str, description: Option<&str>) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO roles (name, description, created) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(name)
    .bind(description)
    .bind(now_iso())
    .fetch_one(db)
    .await?;
    Ok(id)
}

pub async fn update_role(
    db: &Db,
    id: i64,
    name: Option<&str>,
    description: Option<&str>,
) -> AppResult<bool> {
    if name.is_none() && description.is_none() {
        return Err(AppError::Core(CoreError::BadRequest(
            "No data to update".into(),
        )));
    }
    let res = sqlx::query(
        "UPDATE roles SET name = COALESCE($1, name), description = COALESCE($2, description) WHERE id = $3",
    )
    .bind(name)
    .bind(description)
    .bind(id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_role_deleted(db: &Db, id: i64, deleted: bool) -> AppResult<bool> {
    let res = sqlx::query("UPDATE roles SET deleted = $1 WHERE id = $2")
        .bind(if deleted { Some(now_iso()) } else { None })
        .bind(id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// `addRoleToUser` — upsert that revives a soft-deleted assignment.
pub async fn add_role_to_user(db: &Db, user_id: i64, role_id: i64) -> AppResult<bool> {
    let existing: Option<i64> =
        sqlx::query_scalar("SELECT id FROM user_roles WHERE user_id = $1 AND role_id = $2")
            .bind(user_id)
            .bind(role_id)
            .fetch_optional(db)
            .await?;
    if let Some(ur) = existing {
        sqlx::query("UPDATE user_roles SET deleted = NULL WHERE id = $1")
            .bind(ur)
            .execute(db)
            .await?;
    } else {
        sqlx::query("INSERT INTO user_roles (user_id, role_id, created) VALUES ($1, $2, $3)")
            .bind(user_id)
            .bind(role_id)
            .bind(now_iso())
            .execute(db)
            .await?;
    }
    Ok(true)
}

/// `deleteUserRole` — refuses to remove the per-user "self role".
pub async fn delete_user_role(db: &Db, user_id: i64, role_id: i64) -> AppResult<bool> {
    let desc: Option<String> = sqlx::query_scalar(
        r#"
        SELECT r.description
        FROM roles r
        INNER JOIN user_roles ur ON ur.role_id = r.id
        WHERE ur.user_id = $1 AND ur.role_id = $2
        "#,
    )
    .bind(user_id)
    .bind(role_id)
    .fetch_optional(db)
    .await?
    .flatten();
    if desc.as_deref() == Some(Role::SELF_ROLE_DESCRIPTION) {
        return Err(AppError::Core(CoreError::BadRequest(
            "Cannot delete self role".into(),
        )));
    }
    let res = sqlx::query("UPDATE user_roles SET deleted = $1 WHERE user_id = $2 AND role_id = $3")
        .bind(now_iso())
        .bind(user_id)
        .bind(role_id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn add_role_permission(db: &Db, role_id: i64, permission_id: i64) -> AppResult<bool> {
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM role_permissions WHERE role_id = $1 AND permission_id = $2",
    )
    .bind(role_id)
    .bind(permission_id)
    .fetch_optional(db)
    .await?;
    if let Some(rp) = existing {
        sqlx::query("UPDATE role_permissions SET deleted = NULL WHERE id = $1")
            .bind(rp)
            .execute(db)
            .await?;
    } else {
        sqlx::query(
            "INSERT INTO role_permissions (role_id, permission_id, created) VALUES ($1, $2, $3)",
        )
        .bind(role_id)
        .bind(permission_id)
        .bind(now_iso())
        .execute(db)
        .await?;
    }
    Ok(true)
}

pub async fn delete_role_permission(db: &Db, role_id: i64, permission_id: i64) -> AppResult<bool> {
    let res = sqlx::query(
        "UPDATE role_permissions SET deleted = $1 WHERE role_id = $2 AND permission_id = $3",
    )
    .bind(now_iso())
    .bind(role_id)
    .bind(permission_id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// `addRoleRelationship` — set `child.parent_id = parent`, rejecting self-links
/// and cycles (child already an ancestor/descendant).
pub async fn add_role_relationship(db: &Db, parent_id: i64, child_id: i64) -> AppResult<bool> {
    if parent_id == child_id {
        return Err(AppError::Core(CoreError::BadRequest(
            "Parent and child roles cannot be the same".into(),
        )));
    }
    let all = load_all(db).await?;
    let parent_of: HashMap<i64, Option<i64>> = all.iter().map(|r| (r.id, r.parent_id)).collect();
    let mut children_of: HashMap<i64, Vec<i64>> = HashMap::new();
    for r in &all {
        if let Some(p) = r.parent_id {
            children_of.entry(p).or_default().push(r.id);
        }
    }
    if descendants(parent_id, &children_of).contains(&child_id) {
        return Err(AppError::Core(CoreError::BadRequest(
            "Child role is already a child of the parent role".into(),
        )));
    }
    if ancestors(parent_id, &parent_of).contains(&child_id) {
        return Err(AppError::Core(CoreError::BadRequest(
            "Child role is a parent of the parent role".into(),
        )));
    }
    let res = sqlx::query("UPDATE roles SET parent_id = $1 WHERE id = $2")
        .bind(parent_id)
        .bind(child_id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn delete_role_relationship(db: &Db, child_id: i64) -> AppResult<bool> {
    let res = sqlx::query("UPDATE roles SET parent_id = NULL WHERE id = $1")
        .bind(child_id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}
