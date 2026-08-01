use leptos::prelude::*;
use wareboxes_api_contract::web::access::{AccessScopeResource, AccessScopeWorkspace};
use wareboxes_core::dto::AddLoad;
use wareboxes_core::models::{Item, Load, LoadStatus, LoadType, Location};

use crate::api;
use crate::components::SearchField;
use crate::fulfillment_load_detail::LoadDetailPanel;
use crate::fulfillment_shared::{
    cmp_option_str, optional_text, parse_optional_timestamp, short_timestamp,
};
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;

const LOAD_BATCH_SIZE: usize = 500;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadSort {
    Id,
    Type,
    Reference,
    Client,
    Facility,
    Status,
    Appointment,
}

#[component]
pub fn LoadsWorkbench(
    initial_loads: Vec<Load>,
    access: AccessScopeWorkspace,
    catalog_items: Vec<Item>,
    locations: Vec<Location>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let initial_count = initial_loads.len();
    let loads = RwSignal::new(initial_loads);
    let selected = RwSignal::new(None::<Load>);
    let selected_pending = RwSignal::new(false);
    let selected_error = RwSignal::new(None::<String>);
    let create_open = RwSignal::new(false);
    let search = RwSignal::new(String::new());
    let status = RwSignal::new(String::new());
    let load_type = RwSignal::new(String::new());
    let client = RwSignal::new(String::new());
    let facility = RwSignal::new(String::new());
    let date = RwSignal::new(String::new());
    let list_pending = RwSignal::new(false);
    let list_error = RwSignal::new(None::<String>);
    let next_offset = RwSignal::new(initial_count);
    let can_load_more = RwSignal::new(initial_count == LOAD_BATCH_SIZE);
    let sort = RwSignal::new(SortSpec {
        key: LoadSort::Appointment,
        direction: SortDirection::Ascending,
    });
    let toasts = use_toast_bus();
    let filter_clients = StoredValue::new(access.inventory_owners.clone());
    let filter_facilities = StoredValue::new(access.facilities.clone());
    let create_clients = StoredValue::new(access.inventory_owners);
    let create_facilities = StoredValue::new(access.facilities);
    let detail_items = StoredValue::new(catalog_items);
    let detail_locations = StoredValue::new(locations.clone());
    let create_locations = StoredValue::new(locations);

    let open_load = move |load_id: i64| {
        create_open.set(false);
        request_load_detail(
            load_id,
            selected,
            selected_pending,
            selected_error,
            on_unauthorized,
        );
    };

    let refresh_selected = Callback::new(move |load_id: i64| {
        request_load_detail(
            load_id,
            selected,
            selected_pending,
            selected_error,
            on_unauthorized,
        );
        leptos::task::spawn_local(async move {
            match fetch_loads(0, LOAD_BATCH_SIZE).await {
                Ok(next) => {
                    next_offset.set(next.len());
                    can_load_more.set(next.len() == LOAD_BATCH_SIZE);
                    loads.set(next);
                }
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => toasts.error(api_error.message),
            }
        });
    });

    let created = Callback::new(move |load_id: i64| {
        create_open.set(false);
        refresh_selected.run(load_id);
    });

    let load_more = move |_| {
        if list_pending.get_untracked() || !can_load_more.get_untracked() {
            return;
        }
        let offset = next_offset.get_untracked();
        list_pending.set(true);
        list_error.set(None);
        leptos::task::spawn_local(async move {
            match fetch_loads(offset, LOAD_BATCH_SIZE).await {
                Ok(next) => {
                    let count = next.len();
                    loads.update(|current| current.extend(next));
                    next_offset.set(offset + count);
                    can_load_more.set(count == LOAD_BATCH_SIZE);
                    list_pending.set(false);
                }
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => {
                    list_error.set(Some(api_error.message));
                    list_pending.set(false);
                }
            }
        });
    };

    view! {
        <div class="fulfillment-workbench loads-workbench">
            <section class="data-section fulfillment-list">
                <div class="table-toolbar fulfillment-toolbar">
                    <div class="toolbar-summary">
                        <strong>
                            {move || {
                                format_quantity(
                                    filtered_loads(
                                        &loads.get(),
                                        &search.get(),
                                        &status.get(),
                                        &load_type.get(),
                                        &client.get(),
                                        &facility.get(),
                                        &date.get(),
                                    )
                                    .len() as i64,
                                )
                            }}
                        </strong>
                        <span>"loads shown"</span>
                    </div>
                    <div class="fulfillment-filters">
                        <SearchField
                            label="Filter loads".to_owned()
                            placeholder="Load, carrier"
                            value=search
                        />
                        <label>
                            <span class="sr-only">"Load type"</span>
                            <select
                                prop:value=move || load_type.get()
                                on:change=move |event| load_type.set(event_target_value(&event))
                            >
                                <option value="">"All directions"</option>
                                <option value="inbound">"Inbound"</option>
                                <option value="outbound">"Outbound"</option>
                            </select>
                        </label>
                        <label>
                            <span class="sr-only">"Load status"</span>
                            <select
                                prop:value=move || status.get()
                                on:change=move |event| status.set(event_target_value(&event))
                            >
                                <option value="">"All statuses"</option>
                                {LoadStatus::ALL
                                    .into_iter()
                                    .map(|value| {
                                        view! {
                                            <option value=value.as_str()>{title_case(value.as_str())}</option>
                                        }
                                    })
                                    .collect_view()}
                            </select>
                        </label>
                        <label>
                            <span class="sr-only">"Client"</span>
                            <select
                                prop:value=move || client.get()
                                on:change=move |event| client.set(event_target_value(&event))
                            >
                                <option value="">"All clients"</option>
                                {filter_clients
                                    .get_value()
                                    .into_iter()
                                    .map(|owner| view! { <option value=owner.id>{owner.name}</option> })
                                    .collect_view()}
                            </select>
                        </label>
                        <label>
                            <span class="sr-only">"Facility"</span>
                            <select
                                prop:value=move || facility.get()
                                on:change=move |event| facility.set(event_target_value(&event))
                            >
                                <option value="">"All facilities"</option>
                                {filter_facilities
                                    .get_value()
                                    .into_iter()
                                    .map(|site| view! { <option value=site.id>{site.name}</option> })
                                    .collect_view()}
                            </select>
                        </label>
                        <label>
                            <span class="sr-only">"Appointment date"</span>
                            <input
                                class="date-filter"
                                type="date"
                                prop:value=move || date.get()
                                on:input=move |event| date.set(event_target_value(&event))
                            />
                        </label>
                        <button
                            class="button primary-action"
                            type="button"
                            on:click=move |_| {
                                selected.set(None);
                                create_open.set(true);
                            }
                        >
                            "New load"
                        </button>
                    </div>
                </div>
                <div class="table-scroll">
                    <table class="data-table fulfillment-table loads-workbench-table">
                        <caption class="sr-only">"Inbound and outbound loads matching the active filters"</caption>
                        <thead>
                            <tr>
                                <SortableHeader
                                    label="Load"
                                    active=move || sort.get().key == LoadSort::Id
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| SortSpec::select(sort, LoadSort::Id))
                                />
                                <SortableHeader
                                    label="Direction"
                                    active=move || sort.get().key == LoadSort::Type
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| SortSpec::select(sort, LoadSort::Type))
                                />
                                <SortableHeader
                                    label="Reference"
                                    active=move || sort.get().key == LoadSort::Reference
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, LoadSort::Reference)
                                    })
                                />
                                <SortableHeader
                                    label="Client"
                                    active=move || sort.get().key == LoadSort::Client
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| SortSpec::select(sort, LoadSort::Client))
                                />
                                <SortableHeader
                                    label="Facility"
                                    active=move || sort.get().key == LoadSort::Facility
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| SortSpec::select(sort, LoadSort::Facility))
                                />
                                <SortableHeader
                                    label="Status"
                                    active=move || sort.get().key == LoadSort::Status
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| SortSpec::select(sort, LoadSort::Status))
                                />
                                <SortableHeader
                                    label="Appointment"
                                    active=move || sort.get().key == LoadSort::Appointment
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, LoadSort::Appointment)
                                    })
                                />
                            </tr>
                        </thead>
                        <tbody>
                            {move || {
                                let spec = sort.get();
                                let selected_id = selected.get().map(|load| load.id);
                                let mut rows = filtered_loads(
                                    &loads.get(),
                                    &search.get(),
                                    &status.get(),
                                    &load_type.get(),
                                    &client.get(),
                                    &facility.get(),
                                    &date.get(),
                                );
                                rows.sort_by(|left, right| {
                                    let ordering = match spec.key {
                                        LoadSort::Id => left.id.cmp(&right.id),
                                        LoadSort::Type => left.r#type.as_str().cmp(right.r#type.as_str()),
                                        LoadSort::Reference => cmp_option_str(
                                            left.reference_number.as_deref(),
                                            right.reference_number.as_deref(),
                                        ),
                                        LoadSort::Client => cmp_option_str(
                                            left.inventory_owner_name.as_deref(),
                                            right.inventory_owner_name.as_deref(),
                                        ),
                                        LoadSort::Facility => cmp_option_str(
                                            left.facility_name.as_deref(),
                                            right.facility_name.as_deref(),
                                        ),
                                        LoadSort::Status => left.status.as_str().cmp(right.status.as_str()),
                                        LoadSort::Appointment => left
                                            .appointment_time
                                            .or(left.expected_time)
                                            .cmp(&right.appointment_time.or(right.expected_time)),
                                    }
                                    .then_with(|| left.id.cmp(&right.id));
                                    if spec.direction == SortDirection::Ascending {
                                        ordering
                                    } else {
                                        ordering.reverse()
                                    }
                                });
                                rows
                                    .into_iter()
                                    .map(|load| {
                                        let id = load.id;
                                        view! {
                                            <tr
                                                class:active-row=selected_id == Some(id)
                                                on:click=move |_| open_load(id)
                                            >
                                                <td>
                                                    <button
                                                        class="row-link"
                                                        type="button"
                                                        on:click=move |event| {
                                                            event.stop_propagation();
                                                            open_load(id);
                                                        }
                                                    >
                                                        {format!("#{}", load.id)}
                                                    </button>
                                                    <small class="cell-detail">{load.execution_barcode}</small>
                                                </td>
                                                <td>{title_case(load.r#type.as_str())}</td>
                                                <td>
                                                    {load.reference_number.unwrap_or_else(|| "-".to_owned())}
                                                    <small class="cell-detail">
                                                        {load.carrier.unwrap_or_else(|| "Carrier not set".to_owned())}
                                                    </small>
                                                </td>
                                                <td>{load.inventory_owner_name.unwrap_or_else(|| "Unassigned".to_owned())}</td>
                                                <td>{load.facility_name.unwrap_or_else(|| format!("#{}", load.facility_id))}</td>
                                                <td>
                                                    <span class=crate::fulfillment_shared::load_status_class(load.status)>
                                                        {title_case(load.status.as_str())}
                                                    </span>
                                                </td>
                                                <td>
                                                    {load
                                                        .appointment_time
                                                        .or(load.expected_time)
                                                        .map_or_else(|| "-".to_owned(), short_timestamp)}
                                                </td>
                                            </tr>
                                        }
                                    })
                                    .collect_view()
                            }}
                        </tbody>
                    </table>
                    <Show when=move || {
                        filtered_loads(
                            &loads.get(),
                            &search.get(),
                            &status.get(),
                            &load_type.get(),
                            &client.get(),
                            &facility.get(),
                            &date.get(),
                        )
                        .is_empty()
                    }>
                        <p class="empty-state">"No loads match the active filters."</p>
                    </Show>
                </div>
                <div class="table-footer">
                    <span>{move || format!("{} loads loaded", loads.get().len())}</span>
                    <Show when=move || can_load_more.get()>
                        <button
                            class="button secondary-action"
                            type="button"
                            disabled=move || list_pending.get()
                            on:click=load_more
                        >
                            {move || if list_pending.get() { "Loading" } else { "Load more" }}
                        </button>
                    </Show>
                </div>
                <Show when=move || list_error.get().is_some()>
                    <p class="inline-command-error" role="alert">
                        {move || list_error.get().unwrap_or_default()}
                    </p>
                </Show>
            </section>

            <aside class="command-panel fulfillment-detail load-detail-panel">
                <Show
                    when=move || create_open.get()
                    fallback=move || {
                        view! {
                            <Show
                                when=move || selected.get().is_some()
                                fallback=move || {
                                    view! {
                                        <div class="command-placeholder">
                                            <h2>"Load details"</h2>
                                            <p>"Select a load to inspect its appointment, expected freight, progress, notes, and activity."</p>
                                        </div>
                                    }
                                }
                            >
                                {move || {
                                    selected.get().map(|load| {
                                        view! {
                                            <LoadDetailPanel
                                                load
                                                catalog_items=detail_items.get_value()
                                                locations=detail_locations.get_value()
                                                pending=selected_pending
                                                load_error=selected_error
                                                on_refreshed=refresh_selected
                                                on_unauthorized
                                            />
                                        }
                                    })
                                }}
                            </Show>
                        }
                    }
                >
                    <CreateLoadPanel
                        facilities=create_facilities.get_value()
                        clients=create_clients.get_value()
                        locations=create_locations.get_value()
                        on_created=created
                        on_close=Callback::new(move |_| create_open.set(false))
                        on_unauthorized
                    />
                </Show>
            </aside>
        </div>
    }
}

