use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    ConfigureIntegrationOrderItemMappingRequest, IntegrationOrderItemMappingPage,
    IntegrationOrderItemMappingResponse, IntegrationOrderItemMappingStatus, OpaqueCursor,
    OrderEntryItemResponse, RetireIntegrationOrderItemMappingRequest,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;

use crate::api;
#[cfg(target_arch = "wasm32")]
use crate::api::IntegrationMappingFilters;
use crate::components::{Icon, UiIcon};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditorMode {
    Create,
    Reconfigure,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingCommand {
    Configure {
        request: ConfigureIntegrationOrderItemMappingRequest,
        key: String,
    },
    Retire {
        mapping_id: i64,
        request: RetireIntegrationOrderItemMappingRequest,
        key: String,
    },
}

#[derive(Clone, Copy)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(
        dead_code,
        reason = "browser requests consume generation and session signals"
    )
)]
struct MappingSignals {
    access: RwSignal<AccessScopeWorkspace>,
    page: RwSignal<IntegrationOrderItemMappingPage>,
    owner_filter: RwSignal<Option<i64>>,
    source_filter: RwSignal<String>,
    status_filter: RwSignal<IntegrationOrderItemMappingStatus>,
    cursor: RwSignal<Option<OpaqueCursor>>,
    history: RwSignal<Vec<Option<OpaqueCursor>>>,
    page_generation: RwSignal<u64>,
    access_generation: RwSignal<u64>,
    item_generation: RwSignal<u64>,
    loading: RwSignal<bool>,
    access_loading: RwSignal<bool>,
    items_loading: RwSignal<bool>,
    command_pending: RwSignal<bool>,
    items: RwSignal<Vec<OrderEntryItemResponse>>,
    selected: RwSignal<Option<IntegrationOrderItemMappingResponse>>,
    editor: RwSignal<Option<EditorMode>>,
    confirm_retire: RwSignal<bool>,
    retry: RwSignal<Option<PendingCommand>>,
    error: RwSignal<Option<String>>,
    notice: RwSignal<Option<String>>,
    on_unauthorized: Callback<()>,
}

#[derive(Clone, Copy)]
struct MappingDraft {
    owner_id: RwSignal<String>,
    source_key: RwSignal<String>,
    external_item_key: RwSignal<String>,
    external_uom: RwSignal<String>,
    item_id: RwSignal<String>,
    item_search: RwSignal<String>,
}

impl MappingSignals {
    fn new(on_unauthorized: Callback<()>) -> Self {
        Self {
            access: RwSignal::new(AccessScopeWorkspace::default()),
            page: RwSignal::new(IntegrationOrderItemMappingPage::new(Vec::new(), None)),
            owner_filter: RwSignal::new(None),
            source_filter: RwSignal::new(String::new()),
            status_filter: RwSignal::new(IntegrationOrderItemMappingStatus::Active),
            cursor: RwSignal::new(None),
            history: RwSignal::new(Vec::new()),
            page_generation: RwSignal::new(0),
            access_generation: RwSignal::new(0),
            item_generation: RwSignal::new(0),
            loading: RwSignal::new(false),
            access_loading: RwSignal::new(false),
            items_loading: RwSignal::new(false),
            command_pending: RwSignal::new(false),
            items: RwSignal::new(Vec::new()),
            selected: RwSignal::new(None),
            editor: RwSignal::new(None),
            confirm_retire: RwSignal::new(false),
            retry: RwSignal::new(None),
            error: RwSignal::new(None),
            notice: RwSignal::new(None),
            on_unauthorized,
        }
    }
}

impl MappingDraft {
    fn new() -> Self {
        Self {
            owner_id: RwSignal::new(String::new()),
            source_key: RwSignal::new(String::new()),
            external_item_key: RwSignal::new(String::new()),
            external_uom: RwSignal::new(String::new()),
            item_id: RwSignal::new(String::new()),
            item_search: RwSignal::new(String::new()),
        }
    }
}

