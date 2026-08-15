use chrono::Duration;
use serde::{Deserialize, Serialize};

use crate::Timestamp;

pub const MAX_YARD_CODE_LENGTH: usize = 80;
pub const MAX_YARD_NAME_LENGTH: usize = 160;
pub const MAX_YARD_NOTE_LENGTH: usize = 500;
pub const MAX_YARD_FREE_MINUTES: u32 = 10_080;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum YardError {
    #[error("{field} must be nonblank, trimmed, and control-free")]
    InvalidText { field: &'static str },
    #[error("{field} cannot exceed {maximum} characters")]
    TextTooLong { field: &'static str, maximum: usize },
    #[error("appointment end must be after its start")]
    InvalidAppointmentWindow,
    #[error("yard free time cannot exceed {MAX_YARD_FREE_MINUTES} minutes")]
    InvalidFreeMinutes,
    #[error("yard revision must be positive, got {value}")]
    InvalidRevision { value: i64 },
    #[error("yard revision cannot advance beyond its supported range")]
    RevisionExhausted,
    #[error("appointment transition from {from:?} to {to:?} is not allowed")]
    InvalidAppointmentTransition {
        from: YardAppointmentStatus,
        to: YardAppointmentStatus,
    },
    #[error("yard visit transition from {from:?} to {to:?} is not allowed")]
    InvalidVisitTransition {
        from: YardVisitStatus,
        to: YardVisitStatus,
    },
    #[error("{operation:?} does not match a {direction:?} visit")]
    DirectionOperationMismatch {
        direction: YardDirection,
        operation: YardOperation,
    },
    #[error("gate-out time cannot precede gate-in time")]
    InvalidGateInterval,
}

fn required_text(
    value: impl Into<String>,
    field: &'static str,
    maximum: usize,
) -> Result<String, YardError> {
    let value = value.into();
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(YardError::InvalidText { field });
    }
    if value.chars().count() > maximum {
        return Err(YardError::TextTooLong { field, maximum });
    }
    Ok(value)
}

macro_rules! yard_text {
    ($name:ident, $field:literal, $maximum:expr) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, YardError> {
                required_text(value, $field, $maximum).map(Self)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

yard_text!(
    YardAppointmentNumber,
    "appointment number",
    MAX_YARD_CODE_LENGTH
);
yard_text!(YardAssetNumber, "asset number", MAX_YARD_CODE_LENGTH);
yard_text!(YardLocationCode, "yard location code", MAX_YARD_CODE_LENGTH);
yard_text!(YardName, "yard name", MAX_YARD_NAME_LENGTH);
yard_text!(YardNote, "yard note", MAX_YARD_NOTE_LENGTH);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct YardRevision(i64);

impl YardRevision {
    pub const fn new(value: i64) -> Result<Self, YardError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(YardError::InvalidRevision { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    pub const fn next(self) -> Result<Self, YardError> {
        match self.0.checked_add(1) {
            Some(value) => Ok(Self(value)),
            None => Err(YardError::RevisionExhausted),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct YardAppointmentWindow {
    pub scheduled_from: Timestamp,
    pub scheduled_until: Timestamp,
}

impl YardAppointmentWindow {
    pub fn new(scheduled_from: Timestamp, scheduled_until: Timestamp) -> Result<Self, YardError> {
        if scheduled_until <= scheduled_from {
            return Err(YardError::InvalidAppointmentWindow);
        }
        Ok(Self {
            scheduled_from,
            scheduled_until,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct YardFreeMinutes(u32);

impl YardFreeMinutes {
    pub const fn new(value: u32) -> Result<Self, YardError> {
        if value <= MAX_YARD_FREE_MINUTES {
            Ok(Self(value))
        } else {
            Err(YardError::InvalidFreeMinutes)
        }
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YardDirection {
    Inbound,
    Outbound,
}

impl YardDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inbound => "inbound",
            Self::Outbound => "outbound",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "inbound" => Some(Self::Inbound),
            "outbound" => Some(Self::Outbound),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YardAssetKind {
    Trailer,
    Container,
}

impl YardAssetKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trailer => "trailer",
            Self::Container => "container",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "trailer" => Some(Self::Trailer),
            "container" => Some(Self::Container),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YardLocationKind {
    Gate,
    Parking,
    DockDoor,
    Inspection,
    Staging,
}

impl YardLocationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gate => "gate",
            Self::Parking => "parking",
            Self::DockDoor => "dock_door",
            Self::Inspection => "inspection",
            Self::Staging => "staging",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "gate" => Some(Self::Gate),
            "parking" => Some(Self::Parking),
            "dock_door" => Some(Self::DockDoor),
            "inspection" => Some(Self::Inspection),
            "staging" => Some(Self::Staging),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YardAppointmentStatus {
    Scheduled,
    CheckedIn,
    Completed,
    Cancelled,
    NoShow,
}

impl YardAppointmentStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::CheckedIn => "checked_in",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::NoShow => "no_show",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "scheduled" => Some(Self::Scheduled),
            "checked_in" => Some(Self::CheckedIn),
            "completed" => Some(Self::Completed),
            "cancelled" => Some(Self::Cancelled),
            "no_show" => Some(Self::NoShow),
            _ => None,
        }
    }

    pub fn transition(self, to: Self) -> Result<Self, YardError> {
        let valid = matches!(
            (self, to),
            (
                Self::Scheduled,
                Self::CheckedIn | Self::Cancelled | Self::NoShow
            ) | (Self::CheckedIn, Self::Completed)
        );
        if valid {
            Ok(to)
        } else {
            Err(YardError::InvalidAppointmentTransition { from: self, to })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YardVisitStatus {
    GatedIn,
    InYard,
    AtDoor,
    Loading,
    Unloading,
    ReadyToDepart,
    Rejected,
    GatedOut,
}

impl YardVisitStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GatedIn => "gated_in",
            Self::InYard => "in_yard",
            Self::AtDoor => "at_door",
            Self::Loading => "loading",
            Self::Unloading => "unloading",
            Self::ReadyToDepart => "ready_to_depart",
            Self::Rejected => "rejected",
            Self::GatedOut => "gated_out",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "gated_in" => Some(Self::GatedIn),
            "in_yard" => Some(Self::InYard),
            "at_door" => Some(Self::AtDoor),
            "loading" => Some(Self::Loading),
            "unloading" => Some(Self::Unloading),
            "ready_to_depart" => Some(Self::ReadyToDepart),
            "rejected" => Some(Self::Rejected),
            "gated_out" => Some(Self::GatedOut),
            _ => None,
        }
    }

    pub fn spot(self) -> Result<Self, YardError> {
        match self {
            Self::GatedIn | Self::InYard => Ok(Self::InYard),
            _ => Err(YardError::InvalidVisitTransition {
                from: self,
                to: Self::InYard,
            }),
        }
    }

    pub fn assign_door(self) -> Result<Self, YardError> {
        match self {
            Self::GatedIn | Self::InYard => Ok(Self::AtDoor),
            _ => Err(YardError::InvalidVisitTransition {
                from: self,
                to: Self::AtDoor,
            }),
        }
    }

    pub fn begin_operation(
        self,
        direction: YardDirection,
        operation: YardOperation,
    ) -> Result<Self, YardError> {
        if operation.for_direction() != direction {
            return Err(YardError::DirectionOperationMismatch {
                direction,
                operation,
            });
        }
        let target = operation.active_status();
        if self == Self::AtDoor {
            Ok(target)
        } else {
            Err(YardError::InvalidVisitTransition {
                from: self,
                to: target,
            })
        }
    }

    pub fn complete_operation(self) -> Result<Self, YardError> {
        match self {
            Self::Loading | Self::Unloading => Ok(Self::ReadyToDepart),
            _ => Err(YardError::InvalidVisitTransition {
                from: self,
                to: Self::ReadyToDepart,
            }),
        }
    }

    pub fn reject(self) -> Result<Self, YardError> {
        match self {
            Self::GatedIn | Self::InYard => Ok(Self::Rejected),
            _ => Err(YardError::InvalidVisitTransition {
                from: self,
                to: Self::Rejected,
            }),
        }
    }

    pub fn gate_out(self) -> Result<Self, YardError> {
        match self {
            Self::ReadyToDepart | Self::Rejected => Ok(Self::GatedOut),
            _ => Err(YardError::InvalidVisitTransition {
                from: self,
                to: Self::GatedOut,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YardOperation {
    Loading,
    Unloading,
}

impl YardOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Loading => "loading",
            Self::Unloading => "unloading",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "loading" => Some(Self::Loading),
            "unloading" => Some(Self::Unloading),
            _ => None,
        }
    }

    pub const fn for_direction(self) -> YardDirection {
        match self {
            Self::Loading => YardDirection::Outbound,
            Self::Unloading => YardDirection::Inbound,
        }
    }

    const fn active_status(self) -> YardVisitStatus {
        match self {
            Self::Loading => YardVisitStatus::Loading,
            Self::Unloading => YardVisitStatus::Unloading,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct YardDetention {
    pub total_minutes: u64,
    pub free_minutes: u32,
    pub detention_minutes: u64,
    pub billable_hours: u64,
}

pub fn calculate_yard_detention(
    gated_in_at: Timestamp,
    gated_out_at: Timestamp,
    free_minutes: YardFreeMinutes,
) -> Result<YardDetention, YardError> {
    let elapsed = gated_out_at.signed_duration_since(gated_in_at);
    if elapsed < Duration::zero() {
        return Err(YardError::InvalidGateInterval);
    }
    let total_minutes = u64::try_from(elapsed.num_minutes()).unwrap_or(u64::MAX);
    let detention_minutes = total_minutes.saturating_sub(u64::from(free_minutes.get()));
    let billable_hours = detention_minutes.saturating_add(59) / 60;
    Ok(YardDetention {
        total_minutes,
        free_minutes: free_minutes.get(),
        detention_minutes,
        billable_hours,
    })
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[test]
    fn direction_controls_the_dock_operation() {
        assert_eq!(
            YardVisitStatus::AtDoor
                .begin_operation(YardDirection::Inbound, YardOperation::Unloading)
                .unwrap(),
            YardVisitStatus::Unloading
        );
        assert!(matches!(
            YardVisitStatus::AtDoor.begin_operation(YardDirection::Inbound, YardOperation::Loading),
            Err(YardError::DirectionOperationMismatch { .. })
        ));
    }

    #[test]
    fn visit_requires_the_recoverable_execution_sequence() {
        assert!(YardVisitStatus::GatedIn.gate_out().is_err());
        let at_door = YardVisitStatus::GatedIn.assign_door().unwrap();
        let unloading = at_door
            .begin_operation(YardDirection::Inbound, YardOperation::Unloading)
            .unwrap();
        let ready = unloading.complete_operation().unwrap();
        assert_eq!(ready.gate_out().unwrap(), YardVisitStatus::GatedOut);
    }

    #[test]
    fn detention_uses_completed_minutes_and_rounds_billable_hours_up() {
        let entered = Utc.with_ymd_and_hms(2026, 8, 1, 8, 0, 0).unwrap();
        let exited = Utc.with_ymd_and_hms(2026, 8, 1, 10, 31, 0).unwrap();
        let detention =
            calculate_yard_detention(entered, exited, YardFreeMinutes::new(120).unwrap()).unwrap();
        assert_eq!(detention.total_minutes, 151);
        assert_eq!(detention.detention_minutes, 31);
        assert_eq!(detention.billable_hours, 1);
    }

    #[test]
    fn terminal_appointment_states_cannot_be_reopened() {
        assert!(YardAppointmentStatus::Cancelled
            .transition(YardAppointmentStatus::Scheduled)
            .is_err());
        assert_eq!(
            YardAppointmentStatus::Scheduled
                .transition(YardAppointmentStatus::CheckedIn)
                .unwrap()
                .transition(YardAppointmentStatus::Completed)
                .unwrap(),
            YardAppointmentStatus::Completed
        );
    }
}
