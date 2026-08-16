//! Two-person, time-bounded platform support access.

mod commands;
mod events;
mod query;

pub use commands::{approve, reject, request, revoke};
pub use query::{by_id, event_page, options, page};
