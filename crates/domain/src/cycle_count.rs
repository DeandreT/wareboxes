//! Cycle-count tolerance, recount, and variance-review invariants.

use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

pub const MAX_CYCLE_COUNT_RECOUNTS: u16 = 10;
pub const MAX_CYCLE_COUNT_VARIANCE_NOTE_LENGTH: usize = 500;
pub const MAX_CYCLE_COUNT_PERCENTAGE_BASIS_POINTS: u32 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct CycleCountPolicyRevision(i64);

impl CycleCountPolicyRevision {
    pub const fn new(value: i64) -> Result<Self, CycleCountError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(CycleCountError::InvalidRevision { value })
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

impl<'de> Deserialize<'de> for CycleCountPolicyRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct CycleCountVarianceRevision(i64);

impl CycleCountVarianceRevision {
    pub const fn new(value: i64) -> Result<Self, CycleCountError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(CycleCountError::InvalidRevision { value })
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

impl<'de> Deserialize<'de> for CycleCountVarianceRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleCountTolerancePolicy {
    absolute_tolerance_quantity: i64,
    percentage_tolerance_basis_points: u32,
    automatic_recount_limit: u16,
}

impl CycleCountTolerancePolicy {
    pub const fn new(
        absolute_tolerance_quantity: i64,
        percentage_tolerance_basis_points: u32,
        automatic_recount_limit: u16,
    ) -> Result<Self, CycleCountError> {
        if absolute_tolerance_quantity < 0 {
            return Err(CycleCountError::NegativeAbsoluteTolerance {
                value: absolute_tolerance_quantity,
            });
        }
        if percentage_tolerance_basis_points > MAX_CYCLE_COUNT_PERCENTAGE_BASIS_POINTS {
            return Err(CycleCountError::PercentageToleranceOutOfRange {
                value: percentage_tolerance_basis_points,
            });
        }
        if automatic_recount_limit > MAX_CYCLE_COUNT_RECOUNTS {
            return Err(CycleCountError::RecountLimitOutOfRange {
                value: automatic_recount_limit,
            });
        }
        Ok(Self {
            absolute_tolerance_quantity,
            percentage_tolerance_basis_points,
            automatic_recount_limit,
        })
    }

    pub const fn absolute_tolerance_quantity(self) -> i64 {
        self.absolute_tolerance_quantity
    }

    pub const fn percentage_tolerance_basis_points(self) -> u32 {
        self.percentage_tolerance_basis_points
    }

    pub const fn automatic_recount_limit(self) -> u16 {
        self.automatic_recount_limit
    }

