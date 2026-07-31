//! Native authentication: Argon2 password hashing + opaque DB-backed session
//! tokens. The original app used Auth0; identity is intentionally decoupled
//! from credentials (`user_credentials` is a separate table) so an OAuth
//! strategy can be added later without touching the users table.

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, Method};
use cookie::Cookie;
use rand::distributions::Alphanumeric;
use rand::Rng;
use sha2::{Digest, Sha256};
use wareboxes_application::CommandContext;
use wareboxes_core::dto::{UpdateUserAccessScope, WebSessionContext};
use wareboxes_core::models::{TenantAccess, User};
use wareboxes_domain::{FacilityId, InventoryOwnerId, TenantId};

use crate::config::SecurityConfig;
use crate::db::{now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::permissions;
use crate::repo;
use crate::request_context::{current_request_id_or_new, IdempotencyKey};
use crate::state::AppState;

pub const TENANT_ID_HEADER: &str = "x-wareboxes-tenant-id";
const WEB_SESSION_COOKIE: &str = "wareboxes_web_session";
const SECURE_WEB_SESSION_COOKIE: &str = "__Host-wareboxes_web_session";

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
    sqlx::query("SELECT create_session_record($1, $2)")
        .bind(token_hash)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(token)
}

pub async fn create_web_session(
    db: &Db,
    user_id: i64,
    absolute_ttl_seconds: i32,
) -> AppResult<String> {
    let token = random_token();
    let token_hash = session_token_hash(&token);
    sqlx::query("SELECT create_web_session_record($1, $2, $3)")
        .bind(&token_hash)
        .bind(user_id)
        .bind(absolute_ttl_seconds)
        .execute(db)
        .await?;

    let result = async {
        let active_tenant = repo::tenants::default_for_session(db, &token_hash)
            .await?
            .ok_or_else(AppError::forbidden)?;
        if !select_web_session_tenant(db, &token_hash, active_tenant.tenant_id).await? {
            return Err(AppError::forbidden());
        }
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = destroy_session(db, &token).await;
    }
    result.map(|()| token)
}

pub async fn destroy_session(db: &Db, token: &str) -> AppResult<()> {
    let token_hash = session_token_hash(token);
    sqlx::query("SELECT destroy_session_record($1)")
        .bind(token_hash)
        .execute(db)
        .await?;
    Ok(())
}

async fn user_id_for_token(db: &Db, token: &str) -> AppResult<Option<i64>> {
    let token_hash = session_token_hash(token);
    let user_id: Option<i64> = sqlx::query_scalar("SELECT api_session_user_id($1)")
        .bind(token_hash)
        .fetch_one(db)
        .await
        .map_err(AppError::from)?;
    Ok(user_id)
}

async fn web_identity_for_token(
    db: &Db,
    token: &str,
    idle_ttl_seconds: i32,
) -> AppResult<Option<(i64, TenantId)>> {
    let token_hash = session_token_hash(token);
    let identity: Option<(i64, i64)> = sqlx::query_as("SELECT * FROM web_session_identity($1, $2)")
        .bind(&token_hash)
        .bind(idle_ttl_seconds)
        .fetch_optional(db)
        .await?;
    let Some((user_id, tenant_id)) = identity else {
        return Ok(None);
    };
    let tenant_id = TenantId::new(tenant_id)
        .map_err(|error| AppError::internal(format!("invalid web session tenant: {error}")))?;
    Ok(Some((user_id, tenant_id)))
}

pub async fn select_web_session_tenant(
    db: &Db,
    token_hash: &str,
    tenant_id: TenantId,
) -> AppResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT select_web_session_tenant($1, $2)")
            .bind(token_hash)
            .bind(tenant_id.get())
            .fetch_one(db)
            .await?,
    )
}

pub async fn web_session_context(
    db: &Db,
    token_hash: &str,
    user_id: i64,
    tenant_id: TenantId,
) -> AppResult<WebSessionContext> {
    let active_tenant = repo::tenants::access_for_user(db, user_id, tenant_id)
        .await?
        .ok_or_else(AppError::forbidden)?;
    let user = repo::users::get_user_by_id(db, user_id, true)
        .await?
        .ok_or_else(AppError::unauthorized)?;
    permissions::ensure_self_role(db, tenant_id, user_id, &user.email).await?;
    let user = repo::users::enrich_for_tenant(db, tenant_id, user).await?;
    let available_tenants = repo::tenants::list_for_session(db, token_hash).await?;
    let settings = repo::settings::get_user_settings(db, user_id).await?;
    Ok(WebSessionContext {
        user,
        active_tenant,
        available_tenants,
        settings,
    })
}

