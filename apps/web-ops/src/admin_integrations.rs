#[path = "admin_integrations/correction.rs"]
mod correction;
#[path = "admin_integrations/mappings.rs"]
mod mappings;
#[path = "admin_integrations/owner_mappings.rs"]
mod owner_mappings;

use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    DiscardOutboxDeadLetterRequest, InboundIntegrationDetailResponse, InboundIntegrationPage,
    InboundIntegrationSort, InboundPayloadPreviewEncoding, IntegrationOrderProcessingStatus,
    IntegrationSortDirection, OpaqueCursor, OutboundDeliveryAttemptOutcome, OutboundDeliveryStatus,
    OutboundIntegrationDetailResponse, OutboundIntegrationPage, OutboundIntegrationSort,
    ReplayOutboxDeadLetterRequest, ReprocessIntegrationOrderRequest,
};

use crate::api::{self, InboundIntegrationFilters, OutboundIntegrationFilters};
use crate::components::{Icon, SearchField, UiIcon};
use crate::sorting::{SortDirection, SortableHeader};
use crate::workspace_layout::{PaneControls, SplitPaneHandle, SplitPaneState};
use correction::CorrectionPanel;
use mappings::IntegrationItemMappingsWorkspace;
use owner_mappings::IntegrationOwnerMappingsWorkspace;

