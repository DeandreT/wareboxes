use serde::{Deserialize, Serialize};
use std::fmt;

pub const MAX_TENANT_NAME_LENGTH: usize = 200;
pub const MAX_TENANT_SLUG_LENGTH: usize = 63;
pub const MAX_TENANT_LIFECYCLE_REASON_LENGTH: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TenantLifecycleError {
    #[error("{field} must not be blank")]
    Blank { field: &'static str },
    #[error("{field} must not contain control characters")]
    ControlCharacter { field: &'static str },
    #[error("{field} exceeds {max} characters")]
    TooLong { field: &'static str, max: usize },
    #[error("tenant slug must be 3 through 63 lowercase letters, digits, or hyphens and cannot start or end with a hyphen")]
    InvalidSlug,
    #[error("tenant revision must be a positive integer")]
    InvalidRevision,
    #[error("tenant cannot transition from {from} to {to}")]
    InvalidTransition {
        from: TenantStatus,
        to: TenantStatus,
    },
}

fn validate_text(
    value: String,
    field: &'static str,
    max: usize,
) -> Result<String, TenantLifecycleError> {
    if value.trim().is_empty() || value.trim() != value {
        return Err(TenantLifecycleError::Blank { field });
    }
    if value.chars().any(char::is_control) {
        return Err(TenantLifecycleError::ControlCharacter { field });
    }
    if value.chars().count() > max {
        return Err(TenantLifecycleError::TooLong { field, max });
    }
    Ok(value)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TenantName(String);

impl TenantName {
    pub fn new(value: impl Into<String>) -> Result<Self, TenantLifecycleError> {
        validate_text(value.into(), "tenant name", MAX_TENANT_NAME_LENGTH).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TenantSlug(String);

impl TenantSlug {
    pub fn new(value: impl Into<String>) -> Result<Self, TenantLifecycleError> {
        let value = value.into();
        if !(3..=MAX_TENANT_SLUG_LENGTH).contains(&value.len())
            || value.starts_with('-')
            || value.ends_with('-')
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(TenantLifecycleError::InvalidSlug);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TenantLifecycleReason(String);

impl TenantLifecycleReason {
    pub fn new(value: impl Into<String>) -> Result<Self, TenantLifecycleError> {
        validate_text(
            value.into(),
            "tenant lifecycle reason",
            MAX_TENANT_LIFECYCLE_REASON_LENGTH,
        )
        .map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TenantRevision(i64);

impl TenantRevision {
    pub fn new(value: i64) -> Result<Self, TenantLifecycleError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(TenantLifecycleError::InvalidRevision)
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    pub fn checked_next(self) -> Option<Self> {
        self.0
            .checked_add(1)
            .and_then(|value| Self::new(value).ok())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TenantStatus {
    #[default]
    Active,
    Suspended,
}

impl TenantStatus {
    pub const ALL: [Self; 2] = [Self::Active, Self::Suspended];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "suspended" => Some(Self::Suspended),
            _ => None,
        }
    }

    pub fn require_transition(self, next: Self) -> Result<(), TenantLifecycleError> {
        if matches!(
            (self, next),
            (Self::Active, Self::Suspended) | (Self::Suspended, Self::Active)
        ) {
            Ok(())
        } else {
            Err(TenantLifecycleError::InvalidTransition {
                from: self,
                to: next,
            })
        }
    }
}

impl fmt::Display for TenantStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::{TenantLifecycleReason, TenantName, TenantRevision, TenantSlug, TenantStatus};

    #[test]
    fn every_status_round_trips_through_its_persisted_value() {
        let cases = [
            (TenantStatus::Active, "active"),
            (TenantStatus::Suspended, "suspended"),
        ];

        assert_eq!(
            cases.map(|(status, _)| status),
            TenantStatus::ALL,
            "the test cases must cover every tenant status"
        );

        for (status, persisted_value) in cases {
            assert_eq!(status.as_str(), persisted_value);
            assert_eq!(status.to_string(), persisted_value);
            assert_eq!(TenantStatus::parse(persisted_value), Some(status));
        }
    }

    #[test]
    fn parsing_rejects_values_outside_the_domain_vocabulary() {
        for value in ["", "ACTIVE", "deleted", " active", "active "] {
            assert_eq!(TenantStatus::parse(value), None);
        }
    }

    #[test]
    fn active_is_the_default_status() {
        assert_eq!(TenantStatus::default(), TenantStatus::Active);
    }

    #[test]
    fn lifecycle_values_and_transitions_are_exact() {
        assert!(TenantName::new("Northwest operations").is_ok());
        assert!(TenantName::new(" North ").is_err());
        assert!(TenantSlug::new("northwest-3pl").is_ok());
        assert!(TenantSlug::new("Northwest").is_err());
        assert!(TenantSlug::new("-northwest").is_err());
        assert!(TenantLifecycleReason::new("contract placed on hold").is_ok());
        assert!(TenantRevision::new(0).is_err());
        assert_eq!(
            TenantRevision::new(1)
                .unwrap()
                .checked_next()
                .unwrap()
                .get(),
            2
        );
        assert_eq!(
            TenantStatus::Active.require_transition(TenantStatus::Suspended),
            Ok(())
        );
        assert!(TenantStatus::Active
            .require_transition(TenantStatus::Active)
            .is_err());
    }
}
