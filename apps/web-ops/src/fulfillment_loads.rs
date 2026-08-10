use leptos::prelude::*;
use wareboxes_api_contract::web::access::{AccessScopeResource, AccessScopeWorkspace};
use wareboxes_core::dto::AddLoad;
use wareboxes_core::models::{Item, Load, LoadStatus, LoadType, Location};

use crate::api;
use crate::components::SearchField;
use crate::fulfillment_load_detail::LoadDetailPanel;
use crate::fulfillment_shared::{optional_text, parse_optional_timestamp, short_timestamp};
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;
use crate::workspace_layout::{PaneControls, SplitPaneHandle, SplitPaneState};

const LOAD_BATCH_SIZE: usize = 100;

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
    let selected_request_id = RwSignal::new(None::<i64>);
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
    let list_generation = RwSignal::new(0_u64);
    let next_offset = RwSignal::new(initial_count);
    let can_load_more = RwSignal::new(initial_count == LOAD_BATCH_SIZE);
    let sort = RwSignal::new(SortSpec {
        key: LoadSort::Appointment,
        direction: SortDirection::Ascending,
    });
    let layout = SplitPaneState::new("inbound-loads", 760);
    let filter_clients = StoredValue::new(access.inventory_owners.clone());
    let filter_facilities = StoredValue::new(access.facilities.clone());
    let create_clients = StoredValue::new(access.inventory_owners);
    let create_facilities = StoredValue::new(access.facilities);
    let detail_items = StoredValue::new(catalog_items);
    let detail_locations = StoredValue::new(locations.clone());
    let create_locations = StoredValue::new(locations);

    let request_list = Callback::new(move |append: bool| {
        let offset = if append {
            next_offset.get_untracked()
        } else {
            0
        };
        let generation = list_generation.get_untracked().wrapping_add(1);
        list_generation.set(generation);
        list_pending.set(true);
        list_error.set(None);
        let path = load_list_path(
            offset,
            LOAD_BATCH_SIZE,
            &search.get_untracked(),
            &status.get_untracked(),
            &load_type.get_untracked(),
            &client.get_untracked(),
            &facility.get_untracked(),
            &date.get_untracked(),
            sort.get_untracked(),
        );
        leptos::task::spawn_local(async move {
            match fetch_loads(&path).await {
                Ok(next) if list_generation.get_untracked() == generation => {
                    let count = next.len();
                    if append {
                        loads.update(|current| current.extend(next));
                    } else {
                        loads.set(next);
                    }
                    next_offset.set(offset + count);
                    can_load_more.set(count == LOAD_BATCH_SIZE);
                    list_pending.set(false);
                }
                Ok(_) => {}
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) if list_generation.get_untracked() == generation => {
                    list_error.set(Some(api_error.message));
                    list_pending.set(false);
                }
                Err(_) => {}
            }
        });
    });

    let open_load = move |load_id: i64| {
        create_open.set(false);
        layout.show_detail();
        selected_request_id.set(Some(load_id));
        request_load_detail(
            load_id,
            selected,
            selected_request_id,
            selected_pending,
            selected_error,
            on_unauthorized,
        );
    };

    let refresh_selected = Callback::new(move |load_id: i64| {
        selected_request_id.set(Some(load_id));
        request_load_detail(
            load_id,
            selected,
            selected_request_id,
            selected_pending,
            selected_error,
            on_unauthorized,
        );
        request_list.run(false);
    });

    let created = Callback::new(move |load_id: i64| {
        create_open.set(false);
        refresh_selected.run(load_id);
    });

    let load_more = move |_| request_list.run(true);

    view! {
        <div class="fulfillment-workbench loads-workbench split-workspace" class:create-mode=move || create_open.get() style=move || layout.style() data-pane-mode=move || layout.mode_attribute()>
            <section class="data-section fulfillment-list split-master">
                <div class="table-toolbar fulfillment-toolbar">
                    <div class="toolbar-summary">
                        <strong>{move || format_quantity(loads.get().len() as i64)}</strong>
                        <span>"loads shown"</span>
                        <PaneControls layout master_label="load table" detail_label="load detail"/>
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
                                on:change=move |event| { load_type.set(event_target_value(&event)); request_list.run(false); }
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
                                on:change=move |event| { status.set(event_target_value(&event)); request_list.run(false); }
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
                                on:change=move |event| { client.set(event_target_value(&event)); request_list.run(false); }
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
                                on:change=move |event| { facility.set(event_target_value(&event)); request_list.run(false); }
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
                                on:change=move |event| { date.set(event_target_value(&event)); request_list.run(false); }
                            />
                        </label>
                        <button
                            class="button secondary-action"
                            type="button"
                            disabled=move || list_pending.get()
                            on:click=move |_| request_list.run(false)
                        >
                            "Apply filters"
                        </button>
                        <button
                            class="button primary-action"
                            type="button"
                            on:click=move |_| {
                                selected.set(None);
                                selected_request_id.set(None);
                                create_open.set(true);
                                layout.show_detail();
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
                                    on_sort=Callback::new(move |_| select_load_sort(sort, LoadSort::Id, request_list))
                                />
                                <SortableHeader
                                    label="Direction"
                                    active=move || sort.get().key == LoadSort::Type
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| select_load_sort(sort, LoadSort::Type, request_list))
                                />
                                <SortableHeader
                                    label="Reference"
                                    active=move || sort.get().key == LoadSort::Reference
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| select_load_sort(sort, LoadSort::Reference, request_list))
                                />
                                <SortableHeader
                                    label="Client"
                                    active=move || sort.get().key == LoadSort::Client
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| select_load_sort(sort, LoadSort::Client, request_list))
                                />
                                <SortableHeader
                                    label="Facility"
                                    active=move || sort.get().key == LoadSort::Facility
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| select_load_sort(sort, LoadSort::Facility, request_list))
                                />
                                <SortableHeader
                                    label="Status"
                                    active=move || sort.get().key == LoadSort::Status
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| select_load_sort(sort, LoadSort::Status, request_list))
                                />
                                <SortableHeader
                                    label="Appointment"
                                    active=move || sort.get().key == LoadSort::Appointment
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| select_load_sort(sort, LoadSort::Appointment, request_list))
                                />
                            </tr>
                        </thead>
                        <tbody>
                            {move || {
                                let selected_id = selected.get().map(|load| load.id);
                                loads.get()
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
                    <Show when=move || loads.get().is_empty()>
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
            <SplitPaneHandle layout/>
            <aside class="command-panel fulfillment-detail load-detail-panel split-detail">
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

fn select_load_sort(
    sort: RwSignal<SortSpec<LoadSort>>,
    key: LoadSort,
    request_list: Callback<bool>,
) {
    SortSpec::select(sort, key);
    request_list.run(false);
}

#[allow(clippy::too_many_arguments)]
fn load_list_path(
    offset: usize,
    limit: usize,
    search: &str,
    status: &str,
    load_type: &str,
    client: &str,
    facility: &str,
    date: &str,
    sort: SortSpec<LoadSort>,
) -> String {
    let mut parameters = vec![
        format!("offset={offset}"),
        format!("limit={limit}"),
        format!("sort={}", load_sort_value(sort.key)),
        format!("direction={}", sort_direction_value(sort.direction)),
    ];
    for (name, value) in [
        ("search", search.trim()),
        ("status", status),
        ("load_type", load_type),
        ("inventory_owner_id", client),
        ("facility_id", facility),
        ("appointment_date", date),
    ] {
        if !value.is_empty() {
            parameters.push(format!("{name}={}", urlencoding::encode(value)));
        }
    }
    format!("/api/loads?{}", parameters.join("&"))
}

const fn load_sort_value(value: LoadSort) -> &'static str {
    match value {
        LoadSort::Id => "id",
        LoadSort::Type => "type",
        LoadSort::Reference => "reference",
        LoadSort::Client => "inventory_owner",
        LoadSort::Facility => "facility",
        LoadSort::Status => "status",
        LoadSort::Appointment => "appointment",
    }
}

const fn sort_direction_value(value: SortDirection) -> &'static str {
    match value {
        SortDirection::Ascending => "asc",
        SortDirection::Descending => "desc",
    }
}