#[derive(Clone, Copy, PartialEq, Eq)]
enum MonitorTab {
    Inbound,
    Outbound,
    Mappings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IntegrationDirection {
    Inbound,
    Outbound,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MappingTab {
    Owners,
    Items,
}

#[derive(Clone, PartialEq, Eq)]
struct DeadLetterTarget {
    event_id: i64,
    event_type: String,
    event_key: String,
    expected_replay_count: i32,
    previous_attempts: i32,
}

#[derive(Clone, PartialEq, Eq)]
struct SavedReplayCommand {
    event_id: i64,
    request: ReplayOutboxDeadLetterRequest,
    idempotency_key: String,
}

#[derive(Clone, PartialEq, Eq)]
struct SavedDiscardCommand {
    event_id: i64,
    request: DiscardOutboxDeadLetterRequest,
    idempotency_key: String,
}

#[derive(Clone, PartialEq, Eq)]
struct SavedReprocessCommand {
    receipt_id: i64,
    request: ReprocessIntegrationOrderRequest,
    idempotency_key: String,
}

#[derive(Clone, Copy)]
struct MonitorSignals {
    tab: RwSignal<MonitorTab>,
    search: RwSignal<String>,
    source_key: RwSignal<String>,
    event_type: RwSignal<String>,
    outbound_status: RwSignal<Option<OutboundDeliveryStatus>>,
    inbound_sort: RwSignal<InboundIntegrationSort>,
    inbound_direction: RwSignal<IntegrationSortDirection>,
    outbound_sort: RwSignal<OutboundIntegrationSort>,
    outbound_direction: RwSignal<IntegrationSortDirection>,
    inbound_page: RwSignal<InboundIntegrationPage>,
    outbound_page: RwSignal<OutboundIntegrationPage>,
    inbound_cursor: RwSignal<Option<OpaqueCursor>>,
    outbound_cursor: RwSignal<Option<OpaqueCursor>>,
    inbound_history: RwSignal<Vec<Option<OpaqueCursor>>>,
    outbound_history: RwSignal<Vec<Option<OpaqueCursor>>>,
    inbound_generation: RwSignal<u64>,
    outbound_generation: RwSignal<u64>,
    inbound_detail_generation: RwSignal<u64>,
    outbound_detail_generation: RwSignal<u64>,
    inbound_loading: RwSignal<bool>,
    outbound_loading: RwSignal<bool>,
    inbound_detail_loading: RwSignal<bool>,
    outbound_detail_loading: RwSignal<bool>,
    command_pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    notice: RwSignal<Option<String>>,
    selected_inbound_id: RwSignal<Option<i64>>,
    inbound_detail: RwSignal<Option<InboundIntegrationDetailResponse>>,
    reprocess_confirmation: RwSignal<Option<i64>>,
    reprocess_retry: RwSignal<Option<SavedReprocessCommand>>,
    selected_outbound_id: RwSignal<Option<i64>>,
    outbound_detail: RwSignal<Option<OutboundIntegrationDetailResponse>>,
    replay_confirmation: RwSignal<Option<DeadLetterTarget>>,
    replay_retry: RwSignal<Option<SavedReplayCommand>>,
    discard_confirmation: RwSignal<Option<DeadLetterTarget>>,
    discard_reason: RwSignal<String>,
    discard_retry: RwSignal<Option<SavedDiscardCommand>>,
    on_unauthorized: Callback<()>,
}

impl MonitorSignals {
    fn new(on_unauthorized: Callback<()>) -> Self {
        Self {
            tab: RwSignal::new(MonitorTab::Inbound),
            search: RwSignal::new(String::new()),
            source_key: RwSignal::new(String::new()),
            event_type: RwSignal::new(String::new()),
            outbound_status: RwSignal::new(None),
            inbound_sort: RwSignal::new(InboundIntegrationSort::ReceivedAt),
            inbound_direction: RwSignal::new(IntegrationSortDirection::Descending),
            outbound_sort: RwSignal::new(OutboundIntegrationSort::CreatedAt),
            outbound_direction: RwSignal::new(IntegrationSortDirection::Descending),
            inbound_page: RwSignal::new(InboundIntegrationPage::new(Vec::new(), None)),
            outbound_page: RwSignal::new(OutboundIntegrationPage::new(Vec::new(), None)),
            inbound_cursor: RwSignal::new(None),
            outbound_cursor: RwSignal::new(None),
            inbound_history: RwSignal::new(Vec::new()),
            outbound_history: RwSignal::new(Vec::new()),
            inbound_generation: RwSignal::new(0),
            outbound_generation: RwSignal::new(0),
            inbound_detail_generation: RwSignal::new(0),
            outbound_detail_generation: RwSignal::new(0),
            inbound_loading: RwSignal::new(false),
            outbound_loading: RwSignal::new(false),
            inbound_detail_loading: RwSignal::new(false),
            outbound_detail_loading: RwSignal::new(false),
            command_pending: RwSignal::new(false),
            error: RwSignal::new(None),
            notice: RwSignal::new(None),
            selected_inbound_id: RwSignal::new(None),
            inbound_detail: RwSignal::new(None),
            reprocess_confirmation: RwSignal::new(None),
            reprocess_retry: RwSignal::new(None),
            selected_outbound_id: RwSignal::new(None),
            outbound_detail: RwSignal::new(None),
            replay_confirmation: RwSignal::new(None),
            replay_retry: RwSignal::new(None),
            discard_confirmation: RwSignal::new(None),
            discard_reason: RwSignal::new(String::new()),
            discard_retry: RwSignal::new(None),
            on_unauthorized,
        }
    }
}

#[component]
pub fn IntegrationsWorkbench(on_unauthorized: Callback<()>) -> impl IntoView {
    let signals = MonitorSignals::new(on_unauthorized);
    let layout = SplitPaneState::new("integration-monitor", 760);

    Effect::new(move |_| {
        request_inbound(signals, None, Vec::new());
        request_outbound(signals, None, Vec::new());
    });

    let apply = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        refresh_current(signals);
    };
    let refresh = move |_| refresh_current(signals);
    let select_inbound = Callback::new(move |receipt_id: i64| {
        select_inbound_receipt(signals, receipt_id);
        layout.show_detail();
    });
    let select_outbound = Callback::new(move |event_id: i64| {
        select_outbound_event(signals, event_id);
        layout.show_detail();
    });

    view! {
        <section class="integration-monitor">
            <header class="integration-toolbar">
                <div class="segmented-control" role="tablist" aria-label="Integration directions">
                    <button
                        id="integration-inbound-tab"
                        type="button"
                        role="tab"
                        aria-controls="integration-inbound-panel"
                        aria-selected=move || (signals.tab.get() == MonitorTab::Inbound).to_string()
                        class:active=move || signals.tab.get() == MonitorTab::Inbound
                        on:click=move |_| signals.tab.set(MonitorTab::Inbound)
                    >"Inbound"</button>
                    <button
                        id="integration-outbound-tab"
                        type="button"
                        role="tab"
                        aria-controls="integration-outbound-panel"
                        aria-selected=move || (signals.tab.get() == MonitorTab::Outbound).to_string()
                        class:active=move || signals.tab.get() == MonitorTab::Outbound
                        on:click=move |_| signals.tab.set(MonitorTab::Outbound)
                    >"Outbound"</button>
                    <button
                        id="integration-mappings-tab"
                        type="button"
                        role="tab"
                        aria-controls="integration-mappings-panel"
                        aria-selected=move || (signals.tab.get() == MonitorTab::Mappings).to_string()
                        class:active=move || signals.tab.get() == MonitorTab::Mappings
                        on:click=move |_| signals.tab.set(MonitorTab::Mappings)
                    >"Mappings"</button>
                </div>
                <Show when=move || signals.tab.get() != MonitorTab::Mappings>
                <form class="integration-filters" on:submit=apply>
                    <SearchField
                        label="Search integrations".to_owned()
                        placeholder="Event, key, request"
                        value=signals.search
                    />
                    <Show
                        when=move || signals.tab.get() == MonitorTab::Inbound
                        fallback=move || view! {
                            <label>
                                <span>"Event type"</span>
                                <input
                                    type="text"
                                    maxlength="200"
                                    placeholder="Exact event type"
                                    prop:value=move || signals.event_type.get()
                                    on:input=move |event| signals.event_type.set(event_target_value(&event))
                                />
                            </label>
                            <label>
                                <span>"Status"</span>
                                <select
                                    prop:value=move || status_value(signals.outbound_status.get())
                                    on:change=move |event| signals.outbound_status.set(parse_status(&event_target_value(&event)))
                                >
                                    <option value="">"All statuses"</option>
                                    <option value="pending">"Pending"</option>
                                    <option value="claimed">"Claimed"</option>
                                    <option value="retry_scheduled">"Retry scheduled"</option>
                                    <option value="dead_lettered">"Dead lettered"</option>
                                    <option value="published">"Published"</option>
                                    <option value="discarded">"Discarded"</option>
                                </select>
                            </label>
                        }
                    >
                        <label>
                            <span>"Source"</span>
                            <input
                                type="text"
                                maxlength="200"
                                placeholder="Exact source key"
                                prop:value=move || signals.source_key.get()
                                on:input=move |event| signals.source_key.set(event_target_value(&event))
                            />
                        </label>
                    </Show>
                    <button type="submit" class="button secondary-action compact">"Apply"</button>
                </form>
                <PaneControls layout master_label="event list" detail_label="event detail"/>
                <button
                    type="button"
                    class="icon-button"
                    title="Refresh integrations"
                    aria-label="Refresh integrations"
                    disabled=move || active_loading(signals)
                    on:click=refresh
                ><Icon icon=UiIcon::Refresh/></button>
                </Show>
            </header>

            <Show when=move || signals.tab.get() != MonitorTab::Mappings>
                {move || signals.error.get().map(|message| view! {
                    <div class="integration-error" role="alert">{message}</div>
                })}
                {move || signals.notice.get().map(|message| view! {
                    <div class="integration-notice" role="status">{message}</div>
                })}
            </Show>
            <IntegrationDirectionPanel
                signals
                layout
                direction=IntegrationDirection::Inbound
                select_inbound
                select_outbound
            />
            <IntegrationDirectionPanel
                signals
                layout
                direction=IntegrationDirection::Outbound
                select_inbound
                select_outbound
            />
            <IntegrationMappingsWorkspace on_unauthorized monitor_tab=signals.tab/>
        </section>
    }
}

#[component]
fn IntegrationDirectionPanel(
    signals: MonitorSignals,
    layout: SplitPaneState,
    direction: IntegrationDirection,
    select_inbound: Callback<i64>,
    select_outbound: Callback<i64>,
) -> impl IntoView {
    let (monitor_tab, panel_id, tab_id) = match direction {
        IntegrationDirection::Inbound => (
            MonitorTab::Inbound,
            "integration-inbound-panel",
            "integration-inbound-tab",
        ),
        IntegrationDirection::Outbound => (
            MonitorTab::Outbound,
            "integration-outbound-panel",
            "integration-outbound-tab",
        ),
    };

    view! {
        <div
            id=panel_id
            class="integration-body split-workspace"
            role="tabpanel"
            aria-labelledby=tab_id
            hidden=move || signals.tab.get() != monitor_tab
            style=move || layout.style()
            data-pane-mode=move || layout.mode_attribute()
        >
            <Show when=move || signals.tab.get() == monitor_tab>
                <section class="integration-list split-master">
                    <Show
                        when=move || direction == IntegrationDirection::Inbound
                        fallback=move || view! { <OutboundTable signals select=select_outbound/> }
                    >
                        <InboundTable signals select=select_inbound/>
                    </Show>
                </section>
                <SplitPaneHandle layout/>
                <aside class="integration-detail split-detail">
                    <Show
                        when=move || direction == IntegrationDirection::Inbound
                        fallback=move || view! { <OutboundDetail signals/> }
                    >
                        <InboundDetail signals/>
                    </Show>
                </aside>
            </Show>
        </div>
    }
}

#[component]
fn IntegrationMappingsWorkspace(
    on_unauthorized: Callback<()>,
    monitor_tab: RwSignal<MonitorTab>,
) -> impl IntoView {
    let tab = RwSignal::new(MappingTab::Owners);
    view! {
        <section
            id="integration-mappings-panel"
            class="integration-mappings-shell"
            role="tabpanel"
            aria-labelledby="integration-mappings-tab"
            hidden=move || monitor_tab.get() != MonitorTab::Mappings
        >
            <nav class="segmented-control integration-mapping-tabs" role="tablist" aria-label="Order integration mappings">
                <button id="integration-owner-mappings-tab" type="button" role="tab" aria-controls="integration-owner-mappings-panel" aria-selected=move || (tab.get() == MappingTab::Owners).to_string() class:active=move || tab.get() == MappingTab::Owners on:click=move |_| tab.set(MappingTab::Owners)>
                    "Owner identities"
                </button>
                <button id="integration-item-mappings-tab" type="button" role="tab" aria-controls="integration-item-mappings-panel" aria-selected=move || (tab.get() == MappingTab::Items).to_string() class:active=move || tab.get() == MappingTab::Items on:click=move |_| tab.set(MappingTab::Items)>
                    "Item identities"
                </button>
            </nav>
            <div
                id="integration-owner-mappings-panel"
                role="tabpanel"
                aria-labelledby="integration-owner-mappings-tab"
                hidden=move || monitor_tab.get() != MonitorTab::Mappings || tab.get() != MappingTab::Owners
            >
                <Show when=move || monitor_tab.get() == MonitorTab::Mappings && tab.get() == MappingTab::Owners>
                    <IntegrationOwnerMappingsWorkspace on_unauthorized/>
                </Show>
            </div>
            <div
                id="integration-item-mappings-panel"
                role="tabpanel"
                aria-labelledby="integration-item-mappings-tab"
                hidden=move || monitor_tab.get() != MonitorTab::Mappings || tab.get() != MappingTab::Items
            >
                <Show when=move || monitor_tab.get() == MonitorTab::Mappings && tab.get() == MappingTab::Items>
                    <IntegrationItemMappingsWorkspace on_unauthorized/>
                </Show>
            </div>
        </section>
    }
}

#[component]
fn InboundTable(signals: MonitorSignals, select: Callback<i64>) -> impl IntoView {
    view! {
        <div class="integration-table-scroll">
            <table class="data-table integration-table">
                <caption class="sr-only">"Inbound integration receipts"</caption>
                <thead><tr>
                    <SortableHeader label="Received" active=move || signals.inbound_sort.get()==InboundIntegrationSort::ReceivedAt direction=move || display_direction(signals.inbound_direction.get()) on_sort=Callback::new(move |_| change_inbound_sort(signals,InboundIntegrationSort::ReceivedAt))/>
                    <SortableHeader label="Source" active=move || signals.inbound_sort.get()==InboundIntegrationSort::Source direction=move || display_direction(signals.inbound_direction.get()) on_sort=Callback::new(move |_| change_inbound_sort(signals,InboundIntegrationSort::Source))/>
                    <th scope="col">"Deduplication key"</th>
                    <th scope="col">"Processing"</th>
                    <th scope="col">"Scope"</th>
                    <th scope="col">"Content type"</th>
                    <SortableHeader label="Bytes" active=move || signals.inbound_sort.get()==InboundIntegrationSort::PayloadSize direction=move || display_direction(signals.inbound_direction.get()) on_sort=Callback::new(move |_| change_inbound_sort(signals,InboundIntegrationSort::PayloadSize)) numeric=true/>
                    <th scope="col"><span class="sr-only">"Open detail"</span></th>
                </tr></thead>
                <tbody>{move || {
                    let page=signals.inbound_page.get();
                    if !signals.inbound_loading.get() && page.items.is_empty() {
                        view! { <tr><td class="table-empty-row" colspan="8">"No inbound receipts match these filters."</td></tr> }.into_any()
                    } else {
                        page.items.into_iter().map(|receipt| {
                            let receipt_id=receipt.id;
                            let selected=signals.selected_inbound_id.get()==Some(receipt_id);
                            view! { <tr class:selected=selected>
                                <td>{compact_time(&receipt.received_at)}</td>
                                <td><strong class="mono">{receipt.source_key}</strong><small>{receipt.request_id.unwrap_or_else(|| "No request ID".into())}</small></td>
                                <td class="mono truncate-cell">{receipt.deduplication_key}</td>
                                <td>{receipt.processing_status.map_or_else(|| view! { <span class="status muted">"Received"</span> }.into_any(),|status| view! { <span class=processing_status_class(status)>{processing_status_label(status)}</span> }.into_any())}</td>
                                <td>{scope_label(receipt.inventory_owner_name.as_deref(),receipt.facility_name.as_deref())}</td>
                                <td>{receipt.content_type}</td>
                                <td class="numeric">{format_bytes(receipt.payload_bytes)}</td>
                                <td><button type="button" class="icon-button" title="Open receipt detail" aria-label=format!("Open inbound receipt {receipt_id}") aria-pressed=selected on:click=move |_| select.run(receipt_id)><Icon icon=UiIcon::Search/></button></td>
                            </tr> }
                        }).collect_view().into_any()
                    }
                }}</tbody>
            </table>
        </div>
        <PageFooter
            label="receipts"
            count=Signal::derive(move || signals.inbound_page.get().items.len())
            loading=signals.inbound_loading
            has_previous=Signal::derive(move || !signals.inbound_history.get().is_empty())
            has_next=Signal::derive(move || signals.inbound_page.get().has_more())
            previous=Callback::new(move |_| previous_inbound(signals))
            next=Callback::new(move |_| next_inbound(signals))
        />
    }
}

#[component]
fn OutboundTable(signals: MonitorSignals, select: Callback<i64>) -> impl IntoView {
    view! {
        <div class="integration-table-scroll">
            <table class="data-table integration-table">
                <caption class="sr-only">"Outbound integration deliveries"</caption>
                <thead><tr>
                    <SortableHeader label="Created" active=move || signals.outbound_sort.get()==OutboundIntegrationSort::CreatedAt direction=move || display_direction(signals.outbound_direction.get()) on_sort=Callback::new(move |_| change_outbound_sort(signals,OutboundIntegrationSort::CreatedAt))/>
                    <SortableHeader label="Event" active=move || signals.outbound_sort.get()==OutboundIntegrationSort::EventType direction=move || display_direction(signals.outbound_direction.get()) on_sort=Callback::new(move |_| change_outbound_sort(signals,OutboundIntegrationSort::EventType))/>
                    <th scope="col">"Aggregate"</th>
                    <th scope="col">"Scope"</th>
                    <SortableHeader label="Status" active=move || signals.outbound_sort.get()==OutboundIntegrationSort::Status direction=move || display_direction(signals.outbound_direction.get()) on_sort=Callback::new(move |_| change_outbound_sort(signals,OutboundIntegrationSort::Status))/>
                    <SortableHeader label="Attempts" active=move || signals.outbound_sort.get()==OutboundIntegrationSort::Attempts direction=move || display_direction(signals.outbound_direction.get()) on_sort=Callback::new(move |_| change_outbound_sort(signals,OutboundIntegrationSort::Attempts)) numeric=true/>
                    <th scope="col"><span class="sr-only">"Open detail"</span></th>
                </tr></thead>
                <tbody>{move || {
                    let page=signals.outbound_page.get();
                    if !signals.outbound_loading.get() && page.items.is_empty() {
                        view! { <tr><td class="table-empty-row" colspan="7">"No outbound events match these filters."</td></tr> }.into_any()
                    } else {
                        page.items.into_iter().map(|event| {
                            let id=event.id;
                            let selected=signals.selected_outbound_id.get()==Some(id);
                            view! { <tr class:selected=selected>
                                <td>{compact_time(&event.created_at)}</td>
                                <td><strong class="mono">{event.event_type}</strong><small class="truncate-cell">{event.event_key}</small></td>
                                <td><strong>{event.aggregate_type}</strong><small>{format!("{} / seq {}",event.aggregate_id,event.aggregate_sequence)}</small></td>
                                <td>{scope_label(event.inventory_owner_name.as_deref(),event.facility_name.as_deref())}</td>
                                <td><span class=status_class(event.status)>{status_label(event.status)}</span></td>
                                <td class="numeric"><strong>{event.attempts}</strong><small>{format!("{} replays",event.replay_count)}</small></td>
                                <td><button type="button" class="icon-button" title="Open delivery detail" aria-label=format!("Open outbound event {id}") aria-pressed=selected on:click=move |_| select.run(id)><Icon icon=UiIcon::Search/></button></td>
                            </tr> }
                        }).collect_view().into_any()
                    }
                }}</tbody>
            </table>
        </div>
        <PageFooter
            label="events"
            count=Signal::derive(move || signals.outbound_page.get().items.len())
            loading=signals.outbound_loading
            has_previous=Signal::derive(move || !signals.outbound_history.get().is_empty())
            has_next=Signal::derive(move || signals.outbound_page.get().has_more())
            previous=Callback::new(move |_| previous_outbound(signals))
            next=Callback::new(move |_| next_outbound(signals))
        />
    }
}

#[component]
fn PageFooter(
    label: &'static str,
    count: Signal<usize>,
    loading: RwSignal<bool>,
    has_previous: Signal<bool>,
    has_next: Signal<bool>,
    previous: Callback<()>,
    next: Callback<()>,
) -> impl IntoView {
    view! {
        <footer class="integration-page-footer">
            <span>{move || if loading.get() { "Loading".to_owned() } else { format!("{} {label} on this page",count.get()) }}</span>
            <div>
                <button type="button" class="button quiet-action compact" disabled=move || loading.get() || !has_previous.get() on:click=move |_| previous.run(())>"Previous"</button>
                <button type="button" class="button quiet-action compact" disabled=move || loading.get() || !has_next.get() on:click=move |_| next.run(())>"Next"</button>
            </div>
        </footer>
    }
}

#[component]
fn InboundDetail(signals: MonitorSignals) -> impl IntoView {
    view! { {move || {
        if signals.inbound_detail_loading.get() {
            view! { <div class="integration-empty" aria-busy="true"><h2>"Loading receipt detail"</h2></div> }.into_any()
        } else {
            signals.inbound_detail.get().map_or_else(
                || view! { <div class="integration-empty"><h2>"Inbound receipt detail"</h2><p>"Select a receipt to inspect its immutable envelope and retained payload."</p></div> }.into_any(),
                |detail| inbound_detail_view(signals, detail),
            )
        }
    }} }
}

fn inbound_detail_view(
    signals: MonitorSignals,
    detail: InboundIntegrationDetailResponse,
) -> AnyView {
    let receipt = detail.receipt;
    let receipt_id = receipt.id;
    let download_path = api::inbound_payload_download_path(receipt.id);
    let correction_initial_payload = if detail.payload_preview_encoding
        == InboundPayloadPreviewEncoding::Utf8
        && !detail.preview_truncated
    {
        detail.payload_preview.clone()
    } else {
        String::new()
    };
    let encoding = match detail.payload_preview_encoding {
        InboundPayloadPreviewEncoding::Utf8 => "UTF-8",
        InboundPayloadPreviewEncoding::Hex => "Hexadecimal",
    };
    let processing = detail.processing.map(|processing| {
        let processing_for_command = StoredValue::new(processing.clone());
        let payload_for_correction = StoredValue::new(if processing.latest_correction_id.is_some() {
            if processing.latest_correction_payload_truncated {
                String::new()
            } else {
                processing.latest_correction_payload.clone().unwrap_or_default()
            }
        } else {
            correction_initial_payload.clone()
        });
        let status_class = match processing.status {
            IntegrationOrderProcessingStatus::Quarantined => "status held",
            IntegrationOrderProcessingStatus::Processed => "status shipped",
        };
        let status_label = match processing.status {
            IntegrationOrderProcessingStatus::Quarantined => "Quarantined",
            IntegrationOrderProcessingStatus::Processed => "Processed",
        };
        view! {
            <section class="integration-detail-section integration-processing">
                <header><h3>"Order processing"</h3><span class=status_class>{status_label}</span></header>
                <Show when=move || processing_for_command.get_value().status==IntegrationOrderProcessingStatus::Quarantined>
                    <div class="integration-command-band">
                    {move || {
                        if signals.reprocess_confirmation.get()==Some(receipt_id) {
                            view! { <div class="integration-replay-confirmation"><div><strong>"Reprocess retained payload?"</strong><span>"The current fulfillment-order mapping and business configuration will run against the immutable raw envelope."</span></div><div><button type="button" class="button quiet-action compact" disabled=move || signals.command_pending.get() on:click=move |_| signals.reprocess_confirmation.set(None)>"Cancel"</button><button type="button" class="button primary-action compact" disabled=move || signals.command_pending.get() on:click=move |_| submit_reprocess(signals,processing_for_command.get_value())><Icon icon=UiIcon::Refresh/>{move || if signals.command_pending.get() { "Processing" } else { "Reprocess" }}</button></div></div> }.into_any()
                        } else if let Some(saved)=signals.reprocess_retry.get().filter(|saved| saved.receipt_id==receipt_id) {
                            let saved=StoredValue::new(saved);
                            view! { <div class="integration-replay-confirmation"><div><strong>"Reprocess outcome is unknown"</strong><span>"Retry the exact saved command to reconcile without adding another attempt."</span></div><button type="button" class="button secondary-action compact" disabled=move || signals.command_pending.get() on:click=move |_| execute_reprocess(signals,saved.get_value())><Icon icon=UiIcon::Refresh/>"Retry exact command"</button></div> }.into_any()
                        } else {
                            view! { <button type="button" class="button secondary-action compact" disabled=move || signals.command_pending.get() on:click=move |_| signals.reprocess_confirmation.set(Some(receipt_id))><Icon icon=UiIcon::Refresh/>"Reprocess retained payload"</button> }.into_any()
                        }
                    }}
                    <CorrectionPanel signals receipt_id processing=processing_for_command.get_value() initial_payload=payload_for_correction.get_value()/>
                    </div>
                </Show>
                <dl class="integration-facts">
                    <div><dt>"Adapter"</dt><dd class="mono">{processing.adapter_key}</dd></div>
                    <div><dt>"Mapping"</dt><dd>{format!("Version {}",processing.mapping_version)}</dd></div>
                    <div><dt>"Revision"</dt><dd>{processing.revision.get()}</dd></div>
                    <div><dt>"Attempts"</dt><dd>{processing.attempt_count}</dd></div>
                    <div><dt>"Input SHA-256"</dt><dd class="mono wrap-anywhere">{processing.input_payload_sha256}</dd></div>
                    <div><dt>"Last operator"</dt><dd>{processing.attempted_by_name}</dd></div>
                    <div><dt>"Last attempt"</dt><dd>{processing.attempted_at}</dd></div>
                    {processing.order_id.map(|order_id| view! { <div><dt>"Created order"</dt><dd class="mono">{format!("#{order_id}")}</dd></div> })}
                    {processing.error_message.map(|message| view! { <div class="wide integration-failure"><dt>{processing.error_code.unwrap_or_else(|| "processing_error".into())}</dt><dd>{message}</dd></div> })}
                </dl>
                <div class="integration-attempts">{processing.attempts.into_iter().map(|attempt| {
                    let label=match attempt.status { IntegrationOrderProcessingStatus::Quarantined=>"Quarantined",IntegrationOrderProcessingStatus::Processed=>"Processed" };
                    let applied_mappings=attempt.applied_mappings;
                    view! { <article><header><strong>{format!("Attempt {}",attempt.attempt_number)}</strong><span>{label}</span></header><dl><div><dt>"Operator"</dt><dd>{attempt.attempted_by_name}</dd></div><div><dt>"Attempted"</dt><dd>{attempt.attempted_at}</dd></div><div><dt>"Revision"</dt><dd>{attempt.revision.get()}</dd></div>{attempt.order_id.map(|id| view! { <div><dt>"Order"</dt><dd class="mono">{format!("#{id}")}</dd></div> })}{attempt.correction_id.map(|id| view! { <div><dt>"Correction"</dt><dd class="mono">{format!("#{id}")}</dd></div> })}</dl>{(!applied_mappings.is_empty()).then(|| view! { <div class="integration-applied-mappings"><strong>"Applied item mappings"</strong>{applied_mappings.into_iter().map(|mapping| view! { <div><span class="mono">{format!("Line {} · {} / {}",mapping.line_key,mapping.external_item_key,mapping.external_uom)}</span><span>{format!("Item #{} / {} · mapping #{} r{}",mapping.item_id,mapping.requested_uom,mapping.mapping_id,mapping.mapping_revision.get())}</span></div> }).collect_view()}</div> })}{attempt.correction_reason.map(|reason| view! { <p>{reason}</p> })}{attempt.error_message.map(|message| view! { <p class="integration-failure">{message}</p> })}</article> }
                }).collect_view()}</div>
            </section>
        }
    });
    view! { <div class="integration-detail-content">
            <header><div><h2>{receipt.source_key.clone()}</h2><small>{format!("Receipt #{}",receipt.id)}</small></div><a class="button secondary-action compact" href=download_path download=""><Icon icon=UiIcon::Download/>"Download payload"</a></header>
            <dl class="integration-facts">
                <div><dt>"Received"</dt><dd>{receipt.received_at}</dd></div>
                <div><dt>"Content type"</dt><dd>{receipt.content_type}</dd></div>
                <div><dt>"Payload"</dt><dd>{format_bytes(receipt.payload_bytes)}</dd></div>
                <div><dt>"Scope"</dt><dd>{scope_label(receipt.inventory_owner_name.as_deref(),receipt.facility_name.as_deref())}</dd></div>
                {receipt.external_inventory_owner_key.map(|external_key| view! { <div class="wide"><dt>"Partner owner identity"</dt><dd class="mono">{format!("{} · mapping #{} r{}", external_key, receipt.owner_mapping_id.unwrap_or_default(), receipt.owner_mapping_revision.map(|revision| revision.get()).unwrap_or_default())}</dd></div> })}
                <div class="wide"><dt>"Deduplication key"</dt><dd class="mono">{receipt.deduplication_key}</dd></div>
                <div class="wide"><dt>"Request ID"</dt><dd class="mono">{receipt.request_id.unwrap_or_else(|| "Not supplied".into())}</dd></div>
                <div class="wide"><dt>"SHA-256"</dt><dd class="mono wrap-anywhere">{receipt.payload_sha256}</dd></div>
            </dl>
            {processing}
            <section class="integration-detail-section"><header><h3>"Payload preview"</h3><small>{format!("{encoding} / {}{}",format_bytes(detail.preview_bytes),if detail.preview_truncated { " shown" } else { " complete" })}</small></header><pre>{detail.payload_preview}</pre>{detail.preview_truncated.then(|| view! { <p>"Preview is limited to 64 KiB. Download the retained payload for the complete envelope."</p> })}</section>
        </div> }.into_any()
}

#[component]
fn OutboundDetail(signals: MonitorSignals) -> impl IntoView {
    view! { {move || {
        if signals.outbound_detail_loading.get() {
            view! { <div class="integration-empty" aria-busy="true"><h2>"Loading delivery detail"</h2></div> }.into_any()
        } else {
            signals.outbound_detail.get().map_or_else(
                || view! { <div class="integration-empty"><h2>"Outbound delivery detail"</h2><p>"Select an event to inspect its payload and attempts."</p></div> }.into_any(),
                |detail| outbound_detail_view(signals, detail),
            )
        }
    }} }
}

fn outbound_detail_view(
    signals: MonitorSignals,
    detail: OutboundIntegrationDetailResponse,
) -> AnyView {
    let event = detail.event;
    let payload = serde_json::to_string_pretty(&detail.payload).unwrap_or_else(|_| "{}".to_owned());
    let dead_letter_target = StoredValue::new(DeadLetterTarget {
        event_id: event.id,
        event_type: event.event_type.clone(),
        event_key: event.event_key.clone(),
        expected_replay_count: event.replay_count,
        previous_attempts: event.attempts,
    });
    let event_id = event.id;
    let can_replay = event.status == OutboundDeliveryStatus::DeadLettered;
    view! { <div class="integration-detail-content">
        <header><div><h2>{event.event_type.clone()}</h2><small class="mono">{event.event_key.clone()}</small></div><span class=status_class(event.status)>{status_label(event.status)}</span></header>
        <Show when=move || can_replay>
            <div class="integration-command-band">
                {move || {
                    if let Some(confirmation)=signals.replay_confirmation.get().filter(|value| value.event_id==event_id) {
                        let confirmation=StoredValue::new(confirmation);
                        view! { <div class="integration-replay-confirmation">
                            <div><strong>"Replay this dead letter?"</strong><span>{format!("Delivery generation {} failed after {} attempts. The original immutable event will be queued again.",confirmation.get_value().expected_replay_count,confirmation.get_value().previous_attempts)}</span><small class="mono">{format!("{} / {}",confirmation.get_value().event_type,confirmation.get_value().event_key)}</small></div>
                            <div><button type="button" class="button quiet-action compact" disabled=move || signals.command_pending.get() on:click=move |_| signals.replay_confirmation.set(None)>"Cancel"</button><button type="button" class="button primary-action compact" disabled=move || signals.command_pending.get() on:click=move |_| submit_replay(signals,confirmation.get_value())><Icon icon=UiIcon::Refresh/>{move || if signals.command_pending.get() { "Replaying" } else { "Replay now" }}</button></div>
                        </div> }.into_any()
                    } else if let Some(saved)=signals.replay_retry.get().filter(|value| value.event_id==event_id) {
                        let saved=StoredValue::new(saved);
                        view! { <div class="integration-replay-confirmation"><div><strong>"Replay outcome is unknown"</strong><span>"Retry the exact saved command to reconcile the delivery without creating another replay generation."</span></div><button type="button" class="button secondary-action compact" disabled=move || signals.command_pending.get() on:click=move |_| execute_replay(signals,saved.get_value())><Icon icon=UiIcon::Refresh/>{move || if signals.command_pending.get() { "Reconciling" } else { "Retry exact replay" }}</button></div> }.into_any()
                    } else if let Some(confirmation)=signals.discard_confirmation.get().filter(|value| value.event_id==event_id) {
                        let confirmation=StoredValue::new(confirmation);
                        view! { <div class="integration-discard-confirmation">
                            <div><strong>"Discard this dead letter permanently?"</strong><span>"This terminal action unblocks later events on the ordering key. The failure and operator rationale remain auditable after outbox purge."</span><small class="mono">{format!("{} / {}",confirmation.get_value().event_type,confirmation.get_value().event_key)}</small></div>
                            <label><span>"Reason"</span><textarea maxlength="1000" placeholder="Why delivery must not be retried" prop:value=move || signals.discard_reason.get() on:input=move |event| signals.discard_reason.set(event_target_value(&event))></textarea></label>
                            <div><button type="button" class="button quiet-action compact" disabled=move || signals.command_pending.get() on:click=move |_| signals.discard_confirmation.set(None)>"Cancel"</button><button type="button" class="button danger-action compact" disabled=move || signals.command_pending.get() on:click=move |_| submit_discard(signals,confirmation.get_value())><Icon icon=UiIcon::Remove/>{move || if signals.command_pending.get() { "Discarding" } else { "Confirm discard" }}</button></div>
                        </div> }.into_any()
                    } else if let Some(saved)=signals.discard_retry.get().filter(|value| value.event_id==event_id) {
                        let saved=StoredValue::new(saved);
                        view! { <div class="integration-replay-confirmation"><div><strong>"Discard outcome is unknown"</strong><span>"Retry the exact saved command to reconcile the terminal disposition without changing its reason."</span></div><button type="button" class="button danger-action compact" disabled=move || signals.command_pending.get() on:click=move |_| execute_discard(signals,saved.get_value())><Icon icon=UiIcon::Refresh/>{move || if signals.command_pending.get() { "Reconciling" } else { "Retry exact discard" }}</button></div> }.into_any()
                    } else {
                        view! { <div class="integration-dead-letter-actions"><button type="button" class="button secondary-action compact" disabled=move || signals.command_pending.get() on:click=move |_| signals.replay_confirmation.set(Some(dead_letter_target.get_value()))><Icon icon=UiIcon::Refresh/>"Replay delivery"</button><button type="button" class="button danger-action compact" disabled=move || signals.command_pending.get() on:click=move |_| { signals.discard_reason.set(String::new()); signals.discard_confirmation.set(Some(dead_letter_target.get_value())); }><Icon icon=UiIcon::Remove/>"Discard permanently"</button></div> }.into_any()
                    }
                }}
            </div>
        </Show>
        <dl class="integration-facts">
            <div><dt>"Aggregate"</dt><dd>{format!("{} / {}",event.aggregate_type,event.aggregate_id)}</dd></div>
            <div><dt>"Sequence"</dt><dd>{event.aggregate_sequence}</dd></div>
            <div><dt>"Created"</dt><dd>{event.created_at}</dd></div>
            <div><dt>"Available"</dt><dd>{event.available_at}</dd></div>
            <div><dt>"Attempts"</dt><dd>{event.attempts}</dd></div>
            <div><dt>"Replays"</dt><dd>{event.replay_count}</dd></div>
            {event.last_error.map(|error| view! { <div class="wide integration-failure"><dt>"Latest failure"</dt><dd>{error}</dd></div> })}
        </dl>
        {detail.discard.map(|discard| view! { <section class="integration-detail-section integration-discard-evidence"><h3>"Terminal discard"</h3><dl><div><dt>"Operator"</dt><dd>{discard.discarded_by_name}</dd></div><div><dt>"Discarded"</dt><dd>{discard.discarded_at}</dd></div><div><dt>"Generation"</dt><dd>{discard.replay_count}</dd></div><div><dt>"Prior attempts"</dt><dd>{discard.previous_attempts}</dd></div></dl><p>{discard.reason}</p><small class="mono">{format!("Evidence #{}",discard.discard_id)}</small></section> })}
        <section class="integration-detail-section"><h3>"Payload"</h3><pre>{payload}</pre></section>
        <section class="integration-detail-section"><h3>"Delivery attempts"</h3>
            <div class="integration-attempts">{if detail.attempts.is_empty() {
                view! { <p>"No delivery attempts recorded."</p> }.into_any()
            } else {
                detail.attempts.into_iter().map(|attempt| view! { <article>
                    <header><strong>{format!("Attempt {}",attempt.attempt_number)}</strong><span>{attempt_outcome_label(attempt.outcome)}</span></header>
                    <dl><div><dt>"Publisher"</dt><dd>{attempt.publisher_name}</dd></div><div><dt>"Worker"</dt><dd class="mono">{attempt.worker_id}</dd></div><div><dt>"Claimed"</dt><dd>{attempt.claimed_at}</dd></div><div><dt>"Replay"</dt><dd>{attempt.replay_count}</dd></div></dl>
                    {attempt.error.map(|error| view! { <p class="integration-failure">{error}</p> })}
                </article> }).collect_view().into_any()
            }}</div>
        </section>
        <section class="integration-detail-section"><h3>"Replay history"</h3>
            <div class="integration-attempts">{if detail.replays.is_empty() {
                view! { <p>"No dead-letter replays recorded."</p> }.into_any()
            } else {
                detail.replays.into_iter().map(|replay| view! { <article>
                    <header><strong>{format!("Replay {}",replay.replay_count)}</strong><span>{replay.replayed_at}</span></header>
                    <dl><div><dt>"Operator"</dt><dd>{replay.replayed_by_name}</dd></div><div><dt>"Prior attempts"</dt><dd>{replay.previous_attempts}</dd></div><div><dt>"Generation"</dt><dd>{format!("{} -> {}",replay.previous_replay_count,replay.replay_count)}</dd></div><div><dt>"Evidence ID"</dt><dd class="mono">{format!("#{}",replay.replay_id)}</dd></div></dl>
                    <p class="integration-failure">{replay.last_error}</p>
                </article> }).collect_view().into_any()
            }}</div>
        </section>
    </div> }.into_any()
}

fn submit_reprocess(
    signals: MonitorSignals,
    processing: wareboxes_api_contract::v1::InboundIntegrationProcessingResponse,
) {
    let saved = SavedReprocessCommand {
        receipt_id: signals
            .selected_inbound_id
            .get_untracked()
            .unwrap_or_default(),
        request: ReprocessIntegrationOrderRequest {
            expected_revision: processing.revision,
        },
        idempotency_key: api::new_idempotency_key(),
    };
    execute_reprocess(signals, saved);
}

fn execute_reprocess(signals: MonitorSignals, saved: SavedReprocessCommand) {
    if signals.command_pending.get_untracked() || saved.receipt_id <= 0 {
        return;
    }
    signals.command_pending.set(true);
    signals.error.set(None);
    signals.notice.set(None);
    let receipt_id = saved.receipt_id;
    leptos::task::spawn_local(async move {
        let result =
            api::reprocess_inbound_order(receipt_id, &saved.request, &saved.idempotency_key).await;
        match result {
            Ok(result) => {
                signals.reprocess_retry.set(None);
                signals.reprocess_confirmation.set(None);
                request_inbound(signals, None, Vec::new());
                select_inbound_receipt(signals, receipt_id);
                signals.notice.set(Some(match result.status {
                    IntegrationOrderProcessingStatus::Processed => result.order_id.map_or_else(
                        || "Inbound order processed.".to_owned(),
                        |order_id| format!("Inbound order processed as order #{order_id}."),
                    ),
                    IntegrationOrderProcessingStatus::Quarantined => {
                        format!(
                            "Reprocess attempt {} remains quarantined.",
                            result.attempt_count
                        )
                    }
                }));
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) if error.ambiguous_outcome => {
                signals.reprocess_retry.set(Some(saved));
                signals.reprocess_confirmation.set(None);
                signals.error.set(Some(format!(
                    "{} Retry the exact saved reprocess command to reconcile the outcome.",
                    error.message
                )));
            }
            Err(error) => {
                signals.reprocess_retry.set(None);
                signals.reprocess_confirmation.set(None);
                select_inbound_receipt(signals, receipt_id);
                signals.error.set(Some(error.message));
            }
        }
        signals.command_pending.set(false);
    });
}

fn submit_replay(signals: MonitorSignals, confirmation: DeadLetterTarget) {
    let saved = SavedReplayCommand {
        event_id: confirmation.event_id,
        request: ReplayOutboxDeadLetterRequest {
            expected_replay_count: confirmation.expected_replay_count,
        },
        idempotency_key: api::new_idempotency_key(),
    };
    execute_replay(signals, saved);
}

fn execute_replay(signals: MonitorSignals, saved: SavedReplayCommand) {
    if signals.command_pending.get_untracked() {
        return;
    }
    signals.command_pending.set(true);
    signals.error.set(None);
    signals.notice.set(None);
    let event_id = saved.event_id;
    leptos::task::spawn_local(async move {
        let result =
            api::replay_outbound_dead_letter(event_id, &saved.request, &saved.idempotency_key)
                .await;
        match result {
            Ok(result) => {
                signals.replay_retry.set(None);
                signals.replay_confirmation.set(None);
                request_outbound(signals, None, Vec::new());
                select_outbound_event(signals, event_id);
                signals.notice.set(Some(format!(
                    "Replay {} queued for {}.",
                    result.replay_count, result.event_type
                )));
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) if error.ambiguous_outcome => {
                signals.replay_retry.set(Some(saved));
                signals.replay_confirmation.set(None);
                signals.error.set(Some(format!(
                    "{} Retry the exact saved replay to reconcile the outcome.",
                    error.message
                )));
            }
            Err(error) => {
                signals.replay_retry.set(None);
                signals.replay_confirmation.set(None);
                select_outbound_event(signals, event_id);
                signals.error.set(Some(error.message));
            }
        }
        signals.command_pending.set(false);
    });
}

fn submit_discard(signals: MonitorSignals, target: DeadLetterTarget) {
    let reason = signals.discard_reason.get_untracked();
    let reason = reason.trim();
    if reason.is_empty() {
        signals
            .error
            .set(Some("Discard reason is required.".to_owned()));
        return;
    }
    if reason.chars().count() > 1_000 || reason.chars().any(char::is_control) {
        signals.error.set(Some(
            "Discard reason must be at most 1000 control-free characters.".to_owned(),
        ));
        return;
    }
    let saved = SavedDiscardCommand {
        event_id: target.event_id,
        request: DiscardOutboxDeadLetterRequest {
            expected_replay_count: target.expected_replay_count,
            reason: reason.to_owned(),
        },
        idempotency_key: api::new_idempotency_key(),
    };
    execute_discard(signals, saved);
}

fn execute_discard(signals: MonitorSignals, saved: SavedDiscardCommand) {
    if signals.command_pending.get_untracked() {
        return;
    }
    signals.command_pending.set(true);
    signals.error.set(None);
    signals.notice.set(None);
    let event_id = saved.event_id;
    leptos::task::spawn_local(async move {
        let result =
            api::discard_outbound_dead_letter(event_id, &saved.request, &saved.idempotency_key)
                .await;
        match result {
            Ok(result) => {
                signals.discard_retry.set(None);
                signals.discard_confirmation.set(None);
                request_outbound(signals, None, Vec::new());
                select_outbound_event(signals, event_id);
                signals.notice.set(Some(format!(
                    "{} discarded permanently.",
                    result.event_type
                )));
            }
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) if error.ambiguous_outcome => {
                signals.discard_retry.set(Some(saved));
                signals.discard_confirmation.set(None);
                signals.error.set(Some(format!(
                    "{} Retry the exact saved discard to reconcile the outcome.",
                    error.message
                )));
            }
            Err(error) => {
                signals.discard_retry.set(None);
                signals.discard_confirmation.set(None);
                select_outbound_event(signals, event_id);
                signals.error.set(Some(error.message));
            }
        }
        signals.command_pending.set(false);
    });
}

