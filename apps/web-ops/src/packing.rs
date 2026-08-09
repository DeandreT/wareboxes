use leptos::html;
use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    CartonDimensions, CartonMeasurements, CloseCartonRequest, CreateCartonRequest,
    DimensionMillimeters, OpaqueCursor, OpenPackSessionRequest, PackAllocationDispositionResponse,
    PackCartonLifecycleResponse, PackPickedAllocationRequest, PackSessionResponse,
    PackingQueueEntryResponse, PackingQueuePage, RemovePackedContentRequest, VoidCartonRequest,
    WeightGrams,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;
use wareboxes_core::models::Location;

use crate::api;
use crate::components::{Icon, UiIcon};
use crate::toast::{use_toast_bus, ToastBus};
use crate::view_model::format_quantity;

mod commands;
mod identity;
mod removal;
mod view;

use self::commands::{execute_command, PackingCommandResult, PendingPackingCommand};
use self::identity::{advance_item_identity, matching_item_candidates, start_item_identity};
use self::removal::{PackingRemovalDialog, PendingContentRemoval};
use self::view::{
    facility_label, packing_locations, packing_progress_label, selected_location, station_label,
    PackingActive, PackingIdle,
};

#[derive(Clone, Copy)]
struct PackingSignals {
    session: RwSignal<Option<PackSessionResponse>>,
    pending: RwSignal<bool>,
    message: RwSignal<String>,
    error: RwSignal<bool>,
    retry: RwSignal<Option<PendingPackingCommand>>,
    refresh_order_id: RwSignal<Option<i64>>,
    scan: RwSignal<String>,
    source_plate: RwSignal<Option<String>>,
    item_identity: RwSignal<Option<PendingItemIdentity>>,
    removal: RwSignal<Option<PendingContentRemoval>>,
    completed_order_ids: RwSignal<Vec<i64>>,
    focus_epoch: RwSignal<u64>,
    measurements: CartonMeasurementSignals,
    on_unauthorized: Callback<()>,
    toasts: ToastBus,
}

#[derive(Clone, Copy)]
struct PackingQueueSignals {
    entries: RwSignal<Vec<PackingQueueEntryResponse>>,
    next_cursor: RwSignal<Option<OpaqueCursor>>,
    facility_id: RwSignal<Option<i64>>,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
}

impl PackingSignals {
    fn blocked(self) -> bool {
        self.pending.get()
            || self.retry.get().is_some()
            || self.refresh_order_id.get().is_some()
            || self.removal.get().is_some()
    }