#[component]
pub(super) fn IntegrationItemMappingsWorkspace(on_unauthorized: Callback<()>) -> impl IntoView {
    let signals = MappingSignals::new(on_unauthorized);
    let draft = MappingDraft::new();
    load_access(signals);
    load_first_page(signals);

    let apply_filters = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        signals.selected.set(None);
        signals.editor.set(None);
        load_first_page(signals);
    };
    let refresh = move |_| load_first_page(signals);
    let open_create = move |_| open_create_editor(signals, draft);
    let search_items = move |_| {
        let Ok(owner_id) = draft.owner_id.get_untracked().parse::<i64>() else {
            signals
                .error
                .set(Some("Select a client before searching items.".into()));
            return;
        };
        load_items(signals, owner_id, draft.item_search.get_untracked());
    };
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        submit_configuration(signals, draft);
    };

    view! {
        <section class="integration-mapping-workspace">
            <form class="integration-mapping-toolbar" on:submit=apply_filters>
                <label>
                    <span>"Client"</span>
                    <select
                        prop:value=move || optional_id_value(signals.owner_filter.get())
                        on:change=move |event| signals.owner_filter.set(parse_optional_id(&event_target_value(&event)))
                    >
                        <option value="">"All clients"</option>
                        {move || owner_options(&signals.access.get())}
                    </select>
                </label>
                <label class="mapping-source-filter">
                    <span>"Source"</span>
                    <input
                        type="text"
                        maxlength="200"
                        placeholder="Exact source key"
                        prop:value=move || signals.source_filter.get()
                        on:input=move |event| signals.source_filter.set(event_target_value(&event))
                    />
                </label>
                <label>
                    <span>"Status"</span>
                    <select
                        prop:value=move || status_wire(signals.status_filter.get())
                        on:change=move |event| signals.status_filter.set(parse_status(&event_target_value(&event)))
                    >
                        <option value="active">"Active mappings"</option>
                        <option value="retired">"Retired history"</option>
                    </select>
                </label>
                <button type="submit" class="button secondary-action compact">"Apply"</button>
                <span class="mapping-toolbar-spacer"></span>
                <button
                    type="button"
                    class="icon-button"
                    title="Refresh item mappings"
                    aria-label="Refresh item mappings"
                    disabled=move || signals.loading.get()
                    on:click=refresh
                ><Icon icon=UiIcon::Refresh/></button>
                <button
                    type="button"
                    class="button primary-action compact"
                    disabled=move || signals.access_loading.get() || signals.command_pending.get()
                    on:click=open_create
                ><Icon icon=UiIcon::Add/>"New mapping"</button>
            </form>

            {move || signals.error.get().map(|message| view! {
                <div class="integration-error" role="alert">{message}</div>
            })}
            {move || signals.notice.get().map(|message| view! {
                <div class="integration-notice" role="status">{message}</div>
            })}

            <div class="integration-mapping-body">
                <section class="integration-mapping-list">
                    <div class="integration-table-scroll">
                        <table class="data-table integration-table integration-mapping-table">
                            <caption class="sr-only">"External order item mappings"</caption>
                            <thead><tr>
                                <th scope="col">"External identity"</th>
                                <th scope="col">"Source"</th>
                                <th scope="col">"Client"</th>
                                <th scope="col">"Wareboxes item"</th>
                                <th scope="col">"UOM"</th>
                                <th scope="col" class="numeric">"Revision"</th>
                                <th scope="col">"Status"</th>
                                <th scope="col"><span class="sr-only">"Open detail"</span></th>
                            </tr></thead>
                            <tbody>{move || mapping_rows(signals)}</tbody>
                        </table>
                    </div>
                    <footer class="integration-page-footer">
                        <span>{move || if signals.loading.get() { "Loading mappings".to_owned() } else { format!("{} mappings", signals.page.get().items.len()) }}</span>
                        <div>
                            <button type="button" class="button quiet-action compact" disabled=move || signals.loading.get() || signals.history.get().is_empty() on:click=move |_| previous_page(signals)>"Previous"</button>
                            <button type="button" class="button quiet-action compact" disabled=move || signals.loading.get() || signals.page.get().next_cursor.is_none() on:click=move |_| next_page(signals)>"Next"</button>
                        </div>
                    </footer>
                </section>

                <aside class="integration-mapping-detail" aria-label="Item mapping details">
                    {move || mapping_detail(signals, draft, submit, search_items)}
                </aside>
            </div>
        </section>
    }
}

