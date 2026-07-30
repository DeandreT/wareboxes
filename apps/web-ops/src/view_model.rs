use std::collections::BTreeMap;

use wareboxes_api_contract::v1::InventoryBalanceResponse;
use wareboxes_core::dto::WebSessionContext;
use wareboxes_core::models::OrderStatus;

pub fn has_permission(session: &WebSessionContext, permission: &str) -> bool {
    session.user.user_permissions.iter().any(|candidate| {
        candidate.name.eq_ignore_ascii_case("admin")
            || candidate.name.eq_ignore_ascii_case(permission)
    })
}

pub fn user_name(session: &WebSessionContext) -> String {
    let parts = [
        session.user.first_name.as_deref(),
        session.user.last_name.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.trim().is_empty())
    .collect::<Vec<_>>();
    if parts.is_empty() {
        session.user.email.clone()
    } else {
        parts.join(" ")
    }
}

pub fn open_order_count(summaries: &[wareboxes_core::dto::SummaryCount]) -> i64 {
    summaries
        .iter()
        .filter(|summary| {
            matches!(
                OrderStatus::parse(&summary.key),
                Some(
                    OrderStatus::Open
                        | OrderStatus::Processing
                        | OrderStatus::Held
                        | OrderStatus::AwaitingShipment
                )
            )
        })
        .map(|summary| summary.count)
        .sum()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacilityInventory {
    pub facility_name: String,
    pub on_hand: i64,
    pub reserved: i64,
    pub held: i64,
    pub positions: usize,
}

pub fn facility_inventory(balances: &[InventoryBalanceResponse]) -> Vec<FacilityInventory> {
    let mut totals = BTreeMap::<i64, FacilityInventory>::new();
    for balance in balances {
        let total = totals
            .entry(balance.facility_id)
            .or_insert_with(|| FacilityInventory {
                facility_name: balance
                    .facility_name
                    .clone()
                    .unwrap_or_else(|| format!("Facility {}", balance.facility_id)),
                on_hand: 0,
                reserved: 0,
                held: 0,
                positions: 0,
            });
        total.on_hand += balance.quantity.on_hand;
        total.reserved += balance.quantity.reserved;
        total.held += balance.quantity.held;
        total.positions += 1;
    }
    totals.into_values().collect()
}

pub fn format_quantity(quantity: i64) -> String {
    let negative = quantity.is_negative();
    let digits = quantity.unsigned_abs().to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    if negative {
        format!("-{grouped}")
    } else {
        grouped
    }
}

#[cfg(test)]
mod tests {
    use super::format_quantity;

    #[test]
    fn formats_signed_quantities_for_operational_scanning() {
        assert_eq!(format_quantity(0), "0");
        assert_eq!(format_quantity(999), "999");
        assert_eq!(format_quantity(12_345_678), "12,345,678");
        assert_eq!(format_quantity(-4_200), "-4,200");
    }
}
