use serde::{Deserialize, Deserializer, Serialize};

pub const MAX_OUTBOX_DEAD_LETTER_DISCARD_REASON_LENGTH: usize = 1_000;

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
}
