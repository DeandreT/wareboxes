use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

/// Canonical HTTP header carrying an idempotency key.
pub const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";
/// Maximum accepted idempotency-key length.
pub const MAX_IDEMPOTENCY_KEY_LENGTH: usize = 200;

/// Validated identity for one replay-safe command.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Validates an idempotency key.
    pub fn new(value: impl Into<String>) -> Result<Self, IdempotencyKeyError> {
        let value = value.into();
        if value.is_empty() {
            return Err(IdempotencyKeyError::Empty);
        }
        if value.len() > MAX_IDEMPOTENCY_KEY_LENGTH {
            return Err(IdempotencyKeyError::TooLong);
        }
        if !value.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(IdempotencyKeyError::InvalidCharacter);
        }
        Ok(Self(value))
    }

    /// Returns the key.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the value and returns the key.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for IdempotencyKey {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for IdempotencyKey {
    type Err = IdempotencyKeyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for IdempotencyKey {
    type Error = IdempotencyKeyError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for IdempotencyKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Idempotency-key validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdempotencyKeyError {
    #[error("idempotency key cannot be empty")]
    Empty,
    #[error("idempotency key cannot exceed {MAX_IDEMPOTENCY_KEY_LENGTH} bytes")]
    TooLong,
    #[error("idempotency key must contain only visible ASCII characters")]
    InvalidCharacter,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idempotency_keys_round_trip_as_strings() {
        let key = IdempotencyKey::new("receive:device-7:command-42").unwrap();
        let json = serde_json::to_string(&key).unwrap();

        assert_eq!(json, r#""receive:device-7:command-42""#);
        assert_eq!(serde_json::from_str::<IdempotencyKey>(&json).unwrap(), key);
    }

    #[test]
    fn idempotency_keys_reject_ambiguous_values() {
        assert_eq!(IdempotencyKey::new(""), Err(IdempotencyKeyError::Empty));
        assert_eq!(
            IdempotencyKey::new("has spaces"),
            Err(IdempotencyKeyError::InvalidCharacter)
        );
        assert_eq!(
            IdempotencyKey::new("x".repeat(MAX_IDEMPOTENCY_KEY_LENGTH + 1)),
            Err(IdempotencyKeyError::TooLong)
        );
        assert!(serde_json::from_str::<IdempotencyKey>(r#""line\nbreak""#).is_err());
    }
}
