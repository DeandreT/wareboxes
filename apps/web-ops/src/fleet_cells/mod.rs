mod display;
mod forms;

use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    ChangeDataCellStatusRequest, DataCellEventPage, DataCellEventPageRequest, DataCellPage,
    DataCellPageRequest, DataCellResponse, DataCellStatus, OpaqueCursor,
    ReconfigureDataCellRequest, RegisterDataCellRequest,
};

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::{use_toast_bus, ToastBus};

#[derive(Clone)]
pub(super) enum Dialog {
    Register,
    Reconfigure(Box<DataCellResponse>),
    Status(Box<DataCellResponse>, DataCellStatus),
}

#[derive(Clone)]
pub(super) enum PendingCommand {
    Register(RegisterDataCellRequest, String),
    Reconfigure(i64, ReconfigureDataCellRequest, String),
    Status(i64, ChangeDataCellStatusRequest, String),
}

#[derive(Clone, Copy)]
pub(super) struct Signals {
    cells: RwSignal<DataCellPage>,
    events: RwSignal<DataCellEventPage>,
    selected: RwSignal<Option<DataCellResponse>>,
    status: RwSignal<Option<DataCellStatus>>,
    region: RwSignal<String>,
    applied_status: RwSignal<Option<DataCellStatus>>,
    applied_region: RwSignal<String>,
    loading: RwSignal<bool>,
    loaded: RwSignal<bool>,
    detail_loading: RwSignal<bool>,
    events_loading: RwSignal<bool>,
    list_generation: RwSignal<u64>,
    detail_generation: RwSignal<u64>,
    event_generation: RwSignal<u64>,
    error: RwSignal<Option<String>>,
    dialog: RwSignal<Option<Dialog>>,
    command_pending: RwSignal<bool>,
    command_error: RwSignal<Option<String>>,
    retry: RwSignal<Option<PendingCommand>>,
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

#[component]
pub(crate) fn FleetCellsWorkspace(
    initial_page: Option<DataCellPage>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let has_initial = initial_page.is_some();
    let signals = Signals {
        cells: RwSignal::new(initial_page.unwrap_or_else(|| DataCellPage::new(Vec::new(), None))),
        events: RwSignal::new(DataCellEventPage::new(Vec::new(), None)),
        selected: RwSignal::new(None),
        status: RwSignal::new(None),
        region: RwSignal::new(String::new()),
        applied_status: RwSignal::new(None),
        applied_region: RwSignal::new(String::new()),
        loading: RwSignal::new(!has_initial),
        loaded: RwSignal::new(has_initial),
        detail_loading: RwSignal::new(false),
        events_loading: RwSignal::new(false),
        list_generation: RwSignal::new(0),
        detail_generation: RwSignal::new(0),
        event_generation: RwSignal::new(0),
        error: RwSignal::new(None),
        dialog: RwSignal::new(None),
        command_pending: RwSignal::new(false),
        command_error: RwSignal::new(None),
        retry: RwSignal::new(None),
        on_unauthorized,
        toasts: use_toast_bus(),
    };
    let drafts = forms::Drafts::new();
    Effect::new(move |_| {
        if !has_initial {
            refresh(signals);
        }
    });
    let register = move |_| {
        drafts.reset_register();
        signals.command_error.set(None);
        signals.retry.set(None);
        signals.dialog.set(Some(Dialog::Register));
    };
    let apply = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        invalidate_detail(signals);
        apply_filters(signals);
    };
    let retry = move |_| {
        if let Some(command) = signals.retry.get_untracked() {
            dispatch(signals, command);
        }
    };
    view! {
        <section class="fleet-cells-workspace">
            <header class="page-heading fleet-cell-heading"><div><p class="eyebrow">"Fleet control"</p><h1>"Data cells"</h1><p>"Govern tenant placement capacity, isolation mode, region, residency, lifecycle, and immutable operational evidence."</p></div><div><button class="button primary-action" type="button" on:click=register>"Register cell"</button><button class="button secondary-action" type="button" disabled=move || signals.loading.get() on:click=move |_| refresh(signals)><Icon icon=UiIcon::Refresh/><span>"Refresh"</span></button></div></header>
            {move || metrics(signals)}
            <form class="fleet-cell-toolbar" on:submit=apply><label><span>"Region"</span><input maxlength="32" placeholder="All regions" prop:value=move || signals.region.get() on:input=move |event| signals.region.set(event_target_value(&event).to_ascii_lowercase())/></label><label><span>"Status"</span><select prop:value=move || status_wire(signals.status.get()) on:change=move |event| signals.status.set(parse_status(&event_target_value(&event)))><option value="">"All statuses"</option><option value="provisioning">"Provisioning"</option><option value="active">"Active"</option><option value="draining">"Draining"</option><option value="retired">"Retired"</option></select></label><button class="button secondary-action compact" type="submit">"Apply"</button></form>
            <Show when=move || signals.error.get().is_some()><section class="fleet-cell-error" role="alert"><span>{move || signals.error.get().unwrap_or_default()}</span><button class="text-button" type="button" on:click=move |_| refresh(signals)>"Retry reads"</button></section></Show>
            <div class="fleet-cell-layout">{move || cell_panel(signals)}{move || detail_panel(signals,drafts)}</div>
            <Show when=move || signals.retry.get().is_some()><section class="fleet-cell-retry"><span>"The last command outcome is ambiguous. Exact retry preserves its body and idempotency key."</span><button class="button secondary-action compact" type="button" disabled=move || signals.command_pending.get() on:click=retry>"Retry exact command"</button></section></Show>
            {move || signals.dialog.get().map(|dialog|forms::dialog(signals,drafts,dialog))}
        </section>
    }
}

