use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    ChangeTenantStatusRequest, CreateTenantRequest, DataCellPage, TenantLifecycleResponse,
    TenantStatus,
};

use super::{dispatch, load_cells, Dialog, PendingCommand, Signals};
use crate::api;

#[derive(Clone, Copy)]
pub(super) struct Drafts {
    slug: RwSignal<String>,
    name: RwSignal<String>,
    administrator_email: RwSignal<String>,
    data_cell_id: RwSignal<String>,
    residency_requirement: RwSignal<String>,
    reason: RwSignal<String>,
}

impl Drafts {
    pub(super) fn new() -> Self {
        Self {
            slug: RwSignal::new(String::new()),
            name: RwSignal::new(String::new()),
            administrator_email: RwSignal::new(String::new()),
            data_cell_id: RwSignal::new(String::new()),
            residency_requirement: RwSignal::new(String::new()),
            reason: RwSignal::new(String::new()),
        }
    }

    pub(super) fn reset_create(self, cells: &DataCellPage) {
        self.slug.set(String::new());
        self.name.set(String::new());
        self.administrator_email.set(String::new());
        let selected = cells
            .items
            .iter()
            .find(|cell| cell.available_tenant_slots > 0);
        self.data_cell_id.set(
            selected
                .map(|cell| cell.data_cell_id.to_string())
                .unwrap_or_default(),
        );
        self.residency_requirement.set(
            selected
                .map(|cell| cell.residency.clone())
                .unwrap_or_default(),
        );
    }

    pub(super) fn reset_reason(self) {
        self.reason.set(String::new());
    }
}

pub(super) fn dialog(signals: Signals, drafts: Drafts, value: Dialog) -> AnyView {
    let title = match &value {
        Dialog::Create => "Provision tenant",
        Dialog::Status(tenant) if tenant.status == TenantStatus::Active => "Suspend tenant",
        Dialog::Status(_) => "Reactivate tenant",
    };
    view! {
        <div class="tenant-lifecycle-dialog-backdrop" role="presentation">
            <section class="tenant-lifecycle-dialog" role="dialog" aria-modal="true" aria-label=title>
                <header><div><p class="eyebrow">"Platform control"</p><h2>{title}</h2></div><button class="text-button" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Close"</button></header>
                {match value {
                    Dialog::Create => create_form(signals, drafts),
                    Dialog::Status(tenant) => status_form(signals, drafts, *tenant),
                }}
            </section>
        </div>
    }.into_any()
}

fn create_form(signals: Signals, drafts: Drafts) -> AnyView {
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let slug = drafts.slug.get_untracked().trim().to_owned();
        let name = drafts.name.get_untracked().trim().to_owned();
        let administrator_email = drafts
            .administrator_email
            .get_untracked()
            .trim()
            .to_ascii_lowercase();
        let data_cell_id = drafts.data_cell_id.get_untracked().parse::<i64>().ok();
        let residency_requirement = drafts
            .residency_requirement
            .get_untracked()
            .trim()
            .to_ascii_uppercase();
        if slug.is_empty()
            || name.is_empty()
            || administrator_email.is_empty()
            || data_cell_id.is_none()
            || residency_requirement.is_empty()
        {
            signals.command_error.set(Some(
                "Enter identity, administrator, data cell, and residency.".into(),
            ));
            return;
        }
        dispatch(
            signals,
            PendingCommand::Create(
                CreateTenantRequest {
                    slug,
                    name,
                    administrator_email,
                    data_cell_id: data_cell_id.unwrap_or_default(),
                    residency_requirement,
                },
                api::new_idempotency_key(),
            ),
        );
    };
    view! {
        <form on:submit=submit>
            <div class="tenant-lifecycle-form-grid">
                <label><span>"Tenant slug"</span><input required minlength="3" maxlength="63" pattern="[a-z0-9][a-z0-9-]*[a-z0-9]" placeholder="northwest-3pl" prop:value=move || drafts.slug.get() on:input=move |event| drafts.slug.set(event_target_value(&event).to_ascii_lowercase())/><small>"Permanent URL-safe identity; lowercase letters, digits, and hyphens."</small></label>
                <label><span>"Organization name"</span><input required maxlength="200" prop:value=move || drafts.name.get() on:input=move |event| drafts.name.set(event_target_value(&event))/></label>
                <label><span>"Initial administrator email"</span><input type="email" required maxlength="254" prop:value=move || drafts.administrator_email.get() on:input=move |event| drafts.administrator_email.set(event_target_value(&event))/><small>"Must belong to an existing interactive Wareboxes user. Tenant admin scope is provisioned atomically."</small></label>
                <label><span>"Home data cell"</span>{move || cell_selector(signals,drafts)}</label>
                <label><span>"Residency requirement"</span><input required maxlength="16" pattern="[A-Z0-9][A-Z0-9-]*[A-Z0-9]" prop:value=move || drafts.residency_requirement.get() on:input=move |event| drafts.residency_requirement.set(event_target_value(&event).to_ascii_uppercase())/><small>"GLOBAL permits any jurisdiction; a regional code must exactly match the selected cell."</small></label>
            </div>
            <section class="tenant-lifecycle-warning"><strong>"Provisioning creates a placed hard tenant boundary."</strong><span>"The cell capacity and residency constraint are locked with tenant creation. The selected administrator receives tenant authority; no password or infrastructure credential is accepted here."</span></section>
            {feedback(signals)}
            <footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Cancel"</button><button class="button primary-action" type="submit" disabled=move || signals.command_pending.get()>"Provision tenant"</button></footer>
        </form>
    }.into_any()
}

