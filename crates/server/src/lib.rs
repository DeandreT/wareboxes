//! Wareboxes WMS server library. Exposed so integration tests can drive the
//! repository/auth/permission layer directly; `main.rs` is a thin binary over
//! this crate.

pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod permissions;
pub mod repo;
pub mod request_context;
pub mod routes;
pub mod state;
