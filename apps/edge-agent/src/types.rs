use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_IDENTIFIER_LENGTH: usize = 128;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TypeError {
    #[error("{field} must contain between 1 and {max} characters")]
    InvalidLength { field: &'static str, max: usize },
    #[error("{field} contains unsupported characters")]
    InvalidCharacters { field: &'static str },
    #[error("device display name must contain between 1 and 200 characters")]
    InvalidDisplayName,
    #[error("operator reason must contain between 1 and 1,000 characters")]
    InvalidReason,
}

fn validate_identifier(
    value: String,
    field: &'static str,
    max: usize,
) -> Result<String, TypeError> {
    let value = value.trim().to_owned();
    if value.is_empty() || value.len() > max {
        return Err(TypeError::InvalidLength { field, max });
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
    }) {
        return Err(TypeError::InvalidCharacters { field });
    }
    Ok(value)
}

macro_rules! identifier {
    ($name:ident, $field:literal, $max:expr) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, TypeError> {
                validate_identifier(value.into(), $field, $max).map(Self)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = TypeError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }

        impl TryFrom<String> for $name {
            type Error = TypeError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

identifier!(TenantId, "tenant ID", MAX_IDENTIFIER_LENGTH);
identifier!(FacilityId, "facility ID", MAX_IDENTIFIER_LENGTH);
identifier!(DeviceId, "device ID", MAX_IDENTIFIER_LENGTH);
identifier!(CommandId, "command ID", MAX_IDENTIFIER_LENGTH);
identifier!(CorrelationId, "correlation ID", MAX_IDENTIFIER_LENGTH);
identifier!(ActorId, "actor ID", MAX_IDENTIFIER_LENGTH);
identifier!(IdempotencyKey, "idempotency key", 200);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceClass {
    Plc,
    Conveyor,
    Robotics,
    Sortation,
    Printer,
    Scale,
}

impl DeviceClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plc => "plc",
            Self::Conveyor => "conveyor",
            Self::Robotics => "robotics",
            Self::Sortation => "sortation",
            Self::Printer => "printer",
            Self::Scale => "scale",
        }
    }

    pub(crate) fn parse_storage(value: &str) -> Result<Self, TypeError> {
        match value {
            "plc" => Ok(Self::Plc),
            "conveyor" => Ok(Self::Conveyor),
            "robotics" => Ok(Self::Robotics),
            "sortation" => Ok(Self::Sortation),
            "printer" => Ok(Self::Printer),
            "scale" => Ok(Self::Scale),
            _ => Err(TypeError::InvalidCharacters {
                field: "device class",
            }),
        }
    }
}

impl FromStr for DeviceClass {
    type Err = TypeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse_storage(value.trim().to_ascii_lowercase().as_str())
    }
}

impl fmt::Display for DeviceClass {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceDescriptor {
    pub tenant_id: TenantId,
    pub facility_id: FacilityId,
    pub device_id: DeviceId,
    pub class: DeviceClass,
    pub display_name: String,
}

impl DeviceDescriptor {
    pub fn validate(&self) -> Result<(), TypeError> {
        let trimmed = self.display_name.trim();
        if trimmed.is_empty() || trimmed.len() > 200 {
            return Err(TypeError::InvalidDisplayName);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlMode {
    Disabled,
    Automatic,
    ManualFallback,
}

impl ControlMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Automatic => "automatic",
            Self::ManualFallback => "manual_fallback",
        }
    }

    pub(crate) fn parse_storage(value: &str) -> Result<Self, TypeError> {
        match value {
            "disabled" => Ok(Self::Disabled),
            "automatic" => Ok(Self::Automatic),
            "manual_fallback" => Ok(Self::ManualFallback),
            _ => Err(TypeError::InvalidCharacters {
                field: "control mode",
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Unknown,
    Healthy,
    Degraded,
    Offline,
    Faulted,
}

impl HealthState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Offline => "offline",
            Self::Faulted => "faulted",
        }
    }

    pub(crate) fn parse_storage(value: &str) -> Result<Self, TypeError> {
        match value {
            "unknown" => Ok(Self::Unknown),
            "healthy" => Ok(Self::Healthy),
            "degraded" => Ok(Self::Degraded),
            "offline" => Ok(Self::Offline),
            "faulted" => Ok(Self::Faulted),
            _ => Err(TypeError::InvalidCharacters {
                field: "health state",
            }),
        }
    }

    pub const fn permits_automatic_work(self) -> bool {
        matches!(self, Self::Healthy | Self::Degraded)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SafetyConfirmation(());

impl SafetyConfirmation {
    /// Construct only after the local operator has completed the physical safety
    /// checklist and confirmed that quarantined commands were reconciled.
    pub const fn after_physical_safety_checklist() -> Self {
        Self(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlAction {
    Disable,
    EnterManualFallback,
    ResumeAutomation(SafetyConfirmation),
}

impl ControlAction {
    pub const fn target_mode(self) -> ControlMode {
        match self {
            Self::Disable => ControlMode::Disabled,
            Self::EnterManualFallback => ControlMode::ManualFallback,
            Self::ResumeAutomation(_) => ControlMode::Automatic,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceStatus {
    pub descriptor: DeviceDescriptor,
    pub control_mode: ControlMode,
    pub control_reason: String,
    pub control_actor: ActorId,
    pub control_changed_at: DateTime<Utc>,
    pub health: HealthState,
    pub health_message: Option<String>,
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    pub consecutive_health_failures: u32,
}

pub(crate) fn validate_reason(reason: &str) -> Result<String, TypeError> {
    let reason = reason.trim();
    if reason.is_empty() || reason.len() > 1_000 {
        return Err(TypeError::InvalidReason);
    }
    Ok(reason.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_trimmed_and_reject_unsafe_characters() {
        assert_eq!(DeviceId::new("  sorter-01 ").unwrap().as_str(), "sorter-01");
        assert!(DeviceId::new("sorter 01").is_err());
        assert!(DeviceId::new("").is_err());
    }

    #[test]
    fn health_only_permits_explicitly_operational_states() {
        assert!(HealthState::Healthy.permits_automatic_work());
        assert!(HealthState::Degraded.permits_automatic_work());
        assert!(!HealthState::Unknown.permits_automatic_work());
        assert!(!HealthState::Offline.permits_automatic_work());
        assert!(!HealthState::Faulted.permits_automatic_work());
    }
}
