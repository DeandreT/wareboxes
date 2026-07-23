//! Native authentication: Argon2 password hashing + opaque DB-backed session
//! tokens. The original app used Auth0; identity is intentionally decoupled
//! from credentials (`user_credentials` is a separate table) so an OAuth
//! strategy can be added later without touching the users table.

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use rand::distributions::Alphanumeric;
use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::Row;
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_core::models::{TenantAccess, User};
use wareboxes_domain::{CommandContext, FacilityId, InventoryOwnerId, TenantId};

use crate::db::{now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::permissions;
use crate::repo;
use crate::request_context::{current_request_id_or_new, IdempotencyKey};
use crate::state::AppState;

const SESSION_TTL_DAYS: i64 = 30;
pub const TENANT_ID_HEADER: &str = "x-wareboxes-tenant-id";

pub fn hash_password(password: &str) -> AppResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(hash.to_string())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

fn random_token() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(48)
        .map(char::from)
        .collect()
}

fn session_token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub async fn create_session(db: &Db, user_id: i64) -> AppResult<String> {
    let token = random_token();
    let token_hash = session_token_hash(&token);
    let now = chrono::Utc::now();
    let expires = now + chrono::Duration::days(SESSION_TTL_DAYS);
    sqlx::query("INSERT INTO sessions (token, user_id, created, expires) VALUES ($1, $2, $3, $4)")
        .bind(token_hash)
        .bind(user_id)
        .bind(now)
        .bind(expires)
        .execute(db)
        .await?;
    Ok(token)
}

pub async fn destroy_session(db: &Db, token: &str) -> AppResult<()> {
    let token_hash = session_token_hash(token);
    sqlx::query("DELETE FROM sessions WHERE token = $1")
        .bind(token_hash)
        .execute(db)
        .await?;
    Ok(())
}

async fn user_id_for_token(db: &Db, token: &str) -> AppResult<Option<i64>> {
    let token_hash = session_token_hash(token);
    let row = sqlx::query("SELECT user_id, expires FROM sessions WHERE token = $1")
        .bind(token_hash)
        .fetch_optional(db)
        .await?;
    let Some(row) = row else { return Ok(None) };
    let expires: chrono::DateTime<chrono::Utc> = row.try_get("expires")?;
    if expires < chrono::Utc::now() {
        destroy_session(db, token).await?;
        return Ok(None);
    }
    Ok(Some(row.try_get("user_id")?))
}

/// Authenticated principal. Loading it also lazily provisions the per-user
/// "self role" exactly like the original `userHasPermission`.
pub struct CurrentUser {
    pub user: User,
}

#[async_trait::async_trait]
impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|s| s.to_string())
            .ok_or_else(AppError::unauthorized)?;

        let user_id = user_id_for_token(&state.db, &token)
            .await?
            .ok_or_else(AppError::unauthorized)?;

        let user = repo::users::get_user_by_id(&state.db, user_id, true)
            .await?
            .ok_or_else(AppError::unauthorized)?;

        Ok(CurrentUser { user })
    }
}

/// Authenticated user plus an active tenant membership selected by request header.
/// Tenant-scoped commands and queries must use this extractor rather than
/// `CurrentUser` so a missing or unauthorized tenant fails before domain work.
#[derive(Debug)]
pub struct CurrentTenant {
    pub user: User,
    pub tenant: TenantAccess,
}

impl CurrentTenant {
    pub fn command_context(&self, idempotency_key: &IdempotencyKey) -> CommandContext {
        CommandContext {
            tenant_id: self.tenant.tenant_id,
            actor_id: self.tenant.user_id,
            request_id: current_request_id_or_new(),
            idempotency_key: Some(idempotency_key.as_str().to_owned()),
        }
    }

    pub async fn require_permission(&self, db: &Db, permission: &str) -> AppResult<()> {
        if permissions::user_has_permission(db, self.tenant.tenant_id, self.user.id, permission)
            .await?
        {
            Ok(())
        } else {
            Err(AppError::forbidden())
        }
    }

    pub async fn require_any_permission(&self, db: &Db, perms: &[&str]) -> AppResult<()> {
        if permissions::user_has_any_permission(db, self.tenant.tenant_id, self.user.id, perms)
            .await?
        {
            Ok(())
        } else {
            Err(AppError::forbidden())
        }
    }

