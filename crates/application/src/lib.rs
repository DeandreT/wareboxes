//! Transport- and persistence-independent application workflow contracts.

pub mod authorization;
mod context;
pub mod identity;
pub mod integration;
pub mod outbox;
pub mod topology;

pub use context::CommandContext;