fn refresh_current(signals: MonitorSignals) {
    match signals.tab.get_untracked() {
        MonitorTab::Inbound => request_inbound(signals, None, Vec::new()),
        MonitorTab::Outbound => request_outbound(signals, None, Vec::new()),
        MonitorTab::Mappings => {}
    }
}

fn detail_refresh_is_current(
    selected_at_request: Option<i64>,
    selected_now: Option<i64>,
    refresh_generation: Option<u64>,
    current_generation: u64,
) -> bool {
    selected_at_request.is_some()
        && selected_at_request == selected_now
        && refresh_generation == Some(current_generation)
}

fn should_reconcile_detail(active_tab: MonitorTab, request_tab: MonitorTab) -> bool {
    active_tab == request_tab
}

fn request_inbound(
    signals: MonitorSignals,
    cursor: Option<OpaqueCursor>,
    history: Vec<Option<OpaqueCursor>>,
) {
    let generation = signals.inbound_generation.get_untracked() + 1;
    signals.inbound_generation.set(generation);
    signals.inbound_loading.set(true);
    signals.error.set(None);
    let selected_id = signals.selected_inbound_id.get_untracked();
    let previous_detail = signals.inbound_detail.get_untracked();
    let detail_generation = selected_id.map(|_| {
        let generation = signals.inbound_detail_generation.get_untracked() + 1;
        signals.inbound_detail_generation.set(generation);
        signals.inbound_detail.set(None);
        signals.inbound_detail_loading.set(true);
        generation
    });
    let filters = InboundIntegrationFilters {
        query: text_filter(&signals.search.get_untracked()),
        source_key: text_filter(&signals.source_key.get_untracked()),
        ..Default::default()
    };
    let sort = signals.inbound_sort.get_untracked();
    let direction = signals.inbound_direction.get_untracked();
    leptos::task::spawn_local(async move {
        let result = api::inbound_integrations(&filters, sort, direction, cursor.as_ref()).await;
        if signals.inbound_generation.get_untracked() != generation {
            return;
        }
        match result {
            Ok(page) => {
                let selected_still_visible =
                    selected_id.is_some_and(|id| page.items.iter().any(|receipt| receipt.id == id));
                signals.inbound_page.set(page);
                signals.inbound_cursor.set(cursor);
                signals.inbound_history.set(history);
                if detail_refresh_is_current(
                    selected_id,
                    signals.selected_inbound_id.get_untracked(),
                    detail_generation,
                    signals.inbound_detail_generation.get_untracked(),
                ) {
                    match (
                        selected_id,
                        selected_still_visible,
                        should_reconcile_detail(signals.tab.get_untracked(), MonitorTab::Inbound),
                    ) {
                        (Some(id), true, true) => {
                            let notice = signals.notice.get_untracked();
                            select_inbound_receipt(signals, id);
                            signals.notice.set(notice);
                        }
                        (Some(_), true, false) => {
                            signals.inbound_detail.set(previous_detail.clone());
                            signals.inbound_detail_loading.set(false);
                        }
                        (Some(_), false, _) => clear_inbound_selection(signals),
                        _ => {}
                    }
                }
            }
            Err(error) if error.unauthorized => {
                if detail_refresh_is_current(
                    selected_id,
                    signals.selected_inbound_id.get_untracked(),
                    detail_generation,
                    signals.inbound_detail_generation.get_untracked(),
                ) {
                    if signals.tab.get_untracked() != MonitorTab::Inbound {
                        signals.inbound_detail.set(previous_detail.clone());
                    }
                    signals.inbound_detail_loading.set(false);
                }
                signals.on_unauthorized.run(());
            }
            Err(error) => {
                if detail_refresh_is_current(
                    selected_id,
                    signals.selected_inbound_id.get_untracked(),
                    detail_generation,
                    signals.inbound_detail_generation.get_untracked(),
                ) {
                    if signals.tab.get_untracked() != MonitorTab::Inbound {
                        signals.inbound_detail.set(previous_detail);
                    }
                    signals.inbound_detail_loading.set(false);
                }
                signals.error.set(Some(error.message));
            }
        }
        signals.inbound_loading.set(false);
    });
}