fn mapping_rows(signals: MappingSignals) -> AnyView {
    let page = signals.page.get();
    if page.items.is_empty() {
        return view! {
            <tr><td class="table-empty-row" colspan="8">{if signals.loading.get() { "Loading mappings..." } else { "No item mappings match this view." }}</td></tr>
        }
        .into_any();
    }

    page.items
        .into_iter()
        .map(|mapping| {
            let mapping_id = mapping.mapping_id;
            let selected = signals
                .selected
                .get()
                .is_some_and(|value| value.mapping_id == mapping_id);
            let row = StoredValue::new(mapping.clone());
            view! {
                <tr class:selected=selected>
                    <td><strong class="mono">{mapping.external_item_key}</strong><small>{mapping.external_uom}</small></td>
                    <td class="mono">{mapping.source_key}</td>
                    <td>{mapping.inventory_owner_name}</td>
                    <td><strong>{mapping.item_description}</strong><small>{format!("Item #{}", mapping.item_id)}</small></td>
                    <td>{mapping.requested_uom}</td>
                    <td class="numeric">{mapping.revision.get()}</td>
                    <td><span class=mapping_status_class(mapping.status)>{mapping_status_label(mapping.status)}</span></td>
                    <td><button
                        type="button"
                        class="icon-button"
                        title="Open mapping detail"
                        aria-label=format!("Open item mapping {mapping_id}")
                        aria-pressed=selected
                        on:click=move |_| select_mapping(signals, row.get_value())
                    ><Icon icon=UiIcon::Search/></button></td>
                </tr>
            }
        })
        .collect_view()
        .into_any()
}

fn mapping_detail(
    signals: MappingSignals,
    draft: MappingDraft,
    submit: impl Fn(leptos::ev::SubmitEvent) + Copy + 'static,
    search_items: impl Fn(leptos::ev::MouseEvent) + Copy + 'static,
) -> AnyView {
    if let Some(mode) = signals.editor.get() {
        return mapping_editor(signals, draft, mode, submit, search_items);
    }
    let Some(mapping) = signals.selected.get() else {
        return view! {
            <div class="integration-empty"><h2>"Item mapping detail"</h2><p>"Select a mapping to review its external identity and target item."</p></div>
        }
        .into_any();
    };

    let action_mapping = StoredValue::new(mapping.clone());
    let retire_mapping = StoredValue::new(mapping.clone());
    view! {
        <div class="integration-detail-content mapping-detail-content">
            <header>
                <div><h2>{mapping.external_item_key.clone()}</h2><small>{format!("{} / {}", mapping.source_key, mapping.external_uom)}</small></div>
                <span class=mapping_status_class(mapping.status)>{mapping_status_label(mapping.status)}</span>
            </header>
            <dl class="integration-facts mapping-facts">
                <div><dt>"Client"</dt><dd>{mapping.inventory_owner_name}</dd></div>
                <div><dt>"Revision"</dt><dd>{mapping.revision.get()}</dd></div>
                <div class="wide"><dt>"Wareboxes item"</dt><dd>{format!("{} (#{}), {}", mapping.item_description, mapping.item_id, mapping.requested_uom)}</dd></div>
                <div><dt>"Configured"</dt><dd>{compact_time(&mapping.configured_at)}</dd></div>
                <div><dt>"Configured by"</dt><dd>{format!("User #{}", mapping.configured_by)}</dd></div>
                {mapping.retired_at.map(|retired_at| view! { <div><dt>"Retired"</dt><dd>{compact_time(&retired_at)}</dd></div> })}
                {mapping.retired_by.map(|retired_by| view! { <div><dt>"Retired by"</dt><dd>{format!("User #{retired_by}")}</dd></div> })}
            </dl>
            <div class="integration-command-band mapping-actions">
                <Show when=move || signals.retry.get().is_some()>
                    <button type="button" class="button secondary-action compact" disabled=move || signals.command_pending.get() on:click=move |_| retry_command(signals)>"Retry exact command"</button>
                </Show>
                <button type="button" class="button secondary-action compact" disabled=move || signals.command_pending.get() on:click=move |_| open_reconfigure_editor(signals, draft, action_mapping.get_value())>
                    {if mapping.status == IntegrationOrderItemMappingStatus::Active { "Reconfigure" } else { "Re-enable" }}
                </button>
                <Show when=move || mapping.status == IntegrationOrderItemMappingStatus::Active>
                    <Show
                        when=move || signals.confirm_retire.get()
                        fallback=move || view! { <button type="button" class="button danger-action compact" disabled=move || signals.command_pending.get() on:click=move |_| signals.confirm_retire.set(true)>"Retire"</button> }
                    >
                        <span class="destructive-confirm"><span>"Retire this mapping?"</span><button type="button" class="button quiet-action compact" on:click=move |_| signals.confirm_retire.set(false)>"Keep"</button><button type="button" class="button danger-action compact" disabled=move || signals.command_pending.get() on:click=move |_| retire_selected(signals, retire_mapping.get_value())>"Confirm"</button></span>
                    </Show>
                </Show>
            </div>
        </div>
    }
    .into_any()
}