fn metrics(signals: Signals) -> AnyView {
    let cells = signals.cells.get().items;
    let active = cells
        .iter()
        .filter(|cell| cell.status == DataCellStatus::Active)
        .count();
    let draining = cells
        .iter()
        .filter(|cell| cell.status == DataCellStatus::Draining)
        .count();
    let placements: i64 = cells.iter().map(|cell| cell.placement_count).sum();
    let open: u64 = cells
        .iter()
        .filter(|cell| cell.status == DataCellStatus::Active)
        .map(|cell| u64::from(cell.available_tenant_slots))
        .sum();
    view! { <section class="fleet-cell-metrics"><article><span>"Active cells loaded"</span><strong>{active}</strong></article><article><span>"Draining loaded"</span><strong>{draining}</strong></article><article><span>"Tenant placements loaded"</span><strong>{placements}</strong></article><article><span>"Open slots loaded"</span><strong>{open}</strong></article></section> }.into_any()
}

fn cell_panel(signals: Signals) -> AnyView {
    if signals.loading.get() && !signals.loaded.get() {
        return state("Loading data cells", true);
    }
    let page = signals.cells.get();
    let next = page.next_cursor.clone();
    let count = page.items.len();
    let content = if page.items.is_empty() {
        state("No data cells match these filters.", false)
    } else {
        view! { <div class="table-scroll"><table class="dense-table"><caption class="sr-only">"Data cells in the current filtered page"</caption><thead><tr><th>"Cell"</th><th>"Region / residency"</th><th>"Status"</th><th>"Capacity"</th><th></th></tr></thead><tbody>{page.items.into_iter().map(|cell|{let id=cell.data_cell_id;let selected=signals.selected.get().is_some_and(|value|value.data_cell_id==id);view!{<tr class:selected=selected><td><strong>{cell.name.clone()}</strong><small>{format!("{} · {}",cell.key,display::mode_label(cell.mode))}</small></td><td>{cell.region.clone()}<small>{cell.residency.clone()}</small></td><td><span class=display::status_class(cell.status)>{display::status_label(cell.status)}</span></td><td>{display::capacity(&cell)}</td><td><button class="text-button" type="button" on:click=move |_|load_detail(signals,id)>"Inspect"</button></td></tr>}}).collect_view()}</tbody></table></div>}.into_any()
    };
    view! { <section class="fleet-cell-panel fleet-cell-list"><header><div><h2>"Registry"</h2><span>{format!("{count} loaded")}</span></div>{next.map(|cursor|view!{<button class="text-button" type="button" disabled=move || signals.loading.get() on:click=move |_|load_page(signals,Some(cursor.clone()),true)>"Load more"</button>})}</header>{content}</section> }.into_any()
}