fn request_outbound(
    signals: MonitorSignals,
    cursor: Option<OpaqueCursor>,
    history: Vec<Option<OpaqueCursor>>,
) {
    let generation = signals.outbound_generation.get_untracked() + 1;
    signals.outbound_generation.set(generation);
    signals.outbound_loading.set(true);
    signals.error.set(None);
    let selected_id = signals.selected_outbound_id.get_untracked();
    let previous_detail = signals.outbound_detail.get_untracked();
    let detail_generation = selected_id.map(|_| {
        let generation = signals.outbound_detail_generation.get_untracked() + 1;
        signals.outbound_detail_generation.set(generation);
        signals.outbound_detail.set(None);
        signals.outbound_detail_loading.set(true);
        generation
    });
    let filters = OutboundIntegrationFilters {
        query: text_filter(&signals.search.get_untracked()),
        event_type: text_filter(&signals.event_type.get_untracked()),
        status: signals.outbound_status.get_untracked(),
        ..Default::default()
    };
    let sort = signals.outbound_sort.get_untracked();
    let direction = signals.outbound_direction.get_untracked();
    leptos::task::spawn_local(async move {
        let result = api::outbound_integrations(&filters, sort, direction, cursor.as_ref()).await;
        if signals.outbound_generation.get_untracked() != generation {
            return;
        }
        match result {
            Ok(page) => {
                let selected_still_visible =
                    selected_id.is_some_and(|id| page.items.iter().any(|event| event.id == id));
                signals.outbound_page.set(page);
                signals.outbound_cursor.set(cursor);
                signals.outbound_history.set(history);
                if detail_refresh_is_current(
                    selected_id,
                    signals.selected_outbound_id.get_untracked(),
                    detail_generation,
                    signals.outbound_detail_generation.get_untracked(),
                ) {
                    match (
                        selected_id,
                        selected_still_visible,
                        should_reconcile_detail(signals.tab.get_untracked(), MonitorTab::Outbound),
                    ) {
                        (Some(id), true, true) => {
                            let notice = signals.notice.get_untracked();
                            select_outbound_event(signals, id);
                            signals.notice.set(notice);
                        }
                        (Some(_), true, false) => {
                            signals.outbound_detail.set(previous_detail.clone());
                            signals.outbound_detail_loading.set(false);
                        }
                        (Some(_), false, _) => clear_outbound_selection(signals),
                        _ => {}
                    }
                }
            }
            Err(error) if error.unauthorized => {
                if detail_refresh_is_current(
                    selected_id,
                    signals.selected_outbound_id.get_untracked(),
                    detail_generation,
                    signals.outbound_detail_generation.get_untracked(),
                ) {
                    if signals.tab.get_untracked() != MonitorTab::Outbound {
                        signals.outbound_detail.set(previous_detail.clone());
                    }
                    signals.outbound_detail_loading.set(false);
                }
                signals.on_unauthorized.run(());
            }
            Err(error) => {
                if detail_refresh_is_current(
                    selected_id,
                    signals.selected_outbound_id.get_untracked(),
                    detail_generation,
                    signals.outbound_detail_generation.get_untracked(),
                ) {
                    if signals.tab.get_untracked() != MonitorTab::Outbound {
                        signals.outbound_detail.set(previous_detail);
                    }
                    signals.outbound_detail_loading.set(false);
                }
                signals.error.set(Some(error.message));
            }
        }
        signals.outbound_loading.set(false);
    });
}

