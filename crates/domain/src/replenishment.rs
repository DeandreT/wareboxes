use std::cmp::Ordering;
use std::collections::HashSet;
use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{
    CatalogItemId, FacilityId, InventoryBalanceId, InventoryOwnerId, ItemBatchId, LocationId,
    TenantId, Timestamp,
};

pub const MAX_REPLENISHMENT_UOM_LENGTH: usize = 32;
pub const MAX_REPLENISHMENT_SCAN_VALUE_LENGTH: usize = 200;
pub const MAX_REPLENISHMENT_CANCELLATION_NOTE_LENGTH: usize = 500;

/// Canonical unit of measure on one policy natural key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ReplenishmentUom(String);

impl ReplenishmentUom {
    pub fn new(value: impl Into<String>) -> Result<Self, ReplenishmentError> {
        let value = value.into();
        validate_text(&value, MAX_REPLENISHMENT_UOM_LENGTH)
            .map_err(|()| ReplenishmentError::InvalidUom)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ReplenishmentUom {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// Nonnegative stock level used by policy snapshots and planning results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ReplenishmentLevel(i64);

impl ReplenishmentLevel {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: i64) -> Result<Self, ReplenishmentError> {
        if value >= 0 {
            Ok(Self(value))
        } else {
            Err(ReplenishmentError::NegativeLevel { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for ReplenishmentLevel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// Positive quantity attached to one executable replenishment movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ReplenishmentMoveQuantity(i64);

impl ReplenishmentMoveQuantity {
    pub const fn new(value: i64) -> Result<Self, ReplenishmentError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(ReplenishmentError::InvalidMoveQuantity { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for ReplenishmentMoveQuantity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// Optimistic revision of the active policy for a natural key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ReplenishmentPolicyRevision(i64);

impl ReplenishmentPolicyRevision {
    pub const fn new(value: i64) -> Result<Self, ReplenishmentError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(ReplenishmentError::InvalidPolicyRevision { value })
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

impl<'de> Deserialize<'de> for ReplenishmentPolicyRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// Natural key on which at most one active policy may exist.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReplenishmentPolicyScope {
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub item_id: CatalogItemId,
    pub uom: ReplenishmentUom,
    pub pick_face_location_id: LocationId,
}

/// Min/target levels with a meaningful replenishment interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ReplenishmentPolicyThresholds {
    minimum: ReplenishmentLevel,
    target: ReplenishmentLevel,
}

impl ReplenishmentPolicyThresholds {
    pub const fn new(
        minimum: ReplenishmentLevel,
        target: ReplenishmentLevel,
    ) -> Result<Self, ReplenishmentError> {
        if target.get() <= minimum.get() {
            return Err(ReplenishmentError::TargetNotAboveMinimum {
                minimum: minimum.get(),
                target: target.get(),
            });
        }
        Ok(Self { minimum, target })
    }

    pub const fn minimum(self) -> ReplenishmentLevel {
        self.minimum
    }

    pub const fn target(self) -> ReplenishmentLevel {
        self.target
    }
}

impl<'de> Deserialize<'de> for ReplenishmentPolicyThresholds {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawThresholds {
            minimum: ReplenishmentLevel,
            target: ReplenishmentLevel,
        }

        let raw = RawThresholds::deserialize(deserializer)?;
        Self::new(raw.minimum, raw.target).map_err(D::Error::custom)
    }
}

/// Canonical nonempty set of policy-version reserve source locations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ReplenishmentReserveSourceLocationIds(Vec<LocationId>);

impl ReplenishmentReserveSourceLocationIds {
    pub fn new(mut location_ids: Vec<LocationId>) -> Result<Self, ReplenishmentError> {
        location_ids.sort_unstable_by_key(|location_id| location_id.get());
        location_ids.dedup();
        if location_ids.is_empty() {
            return Err(ReplenishmentError::EmptyReserveSourceSet);
        }
        Ok(Self(location_ids))
    }

    pub fn as_slice(&self) -> &[LocationId] {
        &self.0
    }

    pub fn contains(&self, location_id: LocationId) -> bool {
        self.0
            .binary_search_by_key(&location_id.get(), |candidate| candidate.get())
            .is_ok()
    }
}

impl<'de> Deserialize<'de> for ReplenishmentReserveSourceLocationIds {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(Vec::<LocationId>::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// Complete versioned policy definition, independent of persistence identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplenishmentPolicyDefinition {
    scope: ReplenishmentPolicyScope,
    thresholds: ReplenishmentPolicyThresholds,
    reserve_source_location_ids: ReplenishmentReserveSourceLocationIds,
}

impl ReplenishmentPolicyDefinition {
    pub fn new(
        scope: ReplenishmentPolicyScope,
        thresholds: ReplenishmentPolicyThresholds,
        reserve_source_location_ids: ReplenishmentReserveSourceLocationIds,
    ) -> Result<Self, ReplenishmentError> {
        if reserve_source_location_ids.contains(scope.pick_face_location_id) {
            return Err(ReplenishmentError::PickFaceIsReserveSource {
                location_id: scope.pick_face_location_id.get(),
            });
        }
        Ok(Self {
            scope,
            thresholds,
            reserve_source_location_ids,
        })
    }

    pub const fn scope(&self) -> &ReplenishmentPolicyScope {
        &self.scope
    }

    pub const fn thresholds(&self) -> ReplenishmentPolicyThresholds {
        self.thresholds
    }

    pub const fn reserve_source_location_ids(&self) -> &ReplenishmentReserveSourceLocationIds {
        &self.reserve_source_location_ids
    }
}

impl<'de> Deserialize<'de> for ReplenishmentPolicyDefinition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawPolicy {
            scope: ReplenishmentPolicyScope,
            thresholds: ReplenishmentPolicyThresholds,
            reserve_source_location_ids: ReplenishmentReserveSourceLocationIds,
        }

        let raw = RawPolicy::deserialize(deserializer)?;
        Self::new(raw.scope, raw.thresholds, raw.reserve_source_location_ids)
            .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplenishmentPolicyStatus {
    Active,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplenishmentWorkStatus {
    Pending,
    Claimed,
    Completed,
    Cancelled,
}

impl ReplenishmentWorkStatus {
    pub const fn claim(self) -> Result<Self, ReplenishmentError> {
        match self {
            Self::Pending => Ok(Self::Claimed),
            status => Err(ReplenishmentError::WorkNotClaimable { status }),
        }
    }

    pub const fn release(self) -> Result<Self, ReplenishmentError> {
        match self {
            Self::Claimed => Ok(Self::Pending),
            status => Err(ReplenishmentError::WorkNotReleasable { status }),
        }
    }

    pub const fn confirm(self) -> Result<Self, ReplenishmentError> {
        match self {
            Self::Claimed => Ok(Self::Completed),
            status => Err(ReplenishmentError::WorkNotConfirmable { status }),
        }
    }

    pub const fn cancel(self) -> Result<Self, ReplenishmentError> {
        match self {
            Self::Pending => Ok(Self::Cancelled),
            status => Err(ReplenishmentError::WorkNotCancellable { status }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplenishmentClaimReleaseReason {
    WorkInterrupted,
    EquipmentUnavailable,
    SourceBlocked,
    DestinationBlocked,
    InventoryMismatch,
    SafetyIssue,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplenishmentWorkCancellationReason {
    DemandRemoved,
    PolicyReconfigured,
    SourceUnavailable,
    DestinationUnavailable,
    PlanningError,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ReplenishmentWorkCancellationNote(String);

impl ReplenishmentWorkCancellationNote {
    pub fn new(value: impl Into<String>) -> Result<Self, ReplenishmentError> {
        let value = value.into();
        validate_text(&value, MAX_REPLENISHMENT_CANCELLATION_NOTE_LENGTH)
            .map_err(|()| ReplenishmentError::InvalidCancellationNote)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ReplenishmentWorkCancellationNote {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplenishmentInventoryStatus {
    Available,
    Hold,
    Damaged,
    Quarantine,
}

/// Inventory facts evaluated against the exact policy version being planned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplenishmentSourceCandidate {
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub source_location_id: LocationId,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub item_batch_id: ItemBatchId,
    pub item_id: CatalogItemId,
    pub uom: ReplenishmentUom,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub received_at: Timestamp,
    pub inventory_status: ReplenishmentInventoryStatus,
    pub free_quantity: ReplenishmentLevel,
}

/// Rejection is explicit so a planner cannot silently broaden source selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplenishmentSourceIneligibility {
    TenantMismatch,
    InventoryOwnerMismatch,
    FacilityMismatch,
    SourceIsDestinationPickFace,
    SourceNotConfigured,
    ItemMismatch,
    UomMismatch,
    InventoryNotAvailable,
    NoFreeQuantity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplenishmentSourceEligibility {
    Eligible(EligibleReplenishmentSource),
    Ineligible(ReplenishmentSourceIneligibility),
}

/// Candidate proven eligible for deterministic source planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EligibleReplenishmentSource(ReplenishmentSourceCandidate);

impl EligibleReplenishmentSource {
    pub const fn candidate(&self) -> &ReplenishmentSourceCandidate {
        &self.0
    }
}

pub fn assess_replenishment_source(
    policy: &ReplenishmentPolicyDefinition,
    candidate: ReplenishmentSourceCandidate,
) -> ReplenishmentSourceEligibility {
    let scope = policy.scope();
    let rejection = if candidate.tenant_id != scope.tenant_id {
        Some(ReplenishmentSourceIneligibility::TenantMismatch)
    } else if candidate.inventory_owner_id != scope.inventory_owner_id {
        Some(ReplenishmentSourceIneligibility::InventoryOwnerMismatch)
    } else if candidate.facility_id != scope.facility_id {
        Some(ReplenishmentSourceIneligibility::FacilityMismatch)
    } else if candidate.source_location_id == scope.pick_face_location_id {
        Some(ReplenishmentSourceIneligibility::SourceIsDestinationPickFace)
    } else if !policy
        .reserve_source_location_ids()
        .contains(candidate.source_location_id)
    {
        Some(ReplenishmentSourceIneligibility::SourceNotConfigured)
    } else if candidate.item_id != scope.item_id {
        Some(ReplenishmentSourceIneligibility::ItemMismatch)
    } else if candidate.uom != scope.uom {
        Some(ReplenishmentSourceIneligibility::UomMismatch)
    } else if candidate.inventory_status != ReplenishmentInventoryStatus::Available {
        Some(ReplenishmentSourceIneligibility::InventoryNotAvailable)
    } else if candidate.free_quantity == ReplenishmentLevel::ZERO {
        Some(ReplenishmentSourceIneligibility::NoFreeQuantity)
    } else {
        None
    };

    match rejection {
        Some(reason) => ReplenishmentSourceEligibility::Ineligible(reason),
        None => ReplenishmentSourceEligibility::Eligible(EligibleReplenishmentSource(candidate)),
    }
}

/// Server-observed facts retained on every planning result for replay and audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ReplenishmentPlanningSnapshot {
    pick_face_free: ReplenishmentLevel,
    active_inbound: ReplenishmentLevel,
    projected_free: ReplenishmentLevel,
    unallocated_demand: ReplenishmentLevel,
    reserve_free: ReplenishmentLevel,
}

impl ReplenishmentPlanningSnapshot {
    pub fn new(
        pick_face_free: ReplenishmentLevel,
        active_inbound: ReplenishmentLevel,
        unallocated_demand: ReplenishmentLevel,
        reserve_free: ReplenishmentLevel,
    ) -> Result<Self, ReplenishmentError> {
        let projected_free = pick_face_free
            .get()
            .checked_add(active_inbound.get())
            .ok_or(ReplenishmentError::ProjectedFreeOverflow)?;
        Ok(Self {
            pick_face_free,
            active_inbound,
            projected_free: ReplenishmentLevel(projected_free),
            unallocated_demand,
            reserve_free,
        })
    }

    pub const fn pick_face_free(self) -> ReplenishmentLevel {
        self.pick_face_free
    }

    pub const fn active_inbound(self) -> ReplenishmentLevel {
        self.active_inbound
    }

    pub const fn projected_free(self) -> ReplenishmentLevel {
        self.projected_free
    }

    pub const fn unallocated_demand(self) -> ReplenishmentLevel {
        self.unallocated_demand
    }

    pub const fn reserve_free(self) -> ReplenishmentLevel {
        self.reserve_free
    }
}

impl<'de> Deserialize<'de> for ReplenishmentPlanningSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawSnapshot {
            pick_face_free: ReplenishmentLevel,
            active_inbound: ReplenishmentLevel,
            projected_free: ReplenishmentLevel,
            unallocated_demand: ReplenishmentLevel,
            reserve_free: ReplenishmentLevel,
        }

        let raw = RawSnapshot::deserialize(deserializer)?;
        let snapshot = Self::new(
            raw.pick_face_free,
            raw.active_inbound,
            raw.unallocated_demand,
            raw.reserve_free,
        )
        .map_err(D::Error::custom)?;
        if snapshot.projected_free != raw.projected_free {
            return Err(D::Error::custom(
                ReplenishmentError::InconsistentProjectedFree {
                    expected: snapshot.projected_free.get(),
                    actual: raw.projected_free.get(),
                },
            ));
        }
        Ok(snapshot)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplenishmentPlanningOutcome {
    NotNeeded,
    InsufficientReserve,
    PartiallyPlanned,
    FullyPlanned,
}

/// Aggregate policy decision before individual reserve balances are selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplenishmentPlanDecision {
    pub snapshot: ReplenishmentPlanningSnapshot,
    pub required_level: ReplenishmentLevel,
    pub target_gap: ReplenishmentLevel,
    pub planned: ReplenishmentLevel,
    pub remaining: ReplenishmentLevel,
    pub outcome: ReplenishmentPlanningOutcome,
}

pub fn plan_replenishment(
    thresholds: ReplenishmentPolicyThresholds,
    snapshot: ReplenishmentPlanningSnapshot,
) -> ReplenishmentPlanDecision {
    let actionable = snapshot.projected_free().get() < thresholds.minimum().get()
        || snapshot.projected_free().get() < snapshot.unallocated_demand().get();
    let required_level = ReplenishmentLevel(
        thresholds
            .target()
            .get()
            .max(snapshot.unallocated_demand().get()),
    );
    let target_gap = if actionable {
        ReplenishmentLevel(
            required_level
                .get()
                .saturating_sub(snapshot.projected_free().get()),
        )
    } else {
        ReplenishmentLevel::ZERO
    };
    let planned = ReplenishmentLevel(target_gap.get().min(snapshot.reserve_free().get()));
    let remaining = ReplenishmentLevel(target_gap.get() - planned.get());
    let outcome = if !actionable {
        ReplenishmentPlanningOutcome::NotNeeded
    } else if planned == ReplenishmentLevel::ZERO {
        ReplenishmentPlanningOutcome::InsufficientReserve
    } else if planned < target_gap {
        ReplenishmentPlanningOutcome::PartiallyPlanned
    } else {
        ReplenishmentPlanningOutcome::FullyPlanned
    };

    ReplenishmentPlanDecision {
        snapshot,
        required_level,
        target_gap,
        planned,
        remaining,
        outcome,
    }
}

/// One FEFO-ordered source movement. Sequence begins at one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedReplenishmentSource {
    pub sequence: u32,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub item_batch_id: ItemBatchId,
    pub source_location_id: LocationId,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub received_at: Timestamp,
    pub quantity: ReplenishmentMoveQuantity,
}

/// Selects exact sources by expiry (NULLS LAST), receipt time, then balance ID.
pub fn select_replenishment_sources(
    decision: ReplenishmentPlanDecision,
    mut sources: Vec<EligibleReplenishmentSource>,
) -> Result<Vec<PlannedReplenishmentSource>, ReplenishmentError> {
    if decision.planned == ReplenishmentLevel::ZERO {
        return Ok(Vec::new());
    }

    let mut unique_balances = HashSet::with_capacity(sources.len());
    for source in &sources {
        let balance_id = source.candidate().source_inventory_balance_id;
        if !unique_balances.insert(balance_id) {
            return Err(ReplenishmentError::DuplicateSourceBalance {
                inventory_balance_id: balance_id.get(),
            });
        }
    }

    sources.sort_by(compare_eligible_sources);
    let mut remaining = decision.planned.get();
    let mut planned = Vec::new();
    for source in sources {
        if remaining == 0 {
            break;
        }
        let candidate = source.0;
        let quantity = remaining.min(candidate.free_quantity.get());
        remaining -= quantity;
        planned.push(PlannedReplenishmentSource {
            sequence: u32::try_from(planned.len() + 1)
                .map_err(|_| ReplenishmentError::TooManySourceBalances)?,
            source_inventory_balance_id: candidate.source_inventory_balance_id,
            item_batch_id: candidate.item_batch_id,
            source_location_id: candidate.source_location_id,
            lot: candidate.lot,
            serial: candidate.serial,
            expiration: candidate.expiration,
            received_at: candidate.received_at,
            quantity: ReplenishmentMoveQuantity(quantity),
        });
    }
    if remaining != 0 {
        return Err(ReplenishmentError::ReserveSnapshotMismatch {
            unplanned_quantity: remaining,
        });
    }
    Ok(planned)
}

fn compare_eligible_sources(
    left: &EligibleReplenishmentSource,
    right: &EligibleReplenishmentSource,
) -> Ordering {
    let left = left.candidate();
    let right = right.candidate();
    match (&left.expiration, &right.expiration) {
        (Some(left), Some(right)) => left.cmp(right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
    .then_with(|| left.received_at.cmp(&right.received_at))
    .then_with(|| {
        left.source_inventory_balance_id
            .get()
            .cmp(&right.source_inventory_balance_id.get())
    })
}

/// Ensures an in-memory active projection reflects the database uniqueness invariant.
pub fn validate_unique_active_replenishment_policy_scopes(
    scopes: &[ReplenishmentPolicyScope],
) -> Result<(), ReplenishmentError> {
    let mut unique = HashSet::with_capacity(scopes.len());
    for scope in scopes {
        if !unique.insert(scope) {
            return Err(ReplenishmentError::DuplicateActivePolicy {
                scope: scope.clone(),
            });
        }
    }
    Ok(())
}

/// Bounded scanner evidence accepted by loose-stock confirmation commands.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ReplenishmentScanValue(String);

impl ReplenishmentScanValue {
    pub fn new(value: impl Into<String>) -> Result<Self, ReplenishmentError> {
        let value = value.into();
        validate_text(&value, MAX_REPLENISHMENT_SCAN_VALUE_LENGTH)
            .map_err(|()| ReplenishmentError::InvalidScanValue)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ReplenishmentScanValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

fn validate_text(value: &str, max_length: usize) -> Result<(), ()> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().count() > max_length
        || value.chars().any(char::is_control)
    {
        Err(())
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReplenishmentError {
    #[error(
        "replenishment UOM must be nonblank, trimmed, control-free, and at most {MAX_REPLENISHMENT_UOM_LENGTH} characters"
    )]
    InvalidUom,
    #[error("replenishment level cannot be negative, got {value}")]
    NegativeLevel { value: i64 },
    #[error("replenishment movement quantity must be positive, got {value}")]
    InvalidMoveQuantity { value: i64 },
    #[error("replenishment policy revision must be positive, got {value}")]
    InvalidPolicyRevision { value: i64 },
    #[error("target {target} must be greater than minimum {minimum}")]
    TargetNotAboveMinimum { minimum: i64, target: i64 },
    #[error("a replenishment policy must have at least one reserve source location")]
    EmptyReserveSourceSet,
    #[error("pick face location {location_id} cannot also be a reserve source")]
    PickFaceIsReserveSource { location_id: i64 },
    #[error("an active replenishment policy already exists for {scope:?}")]
    DuplicateActivePolicy { scope: ReplenishmentPolicyScope },
    #[error("replenishment work in {status:?} status cannot be claimed")]
    WorkNotClaimable { status: ReplenishmentWorkStatus },
    #[error("replenishment work in {status:?} status cannot be released")]
    WorkNotReleasable { status: ReplenishmentWorkStatus },
    #[error("replenishment work in {status:?} status cannot be confirmed")]
    WorkNotConfirmable { status: ReplenishmentWorkStatus },
    #[error("replenishment work in {status:?} status cannot be cancelled")]
    WorkNotCancellable { status: ReplenishmentWorkStatus },
    #[error(
        "replenishment cancellation note must be nonblank, trimmed, control-free, and at most {MAX_REPLENISHMENT_CANCELLATION_NOTE_LENGTH} characters"
    )]
    InvalidCancellationNote,
    #[error("replenishment cancellation note is required when reason is other")]
    CancellationNoteRequired,
    #[error("inventory balance {inventory_balance_id} appears more than once")]
    DuplicateSourceBalance { inventory_balance_id: i64 },
    #[error("eligible reserve sources are short by {unplanned_quantity}")]
    ReserveSnapshotMismatch { unplanned_quantity: i64 },
    #[error("too many source balances to assign a deterministic sequence")]
    TooManySourceBalances,
    #[error("pick-face free plus active inbound exceeds the supported quantity range")]
    ProjectedFreeOverflow,
    #[error(
        "projected free must equal pick-face free plus active inbound: expected {expected}, got {actual}"
    )]
    InconsistentProjectedFree { expected: i64, actual: i64 },
    #[error(
        "replenishment scan must be nonblank, trimmed, control-free, and at most {MAX_REPLENISHMENT_SCAN_VALUE_LENGTH} characters"
    )]
    InvalidScanValue,
}

impl fmt::Display for ReplenishmentUom {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id<T, E>(value: i64, constructor: impl FnOnce(i64) -> Result<T, E>) -> T {
        constructor(value).ok().unwrap()
    }

    fn scope() -> ReplenishmentPolicyScope {
        ReplenishmentPolicyScope {
            tenant_id: id(1, TenantId::new),
            inventory_owner_id: id(2, InventoryOwnerId::new),
            facility_id: id(3, FacilityId::new),
            item_id: CatalogItemId::new(4).unwrap(),
            uom: ReplenishmentUom::new("each").unwrap(),
            pick_face_location_id: id(20, LocationId::new),
        }
    }

    fn policy() -> ReplenishmentPolicyDefinition {
        ReplenishmentPolicyDefinition::new(
            scope(),
            ReplenishmentPolicyThresholds::new(
                ReplenishmentLevel::new(5).unwrap(),
                ReplenishmentLevel::new(20).unwrap(),
            )
            .unwrap(),
            ReplenishmentReserveSourceLocationIds::new(vec![
                id(12, LocationId::new),
                id(10, LocationId::new),
                id(12, LocationId::new),
            ])
            .unwrap(),
        )
        .unwrap()
    }

    fn candidate(
        balance_id: i64,
        expiration: Option<&str>,
        received_at: &str,
    ) -> ReplenishmentSourceCandidate {
        ReplenishmentSourceCandidate {
            tenant_id: id(1, TenantId::new),
            inventory_owner_id: id(2, InventoryOwnerId::new),
            facility_id: id(3, FacilityId::new),
            source_location_id: id(10, LocationId::new),
            source_inventory_balance_id: id(balance_id, InventoryBalanceId::new),
            item_batch_id: id(balance_id + 100, ItemBatchId::new),
            item_id: CatalogItemId::new(4).unwrap(),
            uom: ReplenishmentUom::new("each").unwrap(),
            lot: Some(format!("LOT-{balance_id}")),
            serial: None,
            expiration: expiration.map(|value| value.parse::<Timestamp>().unwrap()),
            received_at: received_at.parse::<Timestamp>().unwrap(),
            inventory_status: ReplenishmentInventoryStatus::Available,
            free_quantity: ReplenishmentLevel::new(10).unwrap(),
        }
    }

    #[test]
    fn policy_sources_are_nonempty_canonical_and_exclude_the_pick_face() {
        let definition = policy();
        assert_eq!(
            definition
                .reserve_source_location_ids()
                .as_slice()
                .iter()
                .map(|id| id.get())
                .collect::<Vec<_>>(),
            vec![10, 12]
        );
        assert_eq!(
            ReplenishmentPolicyDefinition::new(
                scope(),
                definition.thresholds(),
                ReplenishmentReserveSourceLocationIds::new(vec![id(20, LocationId::new)]).unwrap(),
            ),
            Err(ReplenishmentError::PickFaceIsReserveSource { location_id: 20 })
        );
        assert_eq!(
            ReplenishmentReserveSourceLocationIds::new(Vec::new()),
            Err(ReplenishmentError::EmptyReserveSourceSet)
        );
    }

    #[test]
    fn active_policy_natural_keys_are_unique() {
        let scope = scope();
        assert_eq!(
            validate_unique_active_replenishment_policy_scopes(&[scope.clone(), scope.clone()]),
            Err(ReplenishmentError::DuplicateActivePolicy { scope })
        );
    }

    #[test]
    fn source_eligibility_requires_the_exact_policy_scope_and_source_set() {
        let policy = policy();
        let eligible = assess_replenishment_source(
            &policy,
            candidate(30, Some("2026-09-01T00:00:00Z"), "2026-08-01T00:00:00Z"),
        );
        assert!(matches!(
            eligible,
            ReplenishmentSourceEligibility::Eligible(_)
        ));

        let mut unconfigured = candidate(31, Some("2026-09-01T00:00:00Z"), "2026-08-01T00:00:00Z");
        unconfigured.source_location_id = id(99, LocationId::new);
        assert_eq!(
            assess_replenishment_source(&policy, unconfigured),
            ReplenishmentSourceEligibility::Ineligible(
                ReplenishmentSourceIneligibility::SourceNotConfigured
            )
        );
    }

    #[test]
    fn planning_outcomes_follow_demand_trigger_and_reserve_capacity() {
        let thresholds = policy().thresholds();
        let decide = |projected_free, demand, reserve_free| {
            plan_replenishment(
                thresholds,
                ReplenishmentPlanningSnapshot::new(
                    ReplenishmentLevel::new(projected_free).unwrap(),
                    ReplenishmentLevel::ZERO,
                    ReplenishmentLevel::new(demand).unwrap(),
                    ReplenishmentLevel::new(reserve_free).unwrap(),
                )
                .unwrap(),
            )
        };

        assert_eq!(
            decide(8, 4, 100).outcome,
            ReplenishmentPlanningOutcome::NotNeeded
        );
        assert_eq!(
            decide(2, 4, 0).outcome,
            ReplenishmentPlanningOutcome::InsufficientReserve
        );
        let partial = decide(2, 30, 10);
        assert_eq!(partial.required_level.get(), 30);
        assert_eq!(partial.target_gap.get(), 28);
        assert_eq!(partial.planned.get(), 10);
        assert_eq!(partial.remaining.get(), 18);
        assert_eq!(
            partial.outcome,
            ReplenishmentPlanningOutcome::PartiallyPlanned
        );
        assert_eq!(
            decide(2, 4, 18).outcome,
            ReplenishmentPlanningOutcome::FullyPlanned
        );
    }

    #[test]
    fn source_work_is_fefo_with_null_expiry_last_and_stable_ties() {
        let policy = policy();
        let decision = plan_replenishment(
            policy.thresholds(),
            ReplenishmentPlanningSnapshot::new(
                ReplenishmentLevel::new(2).unwrap(),
                ReplenishmentLevel::ZERO,
                ReplenishmentLevel::new(4).unwrap(),
                ReplenishmentLevel::new(18).unwrap(),
            )
            .unwrap(),
        );
        let candidates = vec![
            candidate(33, None, "2026-07-01T00:00:00Z"),
            candidate(32, Some("2026-09-01T00:00:00Z"), "2026-07-02T00:00:00Z"),
            candidate(31, Some("2026-09-01T00:00:00Z"), "2026-07-02T00:00:00Z"),
        ];
        let eligible = candidates
            .into_iter()
            .map(
                |candidate| match assess_replenishment_source(&policy, candidate) {
                    ReplenishmentSourceEligibility::Eligible(source) => source,
                    ReplenishmentSourceEligibility::Ineligible(reason) => {
                        panic!("unexpected ineligible source: {reason:?}")
                    }
                },
            )
            .collect();
        let planned = select_replenishment_sources(decision, eligible).unwrap();

        assert_eq!(
            planned
                .iter()
                .map(|source| (
                    source.sequence,
                    source.source_inventory_balance_id.get(),
                    source.quantity.get()
                ))
                .collect::<Vec<_>>(),
            vec![(1, 31, 10), (2, 32, 8)]
        );
    }

    #[test]
    fn planning_snapshot_computes_and_validates_projected_free() {
        let snapshot = ReplenishmentPlanningSnapshot::new(
            ReplenishmentLevel::new(7).unwrap(),
            ReplenishmentLevel::new(3).unwrap(),
            ReplenishmentLevel::new(4).unwrap(),
            ReplenishmentLevel::new(20).unwrap(),
        )
        .unwrap();
        assert_eq!(snapshot.projected_free().get(), 10);

        let value = serde_json::to_value(snapshot).unwrap();
        assert_eq!(value["pick_face_free"], 7);
        assert_eq!(value["projected_free"], 10);
        assert!(
            serde_json::from_value::<ReplenishmentPlanningSnapshot>(serde_json::json!({
                "pick_face_free": 7,
                "active_inbound": 3,
                "projected_free": 9,
                "unallocated_demand": 4,
                "reserve_free": 20
            }))
            .is_err()
        );
    }

    #[test]
    fn scanner_values_and_work_transitions_are_strict() {
        assert!(ReplenishmentScanValue::new("R-01").is_ok());
        assert!(ReplenishmentScanValue::new(" R-01 ").is_err());
        assert_eq!(
            ReplenishmentWorkStatus::Pending.claim(),
            Ok(ReplenishmentWorkStatus::Claimed)
        );
        assert!(ReplenishmentWorkStatus::Pending.confirm().is_err());
        assert_eq!(
            ReplenishmentWorkStatus::Pending.cancel(),
            Ok(ReplenishmentWorkStatus::Cancelled)
        );
        assert!(ReplenishmentWorkStatus::Claimed.cancel().is_err());
        assert!(ReplenishmentWorkCancellationNote::new("verified obstruction").is_ok());
        assert!(ReplenishmentWorkCancellationNote::new(" invalid ").is_err());
    }
}
