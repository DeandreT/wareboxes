//! Vendor-return authorization and outbound inventory lifecycle.

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_VENDOR_RETURN_LINES: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VendorReturnError {
    #[error("vendor-return text is empty, untrimmed, or exceeds {max} characters")]
    InvalidText { max: usize },
    #[error("vendor-return quantity must be positive")]
    InvalidQuantity,
    #[error("vendor-return revision must be positive")]
    InvalidRevision,
    #[error("vendor return requires between 1 and {MAX_VENDOR_RETURN_LINES} unique lines")]
    InvalidLines,
    #[error("vendor-return transition from {from:?} to {to:?} is not allowed")]
    InvalidTransition {
        from: VendorReturnStatus,
        to: VendorReturnStatus,
    },
}

macro_rules! vendor_return_text {
    ($name:ident, $max:expr) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, VendorReturnError> {
                let value = value.into();
                if value.is_empty() || value.trim() != value || value.chars().count() > $max {
                    return Err(VendorReturnError::InvalidText { max: $max });
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

vendor_return_text!(VendorReturnNumber, 120);
vendor_return_text!(VendorName, 200);
vendor_return_text!(VendorReference, 200);
vendor_return_text!(VendorReturnNote, 500);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VendorReturnQuantity(i64);

impl VendorReturnQuantity {
    pub const fn new(value: i64) -> Result<Self, VendorReturnError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(VendorReturnError::InvalidQuantity)
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VendorReturnRevision(i64);

impl VendorReturnRevision {
    pub const fn new(value: i64) -> Result<Self, VendorReturnError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(VendorReturnError::InvalidRevision)
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    pub fn next(self) -> Result<Self, VendorReturnError> {
        self.0
            .checked_add(1)
            .ok_or(VendorReturnError::InvalidRevision)
            .and_then(Self::new)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VendorReturnReason {
    Damaged,
    Defective,
    Expired,
    Recall,
    Overstock,
    VendorRequest,
    Other,
}

impl VendorReturnReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Damaged => "damaged",
            Self::Defective => "defective",
            Self::Expired => "expired",
            Self::Recall => "recall",
            Self::Overstock => "overstock",
            Self::VendorRequest => "vendor_request",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "damaged" => Some(Self::Damaged),
            "defective" => Some(Self::Defective),
            "expired" => Some(Self::Expired),
            "recall" => Some(Self::Recall),
            "overstock" => Some(Self::Overstock),
            "vendor_request" => Some(Self::VendorRequest),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VendorReturnStatus {
    Draft,
    Released,
    Shipped,
    Cancelled,
}

impl VendorReturnStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Released => "released",
            Self::Shipped => "shipped",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "draft" => Some(Self::Draft),
            "released" => Some(Self::Released),
            "shipped" => Some(Self::Shipped),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    pub fn require_transition_to(self, next: Self) -> Result<(), VendorReturnError> {
        if matches!(
            (self, next),
            (Self::Draft, Self::Released | Self::Cancelled)
                | (Self::Released, Self::Shipped | Self::Cancelled)
        ) {
            Ok(())
        } else {
            Err(VendorReturnError::InvalidTransition {
                from: self,
                to: next,
            })
        }
    }
}

pub fn validate_vendor_return_lines(balance_ids: &[i64]) -> Result<(), VendorReturnError> {
    if balance_ids.is_empty() || balance_ids.len() > MAX_VENDOR_RETURN_LINES {
        return Err(VendorReturnError::InvalidLines);
    }
    let mut ids = balance_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() != balance_ids.len() || ids.iter().any(|id| *id <= 0) {
        return Err(VendorReturnError::InvalidLines);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_is_explicit_and_terminal() {
        assert!(VendorReturnStatus::Draft
            .require_transition_to(VendorReturnStatus::Released)
            .is_ok());
        assert!(VendorReturnStatus::Released
            .require_transition_to(VendorReturnStatus::Shipped)
            .is_ok());
        assert!(VendorReturnStatus::Shipped
            .require_transition_to(VendorReturnStatus::Cancelled)
            .is_err());
    }

    #[test]
    fn return_lines_are_unique_and_bounded() {
        assert!(validate_vendor_return_lines(&[1, 2]).is_ok());
        assert!(validate_vendor_return_lines(&[1, 1]).is_err());
        assert!(validate_vendor_return_lines(&[]).is_err());
    }
}
