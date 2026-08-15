use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    ChangeTenantStatusRequest, CreateTenantRequest, TenantLifecycleResponse, TenantStatus,
};

use super::{dispatch, Dialog, PendingCommand, Signals};
use crate::api;

#[derive(Clone, Copy)]
pub(super) struct Drafts {
    slug: RwSignal<String>,
    name: RwSignal<String>,
    administrator_email: RwSignal<String>,
    reason: RwSignal<String>,
}

impl Drafts {
    pub(super) fn new() -> Self {
        Self {
            slug: RwSignal::new(String::new()),
            name: RwSignal::new(String::new()),
            administrator_email: RwSignal::new(String::new()),
            reason: RwSignal::new(String::new()),
        }
    }

    pub(super) fn reset_create(self) {
        self.slug.set(String::new());
        self.name.set(String::new());
        self.administrator_email.set(String::new());
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
        if slug.is_empty() || name.is_empty() || administrator_email.is_empty() {
            signals.command_error.set(Some(
                "Enter the slug, name, and existing administrator email.".into(),
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
            </div>
            <section class="tenant-lifecycle-warning"><strong>"Provisioning creates a hard tenant boundary."</strong><span>"The selected administrator receives all facility and client scope plus the tenant admin permission. No password or credential is accepted here."</span></section>
            {feedback(signals)}
            <footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Cancel"</button><button class="button primary-action" type="submit" disabled=move || signals.command_pending.get()>"Provision tenant"</button></footer>
        </form>
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
