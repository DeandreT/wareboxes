use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use sqlx::error::ErrorKind;
use wareboxes_api_contract::web::{ErrorCode, ErrorResponse, FieldError};
use wareboxes_application::ApplicationError;

use crate::request_context::{current_request_id_or_new, REQUEST_ID_HEADER};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Application(#[from] ApplicationError),
    #[error("revision conflict: {0}")]
    RevisionConflict(String),
    #[error("invalid state transition: {0}")]
    InvalidStateTransition(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<wareboxes_persistence_postgres::PersistenceError> for AppError {
    fn from(error: wareboxes_persistence_postgres::PersistenceError) -> Self {
        match error {
            wareboxes_persistence_postgres::PersistenceError::Database(error) => Self::Db(error),
            wareboxes_persistence_postgres::PersistenceError::AuthorizationContextConflict => {
                Self::forbidden()
            }
            wareboxes_persistence_postgres::PersistenceError::InvalidInput(message) => {
                Self::bad_request(message)
            }
            wareboxes_persistence_postgres::PersistenceError::Conflict(message) => {
                Self::conflict(message)
            }
            wareboxes_persistence_postgres::PersistenceError::InvalidData(message) => {
                Self::internal(message)
            }
        }
    }
}

impl From<wareboxes_persistence_postgres::idempotency::CommandIdempotencyError> for AppError {
    fn from(error: wareboxes_persistence_postgres::idempotency::CommandIdempotencyError) -> Self {
        match error {
            wareboxes_persistence_postgres::idempotency::CommandIdempotencyError::Application(
                error,
            ) => Self::Application(error),
            wareboxes_persistence_postgres::idempotency::CommandIdempotencyError::Persistence(
                error,
            ) => error.into(),
        }
    }
}

impl AppError {
    pub fn unauthorized() -> Self {
        Self::Application(ApplicationError::Unauthorized)
    }

    pub fn forbidden() -> Self {
        Self::Application(ApplicationError::Forbidden)
    }

    pub fn not_found(resource: impl Into<String>) -> Self {
        Self::Application(ApplicationError::NotFound(resource.into()))
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::Application(ApplicationError::InvalidRequest(message.into()))
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Application(ApplicationError::Conflict(message.into()))
    }

    pub fn revision_conflict(message: impl Into<String>) -> Self {
        Self::RevisionConflict(message.into())
    }

    pub fn invalid_state_transition(message: impl Into<String>) -> Self {
        Self::InvalidStateTransition(message.into())
    }

    pub fn idempotency_key_reused() -> Self {
        Self::Application(ApplicationError::IdempotencyKeyReused)
    }

    pub fn idempotency_key_required() -> Self {
        Self::Application(ApplicationError::IdempotencyKeyRequired)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Application(ApplicationError::Internal(message.into()))
    }

    pub(crate) fn public_application_error(&self) -> ApplicationError {
        match self {
            AppError::Application(ApplicationError::Internal(_)) => {
                ApplicationError::Internal("internal error".into())
            }
            AppError::Application(error) => error.clone(),
            AppError::RevisionConflict(message) | AppError::InvalidStateTransition(message) => {
                ApplicationError::Conflict(message.clone())
            }
            AppError::Db(sqlx::Error::RowNotFound) => {
                ApplicationError::NotFound("resource".to_string())
            }
            AppError::Db(sqlx::Error::Database(e)) if e.code().as_deref() == Some("55000") => {
                ApplicationError::Conflict("operation violates current resource state".into())
            }
            AppError::Db(sqlx::Error::Database(e)) => match e.kind() {
                ErrorKind::UniqueViolation => {
                    ApplicationError::Conflict("unique constraint violated".into())
                }
                ErrorKind::ForeignKeyViolation => {
                    ApplicationError::InvalidRequest("referenced resource does not exist".into())
                }
                ErrorKind::NotNullViolation => {
                    ApplicationError::InvalidRequest("required value is missing".into())
                }
                ErrorKind::CheckViolation => {
                    ApplicationError::InvalidRequest("constraint check failed".into())
                }
                _ => ApplicationError::Internal("database error".into()),
            },
            AppError::Db(_) => ApplicationError::Internal("database error".into()),
            AppError::Other(_) => ApplicationError::Internal("internal error".into()),
        }
    }

    fn public_contract(&self) -> (StatusCode, ErrorCode, String, Vec<FieldError>) {
        match self.public_application_error() {
            ApplicationError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                ErrorCode::Unauthorized,
                "unauthorized".into(),
                Vec::new(),
            ),
            ApplicationError::Forbidden => (
                StatusCode::FORBIDDEN,
                ErrorCode::Forbidden,
                "forbidden".into(),
                Vec::new(),
            ),
            ApplicationError::NotFound(resource) => (
                StatusCode::NOT_FOUND,
                ErrorCode::NotFound,
                format!("not found: {resource}"),
                Vec::new(),
            ),
            ApplicationError::Validation(details) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::ValidationFailed,
                "validation failed".into(),
                details
                    .into_iter()
                    .map(|detail| FieldError {
                        field: detail.field,
                        message: detail.message,
                    })
                    .collect(),
            ),
            ApplicationError::Conflict(message) => (
                StatusCode::CONFLICT,
                ErrorCode::Conflict,
                message,
                Vec::new(),
            ),
            ApplicationError::IdempotencyKeyReused => (
                StatusCode::CONFLICT,
                ErrorCode::IdempotencyKeyReused,
                "idempotency key was already used with a different request".into(),
                Vec::new(),
            ),
            ApplicationError::IdempotencyKeyRequired => (
                StatusCode::BAD_REQUEST,
                ErrorCode::IdempotencyKeyRequired,
                "idempotency key is required".into(),
                Vec::new(),
            ),
            ApplicationError::InvalidRequest(message) => (
                StatusCode::BAD_REQUEST,
                ErrorCode::InvalidRequest,
                message,
                Vec::new(),
            ),
            ApplicationError::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::InternalError,
                "internal error".into(),
                Vec::new(),
            ),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let request_id = current_request_id_or_new();
        let (status, code, message, details) = self.public_contract();
        if status.is_server_error() {
            tracing::error!(%request_id, error = %self, "request failed");
        }
        let mut response = (
            status,
            Json(ErrorResponse {
                code,
                message,
                request_id: request_id.clone(),
                details,
            }),
        )
            .into_response();
        if let Ok(header_value) = HeaderValue::from_str(&request_id) {
            response
                .headers_mut()
                .insert(HeaderName::from_static(REQUEST_ID_HEADER), header_value);
        }
        response
    }
}

pub type AppResult<T> = Result<T, AppError>;
