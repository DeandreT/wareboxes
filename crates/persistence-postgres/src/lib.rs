//! PostgreSQL infrastructure shared by Wareboxes process composition roots.

pub mod authorization;
pub mod db;
pub mod facilities;
pub mod integration_inbox;
pub mod inventory_balances;
pub mod inventory_holds;
pub mod inventory_rollups;
pub mod locations;
pub mod outbox;
pub mod permissions;
pub mod roles;
pub mod settings;
pub mod users;

#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("database authorization context is already bound to a different scope")]
    AuthorizationContextConflict,
    #[error("invalid persistence request: {0}")]
    InvalidInput(String),
    #[error("persistence conflict: {0}")]
    Conflict(String),
    #[error("invalid persisted data: {0}")]
    InvalidData(String),
}

impl PersistenceError {
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::InvalidInput(message.into())
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    pub fn invalid_data(message: impl Into<String>) -> Self {
        Self::InvalidData(message.into())
    }
}

pub type PersistenceResult<T> = Result<T, PersistenceError>;