fn detail_panel(signals: Signals, drafts: forms::Drafts) -> AnyView {
    if signals.detail_loading.get() {
        return state("Loading cell evidence", true);
    }
    let Some(cell) = signals.selected.get() else {
        return view!{<section class="fleet-cell-panel fleet-cell-detail empty-detail"><Icon icon=UiIcon::Building/><strong>"Select a data cell"</strong><span>"Inspect placement capacity, immutable identity, lifecycle, and attributed changes."</span></section>}.into_any();
    };
    let reconfigure = cell.clone();
    let activate = cell.clone();
    let drain = cell.clone();
    let reactivate = cell.clone();
    let retire = cell.clone();
    let events = signals.events.get();
    let next = events.next_cursor.clone();
    let event_count = events.items.len();
    view! { <section class="fleet-cell-panel fleet-cell-detail"><header><div><p class="eyebrow">{cell.key.clone()}</p><h2>{cell.name.clone()}</h2><span class=display::status_class(cell.status)>{display::status_label(cell.status)}</span></div><div class="fleet-cell-actions">{(cell.status!=DataCellStatus::Retired).then(||view!{<button class="button secondary-action compact" type="button" on:click=move |_|{drafts.reset_reconfigure(&reconfigure);signals.command_error.set(None);signals.retry.set(None);signals.dialog.set(Some(Dialog::Reconfigure(Box::new(reconfigure.clone()))));}>"Reconfigure"</button>})}{(cell.status==DataCellStatus::Provisioning).then(||status_button(signals,drafts,activate,DataCellStatus::Active,"Activate",false))}{(cell.status==DataCellStatus::Active).then(||status_button(signals,drafts,drain,DataCellStatus::Draining,"Drain",true))}{(cell.status==DataCellStatus::Draining).then(||view!{<>{status_button(signals,drafts,reactivate,DataCellStatus::Active,"Reactivate",false)}{status_button(signals,drafts,retire,DataCellStatus::Retired,"Retire",true)}</>})}</div></header><dl class="fleet-cell-facts"><div><dt>"Cell ID"</dt><dd>{cell.data_cell_id}</dd></div><div><dt>"Revision"</dt><dd>{cell.revision.get()}</dd></div><div><dt>"Region"</dt><dd>{cell.region.clone()}</dd></div><div><dt>"Residency"</dt><dd>{cell.residency.clone()}</dd></div><div><dt>"Isolation"</dt><dd>{display::mode_label(cell.mode)}</dd></div><div><dt>"Capacity"</dt><dd>{display::capacity(&cell)}</dd></div><div><dt>"Registered"</dt><dd>{display::short_timestamp(&cell.created_at)}</dd></div><div><dt>"Last reason"</dt><dd>{cell.change_reason.clone().unwrap_or_else(||"Initial registration".into())}</dd></div></dl><section class="fleet-cell-evidence"><header><div><h3>"Immutable evidence"</h3><span>{format!("{event_count} events loaded")}</span></div>{next.map(|cursor|{let id=cell.data_cell_id;view!{<button class="text-button" type="button" disabled=move || signals.events_loading.get() on:click=move |_|load_events(signals,id,Some(cursor.clone()),true)>"Load more"</button>}})}</header>{if signals.events_loading.get()&&events.items.is_empty(){state("Loading events",true)}else if events.items.is_empty(){state("No evidence is available.",false)}else{view!{<ol>{events.items.into_iter().map(|event|view!{<li><div><strong>{event.action.replace('_'," ")}</strong><span>{display::short_timestamp(&event.occurred_at)}</span></div><p>{event.reason.unwrap_or_else(||"Cell registered".into())}</p><small>{format!("Revision {} · {}",event.cell_revision.get(),event.actor_id.map(|id|format!("User #{id}")).unwrap_or_else(||"System".into()))}</small></li>}).collect_view()}</ol>}.into_any()}}</section></section> }.into_any()
}

