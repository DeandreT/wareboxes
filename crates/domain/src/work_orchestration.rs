//! Deterministic, explainable scoring for advisory orchestration of canonical work.

use serde::{Deserialize, Serialize};

use crate::{FacilityId, InventoryOwnerId, LocationId, TenantId};

pub const MAX_ORCHESTRATION_WEIGHT: u32 = 10_000;
pub const MAX_ORCHESTRATION_CANDIDATES: u16 = 500;
pub const MAX_DUE_HORIZON_MINUTES: u32 = 7 * 24 * 60;
pub const MAX_SIGNAL_TTL_SECONDS: u32 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkOrchestrationMode {
    Enabled,
    Disabled,
}

impl WorkOrchestrationMode {
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
pub enum OrchestrationPlanMode {
    Optimized,
    ManualFifo,
}

impl OrchestrationPlanMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Optimized => "optimized",
            Self::ManualFifo => "manual_fifo",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "optimized" => Some(Self::Optimized),
            "manual_fifo" => Some(Self::ManualFifo),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationWorkKind {
    CycleCountItemLocation,
    CycleCountLocation,
    Putaway,
    LicensePlatePutaway,
    InventoryRelocation,
    Replenishment,
    CrossDock,
}

impl OrchestrationWorkKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CycleCountItemLocation => "cycle_count_item_location",
            Self::CycleCountLocation => "cycle_count_location",
            Self::Putaway => "putaway",
            Self::LicensePlatePutaway => "license_plate_putaway",
            Self::InventoryRelocation => "inventory_relocation",
            Self::Replenishment => "replenishment",
            Self::CrossDock => "cross_dock",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "cycle_count_item_location" => Some(Self::CycleCountItemLocation),
            "cycle_count_location" => Some(Self::CycleCountLocation),
            "putaway" => Some(Self::Putaway),
            "license_plate_putaway" => Some(Self::LicensePlatePutaway),
            "inventory_relocation" => Some(Self::InventoryRelocation),
            "replenishment" => Some(Self::Replenishment),
            "cross_dock" => Some(Self::CrossDock),
            _ => None,
        }
    }

    pub const fn resource_kind(self) -> WorkResourceKind {
        match self {
            Self::CycleCountItemLocation | Self::CycleCountLocation => {
                WorkResourceKind::InventoryControl
            }
            Self::Putaway
            | Self::LicensePlatePutaway
            | Self::InventoryRelocation
            | Self::Replenishment
            | Self::CrossDock => WorkResourceKind::MaterialHandling,
        }
    }

    /// Compatible work alternates a delivery movement with a source-facing movement.
    pub const fn interleaves_with(self, previous: Self) -> bool {
        matches!(
            (previous, self),
            (
                Self::Putaway | Self::LicensePlatePutaway | Self::CrossDock,
                Self::InventoryRelocation | Self::Replenishment
            ) | (
                Self::InventoryRelocation | Self::Replenishment,
                Self::Putaway | Self::LicensePlatePutaway | Self::CrossDock
            )
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkResourceKind {
    GeneralLabor,
    InventoryControl,
    MaterialHandling,
    DockDoor,
    PackStation,
    Automation,
}

impl WorkResourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GeneralLabor => "general_labor",
            Self::InventoryControl => "inventory_control",
            Self::MaterialHandling => "material_handling",
            Self::DockDoor => "dock_door",
            Self::PackStation => "pack_station",
            Self::Automation => "automation",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "general_labor" => Some(Self::GeneralLabor),
            "inventory_control" => Some(Self::InventoryControl),
            "material_handling" => Some(Self::MaterialHandling),
            "dock_door" => Some(Self::DockDoor),
            "pack_station" => Some(Self::PackStation),
            "automation" => Some(Self::Automation),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct WorkOrchestrationPolicyRevision(i64);

impl WorkOrchestrationPolicyRevision {
    pub const fn new(value: i64) -> Result<Self, WorkOrchestrationError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(WorkOrchestrationError::InvalidRevision(value))
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

impl<'de> Deserialize<'de> for WorkOrchestrationPolicyRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkOrchestrationPolicyDefinition {
    pub tenant_id: TenantId,
    pub facility_id: FacilityId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub mode: WorkOrchestrationMode,
    pub priority_weight: u32,
    pub due_urgency_weight: u32,
    pub proximity_weight: u32,
    pub interleaving_weight: u32,
    pub congestion_penalty_weight: u32,
    pub bottleneck_penalty_weight: u32,
    pub due_horizon_minutes: u32,
    pub max_candidates: u16,
}

impl WorkOrchestrationPolicyDefinition {
    pub fn validate(&self) -> Result<(), WorkOrchestrationError> {
        let weights = [
            self.priority_weight,
            self.due_urgency_weight,
            self.proximity_weight,
            self.interleaving_weight,
            self.congestion_penalty_weight,
            self.bottleneck_penalty_weight,
        ];
        if let Some(value) = weights
            .iter()
            .copied()
            .find(|value| *value > MAX_ORCHESTRATION_WEIGHT)
        {
            return Err(WorkOrchestrationError::InvalidWeight(value));
        }
        if self.mode == WorkOrchestrationMode::Enabled && weights.iter().all(|value| *value == 0) {
            return Err(WorkOrchestrationError::NoEnabledWeight);
        }
        if self.due_horizon_minutes == 0 || self.due_horizon_minutes > MAX_DUE_HORIZON_MINUTES {
            return Err(WorkOrchestrationError::InvalidDueHorizon(
                self.due_horizon_minutes,
            ));
        }
        if self.max_candidates == 0 || self.max_candidates > MAX_ORCHESTRATION_CANDIDATES {
            return Err(WorkOrchestrationError::InvalidCandidateLimit(
                self.max_candidates,
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestrationScoreEvidence {
    pub work_kind: OrchestrationWorkKind,
    pub task_priority: i64,
    pub due_at: Option<crate::Timestamp>,
    pub overdue_seconds: i64,
    pub due_urgency_basis_points: u16,
    pub current_location_id: LocationId,
    pub source_location_id: LocationId,
    pub destination_location_id: Option<LocationId>,
    pub current_travel_sequence: i64,
    pub source_travel_sequence: i64,
    pub destination_travel_sequence: Option<i64>,
    pub travel_distance: i64,
    pub proximity_basis_points: u16,
    pub previous_work_kind: Option<OrchestrationWorkKind>,
    pub interleaving_compatible: bool,
    pub source_zone_id: Option<i64>,
    pub source_zone_code: Option<String>,
    pub congestion_basis_points: u16,
    pub congestion_queue_depth: i64,
    pub resource_kind: WorkResourceKind,
    pub resource_available_units: i64,
    pub resource_demand_units: i64,
    pub resource_utilization_basis_points: u16,
}

impl OrchestrationScoreEvidence {
    pub fn validate(&self) -> Result<(), WorkOrchestrationError> {
        if self.task_priority < 0
            || self.overdue_seconds < 0
            || self.travel_distance < 0
            || self.congestion_queue_depth < 0
            || self.resource_available_units < 0
            || self.resource_demand_units < 0
            || self.due_urgency_basis_points > 10_000
            || self.proximity_basis_points > 10_000
            || self.congestion_basis_points > 10_000
            || self.resource_utilization_basis_points > 10_000
        {
            return Err(WorkOrchestrationError::InvalidEvidence);
        }
        if self.interleaving_compatible
            != self
                .previous_work_kind
                .is_some_and(|previous| self.work_kind.interleaves_with(previous))
        {
            return Err(WorkOrchestrationError::InvalidEvidence);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestrationScore {
    pub priority_component: i64,
    pub due_urgency_component: i64,
    pub proximity_component: i64,
    pub interleaving_component: i64,
    pub congestion_penalty: i64,
    pub bottleneck_penalty: i64,
    pub total: i64,
}

fn weighted(basis_points: u16, weight: u32) -> Result<i64, WorkOrchestrationError> {
    i64::from(basis_points)
        .checked_mul(i64::from(weight))
        .ok_or(WorkOrchestrationError::ScoreOverflow)
}

pub fn score_orchestration_candidate(
    policy: &WorkOrchestrationPolicyDefinition,
    evidence: &OrchestrationScoreEvidence,
) -> Result<OrchestrationScore, WorkOrchestrationError> {
    policy.validate()?;
    evidence.validate()?;
    let priority_basis_points = u16::try_from(evidence.task_priority.min(1_000) * 10)
        .map_err(|_| WorkOrchestrationError::InvalidEvidence)?;
    let priority_component = weighted(priority_basis_points, policy.priority_weight)?;
    let due_urgency_component =
        weighted(evidence.due_urgency_basis_points, policy.due_urgency_weight)?;
    let proximity_component = weighted(evidence.proximity_basis_points, policy.proximity_weight)?;
    let interleaving_component = weighted(
        if evidence.interleaving_compatible {
            10_000
        } else {
            0
        },
        policy.interleaving_weight,
    )?;
    let congestion_penalty = weighted(
        evidence.congestion_basis_points,
        policy.congestion_penalty_weight,
    )?;
    let bottleneck_penalty = weighted(
        evidence.resource_utilization_basis_points,
        policy.bottleneck_penalty_weight,
    )?;
    let total = priority_component
        .checked_add(due_urgency_component)
        .and_then(|value| value.checked_add(proximity_component))
        .and_then(|value| value.checked_add(interleaving_component))
        .and_then(|value| value.checked_sub(congestion_penalty))
        .and_then(|value| value.checked_sub(bottleneck_penalty))
        .ok_or(WorkOrchestrationError::ScoreOverflow)?;
    Ok(OrchestrationScore {
        priority_component,
        due_urgency_component,
        proximity_component,
        interleaving_component,
        congestion_penalty,
        bottleneck_penalty,
        total,
    })
}

pub fn resource_utilization_basis_points(available_units: i64, demand_units: i64) -> u16 {
    if demand_units <= 0 {
        return 0;
    }
    if available_units <= 0 {
        return 10_000;
    }
    let basis_points = i128::from(demand_units)
        .saturating_mul(10_000)
        .checked_div(i128::from(available_units))
        .unwrap_or(10_000)
        .clamp(0, 10_000);
    u16::try_from(basis_points).unwrap_or(10_000)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoneCongestionSignal {
    pub congestion_basis_points: u16,
    pub queue_depth: i64,
    pub ttl_seconds: u32,
}

impl ZoneCongestionSignal {
    pub fn validate(self) -> Result<(), WorkOrchestrationError> {
        if self.congestion_basis_points > 10_000
            || self.queue_depth < 0
            || self.ttl_seconds == 0
            || self.ttl_seconds > MAX_SIGNAL_TTL_SECONDS
        {
            Err(WorkOrchestrationError::InvalidSignal)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceCapacitySignal {
    pub available_units: i64,
    pub demand_units: i64,
    pub ttl_seconds: u32,
}

impl ResourceCapacitySignal {
    pub fn validate(self) -> Result<(), WorkOrchestrationError> {
        if self.available_units < 0
            || self.demand_units < 0
            || self.ttl_seconds == 0
            || self.ttl_seconds > MAX_SIGNAL_TTL_SECONDS
        {
            Err(WorkOrchestrationError::InvalidSignal)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkOrchestrationError {
    #[error("orchestration policy revision must be positive, got {0}")]
    InvalidRevision(i64),
    #[error("orchestration weight cannot exceed {MAX_ORCHESTRATION_WEIGHT}, got {0}")]
    InvalidWeight(u32),
    #[error("enabled orchestration policy requires at least one non-zero weight")]
    NoEnabledWeight,
    #[error("due horizon must be from 1 to {MAX_DUE_HORIZON_MINUTES} minutes, got {0}")]
    InvalidDueHorizon(u32),
    #[error("candidate limit must be from 1 to {MAX_ORCHESTRATION_CANDIDATES}, got {0}")]
    InvalidCandidateLimit(u16),
    #[error("orchestration score evidence is invalid")]
    InvalidEvidence,
    #[error("orchestration signal is invalid")]
    InvalidSignal,
    #[error("orchestration score overflowed")]
    ScoreOverflow,
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    fn policy(mode: WorkOrchestrationMode) -> WorkOrchestrationPolicyDefinition {
        WorkOrchestrationPolicyDefinition {
            tenant_id: TenantId::new(1).unwrap(),
            facility_id: FacilityId::new(2).unwrap(),
            inventory_owner_id: Some(InventoryOwnerId::new(3).unwrap()),
            mode,
            priority_weight: 10,
            due_urgency_weight: 20,
            proximity_weight: 30,
            interleaving_weight: 40,
            congestion_penalty_weight: 15,
            bottleneck_penalty_weight: 25,
            due_horizon_minutes: 120,
            max_candidates: 50,
        }
    }

    fn evidence() -> OrchestrationScoreEvidence {
        OrchestrationScoreEvidence {
            work_kind: OrchestrationWorkKind::Replenishment,
            task_priority: 80,
            due_at: Some(Utc::now()),
            overdue_seconds: 0,
            due_urgency_basis_points: 7_500,
            current_location_id: LocationId::new(10).unwrap(),
            source_location_id: LocationId::new(11).unwrap(),
            destination_location_id: Some(LocationId::new(12).unwrap()),
            current_travel_sequence: 100,
            source_travel_sequence: 150,
            destination_travel_sequence: Some(180),
            travel_distance: 50,
            proximity_basis_points: 9_500,
            previous_work_kind: Some(OrchestrationWorkKind::Putaway),
            interleaving_compatible: true,
            source_zone_id: Some(9),
            source_zone_code: Some("RESERVE".into()),
            congestion_basis_points: 2_000,
            congestion_queue_depth: 4,
            resource_kind: WorkResourceKind::MaterialHandling,
            resource_available_units: 4,
            resource_demand_units: 3,
            resource_utilization_basis_points: 7_500,
        }
    }

    #[test]
    fn scoring_is_deterministic_explainable_and_penalizes_constraints() {
        let score =
            score_orchestration_candidate(&policy(WorkOrchestrationMode::Enabled), &evidence())
                .unwrap();
        assert_eq!(score.priority_component, 8_000);
        assert_eq!(score.due_urgency_component, 150_000);
        assert_eq!(score.proximity_component, 285_000);
        assert_eq!(score.interleaving_component, 400_000);
        assert_eq!(score.congestion_penalty, 30_000);
        assert_eq!(score.bottleneck_penalty, 187_500);
        assert_eq!(score.total, 625_500);
    }

    #[test]
    fn invalid_policy_and_forged_interleave_evidence_fail_closed() {
        let mut invalid = policy(WorkOrchestrationMode::Enabled);
        invalid.priority_weight = MAX_ORCHESTRATION_WEIGHT + 1;
        assert!(invalid.validate().is_err());
        let mut forged = evidence();
        forged.previous_work_kind = Some(OrchestrationWorkKind::CycleCountLocation);
        assert!(
            score_orchestration_candidate(&policy(WorkOrchestrationMode::Enabled), &forged)
                .is_err()
        );
    }

    #[test]
    fn resource_utilization_is_bounded_and_zero_capacity_is_visible() {
        assert_eq!(resource_utilization_basis_points(4, 3), 7_500);
        assert_eq!(resource_utilization_basis_points(1, 2), 10_000);
        assert_eq!(resource_utilization_basis_points(0, 1), 10_000);
        assert_eq!(resource_utilization_basis_points(0, 0), 0);
    }
}