fn mapping_editor(
    signals: MappingSignals,
    draft: MappingDraft,
    mode: EditorMode,
    submit: impl Fn(leptos::ev::SubmitEvent) + Copy + 'static,
    search_items: impl Fn(leptos::ev::MouseEvent) + Copy + 'static,
) -> AnyView {
    let identity_locked = mode == EditorMode::Reconfigure;
    view! {
        <form class="mapping-editor" on:submit=submit>
            <header>
                <div><p class="eyebrow">"Partner order identity"</p><h2>{if identity_locked { "Reconfigure item mapping" } else { "New item mapping" }}</h2></div>
                <button type="button" class="icon-button" aria-label="Close mapping editor" disabled=move || signals.command_pending.get() on:click=move |_| signals.editor.set(None)><Icon icon=UiIcon::Close/></button>
            </header>
            <fieldset disabled=move || signals.command_pending.get()>
                <label><span>"Client"</span><select required disabled=identity_locked prop:value=move || draft.owner_id.get() on:change=move |event| { draft.owner_id.set(event_target_value(&event)); draft.item_id.set(String::new()); signals.items.set(Vec::new()); }><option value="">"Select client"</option>{move || owner_options(&signals.access.get())}</select></label>
                <label><span>"Source key"</span><input required disabled=identity_locked type="text" maxlength="200" placeholder="acme-edi" prop:value=move || draft.source_key.get() on:input=move |event| draft.source_key.set(event_target_value(&event))/></label>
                <label><span>"External item key"</span><input required disabled=identity_locked type="text" maxlength="200" placeholder="CLIENT-SKU-001" prop:value=move || draft.external_item_key.get() on:input=move |event| draft.external_item_key.set(event_target_value(&event))/></label>
                <label><span>"External UOM"</span><input required disabled=identity_locked type="text" maxlength="32" placeholder="CS" prop:value=move || draft.external_uom.get() on:input=move |event| draft.external_uom.set(event_target_value(&event))/></label>
            </fieldset>
            <div class="mapping-item-search">
                <label><span>"Find Wareboxes item"</span><input type="search" maxlength="200" placeholder="Description or item ID" prop:value=move || draft.item_search.get() on:input=move |event| draft.item_search.set(event_target_value(&event))/></label>
                <button type="button" class="button secondary-action compact" disabled=move || signals.items_loading.get() || signals.command_pending.get() on:click=search_items>{move || if signals.items_loading.get() { "Searching" } else { "Search" }}</button>
            </div>
            <label class="mapping-target-select"><span>"Target item and UOM"</span><select required prop:value=move || draft.item_id.get() on:change=move |event| draft.item_id.set(event_target_value(&event))><option value="">"Select target item"</option>{move || item_options(signals.items.get(), signals.selected.get())}</select></label>
            <Show when=move || signals.retry.get().is_some()><p class="mapping-retry-note">"Retry sends the exact saved request and idempotency key."</p></Show>
            <footer><button type="button" class="button quiet-action" disabled=move || signals.command_pending.get() on:click=move |_| signals.editor.set(None)>"Cancel"</button><button type="submit" class="button primary-action" disabled=move || signals.command_pending.get()>{move || if signals.command_pending.get() { "Saving..." } else if signals.retry.get().is_some() { "Retry save" } else { "Save mapping" }}</button></footer>
        </form>
    }
    .into_any()
}

