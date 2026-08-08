//! Transport- and persistence-independent application workflow contracts.

pub mod authorization;
mod context;
mod error;
pub mod facility_shipping_origin;
pub mod idempotency;
pub mod identity;
pub mod integration;
pub mod inventory;
pub mod order_allocation;
pub mod order_cancellation;
pub mod order_release;
pub mod outbox;
pub mod packing;
pub mod picking;
pub mod replenishment;
pub mod shipping;
pub mod topology;

pub use context::CommandContext;
pub use error::{ApplicationError, ApplicationResult, ValidationIssue};
