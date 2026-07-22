//! Data-access layer. Each module ports the corresponding `app/utils/*.ts`
//! file. Nested aggregates (roles, permissions, order items, facilities) are
//! assembled in Rust rather than via PostgreSQL `json_agg`.

pub mod address;
pub mod audits;
pub mod employees;
pub mod facilities;
pub mod inventory;
pub mod inventory_owners;
pub mod items;
pub mod license_plates;
pub mod loads;
pub mod locations;
pub mod orders;
pub mod permissions;
pub mod roles;
pub mod settings;
pub mod tasks;
pub mod tenants;
pub mod users;
