use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use sqlx::error::ErrorKind;
use wareboxes_core::dto::ErrorResponse;
use wareboxes_core::CoreError;

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

    fn status(&self) -> StatusCode {
        match self.public_core() {
            CoreError::Unauthorized => StatusCode::UNAUTHORIZED,
            CoreError::Forbidden => StatusCode::FORBIDDEN,
            CoreError::NotFound(_) => StatusCode::NOT_FOUND,
            CoreError::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            CoreError::Conflict(_) => StatusCode::CONFLICT,
            CoreError::BadRequest(_) => StatusCode::BAD_REQUEST,
            CoreError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn public_core(&self) -> CoreError {
        match self {
            AppError::Core(c) => c.clone(),
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

    fn public_errors(&self) -> Vec<String> {
        match self.public_core() {
            CoreError::Validation(fields) => fields
                .iter()
                .map(|f| format!("{}: {}", f.field, f.message))
                .collect(),
            other => vec![other.to_string()],
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
        }
        (
            status,
            Json(ErrorResponse {
                errors: self.public_errors(),
            }),
        )
            .into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
