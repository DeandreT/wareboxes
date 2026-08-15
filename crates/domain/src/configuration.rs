use chrono::{DateTime, Utc};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use crate::{FacilityId, InventoryOwnerId, Timestamp};

pub const MAX_CONFIGURATION_CURRENCY_LENGTH: usize = 3;
pub const MAX_CONFIGURATION_RATE_MINOR: u64 = 1_000_000_000_000;
pub const MAX_WAVE_ORDERS: u32 = 10_000;
pub const MAX_PERCENTAGE_BASIS_POINTS: u16 = 10_000;

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
#[serde(rename_all = "snake_case", tag = "level")]
pub enum ConfigurationScope {
    Tenant,
    InventoryOwner {
        inventory_owner_id: InventoryOwnerId,
    },
    Facility {
        facility_id: FacilityId,
    },
    OwnerFacility {
        inventory_owner_id: InventoryOwnerId,
        facility_id: FacilityId,
    },
}

impl ConfigurationScope {
    pub const fn specificity(self) -> u8 {
        match self {
            Self::Tenant => 0,
            Self::InventoryOwner { .. } | Self::Facility { .. } => 1,
            Self::OwnerFacility { .. } => 2,
        }
    }

    pub const fn applies_to(
        self,
        inventory_owner_id: InventoryOwnerId,
        facility_id: FacilityId,
    ) -> bool {
        match self {
            Self::Tenant => true,
            Self::InventoryOwner {
                inventory_owner_id: configured,
            } => configured.get() == inventory_owner_id.get(),
            Self::Facility {
                facility_id: configured,
            } => configured.get() == facility_id.get(),
            Self::OwnerFacility {
                inventory_owner_id: configured_owner,
                facility_id: configured_facility,
            } => {
                configured_owner.get() == inventory_owner_id.get()
                    && configured_facility.get() == facility_id.get()
            }
        }
    }
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

impl ConfigurationStatus {
    pub const fn submit(self) -> Result<Self, ConfigurationError> {
        match self {
            Self::Draft => Ok(Self::PendingApproval),
            actual => Err(ConfigurationError::InvalidTransition {
                actual,
                requested: Self::PendingApproval,
            }),
        }
    }

    pub const fn approve(self) -> Result<Self, ConfigurationError> {
        match self {
            Self::PendingApproval => Ok(Self::Approved),
            actual => Err(ConfigurationError::InvalidTransition {
                actual,
                requested: Self::Approved,
            }),
        }
    }

    pub fn activate(
        self,
        effective_window: ConfigurationEffectiveWindow,
        activated_at: Timestamp,
    ) -> Result<Self, ConfigurationError> {
        if self != Self::Approved {
            return Err(ConfigurationError::InvalidTransition {
                actual: self,
                requested: Self::Active,
            });
        }
        if !effective_window.includes(activated_at) {
            return Err(ConfigurationError::OutsideEffectiveWindow);
        }
        Ok(Self::Active)
    }

