use leptos::prelude::*;
use lucide_leptos::{Download, ExternalLink, Paperclip, Trash2};
use wareboxes_core::dto::{
    AddLoadFile, AddLoadLine, AddLoadNote, ArriveLoad, LoadFileIdRequest, LoadNoteIdRequest,
    LoadUpdate,
};
use wareboxes_core::models::{Item, Load, LoadFileCategory, LoadStatus, LoadType, Location};

use crate::api;
use crate::fulfillment_shared::{
    load_status_class, optional_text, optional_timestamp, order_destination,
    parse_optional_timestamp, short_timestamp, timestamp_input,
};
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadDetailTab {
    Header,
    Freight,
    Notes,
    Activity,
    Documents,
}

#[derive(Clone, Copy)]
struct LoadCommandContext {
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    confirmation: RwSignal<Option<LoadStatus>>,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
    toasts: crate::toast::ToastBus,
}

#[derive(Clone, Copy)]
struct DetailDeleteContext {
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    confirmation: RwSignal<Option<i64>>,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
    toasts: crate::toast::ToastBus,
}

#[component]
pub fn LoadDetailPanel(
    load: Load,
    catalog_items: Vec<Item>,
    locations: Vec<Location>,
    pending: RwSignal<bool>,
    load_error: RwSignal<Option<String>>,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let tab = RwSignal::new(LoadDetailTab::Header);
    let command_pending = RwSignal::new(false);
    let command_error = RwSignal::new(None::<String>);
    let arrival_open = RwSignal::new(false);
    let lifecycle_target = RwSignal::new(None::<LoadStatus>);
    let load_id = load.id;
    let toasts = use_toast_bus();
    let load = StoredValue::new(load);
    let catalog_items = StoredValue::new(catalog_items);
    let locations = StoredValue::new(locations);

    view! {
        <div class="fulfillment-detail-content">
            <div class="detail-heading">
                <div>
                    <span class="eyebrow">
                        {format!(
                            "{} load #{}",
                            title_case(load.get_value().r#type.as_str()),
                            load_id,
                        )}
                    </span>
                    <h2>{load.get_value().reference_number.unwrap_or_else(|| "No reference".to_owned())}</h2>
                </div>
                <span class=load_status_class(load.get_value().status)>
                    {title_case(load.get_value().status.as_str())}
                </span>
            </div>
            <dl class="detail-facts four-column">
                <div>
                    <dt>"Client"</dt>
                    <dd>{load.get_value().inventory_owner_name.unwrap_or_else(|| "Unassigned".to_owned())}</dd>
                </div>
                <div>
                    <dt>"Facility"</dt>
                    <dd>
                        {load
                            .get_value()
                            .facility_name
                            .unwrap_or_else(|| format!("#{}", load.get_value().facility_id))}
                    </dd>
                </div>
                <div>
                    <dt>"Appointment"</dt>
                    <dd>{optional_timestamp(load.get_value().appointment_time)}</dd>
                </div>
                <div>
                    <dt>"Execution barcode"</dt>
                    <dd class="mono">{load.get_value().execution_barcode}</dd>
                </div>
            </dl>
            <div class="detail-tabs" role="tablist" aria-label="Load detail sections">
                {[
                    (LoadDetailTab::Header, "Header"),
                    (LoadDetailTab::Freight, "Freight"),
                    (LoadDetailTab::Notes, "Notes"),
                    (LoadDetailTab::Activity, "Activity"),
                    (LoadDetailTab::Documents, "Documents"),
                ]
                    .into_iter()
                    .map(|(value, label)| {
                        view! {
                            <button
                                type="button"
                                role="tab"
                                aria-selected=move || (tab.get() == value).to_string()
                                class:active=move || tab.get() == value
                                on:click=move |_| tab.set(value)
                            >
                                {label}
                            </button>
                        }
                    })
                    .collect_view()}
            </div>

            <Show when=move || pending.get()>
                <div class="detail-loading" role="status">"Refreshing load..."</div>
            </Show>
            <Show when=move || load_error.get().is_some()>
                <p class="inline-command-error" role="alert">{move || load_error.get().unwrap_or_default()}</p>
            </Show>
            <Show when=move || command_error.get().is_some()>
                <p class="inline-command-error" role="alert">{move || command_error.get().unwrap_or_default()}</p>
            </Show>

            <Show when=move || tab.get() == LoadDetailTab::Header>
                <LoadHeaderForm
                    load=load.get_value()
                    locations=locations.get_value()
                    command_pending
                    command_error
                    on_refreshed
                    on_unauthorized
                />
                <Show when=move || {
                    load.get_value().r#type == LoadType::Inbound
                        && matches!(
                            load.get_value().status,
                            LoadStatus::Planned | LoadStatus::Scheduled
                        )
                }>
                    <section class="manager-action-band">
                        <div>
                            <strong>"Trailer check-in"</strong>
                        </div>
                        <button
                            type="button"
                            class="button primary-action"
                            on:click=move |_| arrival_open.set(true)
                        >
                            "Arrive load"
                        </button>
                    </section>
                </Show>
                <Show when=move || arrival_open.get()>
                    <ArrivalConfirmation
                        load=load.get_value()
                        pending=command_pending
                        error=command_error
                        on_close=Callback::new(move |_| arrival_open.set(false))
                        on_refreshed
                        on_unauthorized
                    />
                </Show>
                <Show when=move || {
                    !manager_actions(load.get_value().status, load.get_value().r#type).is_empty()
                }>
                    <section class="lifecycle-actions">
                        <h3>"Supervisor actions"</h3>
                        <div class="button-row">
                            {manager_actions(load.get_value().status, load.get_value().r#type)
                                .into_iter()
                                .map(|(target, label, danger)| {
                                    view! {
                                        <button
                                            type="button"
                                            class=if danger {
                                                "button danger-action"
                                            } else {
                                                "button secondary-action"
                                            }
                                            on:click=move |_| lifecycle_target.set(Some(target))
                                        >
                                            {label}
                                        </button>
                                    }
                                })
                                .collect_view()}
                        </div>
                    </section>
                </Show>
                <Show when=move || lifecycle_target.get().is_some()>
                    {move || {
                        lifecycle_target.get().map(|target| {
                            let label = title_case(target.as_str());
                            view! {
                                <section
                                    class="confirmation-panel"
                                    role="alertdialog"
                                    aria-labelledby="load-transition-title"
                                >
                                    <h3 id="load-transition-title">{format!("{label} this load?")}</h3>
                                    <p>{transition_confirmation(target)}</p>
                                    <div class="form-actions">
                                        <button
                                            type="button"
                                            class=if matches!(target, LoadStatus::Cancelled | LoadStatus::Rejected) {
                                                "button danger-action"
                                            } else {
                                                "button primary-action"
                                            }
                                            disabled=move || command_pending.get()
                                            on:click=move |_| {
                                                transition_load(load_id, target, LoadCommandContext {
                                                    pending: command_pending,
                                                    error: command_error,
                                                    confirmation: lifecycle_target,
                                                    on_refreshed,
                                                    on_unauthorized,
                                                    toasts,
                                                });
                                            }
                                        >
                                            {format!("Confirm {label}")}
                                        </button>
                                        <button
                                            type="button"
                                            class="button secondary-action"
                                            on:click=move |_| lifecycle_target.set(None)
                                        >
                                            "Go back"
                                        </button>
                                    </div>
                                </section>
                            }
                        })
                    }}
                </Show>
            </Show>

            <Show when=move || tab.get() == LoadDetailTab::Freight>
                <FreightPanel
                    load=load.get_value()
                    catalog_items=catalog_items.get_value()
                    pending=command_pending
                    error=command_error
                    on_refreshed
                    on_unauthorized
                />
            </Show>

            <Show when=move || tab.get() == LoadDetailTab::Notes>
                <NotesPanel
                    load=load.get_value()
                    pending=command_pending
                    error=command_error
                    on_refreshed
                    on_unauthorized
                />
            </Show>

            <Show when=move || tab.get() == LoadDetailTab::Activity>
                <section class="detail-section">
                    <div class="detail-section-title">
                        <h3>"Load activity"</h3>
                        <span>{format!("{} events", load.get_value().activity.len())}</span>
                    </div>
                    <ol class="activity-list">
                        {load
                            .get_value()
                            .activity
                            .into_iter()
                            .rev()
                            .map(|event| {
                                view! {
                                    <li>
                                        <span>{short_timestamp(event.created)}</span>
                                        <div>
                                            <strong>{title_case(&event.action)}</strong>
                                            {event.message.map(|message| view! { <small>{message}</small> })}
                                        </div>
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ol>
                    {load.get_value().activity.is_empty().then(|| {
                        view! { <p class="empty-state">"No load activity has been recorded."</p> }
                    })}
                </section>
            </Show>

            <Show when=move || tab.get() == LoadDetailTab::Documents>
                <DocumentsPanel
                    load=load.get_value()
                    pending=command_pending
                    error=command_error
                    on_refreshed
                    on_unauthorized
                />
            </Show>
        </div>
    }
}

#[component]
fn LoadHeaderForm(
    load: Load,
    locations: Vec<Location>,
    command_pending: RwSignal<bool>,
    command_error: RwSignal<Option<String>>,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let reference = RwSignal::new(load.reference_number.clone().unwrap_or_default());
    let invoice = RwSignal::new(load.invoice_number.clone().unwrap_or_default());
    let carrier = RwSignal::new(load.carrier.clone().unwrap_or_default());
    let trailer = RwSignal::new(load.trailer_number.clone().unwrap_or_default());
    let seal = RwSignal::new(load.seal_number.clone().unwrap_or_default());
    let dock = RwSignal::new(
        load.dock_door_location_id
            .map_or_else(String::new, |id| id.to_string()),
    );
    let expected = RwSignal::new(timestamp_input(load.expected_time));
    let appointment = RwSignal::new(timestamp_input(load.appointment_time));
    let load_id = load.id;
    let toasts = use_toast_bus();

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if command_pending.get_untracked() {
            return;
        }
        let dock_id = match dock.get_untracked().trim() {
            "" => None,
            value => match value.parse::<i64>() {
                Ok(value) => Some(value),
                Err(_) => {
                    command_error.set(Some("Choose a valid dock door.".to_owned()));
                    return;
                }
            },
        };
        let expected_time = match parse_optional_timestamp(&expected.get_untracked()) {
            Ok(value) => value,
            Err(message) => {
                command_error.set(Some(format!("Expected time: {message}")));
                return;
            }
        };
        let appointment_time = match parse_optional_timestamp(&appointment.get_untracked()) {
            Ok(value) => value,
            Err(message) => {
                command_error.set(Some(format!("Appointment time: {message}")));
                return;
            }
        };
        let request = LoadUpdate {
            load_id,
            status: None,
            r#type: None,
            reference_number: optional_text(&reference.get_untracked()),
            invoice_number: optional_text(&invoice.get_untracked()),
            carrier: optional_text(&carrier.get_untracked()),
            trailer_number: optional_text(&trailer.get_untracked()),
            seal_number: optional_text(&seal.get_untracked()),
            dock_door_location_id: dock_id,
            expected_time,
            appointment_time,
            actual_time: None,
            arrival: None,
            departure: None,
            rejected: None,
            closed: None,
        };
        command_pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, bool>("/api/loads/update", &request).await {
                Ok(true) => {
                    command_pending.set(false);
                    toasts.success(format!("Load #{load_id} header updated."));
                    on_refreshed.run(load_id);
                }
                Ok(false) => {
                    command_error.set(Some("The load could not be updated.".to_owned()));
                    command_pending.set(false);
                }
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => {
                    toasts.error(api_error.message.clone());
                    command_error.set(Some(api_error.message));
                    command_pending.set(false);
                }
            }
        });
    };

    view! {
        <form class="fulfillment-form detail-form" on:submit=submit>
            <div class="form-grid two-column">
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
                    <span>"Dock door"</span>
                    <select
                        prop:value=move || dock.get()
                        on:change=move |event| dock.set(event_target_value(&event))
                    >
                        <option value="">"Not assigned"</option>
                        {locations
                            .into_iter()
                            .filter(|location| {
                                location.facility_id == load.facility_id
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
                            .collect_view()}
                    </select>
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
            <dl class="detail-facts four-column compact-facts">
                <div><dt>"Arrival"</dt><dd>{optional_timestamp(load.arrival)}</dd></div>
                <div><dt>"Receiving started"</dt><dd>{optional_timestamp(load.actual_time)}</dd></div>
                <div><dt>"Departure"</dt><dd>{optional_timestamp(load.departure)}</dd></div>
                <div><dt>"Closed"</dt><dd>{optional_timestamp(load.closed)}</dd></div>
            </dl>
            <div class="form-actions">
                <button class="button primary-action" type="submit" disabled=move || command_pending.get()>
                    {move || if command_pending.get() { "Saving" } else { "Save header" }}
                </button>
            </div>
        </form>
    }
}

#[component]
fn ArrivalConfirmation(
    load: Load,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_close: Callback<()>,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let invoice = RwSignal::new(load.invoice_number.clone().unwrap_or_default());
    let arrival = RwSignal::new(String::new());
    let load_id = load.id;
    let toasts = use_toast_bus();

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let arrival_value = match parse_optional_timestamp(&arrival.get_untracked()) {
            Ok(value) => value,
            Err(message) => {
                error.set(Some(message));
                return;
            }
        };
        let request = ArriveLoad {
            invoice_number: optional_text(&invoice.get_untracked()),
            arrival: arrival_value,
        };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            let path = format!("/api/mobile/inbound/loads/{load_id}/arrive");
            match api::internal_post::<_, bool>(&path, &request).await {
                Ok(true) => {
                    pending.set(false);
                    on_close.run(());
                    toasts.success(format!("Load #{load_id} arrived."));
                    on_refreshed.run(load_id);
                }
                Ok(false) => {
                    error.set(Some(
                        "The load could not be arrived in its current state.".to_owned(),
                    ));
                    pending.set(false);
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
        <form
            class="confirmation-panel arrival-confirmation"
            role="alertdialog"
            aria-labelledby="arrive-load-title"
            on:submit=submit
        >
            <h3 id="arrive-load-title">"Confirm trailer arrival"</h3>
            <p>"This makes the load available to receiving operators."</p>
            <div class="form-grid two-column">
                <label>
                    <span>"Invoice"</span>
                    <input
                        prop:value=move || invoice.get()
                        on:input=move |event| invoice.set(event_target_value(&event))
                    />
                </label>
                <label>
                    <span>"Arrival time (UTC)"</span>
                    <input
                        type="datetime-local"
                        prop:value=move || arrival.get()
                        on:input=move |event| arrival.set(event_target_value(&event))
                    />
                    <small>"Leave blank to use server time."</small>
                </label>
            </div>
            <div class="form-actions">
                <button type="submit" class="button primary-action" disabled=move || pending.get()>
                    {move || if pending.get() { "Arriving" } else { "Confirm arrival" }}
                </button>
                <button type="button" class="button secondary-action" on:click=move |_| on_close.run(())>
                    "Go back"
                </button>
            </div>
        </form>
    }
}

#[component]
fn FreightPanel(
    load: Load,
    catalog_items: Vec<Item>,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    match load.r#type {
        LoadType::Inbound => view! {
            <InboundFreightPanel
                load
                catalog_items
                pending
                error
                on_refreshed
                on_unauthorized
            />
        }
        .into_any(),
        LoadType::Outbound => view! { <OutboundFreightPanel load/> }.into_any(),
    }
}

#[component]
fn InboundFreightPanel(
    load: Load,
    catalog_items: Vec<Item>,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let item_id = RwSignal::new(
        catalog_items
            .first()
            .map_or_else(String::new, |item| item.id.to_string()),
    );
    let sku_id = RwSignal::new(String::new());
    let quantity = RwSignal::new("1".to_owned());
    let lot = RwSignal::new(String::new());
    let serial = RwSignal::new(String::new());
    let expiration = RwSignal::new(String::new());
    let add_open = RwSignal::new(false);
    let load_id = load.id;
    let toasts = use_toast_bus();

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let Ok(selected_item) = item_id.get_untracked().parse::<i64>() else {
            error.set(Some("Choose an item.".to_owned()));
            return;
        };
        let selected_sku = match sku_id.get_untracked().trim() {
            "" => None,
            value => match value.parse::<i64>() {
                Ok(value) => Some(value),
                Err(_) => {
                    error.set(Some("SKU ID must be a positive number.".to_owned()));
                    return;
                }
            },
        };
        let Ok(expected_qty) = quantity.get_untracked().trim().parse::<i64>() else {
            error.set(Some("Expected quantity must be a whole number.".to_owned()));
            return;
        };
        if expected_qty <= 0 {
            error.set(Some(
                "Expected quantity must be greater than zero.".to_owned(),
            ));
            return;
        }
        let expiration_value = match parse_optional_timestamp(&expiration.get_untracked()) {
            Ok(value) => value,
            Err(message) => {
                error.set(Some(format!("Expiration: {message}")));
                return;
            }
        };
        let request = AddLoadLine {
            load_id,
            item_id: selected_item,
            sku_id: selected_sku,
            expected_qty,
            lot: optional_text(&lot.get_untracked()),
            serial: optional_text(&serial.get_untracked()),
            expiration: expiration_value,
        };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, i64>("/api/loads/lines/add", &request).await {
                Ok(line_id) => {
                    add_open.set(false);
                    pending.set(false);
                    toasts.success(format!(
                        "Expected line #{line_id} added to load #{load_id}."
                    ));
                    on_refreshed.run(load_id);
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
        <section class="detail-section">
            <div class="detail-section-title">
                <div>
                    <h3>"Expected freight"</h3>
                    <span>{format!("{} lines", load.lines.len())}</span>
                </div>
                <Show when=move || {
                    matches!(load.status, LoadStatus::Planned | LoadStatus::Scheduled | LoadStatus::Arrived)
                }>
                    <button
                        type="button"
                        class="button secondary-action"
                        on:click=move |_| add_open.set(true)
                    >
                        "Add line"
                    </button>
                </Show>
            </div>
            <div class="table-scroll">
                <table class="data-table detail-table freight-table">
                    <thead>
                        <tr>
                            <th>"Line"</th><th>"Item"</th><th>"Lot / serial"</th>
                            <th>"Expiration"</th><th>"State"</th>
                            <th class="numeric">"Expected"</th><th class="numeric">"Received"</th>
                            <th class="numeric">"Rejected"</th><th class="numeric">"Missing"</th>
                            <th>"Missing check"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {load
                            .lines
                            .clone()
                            .into_iter()
                            .map(|line| {
                                let item_label = catalog_items
                                    .iter()
                                    .find(|item| item.id == line.item_id)
                                    .and_then(|item| item.description.clone())
                                    .unwrap_or_else(|| format!("Item #{}", line.item_id));
                                let identity = [line.lot.as_deref(), line.serial.as_deref()]
                                    .into_iter()
                                    .flatten()
                                    .collect::<Vec<_>>()
                                    .join(" / ");
                                let missing_confirmation = if line.missing_qty <= 0 {
                                    "-".to_owned()
                                } else if let Some(confirmed_at) = line.missing_confirmed_at {
                                    line.missing_confirmed_by.map_or_else(
                                        || format!("Confirmed {}", short_timestamp(confirmed_at)),
                                        |user_id| {
                                            format!(
                                                "Confirmed {} by #{}",
                                                short_timestamp(confirmed_at),
                                                user_id
                                            )
                                        },
                                    )
                                } else {
                                    "Open".to_owned()
                                };
                                view! {
                                    <tr>
                                        <td>{format!("#{}", line.id)}</td>
                                        <td><strong>{item_label}</strong><small class="cell-detail">{format!("#{}", line.item_id)}</small></td>
                                        <td>{if identity.is_empty() { "-".to_owned() } else { identity }}</td>
                                        <td>{optional_timestamp(line.expiration)}</td>
                                        <td>{title_case(line.status.as_str())}</td>
                                        <td class="numeric strong">{format_quantity(line.expected_qty)}</td>
                                        <td class="numeric">{format_quantity(line.received_qty)}</td>
                                        <td class="numeric">{format_quantity(line.rejected_qty)}</td>
                                        <td class="numeric">{format_quantity(line.missing_qty)}</td>
                                        <td>{missing_confirmation}</td>
                                    </tr>
                                }
                            })
                            .collect_view()}
                    </tbody>
                </table>
                {load.lines.is_empty().then(|| {
                    view! { <p class="empty-state">"No expected freight lines have been added."</p> }
                })}
            </div>
            <div class="receiving-summary">
                <span>"Receiving progress"</span>
                <strong>
                    {format!(
                        "{} of {} received",
                        format_quantity(load.lines.iter().map(|line| line.received_qty).sum()),
                        format_quantity(load.lines.iter().map(|line| line.expected_qty).sum())
                    )}
                </strong>
                {load.receive_completed.then(|| view! { <span class="status shipped">"Complete"</span> })}
            </div>
            <Show when=move || add_open.get()>
                <form class="inline-command-form" on:submit=submit>
                    <div class="detail-section-title">
                        <h3>"Add expected line"</h3>
                        <button type="button" class="text-button" on:click=move |_| add_open.set(false)>
                            "Close"
                        </button>
                    </div>
                    <div class="form-grid three-column">
                        <label>
                            <span>"Item"</span>
                            <select
                                required
                                prop:value=move || item_id.get()
                                on:change=move |event| item_id.set(event_target_value(&event))
                            >
                                {catalog_items
                                    .clone()
                                    .into_iter()
                                    .map(|item| {
                                        let description = item
                                            .description
                                            .unwrap_or_else(|| format!("Item #{}", item.id));
                                        view! { <option value=item.id>{description}</option> }
                                    })
                                    .collect_view()}
                            </select>
                        </label>
                        <label>
                            <span>"SKU ID"</span>
                            <input
                                inputmode="numeric"
                                placeholder="Optional"
                                prop:value=move || sku_id.get()
                                on:input=move |event| sku_id.set(event_target_value(&event))
                            />
                        </label>
                        <label>
                            <span>"Expected quantity"</span>
                            <input
                                required
                                type="number"
                                min="1"
                                step="1"
                                prop:value=move || quantity.get()
                                on:input=move |event| quantity.set(event_target_value(&event))
                            />
                        </label>
                        <label>
                            <span>"Lot"</span>
                            <input
                                prop:value=move || lot.get()
                                on:input=move |event| lot.set(event_target_value(&event))
                            />
                        </label>
                        <label>
                            <span>"Serial"</span>
                            <input
                                prop:value=move || serial.get()
                                on:input=move |event| serial.set(event_target_value(&event))
                            />
                        </label>
                        <label>
                            <span>"Expiration (UTC)"</span>
                            <input
                                type="datetime-local"
                                prop:value=move || expiration.get()
                                on:input=move |event| expiration.set(event_target_value(&event))
                            />
                        </label>
                    </div>
                    <div class="form-actions">
                        <button class="button primary-action" type="submit" disabled=move || pending.get()>
                            {move || if pending.get() { "Adding" } else { "Add expected line" }}
                        </button>
                        <button
                            class="button secondary-action"
                            type="button"
                            on:click=move |_| add_open.set(false)
                        >
                            "Cancel"
                        </button>
                    </div>
                </form>
            </Show>
        </section>
    }
}

#[component]
fn OutboundFreightPanel(load: Load) -> impl IntoView {
    let empty = load.orders.is_empty();
    view! {
        <section class="detail-section">
            <div class="detail-section-title">
                <h3>"Orders and tracking"</h3>
                <span>{format!("{} orders", load.orders.len())}</span>
            </div>
            <div class="outbound-order-list">
                {load
                    .orders
                    .into_iter()
                    .map(|order| {
                        let destination = order_destination(&order);
                        let line_count = order.order_items.len();
                        let tracking = order.tracking_numbers;
                        view! {
                            <article>
                                <div class="outbound-order-heading">
                                    <strong>{order.order_key}</strong>
                                    <span class=crate::fulfillment_shared::order_status_class(order.status)>
                                        {title_case(order.status.as_str())}
                                    </span>
                                    {order.rush.then(|| view! { <small class="rush">"Rush"</small> })}
                                </div>
                                <dl class="outbound-order-facts">
                                    <div>
                                        <dt>"Client"</dt>
                                        <dd>{order.inventory_owner_name.unwrap_or_else(|| "Unassigned client".to_owned())}</dd>
                                    </div>
                                    <div>
                                        <dt>"Units"</dt>
                                        <dd>{format_quantity(order.ordered_qty)}</dd>
                                    </div>
                                    <div>
                                        <dt>"Lines"</dt>
                                        <dd>{format_quantity(line_count as i64)}</dd>
                                    </div>
                                    <div>
                                        <dt>"Ship by"</dt>
                                        <dd>{optional_timestamp(order.ship_by)}</dd>
                                    </div>
                                    <div class="wide">
                                        <dt>"Destination"</dt>
                                        <dd>{if destination.is_empty() { "Not assigned".to_owned() } else { destination }}</dd>
                                    </div>
                                </dl>
                                <div class="tracking-list compact">
                                    {tracking
                                        .into_iter()
                                        .map(|tracking| {
                                            view! {
                                                <div class="tracking-row">
                                                    <strong>{tracking.tracking_number}</strong>
                                                    <span>{tracking.carrier.unwrap_or_else(|| "Carrier not set".to_owned())}</span>
                                                    <span>{tracking.service.unwrap_or_else(|| "Service not set".to_owned())}</span>
                                                </div>
                                            }
                                        })
                                        .collect_view()}
                                </div>
                            </article>
                        }
                    })
                    .collect_view()}
            </div>
            {empty.then(|| {
                view! { <p class="empty-state">"No orders are linked to this outbound load."</p> }
            })}
        </section>
    }
}

#[component]
fn DocumentsPanel(
    load: Load,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let original_name = RwSignal::new(String::new());
    let path = RwSignal::new(String::new());
    let content_type = RwSignal::new(String::new());
    let category = RwSignal::new(LoadFileCategory::General.as_str().to_owned());
    let delete_target = RwSignal::new(None::<i64>);
    let load_id = load.id;
    let file_count = load.files.len();
    let empty = load.files.is_empty();
    let toasts = use_toast_bus();

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let name_value = original_name.get_untracked().trim().to_owned();
        if name_value.is_empty() {
            error.set(Some("Enter the document name.".to_owned()));
            return;
        }
        let path_value = path.get_untracked().trim().to_owned();
        if document_href(&path_value).is_none() {
            error.set(Some(
                "Enter a browser-safe storage URL or relative path.".to_owned(),
            ));
            return;
        }
        let request = AddLoadFile {
            load_id,
            original_name: name_value.clone(),
            name: stored_document_name(&name_value, &path_value),
            path: path_value,
            content_type: optional_text(&content_type.get_untracked()),
            category: LoadFileCategory::parse(&category.get_untracked()),
        };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, i64>("/api/loads/files/add", &request).await {
                Ok(file_id) => {
                    original_name.set(String::new());
                    path.set(String::new());
                    content_type.set(String::new());
                    category.set(LoadFileCategory::General.as_str().to_owned());
                    pending.set(false);
                    toasts.success(format!("Document #{file_id} attached to load #{load_id}."));
                    on_refreshed.run(load_id);
                }
                Err(api_error) if api_error.unauthorized => {
                    pending.set(false);
                    on_unauthorized.run(());
                }
                Err(api_error) => {
                    toasts.error(api_error.message.clone());
                    error.set(Some(api_error.message));
                    pending.set(false);
                }
            }
        });
    };

    view! {
        <section class="detail-section">
            <div class="detail-section-title">
                <h3>"Documents"</h3>
                <span>{format!("{file_count} records")}</span>
            </div>
            <form class="document-entry" on:submit=submit>
                <label>
                    <span>"Document name"</span>
                    <input
                        type="text"
                        placeholder="BOL-1042.pdf"
                        prop:value=move || original_name.get()
                        on:input=move |event| original_name.set(event_target_value(&event))
                    />
                </label>
                <label>
                    <span>"Category"</span>
                    <select
                        prop:value=move || category.get()
                        on:change=move |event| category.set(event_target_value(&event))
                    >
                        <option value="general">"General"</option>
                        <option value="invoice">"Invoice"</option>
                    </select>
                </label>
                <label class="document-path-field">
                    <span>"Storage URL or path"</span>
                    <input
                        type="text"
                        placeholder="/documents/BOL-1042.pdf"
                        prop:value=move || path.get()
                        on:input=move |event| path.set(event_target_value(&event))
                    />
                </label>
                <label>
                    <span>"Content type"</span>
                    <input
                        type="text"
                        placeholder="application/pdf"
                        prop:value=move || content_type.get()
                        on:input=move |event| content_type.set(event_target_value(&event))
                    />
                </label>
                <button
                    class="button primary-action document-attach-action"
                    type="submit"
                    disabled=move || pending.get()
                >
                    <Paperclip size=15/>
                    <span>{move || if pending.get() { "Attaching" } else { "Attach" }}</span>
                </button>
            </form>
            <div class="document-list">
                {load
                    .files
                    .into_iter()
                    .filter(|file| file.deleted.is_none())
                    .rev()
                    .map(|file| {
                        let file_id = file.id;
                        let href = document_href(&file.path);
                        let original_name = file.original_name;
                        let download_name = original_name.clone();
                        let link_title = format!("Open {original_name}");
                        let download_title = format!("Download {original_name}");
                        let delete_title = format!("Delete {original_name}");
                        view! {
                            <div class="document-row">
                                <div class="document-identity">
                                    <strong title=original_name.clone()>{original_name.clone()}</strong>
                                    <small>{file.content_type.unwrap_or_else(|| "Type not recorded".to_owned())}</small>
                                </div>
                                <span>{title_case(file.category.as_str())}</span>
                                <small>{short_timestamp(file.created)}</small>
                                <div class="document-actions">
                                    {href.map(|href| {
                                        let download_href = href.clone();
                                        view! {
                                            <a
                                                class="button document-action quiet-action"
                                                href=href
                                                target="_blank"
                                                rel="noopener noreferrer"
                                                aria-label=link_title
                                                title="Open document"
                                            >
                                                <ExternalLink size=14/>
                                            </a>
                                            <a
                                                class="button document-action quiet-action"
                                                href=download_href
                                                download=download_name
                                                aria-label=download_title
                                                title="Download document"
                                            >
                                                <Download size=14/>
                                            </a>
                                        }
                                    })}
                                    <Show
                                        when=move || delete_target.get() == Some(file_id)
                                        fallback=move || {
                                            let delete_title = delete_title.clone();
                                            view! {
                                                <button
                                                    class="button document-action danger-action"
                                                    type="button"
                                                    aria-label=delete_title
                                                    title="Delete document"
                                                    disabled=move || pending.get()
                                                    on:click=move |_| delete_target.set(Some(file_id))
                                                >
                                                    <Trash2 size=14/>
                                                </button>
                                            }
                                        }
                                    >
                                        <span class="inline-delete-confirmation">
                                            <strong>"Delete?"</strong>
                                            <button
                                                class="button danger-action"
                                                type="button"
                                                disabled=move || pending.get()
                                                on:click=move |_| {
                                                    delete_load_file(
                                                        file_id,
                                                        load_id,
                                                        DetailDeleteContext {
                                                            pending,
                                                            error,
                                                            confirmation: delete_target,
                                                            on_refreshed,
                                                            on_unauthorized,
                                                            toasts,
                                                        },
                                                    );
                                                }
                                            >
                                                "Yes"
                                            </button>
                                            <button
                                                class="button quiet-action"
                                                type="button"
                                                disabled=move || pending.get()
                                                on:click=move |_| delete_target.set(None)
                                            >
                                                "No"
                                            </button>
                                        </span>
                                    </Show>
                                </div>
                            </div>
                        }
                    })
                    .collect_view()}
            </div>
            {empty.then(|| {
                view! { <p class="empty-state">"No documents are attached to this load."</p> }
            })}
        </section>
    }
}

#[component]
fn NotesPanel(
    load: Load,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let note = RwSignal::new(String::new());
    let delete_target = RwSignal::new(None::<i64>);
    let load_id = load.id;
    let notes = load
        .notes
        .into_iter()
        .filter(|note| note.deleted.is_none())
        .rev()
        .collect::<Vec<_>>();
    let note_count = notes.len();
    let empty = notes.is_empty();
    let toasts = use_toast_bus();
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let note_value = note.get_untracked().trim().to_owned();
        if note_value.is_empty() {
            error.set(Some("Enter a note.".to_owned()));
            return;
        }
        let request = AddLoadNote {
            load_id,
            note: note_value,
        };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, i64>("/api/loads/notes/add", &request).await {
                Ok(note_id) => {
                    note.set(String::new());
                    pending.set(false);
                    toasts.success(format!("Note #{note_id} added to load #{load_id}."));
                    on_refreshed.run(load_id);
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
        <section class="detail-section">
            <div class="detail-section-title">
                <h3>"Load notes"</h3>
                <span>{format!("{note_count} notes")}</span>
            </div>
            <form class="note-entry" on:submit=submit>
                <label>
                    <span class="sr-only">"New load note"</span>
                    <textarea
                        rows="3"
                        placeholder="Add a receiving, carrier, or dock note"
                        prop:value=move || note.get()
                        on:input=move |event| note.set(event_target_value(&event))
                    ></textarea>
                </label>
                <button class="button primary-action" type="submit" disabled=move || pending.get()>
                    {move || if pending.get() { "Adding" } else { "Add note" }}
                </button>
            </form>
            <ol class="notes-list">
                {notes
                    .into_iter()
                    .map(|note| {
                        let note_id = note.id;
                        let delete_title = format!("Delete load note #{note_id}");
                        view! {
                            <li>
                                <span>{short_timestamp(note.created)}</span>
                                <p>{note.note}</p>
                                <div class="note-actions">
                                    <Show
                                        when=move || delete_target.get() == Some(note_id)
                                        fallback=move || {
                                            let delete_title = delete_title.clone();
                                            view! {
                                                <button
                                                    class="button document-action danger-action"
                                                    type="button"
                                                    aria-label=delete_title
                                                    title="Delete note"
                                                    disabled=move || pending.get()
                                                    on:click=move |_| delete_target.set(Some(note_id))
                                                >
                                                    <Trash2 size=14/>
                                                </button>
                                            }
                                        }
                                    >
                                        <span class="inline-delete-confirmation">
                                            <strong>"Delete?"</strong>
                                            <button
                                                class="button danger-action"
                                                type="button"
                                                disabled=move || pending.get()
                                                on:click=move |_| {
                                                    delete_load_note(
                                                        note_id,
                                                        load_id,
                                                        DetailDeleteContext {
                                                            pending,
                                                            error,
                                                            confirmation: delete_target,
                                                            on_refreshed,
                                                            on_unauthorized,
                                                            toasts,
                                                        },
                                                    );
                                                }
                                            >
                                                "Yes"
                                            </button>
                                            <button
                                                class="button quiet-action"
                                                type="button"
                                                disabled=move || pending.get()
                                                on:click=move |_| delete_target.set(None)
                                            >
                                                "No"
                                            </button>
                                        </span>
                                    </Show>
                                </div>
                            </li>
                        }
                    })
                    .collect_view()}
            </ol>
            {empty.then(|| {
                view! { <p class="empty-state">"No notes have been added to this load."</p> }
            })}
        </section>
    }
}

fn delete_load_file(file_id: i64, load_id: i64, context: DetailDeleteContext) {
    if context.pending.get_untracked() {
        return;
    }
    context.pending.set(true);
    context.error.set(None);
    leptos::task::spawn_local(async move {
        let request = LoadFileIdRequest { file_id };
        match api::internal_post::<_, bool>("/api/loads/files/delete", &request).await {
            Ok(true) => {
                context.pending.set(false);
                context.confirmation.set(None);
                context.toasts.success("Document deleted.");
                context.on_refreshed.run(load_id);
            }
            Ok(false) => {
                context
                    .error
                    .set(Some("The document could not be deleted.".to_owned()));
                context.pending.set(false);
            }
            Err(api_error) if api_error.unauthorized => {
                context.pending.set(false);
                context.on_unauthorized.run(());
            }
            Err(api_error) => {
                context.toasts.error(api_error.message.clone());
                context.error.set(Some(api_error.message));
                context.pending.set(false);
            }
        }
    });
}

fn delete_load_note(note_id: i64, load_id: i64, context: DetailDeleteContext) {
    if context.pending.get_untracked() {
        return;
    }
    context.pending.set(true);
    context.error.set(None);
    leptos::task::spawn_local(async move {
        let request = LoadNoteIdRequest {
            load_note_id: note_id,
        };
        match api::internal_post::<_, bool>("/api/loads/notes/delete", &request).await {
            Ok(true) => {
                context.pending.set(false);
                context.confirmation.set(None);
                context.toasts.success("Load note deleted.");
                context.on_refreshed.run(load_id);
            }
            Ok(false) => {
                context
                    .error
                    .set(Some("The note could not be deleted.".to_owned()));
                context.pending.set(false);
            }
            Err(api_error) if api_error.unauthorized => {
                context.pending.set(false);
                context.on_unauthorized.run(());
            }
            Err(api_error) => {
                context.toasts.error(api_error.message.clone());
                context.error.set(Some(api_error.message));
                context.pending.set(false);
            }
        }
    });
}

fn document_href(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty()
        || path.chars().any(char::is_control)
        || path.starts_with("//")
        || path.contains('\\')
    {
        return None;
    }
    let lowercase = path.to_ascii_lowercase();
    if lowercase.starts_with("http://") || lowercase.starts_with("https://") {
        return Some(path.to_owned());
    }
    (!path.contains(':')).then(|| path.to_owned())
}

fn stored_document_name(original_name: &str, path: &str) -> String {
    path.split(['/', '\\'])
        .next_back()
        .and_then(|segment| segment.split(['?', '#']).next())
        .filter(|segment| !segment.is_empty())
        .unwrap_or(original_name)
        .to_owned()
}

fn transition_load(load_id: i64, target: LoadStatus, context: LoadCommandContext) {
    if context.pending.get_untracked() {
        return;
    }
    let request = LoadUpdate {
        load_id,
        status: Some(target),
        r#type: None,
        reference_number: None,
        invoice_number: None,
        carrier: None,
        trailer_number: None,
        seal_number: None,
        dock_door_location_id: None,
        expected_time: None,
        appointment_time: None,
        actual_time: None,
        arrival: None,
        departure: None,
        rejected: None,
        closed: None,
    };
    context.pending.set(true);
    context.error.set(None);
    leptos::task::spawn_local(async move {
        match api::internal_post::<_, bool>("/api/loads/update", &request).await {
            Ok(true) => {
                context.pending.set(false);
                context.confirmation.set(None);
                context
                    .toasts
                    .success(format!("Load #{load_id} moved to {}.", target.as_str()));
                context.on_refreshed.run(load_id);
            }
            Ok(false) => {
                context
                    .error
                    .set(Some("The load could not be updated.".to_owned()));
                context.pending.set(false);
            }
            Err(api_error) if api_error.unauthorized => context.on_unauthorized.run(()),
            Err(api_error) => {
                context.toasts.error(api_error.message.clone());
                context.error.set(Some(api_error.message));
                context.pending.set(false);
            }
        }
    });
}

fn manager_actions(
    status: LoadStatus,
    load_type: LoadType,
) -> Vec<(LoadStatus, &'static str, bool)> {
    match (status, load_type) {
        (LoadStatus::Planned, LoadType::Inbound) => vec![
            (LoadStatus::Scheduled, "Schedule", false),
            (LoadStatus::Cancelled, "Cancel load", true),
        ],
        (LoadStatus::Scheduled, LoadType::Inbound) => {
            vec![(LoadStatus::Cancelled, "Cancel load", true)]
        }
        (LoadStatus::Planned, LoadType::Outbound) => vec![
            (LoadStatus::Scheduled, "Schedule", false),
            (LoadStatus::Arrived, "Mark at dock", false),
            (LoadStatus::Cancelled, "Cancel load", true),
        ],
        (LoadStatus::Scheduled, LoadType::Outbound) => vec![
            (LoadStatus::Arrived, "Mark at dock", false),
            (LoadStatus::Cancelled, "Cancel load", true),
        ],
        (LoadStatus::Arrived, _) => vec![
            (LoadStatus::Rejected, "Reject load", true),
            (LoadStatus::Cancelled, "Cancel load", true),
        ],
        (LoadStatus::Received | LoadStatus::Rejected, _) => {
            vec![(LoadStatus::Closed, "Close load", false)]
        }
        _ => Vec::new(),
    }
}

fn transition_confirmation(target: LoadStatus) -> &'static str {
    match target {
        LoadStatus::Scheduled => "The appointment will become ready for warehouse scheduling.",
        LoadStatus::Arrived => {
            "The load will be marked at the dock and ready for the next operation."
        }
        LoadStatus::Rejected => "The load will stop receiving and require exception resolution.",
        LoadStatus::Closed => "The load must be fully resolved before it can be closed.",
        LoadStatus::Cancelled => "The load will become terminal and cannot be resumed here.",
        _ => "Confirm this load state change.",
    }
}

fn title_case(value: &str) -> String {
    value
        .split(['_', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_links_allow_web_and_relative_references() {
        assert_eq!(
            document_href("https://files.example.test/BOL-1042.pdf"),
            Some("https://files.example.test/BOL-1042.pdf".to_owned())
        );
        assert_eq!(
            document_href("/documents/BOL-1042.pdf"),
            Some("/documents/BOL-1042.pdf".to_owned())
        );
        assert_eq!(
            document_href("documents/BOL-1042.pdf"),
            Some("documents/BOL-1042.pdf".to_owned())
        );
    }

    #[test]
    fn document_links_reject_active_content_and_malformed_references() {
        assert_eq!(document_href("javascript:alert(1)"), None);
        assert_eq!(document_href("data:text/plain,secret"), None);
        assert_eq!(document_href("//files.example.test/secret.pdf"), None);
        assert_eq!(document_href(r"\\files.example.test\secret.pdf"), None);
        assert_eq!(document_href("line\nbreak.pdf"), None);
        assert_eq!(document_href("  "), None);
    }

    #[test]
    fn stored_document_name_uses_the_storage_object_name() {
        assert_eq!(
            stored_document_name("Bill of lading.pdf", "/loads/42/bol-42.pdf?version=3"),
            "bol-42.pdf"
        );
        assert_eq!(
            stored_document_name("Bill of lading.pdf", "/loads/42/"),
            "Bill of lading.pdf"
        );
    }

    #[test]
    fn rf_receiving_transitions_are_not_supervisor_actions() {
        let actions = manager_actions(LoadStatus::Arrived, LoadType::Inbound);
        assert!(!actions
            .iter()
            .any(|(status, _, _)| *status == LoadStatus::Receiving));
        let actions = manager_actions(LoadStatus::Receiving, LoadType::Inbound);
        assert!(actions.is_empty());
    }

    #[test]
    fn terminal_transitions_require_confirmation_copy() {
        assert!(transition_confirmation(LoadStatus::Cancelled).contains("terminal"));
        assert!(transition_confirmation(LoadStatus::Closed).contains("resolved"));
    }
}