pub async fn web_session_context_for_token(
    db: &Db,
    security: &SecurityConfig,
    token: &str,
) -> AppResult<Option<WebSessionContext>> {
    let Some((user_id, tenant_id)) =
        web_identity_for_token(db, token, security.web_session_idle_ttl_seconds).await?
    else {
        return Ok(None);
    };
    Ok(Some(
        web_session_context(db, &session_token_hash(token), user_id, tenant_id).await?,
    ))
}

pub async fn tenant_accesses_for_session(db: &Db, token: &str) -> AppResult<Vec<TenantAccess>> {
    repo::tenants::list_for_session(db, &session_token_hash(token)).await
}

pub async fn default_tenant_for_session(db: &Db, token: &str) -> AppResult<Option<TenantAccess>> {
    repo::tenants::default_for_session(db, &session_token_hash(token)).await
}

pub fn web_session_cookie_name(security: &SecurityConfig) -> &'static str {
    if security.secure_web_session_cookie {
        SECURE_WEB_SESSION_COOKIE
    } else {
        WEB_SESSION_COOKIE
    }
}

pub fn web_session_token(headers: &HeaderMap, security: &SecurityConfig) -> Option<String> {
    let cookie_name = web_session_cookie_name(security);
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .flat_map(Cookie::split_parse)
        .filter_map(Result::ok)
        .find(|cookie| cookie.name() == cookie_name)
        .map(|cookie| cookie.value().to_owned())
}

pub fn require_same_origin(method: &Method, headers: &HeaderMap) -> AppResult<()> {
    if matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    ) {
        return Ok(());
    }
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(AppError::forbidden)?;
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(AppError::forbidden)?;
    let origin_authority = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .ok_or_else(AppError::forbidden)?;
    let origin_authority = origin_authority
        .strip_suffix('/')
        .unwrap_or(origin_authority);
    if origin_authority.eq_ignore_ascii_case(host) {
        Ok(())
    } else {
        Err(AppError::forbidden())
    }
}

#[derive(Clone, Copy, Debug)]
enum SessionKind {
    Api,
    Web { tenant_id: TenantId },
}

/// Authenticated principal backed by an active opaque session.
pub struct CurrentUser {
    pub user: User,
    pub(crate) session_token_hash: String,
    session_kind: SessionKind,
}

impl CurrentUser {
    pub fn require_web_tenant(&self) -> AppResult<TenantId> {
        match self.session_kind {
            SessionKind::Web { tenant_id } => Ok(tenant_id),
            SessionKind::Api => Err(AppError::forbidden()),
        }
    }
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let bearer_token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::to_owned);
        let (token, user_id, session_kind) = if let Some(token) = bearer_token {
            let user_id = user_id_for_token(&state.db, &token)
                .await?
                .ok_or_else(AppError::unauthorized)?;
            (token, user_id, SessionKind::Api)
        } else {
            let token = web_session_token(&parts.headers, &state.security)
                .ok_or_else(AppError::unauthorized)?;
            require_same_origin(&parts.method, &parts.headers)?;
            let (user_id, tenant_id) = web_identity_for_token(
                &state.db,
                &token,
                state.security.web_session_idle_ttl_seconds,
            )
            .await?
            .ok_or_else(AppError::unauthorized)?;
            (token, user_id, SessionKind::Web { tenant_id })
        };

        let user = repo::users::get_user_by_id(&state.db, user_id, true)
            .await?
            .ok_or_else(AppError::unauthorized)?;

        Ok(CurrentUser {
            user,
            session_token_hash: session_token_hash(&token),
            session_kind,
        })
    }
}

/// Authenticated user plus an active tenant membership. API sessions select
/// their tenant by request header; web sessions use their server-side context.
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

impl FromRequestParts<AppState> for CurrentTenant {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let current_user = CurrentUser::from_request_parts(parts, state).await?;
        let tenant_id = match current_user.session_kind {
            SessionKind::Api => {
                let tenant_id = parts
                    .headers
                    .get(TENANT_ID_HEADER)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| AppError::bad_request("tenant context header is required"))?
                    .parse::<i64>()
                    .map_err(|_| {
                        AppError::bad_request("tenant context header must be a positive ID")
                    })?;
                TenantId::new(tenant_id).map_err(|_| {
                    AppError::bad_request("tenant context header must be a positive ID")
                })?
            }
            SessionKind::Web { tenant_id } => tenant_id,
        };

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

    let tenant_id = repo::tenants::provision_default_tenant(db, user_id, email).await?;
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
