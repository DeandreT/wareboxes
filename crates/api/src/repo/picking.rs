//! Typed RF picking claims and confirmations.

mod claim;
mod cluster;
mod confirmation;
mod lifecycle;
mod policy;
mod readiness;
mod reversal;
mod short_shipment;
mod shortage;
mod shortage_read_model;
mod shortage_reallocation;
mod zone;

pub use claim::{claim_by_id, claim_next, current};
pub use cluster::{
    cancel as cancel_cluster, change_cart_status, claim_next as claim_next_cluster, create_cart,
    plan as plan_cluster, workspace as cluster_workspace,
};
pub use confirmation::confirm_content;
pub use lifecycle::{heartbeat, release_claim};
pub(in crate::repo) use readiness::order_pick_readiness_tx;
pub use reversal::{list_confirmation_history, reverse_confirmation};
pub use short_shipment::accept_short_shipment;
pub use shortage::report_shortage;
pub use shortage_read_model::{get_shortage, list_shortages};
pub use shortage_reallocation::reallocate_shortage;
pub use zone::{claim_next as claim_next_zone, workspace as zone_workspace};

const CLAIM_NEXT_OPERATION: &str = "picking.claim_next.v1";
const CLAIM_BY_ID_OPERATION: &str = "picking.claim_by_id.v1";
const HEARTBEAT_OPERATION: &str = "picking.heartbeat.v1";
const RELEASE_OPERATION: &str = "picking.release.v1";
const CONFIRM_OPERATION: &str = "picking.confirm_content.v1";

const MAX_RELEASE_NOTE_LENGTH: usize = 500;
