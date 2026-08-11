use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    InventoryBalancePage, InventoryBalanceResponse,
    InventoryBalanceSort as ApiInventoryBalanceSort, InventoryHoldPage, InventoryHoldResponse,
    InventoryHoldSort as ApiInventoryHoldSort, InventoryHoldStatus,
    InventorySortDirection as ApiInventorySortDirection, OpaqueCursor,
};

use crate::api;
use crate::sorting::{SortDirection, SortSpec};

use super::{HoldSort, PositionSort};

#[derive(Clone, Copy)]
pub(super) struct BalanceListSignals {
    pub balances: RwSignal<Vec<InventoryBalanceResponse>>,
    pub cursor: RwSignal<Option<OpaqueCursor>>,
    pub pending: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    pub generation: RwSignal<u64>,
    pub sort: RwSignal<SortSpec<PositionSort>>,
    pub on_unauthorized: Callback<()>,
}

#[derive(Clone, Copy)]
pub(super) struct HoldListSignals {
    pub holds: RwSignal<Vec<InventoryHoldResponse>>,
    pub cursor: RwSignal<Option<OpaqueCursor>>,
    pub status: RwSignal<InventoryHoldStatus>,
    pub pending: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    pub generation: RwSignal<u64>,
    pub sort: RwSignal<SortSpec<HoldSort>>,
    pub on_unauthorized: Callback<()>,
}

pub(super) fn request_balance_page(signals: BalanceListSignals, cursor: Option<OpaqueCursor>) {
    let append = cursor.is_some();
    let generation = signals.generation.get_untracked().wrapping_add(1);
    signals.generation.set(generation);
    signals.pending.set(true);
    signals.error.set(None);
    let sort = signals.sort.get_untracked();
    leptos::task::spawn_local(async move {
        let response = api::sorted_balances(
            None,
            map_position_sort(sort.key),
            map_direction(sort.direction),
            cursor.as_ref(),
        )
        .await;
        if signals.generation.get_untracked() != generation {
            return;
        }
        match response {
            Ok(page) => {
                if append {
                    signals
                        .balances
                        .update(|current| current.extend(page.items));
                } else {
                    signals.balances.set(page.items);
                }
                signals.cursor.set(page.next_cursor);
                signals.pending.set(false);
            }
            Err(error) if error.unauthorized => {
                signals.pending.set(false);
                signals.on_unauthorized.run(());
            }
            Err(error) => {
                signals.error.set(Some(error.message));
                signals.pending.set(false);
            }
        }
    });
}

pub(super) fn request_hold_page(
    signals: HoldListSignals,
    status: InventoryHoldStatus,
    cursor: Option<OpaqueCursor>,
) {
    let append = cursor.is_some();
    let generation = signals.generation.get_untracked().wrapping_add(1);
    signals.generation.set(generation);
    signals.pending.set(true);
    signals.error.set(None);
    let sort = signals.sort.get_untracked();
    leptos::task::spawn_local(async move {
        let response = api::sorted_holds(
            status,
            None,
            map_hold_sort(sort.key),
            map_direction(sort.direction),
            cursor.as_ref(),
        )
        .await;
        if signals.generation.get_untracked() != generation {
            return;
        }
        match response {
            Ok(page) => {
                if append {
                    signals.holds.update(|current| current.extend(page.items));
                } else {
                    signals.holds.set(page.items);
                }
                signals.cursor.set(page.next_cursor);
                signals.status.set(status);
                signals.pending.set(false);
            }
            Err(error) if error.unauthorized => {
                signals.pending.set(false);
                signals.on_unauthorized.run(());
            }
            Err(error) => {
                signals.error.set(Some(error.message));
                signals.pending.set(false);
            }
        }
    });
}

pub(super) async fn reload(
    status: InventoryHoldStatus,
    balance_sort: SortSpec<PositionSort>,
    hold_sort: SortSpec<HoldSort>,
) -> Result<(InventoryBalancePage, InventoryHoldPage), api::ApiError> {
    let balances = api::sorted_balances(
        None,
        map_position_sort(balance_sort.key),
        map_direction(balance_sort.direction),
        None,
    )
    .await?;
    let holds = api::sorted_holds(
        status,
        None,
        map_hold_sort(hold_sort.key),
        map_direction(hold_sort.direction),
        None,
    )
    .await?;
    Ok((balances, holds))
}

fn map_position_sort(sort: PositionSort) -> ApiInventoryBalanceSort {
    match sort {
        PositionSort::Item => ApiInventoryBalanceSort::Item,
        PositionSort::Client => ApiInventoryBalanceSort::Client,
        PositionSort::Facility => ApiInventoryBalanceSort::Facility,
        PositionSort::Location => ApiInventoryBalanceSort::Location,
        PositionSort::Available => ApiInventoryBalanceSort::Available,
    }
}

fn map_hold_sort(sort: HoldSort) -> ApiInventoryHoldSort {
    match sort {
        HoldSort::Id => ApiInventoryHoldSort::Id,
        HoldSort::Item => ApiInventoryHoldSort::Item,
        HoldSort::Client => ApiInventoryHoldSort::Client,
        HoldSort::Position => ApiInventoryHoldSort::Position,
        HoldSort::Reason => ApiInventoryHoldSort::Reason,
        HoldSort::Created => ApiInventoryHoldSort::Created,
        HoldSort::Quantity => ApiInventoryHoldSort::Quantity,
    }
}

fn map_direction(direction: SortDirection) -> ApiInventorySortDirection {
    match direction {
        SortDirection::Ascending => ApiInventorySortDirection::Ascending,
        SortDirection::Descending => ApiInventorySortDirection::Descending,
    }
}