fn open_create_editor(signals: MappingSignals, draft: MappingDraft) {
    let owner_id = signals.owner_filter.get_untracked().or_else(|| {
        signals
            .access
            .get_untracked()
            .inventory_owners
            .first()
            .map(|owner| owner.id)
    });
    draft
        .owner_id
        .set(owner_id.map_or_else(String::new, |id| id.to_string()));
    draft.source_key.set(signals.source_filter.get_untracked());
    draft.external_item_key.set(String::new());
    draft.external_uom.set(String::new());
    draft.item_id.set(String::new());
    draft.item_search.set(String::new());
    signals.items.set(Vec::new());
    signals.selected.set(None);
    signals.retry.set(None);
    signals.error.set(None);
    signals.notice.set(None);
    signals.confirm_retire.set(false);
    signals.editor.set(Some(EditorMode::Create));
    if let Some(owner_id) = owner_id {
        load_items(signals, owner_id, String::new());
    }
}

fn open_reconfigure_editor(
    signals: MappingSignals,
    draft: MappingDraft,
    mapping: IntegrationOrderItemMappingResponse,
) {
    draft.owner_id.set(mapping.inventory_owner_id.to_string());
    draft.source_key.set(mapping.source_key.clone());
    draft
        .external_item_key
        .set(mapping.external_item_key.clone());
    draft.external_uom.set(mapping.external_uom.clone());
    draft.item_id.set(mapping.item_id.to_string());
    draft.item_search.set(String::new());
    signals.retry.set(None);
    signals.error.set(None);
    signals.notice.set(None);
    signals.confirm_retire.set(false);
    signals.editor.set(Some(EditorMode::Reconfigure));
    load_items(signals, mapping.inventory_owner_id, String::new());
}

fn select_mapping(signals: MappingSignals, mapping: IntegrationOrderItemMappingResponse) {
    signals.selected.set(Some(mapping));
    signals.editor.set(None);
    signals.retry.set(None);
    signals.error.set(None);
    signals.notice.set(None);
    signals.confirm_retire.set(false);
}

fn submit_configuration(signals: MappingSignals, draft: MappingDraft) {
    if let Some(command) = signals.retry.get_untracked() {
        dispatch_command(signals, command);
        return;
    }
    let Ok(inventory_owner_id) = draft.owner_id.get_untracked().parse::<i64>() else {
        signals.error.set(Some("Select a client.".into()));
        return;
    };
    let source_key = draft.source_key.get_untracked().trim().to_owned();
    let external_item_key = draft.external_item_key.get_untracked().trim().to_owned();
    let external_uom = draft.external_uom.get_untracked().trim().to_owned();
    if source_key.is_empty() || external_item_key.is_empty() || external_uom.is_empty() {
        signals.error.set(Some(
            "Source, external item, and external UOM are required.".into(),
        ));
        return;
    }
    let Ok(item_id) = draft.item_id.get_untracked().parse::<i64>() else {
        signals
            .error
            .set(Some("Select a Wareboxes target item.".into()));
        return;
    };
    let selected = signals.selected.get_untracked();
    let requested_uom = signals
        .items
        .get_untracked()
        .into_iter()
        .find(|item| item.item_id == item_id)
        .map(|item| item.requested_uom)
        .or_else(|| {
            selected
                .as_ref()
                .filter(|mapping| mapping.item_id == item_id)
                .map(|mapping| mapping.requested_uom.clone())
        });
    let Some(requested_uom) = requested_uom else {
        signals.error.set(Some(
            "Search again and select a current client item.".into(),
        ));
        return;
    };
    let expected_revision = match (signals.editor.get_untracked(), selected.as_ref()) {
        (Some(EditorMode::Reconfigure), Some(mapping))
            if mapping.status == IntegrationOrderItemMappingStatus::Active =>
        {
            Some(mapping.revision)
        }
        _ => None,
    };
    dispatch_command(
        signals,
        PendingCommand::Configure {
            request: ConfigureIntegrationOrderItemMappingRequest {
                inventory_owner_id,
                source_key,
                external_item_key,
                external_uom,
                item_id,
                requested_uom,
                expected_revision,
            },
            key: api::new_idempotency_key(),
        },
    );
}

fn retire_selected(signals: MappingSignals, mapping: IntegrationOrderItemMappingResponse) {
    dispatch_command(
        signals,
        PendingCommand::Retire {
            mapping_id: mapping.mapping_id,
            request: RetireIntegrationOrderItemMappingRequest {
                expected_revision: mapping.revision,
            },
            key: api::new_idempotency_key(),
        },
    );
}

