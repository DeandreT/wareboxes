//! Data-access layer. Each module ports the corresponding `app/utils/*.ts`
//! file. Nested aggregates (roles, permissions, order items, facilities) are
//! assembled in Rust rather than via PostgreSQL `json_agg`.

pub mod access;
pub mod address;
pub mod audits;
pub mod backorder;
pub mod employees;
pub mod expected_receiving;
pub mod facility_shipping_origin;
pub mod inbound_inspection;
pub mod inbound_receipt;
pub mod integration_monitor;
pub mod integration_order_intake;
pub mod inventory;
mod inventory_allocation;
mod inventory_hold;
pub mod inventory_integrity;
pub(crate) mod inventory_journal;
mod inventory_locking;
pub mod inventory_owners;
pub mod inventory_recall;
mod inventory_status_change;
pub mod item_storage_policy;
pub mod item_substitution;
pub mod item_traceability_policy;
pub mod items;
pub mod license_plates;
pub mod loads;
pub mod order_allocation;
pub mod order_amendment;
pub mod order_cancellation;
pub mod order_creation;
pub mod order_line_amendment;
pub mod order_release;
pub mod orders;
pub mod outbound_load;
pub mod outbound_qa;
pub mod packing;
pub mod pick_wave;
pub mod picking;
pub mod replenishment;
pub mod shipping;
pub mod storage_zone;
pub mod tasks;
pub mod tenants;
pub mod unexpected_receipt;
