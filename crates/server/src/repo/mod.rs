//! Data-access layer. Each module ports the corresponding `app/utils/*.ts`
//! file. Nested aggregates (roles, permissions, order items, warehouses) are
//! assembled in Rust rather than via PostgreSQL `json_agg`.

pub mod accounts;
pub mod address;
pub mod audits;
pub mod employees;
pub mod inventory;
pub mod items;
pub mod license_plates;
pub mod loads;
pub mod locations;
pub mod orders;
pub mod permissions;
pub mod roles;
pub mod settings;
pub mod tasks;
pub mod users;
pub mod warehouses;