fn retry_command(signals: MappingSignals) {
    if let Some(command) = signals.retry.get_untracked() {
        dispatch_command(signals, command);
    }
}

fn dispatch_command(signals: MappingSignals, command: PendingCommand) {
    signals.command_pending.set(true);
    signals.error.set(None);
    signals.notice.set(None);
    signals.retry.set(Some(command.clone()));
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (signals, command);
    }
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        let result = match &command {
            PendingCommand::Configure { request, key } => {
                api::configure_integration_order_item_mapping(request, key).await
            }
            PendingCommand::Retire {
                mapping_id,
                request,
                key,
            } => api::retire_integration_order_item_mapping(*mapping_id, request, key).await,
        };
        signals.command_pending.set(false);
        match result {
            Ok(mapping) => {
                let retired = matches!(command, PendingCommand::Retire { .. });
                signals.retry.set(None);
                signals.editor.set(None);
                signals.confirm_retire.set(false);
                signals.selected.set(Some(mapping));
                signals.status_filter.set(if retired {
                    IntegrationOrderItemMappingStatus::Retired
                } else {
                    IntegrationOrderItemMappingStatus::Active
                });
                signals.notice.set(Some(if retired {
                    "Item mapping retired.".into()
                } else {
                    "Item mapping saved.".into()
                }));
                load_first_page(signals);
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => {
                if !error.ambiguous_outcome {
                    signals.retry.set(None);
                    load_first_page(signals);
                }
                signals.error.set(Some(if error.ambiguous_outcome {
                    format!("{} Retry sends the exact saved command.", error.message)
                } else {
                    error.message
                }));
            }
        }
    });
}

fn load_first_page(signals: MappingSignals) {
    signals.cursor.set(None);
    signals.history.set(Vec::new());
    load_page(signals, None, Vec::new());
}

fn next_page(signals: MappingSignals) {
    let Some(cursor) = signals.page.get_untracked().next_cursor else {
        return;
    };
    let mut history = signals.history.get_untracked();
    history.push(signals.cursor.get_untracked());
    load_page(signals, Some(cursor), history);
}

fn previous_page(signals: MappingSignals) {
    let mut history = signals.history.get_untracked();
    let Some(cursor) = history.pop() else {
        return;
    };
    load_page(signals, cursor, history);
}

