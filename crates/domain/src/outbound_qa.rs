use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

pub const MAX_OUTBOUND_QA_SCAN_VALUE_LENGTH: usize = 200;
pub const MAX_OUTBOUND_QA_CANCELLATION_NOTE_LENGTH: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundQaRequirement {
    NotRequired,
    ScanEveryCarton,
}

impl OutboundQaRequirement {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::ScanEveryCarton => "scan_every_carton",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "not_required" => Some(Self::NotRequired),
            "scan_every_carton" => Some(Self::ScanEveryCarton),
            _ => None,
        }
    }
}

impl fmt::Display for OutboundQaRequirement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundQaSessionStatus {
    Open,
    Passed,
    Cancelled,
}

impl OutboundQaSessionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Passed => "passed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "passed" => Some(Self::Passed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundQaCancellationReason {
    PackingCorrection,
    QualityIssue,
    PolicyError,
    OperatorError,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OutboundQaCancellationNote(String);

impl OutboundQaCancellationNote {
    pub fn new(value: impl Into<String>) -> Result<Self, OutboundQaError> {
        let value = value.into();
        if value.trim() != value {
            return Err(OutboundQaError::UntrimmedCancellationNote);
        }
        if value.is_empty() {
            return Err(OutboundQaError::EmptyCancellationNote);
        }
        if value.chars().count() > MAX_OUTBOUND_QA_CANCELLATION_NOTE_LENGTH {
            return Err(OutboundQaError::CancellationNoteTooLong);
        }
        if value.chars().any(char::is_control) {
            return Err(OutboundQaError::InvalidCancellationNoteCharacter);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundQaCancellationDetails {
    reason: OutboundQaCancellationReason,
    note: Option<OutboundQaCancellationNote>,
}

impl OutboundQaCancellationDetails {
    pub fn new(
        reason: OutboundQaCancellationReason,
        note: Option<OutboundQaCancellationNote>,
    ) -> Result<Self, OutboundQaError> {
        if reason == OutboundQaCancellationReason::Other && note.is_none() {
            return Err(OutboundQaError::CancellationNoteRequired);
        }
        Ok(Self { reason, note })
    }

    pub const fn reason(&self) -> OutboundQaCancellationReason {
        self.reason
    }

    pub fn note(&self) -> Option<&OutboundQaCancellationNote> {
        self.note.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct OutboundQaSessionRevision(i64);

impl OutboundQaSessionRevision {
    pub const fn new(value: i64) -> Result<Self, OutboundQaError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(OutboundQaError::InvalidRevision { value })
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

impl<'de> Deserialize<'de> for OutboundQaSessionRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct OutboundQaPolicyRevision(i64);

impl OutboundQaPolicyRevision {
    pub const fn new(value: i64) -> Result<Self, OutboundQaError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(OutboundQaError::InvalidRevision { value })
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

impl<'de> Deserialize<'de> for OutboundQaPolicyRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct OutboundQaScanValue(String);

impl OutboundQaScanValue {
    pub fn new(value: impl Into<String>) -> Result<Self, OutboundQaError> {
        let value = value.into();
        if value.is_empty()
            || value.trim() != value
            || value.chars().count() > MAX_OUTBOUND_QA_SCAN_VALUE_LENGTH
            || value.chars().any(char::is_control)
        {
            return Err(OutboundQaError::InvalidScanValue);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for OutboundQaScanValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundQaProgress {
    expected_carton_count: i64,
    verified_carton_count: i64,
}

impl OutboundQaProgress {
    pub const fn new(expected: i64, verified: i64) -> Result<Self, OutboundQaError> {
        if expected <= 0 || verified < 0 || verified > expected {
            return Err(OutboundQaError::InvalidProgress { expected, verified });
        }
        Ok(Self {
            expected_carton_count: expected,
            verified_carton_count: verified,
        })
    }

    pub const fn expected_carton_count(self) -> i64 {
        self.expected_carton_count
    }

    pub const fn verified_carton_count(self) -> i64 {
        self.verified_carton_count
    }

    pub const fn is_complete(self) -> bool {
        self.expected_carton_count == self.verified_carton_count
    }
}

pub fn begin_outbound_qa(
    requirement: OutboundQaRequirement,
    carton_count: i64,
) -> Result<(OutboundQaSessionStatus, OutboundQaProgress), OutboundQaError> {
    if requirement != OutboundQaRequirement::ScanEveryCarton {
        return Err(OutboundQaError::NotRequired);
    }
    Ok((
        OutboundQaSessionStatus::Open,
        OutboundQaProgress::new(carton_count, 0)?,
    ))
}

pub fn record_outbound_qa_carton(
    status: OutboundQaSessionStatus,
    progress: OutboundQaProgress,
    already_verified: bool,
) -> Result<OutboundQaProgress, OutboundQaError> {
    if status != OutboundQaSessionStatus::Open {
        return Err(OutboundQaError::SessionNotOpen { status });
    }
    if already_verified {
        return Err(OutboundQaError::CartonAlreadyVerified);
    }
    if progress.is_complete() {
        return Err(OutboundQaError::AllCartonsAlreadyVerified);
    }
    OutboundQaProgress::new(
        progress.expected_carton_count,
        progress.verified_carton_count + 1,
    )
}

pub fn complete_outbound_qa(
    status: OutboundQaSessionStatus,
    progress: OutboundQaProgress,
) -> Result<OutboundQaSessionStatus, OutboundQaError> {
    if status != OutboundQaSessionStatus::Open {
        return Err(OutboundQaError::SessionNotOpen { status });
    }
    if !progress.is_complete() {
        return Err(OutboundQaError::CartonsRemain {
            expected: progress.expected_carton_count,
            verified: progress.verified_carton_count,
        });
    }
    Ok(OutboundQaSessionStatus::Passed)
}

pub const fn cancel_outbound_qa(
    status: OutboundQaSessionStatus,
) -> Result<OutboundQaSessionStatus, OutboundQaError> {
    match status {
        OutboundQaSessionStatus::Open | OutboundQaSessionStatus::Passed => {
            Ok(OutboundQaSessionStatus::Cancelled)
        }
        OutboundQaSessionStatus::Cancelled => {
            Err(OutboundQaError::SessionNotCancellable { status })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutboundQaError {
    #[error("outbound QA revision must be positive, got {value}")]
    InvalidRevision { value: i64 },
    #[error("outbound QA scan value is invalid")]
    InvalidScanValue,
    #[error("outbound QA cancellation note cannot be empty")]
    EmptyCancellationNote,
    #[error("outbound QA cancellation note must be trimmed")]
    UntrimmedCancellationNote,
    #[error(
        "outbound QA cancellation note cannot exceed {MAX_OUTBOUND_QA_CANCELLATION_NOTE_LENGTH} characters"
    )]
    CancellationNoteTooLong,
    #[error("outbound QA cancellation note cannot contain control characters")]
    InvalidCancellationNoteCharacter,
    #[error("outbound QA cancellation reason Other requires a note")]
    CancellationNoteRequired,
    #[error("outbound QA progress is invalid: expected {expected}, verified {verified}")]
    InvalidProgress { expected: i64, verified: i64 },
    #[error("outbound QA is not required")]
    NotRequired,
    #[error("outbound QA session is not open: {status:?}")]
    SessionNotOpen { status: OutboundQaSessionStatus },
    #[error("outbound QA session cannot be cancelled from {status:?}")]
    SessionNotCancellable { status: OutboundQaSessionStatus },
    #[error("carton was already verified")]
    CartonAlreadyVerified,
    #[error("all cartons are already verified")]
    AllCartonsAlreadyVerified,
    #[error("outbound QA still has cartons to verify: {verified} of {expected}")]
    CartonsRemain { expected: i64, verified: i64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qa_requires_each_carton_exactly_once_before_completion() {
        let (status, progress) =
            begin_outbound_qa(OutboundQaRequirement::ScanEveryCarton, 2).unwrap();
        let progress = record_outbound_qa_carton(status, progress, false).unwrap();
        assert!(complete_outbound_qa(status, progress).is_err());
        assert!(record_outbound_qa_carton(status, progress, true).is_err());
        let progress = record_outbound_qa_carton(status, progress, false).unwrap();
        assert_eq!(
            complete_outbound_qa(status, progress),
            Ok(OutboundQaSessionStatus::Passed)
        );
    }

    #[test]
    fn qa_is_not_opened_when_policy_does_not_require_it() {
        assert_eq!(
            begin_outbound_qa(OutboundQaRequirement::NotRequired, 1),
            Err(OutboundQaError::NotRequired)
        );
    }

    #[test]
    fn open_or_passed_qa_can_be_cancelled_with_valid_evidence() {
        assert_eq!(
            cancel_outbound_qa(OutboundQaSessionStatus::Open),
            Ok(OutboundQaSessionStatus::Cancelled)
        );
        assert_eq!(
            cancel_outbound_qa(OutboundQaSessionStatus::Passed),
            Ok(OutboundQaSessionStatus::Cancelled)
        );
        assert_eq!(
            cancel_outbound_qa(OutboundQaSessionStatus::Cancelled),
            Err(OutboundQaError::SessionNotCancellable {
                status: OutboundQaSessionStatus::Cancelled,
            })
        );
        assert_eq!(
            OutboundQaCancellationDetails::new(OutboundQaCancellationReason::Other, None),
            Err(OutboundQaError::CancellationNoteRequired)
        );
        assert!(OutboundQaCancellationDetails::new(
            OutboundQaCancellationReason::PackingCorrection,
            None,
        )
        .is_ok());
    }
}
