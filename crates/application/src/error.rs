//! Transport- and persistence-independent application error semantics.

#[derive(Debug, Clone, thiserror::Error)]
pub enum ApplicationError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("validation failed")]
    Validation(Vec<ValidationIssue>),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("idempotency key was already used with a different request")]
    IdempotencyKeyReused,
    #[error("idempotency key is required")]
    IdempotencyKeyRequired,
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("internal error: {0}")]
    Internal(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub field: String,
    pub message: String,
}

pub type ApplicationResult<T> = Result<T, ApplicationError>;