#[component]
fn CreateLoadPanel(
    facilities: Vec<AccessScopeResource>,
    clients: Vec<AccessScopeResource>,
    locations: Vec<Location>,
    on_created: Callback<i64>,
    on_close: Callback<()>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let facility_id = RwSignal::new(
        facilities
            .first()
            .map_or_else(String::new, |facility| facility.id.to_string()),
    );
    let client_id = RwSignal::new(
        clients
            .first()
            .map_or_else(String::new, |client| client.id.to_string()),
    );
    let load_type = RwSignal::new(LoadType::Inbound);
    let reference = RwSignal::new(String::new());
    let invoice = RwSignal::new(String::new());
    let carrier = RwSignal::new(String::new());
    let trailer = RwSignal::new(String::new());
    let seal = RwSignal::new(String::new());
    let dock = RwSignal::new(String::new());
    let expected = RwSignal::new(String::new());
    let appointment = RwSignal::new(String::new());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let toasts = use_toast_bus();

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let Ok(selected_facility) = facility_id.get_untracked().parse::<i64>() else {
            error.set(Some("Choose a facility.".to_owned()));
            return;
        };
        let Ok(selected_client) = client_id.get_untracked().parse::<i64>() else {
            error.set(Some("Choose a client.".to_owned()));
            return;
        };
        let dock_location_id = match dock.get_untracked().trim() {
            "" => None,
            value => match value.parse::<i64>() {
                Ok(id) => Some(id),
                Err(_) => {
                    error.set(Some("Choose a dock door.".to_owned()));
                    return;
                }
            },
        };
        let expected_time = match parse_optional_timestamp(&expected.get_untracked()) {
            Ok(value) => value,
            Err(message) => {
                error.set(Some(format!("Expected time: {message}")));
                return;
            }
        };
        let appointment_time = match parse_optional_timestamp(&appointment.get_untracked()) {
            Ok(value) => value,
            Err(message) => {
                error.set(Some(format!("Appointment time: {message}")));
                return;
            }
        };
        let request = AddLoad {
            facility_id: selected_facility,
            inventory_owner_id: selected_client,
            r#type: load_type.get_untracked(),
            reference_number: optional_text(&reference.get_untracked()),
            invoice_number: optional_text(&invoice.get_untracked()),
            carrier: optional_text(&carrier.get_untracked()),
            trailer_number: optional_text(&trailer.get_untracked()),
            seal_number: optional_text(&seal.get_untracked()),
            dock_door_location_id: dock_location_id,
            expected_time,
            appointment_time,
        };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, i64>("/api/loads/add", &request).await {
                Ok(load_id) => {
                    pending.set(false);
                    toasts.success(format!("Load #{load_id} created."));
                    on_created.run(load_id);
                }
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => {
                    toasts.error(api_error.message.clone());
                    error.set(Some(api_error.message));
                    pending.set(false);
                }
            }
        });
    };

    view! {
        <form class="fulfillment-form" on:submit=submit>
            <div class="detail-heading">
                <div>
                    <span class="eyebrow">"Load planning"</span>
                    <h2>"New load"</h2>
                </div>
                <button type="button" class="text-button" on:click=move |_| on_close.run(())>
                    "Close"
                </button>
            </div>
            <div class="form-grid two-column">
                <label>
                    <span>"Direction"</span>
                    <select
                        prop:value=move || load_type.get().as_str()
                        on:change=move |event| {
                            if let Some(value) = LoadType::parse(&event_target_value(&event)) {
                                load_type.set(value);
                            }
                        }
                    >
                        <option value="inbound">"Inbound"</option>
                        <option value="outbound">"Outbound"</option>
                    </select>
                </label>
                <label>
                    <span>"Facility"</span>
                    <select
                        required
                        prop:value=move || facility_id.get()
                        on:change=move |event| {
                            facility_id.set(event_target_value(&event));
                            dock.set(String::new());
                        }
                    >
                        {facilities
                            .into_iter()
                            .map(|facility| {
                                view! { <option value=facility.id>{facility.name}</option> }
                            })
                            .collect_view()}
                    </select>
                </label>
                <label>
                    <span>"Client"</span>
                    <select
                        required
                        prop:value=move || client_id.get()
                        on:change=move |event| client_id.set(event_target_value(&event))
                    >
                        {clients
                            .into_iter()
                            .map(|client| view! { <option value=client.id>{client.name}</option> })
                            .collect_view()}
                    </select>
                </label>
                <label>
                    <span>"Dock door"</span>
                    <select
                        prop:value=move || dock.get()
                        on:change=move |event| dock.set(event_target_value(&event))
                    >
                        <option value="">"Not assigned"</option>
                        {move || {
                            let selected = facility_id.get().parse::<i64>().ok();
                            locations
                                .clone()
                                .into_iter()
                                .filter(|location| {
                                    Some(location.facility_id) == selected
                                        && location.active
                                        && location.r#type.eq_ignore_ascii_case("dock")
                                })
                                .map(|location| {
                                    let label = location
                                        .name
                                        .or(location.barcode)
                                        .unwrap_or_else(|| format!("Dock #{}", location.id));
                                    view! { <option value=location.id>{label}</option> }
                                })
                                .collect_view()
                        }}
                    </select>
                </label>
                <label>
                    <span>"Reference"</span>
                    <input
                        prop:value=move || reference.get()
                        on:input=move |event| reference.set(event_target_value(&event))
                    />
                </label>
                <label>
                    <span>"Invoice"</span>
                    <input
                        prop:value=move || invoice.get()
                        on:input=move |event| invoice.set(event_target_value(&event))
                    />
                </label>
                <label>
                    <span>"Carrier"</span>
                    <input
                        prop:value=move || carrier.get()
                        on:input=move |event| carrier.set(event_target_value(&event))
                    />
                </label>
                <label>
                    <span>"Trailer"</span>
                    <input
                        prop:value=move || trailer.get()
                        on:input=move |event| trailer.set(event_target_value(&event))
                    />
                </label>
                <label>
                    <span>"Seal"</span>
                    <input
                        prop:value=move || seal.get()
                        on:input=move |event| seal.set(event_target_value(&event))
                    />
                </label>
                <label>
                    <span>"Expected (UTC)"</span>
                    <input
                        type="datetime-local"
                        prop:value=move || expected.get()
                        on:input=move |event| expected.set(event_target_value(&event))
                    />
                </label>
                <label>
                    <span>"Appointment (UTC)"</span>
                    <input
                        type="datetime-local"
                        prop:value=move || appointment.get()
                        on:input=move |event| appointment.set(event_target_value(&event))
                    />
                </label>
            </div>
            <Show when=move || error.get().is_some()>
                <p class="inline-command-error" role="alert">{move || error.get().unwrap_or_default()}</p>
            </Show>
            <div class="form-actions">
                <button type="submit" class="button primary-action" disabled=move || pending.get()>
                    {move || if pending.get() { "Creating" } else { "Create load" }}
                </button>
                <button type="button" class="button secondary-action" on:click=move |_| on_close.run(())>
                    "Cancel"
                </button>
            </div>
        </form>
    }
}

