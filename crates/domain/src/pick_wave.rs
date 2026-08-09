use std::collections::HashSet;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{OrderId, OrderRevision};

pub const MAX_PICK_WAVE_NAME_LENGTH: usize = 100;
pub const MAX_PICK_WAVE_CANCELLATION_NOTE_LENGTH: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickWaveStatus {
    Planned,
    Released,
    Cancelled,
}

impl PickWaveStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Released => "released",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "planned" => Some(Self::Planned),
            "released" => Some(Self::Released),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickWaveCancellationReason {
    OperationalChange,
    CapacityConstraint,
    OrderChange,
    Other,
}

impl PickWaveCancellationReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OperationalChange => "operational_change",
            Self::CapacityConstraint => "capacity_constraint",
            Self::OrderChange => "order_change",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "operational_change" => Some(Self::OperationalChange),
            "capacity_constraint" => Some(Self::CapacityConstraint),
            "order_change" => Some(Self::OrderChange),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

macro_rules! bounded_text {
    ($name:ident, $max:expr, $error:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, PickWaveError> {
                let value = value.into();
                if value.is_empty()
                    || value.trim() != value
                    || value.chars().count() > $max
                    || value.chars().any(char::is_control)
                {
                    return Err(PickWaveError::$error);
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
            }
        }
    };
}

bounded_text!(PickWaveName, MAX_PICK_WAVE_NAME_LENGTH, InvalidName);
bounded_text!(
    PickWaveCancellationNote,
    MAX_PICK_WAVE_CANCELLATION_NOTE_LENGTH,
    InvalidCancellationNote
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct PickWaveRevision(i64);

impl PickWaveRevision {
    pub const fn new(value: i64) -> Result<Self, PickWaveError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(PickWaveError::InvalidRevision { value })
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

impl<'de> Deserialize<'de> for PickWaveRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickWaveOrderPrecondition {
    pub order_id: OrderId,
    pub expected_revision: OrderRevision,
    pub sequence: u32,
}

pub fn validate_pick_wave_plan(orders: &[PickWaveOrderPrecondition]) -> Result<(), PickWaveError> {
    if orders.is_empty() {
        return Err(PickWaveError::EmptyPlan);
    }
    let mut order_ids = HashSet::with_capacity(orders.len());
    let mut sequences = HashSet::with_capacity(orders.len());
    for order in orders {
        if order.sequence == 0
            || !order_ids.insert(order.order_id)
            || !sequences.insert(order.sequence)
        {
            return Err(PickWaveError::InvalidOrderSet);
        }
    }
    if sequences.len() != orders.len()
        || !(1..=u32::try_from(orders.len()).map_err(|_| PickWaveError::PlanTooLarge)?)
            .all(|sequence| sequences.contains(&sequence))
    {
        return Err(PickWaveError::InvalidOrderSet);
    }
    Ok(())
}

pub fn release_pick_wave(
    status: PickWaveStatus,
    revision: PickWaveRevision,
) -> Result<PickWaveRevision, PickWaveError> {
    if status != PickWaveStatus::Planned {
        return Err(PickWaveError::InvalidTransition { status });
    }
    revision
        .checked_next()
        .ok_or(PickWaveError::RevisionOverflow)
}

pub fn cancel_pick_wave(
    status: PickWaveStatus,
    revision: PickWaveRevision,
    reason: PickWaveCancellationReason,
    note: Option<&PickWaveCancellationNote>,
) -> Result<PickWaveRevision, PickWaveError> {
    if status != PickWaveStatus::Planned {
        return Err(PickWaveError::InvalidTransition { status });
    }
    if reason == PickWaveCancellationReason::Other && note.is_none() {
        return Err(PickWaveError::OtherRequiresNote);
    }
    revision
        .checked_next()
        .ok_or(PickWaveError::RevisionOverflow)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PickWaveError {
    #[error("pick wave name must be trimmed, printable, and at most 100 characters")]
    InvalidName,
    #[error("pick wave cancellation note must be trimmed, printable, and at most 500 characters")]
    InvalidCancellationNote,
    #[error("pick wave revision must be positive, got {value}")]
    InvalidRevision { value: i64 },
    #[error("pick wave revision exceeds supported range")]
    RevisionOverflow,
    #[error("pick wave must contain at least one order")]
    EmptyPlan,
    #[error("pick wave orders and sequences must be unique and contiguous from one")]
    InvalidOrderSet,
    #[error("pick wave contains too many orders")]
    PlanTooLarge,
    #[error("pick wave status {status:?} does not allow this transition")]
    InvalidTransition { status: PickWaveStatus },
    #[error("pick wave cancellation reason Other requires a note")]
    OtherRequiresNote,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(order_id: i64, sequence: u32) -> PickWaveOrderPrecondition {
        PickWaveOrderPrecondition {
            order_id: OrderId::new(order_id).unwrap(),
            expected_revision: OrderRevision::new(2).unwrap(),
            sequence,
        }
    }

    #[test]
    fn plan_requires_unique_contiguous_membership() {
        assert!(validate_pick_wave_plan(&[member(1, 1), member(2, 2)]).is_ok());
        assert_eq!(
            validate_pick_wave_plan(&[member(1, 1), member(1, 2)]),
            Err(PickWaveError::InvalidOrderSet)
        );
        assert_eq!(
            validate_pick_wave_plan(&[member(1, 2)]),
            Err(PickWaveError::InvalidOrderSet)
        );
    }

    #[test]
    fn lifecycle_is_revisioned_and_cancellation_note_is_typed() {
        let revision = PickWaveRevision::new(1).unwrap();
        assert_eq!(
            release_pick_wave(PickWaveStatus::Planned, revision)
                .unwrap()
                .get(),
            2
        );
        assert_eq!(
            cancel_pick_wave(
                PickWaveStatus::Planned,
                revision,
                PickWaveCancellationReason::Other,
                None
            ),
            Err(PickWaveError::OtherRequiresNote)
        );
    }
}
