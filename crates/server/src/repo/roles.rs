//! Ported from `app/utils/roles.ts`. The original computed parent/child role
//! closures and inherited permissions with recursive CTEs + `json_agg`; here
//! we load flat rows and build the closures in Rust so the SQL stays portable.

use std::collections::{HashMap, HashSet};

use sqlx::Row;
use wareboxes_core::models::{Permission, Role};
use wareboxes_core::CoreError;
use wareboxes_domain::TenantId;

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
        self_user_id: row.try_get("self_user_id")?,
        parent_roles: Vec::new(),
        child_roles: Vec::new(),
        role_permissions: Vec::new(),
    })
}

/// Every non-deleted role (used as the graph for closure computation).
async fn load_all(db: &Db, tenant_id: TenantId) -> AppResult<Vec<Role>> {
    let rows = sqlx::query(
        "SELECT id, created, deleted, name, description, parent_id, self_user_id FROM roles WHERE tenant_id = $1 AND deleted IS NULL",
    )
    .bind(tenant_id.get())
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

async fn role_permission_map(
    db: &Db,
    tenant_id: TenantId,
) -> AppResult<HashMap<i64, Vec<Permission>>> {
    let rows = sqlx::query(
        r#"
        SELECT rp.role_id AS role_id,
               p.id AS id, p.created AS created, p.deleted AS deleted,
               UPPER(p.name) AS name, p.description AS description
        FROM role_permissions rp
        INNER JOIN permissions p
            ON p.tenant_id = rp.tenant_id AND p.id = rp.permission_id
        WHERE rp.tenant_id = $1 AND rp.deleted IS NULL AND p.deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
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
pub async fn get_roles(
    db: &Db,
    tenant_id: TenantId,
    show_deleted: bool,
    show_self_role: bool,
) -> AppResult<Vec<Role>> {
    let all = load_all(db, tenant_id).await?;
    let by_id: HashMap<i64, Role> = all.iter().map(|r| (r.id, r.clone())).collect();
    let parent_of: HashMap<i64, Option<i64>> = all.iter().map(|r| (r.id, r.parent_id)).collect();
    let mut children_of: HashMap<i64, Vec<i64>> = HashMap::new();
    for r in &all {
        if let Some(p) = r.parent_id {
            children_of.entry(p).or_default().push(r.id);
        }
    }
    let perm_map = role_permission_map(db, tenant_id).await?;

    // Base set is read separately so show_deleted is honoured for the rows
    // themselves while closures still resolve against all live roles.
    let base_sql = "SELECT id, created, deleted, name, description, parent_id, self_user_id FROM roles WHERE tenant_id = $1";
    let base_rows = sqlx::query(base_sql)
        .bind(tenant_id.get())
        .fetch_all(db)
        .await?;
    let mut roles: Vec<Role> = base_rows.iter().map(map_role).collect::<AppResult<_>>()?;

    roles
        .retain(|r| (show_deleted || r.deleted.is_none()) && (show_self_role || !r.is_self_role()));
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

pub async fn get_role(db: &Db, tenant_id: TenantId, id: i64) -> AppResult<Option<Role>> {
    Ok(get_roles(db, tenant_id, true, true)
        .await?
        .into_iter()
        .find(|r| r.id == id))
}

pub async fn add_role(
    db: &Db,
    tenant_id: TenantId,
    name: &str,
    description: Option<&str>,
) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO roles (tenant_id, name, description, created) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(tenant_id.get())
    .bind(name)
    .bind(description)
    .bind(now_iso())
    .fetch_one(db)
    .await?;
    Ok(id)
}

pub async fn update_role(
    db: &Db,
    tenant_id: TenantId,
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
        "UPDATE roles SET name = COALESCE($1, name), description = COALESCE($2, description) WHERE tenant_id = $3 AND id = $4 AND self_user_id IS NULL",
    )
    .bind(name)
    .bind(description)
    .bind(tenant_id.get())
    .bind(id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_role_deleted(
    db: &Db,
    tenant_id: TenantId,
    id: i64,
    deleted: bool,
) -> AppResult<bool> {
    let res = sqlx::query(
        "UPDATE roles SET deleted = $1 WHERE tenant_id = $2 AND id = $3 AND self_user_id IS NULL",
    )
    .bind(if deleted { Some(now_iso()) } else { None })
    .bind(tenant_id.get())
    .bind(id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// `addRoleToUser` — upsert that revives a soft-deleted assignment.
pub async fn add_role_to_user(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    role_id: i64,
) -> AppResult<bool> {
    let row: Option<i64> = sqlx::query_scalar(
        r#"
        INSERT INTO user_roles (tenant_id, user_id, role_id, created)
        SELECT $1, $2, $3, $4
        WHERE EXISTS (
            SELECT 1 FROM tenant_memberships
            WHERE tenant_id = $1 AND user_id = $2 AND deleted IS NULL
        )
          AND EXISTS (
            SELECT 1 FROM roles
            WHERE tenant_id = $1
              AND id = $3
              AND deleted IS NULL
              AND (self_user_id IS NULL OR self_user_id = $2)
        )
        ON CONFLICT (tenant_id, user_id, role_id) DO UPDATE
        SET deleted = NULL
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(user_id)
    .bind(role_id)
    .bind(now_iso())
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// `deleteUserRole` — refuses to remove the per-user "self role".
pub async fn delete_user_role(
    db: &Db,
    tenant_id: TenantId,
    user_id: i64,
    role_id: i64,
) -> AppResult<bool> {
    let self_user_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT r.self_user_id
        FROM roles r
        INNER JOIN user_roles ur ON ur.tenant_id = r.tenant_id AND ur.role_id = r.id
        WHERE ur.tenant_id = $1 AND ur.user_id = $2 AND ur.role_id = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(user_id)
    .bind(role_id)
    .fetch_optional(db)
    .await?
    .flatten();
    if self_user_id.is_some() {
        return Err(AppError::Core(CoreError::BadRequest(
            "Cannot delete self role".into(),
        )));
    }
    let res = sqlx::query(
        "UPDATE user_roles SET deleted = $1 WHERE tenant_id = $2 AND user_id = $3 AND role_id = $4",
    )
    .bind(now_iso())
    .bind(tenant_id.get())
    .bind(user_id)
    .bind(role_id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn add_role_permission(
    db: &Db,
    tenant_id: TenantId,
    role_id: i64,
    permission_id: i64,
) -> AppResult<bool> {
    let row: Option<i64> = sqlx::query_scalar(
        r#"
        INSERT INTO role_permissions (tenant_id, role_id, permission_id, created)
        SELECT $1, $2, $3, $4
        WHERE EXISTS (
            SELECT 1 FROM roles
            WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL
        )
          AND EXISTS (
            SELECT 1 FROM permissions
            WHERE tenant_id = $1 AND id = $3 AND deleted IS NULL
        )
        ON CONFLICT (tenant_id, role_id, permission_id) DO UPDATE
        SET deleted = NULL
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(role_id)
    .bind(permission_id)
    .bind(now_iso())
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

pub async fn delete_role_permission(
    db: &Db,
    tenant_id: TenantId,
    role_id: i64,
    permission_id: i64,
) -> AppResult<bool> {
    let res = sqlx::query(
        "UPDATE role_permissions SET deleted = $1 WHERE tenant_id = $2 AND role_id = $3 AND permission_id = $4",
    )
    .bind(now_iso())
    .bind(tenant_id.get())
    .bind(role_id)
    .bind(permission_id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// `addRoleRelationship` — set `child.parent_id = parent`, rejecting self-links
/// and cycles (child already an ancestor/descendant).
pub async fn add_role_relationship(
    db: &Db,
    tenant_id: TenantId,
    parent_id: i64,
    child_id: i64,
) -> AppResult<bool> {
    if parent_id == child_id {
        return Err(AppError::Core(CoreError::BadRequest(
            "Parent and child roles cannot be the same".into(),
        )));
    }
    let all = load_all(db, tenant_id).await?;
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
    let res = sqlx::query(
        r#"
        UPDATE roles
        SET parent_id = $1
        WHERE tenant_id = $2
          AND id = $3
          AND self_user_id IS NULL
          AND EXISTS (
              SELECT 1 FROM roles parent
              WHERE parent.tenant_id = $2
                AND parent.id = $1
                AND parent.deleted IS NULL
                AND parent.self_user_id IS NULL
          )
        "#,
    )
    .bind(parent_id)
    .bind(tenant_id.get())
    .bind(child_id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn delete_role_relationship(
    db: &Db,
    tenant_id: TenantId,
    child_id: i64,
) -> AppResult<bool> {
    let res = sqlx::query(
        "UPDATE roles SET parent_id = NULL WHERE tenant_id = $1 AND id = $2 AND self_user_id IS NULL",
    )
        .bind(tenant_id.get())
        .bind(child_id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}
