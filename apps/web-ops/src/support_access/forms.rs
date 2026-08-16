use chrono::{Duration, Utc};
use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    ApproveSupportAccessRequest, RejectSupportAccessRequest, RequestSupportAccessRequest,
    RevokeSupportAccessRequest, SupportAccessPolicyRequest, SupportAccessResponse,
};

use super::{dispatch, load_options, load_tenants, Dialog, PendingCommand, Signals};
use crate::api;

#[derive(Clone, Copy)]
pub(super) struct Drafts {
    tenant_id: RwSignal<Option<i64>>,
    reason: RwSignal<String>,
    duration_hours: RwSignal<i64>,
    all_facilities: RwSignal<bool>,
    facility_ids: RwSignal<Vec<i64>>,
    all_owners: RwSignal<bool>,
    owner_ids: RwSignal<Vec<i64>>,
    permissions: RwSignal<Vec<String>>,
    transition_reason: RwSignal<String>,
}

impl Drafts {
    pub(super) fn new() -> Self {
        Self {
            tenant_id: RwSignal::new(None),
            reason: RwSignal::new(String::new()),
            duration_hours: RwSignal::new(1),
            all_facilities: RwSignal::new(false),
            facility_ids: RwSignal::new(Vec::new()),
            all_owners: RwSignal::new(false),
            owner_ids: RwSignal::new(Vec::new()),
            permissions: RwSignal::new(Vec::new()),
            transition_reason: RwSignal::new(String::new()),
        }
    }

    pub(super) fn reset_request(self) {
        self.tenant_id.set(None);
        self.reason.set(String::new());
        self.duration_hours.set(1);
        self.all_facilities.set(false);
        self.facility_ids.set(Vec::new());
        self.all_owners.set(false);
        self.owner_ids.set(Vec::new());
        self.permissions.set(Vec::new());
    }

    fn reset_transition(self) {
        self.transition_reason.set(String::new());
    }
}

pub(super) fn dialog(signals: Signals, drafts: Drafts, value: Dialog) -> AnyView {
    let title = match value {
        Dialog::Request => "Request support access",
        Dialog::Approve(_) => "Approve support access",
        Dialog::Reject(_) => "Reject support access",
        Dialog::Revoke(_) => "Revoke support access",
    };
    if !matches!(value, Dialog::Request) {
        drafts.reset_transition();
    }
    view! {
        <div class="support-access-dialog-backdrop" role="presentation"><section class="support-access-dialog" role="dialog" aria-modal="true" aria-label=title><header><div><p class="eyebrow">"Platform security"</p><h2>{title}</h2></div><button class="text-button" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Close"</button></header>{match value { Dialog::Request=>request_form(signals,drafts), Dialog::Approve(grant)=>approve_form(signals,*grant), Dialog::Reject(grant)=>reason_form(signals,drafts,*grant,false), Dialog::Revoke(grant)=>reason_form(signals,drafts,*grant,true) }}</section></div>
    }.into_any()
}

