//! Explainable, deterministic advisory slotting policy.

use serde::{Deserialize, Serialize};

use crate::{FacilityId, InventoryOwnerId, TenantId};

pub const MAX_SLOTTING_RECOMMENDATIONS_PER_RUN: u16 = 1_000;
pub const MAX_SLOTTING_LOOKBACK_DAYS: u16 = 365;
pub const MAX_SLOTTING_WEIGHT: u32 = 10_000;
pub const MAX_SLOTTING_NOTE_LENGTH: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlottingAdvisoryMode {
    Enabled,
    Disabled,
}

impl SlottingAdvisoryMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "enabled" => Some(Self::Enabled),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlottingRecommendationReason {
    ForwardPickDemand,
    TravelReduction,
    CapacityRebalance,
}

impl SlottingRecommendationReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ForwardPickDemand => "forward_pick_demand",
            Self::TravelReduction => "travel_reduction",
            Self::CapacityRebalance => "capacity_rebalance",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "forward_pick_demand" => Some(Self::ForwardPickDemand),
            "travel_reduction" => Some(Self::TravelReduction),
            "capacity_rebalance" => Some(Self::CapacityRebalance),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlottingRecommendationStatus {
    Pending,
    Accepted,
    Dismissed,
}

impl SlottingRecommendationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Dismissed => "dismissed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "accepted" => Some(Self::Accepted),
            "dismissed" => Some(Self::Dismissed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlottingDismissalReason {
    CapacityChanged,
    OperationalConstraint,
    ItemStrategy,
    StaleEvidence,
    DuplicateWork,
    Other,
}

impl SlottingDismissalReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CapacityChanged => "capacity_changed",
            Self::OperationalConstraint => "operational_constraint",
            Self::ItemStrategy => "item_strategy",
            Self::StaleEvidence => "stale_evidence",
            Self::DuplicateWork => "duplicate_work",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "capacity_changed" => Some(Self::CapacityChanged),
            "operational_constraint" => Some(Self::OperationalConstraint),
            "item_strategy" => Some(Self::ItemStrategy),
            "stale_evidence" => Some(Self::StaleEvidence),
            "duplicate_work" => Some(Self::DuplicateWork),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SlottingProfileRevision(i64);

impl SlottingProfileRevision {
    pub const fn new(value: i64) -> Result<Self, SlottingError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(SlottingError::InvalidRevision(value))
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

impl<'de> Deserialize<'de> for SlottingProfileRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlottingProfileDefinition {
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub mode: SlottingAdvisoryMode,
    pub demand_lookback_days: u16,
    pub demand_weight: u32,
    pub travel_weight: u32,
    pub activity_weight: u32,
    pub minimum_demand_quantity: i64,
    pub max_recommendations: u16,
    pub default_task_priority: u16,
}

impl SlottingProfileDefinition {
    pub fn validate(&self) -> Result<(), SlottingError> {
        if self.demand_lookback_days == 0 || self.demand_lookback_days > MAX_SLOTTING_LOOKBACK_DAYS
        {
            return Err(SlottingError::InvalidLookbackDays(
                self.demand_lookback_days,
            ));
        }
        for (name, weight) in [
            ("demand", self.demand_weight),
            ("travel", self.travel_weight),
            ("activity", self.activity_weight),
        ] {
            if weight == 0 || weight > MAX_SLOTTING_WEIGHT {
                return Err(SlottingError::InvalidWeight {
                    name,
                    value: weight,
                });
            }
        }
        if self.minimum_demand_quantity <= 0 {
            return Err(SlottingError::InvalidMinimumDemand(
                self.minimum_demand_quantity,
            ));
        }
        if self.max_recommendations == 0
            || self.max_recommendations > MAX_SLOTTING_RECOMMENDATIONS_PER_RUN
        {
            return Err(SlottingError::InvalidRecommendationLimit(
                self.max_recommendations,
            ));
        }
        Ok(())
    }
}

/// Frozen evidence used to explain and reproduce a recommendation score.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlottingScoreEvidence {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlottingScore {
    pub demand_component: i64,
    pub travel_component: i64,
    pub activity_component: i64,
    pub total: i64,
    pub reason: SlottingRecommendationReason,
}

/// Computes a stable integer score. Callers persist both this output and the exact input.
pub fn score_slotting_candidate(
    profile: &SlottingProfileDefinition,
    evidence: &SlottingScoreEvidence,
) -> Result<SlottingScore, SlottingError> {
    profile.validate()?;
    if evidence.outstanding_demand_quantity < 0
        || evidence.historical_pick_quantity < 0
        || evidence.historical_pick_count < 0
        || evidence.source_on_hand <= 0
        || evidence.source_movable_quantity <= 0
        || evidence.recommended_quantity <= 0
        || evidence.recommended_quantity > evidence.source_movable_quantity
        || evidence.destination_on_hand < 0
        || evidence.destination_inbound_planned_quantity < 0
        || evidence.destination_capacity.is_some_and(|capacity| {
            capacity <= 0
                || evidence
                    .destination_on_hand
                    .checked_add(evidence.destination_inbound_planned_quantity)
                    .and_then(|quantity| quantity.checked_add(evidence.recommended_quantity))
                    .is_none_or(|quantity| quantity > capacity)
        })
    {
        return Err(SlottingError::InvalidEvidence);
    }
    let demand = evidence
        .outstanding_demand_quantity
        .checked_add(evidence.historical_pick_quantity)
        .ok_or(SlottingError::ScoreOverflow)?;
    if demand < profile.minimum_demand_quantity {
        return Err(SlottingError::InsufficientDemand {
            observed: demand,
            minimum: profile.minimum_demand_quantity,
        });
    }
    let travel_saving = i64::from(
        evidence
            .source_travel_sequence
            .saturating_sub(evidence.destination_travel_sequence),
    );
    if travel_saving == 0 {
        return Err(SlottingError::NoTravelImprovement);
    }
    let demand_component = demand
        .checked_mul(i64::from(profile.demand_weight))
        .ok_or(SlottingError::ScoreOverflow)?;
    let travel_component = travel_saving
        .checked_mul(evidence.recommended_quantity)
        .and_then(|value| value.checked_mul(i64::from(profile.travel_weight)))
        .ok_or(SlottingError::ScoreOverflow)?;
    let activity_component = evidence
        .historical_pick_count
        .checked_mul(i64::from(profile.activity_weight))
        .ok_or(SlottingError::ScoreOverflow)?;
    let total = demand_component
        .checked_add(travel_component)
        .and_then(|value| value.checked_add(activity_component))
        .ok_or(SlottingError::ScoreOverflow)?;
    let reason = if evidence.outstanding_demand_quantity > 0 {
        SlottingRecommendationReason::ForwardPickDemand
    } else if evidence.destination_capacity.is_some() {
        SlottingRecommendationReason::CapacityRebalance
    } else {
        SlottingRecommendationReason::TravelReduction
    };
    Ok(SlottingScore {
        demand_component,
        travel_component,
        activity_component,
        total,
        reason,
    })
}

pub fn validate_slotting_dismissal(
    reason: SlottingDismissalReason,
    note: Option<&str>,
) -> Result<(), SlottingError> {
    if let Some(note) = note {
        if note.trim() != note
            || note.is_empty()
            || note.chars().count() > MAX_SLOTTING_NOTE_LENGTH
            || note.chars().any(char::is_control)
        {
            return Err(SlottingError::InvalidNote);
        }
    }
    if reason == SlottingDismissalReason::Other && note.is_none() {
        return Err(SlottingError::OtherReasonRequiresNote);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SlottingError {
    #[error("slotting profile revision must be positive, got {0}")]
    InvalidRevision(i64),
    #[error("demand lookback days must be between 1 and {MAX_SLOTTING_LOOKBACK_DAYS}, got {0}")]
    InvalidLookbackDays(u16),
    #[error("{name} weight must be between 1 and {MAX_SLOTTING_WEIGHT}, got {value}")]
    InvalidWeight { name: &'static str, value: u32 },
    #[error("minimum demand quantity must be positive, got {0}")]
    InvalidMinimumDemand(i64),
    #[error("recommendation limit must be between 1 and {MAX_SLOTTING_RECOMMENDATIONS_PER_RUN}, got {0}")]
    InvalidRecommendationLimit(u16),
    #[error("slotting evidence is internally inconsistent")]
    InvalidEvidence,
    #[error("observed demand {observed} is below required minimum {minimum}")]
    InsufficientDemand { observed: i64, minimum: i64 },
    #[error("candidate does not reduce travel sequence")]
    NoTravelImprovement,
    #[error("slotting score overflowed")]
    ScoreOverflow,
    #[error("slotting note must be trimmed, nonempty, printable, and at most {MAX_SLOTTING_NOTE_LENGTH} characters")]
    InvalidNote,
    #[error("other dismissal reason requires a note")]
    OtherReasonRequiresNote,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> SlottingProfileDefinition {
        SlottingProfileDefinition {
            tenant_id: TenantId::new(1).unwrap(),
            inventory_owner_id: InventoryOwnerId::new(2).unwrap(),
            facility_id: FacilityId::new(3).unwrap(),
            mode: SlottingAdvisoryMode::Enabled,
            demand_lookback_days: 30,
            demand_weight: 10,
            travel_weight: 5,
            activity_weight: 2,
            minimum_demand_quantity: 1,
            max_recommendations: 100,
            default_task_priority: 20,
        }
    }

    #[test]
    fn scoring_is_exact_explainable_and_deterministic() {
        let evidence = SlottingScoreEvidence {
            outstanding_demand_quantity: 8,
            historical_pick_quantity: 12,
            historical_pick_count: 3,
            source_travel_sequence: 100,
            destination_travel_sequence: 10,
            source_on_hand: 20,
            source_movable_quantity: 15,
            destination_on_hand: 2,
            destination_inbound_planned_quantity: 0,
            destination_capacity: Some(10),
            recommended_quantity: 8,
        };
        let score = score_slotting_candidate(&profile(), &evidence).unwrap();
        assert_eq!(score.demand_component, 200);
        assert_eq!(score.travel_component, 3_600);
        assert_eq!(score.activity_component, 6);
        assert_eq!(score.total, 3_806);
        assert_eq!(
            score.reason,
            SlottingRecommendationReason::ForwardPickDemand
        );
        assert_eq!(
            score,
            score_slotting_candidate(&profile(), &evidence).unwrap()
        );
    }

    #[test]
    fn capacity_and_non_improving_candidates_fail_closed() {
        let mut evidence = SlottingScoreEvidence {
            outstanding_demand_quantity: 1,
            historical_pick_quantity: 0,
            historical_pick_count: 0,
            source_travel_sequence: 10,
            destination_travel_sequence: 10,
            source_on_hand: 5,
            source_movable_quantity: 5,
            destination_on_hand: 9,
            destination_inbound_planned_quantity: 0,
            destination_capacity: Some(10),
            recommended_quantity: 2,
        };
        assert_eq!(
            score_slotting_candidate(&profile(), &evidence),
            Err(SlottingError::InvalidEvidence)
        );
        evidence.destination_on_hand = 0;
        evidence.destination_inbound_planned_quantity = 9;
        assert_eq!(
            score_slotting_candidate(&profile(), &evidence),
            Err(SlottingError::InvalidEvidence)
        );
        evidence.destination_inbound_planned_quantity = 0;
        evidence.destination_on_hand = 0;
        evidence.destination_capacity = None;
        assert_eq!(
            score_slotting_candidate(&profile(), &evidence),
            Err(SlottingError::NoTravelImprovement)
        );
    }

    #[test]
    fn typed_other_dismissal_requires_bounded_evidence() {
        assert_eq!(
            validate_slotting_dismissal(SlottingDismissalReason::Other, None),
            Err(SlottingError::OtherReasonRequiresNote)
        );
        assert!(validate_slotting_dismissal(
            SlottingDismissalReason::OperationalConstraint,
            Some("Aisle closed")
        )
        .is_ok());
    }
}
