use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    ChangeDataCellStatusRequest, DataCellMode, DataCellResponse, DataCellStatus,
    ReconfigureDataCellRequest, RegisterDataCellRequest,
};

use super::{dispatch, Dialog, PendingCommand, Signals};
use crate::api;

#[derive(Clone, Copy)]
pub(super) struct Drafts {
    key: RwSignal<String>,
    name: RwSignal<String>,
    region: RwSignal<String>,
    residency: RwSignal<String>,
    mode: RwSignal<DataCellMode>,
    capacity: RwSignal<String>,
    reason: RwSignal<String>,
}

impl Drafts {
    pub(super) fn new() -> Self {
        Self {
            key: RwSignal::new(String::new()),
            name: RwSignal::new(String::new()),
            region: RwSignal::new(String::new()),
            residency: RwSignal::new(String::new()),
            mode: RwSignal::new(DataCellMode::Shared),
            capacity: RwSignal::new("100".into()),
            reason: RwSignal::new(String::new()),
        }
    }

    pub(super) fn reset_register(self) {
        self.key.set(String::new());
        self.name.set(String::new());
        self.region.set(String::new());
        self.residency.set(String::new());
        self.mode.set(DataCellMode::Shared);
        self.capacity.set("100".into());
        self.reason.set(String::new());
    }

    pub(super) fn reset_reconfigure(self, cell: &DataCellResponse) {
        self.name.set(cell.name.clone());
        self.capacity.set(cell.max_tenants.to_string());
        self.reason.set(String::new());
    }

    pub(super) fn reset_reason(self) {
        self.reason.set(String::new());
    }
}

pub(super) fn dialog(signals: Signals, drafts: Drafts, value: Dialog) -> AnyView {
    let title = match &value {
        Dialog::Register => "Register data cell",
        Dialog::Reconfigure(_) => "Reconfigure data cell",
        Dialog::Status(_, DataCellStatus::Active) => "Activate data cell",
        Dialog::Status(_, DataCellStatus::Draining) => "Begin draining",
        Dialog::Status(_, DataCellStatus::Retired) => "Retire data cell",
        Dialog::Status(_, DataCellStatus::Provisioning) => "Change data-cell status",
    };
    view! {
        <div class="fleet-cell-dialog-backdrop" role="presentation">
            <section class="fleet-cell-dialog" role="dialog" aria-modal="true" aria-label=title>
                <header><div><p class="eyebrow">"Platform control"</p><h2>{title}</h2></div><button class="text-button" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Close"</button></header>
                {match value {
                    Dialog::Register => register_form(signals,drafts),
                    Dialog::Reconfigure(cell) => reconfigure_form(signals,drafts,*cell),
                    Dialog::Status(cell,status) => status_form(signals,drafts,*cell,status),
                }}
            </section>
        </div>
    }.into_any()
}

