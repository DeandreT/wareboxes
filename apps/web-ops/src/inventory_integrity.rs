use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    CreateInventoryRelocationTaskRequest, InventoryAgingResponse, InventoryBalanceResponse,
    InventoryRelocationWorkRequest, OpaqueCursor,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;
use wareboxes_core::models::Location;

use crate::{api, toast::use_toast_bus, view_model::format_quantity};

mod aging;
mod move_planner;
mod read_views;
mod recall;

use move_planner::{movable_quantity, MovePlanner};

#[derive(Clone)]
struct IntegrityData {
    balances: Vec<InventoryBalanceResponse>,
    balance_next_cursor: Option<OpaqueCursor>,
    locations: Vec<Location>,
}

#[derive(Clone)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(
        dead_code,
        reason = "hydration constructs the terminal inventory-control states"
    )
)]
enum IntegrityState {
    Loading,
    Ready(Box<IntegrityData>),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IntegrityTab {
    Journal,
    Aging,
    Recall,
    Reconciliation,
    MovePlanning,
}

#[component]
pub fn InventoryIntegrityWorkbench(
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
    can_manage_recalls: bool,
) -> impl IntoView {
    let access = StoredValue::new(access);
    let state = RwSignal::new(IntegrityState::Loading);
    let tab = RwSignal::new(IntegrityTab::Journal);
    let recall_target = RwSignal::new(None::<InventoryAgingResponse>);
    let selected_balance_id = RwSignal::new(String::new());
    let selected_balance = RwSignal::new(None::<InventoryBalanceResponse>);
    let destination_location_id = RwSignal::new(String::new());
    let quantity = RwSignal::new("1".to_owned());
    let instructions = RwSignal::new(String::new());
    let task_pending = RwSignal::new(false);
    let task_error = RwSignal::new(None::<String>);
    let toasts = use_toast_bus();
    let open_recall = Callback::new(move |target: InventoryAgingResponse| {
        recall_target.set(Some(target));
        tab.set(IntegrityTab::Recall);
    });

    #[cfg(target_arch = "wasm32")]
    request_integrity(state, on_unauthorized);

    let retry = move |_| request_integrity(state, on_unauthorized);
    let create_task = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if task_pending.get_untracked() {
            return;
        }
        let IntegrityState::Ready(data) = state.get_untracked() else {
            return;
        };
        let Some(source) = selected_balance.get_untracked() else {
            task_error.set(Some("Select a source position.".to_owned()));
            return;
        };
        let Some(destination_id) = positive_id(&destination_location_id.get_untracked()) else {
            task_error.set(Some("Select a destination location.".to_owned()));
            return;
        };
        let Some(destination) = data
            .locations
            .iter()
            .find(|location| location.id == destination_id)
        else {
            task_error.set(Some(
                "The selected destination location is no longer loaded.".to_owned(),
            ));
            return;
        };
        if source.facility_id != destination.facility_id {
            task_error.set(Some(
                "Source and destination must belong to the same facility.".to_owned(),
            ));
            return;
        }
        if source.location_id == destination.id {
            task_error.set(Some(
                "Destination must differ from the source location.".to_owned(),
            ));
            return;
        }
        let movable = movable_quantity(&source);
        let Ok(quantity_value) = quantity.get_untracked().trim().parse::<i64>() else {
            task_error.set(Some("Enter a whole-number quantity.".to_owned()));
            return;
        };
        if source.license_plate_id.is_none() && (quantity_value <= 0 || quantity_value > movable) {
            task_error.set(Some(format!(
                "Quantity must be between 1 and {}.",
                format_quantity(movable)
            )));
            return;
        }
        let instructions_value = optional_text(&instructions.get_untracked());
        let work = source.license_plate_id.map_or_else(
            || InventoryRelocationWorkRequest::LooseBalance {
                source_inventory_balance_id: source.id,
                quantity: quantity_value,
            },
            |license_plate_id| InventoryRelocationWorkRequest::LicensePlate { license_plate_id },
        );
        let key = api::new_idempotency_key();
        task_pending.set(true);
        task_error.set(None);
        leptos::task::spawn_local(async move {
            let result = api::create_inventory_relocation_task(
                &CreateInventoryRelocationTaskRequest {
                    work,
                    destination_location_id: destination_id,
                    priority: Some(50),
                    assigned_user_id: None,
                    scheduled_for: None,
                    due_at: None,
                    instructions: instructions_value,
                },
                &key,
            )
            .await
            .map(|response| response.task_id);

            match result {
                Ok(task_id) => {
                    toasts.success(format!("Move task #{task_id} is ready for RF execution."));
                    selected_balance_id.set(String::new());
                    selected_balance.set(None);
                    destination_location_id.set(String::new());
                    quantity.set("1".to_owned());
                    instructions.set(String::new());
                }
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) => {
                    toasts.error(error.message.clone());
                    task_error.set(Some(error.message));
                }
            }
            task_pending.set(false);
        });
    };

    view! {
        <section class="integrity-workbench">
            {move || match state.get() {
                IntegrityState::Loading => {
                    view! {
                        <div class="data-section integrity-state" aria-live="polite">
                            <span class="loading-line" aria-hidden="true"></span>
                            <strong>"Loading inventory controls"</strong>
                        </div>
                    }
                        .into_any()
                }
                IntegrityState::Failed(message) => {
                    view! {
                        <div class="data-section integrity-state" role="alert">
                            <strong>"Inventory controls are unavailable"</strong>
                            <span>{message}</span>
                            <button class="button secondary-action" type="button" on:click=retry>
                                "Retry"
                            </button>
                        </div>
                    }
                        .into_any()
                }
                IntegrityState::Ready(data) => {
                    let on_hand = data
                        .balances
                        .iter()
                        .map(|balance| balance.quantity.on_hand)
                        .sum::<i64>();
                    let committed = data
                        .balances
                        .iter()
                        .map(|balance| balance.quantity.reserved + balance.quantity.held)
                        .sum::<i64>();
                    view! {
                        <div class="integrity-summary" aria-label="Inventory integrity summary">
                            <div><span>"Traceability"</span><strong>"Scoped journal"</strong></div>
                            <div><span>"Reconciliation"</span><strong>"Live projections"</strong></div>
                            <div><span>"On hand loaded"</span><strong>{format_quantity(on_hand)}</strong></div>
                            <div><span>"Committed loaded"</span><strong>{format_quantity(committed)}</strong></div>
                            <div><span>"Execution"</span><strong>"RF-directed moves"</strong></div>
                        </div>

                        <div class="integrity-tabs" role="tablist" aria-label="Inventory controls">
                            <button
                                type="button"
                                role="tab"
                                aria-selected=move || (tab.get() == IntegrityTab::Journal).to_string()
                                class:active=move || tab.get() == IntegrityTab::Journal
                                on:click=move |_| tab.set(IntegrityTab::Journal)
                            >
                                "Journal"
                            </button>
                            <button
                                type="button"
                                role="tab"
                                aria-selected=move || (tab.get() == IntegrityTab::Aging).to_string()
                                class:active=move || tab.get() == IntegrityTab::Aging
                                on:click=move |_| tab.set(IntegrityTab::Aging)
                            >
                                "Aging"
                            </button>
                            {can_manage_recalls.then(|| view! {
                                <button
                                    type="button"
                                    role="tab"
                                    aria-selected=move || (tab.get() == IntegrityTab::Recall).to_string()
                                    class:active=move || tab.get() == IntegrityTab::Recall
                                    on:click=move |_| tab.set(IntegrityTab::Recall)
                                >
                                    "Recalls"
                                </button>
                            })}
                            <button
                                type="button"
                                role="tab"
                                aria-selected=move || {
                                    (tab.get() == IntegrityTab::Reconciliation).to_string()
                                }
                                class:active=move || tab.get() == IntegrityTab::Reconciliation
                                on:click=move |_| tab.set(IntegrityTab::Reconciliation)
                            >
                                "Reconciliation"
                            </button>
                            <button
                                type="button"
                                role="tab"
                                aria-selected=move || {
                                    (tab.get() == IntegrityTab::MovePlanning).to_string()
                                }
                                class:active=move || tab.get() == IntegrityTab::MovePlanning
                                on:click=move |_| tab.set(IntegrityTab::MovePlanning)
                            >
                                "Move planning"
                            </button>
                        </div>

                        {move || match tab.get() {
                            IntegrityTab::Journal => {
                                view! { <read_views::JournalView on_unauthorized/> }.into_any()
                            }
                            IntegrityTab::Aging => {
                                view! {
                                    <aging::AgingView
                                        on_unauthorized
                                        on_recall=open_recall
                                        can_manage_recalls
                                    />
                                }
                                .into_any()
                            }
                            IntegrityTab::Recall => {
                                view! { <recall::RecallView access=access.get_value() on_unauthorized target=recall_target/> }.into_any()
                            }
                            IntegrityTab::Reconciliation => {
                                view! { <read_views::ReconciliationView on_unauthorized/> }.into_any()
                            }
                            IntegrityTab::MovePlanning => {
                                view! {
                                    <MovePlanner
                                        balances=data.balances.clone()
                                        initial_cursor=data.balance_next_cursor.clone()
                                        locations=data.locations.clone()
                                        selected_balance_id
                                        selected_balance
                                        destination_location_id
                                        quantity
                                        instructions
                                        pending=task_pending
                                        error=task_error
                                        on_submit=Callback::new(create_task)
                                        on_unauthorized
                                    />
                                }
                                    .into_any()
                            }
                        }}
                    }
                        .into_any()
                }
            }}
        </section>
    }
}