    fn refocus(self) {
        self.focus_epoch
            .update(|epoch| *epoch = epoch.saturating_add(1));
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PackingLocation {
    id: i64,
    facility_id: i64,
    label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct IdentityCandidate {
    allocation_id: i64,
    lot: Option<String>,
    serial: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IdentityScanStage {
    Lot,
    Serial,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingItemIdentity {
    item_barcode: String,
    candidates: Vec<IdentityCandidate>,
    lot_scan: Option<String>,
    stage: IdentityScanStage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ResolvedItemIdentity {
    allocation_id: i64,
    item_barcode: String,
    lot_scan: Option<String>,
    serial_scan: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ItemIdentityResolution {
    Await(PendingItemIdentity),
    Resolved(ResolvedItemIdentity),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct IdentityScanError {
    message: &'static str,
    reset: bool,
}

#[component]
pub(crate) fn PackingWorkspace(
    initial_queue: PackingQueuePage,
    access: AccessScopeWorkspace,
    locations: Vec<Location>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let packing_locations = packing_locations(locations);
    let selected_location_id = RwSignal::new(
        packing_locations
            .first()
            .map_or_else(String::new, |location| location.id.to_string()),
    );
    let station_locations = StoredValue::new(packing_locations);
    let station_facilities = StoredValue::new(access.facilities);
    let queue = PackingQueueSignals {
        entries: RwSignal::new(initial_queue.items),
        next_cursor: RwSignal::new(initial_queue.next_cursor),
        facility_id: RwSignal::new(None),
        pending: RwSignal::new(false),
        error: RwSignal::new(None),
        generation: RwSignal::new(0),
    };
    let session = RwSignal::new(None::<PackSessionResponse>);
    let pending = RwSignal::new(false);
    let message = RwSignal::new("Scan or select an order ready for packing.".to_owned());
    let error = RwSignal::new(false);
    let retry = RwSignal::new(None::<PendingPackingCommand>);
    let refresh_order_id = RwSignal::new(None::<i64>);
    let scan = RwSignal::new(String::new());
    let source_plate = RwSignal::new(None::<String>);
    let item_identity = RwSignal::new(None::<PendingItemIdentity>);
    let removal = RwSignal::new(None::<PendingContentRemoval>);
    let completed_order_ids = RwSignal::new(Vec::<i64>::new());
    let focus_epoch = RwSignal::new(0_u64);
    let measurements = CartonMeasurementSignals {
        weight: RwSignal::new(String::new()),
        length: RwSignal::new(String::new()),
        width: RwSignal::new(String::new()),
        height: RwSignal::new(String::new()),
    };
    let toasts = use_toast_bus();
    let scan_input = NodeRef::<html::Input>::new();
    let signals = PackingSignals {
        session,
        pending,
        message,
        error,
        retry,
        refresh_order_id,
        scan,
        source_plate,
        item_identity,
        removal,
        completed_order_ids,
        focus_epoch,
        measurements,
        on_unauthorized,
        toasts,
    };

    #[cfg(target_arch = "wasm32")]
    install_packing_queue_poll(queue, signals);

    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        selected_location_id.get();
        let facility_id = selected_location(station_locations, selected_location_id)
            .map(|location| location.facility_id);
        if queue.facility_id.get_untracked() != facility_id {
            invalidate_packing_queue_request(queue);
            queue.facility_id.set(facility_id);
            queue.entries.set(Vec::new());
            queue.next_cursor.set(None);
            queue.error.set(None);
            request_packing_queue(queue, signals, false);
        }
    });

    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        focus_epoch.get();
        if let Some(input) = scan_input.get() {
            let _ = input.focus();
        }
    });

    let start_order = Callback::new(move |order_id: i64| {
        if signals.blocked() {
            return;
        }
        let order = queue
            .entries
            .get_untracked()
            .into_iter()
            .find(|order| order.order_id == order_id);
        let Some(order) = order else {
            set_error(
                signals,
                "That order is no longer available at this station.",
            );
            return;
        };
        let location = selected_location(station_locations, selected_location_id);
        let Some(location) = location else {
            set_error(
                signals,
                "Select an active packing location before starting.",
            );
            return;
        };
        if order.facility_id != location.facility_id {
            set_error(
                signals,
                "That order belongs to a different facility than the selected station.",
            );
            return;
        }
        invalidate_packing_queue_request(queue);
        begin_or_resume_order(order, location, signals);
    });

    let submit_idle = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let scanned = scan.get_untracked().trim().to_owned();
        if scanned.is_empty() {
            set_error(signals, "Scan an order number.");
            return;
        }
        let selected_facility_id = selected_location(station_locations, selected_location_id)
            .map(|location| location.facility_id);
        let order_id = queue
            .entries
            .get_untracked()
            .into_iter()
            .find(|order| {
                Some(order.facility_id) == selected_facility_id
                    && order.order_key.eq_ignore_ascii_case(&scanned)
            })
            .map(|order| order.order_id);
        let Some(order_id) = order_id else {
            set_error(signals, "No order ready for packing matches that scan.");
            return;
        };
        start_order.run(order_id);
    };

    let submit_active = Callback::new(move |_| submit_station_scan(signals));
    let retry_command = Callback::new(move |_| {
        if pending.get_untracked() {
            return;
        }
        if let Some(command) = retry.get_untracked() {
            dispatch_command(command, signals);
        } else if let Some(order_id) = refresh_order_id.get_untracked() {
            refresh_session(order_id, signals);
        }
    });
    let close_carton = Callback::new(move |_| close_current_carton(signals));
    let void_carton = Callback::new(move |_| void_current_carton(signals));
    let start_removal = Callback::new(move |selection: PendingContentRemoval| {
        if signals.blocked() {
            return;
        }
        signals.scan.set(String::new());
        signals.item_identity.set(None);
        signals.error.set(false);
        signals
            .message
            .set("Scan the carton content and original tote to reverse packing.".to_owned());
        signals.removal.set(Some(selection));
    });
    let cancel_removal = Callback::new(move |_| {
        if signals.pending.get_untracked() || signals.retry.get_untracked().is_some() {
            return;
        }
        signals.removal.set(None);
        signals.error.set(false);
        signals
            .message
            .set("Continue packing the order.".to_owned());
        signals.refocus();
    });
    let submit_removal = Callback::new(move |request: RemovePackedContentRequest| {
        let Some(selection) = signals.removal.get_untracked() else {
            return;
        };
        dispatch_command(
            PendingPackingCommand::RemoveContent {
                session_id: selection.session_id,
                carton_id: selection.carton_id,
                content_id: selection.content_id,
                request,
                idempotency_key: api::new_idempotency_key(),
            },
            signals,
        );
    });
    let change_source = Callback::new(move |_| {
        source_plate.set(None);
        item_identity.set(None);
        removal.set(None);
        scan.set(String::new());
        error.set(false);
        message.set("Scan a source tote.".to_owned());
        signals.refocus();
    });
    let next_order = Callback::new(move |_| {
        session.set(None);
        source_plate.set(None);
        item_identity.set(None);
        scan.set(String::new());
        error.set(false);
        message.set("Scan or select an order ready for packing.".to_owned());
        signals.refocus();
        request_packing_queue(queue, signals, false);
    });
    let load_more = Callback::new(move |_| request_packing_queue(queue, signals, true));

    view! {
        <section class="packing-workspace">
            <header class="packing-station-bar">
                <div class="packing-station-heading">
                    <Icon icon=UiIcon::Packing/>
                    <div>
                        <h1>"Packing station"</h1>
                        <span>{move || station_label(session.get(), station_locations)}</span>
                    </div>
                </div>
                <label class="packing-location-select">
                    <span>"Location"</span>
                    <select
                        prop:value=move || selected_location_id.get()
                        disabled=move || session.get().is_some() || signals.blocked()
                        on:change=move |event| selected_location_id.set(event_target_value(&event))
                    >
                        {station_locations
                            .get_value()
                            .into_iter()
                            .map(|location| {
                                view! { <option value=location.id.to_string()>{location.label}</option> }
                            })
                            .collect_view()}
                    </select>
                </label>
                <div class="packing-station-summary">
                    <strong>{move || packing_progress_label(session.get())}</strong>
                    <span>{move || facility_label(
                        session.get(),
                        station_locations,
                        station_facilities,
                        selected_location_id.get(),
                    )}</span>
                </div>
            </header>
            <Show
                when=move || session.get().is_some()
                fallback=move || view! {
                    <PackingIdle
                        queue
                        scan
                        scan_input
                        selected_location_id
                        station_locations
                        signals
                        start_order
                        on_retry=retry_command
                        on_load_more=load_more
                        submit_idle
                    />
                }
            >
                {move || session.get().map(|current| view! {
                    <PackingActive
                        current
                        scan
                        scan_input
                        source_plate
                        signals
                        on_submit=submit_active
                        on_retry=retry_command
                        on_change_source=change_source
                        on_close=close_carton
                        on_void=void_carton
                        on_remove=start_removal
                        on_next_order=next_order
                    />
                })}
            </Show>
            <Show when=move || removal.get().is_some()>
                {move || removal.get().map(|selection| view! {
                    <PackingRemovalDialog
                        selection
                        pending=Signal::derive(move || pending.get())
                        retrying=Signal::derive(move || retry.get().is_some())
                        command_error=Signal::derive(move || error.get().then(|| message.get()))
                        on_cancel=cancel_removal
                        on_submit=submit_removal
                        on_retry=retry_command
                    />
                })}
            </Show>
        </section>
    }
}

fn invalidate_packing_queue_request(queue: PackingQueueSignals) {
    queue
        .generation
        .update(|generation| *generation = generation.saturating_add(1));
    queue.pending.set(false);
}

fn packing_queue_is_idle(signals: PackingSignals) -> bool {
    signals.session.get_untracked().is_none()
        && !signals.pending.get_untracked()
        && signals.retry.get_untracked().is_none()
        && signals.refresh_order_id.get_untracked().is_none()
        && signals.scan.get_untracked().trim().is_empty()
        && signals.source_plate.get_untracked().is_none()
        && signals.item_identity.get_untracked().is_none()
        && signals.removal.get_untracked().is_none()
}

fn request_packing_queue(queue: PackingQueueSignals, signals: PackingSignals, append: bool) {
    if queue.pending.get_untracked() || !packing_queue_is_idle(signals) {
        return;
    }
    let facility_id = queue.facility_id.get_untracked();
    let cursor = if append {
        let Some(cursor) = queue.next_cursor.get_untracked() else {
            return;
        };
        Some(cursor)
    } else {
        None
    };
    let generation = queue.generation.get_untracked().saturating_add(1);
    queue.generation.set(generation);
    queue.pending.set(true);
    queue.error.set(None);
    leptos::task::spawn_local(async move {
        let result = api::packing_queue(facility_id, cursor.as_ref()).await;
        if queue.generation.get_untracked() != generation || !packing_queue_is_idle(signals) {
            if queue.generation.get_untracked() == generation {
                queue.pending.set(false);
            }
            return;
        }
        match result {
            Ok(page) => {
                if append {
                    queue.entries.update(|entries| {
                        for entry in page.items {
                            if !entries
                                .iter()
                                .any(|current| current.order_id == entry.order_id)
                            {
                                entries.push(entry);
                            }
                        }
                    });
                } else {
                    queue.entries.set(page.items);
                }
                queue.next_cursor.set(page.next_cursor);
                queue.pending.set(false);
            }
            Err(error) if error.unauthorized => {
                queue.pending.set(false);
                signals.on_unauthorized.run(());
            }
            Err(error) => {
                queue.pending.set(false);
                queue.error.set(Some(error.message));
            }
        }
    });
}

#[cfg(target_arch = "wasm32")]
fn install_packing_queue_poll(queue: PackingQueueSignals, signals: PackingSignals) {
    use std::time::Duration;

    let Some(owner) = Owner::current() else {
        return;
    };
    let Ok(handle) = set_interval_with_handle(
        move || {
            if packing_queue_is_idle(signals) && !queue.pending.get_untracked() {
                owner.with(|| request_packing_queue(queue, signals, false));
            }
        },
        Duration::from_secs(15),
    ) else {
        return;
    };
    on_cleanup(move || handle.clear());
}

#[derive(Clone, Copy)]
struct CartonMeasurementSignals {
    weight: RwSignal<String>,
    length: RwSignal<String>,
    width: RwSignal<String>,
    height: RwSignal<String>,
}

impl CartonMeasurementSignals {
    fn clear(self) {
        self.weight.set(String::new());
        self.length.set(String::new());
        self.width.set(String::new());
        self.height.set(String::new());
    }
}

fn begin_or_resume_order(
    order: PackingQueueEntryResponse,
    location: PackingLocation,
    signals: PackingSignals,
) {
    signals.item_identity.set(None);
    signals.pending.set(true);
    signals.error.set(false);
    signals
        .message
        .set(format!("Checking order {}...", order.order_key));
    leptos::task::spawn_local(async move {
        match api::pack_session_for_order(order.order_id).await {
            Ok(Some(current)) => {
                signals.pending.set(false);
                signals.scan.set(String::new());
                signals.session.set(Some(current));
                signals.message.set("Pack session resumed.".to_owned());
                signals.refocus();
            }
            Ok(None) => {
                signals.pending.set(false);
                dispatch_command(
                    PendingPackingCommand::Open {
                        order_id: order.order_id,
                        request: OpenPackSessionRequest {
                            facility_id: location.facility_id,
                            station_location_id: location.id,
                            expected_revision: order.revision,
                        },
                        idempotency_key: api::new_idempotency_key(),
                    },
                    signals,
                );
            }
            Err(api_error) if api_error.unauthorized => signals.on_unauthorized.run(()),
            Err(api_error) => {
                signals.pending.set(false);
                set_error(signals, api_error.message);
            }
        }
    });
}

fn submit_station_scan(signals: PackingSignals) {
    if signals.blocked() {
        return;
    }
    let scanned = signals.scan.get_untracked().trim().to_owned();
    if scanned.is_empty() {
        set_error(signals, "A scan is required.");
        return;
    }
    let Some(current) = signals.session.get_untracked() else {
        return;
    };
    if current.progress.expected_allocation_count > 0
        && current.progress.packed_allocation_count >= current.progress.expected_allocation_count
    {
        set_error(signals, "All items are packed. Close the active carton.");
        return;
    }
    let open_carton = current
        .cartons
        .iter()
        .find(|carton| matches!(carton.lifecycle, PackCartonLifecycleResponse::Open));
    let Some(carton) = open_carton else {
        dispatch_command(
            PendingPackingCommand::CreateCarton {
                session_id: current.session_id,
                request: CreateCartonRequest {
                    carton_barcode: scanned,
                    expected_revision: current.revision,
                },
                idempotency_key: api::new_idempotency_key(),
            },
            signals,
        );
        return;
    };
    let Some(source_barcode) = signals.source_plate.get_untracked() else {
        let valid_source = current.allocations.iter().any(|allocation| {
            matches!(
                allocation.disposition,
                PackAllocationDispositionResponse::Available
            ) && allocation.license_plate_barcode == scanned
        });
        if !valid_source {
            set_error(
                signals,
                "That tote has no unpacked allocation for this order.",
            );
            return;
        }
        signals.source_plate.set(Some(scanned.clone()));
        signals.item_identity.set(None);
        signals.scan.set(String::new());
        signals.error.set(false);
        signals
            .message
            .set(format!("Tote {scanned} selected. Scan an item."));
        signals.refocus();
        return;
    };
    if let Some(pending_identity) = signals.item_identity.get_untracked() {
        match advance_item_identity(&pending_identity, &scanned) {
            Ok(resolution) => {
                apply_identity_resolution(resolution, &current, carton, source_barcode, signals)
            }
            Err(identity_error) => {
                if identity_error.reset {
                    signals.item_identity.set(None);
                }
                set_error(signals, identity_error.message);
            }
        }
        return;
    }
    let candidates = matching_item_candidates(&current, &source_barcode, &scanned);
    if candidates.is_empty() {
        set_error(signals, "That item is not available in the selected tote.");
        return;
    }
    match start_item_identity(scanned, candidates) {
        Ok(resolution) => {
            apply_identity_resolution(resolution, &current, carton, source_barcode, signals)
        }
        Err(identity_error) => set_error(signals, identity_error.message),
    }
}

fn apply_identity_resolution(
    resolution: ItemIdentityResolution,
    session: &PackSessionResponse,
    carton: &wareboxes_api_contract::v1::PackCartonResponse,
    source_barcode: String,
    signals: PackingSignals,
) {
    match resolution {
        ItemIdentityResolution::Await(pending_identity) => {
            let message = match pending_identity.stage {
                IdentityScanStage::Lot => "Item selected. Scan the lot.",
                IdentityScanStage::Serial => "Identity narrowed. Scan the serial.",
            };
            signals.item_identity.set(Some(pending_identity));
            signals.scan.set(String::new());
            signals.error.set(false);
            signals.message.set(message.to_owned());
            signals.refocus();
        }
        ItemIdentityResolution::Resolved(identity) => {
            let allocation_is_current = session.allocations.iter().any(|allocation| {
                allocation.inventory_allocation_id == identity.allocation_id
                    && allocation.license_plate_barcode == source_barcode
                    && matches!(
                        allocation.disposition,
                        PackAllocationDispositionResponse::Available
                    )
            });
            if !allocation_is_current {
                signals.item_identity.set(None);
                set_error(
                    signals,
                    "That allocation is no longer available. Scan the item again.",
                );
                return;
            }
            signals.item_identity.set(None);
            dispatch_command(
                PendingPackingCommand::PackAllocation {
                    session_id: session.session_id,
                    carton_id: carton.carton_id,
                    request: PackPickedAllocationRequest {
                        inventory_allocation_id: identity.allocation_id,
                        item_barcode: identity.item_barcode,
                        lot_scan: identity.lot_scan,
                        serial_scan: identity.serial_scan,
                        source_license_plate_barcode: source_barcode,
                        carton_barcode: carton.carton_barcode.clone(),
                        expected_revision: session.revision,
                    },
                    idempotency_key: api::new_idempotency_key(),
                },
                signals,
            );
        }
    }
}

fn close_current_carton(signals: PackingSignals) {
    if signals.blocked() {
        return;
    }
    if signals.item_identity.get_untracked().is_some() {
        set_error(
            signals,
            "Finish the lot or serial scan, or change tote before closing the carton.",
        );
        return;
    }
    let Some(current) = signals.session.get_untracked() else {
        return;
    };
    let Some(carton) = current
        .cartons
        .iter()
        .find(|carton| matches!(carton.lifecycle, PackCartonLifecycleResponse::Open))
    else {
        set_error(signals, "There is no open carton to close.");
        return;
    };
    let measurements = match parse_measurements(signals.measurements) {
        Ok(value) => value,
        Err(message) => {
            set_error(signals, message);
            return;
        }
    };
    dispatch_command(
        PendingPackingCommand::CloseCarton {
            session_id: current.session_id,
            carton_id: carton.carton_id,
            request: CloseCartonRequest {
                carton_barcode: carton.carton_barcode.clone(),
                measurements,
                expected_revision: current.revision,
            },
            idempotency_key: api::new_idempotency_key(),
        },
        signals,
    );
}

fn void_current_carton(signals: PackingSignals) {
    if signals.blocked() {
        return;
    }
    if signals.item_identity.get_untracked().is_some() {
        set_error(
            signals,
            "Finish the lot or serial scan, or change tote before voiding the carton.",
        );
        return;
    }
    let Some(current) = signals.session.get_untracked() else {
        return;
    };
    let Some(carton) = current
        .cartons
        .iter()
        .find(|carton| matches!(carton.lifecycle, PackCartonLifecycleResponse::Open))
    else {
        set_error(signals, "There is no open carton to void.");
        return;
    };
    if carton.content_count != 0 {
        set_error(signals, "Only an empty carton can be voided.");
        return;
    }
    dispatch_command(
        PendingPackingCommand::VoidCarton {
            session_id: current.session_id,
            carton_id: carton.carton_id,
            request: VoidCartonRequest {
                carton_barcode: carton.carton_barcode.clone(),
                expected_revision: current.revision,
            },
            idempotency_key: api::new_idempotency_key(),
        },
        signals,
    );
}

fn dispatch_command(command: PendingPackingCommand, signals: PackingSignals) {
    if signals.pending.get_untracked() {
        return;
    }
    signals.pending.set(true);
    signals.error.set(false);
    signals.retry.set(Some(command.clone()));
    signals.message.set(command.pending_message().to_owned());
    leptos::task::spawn_local(async move {
        match execute_command(&command).await {
            Ok(result) => apply_command_result(result, signals),
            Err(api_error) if api_error.unauthorized => {
                signals.pending.set(false);
                signals.retry.set(None);
                signals.on_unauthorized.run(());
            }
            Err(api_error) => {
                signals.pending.set(false);
                signals.error.set(true);
                if api_error.ambiguous_outcome {
                    signals.message.set(format!(
                        "{} The result is unknown; retry the saved command.",
                        api_error.message
                    ));
                } else {
                    signals.retry.set(None);
                    signals.message.set(api_error.message.clone());
                    signals.refocus();
                }
                signals.toasts.error(api_error.message);
            }
        }
    });
}

fn apply_command_result(result: PackingCommandResult, signals: PackingSignals) {
    signals.pending.set(false);
    signals.retry.set(None);
    signals.error.set(false);
    signals.scan.set(String::new());
    signals.item_identity.set(None);
    match result {
        PackingCommandResult::Opened(current) => {
            let current = *current;
            signals
                .message
                .set("Pack session opened. Scan a carton.".to_owned());
            signals
                .toasts
                .success(format!("Packing started for {}.", current.order_key));
            signals.session.set(Some(current));
            signals.refocus();
        }
        PackingCommandResult::Created {
            order_id,
            carton_barcode,
        } => {
            signals.source_plate.set(None);
            signals.measurements.clear();
            signals
                .message
                .set(format!("Carton {carton_barcode} opened."));
            signals
                .toasts
                .success(format!("Carton {carton_barcode} opened."));
            refresh_session(order_id, signals);
        }
        PackingCommandResult::Packed {
            order_id,
            quantity,
            uom,
        } => {
            signals
                .message
                .set(format!("Packed {} {uom}.", format_quantity(quantity)));
            refresh_session(order_id, signals);
        }
        PackingCommandResult::Removed {
            order_id,
            quantity,
            uom,
            destination_tote_barcode,
        } => {
            signals.removal.set(None);
            signals
                .source_plate
                .set(Some(destination_tote_barcode.clone()));
            signals.message.set(format!(
                "Returned {} {uom} to tote {destination_tote_barcode}. Scan the item to repack it.",
                format_quantity(quantity)
            ));
            signals.toasts.success(format!(
                "Returned {} {uom} to tote {destination_tote_barcode}.",
                format_quantity(quantity)
            ));
            refresh_session(order_id, signals);
        }
        PackingCommandResult::Closed {
            order_id,
            carton_barcode,
            ready,
        } => {
            signals.source_plate.set(None);
            signals.measurements.clear();
            if ready {
                signals.completed_order_ids.update(|ids| {
                    if !ids.contains(&order_id) {
                        ids.push(order_id)
                    }
                });
                signals.message.set(format!(
                    "Carton {carton_barcode} closed. Order is ready to ship."
                ));
                signals.toasts.success(format!(
                    "Carton {carton_barcode} closed; order is ready to ship."
                ));
                refresh_session(order_id, signals);
            } else {
                signals.message.set(format!(
                    "Carton {carton_barcode} closed. Scan the next carton."
                ));
                signals
                    .toasts
                    .success(format!("Carton {carton_barcode} closed."));
                refresh_session(order_id, signals);
            }
        }
        PackingCommandResult::Voided {
            order_id,
            carton_barcode,
        } => {
            signals.source_plate.set(None);
            signals.measurements.clear();
            signals.message.set(format!(
                "Carton {carton_barcode} voided. Scan a new carton."
            ));
            signals
                .toasts
                .success(format!("Empty carton {carton_barcode} voided."));
            refresh_session(order_id, signals);
        }
    }
}

fn refresh_session(order_id: i64, signals: PackingSignals) {
    let recovering = signals.refresh_order_id.get_untracked().is_some();
    signals.item_identity.set(None);
    signals.pending.set(true);
    signals.error.set(false);
    leptos::task::spawn_local(async move {
        match api::pack_session_for_order(order_id).await {
            Ok(Some(current)) => {
                let all_allocations_packed = current.progress.expected_allocation_count > 0
                    && current.progress.packed_allocation_count
                        >= current.progress.expected_allocation_count;
                let selected_source_complete = signals
                    .source_plate
                    .get_untracked()
                    .as_deref()
                    .is_some_and(|source_barcode| {
                        !current.allocations.iter().any(|allocation| {
                            matches!(
                                allocation.disposition,
                                PackAllocationDispositionResponse::Available
                            ) && allocation.license_plate_barcode == source_barcode
                        })
                    });
                signals.pending.set(false);
                signals.refresh_order_id.set(None);
                if all_allocations_packed {
                    signals.source_plate.set(None);
                    signals.message.set(
                        "All items packed. Enter measurements, then close the carton.".to_owned(),
                    );
                } else if selected_source_complete {
                    signals.source_plate.set(None);
                    signals
                        .message
                        .set("Source tote complete. Scan the next source tote.".to_owned());
                }
                signals.session.set(Some(current));
                if recovering && !all_allocations_packed && !selected_source_complete {
                    signals.message.set("Pack session refreshed.".to_owned());
                }
                signals.refocus();
            }
            Ok(None) => {
                signals.pending.set(false);
                signals.refresh_order_id.set(Some(order_id));
                set_error(signals, "The committed pack session could not be reloaded.");
            }
            Err(api_error) if api_error.unauthorized => signals.on_unauthorized.run(()),
            Err(api_error) => {
                signals.pending.set(false);
                signals.refresh_order_id.set(Some(order_id));
                set_error(
                    signals,
                    format!(
                        "Command committed, but the station could not refresh: {}",
                        api_error.message
                    ),
                );
            }
        }
    });
}

fn set_error(signals: PackingSignals, message: impl Into<String>) {
    signals.error.set(true);
    signals.message.set(message.into());
    signals.scan.set(String::new());
    signals.refocus();
}

fn parse_measurements(signals: CartonMeasurementSignals) -> Result<CartonMeasurements, String> {
    let weight = optional_positive(&signals.weight.get_untracked(), "weight")?
        .map(WeightGrams::new)
        .transpose()
        .map_err(|error| error.to_string())?;
    let dimensions = [
        signals.length.get_untracked(),
        signals.width.get_untracked(),
        signals.height.get_untracked(),
    ];
    let populated = dimensions
        .iter()
        .filter(|value| !value.trim().is_empty())
        .count();
    let dimensions = if populated == 0 {
        None
    } else if populated != 3 {
        return Err("Enter all three carton dimensions or leave all three blank.".to_owned());
    } else {
        let length_mm = DimensionMillimeters::new(required_positive(&dimensions[0], "length")?)
            .map_err(|error| error.to_string())?;
        let width_mm = DimensionMillimeters::new(required_positive(&dimensions[1], "width")?)
            .map_err(|error| error.to_string())?;
        let height_mm = DimensionMillimeters::new(required_positive(&dimensions[2], "height")?)
            .map_err(|error| error.to_string())?;
        Some(CartonDimensions {
            length_mm,
            width_mm,
            height_mm,
        })
    };
    Ok(CartonMeasurements {
        weight_grams: weight,
        dimensions,
    })
}

fn optional_positive(value: &str, label: &str) -> Result<Option<i64>, String> {
    if value.trim().is_empty() {
        Ok(None)
    } else {
        required_positive(value, label).map(Some)
    }
}

fn required_positive(value: &str, label: &str) -> Result<i64, String> {
    value
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("Enter a positive whole-number {label}."))
}

#[cfg(test)]
mod tests {
    use super::required_positive;

    #[test]
    fn measurements_accept_only_positive_whole_numbers() {
        assert_eq!(required_positive(" 1250 ", "weight"), Ok(1250));
        assert!(required_positive("0", "weight").is_err());
        assert!(required_positive("1.5", "weight").is_err());
    }
}