fn select_inbound_receipt(signals: MonitorSignals, receipt_id: i64) {
    let generation = signals.inbound_detail_generation.get_untracked() + 1;
    signals.inbound_detail_generation.set(generation);
    signals.selected_inbound_id.set(Some(receipt_id));
    signals.inbound_detail.set(None);
    signals.inbound_detail_loading.set(true);
    signals.error.set(None);
    signals.notice.set(None);
    if signals
        .reprocess_retry
        .get_untracked()
        .as_ref()
        .map(|saved| saved.receipt_id)
        != Some(receipt_id)
    {
        signals.reprocess_retry.set(None);
    }
    signals.reprocess_confirmation.set(None);
    leptos::task::spawn_local(async move {
        let result = api::inbound_integration_detail(receipt_id).await;
        if signals.inbound_detail_generation.get_untracked() != generation
            || signals.selected_inbound_id.get_untracked() != Some(receipt_id)
        {
            return;
        }
        match result {
            Ok(detail) => signals.inbound_detail.set(Some(detail)),
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.error.set(Some(error.message)),
        }
        signals.inbound_detail_loading.set(false);
    });
}

fn select_outbound_event(signals: MonitorSignals, event_id: i64) {
    let generation = signals.outbound_detail_generation.get_untracked() + 1;
    signals.outbound_detail_generation.set(generation);
    signals.selected_outbound_id.set(Some(event_id));
    signals.outbound_detail.set(None);
    signals.outbound_detail_loading.set(true);
    signals.error.set(None);
    signals.notice.set(None);
    if signals
        .replay_retry
        .get_untracked()
        .as_ref()
        .map(|value| value.event_id)
        != Some(event_id)
    {
        signals.replay_retry.set(None);
    }
    if signals
        .discard_retry
        .get_untracked()
        .as_ref()
        .map(|value| value.event_id)
        != Some(event_id)
    {
        signals.discard_retry.set(None);
    }
    signals.replay_confirmation.set(None);
    signals.discard_confirmation.set(None);
    leptos::task::spawn_local(async move {
        let result = api::outbound_integration_detail(event_id).await;
        if signals.outbound_detail_generation.get_untracked() != generation
            || signals.selected_outbound_id.get_untracked() != Some(event_id)
        {
            return;
        }
        match result {
            Ok(detail) => signals.outbound_detail.set(Some(detail)),
            Err(error) if error.unauthorized => signals.on_unauthorized.run(()),
            Err(error) => signals.error.set(Some(error.message)),
        }
        signals.outbound_detail_loading.set(false);
    });
}