async fn fetch_loads(path: &str) -> Result<Vec<Load>, api::ApiError> {
    api::internal_get(path).await
}

fn request_load_detail(
    load_id: i64,
    selected: RwSignal<Option<Load>>,
    selected_request_id: RwSignal<Option<i64>>,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_unauthorized: Callback<()>,
) {
    pending.set(true);
    error.set(None);
    leptos::task::spawn_local(async move {
        match api::internal_get::<Option<Load>>(&format!("/api/loads/{load_id}")).await {
            Ok(Some(load)) if selected_request_id.get_untracked() == Some(load_id) => {
                selected.set(Some(load));
                pending.set(false);
            }
            Ok(None) if selected_request_id.get_untracked() == Some(load_id) => {
                selected.set(None);
                error.set(Some(
                    "Load not found or outside your warehouse scope.".to_owned(),
                ));
                pending.set(false);
            }
            Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
            Err(api_error) if selected_request_id.get_untracked() == Some(load_id) => {
                error.set(Some(api_error.message));
                pending.set(false);
            }
            Ok(Some(_)) | Ok(None) | Err(_) => {}
        }
    });
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

    #[test]
    fn list_path_carries_sort_and_filters_to_the_server() {
        let path = load_list_path(
            100,
            100,
            "  trailer 7 ",
            "arrived",
            "inbound",
            "12",
            "9",
            "2026-08-10",
            SortSpec {
                key: LoadSort::Appointment,
                direction: SortDirection::Descending,
            },
        );
        assert!(path.contains("offset=100&limit=100"));
        assert!(path.contains("sort=appointment&direction=desc"));
        assert!(path.contains("search=trailer%207"));
        assert!(path.contains("inventory_owner_id=12"));
        assert!(path.contains("appointment_date=2026-08-10"));
    }
}
