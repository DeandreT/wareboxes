//! Effective Billing decision evidence frozen with each reconciliation charge.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wareboxes_domain::{
    BillableEventType, BillingRateDefinition, BillingRateId, BillingUnit, ConfigurationScope,
    ConfigurationVersionId, CurrencyCode, DecisionRuleDefinition,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingDecisionPolicySource {
    ContractRate,
    Configuration,
}

impl BillingDecisionPolicySource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ContractRate => "contract_rate",
            Self::Configuration => "configuration",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingDecisionPolicyReadModel {
    pub source: BillingDecisionPolicySource,
    pub contract_rate_id: Option<BillingRateId>,
    pub contract_rate_revision: Option<i64>,
    pub configuration_id: Option<ConfigurationVersionId>,
    pub configuration_revision: Option<i64>,
    pub configuration_scope: Option<ConfigurationScope>,
    pub event_type: BillableEventType,
    pub unit: BillingUnit,
    pub currency: String,
    pub rate_minor: u64,
    pub minimum_charge_minor: u64,
    pub policy_hash: String,
}

impl BillingDecisionPolicyReadModel {
    pub fn contract_rate(
        rate_id: BillingRateId,
        rate_revision: i64,
        definition: &BillingRateDefinition,
    ) -> Result<Self, BillingDecisionPolicyError> {
        if rate_revision <= 0 {
            return Err(BillingDecisionPolicyError::InvalidRateRevision);
        }
        Ok(Self::new(
            BillingDecisionPolicySource::ContractRate,
            Some(rate_id),
            Some(rate_revision),
            None,
            None,
            None,
            definition.event_type,
            definition.unit,
            definition.currency.as_str().to_owned(),
            definition.rate_minor,
            definition.minimum_charge_minor,
        ))
    }

    pub fn from_configuration(
        configuration_id: ConfigurationVersionId,
        configuration_revision: i64,
        configuration_scope: ConfigurationScope,
        definition: &DecisionRuleDefinition,
    ) -> Result<Self, BillingDecisionPolicyError> {
        if configuration_revision <= 0 {
            return Err(BillingDecisionPolicyError::InvalidConfigurationRevision);
        }
        let DecisionRuleDefinition::Billing {
            event_type,
            unit,
            ref currency,
            rate_minor,
            minimum_charge_minor,
        } = *definition
        else {
            return Err(BillingDecisionPolicyError::WrongConfigurationKind);
        };
        definition
            .validate()
            .map_err(BillingDecisionPolicyError::InvalidConfiguration)?;
        Ok(Self::new(
            BillingDecisionPolicySource::Configuration,
            None,
            None,
            Some(configuration_id),
            Some(configuration_revision),
            Some(configuration_scope),
            event_type,
            unit,
            currency.clone(),
            rate_minor,
            minimum_charge_minor,
        ))
    }

    pub fn rate_definition(&self) -> Result<BillingRateDefinition, BillingDecisionPolicyError> {
        BillingRateDefinition::new(
            self.event_type,
            self.unit,
            CurrencyCode::new(self.currency.clone())
                .map_err(BillingDecisionPolicyError::InvalidRate)?,
            self.rate_minor,
            self.minimum_charge_minor,
        )
        .map_err(BillingDecisionPolicyError::InvalidRate)
    }