fn clear_inbound_selection(signals: MonitorSignals) {
    signals
        .inbound_detail_generation
        .update(|value| *value += 1);
    signals.selected_inbound_id.set(None);
    signals.inbound_detail.set(None);
    signals.inbound_detail_loading.set(false);
    signals.reprocess_confirmation.set(None);
}

fn clear_outbound_selection(signals: MonitorSignals) {
    signals
        .outbound_detail_generation
        .update(|value| *value += 1);
    signals.selected_outbound_id.set(None);
    signals.outbound_detail.set(None);
    signals.outbound_detail_loading.set(false);
    signals.replay_confirmation.set(None);
    signals.discard_confirmation.set(None);
}

fn change_inbound_sort(signals: MonitorSignals, sort: InboundIntegrationSort) {
    if signals.inbound_sort.get_untracked() == sort {
        signals
            .inbound_direction
            .update(|value| *value = reverse_direction(*value));
    } else {
        signals.inbound_sort.set(sort);
        signals
            .inbound_direction
            .set(IntegrationSortDirection::Ascending);
    }
    request_inbound(signals, None, Vec::new());
}
fn change_outbound_sort(signals: MonitorSignals, sort: OutboundIntegrationSort) {
    if signals.outbound_sort.get_untracked() == sort {
        signals
            .outbound_direction
            .update(|value| *value = reverse_direction(*value));
    } else {
        signals.outbound_sort.set(sort);
        signals
            .outbound_direction
            .set(IntegrationSortDirection::Ascending);
    }
    request_outbound(signals, None, Vec::new());
}
fn next_inbound(signals: MonitorSignals) {
    if let Some(next) = signals.inbound_page.get_untracked().next_cursor {
        let mut history = signals.inbound_history.get_untracked();
        history.push(signals.inbound_cursor.get_untracked());
        request_inbound(signals, Some(next), history);
    }
}
fn previous_inbound(signals: MonitorSignals) {
    let mut history = signals.inbound_history.get_untracked();
    if let Some(previous) = history.pop() {
        request_inbound(signals, previous, history);
    }
}
fn next_outbound(signals: MonitorSignals) {
    if let Some(next) = signals.outbound_page.get_untracked().next_cursor {
        let mut history = signals.outbound_history.get_untracked();
        history.push(signals.outbound_cursor.get_untracked());
        request_outbound(signals, Some(next), history);
    }
}
fn previous_outbound(signals: MonitorSignals) {
    let mut history = signals.outbound_history.get_untracked();
    if let Some(previous) = history.pop() {
        request_outbound(signals, previous, history);
    }
}