#[cfg(target_arch = "wasm32")]
fn load_page(
    signals: MappingSignals,
    cursor: Option<OpaqueCursor>,
    history: Vec<Option<OpaqueCursor>>,
) {
    let generation = signals.page_generation.get_untracked().wrapping_add(1);
    signals.page_generation.set(generation);
    signals.loading.set(true);
    let filters = IntegrationMappingFilters {
        inventory_owner_id: signals.owner_filter.get_untracked(),
        source_key: text_filter(&signals.source_filter.get_untracked()),
        item_id: None,
        status: Some(signals.status_filter.get_untracked()),
    };
    leptos::task::spawn_local(async move {
        let result = api::integration_order_item_mappings(&filters, cursor.as_ref()).await;
        if signals.page_generation.get_untracked() != generation {
            return;
        }
        signals.loading.set(false);
        match result {
            Ok(page) => {
                let selected_id = signals
                    .selected
                    .get_untracked()
                    .map(|value| value.mapping_id);
                if let Some(selected_id) = selected_id {
                    signals.selected.set(
                        page.items
                            .iter()
                            .find(|value| value.mapping_id == selected_id)
                            .cloned(),
                    );
                }
                signals.page.set(page);
                signals.cursor.set(cursor);
                signals.history.set(history);
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.error.set(Some(error.message)),
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn load_page(
    _signals: MappingSignals,
    _cursor: Option<OpaqueCursor>,
    _history: Vec<Option<OpaqueCursor>>,
) {
}

#[cfg(target_arch = "wasm32")]
fn load_access(signals: MappingSignals) {
    let generation = signals.access_generation.get_untracked().wrapping_add(1);
    signals.access_generation.set(generation);
    signals.access_loading.set(true);
    leptos::task::spawn_local(async move {
        let result = api::access().await;
        if signals.access_generation.get_untracked() != generation {
            return;
        }
        signals.access_loading.set(false);
        match result {
            Ok(access) => signals.access.set(access),
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.error.set(Some(error.message)),
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn load_access(_signals: MappingSignals) {}

#[cfg(target_arch = "wasm32")]
fn load_items(signals: MappingSignals, owner_id: i64, search: String) {
    let generation = signals.item_generation.get_untracked().wrapping_add(1);
    signals.item_generation.set(generation);
    signals.items_loading.set(true);
    leptos::task::spawn_local(async move {
        let result = api::order_entry_items(owner_id, &search).await;
        if signals.item_generation.get_untracked() != generation {
            return;
        }
        signals.items_loading.set(false);
        match result {
            Ok(items) => signals.items.set(items),
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.error.set(Some(error.message)),
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn load_items(_signals: MappingSignals, _owner_id: i64, _search: String) {}

fn owner_options(access: &AccessScopeWorkspace) -> AnyView {
    access
        .inventory_owners
        .iter()
        .map(|owner| view! { <option value=owner.id.to_string()>{owner.name.clone()}</option> })
        .collect_view()
        .into_any()
}

fn item_options(
    items: Vec<OrderEntryItemResponse>,
    current: Option<IntegrationOrderItemMappingResponse>,
) -> AnyView {
    let current_missing = current
        .as_ref()
        .filter(|mapping| !items.iter().any(|item| item.item_id == mapping.item_id));
    let retained = current_missing.map(|mapping| {
        view! { <option value=mapping.item_id.to_string()>{format!("{} / {} (#{} current)", mapping.item_description, mapping.requested_uom, mapping.item_id)}</option> }
    });
    view! {
        {retained}
        {items.into_iter().map(|item| {
            let description = item.description.unwrap_or_else(|| format!("Item #{}", item.item_id));
            view! { <option value=item.item_id.to_string()>{format!("{description} / {} (#{} )", item.requested_uom, item.item_id)}</option> }
        }).collect_view()}
    }
    .into_any()
}

fn parse_optional_id(value: &str) -> Option<i64> {
    value.parse::<i64>().ok().filter(|id| *id > 0)
}

fn optional_id_value(value: Option<i64>) -> String {
    value.map_or_else(String::new, |id| id.to_string())
}

#[cfg(any(target_arch = "wasm32", test))]
fn text_filter(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn parse_status(value: &str) -> IntegrationOrderItemMappingStatus {
    if value == "retired" {
        IntegrationOrderItemMappingStatus::Retired
    } else {
        IntegrationOrderItemMappingStatus::Active
    }
}

fn status_wire(value: IntegrationOrderItemMappingStatus) -> &'static str {
    match value {
        IntegrationOrderItemMappingStatus::Active => "active",
        IntegrationOrderItemMappingStatus::Retired => "retired",
    }
}

fn mapping_status_label(value: IntegrationOrderItemMappingStatus) -> &'static str {
    match value {
        IntegrationOrderItemMappingStatus::Active => "Active",
        IntegrationOrderItemMappingStatus::Retired => "Retired",
    }
}

fn mapping_status_class(value: IntegrationOrderItemMappingStatus) -> &'static str {
    match value {
        IntegrationOrderItemMappingStatus::Active => "status shipped",
        IntegrationOrderItemMappingStatus::Retired => "status muted",
    }
}

fn compact_time(value: &str) -> String {
    value.split_once('T').map_or_else(
        || value.to_owned(),
        |(date, time)| format!("{date} {}", &time[..time.len().min(8)]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_and_status_labels_are_exact() {
        assert_eq!(parse_optional_id("17"), Some(17));
        assert_eq!(parse_optional_id("0"), None);
        assert_eq!(text_filter("  partner-api  "), Some("partner-api".into()));
        assert_eq!(text_filter("   "), None);
        assert_eq!(
            parse_status("retired"),
            IntegrationOrderItemMappingStatus::Retired
        );
        assert_eq!(
            mapping_status_label(IntegrationOrderItemMappingStatus::Active),
            "Active"
        );
    }

    #[test]
    fn timestamps_are_dense() {
        assert_eq!(
            compact_time("2026-08-10T11:22:33+00:00"),
            "2026-08-10 11:22:33"
        );
    }
}
