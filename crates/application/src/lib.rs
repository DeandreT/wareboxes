//! Transport- and persistence-independent application workflow contracts.

mod context;
pub mod outbox;

pub use context::CommandContext;
