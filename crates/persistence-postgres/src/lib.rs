//! PostgreSQL infrastructure shared by Wareboxes process composition roots.

pub mod db;

#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("database authorization context is already bound to a different scope")]
    AuthorizationContextConflict,
}

pub type PersistenceResult<T> = Result<T, PersistenceError>;
