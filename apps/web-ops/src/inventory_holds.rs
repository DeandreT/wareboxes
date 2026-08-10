use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    InboundInspectionOutcome, InventoryBalanceResponse, InventoryHoldReason, InventoryHoldResponse,
    InventoryHoldStatus, OpaqueCursor, PlaceInventoryHoldRequest,
};

use crate::api;
use crate::components::{Icon, SearchField, UiIcon};
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;

#[path = "inventory_holds/model.rs"]
mod model;
use model::{
    balance_item_detail, balance_matches, facility_label, hold_facility_label, hold_item_label,
    hold_location_label, hold_matches, item_label, location_label, sort_holds, sort_positions,
    tracking_label, HoldSort, PositionSort,
};
#[path = "inventory_holds/panels.rs"]
mod panels;
use panels::{
    inspection_request, is_receipt_inspection_hold, retain_inspection_attempt, InspectionAttempt,
    InspectionPanel, ReleasePanel,
};

const HOLD_REASONS: [InventoryHoldReason; 6] = [
    InventoryHoldReason::QualityInspection,
    InventoryHoldReason::DamageSuspected,
    InventoryHoldReason::InventoryDiscrepancy,
    InventoryHoldReason::Regulatory,
    InventoryHoldReason::CustomerRequest,
    InventoryHoldReason::Other,
];

