use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{
    ActualPickQuantity, CatalogItemId, OrderRevision, OrderStatus, PickShortageRevision,
    PickShortageStatus, MAX_REQUESTED_UOM_LENGTH,
};

pub const MAX_ITEM_SUBSTITUTION_NOTE_LENGTH: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SubstitutionUom(String);

impl SubstitutionUom {
    pub fn new(value: impl Into<String>) -> Result<Self, ItemSubstitutionError> {
        let value = value.into();
        if value.is_empty()
            || value.trim() != value
            || value.chars().count() > MAX_REQUESTED_UOM_LENGTH
            || value.chars().any(char::is_control)
        {
            return Err(ItemSubstitutionError::InvalidUom);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SubstitutionUom {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SubstitutionUom {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ItemSubstitutionPolicyRevision(i64);

impl ItemSubstitutionPolicyRevision {
    pub const fn new(value: i64) -> Result<Self, ItemSubstitutionError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(ItemSubstitutionError::InvalidRevision { value })
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

impl<'de> Deserialize<'de> for ItemSubstitutionPolicyRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SubstitutionQuantity(i64);

impl SubstitutionQuantity {
    pub const fn new(value: i64) -> Result<Self, ItemSubstitutionError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(ItemSubstitutionError::InvalidQuantity { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for SubstitutionQuantity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemSubstitutionDefinition {
    pub source_item_id: CatalogItemId,
    pub source_uom: SubstitutionUom,
    pub substitute_item_id: CatalogItemId,
    pub substitute_uom: SubstitutionUom,
    pub source_quantity: SubstitutionQuantity,
    pub substitute_quantity: SubstitutionQuantity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemSubstitutionReason {
    ClientAuthorized,
    InventoryUnavailable,
    ServiceRecovery,
    Other,
}

impl ItemSubstitutionReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClientAuthorized => "client_authorized",
            Self::InventoryUnavailable => "inventory_unavailable",
            Self::ServiceRecovery => "service_recovery",
            Self::Other => "other",
        }
    }

    pub const fn requires_note(self) -> bool {
        matches!(self, Self::Other)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ItemSubstitutionNote(String);

impl ItemSubstitutionNote {
    pub fn new(value: impl Into<String>) -> Result<Self, ItemSubstitutionError> {
        let value = value.into();
        if value.is_empty()
            || value.trim() != value
            || value.chars().count() > MAX_ITEM_SUBSTITUTION_NOTE_LENGTH
            || value.chars().any(char::is_control)
        {
            return Err(ItemSubstitutionError::InvalidNote);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ItemSubstitutionNote {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemSubstitutionDetails {
    pub reason: ItemSubstitutionReason,
    pub note: Option<ItemSubstitutionNote>,
}

impl ItemSubstitutionDetails {
    pub fn new(
        reason: ItemSubstitutionReason,
        note: Option<ItemSubstitutionNote>,
    ) -> Result<Self, ItemSubstitutionError> {
        if reason.requires_note() && note.is_none() {
            return Err(ItemSubstitutionError::OtherRequiresNote);
        }
        Ok(Self { reason, note })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubstitutePickShortageTransition {
    pub accepted_source_quantity: SubstitutionQuantity,
    pub substitute_quantity: SubstitutionQuantity,
    pub shortage_revision: PickShortageRevision,
    pub order_revision: OrderRevision,
}

#[allow(clippy::too_many_arguments)]
pub fn substitute_pick_shortage(
    status: PickShortageStatus,
    order_status: OrderStatus,
    shortage_revision: PickShortageRevision,
    order_revision: OrderRevision,
    short_quantity: i64,
    reallocated_quantity: ActualPickQuantity,
    recovery_terminal_quantity: ActualPickQuantity,
    remaining_quantity: ActualPickQuantity,
    source_item_id: CatalogItemId,
    source_uom: &SubstitutionUom,
    definition: &ItemSubstitutionDefinition,
) -> Result<SubstitutePickShortageTransition, ItemSubstitutionError> {
    if status != PickShortageStatus::AwaitingInventory {
        return Err(ItemSubstitutionError::ShortageNotAwaitingInventory);
    }
    if order_status != OrderStatus::Processing {
        return Err(ItemSubstitutionError::OrderNotProcessing);
    }
    if remaining_quantity.get() <= 0
        || reallocated_quantity != recovery_terminal_quantity
        || reallocated_quantity
            .get()
            .checked_add(remaining_quantity.get())
            != Some(short_quantity)
    {
        return Err(ItemSubstitutionError::InconsistentShortageQuantities);
    }
    if source_item_id != definition.source_item_id || source_uom != &definition.source_uom {
        return Err(ItemSubstitutionError::PolicySourceMismatch);
    }
    let accepted_source_quantity = SubstitutionQuantity::new(remaining_quantity.get())?;
    let substitute_quantity = definition.substitute_for(accepted_source_quantity)?;
    Ok(SubstitutePickShortageTransition {
        accepted_source_quantity,
        substitute_quantity,
        shortage_revision: shortage_revision
            .checked_next()
            .ok_or(ItemSubstitutionError::RevisionOverflow)?,
        order_revision: order_revision
            .checked_next()
            .ok_or(ItemSubstitutionError::RevisionOverflow)?,
    })
}

impl ItemSubstitutionDefinition {
    pub fn new(
        source_item_id: CatalogItemId,
        source_uom: SubstitutionUom,
        substitute_item_id: CatalogItemId,
        substitute_uom: SubstitutionUom,
        source_quantity: SubstitutionQuantity,
        substitute_quantity: SubstitutionQuantity,
    ) -> Result<Self, ItemSubstitutionError> {
        if source_item_id == substitute_item_id && source_uom == substitute_uom {
            return Err(ItemSubstitutionError::SameItemAndUom);
        }
        Ok(Self {
            source_item_id,
            source_uom,
            substitute_item_id,
            substitute_uom,
            source_quantity,
            substitute_quantity,
        })
    }

    pub fn substitute_for(
        &self,
        source_quantity: SubstitutionQuantity,
    ) -> Result<SubstitutionQuantity, ItemSubstitutionError> {
        let scaled = source_quantity
            .get()
            .checked_mul(self.substitute_quantity.get())
            .ok_or(ItemSubstitutionError::QuantityOverflow)?;
        if scaled % self.source_quantity.get() != 0 {
            return Err(ItemSubstitutionError::InexactConversion);
        }
        SubstitutionQuantity::new(scaled / self.source_quantity.get())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemSubstitutionError {
    InvalidRevision { value: i64 },
    InvalidQuantity { value: i64 },
    InvalidUom,
    InvalidNote,
    OtherRequiresNote,
    SameItemAndUom,
    InexactConversion,
    QuantityOverflow,
    ShortageNotAwaitingInventory,
    OrderNotProcessing,
    InconsistentShortageQuantities,
    PolicySourceMismatch,
    RevisionOverflow,
}

impl fmt::Display for ItemSubstitutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRevision { value } => {
                write!(
                    formatter,
                    "item substitution revision must be positive, got {value}"
                )
            }
            Self::InvalidQuantity { value } => {
                write!(
                    formatter,
                    "substitution quantity must be positive, got {value}"
                )
            }
            Self::InvalidUom => formatter.write_str("substitution UOM is invalid"),
            Self::InvalidNote => formatter.write_str("item substitution note is invalid"),
            Self::OtherRequiresNote => {
                formatter.write_str("other substitution reason requires a note")
            }
            Self::SameItemAndUom => {
                formatter.write_str("source and substitute item/UOM must differ")
            }
            Self::InexactConversion => {
                formatter.write_str("substitution conversion must produce a whole quantity")
            }
            Self::QuantityOverflow => formatter.write_str("substitution quantity overflow"),
            Self::ShortageNotAwaitingInventory => {
                formatter.write_str("pick shortage must be awaiting inventory")
            }
            Self::OrderNotProcessing => formatter.write_str("order must be processing"),
            Self::InconsistentShortageQuantities => {
                formatter.write_str("pick shortage recovery quantities are inconsistent")
            }
            Self::PolicySourceMismatch => {
                formatter.write_str("substitution policy does not match the shortage item")
            }
            Self::RevisionOverflow => formatter.write_str("substitution revision overflow"),
        }
    }
}

impl std::error::Error for ItemSubstitutionError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(source: i64, substitute: i64) -> ItemSubstitutionDefinition {
        ItemSubstitutionDefinition::new(
            CatalogItemId::new(1).unwrap(),
            SubstitutionUom::new("case").unwrap(),
            CatalogItemId::new(2).unwrap(),
            SubstitutionUom::new("each").unwrap(),
            SubstitutionQuantity::new(source).unwrap(),
            SubstitutionQuantity::new(substitute).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn conversion_is_exact_and_overflow_safe() {
        assert_eq!(
            definition(2, 3)
                .substitute_for(SubstitutionQuantity::new(4).unwrap())
                .unwrap()
                .get(),
            6
        );
        assert_eq!(
            definition(2, 3).substitute_for(SubstitutionQuantity::new(3).unwrap()),
            Err(ItemSubstitutionError::InexactConversion)
        );
    }

    #[test]
    fn identity_policy_is_rejected() {
        let result = ItemSubstitutionDefinition::new(
            CatalogItemId::new(1).unwrap(),
            SubstitutionUom::new("each").unwrap(),
            CatalogItemId::new(1).unwrap(),
            SubstitutionUom::new("each").unwrap(),
            SubstitutionQuantity::new(1).unwrap(),
            SubstitutionQuantity::new(1).unwrap(),
        );
        assert_eq!(result, Err(ItemSubstitutionError::SameItemAndUom));
    }
}
