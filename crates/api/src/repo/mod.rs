//! Data-access layer. Each module ports the corresponding `app/utils/*.ts`
//! file. Nested aggregates (roles, permissions, order items, facilities) are
//! assembled in Rust rather than via PostgreSQL `json_agg`.

pub mod access;
pub mod address;
pub mod audits;
pub mod employees;
pub mod expected_receiving;
pub mod facility_shipping_origin;
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
pub mod order_allocation;
pub mod order_cancellation;
pub mod order_creation;
pub mod order_release;
pub mod orders;
pub mod packing;
pub mod picking;
pub mod replenishment;
pub mod shipping;
pub mod tasks;
pub mod tenants;
