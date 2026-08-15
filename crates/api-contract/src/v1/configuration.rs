use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use super::{OpaqueCursor, PageLimit, Revision};

pub const MAX_CONFIGURATION_RATE_MINOR: u64 = 1_000_000_000_000;
pub const MAX_CONFIGURATION_WAVE_ORDERS: u32 = 10_000;
pub const MAX_CONFIGURATION_PERCENTAGE_BASIS_POINTS: u16 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionRuleKind {
    Receipt,
    Putaway,
    Allocation,
    Replenishment,
    Wave,
    Pick,
    Pack,
    Count,
    Document,
    Billing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationStatus {
    Draft,
    PendingApproval,
    Approved,
    Active,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "level", deny_unknown_fields)]
pub enum ConfigurationScope {
    Tenant,
    InventoryOwner {
        inventory_owner_id: i64,
    },
    Facility {
        facility_id: i64,
    },
    OwnerFacility {
        inventory_owner_id: i64,
        facility_id: i64,
    },
}

impl ConfigurationScope {
    pub fn validate(self) -> Result<Self, &'static str> {
        match self {
            Self::InventoryOwner { inventory_owner_id }
            | Self::OwnerFacility {
                inventory_owner_id, ..
            } if inventory_owner_id <= 0 => Err("inventory_owner_id must be positive"),
            Self::Facility { facility_id } | Self::OwnerFacility { facility_id, .. }
                if facility_id <= 0 =>
            {
                Err("facility_id must be positive")
            }
            _ => Ok(self),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryRotation {
    Fifo,
    Fefo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillableEventType {
    ReceiptLine,
    ReceivedUnit,
    PalletDay,
    PickLine,
    PickedUnit,
    PackedCarton,
    ShippedUnit,
    ReturnUnit,
    RelabelUnit,
    RefurbishmentUnit,
    KitUnit,
    AssemblyUnit,
    Accessorial,
    DetentionHour,
    ValueAddedServiceUnit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingUnit {
    Event,
    Each,
    Case,
    Pallet,
    Carton,
    Hour,
    Day,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum DecisionRule {
    Receipt {
        allow_unexpected: bool,
        quarantine_unmapped_items: bool,
        over_receipt_tolerance_basis_points: u16,
    },
    Putaway {
        require_zone_compatibility: bool,
        enforce_location_capacity: bool,
        allow_mixed_lots: bool,
    },
    Allocation {
        rotation: InventoryRotation,
        allow_partial: bool,
        require_complete_line: bool,
    },
    Replenishment {
        minimum_percent: u8,
        target_percent: u8,
        include_inbound_projection: bool,
    },
    Wave {
        max_orders: u32,
        require_complete_allocation: bool,
    },
    Pick {
        require_source_location_scan: bool,
        require_item_scan: bool,
        require_destination_container_scan: bool,
    },
    Pack {
        require_station_scan: bool,
        require_weight: bool,
        allow_mixed_orders: bool,
    },
    Count {
        absolute_tolerance: i64,
        percentage_tolerance_basis_points: u16,
        approval_threshold: i64,
    },
    Document {
        generate_packing_slip: bool,
        generate_carton_label: bool,
        require_tracking_barcode: bool,
    },
    Billing {
        event_type: BillableEventType,
        unit: BillingUnit,
        currency: String,
        rate_minor: u64,
        minimum_charge_minor: u64,
    },
}

impl DecisionRule {
    pub const fn kind(&self) -> DecisionRuleKind {
        match self {
            Self::Receipt { .. } => DecisionRuleKind::Receipt,
            Self::Putaway { .. } => DecisionRuleKind::Putaway,
            Self::Allocation { .. } => DecisionRuleKind::Allocation,
            Self::Replenishment { .. } => DecisionRuleKind::Replenishment,
            Self::Wave { .. } => DecisionRuleKind::Wave,
            Self::Pick { .. } => DecisionRuleKind::Pick,
            Self::Pack { .. } => DecisionRuleKind::Pack,
            Self::Count { .. } => DecisionRuleKind::Count,
            Self::Document { .. } => DecisionRuleKind::Document,
            Self::Billing { .. } => DecisionRuleKind::Billing,
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Receipt {
                over_receipt_tolerance_basis_points,
                ..
            } if *over_receipt_tolerance_basis_points
                > MAX_CONFIGURATION_PERCENTAGE_BASIS_POINTS =>
            {
                Err("over-receipt tolerance must not exceed 10000 basis points")
            }
            Self::Allocation {
                allow_partial,
                require_complete_line,
                ..
            } if *allow_partial && *require_complete_line => {
                Err("partial allocation conflicts with complete-line allocation")
            }
            Self::Replenishment {
                minimum_percent,
                target_percent,
                ..
            } if *minimum_percent > 100
                || *target_percent > 100
                || minimum_percent >= target_percent =>
            {
                Err("replenishment minimum must be below target and no greater than 100")
            }
            Self::Wave { max_orders, .. }
                if *max_orders == 0 || *max_orders > MAX_CONFIGURATION_WAVE_ORDERS =>
            {
                Err("max_orders is outside the supported range")
            }
            Self::Count {
                absolute_tolerance,
                percentage_tolerance_basis_points,
                approval_threshold,
            } if *absolute_tolerance < 0
                || *percentage_tolerance_basis_points
                    > MAX_CONFIGURATION_PERCENTAGE_BASIS_POINTS
                || *approval_threshold < *absolute_tolerance =>
            {
                Err("count tolerance or approval threshold is invalid")
            }
            Self::Document {
                generate_packing_slip,
                generate_carton_label,
                ..
            } if !generate_packing_slip && !generate_carton_label => {
                Err("document rule must generate at least one document")
            }
            Self::Billing {
                currency,
                rate_minor,
                minimum_charge_minor,
                ..
            } if currency.len() != 3
                || !currency.bytes().all(|byte| byte.is_ascii_uppercase())
                || *rate_minor == 0
                || *rate_minor > MAX_CONFIGURATION_RATE_MINOR
                || *minimum_charge_minor > MAX_CONFIGURATION_RATE_MINOR =>
            {
                Err("billing currency, rate, or minimum is invalid")
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateConfigurationRequest {
    pub scope: ConfigurationScope,
    pub effective_from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_until: Option<String>,
    pub rule: DecisionRule,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
}

impl<'de> Deserialize<'de> for CreateConfigurationRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawRequest {
            scope: ConfigurationScope,
            effective_from: String,
            #[serde(default)]
            effective_until: Option<String>,
            rule: DecisionRule,
            #[serde(default)]
            expected_revision: Option<Revision>,
        }

        let raw = RawRequest::deserialize(deserializer)?;
        let scope = raw.scope.validate().map_err(D::Error::custom)?;
        if raw.effective_from.trim().is_empty()
            || raw
                .effective_until
                .as_ref()
                .is_some_and(|value| value.trim().is_empty())
        {
            return Err(D::Error::custom("effective timestamps must not be blank"));
        }
        raw.rule.validate().map_err(D::Error::custom)?;
        Ok(Self {
            scope,
            effective_from: raw.effective_from,
            effective_until: raw.effective_until,
            rule: raw.rule,
            expected_revision: raw.expected_revision,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationLifecycleRequest {
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollbackConfigurationRequest {
    pub expected_source_revision: Revision,
    pub effective_from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_until: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationResponse {
    pub configuration_id: i64,
    pub revision: Revision,
    pub scope: ConfigurationScope,
    pub status: ConfigurationStatus,
    pub effective_from: String,
    pub effective_until: Option<String>,
    pub rule: DecisionRule,
    pub created_by: i64,
    pub created_at: String,
    pub submitted_by: Option<i64>,
    pub submitted_at: Option<String>,
    pub approved_by: Option<i64>,
    pub approved_at: Option<String>,
    pub activated_by: Option<i64>,
    pub activated_at: Option<String>,
    pub retired_by: Option<i64>,
    pub retired_at: Option<String>,
    pub rollback_of_configuration_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationPage {
    pub items: Vec<ConfigurationResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<OpaqueCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationPageRequest {
    #[serde(default)]
    pub kind: Option<DecisionRuleKind>,
    #[serde(default)]
    pub status: Option<ConfigurationStatus>,
    #[serde(default)]
    pub inventory_owner_id: Option<i64>,
    #[serde(default)]
    pub facility_id: Option<i64>,
    #[serde(default)]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SimulateConfigurationRequest {
    pub kind: DecisionRuleKind,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub effective_at: String,
}

impl<'de> Deserialize<'de> for SimulateConfigurationRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawRequest {
            kind: DecisionRuleKind,
            inventory_owner_id: i64,
            facility_id: i64,
            effective_at: String,
        }
        let raw = RawRequest::deserialize(deserializer)?;
        if raw.inventory_owner_id <= 0 || raw.facility_id <= 0 {
            return Err(D::Error::custom("scope IDs must be positive"));
        }
        if raw.effective_at.trim().is_empty() {
            return Err(D::Error::custom("effective_at must not be blank"));
        }
        Ok(Self {
            kind: raw.kind,
            inventory_owner_id: raw.inventory_owner_id,
            facility_id: raw.facility_id,
            effective_at: raw.effective_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationSimulationResponse {
    pub kind: DecisionRuleKind,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub effective_at: String,
    pub matched_configuration: Option<ConfigurationResponse>,
    pub evaluated_candidate_count: u32,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn configuration_requests_are_strict_scoped_revisioned_and_typed() {
        let request = serde_json::from_value::<CreateConfigurationRequest>(json!({
            "scope": {
                "level": "owner_facility",
                "inventory_owner_id": 2,
                "facility_id": 3
            },
            "effective_from": "2026-09-01T00:00:00Z",
            "rule": {
                "kind": "allocation",
                "rotation": "fefo",
                "allow_partial": true,
                "require_complete_line": false
            }
        }))
        .unwrap();
        assert_eq!(request.rule.kind(), DecisionRuleKind::Allocation);

        assert!(serde_json::from_value::<CreateConfigurationRequest>(json!({
            "tenant_id": 1,
            "scope": {"level": "tenant"},
            "effective_from": "2026-09-01T00:00:00Z",
            "rule": {
                "kind": "wave",
                "max_orders": 100,
                "require_complete_allocation": true
            }
        }))
        .is_err());
        assert!(serde_json::from_value::<CreateConfigurationRequest>(json!({
            "scope": {"level": "facility", "facility_id": 0},
            "effective_from": "2026-09-01T00:00:00Z",
            "rule": {
                "kind": "wave",
                "max_orders": 100,
                "require_complete_allocation": true
            }
        }))
        .is_err());
    }

    #[test]
    fn incompatible_and_unbounded_rules_fail_at_the_contract_boundary() {
        for invalid in [
            json!({
                "kind": "allocation",
                "rotation": "fifo",
                "allow_partial": true,
                "require_complete_line": true
            }),
            json!({
                "kind": "replenishment",
                "minimum_percent": 90,
                "target_percent": 80,
                "include_inbound_projection": true
            }),
            json!({
                "kind": "billing",
                "event_type": "picked_unit",
                "unit": "each",
                "currency": "usd",
                "rate_minor": 25,
                "minimum_charge_minor": 0
            }),
        ] {
            let rule = serde_json::from_value::<DecisionRule>(invalid).unwrap();
            assert!(rule.validate().is_err());
        }
    }

    #[test]
    fn lifecycle_and_simulation_requests_exclude_server_owned_fields() {
        assert!(
            serde_json::from_value::<ConfigurationLifecycleRequest>(json!({
                "expected_revision": 2
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<ConfigurationLifecycleRequest>(json!({
                "expected_revision": 2,
                "activated_at": "2026-09-01T00:00:00Z"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<SimulateConfigurationRequest>(json!({
                "kind": "pick",
                "inventory_owner_id": 2,
                "facility_id": 3,
                "effective_at": "2026-09-01T00:00:00Z",
                "tenant_id": 1
            }))
            .is_err()
        );
    }
}