fn request_form(signals: Signals, drafts: Drafts) -> AnyView {
    let select_tenant = move |event| {
        let tenant_id = event_target_value(&event).parse::<i64>().ok();
        drafts.tenant_id.set(tenant_id);
        drafts.all_facilities.set(false);
        drafts.facility_ids.set(Vec::new());
        drafts.all_owners.set(false);
        drafts.owner_ids.set(Vec::new());
        drafts.permissions.set(Vec::new());
        signals.options.set(None);
        if let Some(tenant_id) = tenant_id {
            load_options(signals, tenant_id);
        }
    };
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let Some(tenant_id) = drafts.tenant_id.get_untracked() else {
            signals
                .command_error
                .set(Some("Select a target tenant.".into()));
            return;
        };
        let reason = drafts.reason.get_untracked().trim().to_owned();
        if reason.is_empty() {
            signals
                .command_error
                .set(Some("Enter the incident or diagnostic reason.".into()));
            return;
        }
        let all_facilities = drafts.all_facilities.get_untracked();
        let facility_ids = drafts.facility_ids.get_untracked();
        let all_inventory_owners = drafts.all_owners.get_untracked();
        let inventory_owner_ids = drafts.owner_ids.get_untracked();
        let permission_names = drafts.permissions.get_untracked();
        if (!all_facilities && facility_ids.is_empty())
            || (!all_inventory_owners && inventory_owner_ids.is_empty())
            || permission_names.is_empty()
        {
            signals.command_error.set(Some(
                "Select explicit facility, client, and permission scope (or choose an all-scope option)."
                    .into(),
            ));
            return;
        }
        dispatch(
            signals,
            PendingCommand::Request(
                RequestSupportAccessRequest {
                    tenant_id,
                    reason,
                    expires_at: (Utc::now()
                        + Duration::hours(drafts.duration_hours.get_untracked()))
                    .to_rfc3339(),
                    access: SupportAccessPolicyRequest {
                        all_facilities,
                        facility_ids,
                        all_inventory_owners,
                        inventory_owner_ids,
                        permission_names,
                    },
                },
                api::new_idempotency_key(),
            ),
        );
    };
    view! { <form on:submit=submit><section class="support-access-warning"><strong>"A different platform administrator must approve this request."</strong><span>"Access is read-only, cannot include tenant administration, and ends at the exact expiration timestamp even if no cleanup worker runs."</span></section><div class="support-access-form-grid"><label><span>"Target tenant"</span><select required prop:value=move || drafts.tenant_id.get().map_or_else(String::new,|value|value.to_string()) on:change=select_tenant><option value="">"Select tenant"</option>{move || signals.tenants.get().items.into_iter().map(|tenant|view!{<option value=tenant.tenant_id.to_string()>{format!("{} ({})",tenant.name,tenant.slug)}</option>}).collect_view()}</select></label><label><span>"Duration"</span><select prop:value=move || drafts.duration_hours.get().to_string() on:change=move |event| if let Ok(value)=event_target_value(&event).parse(){drafts.duration_hours.set(value)}><option value="1">"1 hour"</option><option value="2">"2 hours"</option><option value="4">"4 hours"</option><option value="8">"8 hours (maximum)"</option></select></label><label class="reason-field"><span>"Incident / diagnostic reason"</span><textarea required maxlength="500" rows="3" placeholder="Incident ID and specific diagnostic purpose" prop:value=move || drafts.reason.get() on:input=move |event| drafts.reason.set(event_target_value(&event))></textarea></label></div>{move || scope_picker(signals,drafts)}{feedback(signals)}<footer><div>{move || signals.tenants.get().next_cursor.map(|cursor|view!{<button class="text-button" type="button" disabled=move || signals.tenant_loading.get() on:click=move |_| load_tenants(signals,Some(cursor.clone()),true)>"Load more tenants"</button>})}</div><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Cancel"</button><button class="button primary-action" type="submit" disabled=move || signals.command_pending.get() || signals.options_loading.get()>"Submit for approval"</button></footer></form> }.into_any()
}

fn scope_picker(signals: Signals, drafts: Drafts) -> AnyView {
    if signals.options_loading.get() {
        return view! { <section class="support-scope-state" aria-busy="true"><span class="loading-line"></span><strong>"Loading target scope"</strong></section> }.into_any();
    }
    let Some(options) = signals.options.get() else {
        return view! { <section class="support-scope-state"><strong>"Select a tenant to load its delegable scope."</strong></section> }.into_any();
    };
    view! { <div class="support-scope-grid"><fieldset><legend>"Facility scope"</legend><label class="scope-all"><input type="checkbox" prop:checked=move || drafts.all_facilities.get() on:change=move |event|{let checked=event_target_checked(&event);drafts.all_facilities.set(checked);if checked{drafts.facility_ids.set(Vec::new());}}/><span>"All active facilities"</span></label><div class="scope-checks">{options.facilities.into_iter().map(|option|check_id(option.id,option.name,drafts.facility_ids,drafts.all_facilities)).collect_view()}</div></fieldset><fieldset><legend>"Client scope"</legend><label class="scope-all"><input type="checkbox" prop:checked=move || drafts.all_owners.get() on:change=move |event|{let checked=event_target_checked(&event);drafts.all_owners.set(checked);if checked{drafts.owner_ids.set(Vec::new());}}/><span>"All active clients"</span></label><div class="scope-checks">{options.inventory_owners.into_iter().map(|option|check_id(option.id,option.name,drafts.owner_ids,drafts.all_owners)).collect_view()}</div></fieldset><fieldset><legend>"Permission scope"</legend><p>"Exact tenant permissions; admin is never available."</p><div class="scope-checks">{options.permission_names.into_iter().map(|name|check_string(name,drafts.permissions)).collect_view()}</div></fieldset></div> }.into_any()
}

fn approve_form(signals: Signals, grant: SupportAccessResponse) -> AnyView {
    let grant_for_submit = grant.clone();
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        dispatch(
            signals,
            PendingCommand::Approve(
                grant_for_submit.support_access_grant_id,
                ApproveSupportAccessRequest {
                    expected_revision: grant_for_submit.revision,
                },
                api::new_idempotency_key(),
            ),
        );
    };
    view! { <form on:submit=submit><section class="support-access-warning danger"><strong>"Approval activates access immediately."</strong><span>"You must be different from the requester. Confirm the tenant, exact scope, permissions, reason, and expiration before approving."</span></section>{grant_summary(&grant)}{feedback(signals)}<footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Cancel"</button><button class="button primary-action" type="submit" disabled=move || signals.command_pending.get()>"Approve exact scope"</button></footer></form> }.into_any()
}