fn status_button(
    signals: Signals,
    drafts: forms::Drafts,
    cell: DataCellResponse,
    status: DataCellStatus,
    label: &'static str,
    danger: bool,
) -> AnyView {
    view!{<button class=if danger{"button danger-action compact"}else{"button primary-action compact"} type="button" on:click=move |_|{drafts.reset_reason();signals.command_error.set(None);signals.retry.set(None);signals.dialog.set(Some(Dialog::Status(Box::new(cell.clone()),status)));}>{label}</button>}.into_any()
}

fn invalidate_detail(signals: Signals) {
    signals.detail_generation.update(|value| *value += 1);
    signals.event_generation.update(|value| *value += 1);
    signals.detail_loading.set(false);
    signals.events_loading.set(false);
    signals.selected.set(None);
    signals.events.set(DataCellEventPage::new(Vec::new(), None));
}
fn refresh(signals: Signals) {
    let selected_id = signals
        .selected
        .get_untracked()
        .map(|cell| cell.data_cell_id);
    load_page(signals, None, false);
    if let Some(id) = selected_id {
        load_detail(signals, id);
    }
}

fn apply_filters(signals: Signals) {
    signals.applied_status.set(signals.status.get_untracked());
    signals
        .applied_region
        .set(signals.region.get_untracked().trim().to_owned());
    load_page(signals, None, false);
}
fn load_page(signals: Signals, cursor: Option<OpaqueCursor>, append: bool) {
    signals.list_generation.update(|value| *value += 1);
    let generation = signals.list_generation.get_untracked();
    signals.loading.set(true);
    signals.error.set(None);
    let region = signals.applied_region.get_untracked();
    let request = DataCellPageRequest {
        status: signals.applied_status.get_untracked(),
        region: (!region.is_empty()).then_some(region),
        cursor,
        limit: wareboxes_api_contract::v1::PageLimit::default(),
    };
    leptos::task::spawn_local(async move {
        match api::data_cells(&request).await {
            Ok(page) if signals.list_generation.get_untracked() == generation => {
                if append {
                    signals.cells.update(|current| {
                        current.items.extend(page.items);
                        current.next_cursor = page.next_cursor;
                    });
                } else {
                    signals.cells.set(page);
                }
            }
            Err(error) if signals.list_generation.get_untracked() == generation => {
                handle_read_error(signals, error)
            }
            _ => {}
        }
        if signals.list_generation.get_untracked() == generation {
            signals.loading.set(false);
            signals.loaded.set(true);
        }
    });
}
fn load_detail(signals: Signals, id: i64) {
    signals.detail_generation.update(|value| *value += 1);
    let generation = signals.detail_generation.get_untracked();
    signals.detail_loading.set(true);
    signals.selected.set(None);
    signals.event_generation.update(|value| *value += 1);
    signals.events.set(DataCellEventPage::new(Vec::new(), None));
    leptos::task::spawn_local(async move {
        match api::data_cell(id).await {
            Ok(cell) if signals.detail_generation.get_untracked() == generation => {
                signals.selected.set(Some(cell));
                load_events(signals, id, None, false);
            }
            Err(error) if signals.detail_generation.get_untracked() == generation => {
                handle_read_error(signals, error)
            }
            _ => {}
        }
        if signals.detail_generation.get_untracked() == generation {
            signals.detail_loading.set(false);
        }
    });
}
fn load_events(signals: Signals, id: i64, cursor: Option<OpaqueCursor>, append: bool) {
    signals.event_generation.update(|value| *value += 1);
    let generation = signals.event_generation.get_untracked();
    signals.events_loading.set(true);
    let request = DataCellEventPageRequest {
        cursor,
        limit: wareboxes_api_contract::v1::PageLimit::default(),
    };
    leptos::task::spawn_local(async move {
        match api::data_cell_events(id, &request).await {
            Ok(page) if signals.event_generation.get_untracked() == generation => {
                if append {
                    signals.events.update(|current| {
                        current.items.extend(page.items);
                        current.next_cursor = page.next_cursor;
                    });
                } else {
                    signals.events.set(page);
                }
            }
            Err(error) if signals.event_generation.get_untracked() == generation => {
                handle_read_error(signals, error)
            }
            _ => {}
        }
        if signals.event_generation.get_untracked() == generation {
            signals.events_loading.set(false);
        }
    });
}

