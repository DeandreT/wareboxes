use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    CreateInventoryStatusTransitionRequest, InventoryBalanceResponse, InventoryBalanceStatus,
    InventoryStatusTransitionReason, OpaqueCursor,
};

use crate::api;
use crate::components::{Icon, SearchField, UiIcon};
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;

const STATUSES: [InventoryBalanceStatus; 4] = [
    InventoryBalanceStatus::Available,
    InventoryBalanceStatus::Hold,
    InventoryBalanceStatus::Damaged,
    InventoryBalanceStatus::Quarantine,
];

const REASONS: [InventoryStatusTransitionReason; 11] = [
    InventoryStatusTransitionReason::QualityInspection,
    InventoryStatusTransitionReason::DamageSuspected,
    InventoryStatusTransitionReason::DamageConfirmed,
    InventoryStatusTransitionReason::InspectionPassed,
    InventoryStatusTransitionReason::InventoryDiscrepancy,
    InventoryStatusTransitionReason::DiscrepancyResolved,
    InventoryStatusTransitionReason::RegulatoryRestriction,
    InventoryStatusTransitionReason::RegulatoryRelease,
    InventoryStatusTransitionReason::CustomerRequest,
    InventoryStatusTransitionReason::CustomerRelease,
    InventoryStatusTransitionReason::Other,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum DispositionSort {
    Item,
    Client,
    Facility,
    Location,
    Status,
    Movable,
}

#[component]
pub fn InventoryDispositionWorkbench(
    initial_balances: Vec<InventoryBalanceResponse>,
    initial_cursor: Option<OpaqueCursor>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let balances = RwSignal::new(initial_balances);
    let next_cursor = RwSignal::new(initial_cursor);
    let filter = RwSignal::new(String::new());
    let selected = RwSignal::new(None::<InventoryBalanceResponse>);
    let quantity = RwSignal::new("1".to_owned());
    let target_status = RwSignal::new(InventoryBalanceStatus::Quarantine);
    let reason = RwSignal::new(InventoryStatusTransitionReason::QualityInspection);
    let note = RwSignal::new(String::new());
    let reference_type = RwSignal::new(String::new());
    let reference_id = RwSignal::new(String::new());
    let command_key = RwSignal::new(None::<String>);
    let command_pending = RwSignal::new(false);
    let load_pending = RwSignal::new(false);
    let command_error = RwSignal::new(None::<String>);
    let list_error = RwSignal::new(None::<String>);
    let sort = RwSignal::new(SortSpec {
        key: DispositionSort::Facility,
        direction: SortDirection::Ascending,
    });
    let toasts = use_toast_bus();

    let select_position = move |balance: InventoryBalanceResponse| {
        let target = default_target(balance.status);
        let movable = movable_quantity(&balance);
        selected.set(Some(balance));
        quantity.set(if movable > 0 { "1" } else { "0" }.to_owned());
        target_status.set(target);
        reason.set(default_reason(target));
        note.set(String::new());
        reference_type.set(String::new());
        reference_id.set(String::new());
        command_key.set(None);
        command_error.set(None);
    };

    let change_target = move |event| {
        let Some(status) = parse_status(&event_target_value(&event)) else {
            return;
        };
        target_status.set(status);
        if !reason_allows_target(reason.get_untracked(), status) {
            reason.set(default_reason(status));
        }
        command_error.set(None);
    };

    let load_more = move |_| {
        let Some(cursor) = next_cursor.get_untracked() else {
            return;
        };
        if load_pending.get_untracked() {
            return;
        }
        load_pending.set(true);
        list_error.set(None);
        leptos::task::spawn_local(async move {
            match api::balances(Some(&cursor)).await {
                Ok(page) => {
                    balances.update(|current| current.extend(page.items));
                    next_cursor.set(page.next_cursor);
                    load_pending.set(false);
                }
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) => {
                    list_error.set(Some(error.message));
                    load_pending.set(false);
                }
            }
        });
    };

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let Some(balance) = selected.get_untracked() else {
            return;
        };
        if command_pending.get_untracked() {
            return;
        }
        let Ok(quantity_value) = quantity.get_untracked().trim().parse::<i64>() else {
            command_error.set(Some("Enter a whole-number quantity.".to_owned()));
            return;
        };
        let movable = movable_quantity(&balance);
        if quantity_value <= 0 || quantity_value > movable {
            command_error.set(Some(format!(
                "Quantity must be between 1 and {}.",
                format_quantity(movable)
            )));
            return;
        }
        let target = target_status.get_untracked();
        if target == balance.status {
            command_error.set(Some(
                "Choose a disposition different from the current status.".to_owned(),
            ));
            return;
        }
        let reason_value = reason.get_untracked();
        if !reason_allows_target(reason_value, target) {
            command_error.set(Some(
                "Choose a reason that is valid for the target disposition.".to_owned(),
            ));
            return;
        }
        let note_value = optional_text(&note.get_untracked());
        if reason_value == InventoryStatusTransitionReason::Other && note_value.is_none() {
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

        let request = CreateInventoryStatusTransitionRequest {
            quantity: quantity_value,
            to_status: target,
            reason: reason_value,
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
            match api::transition_inventory_status(balance.id, &request, &key).await {
                Ok(result) => match api::balances(None).await {
                    Ok(page) => {
                        balances.set(page.items);
                        next_cursor.set(page.next_cursor);
                        selected.set(None);
                        command_key.set(None);
                        toasts.success(format!(
                            "{} {} moved from {} to {}. Transaction #{}.",
                            format_quantity(result.quantity),
                            balance.uom,
                            status_label(result.from_status),
                            status_label(result.to_status),
                            result.inventory_transaction_id
                        ));
                        command_pending.set(false);
                    }
                    Err(error) if error.unauthorized => on_unauthorized.run(()),
                    Err(error) => {
                        let message = format!(
                            "Transaction #{} committed, but positions could not refresh: {}",
                            result.inventory_transaction_id, error.message
                        );
                        command_error.set(Some(message.clone()));
                        toasts.error(message);
                        command_pending.set(false);
                    }
                },
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) => {
                    toasts.error(error.message.clone());
                    command_error.set(Some(error.message));
                    command_pending.set(false);
                }
            }
        });
    };

    view! {
        <div class="disposition-workbench">
            <section class="data-section disposition-browser">
                <div class="table-toolbar">
                    <div class="toolbar-summary">
                        <strong>{move || format_quantity(balances.get().len() as i64)}</strong>
                        <span>"positions loaded"</span>
                    </div>
                    <SearchField
                        label="Filter loaded disposition positions".to_owned()
                        placeholder="Filter positions"
                        value=filter
                    />
                </div>
                <div class="table-scroll">
                    <table class="data-table disposition-table">
                        <caption class="sr-only">"Inventory positions available for disposition changes"</caption>
                        <thead>
                            <tr>
                                <SortableHeader
                                    label="Item"
                                    active=move || sort.get().key == DispositionSort::Item
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, DispositionSort::Item)
                                    })
                                />
                                <SortableHeader
                                    label="Client"
                                    active=move || sort.get().key == DispositionSort::Client
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, DispositionSort::Client)
                                    })
                                />
                                <SortableHeader
                                    label="Facility"
                                    active=move || sort.get().key == DispositionSort::Facility
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, DispositionSort::Facility)
                                    })
                                />
                                <SortableHeader
                                    label="Location"
                                    active=move || sort.get().key == DispositionSort::Location
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, DispositionSort::Location)
                                    })
                                />
                                <SortableHeader
                                    label="Status"
                                    active=move || sort.get().key == DispositionSort::Status
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, DispositionSort::Status)
                                    })
                                />
                                <SortableHeader
                                    label="Movable"
                                    active=move || sort.get().key == DispositionSort::Movable
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, DispositionSort::Movable)
                                    })
                                    numeric=true
                                />
                                <th scope="col" class="action-column">"Action"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {move || {
                                let query = filter.get().trim().to_ascii_lowercase();
                                let mut matching = balances
                                    .get()
                                    .into_iter()
                                    .filter(|balance| {
                                        movable_quantity(balance) > 0
                                            && balance_matches(balance, &query)
                                    })
                                    .collect::<Vec<_>>();
                                sort_positions(&mut matching, sort.get());
                                if matching.is_empty() {
                                    view! {
                                        <tr>
                                            <td class="table-empty-row" colspan="7">
                                                "No movable positions match this filter."
                                            </td>
                                        </tr>
                                    }
                                        .into_any()
                                } else {
                                    matching
                                        .into_iter()
                                        .map(|balance| {
                                            let active = selected
                                                .get()
                                                .as_ref()
                                                .is_some_and(|current| current.id == balance.id);
                                            let action_balance = balance.clone();
                                            view! {
                                                <tr class:active-row=active>
                                                    <td>
                                                        <strong>{item_label(&balance)}</strong>
                                                        {item_detail(&balance).map(|detail| {
                                                            view! { <small class="cell-detail">{detail}</small> }
                                                        })}
                                                    </td>
                                                    <td>{balance.inventory_owner_name.clone()}</td>
                                                    <td>{facility_label(&balance)}</td>
                                                    <td><strong>{location_label(&balance)}</strong></td>
                                                    <td>
                                                        <span class=status_class(balance.status)>
                                                            {status_label(balance.status)}
                                                        </span>
                                                    </td>
                                                    <td class="numeric strong">
                                                        {format_quantity(movable_quantity(&balance))}
                                                        <small class="uom-detail">{balance.uom.clone()}</small>
                                                    </td>
                                                    <td class="action-column">
                                                        <button
                                                            class="button table-action"
                                                            type="button"
                                                            on:click=move |_| {
                                                                select_position(action_balance.clone());
                                                            }
                                                        >
                                                            <Icon icon=UiIcon::Disposition/>
                                                            <span>{if active { "Selected" } else { "Change" }}</span>
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
                            next_cursor
                                .get()
                                .map_or("All positions loaded", |_| "More positions available")
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
                        on:click=load_more
                        disabled=move || next_cursor.get().is_none() || load_pending.get()
                    >
                        {move || if load_pending.get() { "Loading" } else { "Load more" }}
                    </button>
                </div>
            </section>

            <aside class="command-panel disposition-command" aria-labelledby="disposition-command-title">
                {move || {
                    selected
                        .get()
                        .map(|balance| {
                            let source_status = balance.status;
                            view! {
                                <form class="hold-form" on:submit=submit>
                                    <div class="command-panel-heading">
                                        <span class="command-icon" aria-hidden="true">
                                            <Icon icon=UiIcon::Disposition/>
                                        </span>
                                        <div>
                                            <p class="eyebrow">"Journaled movement"</p>
                                            <h2 id="disposition-command-title">"Change disposition"</h2>
                                        </div>
                                    </div>
                                    <dl class="position-facts">
                                        <div class="wide">
                                            <dt>"Item"</dt>
                                            <dd>{item_label(&balance)}</dd>
                                        </div>
                                        <div><dt>"Client"</dt><dd>{balance.inventory_owner_name.clone()}</dd></div>
                                        <div><dt>"Facility"</dt><dd>{facility_label(&balance)}</dd></div>
                                        <div><dt>"Location"</dt><dd>{location_label(&balance)}</dd></div>
                                        <div><dt>"Current"</dt><dd>{status_label(balance.status)}</dd></div>
                                        <div><dt>"Movable"</dt><dd>{format!("{} {}", format_quantity(movable_quantity(&balance)), balance.uom)}</dd></div>
                                    </dl>

                                    <label for="disposition-quantity">"Quantity"</label>
                                    <input
                                        id="disposition-quantity"
                                        type="number"
                                        min="1"
                                        max=movable_quantity(&balance)
                                        step="1"
                                        required
                                        prop:value=move || quantity.get()
                                        on:input=move |event| {
                                            quantity.set(event_target_value(&event));
                                            command_error.set(None);
                                        }
                                    />

                                    <label for="target-status">"Target disposition"</label>
                                    <select
                                        id="target-status"
                                        prop:value=move || status_value(target_status.get())
                                        on:change=change_target
                                    >
                                        {STATUSES
                                            .into_iter()
                                            .filter(|status| *status != source_status)
                                            .map(|status| {
                                                view! {
                                                    <option
                                                        value=status_value(status)
                                                        prop:selected=move || {
                                                            target_status.get() == status
                                                        }
                                                    >
                                                        {status_label(status)}
                                                    </option>
                                                }
                                            })
                                            .collect_view()}
                                    </select>

                                    <label for="disposition-reason">"Reason"</label>
                                    <select
                                        id="disposition-reason"
                                        prop:value=move || reason_value(reason.get())
                                        on:change=move |event| {
                                            if let Some(value) = parse_reason(&event_target_value(&event)) {
                                                reason.set(value);
                                                command_error.set(None);
                                            }
                                        }
                                    >
                                        {move || {
                                            let target = target_status.get();
                                            REASONS
                                                .into_iter()
                                                .filter(|reason_option| {
                                                    reason_allows_target(*reason_option, target)
                                                })
                                                .map(|reason_option| {
                                                    view! {
                                                        <option
                                                            value=reason_value(reason_option)
                                                            prop:selected=move || {
                                                                reason.get() == reason_option
                                                            }
                                                        >
                                                            {reason_label(reason_option)}
                                                        </option>
                                                    }
                                                })
                                                .collect_view()
                                        }}
                                    </select>

                                    <label for="disposition-note">"Note"</label>
                                    <textarea
                                        id="disposition-note"
                                        maxlength="1000"
                                        placeholder="Optional unless reason is Other"
                                        prop:value=move || note.get()
                                        on:input=move |event| {
                                            note.set(event_target_value(&event));
                                            command_error.set(None);
                                        }
                                    ></textarea>

                                    <details class="reference-fields">
                                        <summary>"Business reference"</summary>
                                        <div>
                                            <label for="disposition-reference-type">"Type"</label>
                                            <input
                                                id="disposition-reference-type"
                                                maxlength="100"
                                                placeholder="receipt"
                                                prop:value=move || reference_type.get()
                                                on:input=move |event| {
                                                    reference_type.set(event_target_value(&event));
                                                    command_error.set(None);
                                                }
                                            />
                                            <label for="disposition-reference-id">"ID"</label>
                                            <input
                                                id="disposition-reference-id"
                                                type="number"
                                                min="1"
                                                step="1"
                                                prop:value=move || reference_id.get()
                                                on:input=move |event| {
                                                    reference_id.set(event_target_value(&event));
                                                    command_error.set(None);
                                                }
                                            />
                                        </div>
                                    </details>

                                    {move || {
                                        command_error.get().map(|message| {
                                            view! {
                                                <div class="inline-command-error" role="alert">{message}</div>
                                            }
                                        })
                                    }}

                                    <div class="command-actions">
                                        <button
                                            class="button quiet-action"
                                            type="button"
                                            on:click=move |_| {
                                                selected.set(None);
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
                                            {move || {
                                                if command_pending.get() {
                                                    "Committing"
                                                } else {
                                                    "Commit change"
                                                }
                                            }}
                                        </button>
                                    </div>
                                </form>
                            }
                            .into_any()
                        })
                        .unwrap_or_else(|| {
                            view! {
                                <div class="command-placeholder">
                                    <span class="command-icon" aria-hidden="true">
                                        <Icon icon=UiIcon::Disposition/>
                                    </span>
                                    <p class="eyebrow">"Inventory disposition"</p>
                                    <h2 id="disposition-command-title">"Select a stock position"</h2>
                                    <p>"Choose an available position to quarantine, mark damaged, place on status hold, or release."</p>
                                </div>
                            }
                            .into_any()
                        })
                }}
            </aside>
        </div>
    }
}

fn sort_positions(balances: &mut [InventoryBalanceResponse], spec: SortSpec<DispositionSort>) {
    balances.sort_by(|left, right| {
        let ordering = match spec.key {
            DispositionSort::Item => item_label(left)
                .to_ascii_lowercase()
                .cmp(&item_label(right).to_ascii_lowercase()),
            DispositionSort::Client => left
                .inventory_owner_name
                .to_ascii_lowercase()
                .cmp(&right.inventory_owner_name.to_ascii_lowercase()),
            DispositionSort::Facility => facility_label(left)
                .to_ascii_lowercase()
                .cmp(&facility_label(right).to_ascii_lowercase())
                .then_with(|| {
                    location_label(left)
                        .to_ascii_lowercase()
                        .cmp(&location_label(right).to_ascii_lowercase())
                }),
            DispositionSort::Location => location_label(left)
                .to_ascii_lowercase()
                .cmp(&location_label(right).to_ascii_lowercase()),
            DispositionSort::Status => status_label(left.status).cmp(status_label(right.status)),
            DispositionSort::Movable => movable_quantity(left).cmp(&movable_quantity(right)),
        }
        .then_with(|| left.id.cmp(&right.id));
        if spec.direction == SortDirection::Ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
}

fn balance_matches(balance: &InventoryBalanceResponse, query: &str) -> bool {
    query.is_empty()
        || [
            balance.inventory_owner_name.as_str(),
            balance.facility_name.as_deref().unwrap_or_default(),
            balance.location_name.as_deref().unwrap_or_default(),
            balance.location_barcode.as_deref().unwrap_or_default(),
            balance.item_description.as_deref().unwrap_or_default(),
            balance.primary_sku.as_deref().unwrap_or_default(),
            balance.lot.as_deref().unwrap_or_default(),
            balance.serial.as_deref().unwrap_or_default(),
            status_label(balance.status),
        ]
        .iter()
        .any(|value| value.to_ascii_lowercase().contains(query))
}

fn movable_quantity(balance: &InventoryBalanceResponse) -> i64 {
    balance
        .quantity
        .on_hand
        .saturating_sub(balance.quantity.reserved)
        .saturating_sub(balance.quantity.held)
}

fn facility_label(balance: &InventoryBalanceResponse) -> String {
    balance
        .facility_name
        .clone()
        .unwrap_or_else(|| format!("Facility #{}", balance.facility_id))
}

fn location_label(balance: &InventoryBalanceResponse) -> String {
    balance
        .location_barcode
        .clone()
        .or_else(|| balance.location_name.clone())
        .unwrap_or_else(|| format!("#{}", balance.location_id))
}

fn item_label(balance: &InventoryBalanceResponse) -> String {
    balance
        .primary_sku
        .clone()
        .or_else(|| balance.item_description.clone())
        .unwrap_or_else(|| format!("#{}", balance.item_id))
}

fn item_detail(balance: &InventoryBalanceResponse) -> Option<String> {
    balance
        .primary_sku
        .as_ref()
        .and(balance.item_description.clone())
}

fn default_target(current: InventoryBalanceStatus) -> InventoryBalanceStatus {
    match current {
        InventoryBalanceStatus::Available => InventoryBalanceStatus::Quarantine,
        InventoryBalanceStatus::Hold
        | InventoryBalanceStatus::Damaged
        | InventoryBalanceStatus::Quarantine => InventoryBalanceStatus::Available,
    }
}

fn default_reason(target: InventoryBalanceStatus) -> InventoryStatusTransitionReason {
    match target {
        InventoryBalanceStatus::Available => InventoryStatusTransitionReason::InspectionPassed,
        InventoryBalanceStatus::Hold | InventoryBalanceStatus::Quarantine => {
            InventoryStatusTransitionReason::QualityInspection
        }
        InventoryBalanceStatus::Damaged => InventoryStatusTransitionReason::DamageConfirmed,
    }
}

fn reason_allows_target(
    reason: InventoryStatusTransitionReason,
    target: InventoryBalanceStatus,
) -> bool {
    match reason {
        InventoryStatusTransitionReason::QualityInspection
        | InventoryStatusTransitionReason::DamageSuspected
        | InventoryStatusTransitionReason::InventoryDiscrepancy
        | InventoryStatusTransitionReason::RegulatoryRestriction
        | InventoryStatusTransitionReason::CustomerRequest => matches!(
            target,
            InventoryBalanceStatus::Hold | InventoryBalanceStatus::Quarantine
        ),
        InventoryStatusTransitionReason::DamageConfirmed => {
            target == InventoryBalanceStatus::Damaged
        }
        InventoryStatusTransitionReason::InspectionPassed
        | InventoryStatusTransitionReason::DiscrepancyResolved
        | InventoryStatusTransitionReason::RegulatoryRelease
        | InventoryStatusTransitionReason::CustomerRelease => {
            target == InventoryBalanceStatus::Available
        }
        InventoryStatusTransitionReason::Other => true,
    }
}

fn status_label(status: InventoryBalanceStatus) -> &'static str {
    match status {
        InventoryBalanceStatus::Available => "Available",
        InventoryBalanceStatus::Hold => "Hold",
        InventoryBalanceStatus::Damaged => "Damaged",
        InventoryBalanceStatus::Quarantine => "Quarantine",
    }
}

fn status_value(status: InventoryBalanceStatus) -> &'static str {
    match status {
        InventoryBalanceStatus::Available => "available",
        InventoryBalanceStatus::Hold => "hold",
        InventoryBalanceStatus::Damaged => "damaged",
        InventoryBalanceStatus::Quarantine => "quarantine",
    }
}

fn parse_status(value: &str) -> Option<InventoryBalanceStatus> {
    Some(match value {
        "available" => InventoryBalanceStatus::Available,
        "hold" => InventoryBalanceStatus::Hold,
        "damaged" => InventoryBalanceStatus::Damaged,
        "quarantine" => InventoryBalanceStatus::Quarantine,
        _ => return None,
    })
}

fn status_class(status: InventoryBalanceStatus) -> &'static str {
    match status {
        InventoryBalanceStatus::Available => "status shipped",
        InventoryBalanceStatus::Hold | InventoryBalanceStatus::Damaged => "status held",
        InventoryBalanceStatus::Quarantine => "status processing",
    }
}

fn reason_label(reason: InventoryStatusTransitionReason) -> &'static str {
    match reason {
        InventoryStatusTransitionReason::QualityInspection => "Quality inspection",
        InventoryStatusTransitionReason::DamageSuspected => "Damage suspected",
        InventoryStatusTransitionReason::DamageConfirmed => "Damage confirmed",
        InventoryStatusTransitionReason::InspectionPassed => "Inspection passed",
        InventoryStatusTransitionReason::InventoryDiscrepancy => "Inventory discrepancy",
        InventoryStatusTransitionReason::DiscrepancyResolved => "Discrepancy resolved",
        InventoryStatusTransitionReason::RegulatoryRestriction => "Regulatory restriction",
        InventoryStatusTransitionReason::RegulatoryRelease => "Regulatory release",
        InventoryStatusTransitionReason::CustomerRequest => "Client request",
        InventoryStatusTransitionReason::CustomerRelease => "Client release",
        InventoryStatusTransitionReason::Other => "Other",
    }
}

fn reason_value(reason: InventoryStatusTransitionReason) -> &'static str {
    match reason {
        InventoryStatusTransitionReason::QualityInspection => "quality_inspection",
        InventoryStatusTransitionReason::DamageSuspected => "damage_suspected",
        InventoryStatusTransitionReason::DamageConfirmed => "damage_confirmed",
        InventoryStatusTransitionReason::InspectionPassed => "inspection_passed",
        InventoryStatusTransitionReason::InventoryDiscrepancy => "inventory_discrepancy",
        InventoryStatusTransitionReason::DiscrepancyResolved => "discrepancy_resolved",
        InventoryStatusTransitionReason::RegulatoryRestriction => "regulatory_restriction",
        InventoryStatusTransitionReason::RegulatoryRelease => "regulatory_release",
        InventoryStatusTransitionReason::CustomerRequest => "customer_request",
        InventoryStatusTransitionReason::CustomerRelease => "customer_release",
        InventoryStatusTransitionReason::Other => "other",
    }
}

fn parse_reason(value: &str) -> Option<InventoryStatusTransitionReason> {
    Some(match value {
        "quality_inspection" => InventoryStatusTransitionReason::QualityInspection,
        "damage_suspected" => InventoryStatusTransitionReason::DamageSuspected,
        "damage_confirmed" => InventoryStatusTransitionReason::DamageConfirmed,
        "inspection_passed" => InventoryStatusTransitionReason::InspectionPassed,
        "inventory_discrepancy" => InventoryStatusTransitionReason::InventoryDiscrepancy,
        "discrepancy_resolved" => InventoryStatusTransitionReason::DiscrepancyResolved,
        "regulatory_restriction" => InventoryStatusTransitionReason::RegulatoryRestriction,
        "regulatory_release" => InventoryStatusTransitionReason::RegulatoryRelease,
        "customer_request" => InventoryStatusTransitionReason::CustomerRequest,
        "customer_release" => InventoryStatusTransitionReason::CustomerRelease,
        "other" => InventoryStatusTransitionReason::Other,
        _ => return None,
    })
}

fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn optional_positive_id(value: &str) -> Option<i64> {
    value.trim().parse::<i64>().ok().filter(|id| *id > 0)
}

#[cfg(test)]
mod tests {
    use super::{default_reason, default_target, parse_reason, reason_allows_target, reason_label};
    use wareboxes_api_contract::v1::{InventoryBalanceStatus, InventoryStatusTransitionReason};

    #[test]
    fn disposition_defaults_are_valid_for_each_source_status() {
        for source in [
            InventoryBalanceStatus::Available,
            InventoryBalanceStatus::Hold,
            InventoryBalanceStatus::Damaged,
            InventoryBalanceStatus::Quarantine,
        ] {
            let target = default_target(source);
            assert_ne!(source, target);
            assert!(reason_allows_target(default_reason(target), target));
        }
    }

    #[test]
    fn client_reasons_use_warehouse_facing_copy() {
        let reason = parse_reason("customer_request").unwrap();
        assert_eq!(reason, InventoryStatusTransitionReason::CustomerRequest);
        assert_eq!(reason_label(reason), "Client request");
    }
}
