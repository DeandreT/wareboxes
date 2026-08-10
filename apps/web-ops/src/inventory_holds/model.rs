use wareboxes_api_contract::v1::{InventoryBalanceResponse, InventoryHoldResponse};

use crate::sorting::{SortDirection, SortSpec};

use super::reason_label;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PositionSort {
    Item,
    Client,
    Facility,
    Location,
    Available,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HoldSort {
    Id,
    Item,
    Client,
    Position,
    Reason,
    Created,
    Quantity,
}

pub(super) fn item_label(balance: &InventoryBalanceResponse) -> String {
    balance
        .primary_sku
        .clone()
        .or_else(|| balance.item_description.clone())
        .unwrap_or_else(|| format!("Item #{}", balance.item_id))
}

pub(super) fn balance_item_detail(balance: &InventoryBalanceResponse) -> Option<String> {
    balance
        .primary_sku
        .as_ref()
        .and(balance.item_description.clone())
}

pub(super) fn sort_positions(
    balances: &mut [InventoryBalanceResponse],
    spec: SortSpec<PositionSort>,
) {
    balances.sort_by(|left, right| {
        let ordering = match spec.key {
            PositionSort::Item => item_label(left)
                .to_ascii_lowercase()
                .cmp(&item_label(right).to_ascii_lowercase()),
            PositionSort::Client => left
                .inventory_owner_name
                .to_ascii_lowercase()
                .cmp(&right.inventory_owner_name.to_ascii_lowercase()),
            PositionSort::Facility => facility_label(left)
                .to_ascii_lowercase()
                .cmp(&facility_label(right).to_ascii_lowercase())
                .then_with(|| {
                    location_label(left)
                        .to_ascii_lowercase()
                        .cmp(&location_label(right).to_ascii_lowercase())
                })
                .then_with(|| {
                    item_label(left)
                        .to_ascii_lowercase()
                        .cmp(&item_label(right).to_ascii_lowercase())
                }),
            PositionSort::Location => location_label(left)
                .to_ascii_lowercase()
                .cmp(&location_label(right).to_ascii_lowercase()),
            PositionSort::Available => left.quantity.available.cmp(&right.quantity.available),
        }
        .then_with(|| left.id.cmp(&right.id));
        if spec.direction == SortDirection::Ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
}

pub(super) fn sort_holds(holds: &mut [InventoryHoldResponse], spec: SortSpec<HoldSort>) {
    holds.sort_by(|left, right| {
        let ordering = match spec.key {
            HoldSort::Id => left.id.cmp(&right.id),
            HoldSort::Item => hold_item_label(left)
                .to_ascii_lowercase()
                .cmp(&hold_item_label(right).to_ascii_lowercase()),
            HoldSort::Client => left
                .inventory_owner_name
                .to_ascii_lowercase()
                .cmp(&right.inventory_owner_name.to_ascii_lowercase()),
            HoldSort::Position => hold_facility_label(left)
                .to_ascii_lowercase()
                .cmp(&hold_facility_label(right).to_ascii_lowercase())
                .then_with(|| {
                    hold_location_label(left)
                        .to_ascii_lowercase()
                        .cmp(&hold_location_label(right).to_ascii_lowercase())
                }),
            HoldSort::Reason => reason_label(left.reason).cmp(reason_label(right.reason)),
            HoldSort::Created => left.created_at.cmp(&right.created_at),
            HoldSort::Quantity => left.quantity.cmp(&right.quantity),
        }
        .then_with(|| left.id.cmp(&right.id));
        if spec.direction == SortDirection::Ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
}

pub(super) fn facility_label(balance: &InventoryBalanceResponse) -> String {
    balance
        .facility_name
        .clone()
        .unwrap_or_else(|| format!("Facility #{}", balance.facility_id))
}

pub(super) fn location_label(balance: &InventoryBalanceResponse) -> String {
    balance
        .location_barcode
        .clone()
        .or_else(|| balance.location_name.clone())
        .unwrap_or_else(|| format!("Location #{}", balance.location_id))
}

pub(super) fn balance_matches(balance: &InventoryBalanceResponse, query: &str) -> bool {
    query.is_empty()
        || [
            balance.inventory_owner_name.as_str(),
            balance.facility_name.as_deref().unwrap_or_default(),
            balance.location_name.as_deref().unwrap_or_default(),
            balance.location_barcode.as_deref().unwrap_or_default(),
            balance.license_plate_barcode.as_deref().unwrap_or_default(),
            balance.item_description.as_deref().unwrap_or_default(),
            balance.primary_sku.as_deref().unwrap_or_default(),
            balance.lot.as_deref().unwrap_or_default(),
            balance.serial.as_deref().unwrap_or_default(),
        ]
        .iter()
        .any(|value| value.to_ascii_lowercase().contains(query))
}

pub(super) fn hold_item_label(hold: &InventoryHoldResponse) -> String {
    hold.item_description
        .clone()
        .unwrap_or_else(|| format!("Item #{}", hold.item_id))
}

pub(super) fn hold_facility_label(hold: &InventoryHoldResponse) -> String {
    hold.facility_name
        .clone()
        .unwrap_or_else(|| format!("Facility #{}", hold.facility_id))
}

pub(super) fn hold_location_label(hold: &InventoryHoldResponse) -> String {
    hold.location_barcode
        .clone()
        .or_else(|| hold.location_name.clone())
        .unwrap_or_else(|| format!("Location #{}", hold.location_id))
}

pub(super) fn tracking_label(hold: &InventoryHoldResponse) -> Option<String> {
    match (&hold.lot, &hold.serial) {
        (Some(lot), Some(serial)) => Some(format!("Lot {lot} / Serial {serial}")),
        (Some(lot), None) => Some(format!("Lot {lot}")),
        (None, Some(serial)) => Some(format!("Serial {serial}")),
        (None, None) => hold
            .license_plate_barcode
            .as_ref()
            .map(|barcode| format!("LPN {barcode}")),
    }
}

pub(super) fn hold_matches(hold: &InventoryHoldResponse, query: &str) -> bool {
    query.is_empty()
        || [
            hold.inventory_owner_name.as_str(),
            hold.facility_name.as_deref().unwrap_or_default(),
            hold.location_name.as_deref().unwrap_or_default(),
            hold.location_barcode.as_deref().unwrap_or_default(),
            hold.license_plate_barcode.as_deref().unwrap_or_default(),
            hold.item_description.as_deref().unwrap_or_default(),
            hold.lot.as_deref().unwrap_or_default(),
            hold.serial.as_deref().unwrap_or_default(),
            hold.note.as_deref().unwrap_or_default(),
            reason_label(hold.reason),
        ]
        .iter()
        .any(|value| value.to_ascii_lowercase().contains(query))
        || hold.id.to_string().contains(query)
}