#[component]
pub fn QuantityHoldsWorkbench(
    initial_balances: Vec<InventoryBalanceResponse>,
    initial_balance_cursor: Option<OpaqueCursor>,
    initial_holds: Vec<InventoryHoldResponse>,
    initial_hold_cursor: Option<OpaqueCursor>,
    can_inspect_receipts: bool,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let balances = RwSignal::new(initial_balances);
    let balance_cursor = RwSignal::new(initial_balance_cursor);
    let holds = RwSignal::new(initial_holds);
    let hold_cursor = RwSignal::new(initial_hold_cursor);
    let hold_status = RwSignal::new(InventoryHoldStatus::Active);
    let balance_filter = RwSignal::new(String::new());
    let hold_filter = RwSignal::new(String::new());
    let selected_balance = RwSignal::new(None::<InventoryBalanceResponse>);
    let release_candidate = RwSignal::new(None::<InventoryHoldResponse>);
    let inspection_candidate = RwSignal::new(None::<InventoryHoldResponse>);
    let inspection_outcome = RwSignal::new(InboundInspectionOutcome::Approved);
    let inspection_note = RwSignal::new(String::new());
    let inspection_attempt = RwSignal::new(None::<InspectionAttempt>);
    let quantity = RwSignal::new("1".to_owned());
    let reason = RwSignal::new(InventoryHoldReason::QualityInspection);
    let note = RwSignal::new(String::new());
    let reference_type = RwSignal::new(String::new());
    let reference_id = RwSignal::new(String::new());
    let command_key = RwSignal::new(None::<String>);
    let command_pending = RwSignal::new(false);
    let list_pending = RwSignal::new(false);
    let balance_pending = RwSignal::new(false);
    let command_error = RwSignal::new(None::<String>);
    let list_error = RwSignal::new(None::<String>);
    let balance_sort = RwSignal::new(SortSpec {
        key: PositionSort::Facility,
        direction: SortDirection::Ascending,
    });
    let hold_sort = RwSignal::new(SortSpec {
        key: HoldSort::Created,
        direction: SortDirection::Descending,
    });
    let toasts = use_toast_bus();

    let select_balance = move |balance: InventoryBalanceResponse| {
        let available = balance.quantity.available;
        selected_balance.set(Some(balance));
        release_candidate.set(None);
        inspection_candidate.set(None);
        quantity.set(if available > 0 { "1" } else { "0" }.to_owned());
        reason.set(InventoryHoldReason::QualityInspection);
        note.set(String::new());
        reference_type.set(String::new());
        reference_id.set(String::new());
        command_key.set(None);
        command_error.set(None);
    };

    let load_more_balances = move |_| {
        let Some(cursor) = balance_cursor.get_untracked() else {
            return;
        };
        if balance_pending.get_untracked() {
            return;
        }
        balance_pending.set(true);
        list_error.set(None);
        leptos::task::spawn_local(async move {
            match api::balances(Some(&cursor)).await {
                Ok(page) => {
                    balances.update(|current| current.extend(page.items));
                    balance_cursor.set(page.next_cursor);
                    balance_pending.set(false);
                }
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) => {
                    list_error.set(Some(error.message));
                    balance_pending.set(false);
                }
            }
        });
    };

    let switch_hold_status = move |status: InventoryHoldStatus| {
        if list_pending.get_untracked() || hold_status.get_untracked() == status {
            return;
        }
        list_pending.set(true);
        list_error.set(None);
        leptos::task::spawn_local(async move {
            match api::holds(status, None).await {
                Ok(page) => {
                    holds.set(page.items);
                    hold_cursor.set(page.next_cursor);
                    hold_status.set(status);
                    list_pending.set(false);
                }
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) => {
                    list_error.set(Some(error.message));
                    list_pending.set(false);
                }
            }
        });
    };

    let load_more_holds = move |_| {
        let Some(cursor) = hold_cursor.get_untracked() else {
            return;
        };
        if list_pending.get_untracked() {
            return;
        }
        let status = hold_status.get_untracked();
        list_pending.set(true);
        list_error.set(None);
        leptos::task::spawn_local(async move {
            match api::holds(status, Some(&cursor)).await {
                Ok(page) => {
                    holds.update(|current| current.extend(page.items));
                    hold_cursor.set(page.next_cursor);
                    list_pending.set(false);
                }
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) => {
                    list_error.set(Some(error.message));
                    list_pending.set(false);
                }
            }
        });
    };

    let place_hold = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let Some(balance) = selected_balance.get_untracked() else {
            return;
        };
        if command_pending.get_untracked() {
            return;
        }

        let Ok(quantity_value) = quantity.get_untracked().trim().parse::<i64>() else {
            command_error.set(Some("Enter a whole-number quantity.".to_owned()));
            return;
        };
        if quantity_value <= 0 || quantity_value > balance.quantity.available {
            command_error.set(Some(format!(
                "Quantity must be between 1 and {}.",
                format_quantity(balance.quantity.available)
            )));
            return;
        }
        let note_value = optional_text(&note.get_untracked());
        if reason.get_untracked() == InventoryHoldReason::Other && note_value.is_none() {
            command_error.set(Some("Enter a note for the Other reason.".to_owned()));
            return;
        }
        let reference_type_value = optional_text(&reference_type.get_untracked());
        let reference_id_value = optional_positive_id(&reference_id.get_untracked());
        if reference_type_value.is_some() != reference_id_value.is_some() {
            command_error.set(Some(
                "Reference type and a positive reference ID must be entered together.".to_owned(),
            ));
            return;
        }

        let request = PlaceInventoryHoldRequest {
            inventory_balance_id: balance.id,
            quantity: quantity_value,
            reason: reason.get_untracked(),
            note: note_value,
            reference_type: reference_type_value,
            reference_id: reference_id_value,
        };
        let key = command_key
            .get_untracked()
            .unwrap_or_else(api::new_idempotency_key);
        command_key.set(Some(key.clone()));
        command_pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::place_hold(&request, &key).await {
                Ok(result) => {
                    let refresh = reload(InventoryHoldStatus::Active).await;
                    match refresh {
                        Ok((balance_page, hold_page)) => {
                            balances.set(balance_page.items);
                            balance_cursor.set(balance_page.next_cursor);
                            holds.set(hold_page.items);
                            hold_cursor.set(hold_page.next_cursor);
                            hold_status.set(InventoryHoldStatus::Active);
                            selected_balance.set(None);
                            command_key.set(None);
                            toasts.success(format!(
                                "Hold #{} placed for {} {}.",
                                result.hold_id,
                                format_quantity(request.quantity),
                                balance.uom
                            ));
                        }
                        Err(error) if error.unauthorized => on_unauthorized.run(()),
                        Err(error) => {
                            let message = format!(
                                "Hold #{} was placed, but the workbench could not refresh: {}",
                                result.hold_id, error.message
                            );
                            command_error.set(Some(message.clone()));
                            toasts.error(message);
                        }
                    }
                    command_pending.set(false);
                }
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) => {
                    toasts.error(error.message.clone());
                    command_error.set(Some(error.message));
                    command_pending.set(false);
                }
            }
        });
    };

    let confirm_release = move |_| {
        let Some(hold) = release_candidate.get_untracked() else {
            return;
        };
        if command_pending.get_untracked() {
            return;
        }
        let key = command_key
            .get_untracked()
            .unwrap_or_else(api::new_idempotency_key);
        command_key.set(Some(key.clone()));
        command_pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::release_hold(hold.id, &key).await {
                Ok(result) => {
                    let refresh = reload(InventoryHoldStatus::Active).await;
                    match refresh {
                        Ok((balance_page, hold_page)) => {
                            balances.set(balance_page.items);
                            balance_cursor.set(balance_page.next_cursor);
                            holds.set(hold_page.items);
                            hold_cursor.set(hold_page.next_cursor);
                            hold_status.set(InventoryHoldStatus::Active);
                            release_candidate.set(None);
                            command_key.set(None);
                            toasts.success(format!(
                                "Hold #{} released; {} {} returned to the position.",
                                result.hold_id,
                                format_quantity(result.released_quantity),
                                hold.uom
                            ));
                        }
                        Err(error) if error.unauthorized => on_unauthorized.run(()),
                        Err(error) => {
                            let message = format!(
                                "Hold #{} was released, but the workbench could not refresh: {}",
                                result.hold_id, error.message
                            );
                            command_error.set(Some(message.clone()));
                            toasts.error(message);
                        }
                    }
                    command_pending.set(false);
                }
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) => {
                    toasts.error(error.message.clone());
                    command_error.set(Some(error.message));
                    command_pending.set(false);
                }
            }
        });
    };

    let confirm_inspection = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let Some(hold) = inspection_candidate.get_untracked() else {
            return;
        };
        if command_pending.get_untracked() {
            return;
        }
        let attempt = match inspection_attempt.get_untracked() {
            Some(attempt) => attempt,
            None => {
                let request = match inspection_request(
                    inspection_outcome.get_untracked(),
                    &inspection_note.get_untracked(),
                ) {
                    Ok(request) => request,
                    Err(message) => {
                        command_error.set(Some(message.to_owned()));
                        return;
                    }
                };
                InspectionAttempt {
                    request,
                    idempotency_key: api::new_idempotency_key(),
                }
            }
        };
        inspection_attempt.set(Some(attempt.clone()));
        command_pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::dispose_inbound_inspection(
                hold.id,
                &attempt.request,
                &attempt.idempotency_key,
            )
            .await
            {
                Ok(result) => {
                    inspection_candidate.set(None);
                    inspection_attempt.set(None);
                    match reload(InventoryHoldStatus::Active).await {
                        Ok((balance_page, hold_page)) => {
                            balances.set(balance_page.items);
                            balance_cursor.set(balance_page.next_cursor);
                            holds.set(hold_page.items);
                            hold_cursor.set(hold_page.next_cursor);
                            hold_status.set(InventoryHoldStatus::Active);
                            let action = match result.outcome {
                                InboundInspectionOutcome::Approved => "approved",
                                InboundInspectionOutcome::Damaged => "marked damaged",
                            };
                            toasts.success(format!(
                                "Hold #{} inspected; {} {} {}.",
                                result.inventory_hold_id,
                                format_quantity(result.quantity),
                                result.uom,
                                action
                            ));
                        }
                        Err(error) if error.unauthorized => on_unauthorized.run(()),
                        Err(error) => {
                            let message = format!(
                                "Inspection #{} committed, but the workbench could not refresh: {}",
                                result.disposition_id, error.message
                            );
                            command_error.set(Some(message.clone()));
                            toasts.error(message);
                        }
                    }
                    command_pending.set(false);
                }
                Err(error) if error.unauthorized => {
                    command_pending.set(false);
                    on_unauthorized.run(());
                }
                Err(error) => {
                    if !retain_inspection_attempt(error.ambiguous_outcome) {
                        inspection_attempt.set(None);
                    }
                    let message = if error.ambiguous_outcome {
                        format!("{} Retry to resolve the saved command.", error.message)
                    } else {
                        error.message
                    };
                    toasts.error(message.clone());
                    command_error.set(Some(message));
                    command_pending.set(false);
                }
            }
        });
    };

    view! {
        <section class="holds-workbench">
            <div class="holds-position-grid">
                <section class="data-section position-browser" aria-labelledby="position-browser-title">
                    <div class="section-title workbench-title">
                        <div>
                            <p class="eyebrow">"Available inventory"</p>
                            <h2 id="position-browser-title">"Select a stock position"</h2>
                        </div>
                        <SearchField
                            label="Filter loaded inventory positions".to_owned()
                            placeholder="Filter positions"
                            value=balance_filter
                        />
                    </div>
                    <div class="table-scroll">
                        <table class="data-table balance-select-table">
                            <caption class="sr-only">"Inventory positions available for quantity holds"</caption>
                            <thead>
                                <tr>
                                    <SortableHeader
                                        label="Item"
                                        active=move || balance_sort.get().key == PositionSort::Item
                                        direction=move || balance_sort.get().direction
                                        on_sort=Callback::new(move |_| {
                                            SortSpec::select(balance_sort, PositionSort::Item)
                                        })
                                    />
                                    <SortableHeader
                                        label="Client"
                                        active=move || balance_sort.get().key == PositionSort::Client
                                        direction=move || balance_sort.get().direction
                                        on_sort=Callback::new(move |_| {
                                            SortSpec::select(balance_sort, PositionSort::Client)
                                        })
                                        column_class="position-owner-column"
                                    />
                                    <SortableHeader
                                        label="Facility"
                                        active=move || balance_sort.get().key == PositionSort::Facility
                                        direction=move || balance_sort.get().direction
                                        on_sort=Callback::new(move |_| {
                                            SortSpec::select(balance_sort, PositionSort::Facility)
                                        })
                                        column_class="position-facility-column"
                                    />
                                    <SortableHeader
                                        label="Location"
                                        active=move || balance_sort.get().key == PositionSort::Location
                                        direction=move || balance_sort.get().direction
                                        on_sort=Callback::new(move |_| {
                                            SortSpec::select(balance_sort, PositionSort::Location)
                                        })
                                    />
                                    <SortableHeader
                                        label="Available"
                                        active=move || balance_sort.get().key == PositionSort::Available
                                        direction=move || balance_sort.get().direction
                                        on_sort=Callback::new(move |_| {
                                            SortSpec::select(balance_sort, PositionSort::Available)
                                        })
                                        numeric=true
                                    />
                                    <th scope="col" class="action-column">"Action"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {move || {
                                    let query = balance_filter.get().trim().to_ascii_lowercase();
                                    let mut matching = balances
                                        .get()
                                        .into_iter()
                                        .filter(|balance| {
                                            balance.quantity.available > 0
                                                && balance_matches(balance, &query)
                                        })
                                        .collect::<Vec<_>>();
                                    sort_positions(&mut matching, balance_sort.get());
                                    if matching.is_empty() {
                                        view! {
                                            <tr>
                                                <td class="table-empty-row" colspan="6">
                                                    "No available positions match this filter."
                                                </td>
                                            </tr>
                                        }
                                            .into_any()
                                    } else {
                                        matching
                                            .into_iter()
                                            .map(|balance| {
                                                let selected = selected_balance
                                                    .get()
                                                    .as_ref()
                                                    .is_some_and(|current| current.id == balance.id);
                                                let row_class = selected.then_some("selected-row");
                                                let action_balance = balance.clone();
                                                let item_detail = balance_item_detail(&balance);
                                                view! {
                                                    <tr class=row_class>
                                                        <td>
                                                            <strong>{item_label(&balance)}</strong>
                                                            {item_detail.map(|description| {
                                                                view! { <small class="cell-detail">{description}</small> }
                                                            })}
                                                        </td>
                                                        <td class="position-owner-column">
                                                            {balance.inventory_owner_name.clone()}
                                                        </td>
                                                        <td class="position-facility-column">
                                                            {facility_label(&balance)}
                                                        </td>
                                                        <td>{location_label(&balance)}</td>
                                                        <td class="numeric strong">
                                                            {format_quantity(balance.quantity.available)}
                                                            <small class="uom-detail">{balance.uom.clone()}</small>
                                                        </td>
                                                        <td class="action-column">
                                                            <button
                                                                class="button table-action"
                                                                type="button"
                                                                on:click=move |_| select_balance(action_balance.clone())
                                                            >
                                                                <Icon icon=UiIcon::Holds/>
                                                                <span>{if selected { "Selected" } else { "Hold" }}</span>
                                                            </button>
                                                        </td>
                                                    </tr>
                                                }
                                            })
                                            .collect_view()
                                            .into_any()
                                    }
                                }}
                            </tbody>
                        </table>
                    </div>
                    <div class="table-footer">
                        <span>
                            {move || {
                                balance_cursor
                                    .get()
                                    .map_or("All positions loaded", |_| "More positions available")
                            }}
                        </span>
                        <button
                            class="button secondary-action"
                            type="button"
                            on:click=load_more_balances
                            disabled=move || {
                                balance_cursor.get().is_none() || balance_pending.get()
                            }
                        >
                            {move || if balance_pending.get() { "Loading" } else { "Load more" }}
                        </button>
                    </div>
                </section>

                <aside class="command-panel" aria-labelledby="hold-command-title">
                    {move || {
                        if let Some(hold) = inspection_candidate.get() {
                            view! {
                                <InspectionPanel
                                    hold
                                    outcome=inspection_outcome
                                    note=inspection_note
                                    pending=command_pending
                                    error=command_error
                                    retry_retained=Signal::derive(move || {
                                        inspection_attempt.get().is_some()
                                    })
                                    on_change=Callback::new(move |_| {
                                        inspection_attempt.set(None);
                                        command_error.set(None);
                                    })
                                    on_confirm=Callback::new(confirm_inspection)
                                    on_cancel=Callback::new(move |_| {
                                        inspection_candidate.set(None);
                                        inspection_attempt.set(None);
                                        command_error.set(None);
                                    })
                                />
                            }
                                .into_any()
                        } else if let Some(hold) = release_candidate.get() {
                            view! {
                                <ReleasePanel
                                    hold
                                    pending=command_pending
                                    error=command_error
                                    on_confirm=Callback::new(confirm_release)
                                    on_cancel=Callback::new(move |_| {
                                        release_candidate.set(None);
                                        command_key.set(None);
                                        command_error.set(None);
                                    })
                                />
                            }
                                .into_any()
                        } else if let Some(balance) = selected_balance.get() {
                            view! {
                                <form class="hold-form" on:submit=place_hold>
                                    <div class="command-panel-heading">
                                        <span class="command-icon"><Icon icon=UiIcon::Holds/></span>
                                        <div>
                                            <p class="eyebrow">"Place quantity hold"</p>
                                            <h2 id="hold-command-title">{item_label(&balance)}</h2>
                                        </div>
                                    </div>
                                    <dl class="position-facts">
                                        <div><dt>"Client"</dt><dd>{balance.inventory_owner_name.clone()}</dd></div>
                                        <div><dt>"Facility"</dt><dd>{facility_label(&balance)}</dd></div>
                                        <div><dt>"Location"</dt><dd>{location_label(&balance)}</dd></div>
                                        <div>
                                            <dt>"Available"</dt>
                                            <dd>{format!("{} {}", format_quantity(balance.quantity.available), balance.uom)}</dd>
                                        </div>
                                    </dl>

                                    <label for="hold-quantity">"Quantity"</label>
                                    <input
                                        id="hold-quantity"
                                        type="number"
                                        min="1"
                                        max=balance.quantity.available
                                        required
                                        prop:value=move || quantity.get()
                                        on:input=move |event| {
                                            quantity.set(event_target_value(&event));
                                            command_key.set(None);
                                            command_error.set(None);
                                        }
                                    />

                                    <label for="hold-reason">"Reason"</label>
                                    <select
                                        id="hold-reason"
                                        prop:value=move || reason_code(reason.get())
                                        on:change=move |event| {
                                            if let Some(value) = parse_reason(&event_target_value(&event)) {
                                                reason.set(value);
                                                command_key.set(None);
                                                command_error.set(None);
                                            }
                                        }
                                    >
                                        {HOLD_REASONS
                                            .into_iter()
                                            .map(|option| {
                                                view! {
                                                    <option value=reason_code(option)>
                                                        {reason_label(option)}
                                                    </option>
                                                }
                                            })
                                            .collect_view()}
                                    </select>

                                    <label for="hold-note">
                                        {move || {
                                            if reason.get() == InventoryHoldReason::Other {
                                                "Note (required)"
                                            } else {
                                                "Note"
                                            }
                                        }}
                                    </label>
                                    <textarea
                                        id="hold-note"
                                        rows="3"
                                        maxlength="1000"
                                        prop:value=move || note.get()
                                        on:input=move |event| {
                                            note.set(event_target_value(&event));
                                            command_key.set(None);
                                            command_error.set(None);
                                        }
                                    ></textarea>

                                    <details class="reference-fields">
                                        <summary>"Business reference"</summary>
                                        <div>
                                            <label for="hold-reference-type">"Type"</label>
                                            <input
                                                id="hold-reference-type"
                                                type="text"
                                                maxlength="100"
                                                prop:value=move || reference_type.get()
                                                on:input=move |event| {
                                                    reference_type.set(event_target_value(&event));
                                                    command_key.set(None);
                                                    command_error.set(None);
                                                }
                                            />
                                            <label for="hold-reference-id">"ID"</label>
                                            <input
                                                id="hold-reference-id"
                                                type="number"
                                                min="1"
                                                prop:value=move || reference_id.get()
                                                on:input=move |event| {
                                                    reference_id.set(event_target_value(&event));
                                                    command_key.set(None);
                                                    command_error.set(None);
                                                }
                                            />
                                        </div>
                                    </details>

                                    {move || {
                                        command_error.get().map(|message| {
                                            view! { <div class="inline-command-error" role="alert">{message}</div> }
                                        })
                                    }}

                                    <div class="command-actions">
                                        <button
                                            class="button quiet-action"
                                            type="button"
                                            on:click=move |_| {
                                                selected_balance.set(None);
                                                command_key.set(None);
                                                command_error.set(None);
                                            }
                                        >
                                            "Cancel"
                                        </button>
                                        <button
                                            class="button primary-action"
                                            type="submit"
                                            disabled=move || command_pending.get()
                                        >
                                            <Icon icon=UiIcon::Holds/>
                                            <span>
                                                {move || {
                                                    if command_pending.get() {
                                                        "Placing hold"
                                                    } else {
                                                        "Place hold"
                                                    }
                                                }}
                                            </span>
                                        </button>
                                    </div>
                                </form>
                            }
                                .into_any()
                        } else {
                            view! {
                                <div class="command-placeholder">
                                    <span class="command-icon"><Icon icon=UiIcon::Holds/></span>
                                    <p class="eyebrow">"Quantity hold"</p>
                                    <h2 id="hold-command-title">"No position selected"</h2>
                                    <p>"Select an available inventory position to begin."</p>
                                </div>
                            }
                                .into_any()
                        }
                    }}
                </aside>
            </div>

            <section class="data-section hold-ledger" aria-labelledby="hold-ledger-title">
                <div class="section-title ledger-toolbar">
                    <div>
                        <p class="eyebrow">"Inventory restrictions"</p>
                        <h2 id="hold-ledger-title">"Quantity hold ledger"</h2>
                    </div>
                    <div class="ledger-controls">
                        <div class="segmented-control" role="group" aria-label="Hold status">
                            <button
                                type="button"
                                class:active=move || hold_status.get() == InventoryHoldStatus::Active
                                on:click=move |_| switch_hold_status(InventoryHoldStatus::Active)
                            >
                                "Active"
                            </button>
                            <button
                                type="button"
                                class:active=move || hold_status.get() == InventoryHoldStatus::Released
                                on:click=move |_| switch_hold_status(InventoryHoldStatus::Released)
                            >
                                "Released"
                            </button>
                        </div>
                        <SearchField
                            label="Filter loaded holds".to_owned()
                            placeholder="Filter holds"
                            value=hold_filter
                        />
                    </div>
                </div>
                <div class="hold-summary-strip">
                    <span>
                        <strong>{move || format_quantity(holds.get().len() as i64)}</strong>
                        " records loaded"
                    </span>
                    <span>
                        <strong>
                            {move || {
                                format_quantity(
                                    holds.get().iter().map(|hold| hold.quantity).sum::<i64>(),
                                )
                            }}
                        </strong>
                        " units"
                    </span>
                </div>
                <div class="table-scroll">
                    <table class="data-table holds-table">
                        <caption class="sr-only">"Quantity holds in the current access scope"</caption>
                        <thead>
                            <tr>
                                <SortableHeader
                                    label="Hold"
                                    active=move || hold_sort.get().key == HoldSort::Id
                                    direction=move || hold_sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(hold_sort, HoldSort::Id)
                                    })
                                />
                                <SortableHeader
                                    label="Item"
                                    active=move || hold_sort.get().key == HoldSort::Item
                                    direction=move || hold_sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(hold_sort, HoldSort::Item)
                                    })
                                />
                                <SortableHeader
                                    label="Client"
                                    active=move || hold_sort.get().key == HoldSort::Client
                                    direction=move || hold_sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(hold_sort, HoldSort::Client)
                                    })
                                    column_class="hold-owner-column"
                                />
                                <SortableHeader
                                    label="Facility / location"
                                    active=move || hold_sort.get().key == HoldSort::Position
                                    direction=move || hold_sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(hold_sort, HoldSort::Position)
                                    })
                                />
                                <SortableHeader
                                    label="Reason"
                                    active=move || hold_sort.get().key == HoldSort::Reason
                                    direction=move || hold_sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(hold_sort, HoldSort::Reason)
                                    })
                                />
                                <SortableHeader
                                    label="Created"
                                    active=move || hold_sort.get().key == HoldSort::Created
                                    direction=move || hold_sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(hold_sort, HoldSort::Created)
                                    })
                                    column_class="hold-created-column"
                                />
                                <SortableHeader
                                    label="Quantity"
                                    active=move || hold_sort.get().key == HoldSort::Quantity
                                    direction=move || hold_sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(hold_sort, HoldSort::Quantity)
                                    })
                                    numeric=true
                                />
                                <th scope="col" class="action-column">"Action"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {move || {
                                let query = hold_filter.get().trim().to_ascii_lowercase();
                                let mut matching = holds
                                    .get()
                                    .into_iter()
                                    .filter(|hold| hold_matches(hold, &query))
                                    .collect::<Vec<_>>();
                                sort_holds(&mut matching, hold_sort.get());
                                if matching.is_empty() {
                                    view! {
                                        <tr>
                                            <td class="table-empty-row" colspan="8">
                                                "No holds match this view."
                                            </td>
                                        </tr>
                                    }
                                        .into_any()
                                } else {
                                    matching
                                        .into_iter()
                                        .map(|hold| {
                                            let release_hold = hold.clone();
                                            let inspection_hold = is_receipt_inspection_hold(&hold);
                                            let can_release = hold.status == InventoryHoldStatus::Active
                                                && !inspection_hold;
                                            let inspect_hold = hold.clone();
                                            view! {
                                                <tr>
                                                    <td><strong>{format!("#{}", hold.id)}</strong></td>
                                                    <td>
                                                        <strong>{hold_item_label(&hold)}</strong>
                                                        {tracking_label(&hold).map(|tracking| {
                                                            view! { <small class="cell-detail">{tracking}</small> }
                                                        })}
                                                    </td>
                                                    <td class="hold-owner-column">
                                                        {hold.inventory_owner_name.clone()}
                                                    </td>
                                                    <td>
                                                        <strong>{hold_facility_label(&hold)}</strong>
                                                        <small class="cell-detail">{hold_location_label(&hold)}</small>
                                                    </td>
                                                    <td>
                                                        <span class="reason-label">{reason_label(hold.reason)}</span>
                                                        {hold.note.clone().map(|note| {
                                                            view! { <small class="cell-detail">{note}</small> }
                                                        })}
                                                    </td>
                                                    <td class="hold-created-column">
                                                        {compact_timestamp(&hold.created_at)}
                                                    </td>
                                                    <td class="numeric strong">
                                                        {format_quantity(hold.quantity)}
                                                        <small class="uom-detail">{hold.uom.clone()}</small>
                                                    </td>
                                                    <td class="action-column">
                                                        {if inspection_hold && can_inspect_receipts {
                                                            view! {
                                                                <button
                                                                    class="button table-action inspection-action"
                                                                    type="button"
                                                                    on:click=move |_| {
                                                                        inspection_candidate.set(Some(inspect_hold.clone()));
                                                                        release_candidate.set(None);
                                                                        selected_balance.set(None);
                                                                        inspection_outcome.set(InboundInspectionOutcome::Approved);
                                                                        inspection_note.set(String::new());
                                                                        inspection_attempt.set(None);
                                                                        command_error.set(None);
                                                                    }
                                                                >
                                                                    <Icon icon=UiIcon::Disposition/>
                                                                    <span>"Inspect"</span>
                                                                </button>
                                                            }
                                                                .into_any()
                                                        } else if inspection_hold {
                                                            view! { <span class="cell-detail">"Supervisor"</span> }
                                                                .into_any()
                                                        } else if can_release {
                                                            view! {
                                                                <button
                                                                    class="button table-action danger"
                                                                    type="button"
                                                                    on:click=move |_| {
                                                                        release_candidate.set(Some(release_hold.clone()));
                                                                        inspection_candidate.set(None);
                                                                        selected_balance.set(None);
                                                                        command_key.set(Some(api::new_idempotency_key()));
                                                                        command_error.set(None);
                                                                    }
                                                                >
                                                                    "Release"
                                                                </button>
                                                            }
                                                                .into_any()
                                                        } else {
                                                            ().into_any()
                                                        }}
                                                    </td>
                                                </tr>
                                            }
                                        })
                                        .collect_view()
                                        .into_any()
                                }
                            }}
                        </tbody>
                    </table>
                </div>
                <div class="table-footer">
                    <span>
                        {move || {
                            hold_cursor
                                .get()
                                .map_or("All holds loaded", |_| "More holds available")
                        }}
                    </span>
                    {move || {
                        list_error.get().map(|message| {
                            view! { <span class="inline-error" role="alert">{message}</span> }
                        })
                    }}
                    <button
                        class="button secondary-action"
                        type="button"
                        on:click=load_more_holds
                        disabled=move || hold_cursor.get().is_none() || list_pending.get()
                    >
                        {move || if list_pending.get() { "Loading" } else { "Load more" }}
                    </button>
                </div>
            </section>
        </section>
    }
}

