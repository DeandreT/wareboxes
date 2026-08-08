use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Serialize};

use crate::{LocationId, OutboundLoadId};

pub const MAX_OUTBOUND_LOAD_REFERENCE_LENGTH: usize = 100;
pub const MAX_OUTBOUND_LOAD_TRAILER_NUMBER_LENGTH: usize = 100;
pub const MAX_OUTBOUND_LOAD_SEAL_NUMBER_LENGTH: usize = 100;
pub const MAX_OUTBOUND_LOAD_SCAN_VALUE_LENGTH: usize = 200;
pub const MAX_OUTBOUND_LOAD_CANCELLATION_NOTE_LENGTH: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundLoadStatus {
    Planned,
    Staging,
    Loading,
    ReadyToDepart,
    Departed,
    Cancelled,
}

impl OutboundLoadStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Staging => "staging",
            Self::Loading => "loading",
            Self::ReadyToDepart => "ready_to_depart",
            Self::Departed => "departed",
            Self::Cancelled => "cancelled",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Departed | Self::Cancelled)
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "planned" => Some(Self::Planned),
            "staging" => Some(Self::Staging),
            "loading" => Some(Self::Loading),
            "ready_to_depart" => Some(Self::ReadyToDepart),
            "departed" => Some(Self::Departed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

impl fmt::Display for OutboundLoadStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

macro_rules! positive_revision {
    ($name:ident, $field:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(i64);

        impl $name {
            pub const fn new(value: i64) -> Result<Self, OutboundLoadError> {
                if value > 0 {
                    Ok(Self(value))
                } else {
                    Err(OutboundLoadError::InvalidRevision {
                        field: $field,
                        value,
                    })
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

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = i64::deserialize(deserializer)?;
                Self::new(value).map_err(D::Error::custom)
            }
        }
    };
}

positive_revision!(OutboundLoadRevision, "outbound load revision");
positive_revision!(
    PackedCartonPositionRevision,
    "packed carton position revision"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PackedCartonPositionState {
    Packed {
        location_id: LocationId,
    },
    Staged {
        outbound_load_id: OutboundLoadId,
        staging_location_id: LocationId,
    },
    Loaded {
        outbound_load_id: OutboundLoadId,
        load_sequence: u32,
    },
    Departed {
        outbound_load_id: Option<OutboundLoadId>,
        load_sequence: Option<u32>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackedCartonMovementKind {
    Stage,
    Load,
    Unload,
    Unstage,
}

impl PackedCartonMovementKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stage => "stage",
            Self::Load => "load",
            Self::Unload => "unload",
            Self::Unstage => "unstage",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "stage" => Some(Self::Stage),
            "load" => Some(Self::Load),
            "unload" => Some(Self::Unload),
            "unstage" => Some(Self::Unstage),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundLoadProgress {
    planned_shipment_count: u32,
    planned_carton_count: u32,
    staged_carton_count: u32,
    loaded_carton_count: u32,
    status: OutboundLoadStatus,
}

impl OutboundLoadProgress {
    pub fn planned(
        planned_shipment_count: u32,
        planned_carton_count: u32,
    ) -> Result<Self, OutboundLoadError> {
        Self::restore(
            planned_shipment_count,
            planned_carton_count,
            0,
            0,
            OutboundLoadStatus::Planned,
        )
    }

    pub fn restore(
        planned_shipment_count: u32,
        planned_carton_count: u32,
        staged_carton_count: u32,
        loaded_carton_count: u32,
        status: OutboundLoadStatus,
    ) -> Result<Self, OutboundLoadError> {
        let progress = Self {
            planned_shipment_count,
            planned_carton_count,
            staged_carton_count,
            loaded_carton_count,
            status,
        };
        progress.validate()?;
        Ok(progress)
    }

    pub const fn planned_shipment_count(self) -> u32 {
        self.planned_shipment_count
    }

    pub const fn planned_carton_count(self) -> u32 {
        self.planned_carton_count
    }

    pub const fn staged_carton_count(self) -> u32 {
        self.staged_carton_count
    }

    pub const fn loaded_carton_count(self) -> u32 {
        self.loaded_carton_count
    }

    pub const fn status(self) -> OutboundLoadStatus {
        self.status
    }

    pub const fn packed_carton_count(self) -> u32 {
        self.planned_carton_count
            .saturating_sub(self.staged_carton_count + self.loaded_carton_count)
    }

    fn validate(self) -> Result<(), OutboundLoadError> {
        if self.planned_shipment_count == 0 || self.planned_carton_count == 0 {
            return Err(OutboundLoadError::EmptyPlan);
        }
        let positioned = self
            .staged_carton_count
            .checked_add(self.loaded_carton_count)
            .ok_or(OutboundLoadError::InvalidProgress)?;
        if positioned > self.planned_carton_count {
            return Err(OutboundLoadError::InvalidProgress);
        }
        let valid_status = match self.status {
            OutboundLoadStatus::Planned | OutboundLoadStatus::Cancelled => positioned == 0,
            OutboundLoadStatus::Staging => self.loaded_carton_count == 0,
            OutboundLoadStatus::Loading => true,
            OutboundLoadStatus::ReadyToDepart | OutboundLoadStatus::Departed => {
                self.staged_carton_count == 0
                    && self.loaded_carton_count == self.planned_carton_count
            }
        };
        if !valid_status {
            return Err(OutboundLoadError::InvalidProgress);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboundLoadTransition {
    pub progress: OutboundLoadProgress,
    pub advances_load_revision: bool,
}

pub fn release_outbound_load(
    mut progress: OutboundLoadProgress,
) -> Result<OutboundLoadTransition, OutboundLoadError> {
    require_status(progress.status, OutboundLoadStatus::Planned)?;
    progress.status = OutboundLoadStatus::Staging;
    Ok(phase_transition(progress))
}

pub fn record_outbound_carton_staged(
    mut progress: OutboundLoadProgress,
) -> Result<OutboundLoadTransition, OutboundLoadError> {
    require_one_of(
        progress.status,
        &[OutboundLoadStatus::Staging, OutboundLoadStatus::Loading],
    )?;
    let positioned = progress
        .staged_carton_count
        .checked_add(progress.loaded_carton_count)
        .ok_or(OutboundLoadError::InvalidProgress)?;
    if positioned >= progress.planned_carton_count {
        return Err(OutboundLoadError::InvalidProgress);
    }
    progress.staged_carton_count += 1;
    Ok(movement_transition(progress))
}

pub fn start_outbound_load_loading(
    mut progress: OutboundLoadProgress,
) -> Result<OutboundLoadTransition, OutboundLoadError> {
    require_status(progress.status, OutboundLoadStatus::Staging)?;
    if progress.staged_carton_count != progress.planned_carton_count
        || progress.loaded_carton_count != 0
    {
        return Err(OutboundLoadError::IncompleteStaging);
    }
    progress.status = OutboundLoadStatus::Loading;
    Ok(phase_transition(progress))
}

pub fn record_outbound_carton_loaded(
    mut progress: OutboundLoadProgress,
) -> Result<OutboundLoadTransition, OutboundLoadError> {
    require_status(progress.status, OutboundLoadStatus::Loading)?;
    if progress.staged_carton_count == 0 {
        return Err(OutboundLoadError::CartonNotStaged);
    }
    progress.staged_carton_count -= 1;
    progress.loaded_carton_count += 1;
    Ok(movement_transition(progress))
}

pub fn complete_outbound_load_loading(
    mut progress: OutboundLoadProgress,
) -> Result<OutboundLoadTransition, OutboundLoadError> {
    require_status(progress.status, OutboundLoadStatus::Loading)?;
    if progress.staged_carton_count != 0
        || progress.loaded_carton_count != progress.planned_carton_count
    {
        return Err(OutboundLoadError::IncompleteLoading);
    }
    progress.status = OutboundLoadStatus::ReadyToDepart;
    Ok(phase_transition(progress))
}

pub fn record_outbound_carton_unloaded(
    mut progress: OutboundLoadProgress,
) -> Result<OutboundLoadTransition, OutboundLoadError> {
    require_one_of(
        progress.status,
        &[
            OutboundLoadStatus::Loading,
            OutboundLoadStatus::ReadyToDepart,
        ],
    )?;
    if progress.loaded_carton_count == 0 {
        return Err(OutboundLoadError::CartonNotLoaded);
    }
    let advances_load_revision = progress.status == OutboundLoadStatus::ReadyToDepart;
    progress.status = OutboundLoadStatus::Loading;
    progress.loaded_carton_count -= 1;
    progress.staged_carton_count += 1;
    Ok(OutboundLoadTransition {
        progress,
        advances_load_revision,
    })
}

pub fn record_outbound_carton_unstaged(
    mut progress: OutboundLoadProgress,
) -> Result<OutboundLoadTransition, OutboundLoadError> {
    require_one_of(
        progress.status,
        &[OutboundLoadStatus::Staging, OutboundLoadStatus::Loading],
    )?;
    if progress.staged_carton_count == 0 {
        return Err(OutboundLoadError::CartonNotStaged);
    }
    progress.staged_carton_count -= 1;
    Ok(movement_transition(progress))
}

pub fn depart_outbound_load(
    mut progress: OutboundLoadProgress,
) -> Result<OutboundLoadTransition, OutboundLoadError> {
    require_status(progress.status, OutboundLoadStatus::ReadyToDepart)?;
    progress.status = OutboundLoadStatus::Departed;
    Ok(phase_transition(progress))
}

pub fn cancel_outbound_load(
    mut progress: OutboundLoadProgress,
    all_cartons_at_original_packed_position: bool,
) -> Result<OutboundLoadTransition, OutboundLoadError> {
    require_one_of(
        progress.status,
        &[
            OutboundLoadStatus::Planned,
            OutboundLoadStatus::Staging,
            OutboundLoadStatus::Loading,
        ],
    )?;
    if progress.staged_carton_count != 0
        || progress.loaded_carton_count != 0
        || !all_cartons_at_original_packed_position
    {
        return Err(OutboundLoadError::CartonsNotRestored);
    }
    progress.status = OutboundLoadStatus::Cancelled;
    Ok(phase_transition(progress))
}

pub fn stage_packed_carton(
    current: PackedCartonPositionState,
    original_packed_location_id: LocationId,
    outbound_load_id: OutboundLoadId,
    staging_location_id: LocationId,
) -> Result<PackedCartonPositionState, OutboundLoadError> {
    match current {
        PackedCartonPositionState::Packed { location_id }
            if location_id == original_packed_location_id && location_id != staging_location_id =>
        {
            Ok(PackedCartonPositionState::Staged {
                outbound_load_id,
                staging_location_id,
            })
        }
        _ => Err(OutboundLoadError::InvalidCartonPositionTransition),
    }
}

pub fn load_packed_carton(
    current: PackedCartonPositionState,
    outbound_load_id: OutboundLoadId,
    load_sequence: u32,
) -> Result<PackedCartonPositionState, OutboundLoadError> {
    if load_sequence == 0 {
        return Err(OutboundLoadError::InvalidLoadSequence);
    }
    match current {
        PackedCartonPositionState::Staged {
            outbound_load_id: current_load_id,
            ..
        } if current_load_id == outbound_load_id => Ok(PackedCartonPositionState::Loaded {
            outbound_load_id,
            load_sequence,
        }),
        _ => Err(OutboundLoadError::InvalidCartonPositionTransition),
    }
}

pub fn unload_packed_carton(
    current: PackedCartonPositionState,
    outbound_load_id: OutboundLoadId,
    staging_location_id: LocationId,
) -> Result<PackedCartonPositionState, OutboundLoadError> {
    match current {
        PackedCartonPositionState::Loaded {
            outbound_load_id: current_load_id,
            ..
        } if current_load_id == outbound_load_id => Ok(PackedCartonPositionState::Staged {
            outbound_load_id,
            staging_location_id,
        }),
        _ => Err(OutboundLoadError::InvalidCartonPositionTransition),
    }
}

pub fn unstage_packed_carton(
    current: PackedCartonPositionState,
    outbound_load_id: OutboundLoadId,
    original_packed_location_id: LocationId,
) -> Result<PackedCartonPositionState, OutboundLoadError> {
    match current {
        PackedCartonPositionState::Staged {
            outbound_load_id: current_load_id,
            ..
        } if current_load_id == outbound_load_id => Ok(PackedCartonPositionState::Packed {
            location_id: original_packed_location_id,
        }),
        _ => Err(OutboundLoadError::InvalidCartonPositionTransition),
    }
}

pub fn depart_packed_carton(
    current: PackedCartonPositionState,
    outbound_load_id: OutboundLoadId,
    load_sequence: u32,
) -> Result<PackedCartonPositionState, OutboundLoadError> {
    match current {
        PackedCartonPositionState::Loaded {
            outbound_load_id: current_load_id,
            load_sequence: current_sequence,
        } if current_load_id == outbound_load_id && current_sequence == load_sequence => {
            Ok(PackedCartonPositionState::Departed {
                outbound_load_id: Some(outbound_load_id),
                load_sequence: Some(load_sequence),
            })
        }
        _ => Err(OutboundLoadError::InvalidCartonPositionTransition),
    }
}

fn phase_transition(progress: OutboundLoadProgress) -> OutboundLoadTransition {
    OutboundLoadTransition {
        progress,
        advances_load_revision: true,
    }
}

fn movement_transition(progress: OutboundLoadProgress) -> OutboundLoadTransition {
    OutboundLoadTransition {
        progress,
        advances_load_revision: false,
    }
}

fn require_status(
    actual: OutboundLoadStatus,
    expected: OutboundLoadStatus,
) -> Result<(), OutboundLoadError> {
    if actual == expected {
        Ok(())
    } else {
        Err(OutboundLoadError::InvalidStatus { actual })
    }
}

fn require_one_of(
    actual: OutboundLoadStatus,
    expected: &[OutboundLoadStatus],
) -> Result<(), OutboundLoadError> {
    if expected.contains(&actual) {
        Ok(())
    } else {
        Err(OutboundLoadError::InvalidStatus { actual })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundLoadCancellationReason {
    RouteCancelled,
    CarrierCancelled,
    EquipmentUnavailable,
    PlanningError,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct OutboundLoadCancellationNote(String);

impl OutboundLoadCancellationNote {
    pub fn new(value: impl Into<String>) -> Result<Self, OutboundLoadError> {
        validate_text(
            value.into(),
            "outbound load cancellation note",
            MAX_OUTBOUND_LOAD_CANCELLATION_NOTE_LENGTH,
        )
        .map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for OutboundLoadCancellationNote {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundLoadCancellationDetails {
    pub reason: OutboundLoadCancellationReason,
    pub note: Option<OutboundLoadCancellationNote>,
}

impl OutboundLoadCancellationDetails {
    pub fn new(
        reason: OutboundLoadCancellationReason,
        note: Option<OutboundLoadCancellationNote>,
    ) -> Result<Self, OutboundLoadError> {
        if reason == OutboundLoadCancellationReason::Other && note.is_none() {
            return Err(OutboundLoadError::CancellationNoteRequired);
        }
        Ok(Self { reason, note })
    }
}

macro_rules! outbound_text_value {
    ($name:ident, $field:literal, $maximum:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, OutboundLoadError> {
                validate_text(value.into(), $field, $maximum).map(Self)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl FromStr for $name {
            type Err = OutboundLoadError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }

        impl TryFrom<String> for $name {
            type Error = OutboundLoadError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(D::Error::custom)
            }
        }
    };
}

outbound_text_value!(
    OutboundLoadReference,
    "outbound load reference",
    MAX_OUTBOUND_LOAD_REFERENCE_LENGTH
);
outbound_text_value!(
    TrailerNumber,
    "trailer number",
    MAX_OUTBOUND_LOAD_TRAILER_NUMBER_LENGTH
);
outbound_text_value!(
    SealNumber,
    "seal number",
    MAX_OUTBOUND_LOAD_SEAL_NUMBER_LENGTH
);
outbound_text_value!(
    OutboundLoadScanValue,
    "outbound load scan value",
    MAX_OUTBOUND_LOAD_SCAN_VALUE_LENGTH
);

fn validate_text(
    value: String,
    field: &'static str,
    maximum: usize,
) -> Result<String, OutboundLoadError> {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(OutboundLoadError::InvalidText { field });
    }
    if value.chars().count() > maximum {
        return Err(OutboundLoadError::TextTooLong { field, maximum });
    }
    Ok(value)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutboundLoadError {
    #[error("outbound load plan requires at least one shipment and carton")]
    EmptyPlan,
    #[error("outbound load progress is inconsistent")]
    InvalidProgress,
    #[error("outbound load has status {actual}")]
    InvalidStatus { actual: OutboundLoadStatus },
    #[error("all planned cartons must be staged before loading starts")]
    IncompleteStaging,
    #[error("all planned cartons must be loaded before loading completes")]
    IncompleteLoading,
    #[error("packed carton is not staged")]
    CartonNotStaged,
    #[error("packed carton is not loaded")]
    CartonNotLoaded,
    #[error("every carton must be restored to its original packed position")]
    CartonsNotRestored,
    #[error("packed carton position transition is invalid")]
    InvalidCartonPositionTransition,
    #[error("load sequence must be positive")]
    InvalidLoadSequence,
    #[error("{field} must be a positive integer, got {value}")]
    InvalidRevision { field: &'static str, value: i64 },
    #[error("{field} is invalid")]
    InvalidText { field: &'static str },
    #[error("{field} exceeds {maximum} characters")]
    TextTooLong { field: &'static str, maximum: usize },
    #[error("cancellation reason other requires a note")]
    CancellationNoteRequired,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(value: i64) -> LocationId {
        LocationId::new(value).unwrap()
    }

    fn load() -> OutboundLoadId {
        OutboundLoadId::new(9).unwrap()
    }

    #[test]
    fn movement_commands_do_not_serialize_the_load_phase() {
        let progress = release_outbound_load(OutboundLoadProgress::planned(1, 2).unwrap())
            .unwrap()
            .progress;
        let first = record_outbound_carton_staged(progress).unwrap();
        assert!(!first.advances_load_revision);
        let second = record_outbound_carton_staged(first.progress).unwrap();
        assert!(!second.advances_load_revision);
        let loading = start_outbound_load_loading(second.progress).unwrap();
        assert!(loading.advances_load_revision);
    }

    #[test]
    fn ready_unload_reopens_once_and_position_recovery_is_exact() {
        let staged =
            OutboundLoadProgress::restore(1, 1, 1, 0, OutboundLoadStatus::Staging).unwrap();
        let loading = start_outbound_load_loading(staged).unwrap().progress;
        let loaded = record_outbound_carton_loaded(loading).unwrap().progress;
        let ready = complete_outbound_load_loading(loaded).unwrap().progress;
        let reopened = record_outbound_carton_unloaded(ready).unwrap();
        assert!(reopened.advances_load_revision);
        assert_eq!(reopened.progress.status(), OutboundLoadStatus::Loading);

        let packed = PackedCartonPositionState::Packed {
            location_id: location(1),
        };
        let staged = stage_packed_carton(packed, location(1), load(), location(2)).unwrap();
        let loaded = load_packed_carton(staged, load(), 1).unwrap();
        let staged = unload_packed_carton(loaded, load(), location(2)).unwrap();
        assert_eq!(
            unstage_packed_carton(staged, load(), location(1)).unwrap(),
            packed
        );
    }

    #[test]
    fn cancellation_requires_the_projection_and_every_original_position() {
        let progress =
            OutboundLoadProgress::restore(1, 2, 0, 0, OutboundLoadStatus::Loading).unwrap();
        assert!(cancel_outbound_load(progress, false).is_err());
        let cancelled = cancel_outbound_load(progress, true).unwrap();
        assert_eq!(cancelled.progress.status(), OutboundLoadStatus::Cancelled);
        assert!(cancelled.advances_load_revision);
    }

    #[test]
    fn bounded_values_and_other_cancellation_note_are_strict() {
        assert!(OutboundLoadReference::new(" LOAD-1").is_err());
        assert!(TrailerNumber::new("TRAILER-1").is_ok());
        assert!(
            OutboundLoadCancellationDetails::new(OutboundLoadCancellationReason::Other, None,)
                .is_err()
        );
    }
}