    pub fn is_consistent(&self) -> bool {
        let identity_valid = match self.source {
            BillingDecisionPolicySource::ContractRate => {
                self.contract_rate_id.is_some()
                    && self
                        .contract_rate_revision
                        .is_some_and(|revision| revision > 0)
                    && self.configuration_id.is_none()
                    && self.configuration_revision.is_none()
                    && self.configuration_scope.is_none()
            }
            BillingDecisionPolicySource::Configuration => {
                self.contract_rate_id.is_none()
                    && self.contract_rate_revision.is_none()
                    && self.configuration_id.is_some()
                    && self
                        .configuration_revision
                        .is_some_and(|revision| revision > 0)
                    && self.configuration_scope.is_some()
            }
        };
        identity_valid
            && self.rate_definition().is_ok()
            && self.policy_hash
                == billing_decision_policy_hash(
                    self.source,
                    self.contract_rate_id,
                    self.contract_rate_revision,
                    self.configuration_id,
                    self.configuration_revision,
                    self.configuration_scope,
                    self.event_type,
                    self.unit,
                    &self.currency,
                    self.rate_minor,
                    self.minimum_charge_minor,
                )
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        source: BillingDecisionPolicySource,
        contract_rate_id: Option<BillingRateId>,
        contract_rate_revision: Option<i64>,
        configuration_id: Option<ConfigurationVersionId>,
        configuration_revision: Option<i64>,
        configuration_scope: Option<ConfigurationScope>,
        event_type: BillableEventType,
        unit: BillingUnit,
        currency: String,
        rate_minor: u64,
        minimum_charge_minor: u64,
    ) -> Self {
        let policy_hash = billing_decision_policy_hash(
            source,
            contract_rate_id,
            contract_rate_revision,
            configuration_id,
            configuration_revision,
            configuration_scope,
            event_type,
            unit,
            &currency,
            rate_minor,
            minimum_charge_minor,
        );
        Self {
            source,
            contract_rate_id,
            contract_rate_revision,
            configuration_id,
            configuration_revision,
            configuration_scope,
            event_type,
            unit,
            currency,
            rate_minor,
            minimum_charge_minor,
            policy_hash,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn billing_decision_policy_hash(
    source: BillingDecisionPolicySource,
    contract_rate_id: Option<BillingRateId>,
    contract_rate_revision: Option<i64>,
    configuration_id: Option<ConfigurationVersionId>,
    configuration_revision: Option<i64>,
    configuration_scope: Option<ConfigurationScope>,
    event_type: BillableEventType,
    unit: BillingUnit,
    currency: &str,
    rate_minor: u64,
    minimum_charge_minor: u64,
) -> String {
    let canonical = format!(
        "billing-decision-policy-v1|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        source.as_str(),
        optional_id(contract_rate_id.map(BillingRateId::get)),
        optional_id(contract_rate_revision),
        optional_id(configuration_id.map(ConfigurationVersionId::get)),
        optional_id(configuration_revision),
        configuration_scope.map_or_else(|| "-".to_owned(), scope_component),
        event_name(event_type),
        unit_name(unit),
        currency,
        rate_minor,
        minimum_charge_minor,
    );
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

fn optional_id(value: Option<i64>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| value.to_string())
}

fn scope_component(scope: ConfigurationScope) -> String {
    match scope {
        ConfigurationScope::Tenant => "tenant".to_owned(),
        ConfigurationScope::InventoryOwner { inventory_owner_id } => {
            format!("inventory_owner:{}", inventory_owner_id.get())
        }
        ConfigurationScope::Facility { facility_id } => {
            format!("facility:{}", facility_id.get())
        }
        ConfigurationScope::OwnerFacility {
            inventory_owner_id,
            facility_id,
        } => format!(
            "owner_facility:{}:{}",
            inventory_owner_id.get(),
            facility_id.get()
        ),
    }
}

pub const fn event_name(value: BillableEventType) -> &'static str {
    match value {
        BillableEventType::ReceiptLine => "receipt_line",
        BillableEventType::ReceivedUnit => "received_unit",
        BillableEventType::PalletDay => "pallet_day",
        BillableEventType::PickLine => "pick_line",
        BillableEventType::PickedUnit => "picked_unit",
        BillableEventType::PackedCarton => "packed_carton",
        BillableEventType::ShippedUnit => "shipped_unit",
        BillableEventType::ReturnUnit => "return_unit",
        BillableEventType::RelabelUnit => "relabel_unit",
        BillableEventType::RefurbishmentUnit => "refurbishment_unit",
        BillableEventType::KitUnit => "kit_unit",
        BillableEventType::AssemblyUnit => "assembly_unit",
        BillableEventType::Accessorial => "accessorial",
        BillableEventType::DetentionHour => "detention_hour",
        BillableEventType::ValueAddedServiceUnit => "value_added_service_unit",
    }
}

pub const fn unit_name(value: BillingUnit) -> &'static str {
    match value {
        BillingUnit::Event => "event",
        BillingUnit::Each => "each",
        BillingUnit::Case => "case",
        BillingUnit::Pallet => "pallet",
        BillingUnit::Carton => "carton",
        BillingUnit::Hour => "hour",
        BillingUnit::Day => "day",
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BillingDecisionPolicyError {
    #[error("resolved configuration is not a Billing rule")]
    WrongConfigurationKind,
    #[error("resolved Billing configuration revision is invalid")]
    InvalidConfigurationRevision,
    #[error("resolved contract billing rate revision is invalid")]
    InvalidRateRevision,
    #[error("resolved Billing configuration is invalid: {0}")]
    InvalidConfiguration(wareboxes_domain::ConfigurationError),
    #[error("effective Billing rate is invalid: {0}")]
    InvalidRate(wareboxes_domain::BillingError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_domain::{FacilityId, InventoryOwnerId};

    #[test]
    fn contract_and_configuration_sources_have_distinct_stable_hashes() {
        let definition = BillingRateDefinition::new(
            BillableEventType::Accessorial,
            BillingUnit::Event,
            CurrencyCode::new("USD".into()).unwrap(),
            250,
            500,
        )
        .unwrap();
        let contract = BillingDecisionPolicyReadModel::contract_rate(
            BillingRateId::new(7).unwrap(),
            2,
            &definition,
        )
        .unwrap();
        let configuration = BillingDecisionPolicyReadModel::from_configuration(
            ConfigurationVersionId::new(9).unwrap(),
            3,
            ConfigurationScope::OwnerFacility {
                inventory_owner_id: InventoryOwnerId::new(11).unwrap(),
                facility_id: FacilityId::new(13).unwrap(),
            },
            &DecisionRuleDefinition::Billing {
                event_type: BillableEventType::Accessorial,
                unit: BillingUnit::Event,
                currency: "USD".into(),
                rate_minor: 250,
                minimum_charge_minor: 500,
            },
        )
        .unwrap();
        assert!(contract.is_consistent());
        assert!(configuration.is_consistent());
        assert_ne!(contract.policy_hash, configuration.policy_hash);
    }
}