pub(super) fn dispatch(signals: Signals, command: PendingCommand) {
    if signals.command_pending.get_untracked() {
        return;
    }
    signals.command_pending.set(true);
    signals.command_error.set(None);
    signals.retry.set(Some(command.clone()));
    leptos::task::spawn_local(async move {
        match execute(&command).await {
            Ok(cell) => {
                signals.toasts.success("Data-cell registry updated.");
                signals.retry.set(None);
                signals.dialog.set(None);
                refresh_cell(signals, cell.clone());
                signals.selected.set(Some(cell.clone()));
                load_events(signals, cell.data_cell_id, None, false);
            }
            Err(error) => {
                if error.unauthorized {
                    signals.on_unauthorized.run(());
                }
                signals.command_error.set(Some(error.message.clone()));
                if !error.ambiguous_outcome {
                    signals.retry.set(None);
                }
            }
        }
        signals.command_pending.set(false);
    });
}
async fn execute(command: &PendingCommand) -> Result<DataCellResponse, api::ApiError> {
    match command {
        PendingCommand::Register(request, key) => api::register_data_cell(request, key).await,
        PendingCommand::Reconfigure(id, request, key) => {
            api::reconfigure_data_cell(*id, request, key).await
        }
        PendingCommand::Status(id, request, key) => {
            api::change_data_cell_status(*id, request, key).await
        }
    }
}
fn refresh_cell(signals: Signals, cell: DataCellResponse) {
    let status = signals.applied_status.get_untracked();
    let region = signals.applied_region.get_untracked();
    let matches = matches_filters(status, &region, cell.status, &cell.region);
    signals.cells.update(|page| {
        page.items
            .retain(|candidate| candidate.data_cell_id != cell.data_cell_id);
        if matches {
            page.items.insert(0, cell);
        }
    });
}

fn matches_filters(
    filter_status: Option<DataCellStatus>,
    filter_region: &str,
    cell_status: DataCellStatus,
    cell_region: &str,
) -> bool {
    filter_status.is_none_or(|status| status == cell_status)
        && (filter_region.trim().is_empty() || filter_region.trim() == cell_region)
}
fn handle_read_error(signals: Signals, error: api::ApiError) {
    if error.unauthorized {
        signals.on_unauthorized.run(());
    }
    signals.error.set(Some(error.message));
}
fn state(label: &'static str, loading: bool) -> AnyView {
    view!{<section class="fleet-cell-state" aria-busy=loading><Show when=move ||loading><span class="loading-line"></span></Show><strong>{label}</strong></section>}.into_any()
}
fn status_wire(status: Option<DataCellStatus>) -> &'static str {
    match status {
        None => "",
        Some(DataCellStatus::Provisioning) => "provisioning",
        Some(DataCellStatus::Active) => "active",
        Some(DataCellStatus::Draining) => "draining",
        Some(DataCellStatus::Retired) => "retired",
    }
}
fn parse_status(value: &str) -> Option<DataCellStatus> {
    match value {
        "provisioning" => Some(DataCellStatus::Provisioning),
        "active" => Some(DataCellStatus::Active),
        "draining" => Some(DataCellStatus::Draining),
        "retired" => Some(DataCellStatus::Retired),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconciliation_respects_applied_status_and_region_filters() {
        assert!(matches_filters(
            Some(DataCellStatus::Active),
            "us-west",
            DataCellStatus::Active,
            "us-west"
        ));
        assert!(!matches_filters(
            Some(DataCellStatus::Draining),
            "us-west",
            DataCellStatus::Active,
            "us-west"
        ));
        assert!(!matches_filters(
            None,
            "eu-central",
            DataCellStatus::Active,
            "us-west"
        ));
        assert!(matches_filters(
            None,
            "  ",
            DataCellStatus::Retired,
            "us-west"
        ));
    }
}
