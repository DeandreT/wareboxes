use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{BillableEventType, BillingUnit, Timestamp};

pub const MAX_BILLING_CONTRACT_NUMBER_LENGTH: usize = 80;
pub const MAX_BILLING_DESCRIPTION_LENGTH: usize = 500;
pub const MAX_BILLING_SOURCE_TYPE_LENGTH: usize = 64;
pub const MAX_BILLING_QUANTITY: i64 = 1_000_000_000_000;
pub const MAX_BILLING_RATE_MINOR: u64 = 1_000_000_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BillingContractNumber(String);

impl BillingContractNumber {
    pub fn new(value: String) -> Result<Self, BillingError> {
        let value = value.trim().to_owned();
        if value.is_empty() || value.len() > MAX_BILLING_CONTRACT_NUMBER_LENGTH {
            return Err(BillingError::InvalidContractNumber);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CurrencyCode(String);

impl CurrencyCode {
    pub fn new(value: String) -> Result<Self, BillingError> {
        let value = value.trim().to_ascii_uppercase();
        if value.len() != 3 || !value.bytes().all(|byte| byte.is_ascii_uppercase()) {
            return Err(BillingError::InvalidCurrency);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingContractStatus {
    Draft,
    Active,
    Closed,
}

impl BillingContractStatus {
    pub const fn activate(self) -> Result<Self, BillingError> {
        match self {
            Self::Draft => Ok(Self::Active),
            _ => Err(BillingError::InvalidContractTransition),
        }
    }

    pub const fn close(self) -> Result<Self, BillingError> {
        match self {
            Self::Active => Ok(Self::Closed),
            _ => Err(BillingError::InvalidContractTransition),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingEffectiveWindow {
    pub effective_from: Timestamp,
    pub effective_until: Option<Timestamp>,
}

impl BillingEffectiveWindow {
    pub fn new(
        effective_from: Timestamp,
        effective_until: Option<Timestamp>,
    ) -> Result<Self, BillingError> {
        if effective_until.is_some_and(|until| until <= effective_from) {
            return Err(BillingError::InvalidEffectiveWindow);
        }
        Ok(Self {
            effective_from,
            effective_until,
        })
    }

    pub fn includes(self, timestamp: Timestamp) -> bool {
        timestamp >= self.effective_from
            && self.effective_until.is_none_or(|until| timestamp < until)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingRateDefinition {
    pub event_type: BillableEventType,
    pub unit: BillingUnit,
    pub currency: CurrencyCode,
    pub rate_minor: u64,
    pub minimum_charge_minor: u64,
}

impl BillingRateDefinition {
    pub fn new(
        event_type: BillableEventType,
        unit: BillingUnit,
        currency: CurrencyCode,
        rate_minor: u64,
        minimum_charge_minor: u64,
    ) -> Result<Self, BillingError> {
        if rate_minor == 0
            || rate_minor > MAX_BILLING_RATE_MINOR
            || minimum_charge_minor > MAX_BILLING_RATE_MINOR
        {
            return Err(BillingError::InvalidRate);
        }
        Ok(Self {
            event_type,
            unit,
            currency,
            rate_minor,
            minimum_charge_minor,
        })
    }

    pub fn charge_minor(&self, quantity: BillingQuantity) -> Result<u64, BillingError> {
        let gross = self
            .rate_minor
            .checked_mul(quantity.get())
            .ok_or(BillingError::AmountOverflow)?;
        Ok(gross.max(self.minimum_charge_minor))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BillingQuantity(u64);

impl BillingQuantity {
    pub fn new(value: i64) -> Result<Self, BillingError> {
        if !(1..=MAX_BILLING_QUANTITY).contains(&value) {
            return Err(BillingError::InvalidQuantity);
        }
        Ok(Self(
            u64::try_from(value).map_err(|_| BillingError::InvalidQuantity)?,
        ))
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingRunStatus {
    PendingReview,
    Approved,
    Rejected,
    Exported,
}

impl BillingRunStatus {
    pub const fn approve(self) -> Result<Self, BillingError> {
        match self {
            Self::PendingReview => Ok(Self::Approved),
            _ => Err(BillingError::InvalidReviewTransition),
        }
    }

    pub const fn reject(self) -> Result<Self, BillingError> {
        match self {
            Self::PendingReview => Ok(Self::Rejected),
            _ => Err(BillingError::InvalidReviewTransition),
        }
    }

    pub const fn export(self) -> Result<Self, BillingError> {
        match self {
            Self::Approved => Ok(Self::Exported),
            _ => Err(BillingError::InvalidReviewTransition),
        }
    }
}

pub fn validate_review_separation(generated_by: i64, reviewed_by: i64) -> Result<(), BillingError> {
    if generated_by <= 0 || reviewed_by <= 0 || generated_by == reviewed_by {
        Err(BillingError::ReviewSeparationRequired)
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum BillingError {
    #[error("billing contract number must be non-blank and no longer than 80 characters")]
    InvalidContractNumber,
    #[error("currency must be a three-letter ISO-style uppercase code")]
    InvalidCurrency,
    #[error("billing effective_until must be later than effective_from")]
    InvalidEffectiveWindow,
    #[error("billing rate or minimum is outside the supported range")]
    InvalidRate,
    #[error("billable quantity must be positive and within the supported range")]
    InvalidQuantity,
    #[error("billing amount exceeds the supported range")]
    AmountOverflow,
    #[error("billing contract lifecycle transition is invalid")]
    InvalidContractTransition,
    #[error("billing review or export lifecycle transition is invalid")]
    InvalidReviewTransition,
    #[error("billing review requires a different administrator from the run generator")]
    ReviewSeparationRequired,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_apply_minimums_and_fail_on_overflow() {
        let rate = BillingRateDefinition::new(
            BillableEventType::PickedUnit,
            BillingUnit::Each,
            CurrencyCode::new("usd".into()).unwrap(),
            25,
            100,
        )
        .unwrap();
        assert_eq!(rate.charge_minor(BillingQuantity::new(2).unwrap()), Ok(100));
        assert_eq!(rate.charge_minor(BillingQuantity::new(5).unwrap()), Ok(125));
    }

    #[test]
    fn contract_and_run_lifecycles_are_explicit() {
        assert_eq!(
            BillingContractStatus::Draft.activate(),
            Ok(BillingContractStatus::Active)
        );
        assert!(BillingContractStatus::Draft.close().is_err());
        assert_eq!(
            BillingRunStatus::PendingReview.approve(),
            Ok(BillingRunStatus::Approved)
        );
        assert_eq!(
            BillingRunStatus::Approved.export(),
            Ok(BillingRunStatus::Exported)
        );
        assert!(validate_review_separation(7, 7).is_err());
    }

    #[test]
    fn effective_windows_are_half_open() {
        let from = "2026-08-01T00:00:00Z".parse().unwrap();
        let until = "2026-09-01T00:00:00Z".parse().unwrap();
        let window = BillingEffectiveWindow::new(from, Some(until)).unwrap();
        assert!(window.includes(from));
        assert!(!window.includes(until));
    }
}