fn register_form(signals: Signals, drafts: Drafts) -> AnyView {
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let capacity = match drafts.capacity.get_untracked().parse::<u32>() {
            Ok(value) if value > 0 => value,
            _ => {
                signals
                    .command_error
                    .set(Some("Enter a positive tenant capacity.".into()));
                return;
            }
        };
        let mode = drafts.mode.get_untracked();
        if mode == DataCellMode::Dedicated && capacity != 1 {
            signals
                .command_error
                .set(Some("A dedicated cell must have capacity 1.".into()));
            return;
        }
        dispatch(
            signals,
            PendingCommand::Register(
                RegisterDataCellRequest {
                    key: drafts.key.get_untracked().trim().to_ascii_lowercase(),
                    name: drafts.name.get_untracked().trim().to_owned(),
                    region: drafts.region.get_untracked().trim().to_ascii_lowercase(),
                    residency: drafts.residency.get_untracked().trim().to_ascii_uppercase(),
                    mode,
                    max_tenants: capacity,
                },
                api::new_idempotency_key(),
            ),
        );
    };
    view! {
        <form on:submit=submit>
            <div class="fleet-cell-form-grid">
                <label><span>"Permanent cell key"</span><input required minlength="3" maxlength="63" pattern="[a-z0-9][a-z0-9-]*[a-z0-9]" placeholder="us-west-2-a" prop:value=move || drafts.key.get() on:input=move |event| drafts.key.set(event_target_value(&event).to_ascii_lowercase())/></label>
                <label><span>"Display name"</span><input required maxlength="200" prop:value=move || drafts.name.get() on:input=move |event| drafts.name.set(event_target_value(&event))/></label>
                <label><span>"Region"</span><input required maxlength="32" pattern="[a-z0-9][a-z0-9-]*[a-z0-9]" placeholder="us-west-2" prop:value=move || drafts.region.get() on:input=move |event| drafts.region.set(event_target_value(&event).to_ascii_lowercase())/></label>
                <label><span>"Residency jurisdiction"</span><input required maxlength="16" pattern="[A-Z0-9][A-Z0-9-]*[A-Z0-9]" placeholder="US" prop:value=move || drafts.residency.get() on:input=move |event| drafts.residency.set(event_target_value(&event).to_ascii_uppercase())/></label>
                <label><span>"Isolation mode"</span><select prop:value=move || mode_wire(drafts.mode.get()) on:change=move |event| { let mode=parse_mode(&event_target_value(&event)); drafts.mode.set(mode); if mode==DataCellMode::Dedicated { drafts.capacity.set("1".into()); } }><option value="shared">"Shared"</option><option value="dedicated">"Dedicated"</option></select></label>
                <label><span>"Tenant capacity"</span><input type="number" min="1" max="1000000" required disabled=move || drafts.mode.get()==DataCellMode::Dedicated prop:value=move || drafts.capacity.get() on:input=move |event| drafts.capacity.set(event_target_value(&event))/></label>
            </div>
            <section class="fleet-cell-warning"><strong>"Registration does not expose infrastructure secrets."</strong><span>"Endpoints and credentials remain in the deployment secret plane. Activate this cell only after readiness, recovery, and monitoring checks pass."</span></section>
            {feedback(signals)}
            <footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Cancel"</button><button class="button primary-action" type="submit" disabled=move || signals.command_pending.get()>"Register provisioning cell"</button></footer>
        </form>
    }.into_any()
}

fn reconfigure_form(signals: Signals, drafts: Drafts, cell: DataCellResponse) -> AnyView {
    let selected = cell.clone();
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let capacity = match drafts.capacity.get_untracked().parse::<u32>() {
            Ok(value) if value > 0 => value,
            _ => {
                signals
                    .command_error
                    .set(Some("Enter a positive tenant capacity.".into()));
                return;
            }
        };
        let reason = drafts.reason.get_untracked().trim().to_owned();
        if reason.is_empty() {
            signals
                .command_error
                .set(Some("Enter an attributed reason.".into()));
            return;
        }
        dispatch(
            signals,
            PendingCommand::Reconfigure(
                selected.data_cell_id,
                ReconfigureDataCellRequest {
                    expected_revision: selected.revision,
                    name: drafts.name.get_untracked().trim().to_owned(),
                    max_tenants: capacity,
                    reason,
                },
                api::new_idempotency_key(),
            ),
        );
    };
    view! {
        <form on:submit=submit>
            <dl class="fleet-cell-confirm"><div><dt>"Cell"</dt><dd>{cell.key}</dd></div><div><dt>"Current placements"</dt><dd>{cell.placement_count}</dd></div><div><dt>"Revision"</dt><dd>{cell.revision.get()}</dd></div></dl>
            <div class="fleet-cell-form-grid"><label><span>"Display name"</span><input required maxlength="200" prop:value=move || drafts.name.get() on:input=move |event| drafts.name.set(event_target_value(&event))/></label><label><span>"Tenant capacity"</span><input type="number" min=cell.placement_count.max(1) max="1000000" required disabled=cell.mode==DataCellMode::Dedicated prop:value=move || drafts.capacity.get() on:input=move |event| drafts.capacity.set(event_target_value(&event))/></label></div>
            <label><span>"Attributed reason"</span><textarea required maxlength="500" rows="4" prop:value=move || drafts.reason.get() on:input=move |event| drafts.reason.set(event_target_value(&event))></textarea></label>
            {feedback(signals)}
            <footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Cancel"</button><button class="button primary-action" type="submit" disabled=move || signals.command_pending.get()>"Save revision"</button></footer>
        </form>
    }.into_any()
}