    pub fn allowed_variance_quantity(self, system_quantity: i64) -> Result<i64, CycleCountError> {
        if system_quantity < 0 {
            return Err(CycleCountError::NegativeSystemQuantity {
                value: system_quantity,
            });
        }
        let numerator = i128::from(system_quantity)
            .checked_mul(i128::from(self.percentage_tolerance_basis_points))
            .ok_or(CycleCountError::ToleranceOverflow)?;
        let percentage = numerator
            .checked_add(i128::from(MAX_CYCLE_COUNT_PERCENTAGE_BASIS_POINTS - 1))
            .ok_or(CycleCountError::ToleranceOverflow)?
            / i128::from(MAX_CYCLE_COUNT_PERCENTAGE_BASIS_POINTS);
        let percentage =
            i64::try_from(percentage).map_err(|_| CycleCountError::ToleranceOverflow)?;
        Ok(self.absolute_tolerance_quantity.max(percentage))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleCountDisposition {
    Posted,
    RecountRequired,
    ApprovalRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleCountVarianceStatus {
    AwaitingRecount,
    AwaitingApproval,
    Posted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleCountVarianceDecision {
    ApproveAdjustment,
    RequestRecount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleCountVarianceReason {
    VerifiedPhysicalCount,
    PackagingOrUomIssue,
    ReceivingOrShippingTiming,
    SuspectedMiscount,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct CycleCountVarianceNote(String);

impl CycleCountVarianceNote {
    pub fn new(value: impl Into<String>) -> Result<Self, CycleCountError> {
        let value = value.into();
        if value.is_empty()
            || value.trim() != value
            || value.chars().count() > MAX_CYCLE_COUNT_VARIANCE_NOTE_LENGTH
        {
            return Err(CycleCountError::InvalidVarianceNote);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for CycleCountVarianceNote {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleCountVarianceDecisionDetails {
    pub decision: CycleCountVarianceDecision,
    pub reason: CycleCountVarianceReason,
    pub note: Option<CycleCountVarianceNote>,
}

impl CycleCountVarianceDecisionDetails {
    pub fn new(
        decision: CycleCountVarianceDecision,
        reason: CycleCountVarianceReason,
        note: Option<CycleCountVarianceNote>,
    ) -> Result<Self, CycleCountError> {
        if reason == CycleCountVarianceReason::Other && note.is_none() {
            return Err(CycleCountError::OtherReasonRequiresNote);
        }
        Ok(Self {
            decision,
            reason,
            note,
        })
    }
}

pub fn decide_cycle_count_disposition(
    policy: CycleCountTolerancePolicy,
    system_quantity: i64,
    variance_quantity: i64,
    automatic_recounts_used: u16,
) -> Result<CycleCountDisposition, CycleCountError> {
    if automatic_recounts_used > policy.automatic_recount_limit {
        return Err(CycleCountError::RecountUsageExceedsLimit {
            used: automatic_recounts_used,
            limit: policy.automatic_recount_limit,
        });
    }
    let magnitude = variance_quantity
        .checked_abs()
        .ok_or(CycleCountError::VarianceMagnitudeOverflow)?;
    if magnitude <= policy.allowed_variance_quantity(system_quantity)? {
        return Ok(CycleCountDisposition::Posted);
    }
    if automatic_recounts_used < policy.automatic_recount_limit {
        Ok(CycleCountDisposition::RecountRequired)
    } else {
        Ok(CycleCountDisposition::ApprovalRequired)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CycleCountError {
    #[error("cycle count revision must be positive, got {value}")]
    InvalidRevision { value: i64 },
    #[error("cycle count absolute tolerance cannot be negative, got {value}")]
    NegativeAbsoluteTolerance { value: i64 },
    #[error("cycle count percentage tolerance is out of range, got {value}")]
    PercentageToleranceOutOfRange { value: u32 },
    #[error("cycle count recount limit is out of range, got {value}")]
    RecountLimitOutOfRange { value: u16 },
    #[error("cycle count system quantity cannot be negative, got {value}")]
    NegativeSystemQuantity { value: i64 },
    #[error("cycle count tolerance is out of range")]
    ToleranceOverflow,
    #[error("cycle count variance magnitude is out of range")]
    VarianceMagnitudeOverflow,
    #[error("cycle count recount usage {used} exceeds configured limit {limit}")]
    RecountUsageExceedsLimit { used: u16, limit: u16 },
    #[error("cycle count variance note is invalid")]
    InvalidVarianceNote,
    #[error("cycle count variance reason other requires a note")]
    OtherReasonRequiresNote,
}

impl fmt::Display for CycleCountTolerancePolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "absolute {}, percentage {} bps, recounts {}",
            self.absolute_tolerance_quantity,
            self.percentage_tolerance_basis_points,
            self.automatic_recount_limit
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerance_uses_the_larger_absolute_or_rounded_percentage_value() {
        let policy = CycleCountTolerancePolicy::new(2, 250, 1).unwrap();
        assert_eq!(policy.allowed_variance_quantity(40), Ok(2));
        assert_eq!(policy.allowed_variance_quantity(101), Ok(3));
    }

    #[test]
    fn disposition_posts_recounts_then_requires_approval() {
        let policy = CycleCountTolerancePolicy::new(1, 0, 1).unwrap();
        assert_eq!(
            decide_cycle_count_disposition(policy, 10, -1, 0),
            Ok(CycleCountDisposition::Posted)
        );
        assert_eq!(
            decide_cycle_count_disposition(policy, 10, -3, 0),
            Ok(CycleCountDisposition::RecountRequired)
        );
        assert_eq!(
            decide_cycle_count_disposition(policy, 10, -3, 1),
            Ok(CycleCountDisposition::ApprovalRequired)
        );
    }

    #[test]
    fn other_variance_decision_requires_a_note() {
        assert_eq!(
            CycleCountVarianceDecisionDetails::new(
                CycleCountVarianceDecision::RequestRecount,
                CycleCountVarianceReason::Other,
                None,
            ),
            Err(CycleCountError::OtherReasonRequiresNote)
        );
    }
}
