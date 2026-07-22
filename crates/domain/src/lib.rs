//! Domain identifiers and invariants shared across application boundaries.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{kind} must be a positive integer, got {value}")]
pub struct InvalidId {
    kind: &'static str,
    value: i64,
}

macro_rules! positive_id {
    ($name:ident, $label:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(i64);

        impl $name {
            pub fn new(value: i64) -> Result<Self, InvalidId> {
                if value > 0 {
                    Ok(Self(value))
                } else {
                    Err(InvalidId {
                        kind: $label,
                        value,
                    })
                }
            }

            pub const fn get(self) -> i64 {
                self.0
            }
        }

        impl TryFrom<i64> for $name {
            type Error = InvalidId;

            fn try_from(value: i64) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for i64 {
            fn from(value: $name) -> Self {
                value.get()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

positive_id!(TenantId, "tenant ID");
positive_id!(InventoryOwnerId, "inventory owner ID");
positive_id!(FacilityId, "facility ID");
positive_id!(UserId, "user ID");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandContext {
    pub tenant_id: TenantId,
    pub actor_id: UserId,
    pub request_id: String,
    pub idempotency_key: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_ids_reject_non_positive_values() {
        assert!(TenantId::new(0).is_err());
        assert!(FacilityId::new(-1).is_err());
        assert_eq!(InventoryOwnerId::new(7).map(InventoryOwnerId::get), Ok(7));
    }

    #[test]
    fn scoped_ids_do_not_compare_across_types() {
        let tenant = TenantId::new(4).unwrap();
        let facility = FacilityId::new(4).unwrap();

        assert_eq!(tenant.get(), facility.get());
    }
}
