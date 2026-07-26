use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

/// Positive revision used for optimistic concurrency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Revision(i64);

impl Revision {
    /// Validates a revision.
    pub const fn new(value: i64) -> Result<Self, RevisionError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(RevisionError(value))
        }
    }

    /// Returns the revision number.
    pub const fn get(self) -> i64 {
        self.0
    }

    /// Returns the next revision when the numeric range is not exhausted.
    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl TryFrom<i64> for Revision {
    type Error = RevisionError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for Revision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Revision validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("revision must be a positive integer, got {0}")]
pub struct RevisionError(i64);

/// Required expected revision for an optimistic write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionPrecondition {
    expected_revision: Revision,
}

impl RevisionPrecondition {
    /// Creates an optimistic revision precondition.
    pub const fn new(expected_revision: Revision) -> Self {
        Self { expected_revision }
    }

    /// Returns the revision the caller observed.
    pub const fn expected_revision(self) -> Revision {
        self.expected_revision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revisions_round_trip_as_positive_integers() {
        let revision = Revision::new(7).unwrap();
        assert_eq!(serde_json::to_string(&revision).unwrap(), "7");
        assert_eq!(serde_json::from_str::<Revision>("7").unwrap(), revision);
        assert_eq!(revision.checked_next(), Revision::new(8).ok());
    }

    #[test]
    fn revisions_reject_non_positive_numbers() {
        assert_eq!(Revision::new(0), Err(RevisionError(0)));
        assert_eq!(Revision::new(-1), Err(RevisionError(-1)));
        assert!(serde_json::from_str::<Revision>("0").is_err());
        assert!(serde_json::from_str::<Revision>("-1").is_err());
    }

    #[test]
    fn revision_preconditions_have_a_stable_shape() {
        let precondition = RevisionPrecondition::new(Revision::new(12).unwrap());
        let json = serde_json::to_string(&precondition).unwrap();

        assert_eq!(json, r#"{"expected_revision":12}"#);
        assert_eq!(
            serde_json::from_str::<RevisionPrecondition>(&json).unwrap(),
            precondition
        );
        assert!(serde_json::from_str::<RevisionPrecondition>(
            r#"{"expected_revision":12,"force":true}"#
        )
        .is_err());
    }
}
