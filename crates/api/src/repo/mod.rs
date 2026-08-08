//! Data-access layer. Each module ports the corresponding `app/utils/*.ts`
//! file. Nested aggregates (roles, permissions, order items, facilities) are
//! assembled in Rust rather than via PostgreSQL `json_agg`.

pub mod access;
pub mod address;
pub mod audits;
pub mod employees;
pub mod expected_receiving;
pub mod inbound_receipt;
pub mod inventory;
mod inventory_allocation;
mod inventory_hold;
pub(crate) mod inventory_journal;
mod inventory_locking;
pub mod inventory_owners;
mod inventory_status_change;
pub mod items;
pub mod license_plates;
pub mod loads;
pub mod orders;
pub mod tasks;
pub mod tenants;