fn cell_selector(signals: Signals, drafts: Drafts) -> AnyView {
    let page = signals.cells.get();
    let next_cursor = page.next_cursor.clone();
    view! {
        <select required prop:value=move || drafts.data_cell_id.get() on:change=move |event| {
            let selected=event_target_value(&event);
            drafts.data_cell_id.set(selected.clone());
            if let Ok(id)=selected.parse::<i64>() {
                if let Some(cell)=signals.cells.get_untracked().items.into_iter().find(|cell|cell.data_cell_id==id) {
                    drafts.residency_requirement.set(cell.residency);
                }
            }
        }>{page.items.into_iter().filter(|cell|cell.available_tenant_slots>0).map(|cell|view!{<option value=cell.data_cell_id.to_string()>{format!("{} · {} · {} · {} open",cell.name,cell.region,cell.residency,cell.available_tenant_slots)}</option>}).collect_view()}</select>
        {next_cursor.map(|cursor| view! { <button class="text-button" type="button" disabled=move || signals.cells_loading.get() on:click=move |_| load_cells(signals,Some(cursor.clone()),true)>{move || if signals.cells_loading.get() { "Loading cells…" } else { "Load more active cells" }}</button> })}
    }.into_any()
}

fn status_form(signals: Signals, drafts: Drafts, tenant: TenantLifecycleResponse) -> AnyView {
    let suspending = tenant.status == TenantStatus::Active;
    let next_status = if suspending {
        TenantStatus::Suspended
    } else {
        TenantStatus::Active
    };
    let tenant_for_submit = tenant.clone();
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
                tenant_for_submit.tenant_id,
                ChangeTenantStatusRequest {
                    expected_revision: tenant_for_submit.revision,
                    status: next_status,
                    reason,
                },
                api::new_idempotency_key(),
            ),
        );
    };
    view! {
        <form on:submit=submit>
            <section class=if suspending { "tenant-lifecycle-warning danger" } else { "tenant-lifecycle-warning" }>
                <strong>{if suspending { "Suspension immediately ends access." } else { "Reactivation does not restore credentials." }}</strong>
                <span>{if suspending { "All member sessions and active service-account credentials are revoked atomically. In-flight requests fail closed when their tenant context is revalidated." } else { "Members can sign in again, but every integration must receive a newly issued credential." }}</span>
            </section>
            <dl class="tenant-lifecycle-confirm"><div><dt>"Tenant"</dt><dd>{tenant.name}</dd></div><div><dt>"Slug"</dt><dd>{tenant.slug}</dd></div><div><dt>"Current revision"</dt><dd>{tenant.revision.get()}</dd></div></dl>
            <label><span>"Attributed reason"</span><textarea required maxlength="500" rows="4" prop:value=move || drafts.reason.get() on:input=move |event| drafts.reason.set(event_target_value(&event))></textarea></label>
            {feedback(signals)}
            <footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Cancel"</button><button class=if suspending { "button danger-action" } else { "button primary-action" } type="submit" disabled=move || signals.command_pending.get()>{if suspending { "Suspend and revoke access" } else { "Reactivate tenant" }}</button></footer>
        </form>
    }.into_any()
}

fn feedback(signals: Signals) -> AnyView {
    view! { <>{move || signals.command_error.get().map(|message| view! { <p class="inline-command-error" role="alert">{message}</p> })}</> }.into_any()
}
