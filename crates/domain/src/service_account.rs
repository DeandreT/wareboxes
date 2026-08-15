use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{FacilityId, InventoryOwnerId};

pub const MAX_SERVICE_ACCOUNT_NAME_LENGTH: usize = 120;
pub const MAX_SERVICE_ACCOUNT_DESCRIPTION_LENGTH: usize = 500;
pub const MAX_SERVICE_ACCOUNT_LABEL_LENGTH: usize = 120;
pub const MAX_SERVICE_ACCOUNT_REASON_LENGTH: usize = 500;
pub const MAX_SERVICE_ACCOUNT_PERMISSION_LENGTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceAccountError {
    #[error("{field} must not be blank")]
    Blank { field: &'static str },
    #[error("{field} must not contain control characters")]
    ControlCharacter { field: &'static str },
    #[error("{field} exceeds {max} characters")]
    TooLong { field: &'static str, max: usize },
    #[error("service account access must include at least one facility")]
    MissingFacilityScope,
    #[error("service account access must include at least one inventory owner")]
    MissingOwnerScope,
    #[error("all-facility access cannot include individual facility IDs")]
    ConflictingFacilityScope,
    #[error("all-owner access cannot include individual inventory-owner IDs")]
    ConflictingOwnerScope,
    #[error("service account access contains a duplicate facility ID")]
    DuplicateFacility,
    #[error("service account access contains a duplicate inventory-owner ID")]
    DuplicateOwner,
    #[error("service account access must include at least one permission")]
    MissingPermission,
    #[error("service account permission `{0}` is invalid")]
    InvalidPermission(String),
    #[error("service accounts cannot be granted the admin permission")]
    AdminPermission,
    #[error("service account access contains a duplicate permission")]
    DuplicatePermission,
    #[error("service account revision must be a positive integer")]
    InvalidRevision,
    #[error("service account bearer token must use the wbs_sa_ prefix and 48 alphanumeric secret characters")]
    InvalidBearerToken,
}

fn validate_text(
    value: String,
    field: &'static str,
    max: usize,
) -> Result<String, ServiceAccountError> {
    if value.trim().is_empty() || value.trim() != value {
        return Err(ServiceAccountError::Blank { field });
    }
    if value.chars().any(char::is_control) {
        return Err(ServiceAccountError::ControlCharacter { field });
    }
    if value.chars().count() > max {
        return Err(ServiceAccountError::TooLong { field, max });
    }
    Ok(value)
}

macro_rules! service_account_text {
    ($name:ident, $field:literal, $max:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ServiceAccountError> {
                validate_text(value.into(), $field, $max).map(Self)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

service_account_text!(
    ServiceAccountName,
    "service account name",
    MAX_SERVICE_ACCOUNT_NAME_LENGTH
);
service_account_text!(
    ServiceAccountDescription,
    "service account description",
    MAX_SERVICE_ACCOUNT_DESCRIPTION_LENGTH
);
service_account_text!(
    ServiceAccountCredentialLabel,
    "credential label",
    MAX_SERVICE_ACCOUNT_LABEL_LENGTH
);
service_account_text!(
    ServiceAccountReason,
    "service account reason",
    MAX_SERVICE_ACCOUNT_REASON_LENGTH
);

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ServiceAccountBearerToken(String);

impl ServiceAccountBearerToken {
    pub fn new(value: impl Into<String>) -> Result<Self, ServiceAccountError> {
        let value = value.into();
        let Some(secret) = value.strip_prefix("wbs_sa_") else {
            return Err(ServiceAccountError::InvalidBearerToken);
        };
        if secret.len() != 48 || !secret.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            return Err(ServiceAccountError::InvalidBearerToken);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn prefix(&self) -> &str {
        &self.0[..15]
    }
}

impl std::fmt::Debug for ServiceAccountBearerToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ServiceAccountBearerToken(REDACTED)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ServiceAccountRevision(i64);

impl ServiceAccountRevision {
    pub fn new(value: i64) -> Result<Self, ServiceAccountError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(ServiceAccountError::InvalidRevision)
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
pub enum ServiceAccountStatus {
    Active,
    Disabled,
}

impl ServiceAccountStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceAccountAccessPolicy {
    pub all_facilities: bool,
    pub facility_ids: Vec<FacilityId>,
    pub all_inventory_owners: bool,
    pub inventory_owner_ids: Vec<InventoryOwnerId>,
    pub permission_names: Vec<String>,
}

impl ServiceAccountAccessPolicy {
    pub fn validate(&self) -> Result<(), ServiceAccountError> {
        if self.all_facilities && !self.facility_ids.is_empty() {
            return Err(ServiceAccountError::ConflictingFacilityScope);
        }
        if !self.all_facilities && self.facility_ids.is_empty() {
            return Err(ServiceAccountError::MissingFacilityScope);
        }
        if self.all_inventory_owners && !self.inventory_owner_ids.is_empty() {
            return Err(ServiceAccountError::ConflictingOwnerScope);
        }
        if !self.all_inventory_owners && self.inventory_owner_ids.is_empty() {
            return Err(ServiceAccountError::MissingOwnerScope);
        }
        if self.facility_ids.iter().collect::<HashSet<_>>().len() != self.facility_ids.len() {
            return Err(ServiceAccountError::DuplicateFacility);
        }
        if self
            .inventory_owner_ids
            .iter()
            .collect::<HashSet<_>>()
            .len()
            != self.inventory_owner_ids.len()
        {
            return Err(ServiceAccountError::DuplicateOwner);
        }
        if self.permission_names.is_empty() {
            return Err(ServiceAccountError::MissingPermission);
        }
        let mut permissions = HashSet::new();
        for name in &self.permission_names {
            if name == "admin" {
                return Err(ServiceAccountError::AdminPermission);
            }
            if name.is_empty()
                || name.len() > MAX_SERVICE_ACCOUNT_PERMISSION_LENGTH
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            {
                return Err(ServiceAccountError::InvalidPermission(name.clone()));
            }
            if !permissions.insert(name) {
                return Err(ServiceAccountError::DuplicatePermission);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> ServiceAccountAccessPolicy {
        ServiceAccountAccessPolicy {
            all_facilities: false,
            facility_ids: vec![FacilityId::new(3).unwrap()],
            all_inventory_owners: false,
            inventory_owner_ids: vec![InventoryOwnerId::new(4).unwrap()],
            permission_names: vec!["orders".into()],
        }
    }

    #[test]
    fn service_account_access_is_explicit_and_non_admin() {
        assert_eq!(policy().validate(), Ok(()));

        let mut invalid = policy();
        invalid.permission_names = vec!["admin".into()];
        assert_eq!(
            invalid.validate(),
            Err(ServiceAccountError::AdminPermission)
        );

        let mut invalid = policy();
        invalid.all_facilities = true;
        assert_eq!(
            invalid.validate(),
            Err(ServiceAccountError::ConflictingFacilityScope)
        );
    }

    #[test]
    fn service_account_text_and_revision_are_bounded() {
        assert!(ServiceAccountName::new("ERP order intake").is_ok());
        assert!(ServiceAccountName::new(" ERP ").is_err());
        assert!(ServiceAccountReason::new("rotation after vendor handoff").is_ok());
        assert!(ServiceAccountRevision::new(0).is_err());
        assert_eq!(
            ServiceAccountRevision::new(1)
                .unwrap()
                .checked_next()
                .unwrap()
                .get(),
            2
        );
        assert!(ServiceAccountBearerToken::new(format!("wbs_sa_{}", "A".repeat(48))).is_ok());
        assert!(ServiceAccountBearerToken::new("wbs_sa_too_short").is_err());
    }
}
