//! Transport- and persistence-independent application workflow contracts.

pub mod authorization;
mod context;
mod error;
pub mod idempotency;
pub mod identity;
pub mod integration;
pub mod inventory;
pub mod outbox;
pub mod topology;

pub use context::CommandContext;
pub use error::{ApplicationError, ApplicationResult, ValidationIssue};
