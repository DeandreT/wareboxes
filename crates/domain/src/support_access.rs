use std::collections::HashSet;

use chrono::Duration;
use serde::{Deserialize, Serialize};

use crate::{FacilityId, InventoryOwnerId, Timestamp};

pub const MAX_SUPPORT_ACCESS_REASON_LENGTH: usize = 500;
pub const MAX_SUPPORT_ACCESS_PERMISSION_LENGTH: usize = 64;
pub const MAX_SUPPORT_ACCESS_DURATION_HOURS: i64 = 8;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SupportAccessError {
    #[error("support access reason must not be blank or padded")]
    InvalidReason,
    #[error("support access reason exceeds {MAX_SUPPORT_ACCESS_REASON_LENGTH} characters")]
    ReasonTooLong,
    #[error("support access reason must not contain control characters")]
    ReasonControlCharacter,
    #[error("support access must expire after it is requested")]
    InvalidExpiration,
    #[error("support access cannot exceed {MAX_SUPPORT_ACCESS_DURATION_HOURS} hours")]
    DurationTooLong,
    #[error("support access must include at least one facility")]
    MissingFacilityScope,
    #[error("all-facility access cannot include individual facility IDs")]
    ConflictingFacilityScope,
    #[error("support access contains a duplicate facility ID")]
    DuplicateFacility,
    #[error("support access must include at least one inventory owner")]
    MissingOwnerScope,
    #[error("all-owner access cannot include individual inventory-owner IDs")]
    ConflictingOwnerScope,
    #[error("support access contains a duplicate inventory-owner ID")]
    DuplicateOwner,
    #[error("support access must include at least one permission")]
    MissingPermission,
    #[error("support access permission `{0}` is invalid")]
    InvalidPermission(String),
    #[error("support access cannot be granted the admin permission")]
    AdminPermission,
    #[error("support access contains a duplicate permission")]
    DuplicatePermission,
    #[error("support access revision must be a positive integer")]
    InvalidRevision,
    #[error("support access cannot transition from {from} to {to}")]
    InvalidTransition {
        from: SupportAccessStatus,
        to: SupportAccessStatus,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SupportAccessReason(String);

impl SupportAccessReason {
    pub fn new(value: impl Into<String>) -> Result<Self, SupportAccessError> {
        let value = value.into();
        if value.trim().is_empty() || value.trim() != value {
            return Err(SupportAccessError::InvalidReason);
        }
        if value.chars().any(char::is_control) {
            return Err(SupportAccessError::ReasonControlCharacter);
        }
        if value.chars().count() > MAX_SUPPORT_ACCESS_REASON_LENGTH {
            return Err(SupportAccessError::ReasonTooLong);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SupportAccessRevision(i64);

impl SupportAccessRevision {
    pub fn new(value: i64) -> Result<Self, SupportAccessError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(SupportAccessError::InvalidRevision)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportAccessStatus {
    Pending,
    Active,
    Rejected,
    Revoked,
    Expired,
}

impl SupportAccessStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Rejected => "rejected",
            Self::Revoked => "revoked",
            Self::Expired => "expired",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "active" => Some(Self::Active),
            "rejected" => Some(Self::Rejected),
            "revoked" => Some(Self::Revoked),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }

    pub fn effective(self, expires_at: Timestamp, now: Timestamp) -> Self {
        if matches!(self, Self::Pending | Self::Active) && expires_at <= now {
            Self::Expired
        } else {
            self
        }
    }

    pub fn require_transition(self, next: Self) -> Result<(), SupportAccessError> {
        if matches!(
            (self, next),
            (Self::Pending, Self::Active)
                | (Self::Pending, Self::Rejected)
                | (Self::Active, Self::Revoked)
        ) {
            Ok(())
        } else {
            Err(SupportAccessError::InvalidTransition {
                from: self,
                to: next,
            })
        }
    }
}

impl std::fmt::Display for SupportAccessStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportAccessPolicy {
    pub all_facilities: bool,
    pub facility_ids: Vec<FacilityId>,
    pub all_inventory_owners: bool,
    pub inventory_owner_ids: Vec<InventoryOwnerId>,
    pub permission_names: Vec<String>,
}

impl SupportAccessPolicy {
    pub fn validate(&self) -> Result<(), SupportAccessError> {
        if self.all_facilities && !self.facility_ids.is_empty() {
            return Err(SupportAccessError::ConflictingFacilityScope);
        }
        if !self.all_facilities && self.facility_ids.is_empty() {
            return Err(SupportAccessError::MissingFacilityScope);
        }
        if self.facility_ids.iter().collect::<HashSet<_>>().len() != self.facility_ids.len() {
            return Err(SupportAccessError::DuplicateFacility);
        }
        if self.all_inventory_owners && !self.inventory_owner_ids.is_empty() {
            return Err(SupportAccessError::ConflictingOwnerScope);
        }
        if !self.all_inventory_owners && self.inventory_owner_ids.is_empty() {
            return Err(SupportAccessError::MissingOwnerScope);
        }
        if self
            .inventory_owner_ids
            .iter()
            .collect::<HashSet<_>>()
            .len()
            != self.inventory_owner_ids.len()
        {
            return Err(SupportAccessError::DuplicateOwner);
        }
        if self.permission_names.is_empty() {
            return Err(SupportAccessError::MissingPermission);
        }
        let mut names = HashSet::new();
        for name in &self.permission_names {
            if name == "admin" {
                return Err(SupportAccessError::AdminPermission);
            }
            if name.is_empty()
                || name.len() > MAX_SUPPORT_ACCESS_PERMISSION_LENGTH
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            {
                return Err(SupportAccessError::InvalidPermission(name.clone()));
            }
            if !names.insert(name) {
                return Err(SupportAccessError::DuplicatePermission);
            }
        }
        Ok(())
    }
}

pub fn validate_support_access_window(
    requested_at: Timestamp,
    expires_at: Timestamp,
) -> Result<(), SupportAccessError> {
    if expires_at <= requested_at {
        return Err(SupportAccessError::InvalidExpiration);
    }
    if expires_at - requested_at > Duration::hours(MAX_SUPPORT_ACCESS_DURATION_HOURS) {
        return Err(SupportAccessError::DurationTooLong);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use super::*;

    fn policy() -> SupportAccessPolicy {
        SupportAccessPolicy {
            all_facilities: false,
            facility_ids: vec![FacilityId::new(4).unwrap()],
            all_inventory_owners: false,
            inventory_owner_ids: vec![InventoryOwnerId::new(5).unwrap()],
            permission_names: vec!["wms".into()],
        }
    }

    #[test]
    fn policy_is_explicit_and_cannot_delegate_tenant_administration() {
        assert_eq!(policy().validate(), Ok(()));
        let mut invalid = policy();
        invalid.permission_names = vec!["admin".into()];
        assert_eq!(invalid.validate(), Err(SupportAccessError::AdminPermission));
    }

    #[test]
    fn access_window_and_separation_transitions_are_bounded() {
        let now = Utc::now();
        assert_eq!(
            validate_support_access_window(now, now + Duration::hours(8)),
            Ok(())
        );
        assert_eq!(
            validate_support_access_window(now, now + Duration::hours(9)),
            Err(SupportAccessError::DurationTooLong)
        );
        assert_eq!(
            SupportAccessStatus::Pending.require_transition(SupportAccessStatus::Active),
            Ok(())
        );
        assert!(SupportAccessStatus::Active
            .require_transition(SupportAccessStatus::Rejected)
            .is_err());
    }
}