fn status_form(
    signals: Signals,
    drafts: Drafts,
    cell: DataCellResponse,
    status: DataCellStatus,
) -> AnyView {
    let selected = cell.clone();
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let reason = drafts.reason.get_untracked().trim().to_owned();
        if reason.is_empty() {
            signals
                .command_error
                .set(Some("Enter an attributed reason.".into()));
            return;
        }
        dispatch(
            signals,
            PendingCommand::Status(
                selected.data_cell_id,
                ChangeDataCellStatusRequest {
                    expected_revision: selected.revision,
                    status,
                    reason,
                },
                api::new_idempotency_key(),
            ),
        );
    };
    let danger = status == DataCellStatus::Draining || status == DataCellStatus::Retired;
    view! { <form on:submit=submit><section class=if danger {"fleet-cell-warning danger"}else{"fleet-cell-warning"}><strong>{status_warning(status)}</strong><span>{status_explanation(status)}</span></section><dl class="fleet-cell-confirm"><div><dt>"Cell"</dt><dd>{cell.key}</dd></div><div><dt>"Placements"</dt><dd>{cell.placement_count}</dd></div><div><dt>"Revision"</dt><dd>{cell.revision.get()}</dd></div></dl><label><span>"Attributed reason"</span><textarea required maxlength="500" rows="4" prop:value=move || drafts.reason.get() on:input=move |event| drafts.reason.set(event_target_value(&event))></textarea></label>{feedback(signals)}<footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Cancel"</button><button class=if danger {"button danger-action"}else{"button primary-action"} type="submit" disabled=move || signals.command_pending.get()>{status_button(status)}</button></footer></form> }.into_any()
}

fn feedback(signals: Signals) -> AnyView {
    view! { <>{move || signals.command_error.get().map(|message| view! { <p class="inline-command-error" role="alert">{message}</p> })}</> }.into_any()
}
const fn mode_wire(mode: DataCellMode) -> &'static str {
    match mode {
        DataCellMode::Shared => "shared",
        DataCellMode::Dedicated => "dedicated",
    }
}
fn parse_mode(value: &str) -> DataCellMode {
    if value == "dedicated" {
        DataCellMode::Dedicated
    } else {
        DataCellMode::Shared
    }
}
const fn status_warning(status: DataCellStatus) -> &'static str {
    match status {
        DataCellStatus::Active => "Activation opens the cell for placement.",
        DataCellStatus::Draining => "Draining blocks every new tenant placement.",
        DataCellStatus::Retired => "Retirement is terminal and requires an empty cell.",
        DataCellStatus::Provisioning => "Unsupported transition.",
    }
}
const fn status_explanation(status: DataCellStatus) -> &'static str {
    match status{DataCellStatus::Active=>"Confirm infrastructure readiness, restore coverage, monitoring, and residency controls before activation.",DataCellStatus::Draining=>"Existing tenants remain operational; move them through the governed movement workflow before retirement.",DataCellStatus::Retired=>"The registry retains identity and immutable evidence, but the cell can never accept another tenant.",DataCellStatus::Provisioning=>"Return to the data-cell detail and choose a valid transition."}
}
const fn status_button(status: DataCellStatus) -> &'static str {
    match status {
        DataCellStatus::Active => "Activate cell",
        DataCellStatus::Draining => "Begin draining",
        DataCellStatus::Retired => "Retire empty cell",
        DataCellStatus::Provisioning => "Change status",
    }
}
