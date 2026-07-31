//! Wareboxes HTTP API and authentication boundary.
//!
//! Deployable processes compose this crate rather than owning reusable API
//! behavior themselves.

pub mod auth;
pub mod config;
pub use wareboxes_persistence_postgres::db;
pub mod error;
mod identity;
pub mod permissions;
pub mod repo;
pub mod request_context;
pub mod routes;
pub mod state;
#[cfg(feature = "ssr")]
pub mod web_app;
