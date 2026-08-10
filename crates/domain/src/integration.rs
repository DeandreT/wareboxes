use serde::{Deserialize, Deserializer, Serialize};

pub const MAX_OUTBOX_DEAD_LETTER_DISCARD_REASON_LENGTH: usize = 1_000;
pub const MAX_INTEGRATION_PROCESSING_ERROR_CODE_LENGTH: usize = 100;
pub const MAX_INTEGRATION_PROCESSING_ERROR_MESSAGE_LENGTH: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationInboxProcessingStatus {
    Quarantined,
    Processed,
}

impl IntegrationInboxProcessingStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Quarantined => "quarantined",
            Self::Processed => "processed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "quarantined" => Some(Self::Quarantined),
            "processed" => Some(Self::Processed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IntegrationInboxProcessingRevision(i64);

impl IntegrationInboxProcessingRevision {
    pub fn new(value: i64) -> Result<Self, &'static str> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err("integration inbox processing revision must be positive")
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutboxDeadLetterDiscardReasonError {
    #[error("discard reason must be trimmed, nonempty, and control-free")]
    Invalid,
    #[error(
        "discard reason cannot exceed {MAX_OUTBOX_DEAD_LETTER_DISCARD_REASON_LENGTH} characters"
    )]
    TooLong,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct OutboxDeadLetterDiscardReason(String);

impl OutboxDeadLetterDiscardReason {
    pub fn new(value: impl Into<String>) -> Result<Self, OutboxDeadLetterDiscardReasonError> {
        let value = value.into();
        if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
            return Err(OutboxDeadLetterDiscardReasonError::Invalid);
        }
        if value.chars().count() > MAX_OUTBOX_DEAD_LETTER_DISCARD_REASON_LENGTH {
            return Err(OutboxDeadLetterDiscardReasonError::TooLong);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for OutboxDeadLetterDiscardReason {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discard_reason_is_bounded_and_operator_safe() {
        assert!(OutboxDeadLetterDiscardReason::new("partner endpoint retired").is_ok());
        assert!(OutboxDeadLetterDiscardReason::new(" partner endpoint retired").is_err());
        assert!(OutboxDeadLetterDiscardReason::new("contains\ncontrol").is_err());
        assert!(OutboxDeadLetterDiscardReason::new(
            "x".repeat(MAX_OUTBOX_DEAD_LETTER_DISCARD_REASON_LENGTH + 1)
        )
        .is_err());
    }

    #[test]
    fn inbox_processing_status_and_revision_are_strict() {
        assert_eq!(
            IntegrationInboxProcessingStatus::parse("quarantined"),
            Some(IntegrationInboxProcessingStatus::Quarantined)
        );
        assert_eq!(
            IntegrationInboxProcessingStatus::Processed.as_str(),
            "processed"
        );
        assert!(IntegrationInboxProcessingStatus::parse("received").is_none());
        assert_eq!(IntegrationInboxProcessingRevision::new(2).unwrap().get(), 2);
        assert!(IntegrationInboxProcessingRevision::new(0).is_err());
    }
}