async fn reload(
    status: InventoryHoldStatus,
) -> Result<
    (
        wareboxes_api_contract::v1::InventoryBalancePage,
        wareboxes_api_contract::v1::InventoryHoldPage,
    ),
    api::ApiError,
> {
    let balances = api::balances(None).await?;
    let holds = api::holds(status, None).await?;
    Ok((balances, holds))
}

fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn optional_positive_id(value: &str) -> Option<i64> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        value.parse::<i64>().ok().filter(|id| *id > 0)
    }
}

fn reason_code(reason: InventoryHoldReason) -> &'static str {
    match reason {
        InventoryHoldReason::QualityInspection => "quality_inspection",
        InventoryHoldReason::DamageSuspected => "damage_suspected",
        InventoryHoldReason::InventoryDiscrepancy => "inventory_discrepancy",
        InventoryHoldReason::Regulatory => "regulatory",
        InventoryHoldReason::CustomerRequest => "customer_request",
        InventoryHoldReason::Other => "other",
    }
}

fn parse_reason(value: &str) -> Option<InventoryHoldReason> {
    Some(match value {
        "quality_inspection" => InventoryHoldReason::QualityInspection,
        "damage_suspected" => InventoryHoldReason::DamageSuspected,
        "inventory_discrepancy" => InventoryHoldReason::InventoryDiscrepancy,
        "regulatory" => InventoryHoldReason::Regulatory,
        "customer_request" => InventoryHoldReason::CustomerRequest,
        "other" => InventoryHoldReason::Other,
        _ => return None,
    })
}