async fn fetch_loads(offset: usize, limit: usize) -> Result<Vec<Load>, api::ApiError> {
    api::internal_get(&format!("/api/loads?offset={offset}&limit={limit}")).await
}

fn request_load_detail(
    load_id: i64,
    selected: RwSignal<Option<Load>>,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_unauthorized: Callback<()>,
) {
    pending.set(true);
    error.set(None);
    leptos::task::spawn_local(async move {
        match api::internal_get::<Option<Load>>(&format!("/api/loads/{load_id}")).await {
            Ok(Some(load)) => {
                selected.set(Some(load));
                pending.set(false);
            }
            Ok(None) => {
                selected.set(None);
                error.set(Some(
                    "Load not found or outside your warehouse scope.".to_owned(),
                ));
                pending.set(false);
            }
            Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
            Err(api_error) => {
                error.set(Some(api_error.message));
                pending.set(false);
            }
        }
    });
}

fn filtered_loads(
    loads: &[Load],
    search: &str,
    status: &str,
    load_type: &str,
    client: &str,
    facility: &str,
    date: &str,
) -> Vec<Load> {
    let needle = search.trim().to_ascii_lowercase();
    let client_id = client.parse::<i64>().ok();
    let facility_id = facility.parse::<i64>().ok();
    loads
        .iter()
        .filter(|load| {
            (status.is_empty() || load.status.as_str() == status)
                && (load_type.is_empty() || load.r#type.as_str() == load_type)
                && client_id.is_none_or(|id| load.inventory_owner_id == id)
                && facility_id.is_none_or(|id| load.facility_id == id)
                && (date.is_empty()
                    || load
                        .appointment_time
                        .or(load.expected_time)
                        .unwrap_or(load.created)
                        .format("%Y-%m-%d")
                        .to_string()
                        == date)
                && (needle.is_empty()
                    || [
                        load.reference_number.as_deref(),
                        load.invoice_number.as_deref(),
                        load.carrier.as_deref(),
                        load.trailer_number.as_deref(),
                        load.seal_number.as_deref(),
                        load.inventory_owner_name.as_deref(),
                        load.facility_name.as_deref(),
                        Some(load.execution_barcode.as_str()),
                    ]
                    .into_iter()
                    .flatten()
                    .any(|value| value.to_ascii_lowercase().contains(&needle)))
        })
        .cloned()
        .collect()
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_case_keeps_wire_values_readable() {
        assert_eq!(title_case("inbound"), "Inbound");
        assert_eq!(title_case("receiving"), "Receiving");
    }
}
