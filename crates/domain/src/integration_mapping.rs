//! Partner-facing order item identity mapping invariants.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

use crate::{CatalogItemId, InventoryOwnerId, TenantId};

pub const MAX_INTEGRATION_SOURCE_KEY_LENGTH: usize = 200;
pub const MAX_EXTERNAL_ITEM_KEY_LENGTH: usize = 200;
pub const MAX_EXTERNAL_ITEM_UOM_LENGTH: usize = 32;

macro_rules! mapping_text {
    ($name:ident, $limit:ident, $label:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, IntegrationMappingError> {
                let value = value.into();
                if value.is_empty()
                    || value.trim() != value
                    || value.chars().count() > $limit
                    || value.chars().any(char::is_control)
                {
                    return Err(IntegrationMappingError::InvalidText {
                        field: $label,
                        max_length: $limit,
                    });
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
            }
        }
    };
}

mapping_text!(
    IntegrationSourceKey,
    MAX_INTEGRATION_SOURCE_KEY_LENGTH,
    "integration source key"
);
mapping_text!(
    ExternalItemKey,
    MAX_EXTERNAL_ITEM_KEY_LENGTH,
    "external item key"
);
mapping_text!(
    ExternalItemUom,
    MAX_EXTERNAL_ITEM_UOM_LENGTH,
    "external item UOM"
);
mapping_text!(
    IntegrationMappedUom,
    MAX_EXTERNAL_ITEM_UOM_LENGTH,
    "mapped item UOM"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationOrderItemMappingStatus {
    Active,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct IntegrationOrderItemMappingRevision(i64);

impl IntegrationOrderItemMappingRevision {
    pub const fn new(value: i64) -> Result<Self, IntegrationMappingError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(IntegrationMappingError::InvalidRevision { value })
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

impl<'de> Deserialize<'de> for IntegrationOrderItemMappingRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(i64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationOrderItemMappingDefinition {
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub source_key: IntegrationSourceKey,
    pub external_item_key: ExternalItemKey,
    pub external_uom: ExternalItemUom,
    pub item_id: CatalogItemId,
    pub requested_uom: IntegrationMappedUom,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IntegrationMappingError {
    #[error(
        "{field} must be trimmed, nonempty, control-free, and at most {max_length} characters"
    )]
    InvalidText {
        field: &'static str,
        max_length: usize,
    },
    #[error("integration order item mapping revision must be positive, got {value}")]
    InvalidRevision { value: i64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partner_identity_values_are_bounded_and_operator_safe() {
        assert!(IntegrationSourceKey::new("acme-edi").is_ok());
        assert!(ExternalItemKey::new("CLIENT-SKU-100").is_ok());
        assert!(ExternalItemUom::new("CS").is_ok());
        assert!(ExternalItemKey::new(" client-sku").is_err());
        assert!(ExternalItemKey::new("client\nsku").is_err());
        assert!(ExternalItemKey::new("x".repeat(MAX_EXTERNAL_ITEM_KEY_LENGTH + 1)).is_err());
    }

    #[test]
    fn mapping_revision_is_strict_and_monotonic() {
        let revision = IntegrationOrderItemMappingRevision::new(1).unwrap();
        assert_eq!(revision.checked_next().unwrap().get(), 2);
        assert!(IntegrationOrderItemMappingRevision::new(0).is_err());
    }
}
