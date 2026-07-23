use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use sqlx::error::ErrorKind;
use wareboxes_core::dto::{ErrorCode, ErrorResponse};
use wareboxes_core::{CoreError, FieldError};

use crate::request_context::{current_request_id_or_new, REQUEST_ID_HEADER};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl AppError {
    pub fn unauthorized() -> Self {
        Self::Core(CoreError::Unauthorized)
    }

    pub fn forbidden() -> Self {
        Self::Core(CoreError::Forbidden)
    }

    pub fn not_found(resource: impl Into<String>) -> Self {
        Self::Core(CoreError::NotFound(resource.into()))
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::Core(CoreError::BadRequest(message.into()))
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Core(CoreError::Conflict(message.into()))
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Core(CoreError::Internal(message.into()))
    }

    fn public_core(&self) -> CoreError {
        match self {
            AppError::Core(CoreError::Internal(_)) => CoreError::Internal("internal error".into()),
            AppError::Core(core) => core.clone(),
            AppError::Db(sqlx::Error::RowNotFound) => CoreError::NotFound("resource".to_string()),
            AppError::Db(sqlx::Error::Database(e)) => match e.kind() {
                ErrorKind::UniqueViolation => {
                    CoreError::Conflict("unique constraint violated".into())
                }
                ErrorKind::ForeignKeyViolation => {
                    CoreError::BadRequest("referenced resource does not exist".into())
                }
                ErrorKind::NotNullViolation => {
                    CoreError::BadRequest("required value is missing".into())
                }
                ErrorKind::CheckViolation => {
                    CoreError::BadRequest("constraint check failed".into())
                }
                _ => CoreError::Internal("database error".into()),
            },
            AppError::Db(_) => CoreError::Internal("database error".into()),
            AppError::Other(_) => CoreError::Internal("internal error".into()),
        }
    }

    fn public_contract(&self) -> (StatusCode, ErrorCode, String, Vec<FieldError>) {
        match self.public_core() {
            CoreError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                ErrorCode::Unauthorized,
                "unauthorized".into(),
                Vec::new(),
            ),
            CoreError::Forbidden => (
                StatusCode::FORBIDDEN,
                ErrorCode::Forbidden,
                "forbidden".into(),
                Vec::new(),
            ),
            CoreError::NotFound(resource) => (
                StatusCode::NOT_FOUND,
                ErrorCode::NotFound,
                format!("not found: {resource}"),
                Vec::new(),
            ),
            CoreError::Validation(details) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::ValidationFailed,
                "validation failed".into(),
                details,
            ),
            CoreError::Conflict(message) => (
                StatusCode::CONFLICT,
                ErrorCode::Conflict,
                message,
                Vec::new(),
            ),
            CoreError::BadRequest(message) => (
                StatusCode::BAD_REQUEST,
                ErrorCode::InvalidRequest,
                message,
                Vec::new(),
            ),
            CoreError::Internal(_) => (
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