fn reason_form(
    signals: Signals,
    drafts: Drafts,
    grant: SupportAccessResponse,
    revoking: bool,
) -> AnyView {
    let grant_for_submit = grant.clone();
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let reason = drafts.transition_reason.get_untracked().trim().to_owned();
        if reason.is_empty() {
            signals
                .command_error
                .set(Some("Enter an attributed reason.".into()));
            return;
        }
        let key = api::new_idempotency_key();
        let command = if revoking {
            PendingCommand::Revoke(
                grant_for_submit.support_access_grant_id,
                RevokeSupportAccessRequest {
                    expected_revision: grant_for_submit.revision,
                    reason,
                },
                key,
            )
        } else {
            PendingCommand::Reject(
                grant_for_submit.support_access_grant_id,
                RejectSupportAccessRequest {
                    expected_revision: grant_for_submit.revision,
                    reason,
                },
                key,
            )
        };
        dispatch(signals, command);
    };
    view! { <form on:submit=submit><section class=if revoking{"support-access-warning danger"}else{"support-access-warning"}><strong>{if revoking{"Revocation fails closed immediately."}else{"Rejection is final for this request."}}</strong><span>{if revoking{"The support identity loses tenant visibility and every delegated permission on the next authorization check."}else{"The requester must submit a new request if access is still needed."}}</span></section>{grant_summary(&grant)}<label><span>"Attributed reason"</span><textarea required maxlength="500" rows="3" prop:value=move || drafts.transition_reason.get() on:input=move |event| drafts.transition_reason.set(event_target_value(&event))></textarea></label>{feedback(signals)}<footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Cancel"</button><button class=if revoking{"button danger-action"}else{"button primary-action"} type="submit" disabled=move || signals.command_pending.get()>{if revoking{"Revoke now"}else{"Reject request"}}</button></footer></form> }.into_any()
}

fn grant_summary(grant: &SupportAccessResponse) -> AnyView {
    view! { <dl class="support-confirm"><div><dt>"Tenant"</dt><dd>{grant.tenant_name.clone()}</dd></div><div><dt>"Requester"</dt><dd>{grant.requested_by_email.clone()}</dd></div><div><dt>"Facilities"</dt><dd>{if grant.access.all_facilities{"All".into()}else{format!("{:?}",grant.access.facility_ids)}}</dd></div><div><dt>"Clients"</dt><dd>{if grant.access.all_inventory_owners{"All".into()}else{format!("{:?}",grant.access.inventory_owner_ids)}}</dd></div><div><dt>"Permissions"</dt><dd>{grant.access.permission_names.join(", ")}</dd></div><div><dt>"Expires"</dt><dd>{super::display::short_timestamp(&grant.expires_at)}</dd></div><div><dt>"Reason"</dt><dd>{grant.reason.clone()}</dd></div></dl> }.into_any()
}

fn check_id(id: i64, label: String, selected: RwSignal<Vec<i64>>, all: RwSignal<bool>) -> AnyView {
    view! { <label><input type="checkbox" disabled=move || all.get() prop:checked=move || selected.get().contains(&id) on:change=move |event| selected.update(|ids|set_membership(ids,id,event_target_checked(&event)))/><span>{label}</span></label> }.into_any()
}

fn check_string(value: String, selected: RwSignal<Vec<String>>) -> AnyView {
    let checked_value = value.clone();
    let change_value = value.clone();
    view! { <label><input type="checkbox" prop:checked=move || selected.get().contains(&checked_value) on:change=move |event|selected.update(|values|set_membership(values,change_value.clone(),event_target_checked(&event)))/><span>{value}</span></label> }.into_any()
}

fn set_membership<T: PartialEq>(values: &mut Vec<T>, value: T, checked: bool) {
    if checked {
        if !values.contains(&value) {
            values.push(value);
        }
    } else {
        values.retain(|current| *current != value);
    }
}

fn feedback(signals: Signals) -> AnyView {
    view! { <>{move || signals.command_error.get().map(|message|view!{<p class="inline-command-error" role="alert">{message}</p>})}</> }.into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_membership_is_unique_and_reversible() {
        let mut values = vec![1];
        set_membership(&mut values, 1, true);
        assert_eq!(values, vec![1]);
        set_membership(&mut values, 2, true);
        assert_eq!(values, vec![1, 2]);
        set_membership(&mut values, 1, false);
        assert_eq!(values, vec![2]);
    }
}
