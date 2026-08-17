use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_CARRIER_ACCOUNT_KEY_LENGTH: usize = 200;
pub const MAX_CARRIER_ACCOUNT_NAME_LENGTH: usize = 200;
pub const MAX_CARRIER_FAILURE_CODE_LENGTH: usize = 100;
pub const MAX_CARRIER_FAILURE_MESSAGE_LENGTH: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CarrierAccountStatus {
    Active,
    Disabled,
}

impl CarrierAccountStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CarrierManifestJobStatus {
    Queued,
    Processing,
    RetryScheduled,
    Succeeded,
    Failed,
    Cancelled,
}

impl CarrierManifestJobStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Processing => "processing",
            Self::RetryScheduled => "retry_scheduled",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CarrierAccountKey(String);

impl CarrierAccountKey {
    pub fn new(value: impl Into<String>) -> Result<Self, CarrierError> {
        bounded_text(
            value.into(),
            "carrier account key",
            MAX_CARRIER_ACCOUNT_KEY_LENGTH,
        )
        .map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CarrierAccountName(String);

impl CarrierAccountName {
    pub fn new(value: impl Into<String>) -> Result<Self, CarrierError> {
        bounded_text(
            value.into(),
            "carrier account name",
            MAX_CARRIER_ACCOUNT_NAME_LENGTH,
        )
        .map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CarrierFailureCode(String);

impl CarrierFailureCode {
    pub fn new(value: impl Into<String>) -> Result<Self, CarrierError> {
        bounded_text(
            value.into(),
            "carrier failure code",
            MAX_CARRIER_FAILURE_CODE_LENGTH,
        )
        .map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CarrierFailureMessage(String);

impl CarrierFailureMessage {
    pub fn new(value: impl Into<String>) -> Result<Self, CarrierError> {
        bounded_text(
            value.into(),
            "carrier failure message",
            MAX_CARRIER_FAILURE_MESSAGE_LENGTH,
        )
        .map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CarrierError {
    #[error("{field} must be trimmed, nonempty, printable text no longer than {max} characters")]
    InvalidText { field: &'static str, max: usize },
}

fn bounded_text(value: String, field: &'static str, max: usize) -> Result<String, CarrierError> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().count() > max
        || value.chars().any(char::is_control)
    {
        return Err(CarrierError::InvalidText { field, max });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_values_are_bounded_and_exact() {
        assert_eq!(
            CarrierAccountKey::new("gateway-account").unwrap().as_str(),
            "gateway-account"
        );
        assert!(CarrierAccountKey::new(" gateway-account").is_err());
        assert!(CarrierAccountName::new("\n").is_err());
    }

    #[test]
    fn manifest_job_terminal_states_are_explicit() {
        assert!(!CarrierManifestJobStatus::RetryScheduled.is_terminal());
        assert!(CarrierManifestJobStatus::Succeeded.is_terminal());
        assert!(CarrierManifestJobStatus::Failed.is_terminal());
    }
}