    pub fn require_facility(&self, facility_id: i64) -> AppResult<FacilityId> {
        let facility_id = FacilityId::new(facility_id)
            .map_err(|_| AppError::bad_request("facility ID must be positive"))?;
        if self.tenant.site_scope.includes(facility_id) {
            Ok(facility_id)
        } else {
            Err(AppError::forbidden())
        }
    }

    pub fn require_inventory_owner(&self, inventory_owner_id: i64) -> AppResult<InventoryOwnerId> {
        let inventory_owner_id = InventoryOwnerId::new(inventory_owner_id)
            .map_err(|_| AppError::bad_request("inventory owner ID must be positive"))?;
        if self.tenant.owner_scope.includes(inventory_owner_id) {
            Ok(inventory_owner_id)
        } else {
            Err(AppError::forbidden())
        }
    }

    pub fn require_scope_delegation(&self, scope: &UpdateUserAccessScope) -> AppResult<()> {
        if scope.all_facilities && !self.tenant.site_scope.all_facilities {
            return Err(AppError::forbidden());
        }
        for facility_id in &scope.facility_ids {
            self.require_facility(*facility_id)?;
        }

        if scope.all_inventory_owners && !self.tenant.owner_scope.all_inventory_owners {
            return Err(AppError::forbidden());
        }
        for inventory_owner_id in &scope.inventory_owner_ids {
            self.require_inventory_owner(*inventory_owner_id)?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl FromRequestParts<AppState> for CurrentTenant {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let current_user = CurrentUser::from_request_parts(parts, state).await?;
        let tenant_id = parts
            .headers
            .get(TENANT_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| AppError::bad_request("tenant context header is required"))?
            .parse::<i64>()
            .map_err(|_| AppError::bad_request("tenant context header must be a positive ID"))?;
        let tenant_id = TenantId::new(tenant_id)
            .map_err(|_| AppError::bad_request("tenant context header must be a positive ID"))?;

        let tenant = repo::tenants::access_for_user(&state.db, current_user.user.id, tenant_id)
            .await?
            .ok_or_else(AppError::forbidden)?;
        permissions::ensure_self_role(
            &state.db,
            tenant.tenant_id,
            current_user.user.id,
            &current_user.user.email,
        )
        .await?;
        let user =
            repo::users::enrich_for_tenant(&state.db, tenant.tenant_id, current_user.user).await?;

        Ok(Self { user, tenant })
    }
}

/// Create a user + credentials (used by registration and admin bootstrap).
pub async fn register_user(
    db: &Db,
    email: &str,
    password: &str,
    first_name: Option<&str>,
    last_name: Option<&str>,
) -> AppResult<User> {
    if repo::users::get_user_by_email(db, email, true)
        .await?
        .is_some()
    {
        return Err(AppError::conflict("A user with that email already exists"));
    }
    let now = now_iso();
    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (email, first_name, last_name, created) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(email)
    .bind(first_name)
    .bind(last_name)
    .bind(now)
    .fetch_one(db)
    .await?;

    let hash = hash_password(password)?;
    sqlx::query(
        "INSERT INTO user_credentials (user_id, password_hash, created) VALUES ($1, $2, $3)",
    )
    .bind(user_id)
    .bind(&hash)
    .bind(now)
    .execute(db)
    .await?;

    let tenant_id = repo::tenants::ensure_default_for_user(db, user_id, email).await?;
    permissions::ensure_self_role(db, tenant_id, user_id, email).await?;

    let user = repo::users::get_user_by_id(db, user_id, true)
        .await?
        .ok_or_else(|| AppError::internal("user vanished after creation"))?;
    repo::users::enrich_for_tenant(db, tenant_id, user).await
}

pub async fn verify_credentials(db: &Db, email: &str, password: &str) -> AppResult<Option<User>> {
    let Some(user) = repo::users::get_user_by_email(db, email, false).await? else {
        return Ok(None);
    };
    let hash: Option<String> =
        sqlx::query_scalar("SELECT password_hash FROM user_credentials WHERE user_id = $1")
            .bind(user.id)
            .fetch_optional(db)
            .await?;
    match hash {
        Some(h) if verify_password(password, &h) => Ok(Some(user)),
        _ => Ok(None),
    }
}
