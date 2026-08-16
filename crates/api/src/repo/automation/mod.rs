//! Facility automation registry, cloud command queue, and edge evidence.

mod commands;
mod edge;
mod events;
pub(crate) mod mapping;
mod query;

pub use commands::{change_control, enqueue_command, register_device, resolve_command};
pub use edge::{
    acknowledge_command, assigned_devices, pull_commands, record_heartbeat, report_command,
};
pub use query::workspace;

pub const EDGE_PERMISSION: &str = "automation_edge";
pub const SUPERVISOR_PERMISSION: &str = "wms_supervisor";

const DELIVERY_LEASE_SECONDS: i64 = 30;
const HEALTH_FRESH_SECONDS: i64 = 120;
const MAX_WORKSPACE_ROWS: i64 = 500;