fn reason_label(reason: InventoryHoldReason) -> &'static str {
    match reason {
        InventoryHoldReason::QualityInspection => "Quality inspection",
        InventoryHoldReason::DamageSuspected => "Damage suspected",
        InventoryHoldReason::InventoryDiscrepancy => "Inventory discrepancy",
        InventoryHoldReason::Regulatory => "Regulatory",
        InventoryHoldReason::CustomerRequest => "Customer request",
        InventoryHoldReason::Other => "Other",
    }
}

fn compact_timestamp(timestamp: &str) -> String {
    timestamp.get(..16).unwrap_or(timestamp).replace('T', " ")
}

#[cfg(test)]
mod tests {
    use super::{optional_positive_id, optional_text, parse_reason, reason_label};
    use wareboxes_api_contract::v1::InventoryHoldReason;

    #[test]
    fn normalizes_optional_command_fields() {
        assert_eq!(optional_text("  "), None);
        assert_eq!(optional_text(" QA review "), Some("QA review".to_owned()));
        assert_eq!(optional_positive_id(""), None);
        assert_eq!(optional_positive_id("0"), None);
        assert_eq!(optional_positive_id("42"), Some(42));
    }

    #[test]
    fn hold_reason_controls_round_trip() {
        assert_eq!(
            parse_reason("inventory_discrepancy"),
            Some(InventoryHoldReason::InventoryDiscrepancy)
        );
        assert_eq!(
            reason_label(InventoryHoldReason::CustomerRequest),
            "Customer request"
        );
        assert_eq!(parse_reason("unknown"), None);
    }
}