    pub const fn retire(self) -> Result<Self, ConfigurationError> {
        match self {
            Self::Approved | Self::Active => Ok(Self::Retired),
            actual => Err(ConfigurationError::InvalidTransition {
                actual,
                requested: Self::Retired,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ConfigurationEffectiveWindow {
    pub effective_from: Timestamp,
    pub effective_until: Option<Timestamp>,
}

impl ConfigurationEffectiveWindow {
    pub fn new(
        effective_from: Timestamp,
        effective_until: Option<Timestamp>,
    ) -> Result<Self, ConfigurationError> {
        if effective_until.is_some_and(|until| until <= effective_from) {
            return Err(ConfigurationError::InvalidEffectiveWindow);
        }
        Ok(Self {
            effective_from,
            effective_until,
        })
    }

    pub fn includes(self, timestamp: Timestamp) -> bool {
        timestamp >= self.effective_from
            && self
                .effective_until
                .is_none_or(|effective_until| timestamp < effective_until)
    }
}

impl<'de> Deserialize<'de> for ConfigurationEffectiveWindow {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawWindow {
            effective_from: DateTime<Utc>,
            effective_until: Option<DateTime<Utc>>,
        }

        let raw = RawWindow::deserialize(deserializer)?;
        Self::new(raw.effective_from, raw.effective_until).map_err(D::Error::custom)
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
pub enum DecisionRuleDefinition {
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

impl DecisionRuleDefinition {
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

    pub fn validate(&self) -> Result<(), ConfigurationError> {
        match self {
            Self::Receipt {
                over_receipt_tolerance_basis_points,
                ..
            } if *over_receipt_tolerance_basis_points > MAX_PERCENTAGE_BASIS_POINTS => {
                Err(ConfigurationError::InvalidPercentage)
            }
            Self::Allocation {
                allow_partial,
                require_complete_line,
                ..
            } if *allow_partial && *require_complete_line => {
                Err(ConfigurationError::ConflictingAllocationPolicy)
            }
            Self::Replenishment {
                minimum_percent,
                target_percent,
                ..
            } if *minimum_percent > 100
                || *target_percent > 100
                || minimum_percent >= target_percent =>
            {
                Err(ConfigurationError::InvalidReplenishmentLevels)
            }
            Self::Wave { max_orders, .. } if *max_orders == 0 || *max_orders > MAX_WAVE_ORDERS => {
                Err(ConfigurationError::InvalidWaveSize)
            }
            Self::Count {
                absolute_tolerance,
                percentage_tolerance_basis_points,
                approval_threshold,
            } if *absolute_tolerance < 0
                || *percentage_tolerance_basis_points > MAX_PERCENTAGE_BASIS_POINTS
                || *approval_threshold < *absolute_tolerance =>
            {
                Err(ConfigurationError::InvalidCountTolerance)
            }
            Self::Document {
                generate_packing_slip,
                generate_carton_label,
                ..
            } if !generate_packing_slip && !generate_carton_label => {
                Err(ConfigurationError::EmptyDocumentPolicy)
            }
            Self::Billing {
                currency,
                rate_minor,
                minimum_charge_minor,
                ..
            } if !valid_currency(currency)
                || *rate_minor == 0
                || *rate_minor > MAX_CONFIGURATION_RATE_MINOR
                || *minimum_charge_minor > MAX_CONFIGURATION_RATE_MINOR =>
            {
                Err(ConfigurationError::InvalidBillingRate)
            }
            _ => Ok(()),
        }
    }
}

fn valid_currency(currency: &str) -> bool {
    currency.len() == MAX_CONFIGURATION_CURRENCY_LENGTH
        && currency.bytes().all(|byte| byte.is_ascii_uppercase())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveDecisionRule {
    pub configuration_id: i64,
    pub revision: i64,
    pub scope: ConfigurationScope,
    pub status: ConfigurationStatus,
    pub effective_window: ConfigurationEffectiveWindow,
    pub definition: DecisionRuleDefinition,
}

impl EffectiveDecisionRule {
    pub fn validate(&self) -> Result<(), ConfigurationError> {
        if self.configuration_id <= 0 || self.revision <= 0 {
            return Err(ConfigurationError::InvalidIdentity);
        }
        self.definition.validate()
    }
}

pub fn resolve_effective_rule(
    candidates: &[EffectiveDecisionRule],
    kind: DecisionRuleKind,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    effective_at: Timestamp,
) -> Result<Option<&EffectiveDecisionRule>, ConfigurationError> {
    for candidate in candidates {
        candidate.validate()?;
    }
    Ok(candidates
        .iter()
        .filter(|candidate| {
            candidate.status == ConfigurationStatus::Active
                && candidate.definition.kind() == kind
                && candidate.scope.applies_to(inventory_owner_id, facility_id)
                && candidate.effective_window.includes(effective_at)
        })
        .max_by_key(|candidate| {
            (
                candidate.scope.specificity(),
                candidate.effective_window.effective_from,
                candidate.revision,
                candidate.configuration_id,
            )
        }))
}

pub fn rollback_as_draft(
    source: &EffectiveDecisionRule,
    next_configuration_id: i64,
    next_revision: i64,
    effective_window: ConfigurationEffectiveWindow,
) -> Result<EffectiveDecisionRule, ConfigurationError> {
    source.validate()?;
    if !matches!(
        source.status,
        ConfigurationStatus::Approved | ConfigurationStatus::Active | ConfigurationStatus::Retired
    ) {
        return Err(ConfigurationError::RollbackSourceNotApproved);
    }
    let rollback = EffectiveDecisionRule {
        configuration_id: next_configuration_id,
        revision: next_revision,
        scope: source.scope,
        status: ConfigurationStatus::Draft,
        effective_window,
        definition: source.definition.clone(),
    };
    rollback.validate()?;
    Ok(rollback)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ConfigurationError {
    #[error("configuration identity and revision must be positive")]
    InvalidIdentity,
    #[error("effective_until must be later than effective_from")]
    InvalidEffectiveWindow,
    #[error("the configuration is not effective at the requested activation time")]
    OutsideEffectiveWindow,
    #[error("configuration cannot transition from {actual:?} to {requested:?}")]
    InvalidTransition {
        actual: ConfigurationStatus,
        requested: ConfigurationStatus,
    },
    #[error("percentage basis points must be between zero and 10000")]
    InvalidPercentage,
    #[error("partial allocation conflicts with complete-line allocation")]
    ConflictingAllocationPolicy,
    #[error("replenishment minimum must be below a target no greater than 100")]
    InvalidReplenishmentLevels,
    #[error("wave size is outside the supported range")]
    InvalidWaveSize,
    #[error("count tolerances or approval threshold are invalid")]
    InvalidCountTolerance,
    #[error("document policy must generate at least one document")]
    EmptyDocumentPolicy,
    #[error("billing currency, rate, or minimum is invalid")]
    InvalidBillingRate,
    #[error("only an approved historical configuration can be rolled back")]
    RollbackSourceNotApproved,
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn timestamp(day: u32) -> Timestamp {
        Utc.with_ymd_and_hms(2026, 8, day, 12, 0, 0)
            .single()
            .unwrap()
    }

    fn owner(value: i64) -> InventoryOwnerId {
        InventoryOwnerId::new(value).unwrap()
    }

    fn facility(value: i64) -> FacilityId {
        FacilityId::new(value).unwrap()
    }

    fn rule(
        configuration_id: i64,
        revision: i64,
        scope: ConfigurationScope,
        effective_from: u32,
    ) -> EffectiveDecisionRule {
        EffectiveDecisionRule {
            configuration_id,
            revision,
            scope,
            status: ConfigurationStatus::Active,
            effective_window: ConfigurationEffectiveWindow::new(timestamp(effective_from), None)
                .unwrap(),
            definition: DecisionRuleDefinition::Allocation {
                rotation: InventoryRotation::Fefo,
                allow_partial: true,
                require_complete_line: false,
            },
        }
    }

    #[test]
    fn inheritance_selects_owner_facility_then_latest_effective_revision() {
        let owner = owner(10);
        let facility = facility(20);
        let candidates = vec![
            rule(1, 1, ConfigurationScope::Tenant, 1),
            rule(
                2,
                1,
                ConfigurationScope::InventoryOwner {
                    inventory_owner_id: owner,
                },
                1,
            ),
            rule(
                3,
                1,
                ConfigurationScope::OwnerFacility {
                    inventory_owner_id: owner,
                    facility_id: facility,
                },
                1,
            ),
            rule(
                4,
                2,
                ConfigurationScope::OwnerFacility {
                    inventory_owner_id: owner,
                    facility_id: facility,
                },
                2,
            ),
        ];

        let resolved = resolve_effective_rule(
            &candidates,
            DecisionRuleKind::Allocation,
            owner,
            facility,
            timestamp(3),
        )
        .unwrap()
        .unwrap();
        assert_eq!(resolved.configuration_id, 4);
    }

    #[test]
    fn inactive_expired_and_foreign_rules_never_resolve() {
        let mut expired = rule(1, 1, ConfigurationScope::Tenant, 1);
        expired.effective_window =
            ConfigurationEffectiveWindow::new(timestamp(1), Some(timestamp(2))).unwrap();
        let mut draft = rule(2, 1, ConfigurationScope::Tenant, 1);
        draft.status = ConfigurationStatus::Draft;
        let foreign = rule(
            3,
            1,
            ConfigurationScope::InventoryOwner {
                inventory_owner_id: owner(99),
            },
            1,
        );
        assert!(resolve_effective_rule(
            &[expired, draft, foreign],
            DecisionRuleKind::Allocation,
            owner(10),
            facility(20),
            timestamp(3),
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn invalid_typed_rules_fail_before_promotion_or_simulation() {
        let invalid_rules = [
            DecisionRuleDefinition::Replenishment {
                minimum_percent: 90,
                target_percent: 80,
                include_inbound_projection: true,
            },
            DecisionRuleDefinition::Allocation {
                rotation: InventoryRotation::Fifo,
                allow_partial: true,
                require_complete_line: true,
            },
            DecisionRuleDefinition::Billing {
                event_type: BillableEventType::PickedUnit,
                unit: BillingUnit::Each,
                currency: "usd".into(),
                rate_minor: 25,
                minimum_charge_minor: 0,
            },
        ];
        assert!(invalid_rules.iter().all(|rule| rule.validate().is_err()));
    }

    #[test]
    fn approval_activation_retirement_and_rollback_are_explicit() {
        let window = ConfigurationEffectiveWindow::new(timestamp(1), None).unwrap();
        let submitted = ConfigurationStatus::Draft.submit().unwrap();
        let approved = submitted.approve().unwrap();
        let active = approved.activate(window, timestamp(2)).unwrap();
        assert_eq!(active.retire().unwrap(), ConfigurationStatus::Retired);
        assert!(ConfigurationStatus::Draft.approve().is_err());

        let source = rule(7, 3, ConfigurationScope::Tenant, 1);
        let rollback = rollback_as_draft(&source, 8, 4, window).unwrap();
        assert_eq!(rollback.status, ConfigurationStatus::Draft);
        assert_eq!(rollback.definition, source.definition);
        assert_eq!(rollback.configuration_id, 8);
    }

    #[test]
    fn effective_windows_are_half_open_and_deserialization_revalidates_them() {
        let window = ConfigurationEffectiveWindow::new(timestamp(1), Some(timestamp(3))).unwrap();
        assert!(window.includes(timestamp(1)));
        assert!(!window.includes(timestamp(3)));
        assert!(
            serde_json::from_value::<ConfigurationEffectiveWindow>(serde_json::json!({
                "effective_from": "2026-08-03T12:00:00Z",
                "effective_until": "2026-08-02T12:00:00Z"
            }))
            .is_err()
        );
    }
}
