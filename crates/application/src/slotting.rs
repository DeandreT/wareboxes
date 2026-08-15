//! Advisory slotting commands and read models.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    FacilityId, InventoryBalanceId, InventoryOwnerId, LocationId, SlottingDismissalReason,
    SlottingProfileDefinition, SlottingProfileId, SlottingProfileRevision,
    SlottingRecommendationId, SlottingRecommendationReason, SlottingRecommendationStatus,
    SlottingRunId, SlottingScore, SlottingScoreEvidence, TenantId, Timestamp, UserId,
};

pub const CONFIGURE_SLOTTING_PROFILE_OPERATION: &str = "optimization.slotting.profile.configure.v1";
pub const RUN_SLOTTING_OPERATION: &str = "optimization.slotting.run.v1";
pub const ACCEPT_SLOTTING_RECOMMENDATION_OPERATION: &str =
    "optimization.slotting.recommendation.accept.v1";
pub const DISMISS_SLOTTING_RECOMMENDATION_OPERATION: &str =
    "optimization.slotting.recommendation.dismiss.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigureSlottingProfileCommand {
    pub definition: SlottingProfileDefinition,
    pub expected_revision: Option<SlottingProfileRevision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RunSlottingCommand {
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub expected_profile_revision: SlottingProfileRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AcceptSlottingRecommendationCommand {
    pub recommendation_id: SlottingRecommendationId,
    pub expected_revision: i64,
    pub task_priority: Option<u16>,
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DismissSlottingRecommendationCommand {
    pub recommendation_id: SlottingRecommendationId,
    pub expected_revision: i64,
    pub reason: SlottingDismissalReason,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlottingProfileReadModel {
    pub slotting_profile_id: SlottingProfileId,
    pub definition: SlottingProfileDefinition,
    pub revision: SlottingProfileRevision,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
    pub effective_from: Timestamp,
    pub supersedes_slotting_profile_id: Option<SlottingProfileId>,
    pub effective_to: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlottingRunReadModel {
    pub slotting_run_id: SlottingRunId,
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub slotting_profile_id: SlottingProfileId,
    pub profile_revision: SlottingProfileRevision,
    pub demand_window_started_at: Timestamp,
    pub input_snapshot_at: Timestamp,
    pub configuration_snapshot: serde_json::Value,
    pub candidate_count: i64,
    pub recommendation_count: i64,
    pub generated_by: UserId,
    pub generated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlottingRecommendationReadModel {
    pub slotting_recommendation_id: SlottingRecommendationId,
    pub slotting_run_id: SlottingRunId,
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub uom: String,
    pub source_location_id: LocationId,
    pub source_location_label: String,
    pub source_zone_code: String,
    pub destination_location_id: LocationId,
    pub destination_location_label: String,
    pub destination_zone_code: String,
    pub recommended_quantity: i64,
    pub reason: SlottingRecommendationReason,
    pub score: SlottingScore,
    pub evidence: SlottingScoreEvidence,
    pub item_storage_policy_id: i64,
    pub item_storage_policy_revision: i64,
    pub status: SlottingRecommendationStatus,
    pub revision: i64,
    pub decided_by: Option<UserId>,
    pub decided_at: Option<Timestamp>,
    pub dismissal_reason: Option<SlottingDismissalReason>,
    pub dismissal_note: Option<String>,
    pub inventory_relocation_task_id: Option<i64>,
    pub created_at: Timestamp,
}

pub type ConfigureSlottingProfileResult = SlottingProfileReadModel;
pub type RunSlottingResult = SlottingRunReadModel;
pub type AcceptSlottingRecommendationResult = SlottingRecommendationReadModel;
pub type DismissSlottingRecommendationResult = SlottingRecommendationReadModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlottingProfileCursor {
    pub after_configured_at: Timestamp,
    pub after_slotting_profile_id: SlottingProfileId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlottingProfilePageQuery {
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
    pub include_history: bool,
    pub cursor: Option<SlottingProfileCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlottingProfilePage {
    pub items: Vec<SlottingProfileReadModel>,
    pub next_cursor: Option<SlottingProfileCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlottingRecommendationCursor {
    pub after_score: i64,
    pub after_slotting_recommendation_id: SlottingRecommendationId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlottingRecommendationPageQuery {
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
    pub slotting_run_id: Option<SlottingRunId>,
    pub status: Option<SlottingRecommendationStatus>,
    pub cursor: Option<SlottingRecommendationCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlottingRecommendationPage {
    pub items: Vec<SlottingRecommendationReadModel>,
    pub next_cursor: Option<SlottingRecommendationCursor>,
}