fn text_filter(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}
fn reverse_direction(value: IntegrationSortDirection) -> IntegrationSortDirection {
    match value {
        IntegrationSortDirection::Ascending => IntegrationSortDirection::Descending,
        IntegrationSortDirection::Descending => IntegrationSortDirection::Ascending,
    }
}
fn display_direction(value: IntegrationSortDirection) -> SortDirection {
    match value {
        IntegrationSortDirection::Ascending => SortDirection::Ascending,
        IntegrationSortDirection::Descending => SortDirection::Descending,
    }
}
fn active_loading(signals: MonitorSignals) -> bool {
    match signals.tab.get() {
        MonitorTab::Inbound => signals.inbound_loading.get(),
        MonitorTab::Outbound => signals.outbound_loading.get(),
        MonitorTab::Mappings => false,
    }
}
fn parse_status(value: &str) -> Option<OutboundDeliveryStatus> {
    match value {
        "pending" => Some(OutboundDeliveryStatus::Pending),
        "claimed" => Some(OutboundDeliveryStatus::Claimed),
        "retry_scheduled" => Some(OutboundDeliveryStatus::RetryScheduled),
        "dead_lettered" => Some(OutboundDeliveryStatus::DeadLettered),
        "published" => Some(OutboundDeliveryStatus::Published),
        "discarded" => Some(OutboundDeliveryStatus::Discarded),
        _ => None,
    }
}
fn status_value(value: Option<OutboundDeliveryStatus>) -> &'static str {
    match value {
        Some(OutboundDeliveryStatus::Pending) => "pending",
        Some(OutboundDeliveryStatus::Claimed) => "claimed",
        Some(OutboundDeliveryStatus::RetryScheduled) => "retry_scheduled",
        Some(OutboundDeliveryStatus::DeadLettered) => "dead_lettered",
        Some(OutboundDeliveryStatus::Published) => "published",
        Some(OutboundDeliveryStatus::Discarded) => "discarded",
        None => "",
    }
}
fn status_label(value: OutboundDeliveryStatus) -> &'static str {
    match value {
        OutboundDeliveryStatus::Pending => "Pending",
        OutboundDeliveryStatus::Claimed => "Claimed",
        OutboundDeliveryStatus::RetryScheduled => "Retry scheduled",
        OutboundDeliveryStatus::DeadLettered => "Dead lettered",
        OutboundDeliveryStatus::Published => "Published",
        OutboundDeliveryStatus::Discarded => "Discarded",
    }
}
fn status_class(value: OutboundDeliveryStatus) -> &'static str {
    match value {
        OutboundDeliveryStatus::Published => "status shipped",
        OutboundDeliveryStatus::Pending | OutboundDeliveryStatus::Claimed => "status open",
        OutboundDeliveryStatus::RetryScheduled => "status processing",
        OutboundDeliveryStatus::DeadLettered => "status held",
        OutboundDeliveryStatus::Discarded => "status muted",
    }
}
fn processing_status_label(value: IntegrationOrderProcessingStatus) -> &'static str {
    match value {
        IntegrationOrderProcessingStatus::Quarantined => "Quarantined",
        IntegrationOrderProcessingStatus::Processed => "Processed",
    }
}
fn processing_status_class(value: IntegrationOrderProcessingStatus) -> &'static str {
    match value {
        IntegrationOrderProcessingStatus::Quarantined => "status held",
        IntegrationOrderProcessingStatus::Processed => "status shipped",
    }
}
fn attempt_outcome_label(value: Option<OutboundDeliveryAttemptOutcome>) -> &'static str {
    match value {
        Some(OutboundDeliveryAttemptOutcome::Published) => "Published",
        Some(OutboundDeliveryAttemptOutcome::RetryScheduled) => "Retry scheduled",
        Some(OutboundDeliveryAttemptOutcome::PermanentFailure) => "Permanent failure",
        Some(OutboundDeliveryAttemptOutcome::RetryExhausted) => "Retry exhausted",
        Some(OutboundDeliveryAttemptOutcome::LeaseLost) => "Lease lost",
        None => "In progress",
    }
}
fn scope_label(owner: Option<&str>, facility: Option<&str>) -> String {
    match (owner, facility) {
        (Some(owner), Some(facility)) => format!("{owner} / {facility}"),
        (Some(owner), None) => owner.to_owned(),
        (None, Some(facility)) => facility.to_owned(),
        (None, None) => "Organization".to_owned(),
    }
}
fn compact_time(value: &str) -> String {
    value.split_once('T').map_or_else(
        || value.to_owned(),
        |(date, time)| format!("{date} {}", &time[..time.len().min(8)]),
    )
}
fn format_bytes(value: i64) -> String {
    if value >= 1_048_576 {
        format!("{:.1} MB", value as f64 / 1_048_576.0)
    } else if value >= 1024 {
        format!("{:.1} KB", value as f64 / 1024.0)
    } else {
        format!("{value} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tab_status_and_scope_labels_are_exact() {
        assert_eq!(
            parse_status("dead_lettered"),
            Some(OutboundDeliveryStatus::DeadLettered)
        );
        assert_eq!(parse_status("all"), None);
        assert_eq!(scope_label(Some("Client"), Some("West")), "Client / West");
    }
    #[test]
    fn byte_and_time_labels_are_dense() {
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(
            compact_time("2026-08-10T11:22:33+00:00"),
            "2026-08-10 11:22:33"
        );
    }
    #[test]
    fn inbound_processing_labels_distinguish_actionable_quarantine() {
        assert_eq!(
            processing_status_label(IntegrationOrderProcessingStatus::Quarantined),
            "Quarantined"
        );
        assert_eq!(
            processing_status_class(IntegrationOrderProcessingStatus::Quarantined),
            "status held"
        );
        assert_eq!(
            processing_status_label(IntegrationOrderProcessingStatus::Processed),
            "Processed"
        );
    }

    #[test]
    fn detail_refresh_reconciliation_rejects_changed_selection_or_newer_request() {
        assert!(detail_refresh_is_current(Some(41), Some(41), Some(7), 7));
        assert!(!detail_refresh_is_current(Some(41), Some(42), Some(7), 7));
        assert!(!detail_refresh_is_current(Some(41), Some(41), Some(7), 8));
        assert!(!detail_refresh_is_current(None, None, None, 7));
    }

    #[test]
    fn detail_refresh_reconciliation_rejects_a_direction_switch() {
        assert!(should_reconcile_detail(
            MonitorTab::Inbound,
            MonitorTab::Inbound
        ));
        assert!(!should_reconcile_detail(
            MonitorTab::Outbound,
            MonitorTab::Inbound
        ));
        assert!(!should_reconcile_detail(
            MonitorTab::Mappings,
            MonitorTab::Outbound
        ));
    }
}
