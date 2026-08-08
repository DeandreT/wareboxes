//! Typed RF picking claims and confirmations.

mod claim;
mod confirmation;
mod lifecycle;

pub use claim::{claim_by_id, claim_next, current};
pub use confirmation::confirm_content;
pub use lifecycle::{heartbeat, release_claim};

const CLAIM_NEXT_OPERATION: &str = "picking.claim_next.v1";
const CLAIM_BY_ID_OPERATION: &str = "picking.claim_by_id.v1";
const HEARTBEAT_OPERATION: &str = "picking.heartbeat.v1";
const RELEASE_OPERATION: &str = "picking.release.v1";
const CONFIRM_OPERATION: &str = "picking.confirm_content.v1";

const MAX_RELEASE_NOTE_LENGTH: usize = 500;
