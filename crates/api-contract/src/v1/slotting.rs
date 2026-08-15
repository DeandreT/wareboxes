use serde::{Deserialize, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlottingAdvisoryMode {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlottingRecommendationReason {
    ForwardPickDemand,
    TravelReduction,
    CapacityRebalance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlottingRecommendationStatus {
    Pending,
    Accepted,
    Dismissed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlottingDismissalReason {
    CapacityChanged,
    OperationalConstraint,
    ItemStrategy,
    StaleEvidence,
    DuplicateWork,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureSlottingProfileRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub mode: SlottingAdvisoryMode,
    pub demand_lookback_days: u16,
    pub demand_weight: u32,
    pub travel_weight: u32,
    pub activity_weight: u32,
    pub minimum_demand_quantity: i64,
    pub max_recommendations: u16,
    pub default_task_priority: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlottingProfileResponse {
    pub slotting_profile_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub mode: SlottingAdvisoryMode,
    pub demand_lookback_days: u16,
    pub demand_weight: u32,
    pub travel_weight: u32,
    pub activity_weight: u32,
    pub minimum_demand_quantity: i64,
    pub max_recommendations: u16,
    pub default_task_priority: u16,
    pub revision: Revision,
    pub configured_by: i64,
    pub configured_at: String,
    pub effective_from: String,
    pub supersedes_slotting_profile_id: Option<i64>,
    pub effective_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SlottingProfilePageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default)]
    pub include_history: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

pub type SlottingProfilePage = CursorPage<SlottingProfileResponse>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSlottingRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub expected_profile_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlottingRunResponse {
    pub slotting_run_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub slotting_profile_id: i64,
    pub profile_revision: Revision,
    pub demand_window_started_at: String,
    pub input_snapshot_at: String,
    pub configuration_snapshot: serde_json::Value,
    pub candidate_count: i64,
    pub recommendation_count: i64,
    pub generated_by: i64,
    pub generated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlottingScoreEvidenceResponse {
    pub outstanding_demand_quantity: i64,
    pub historical_pick_quantity: i64,
    pub historical_pick_count: i64,
    pub source_travel_sequence: u32,
    pub destination_travel_sequence: u32,
    pub source_on_hand: i64,
    pub source_movable_quantity: i64,
    pub destination_on_hand: i64,
    pub destination_inbound_planned_quantity: i64,
    pub destination_capacity: Option<i64>,
    pub recommended_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlottingScoreResponse {
    pub demand_component: i64,
    pub travel_component: i64,
    pub activity_component: i64,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlottingRecommendationResponse {
    pub slotting_recommendation_id: i64,
    pub slotting_run_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub source_inventory_balance_id: i64,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub uom: String,
    pub source_location_id: i64,
    pub source_location_label: String,
    pub source_zone_code: String,
    pub destination_location_id: i64,
    pub destination_location_label: String,
    pub destination_zone_code: String,
    pub recommended_quantity: i64,
    pub reason: SlottingRecommendationReason,
    pub score: SlottingScoreResponse,
    pub evidence: SlottingScoreEvidenceResponse,
    pub item_storage_policy_id: i64,
    pub item_storage_policy_revision: i64,
    pub status: SlottingRecommendationStatus,
    pub revision: Revision,
    pub decided_by: Option<i64>,
    pub decided_at: Option<String>,
    pub dismissal_reason: Option<SlottingDismissalReason>,
    pub dismissal_note: Option<String>,
    pub inventory_relocation_task_id: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SlottingRecommendationPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slotting_run_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SlottingRecommendationStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

pub type SlottingRecommendationPage = CursorPage<SlottingRecommendationResponse>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptSlottingRecommendationRequest {
    pub expected_revision: Revision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_priority: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DismissSlottingRecommendationRequest {
    pub expected_revision: Revision,
    pub reason: SlottingDismissalReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slotting_commands_are_strict_and_typed() {
        assert!(
            serde_json::from_value::<RunSlottingRequest>(serde_json::json!({
                "inventory_owner_id": 1,
                "facility_id": 2,
                "expected_profile_revision": 1,
                "execute": true
            }))
            .is_err()
        );
        let dismissal: DismissSlottingRecommendationRequest = serde_json::from_value(
            serde_json::json!({"expected_revision":1,"reason":"capacity_changed"}),
        )
        .unwrap();
        assert_eq!(dismissal.reason, SlottingDismissalReason::CapacityChanged);
    }
}