#[cfg(target_arch = "wasm32")]
fn request_integrity(state: RwSignal<IntegrityState>, on_unauthorized: Callback<()>) {
    state.set(IntegrityState::Loading);
    leptos::task::spawn_local(async move {
        let result = async {
            let balance_page = api::balances(None).await?;
            let locations = api::internal_get("/api/locations").await?;
            Ok::<_, api::ApiError>(IntegrityData {
                balances: balance_page.items,
                balance_next_cursor: balance_page.next_cursor,
                locations,
            })
        }
        .await;

        match result {
            Ok(data) => state.set(IntegrityState::Ready(Box::new(data))),
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) => state.set(IntegrityState::Failed(error.message)),
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn request_integrity(_state: RwSignal<IntegrityState>, _on_unauthorized: Callback<()>) {}

fn positive_id(value: &str) -> Option<i64> {
    value.trim().parse::<i64>().ok().filter(|id| *id > 0)
}

fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{optional_text, positive_id};

    #[test]
    fn move_planner_normalizes_command_fields() {
        assert_eq!(positive_id("42"), Some(42));
        assert_eq!(positive_id("0"), None);
        assert_eq!(optional_text("  "), None);
        assert_eq!(
            optional_text("  Keep upright "),
            Some("Keep upright".into())
        );
    }
}
