use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    ChangeServiceAccountStatusRequest, CreateServiceAccountRequest,
    IssueServiceAccountCredentialRequest, RevokeServiceAccountCredentialRequest,
    ServiceAccountAccessRequest, ServiceAccountResponse, ServiceAccountStatus,
    UpdateServiceAccountAccessRequest,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;

use super::{dispatch, Dialog, PendingCommand, Signals};
use crate::api;

#[derive(Clone, Copy)]
pub(super) struct Drafts {
    name: RwSignal<String>,
    description: RwSignal<String>,
    all_facilities: RwSignal<bool>,
    facility_ids: RwSignal<Vec<i64>>,
    all_owners: RwSignal<bool>,
    owner_ids: RwSignal<Vec<i64>>,
    permissions: RwSignal<Vec<String>>,
    credential_label: RwSignal<String>,
    credential_expiry: RwSignal<String>,
    reason: RwSignal<String>,
}

impl Drafts {
    pub(super) fn new() -> Self {
        Self {
            name: RwSignal::new(String::new()),
            description: RwSignal::new(String::new()),
            all_facilities: RwSignal::new(false),
            facility_ids: RwSignal::new(Vec::new()),
            all_owners: RwSignal::new(false),
            owner_ids: RwSignal::new(Vec::new()),
            permissions: RwSignal::new(Vec::new()),
            credential_label: RwSignal::new(String::new()),
            credential_expiry: RwSignal::new(String::new()),
            reason: RwSignal::new(String::new()),
        }
    }

    pub(super) fn reset_create(self) {
        self.name.set(String::new());
        self.description.set(String::new());
        self.reset_access(None);
    }

    pub(super) fn reset_access(self, account: Option<&ServiceAccountResponse>) {
        let access = account.map(|value| &value.access);
        self.all_facilities
            .set(access.is_some_and(|value| value.all_facilities));
        self.facility_ids.set(
            access
                .map(|value| value.facility_ids.clone())
                .unwrap_or_default(),
        );
        self.all_owners
            .set(access.is_some_and(|value| value.all_inventory_owners));
        self.owner_ids.set(
            access
                .map(|value| value.inventory_owner_ids.clone())
                .unwrap_or_default(),
        );
        self.permissions.set(
            access
                .map(|value| value.permission_names.clone())
                .unwrap_or_default(),
        );
    }

    pub(super) fn reset_credential(self) {
        self.credential_label.set(String::new());
        self.credential_expiry.set(String::new());
    }

    pub(super) fn reset_reason(self) {
        self.reason.set(String::new());
    }
}

pub(super) fn dialog(
    signals: Signals,
    drafts: Drafts,
    access: StoredValue<AccessScopeWorkspace>,
    value: Dialog,
) -> AnyView {
    let title = match &value {
        Dialog::Create => "Create service account",
        Dialog::Access(_) => "Update access",
        Dialog::Issue(_) => "Issue credential",
        Dialog::Status(account) if account.status == ServiceAccountStatus::Active => {
            "Disable service account"
        }
        Dialog::Status(_) => "Enable service account",
        Dialog::Revoke(_, _) => "Revoke credential",
    };
    view! {
        <div class="service-account-dialog-backdrop" role="presentation">
            <section class="service-account-dialog" role="dialog" aria-modal="true" aria-label=title>
                <header><div><p class="eyebrow">"Integration identity"</p><h2>{title}</h2></div><button class="text-button" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Close"</button></header>
                {match value {
                    Dialog::Create => access_form(signals, drafts, access, None),
                    Dialog::Access(account) => access_form(signals, drafts, access, Some(account)),
                    Dialog::Issue(account) => credential_form(signals, drafts, account),
                    Dialog::Status(account) => status_form(signals, drafts, account),
                    Dialog::Revoke(account, credential_id) => revoke_form(signals, drafts, account, credential_id),
                }}
            </section>
        </div>
    }.into_any()
}

fn access_form(
    signals: Signals,
    drafts: Drafts,
    access: StoredValue<AccessScopeWorkspace>,
    current: Option<ServiceAccountResponse>,
) -> AnyView {
    let editing = current.is_some();
    let current_for_submit = current.clone();
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let request = (|| {
            let policy = access_request(drafts, access)?;
            if let Some(account) = current_for_submit.as_ref() {
                Ok(PendingCommand::Access(
                    account.service_account_id,
                    UpdateServiceAccountAccessRequest {
                        expected_revision: account.revision,
                        access: policy,
                    },
                    api::new_idempotency_key(),
                ))
            } else {
                let name = drafts.name.get_untracked().trim().to_owned();
                if name.is_empty() {
                    return Err("Enter a service-account name.".to_owned());
                }
                Ok(PendingCommand::Create(
                    CreateServiceAccountRequest {
                        name,
                        description: nonblank(drafts.description.get_untracked()),
                        access: policy,
                    },
                    api::new_idempotency_key(),
                ))
            }
        })();
        match request {
            Ok(command) => dispatch(signals, command),
            Err(message) => signals.command_error.set(Some(message)),
        }
    };
    view! {
        <form on:submit=submit>
            {(!editing).then(|| view! { <div class="service-account-form-grid"><label><span>"Name"</span><input required maxlength="120" prop:value=move || drafts.name.get() on:input=move |event| drafts.name.set(event_target_value(&event))/></label><label><span>"Description"</span><input maxlength="500" prop:value=move || drafts.description.get() on:input=move |event| drafts.description.set(event_target_value(&event))/></label></div> })}
            <div class="service-account-scope-grid">
                <fieldset><legend>"Facility scope"</legend><Show when=move || signals.can_delegate_all_facilities.get()><label class="scope-all"><input type="checkbox" prop:checked=move || drafts.all_facilities.get() on:change=move |event| { let checked=event_target_checked(&event); drafts.all_facilities.set(checked); if checked { drafts.facility_ids.set(Vec::new()); } }/><span>"All tenant facilities"</span></label></Show><div class="scope-checks">{access.with_value(|value| value.facilities.clone()).into_iter().map(|resource| check_id(resource.id, resource.name, drafts.facility_ids, drafts.all_facilities)).collect_view()}</div></fieldset>
                <fieldset><legend>"Client scope"</legend><Show when=move || signals.can_delegate_all_owners.get()><label class="scope-all"><input type="checkbox" prop:checked=move || drafts.all_owners.get() on:change=move |event| { let checked=event_target_checked(&event); drafts.all_owners.set(checked); if checked { drafts.owner_ids.set(Vec::new()); } }/><span>"All tenant clients"</span></label></Show><div class="scope-checks">{access.with_value(|value| value.inventory_owners.clone()).into_iter().map(|resource| check_id(resource.id, resource.name, drafts.owner_ids, drafts.all_owners)).collect_view()}</div></fieldset>
                <fieldset><legend>"Integration permissions"</legend><div class="scope-checks">{move || signals.options.get().into_iter().map(|name| { let label=name.replace('_', " "); check_text(name, label, drafts.permissions) }).collect_view()}</div></fieldset>
            </div>
            <p class="service-account-help">"Scopes are intersected at request time. The admin permission is never assignable to a service account."</p>
            {feedback(signals)}
            <footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Cancel"</button><button class="button primary-action" type="submit" disabled=move || signals.command_pending.get()>{if editing { "Save access" } else { "Create account" }}</button></footer>
        </form>
    }.into_any()
}

fn credential_form(signals: Signals, drafts: Drafts, account: ServiceAccountResponse) -> AnyView {
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let label = drafts.credential_label.get_untracked().trim().to_owned();
        if label.is_empty() {
            signals
                .command_error
                .set(Some("Enter a credential label.".into()));
            return;
        }
        let bearer_token = api::generate_service_account_bearer();
        if bearer_token.is_empty() {
            signals.command_error.set(Some(
                "Credential generation is available after the page loads in your browser.".into(),
            ));
            return;
        }
        let request = IssueServiceAccountCredentialRequest {
            expected_revision: account.revision,
            label,
            expires_at: nonblank(drafts.credential_expiry.get_untracked()),
            bearer_token: bearer_token.clone(),
        };
        dispatch(
            signals,
            PendingCommand::Issue(
                account.service_account_id,
                request,
                api::new_idempotency_key(),
            ),
        );
    };
    view! { <form on:submit=submit><div class="service-account-form-grid"><label><span>"Credential label"</span><input required maxlength="120" prop:value=move || drafts.credential_label.get() on:input=move |event| drafts.credential_label.set(event_target_value(&event))/></label><label><span>"Expiry (optional RFC 3339)"</span><input placeholder="2027-08-15T12:00:00Z" prop:value=move || drafts.credential_expiry.get() on:input=move |event| drafts.credential_expiry.set(event_target_value(&event))/></label></div><div class="secret-warning"><strong>"One-time secret"</strong><span>"The browser generates the bearer token. Only its SHA-256 hash reaches storage; copy the token after issuance because it cannot be recovered."</span></div>{feedback(signals)}<footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Cancel"</button><button class="button primary-action" type="submit" disabled=move || signals.command_pending.get()>"Generate and issue"</button></footer></form> }.into_any()
}

fn status_form(signals: Signals, drafts: Drafts, account: ServiceAccountResponse) -> AnyView {
    let disabling = account.status == ServiceAccountStatus::Active;
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let reason = drafts.reason.get_untracked().trim().to_owned();
        if reason.is_empty() {
            signals
                .command_error
                .set(Some("Enter an attributed status-change reason.".into()));
            return;
        }
        dispatch(
            signals,
            PendingCommand::Status(
                account.service_account_id,
                ChangeServiceAccountStatusRequest {
                    expected_revision: account.revision,
                    status: if disabling {
                        ServiceAccountStatus::Disabled
                    } else {
                        ServiceAccountStatus::Active
                    },
                    reason,
                },
                api::new_idempotency_key(),
            ),
        );
    };
    view! { <form on:submit=submit>{disabling.then(||view!{<div class="secret-warning danger"><strong>"All active credentials will be revoked"</strong><span>"Re-enabling the account will not restore them. Issue a new credential after reactivation."</span></div>})}<label><span>"Reason"</span><textarea required maxlength="500" prop:value=move || drafts.reason.get() on:input=move |event| drafts.reason.set(event_target_value(&event))></textarea></label>{feedback(signals)}<footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Cancel"</button><button class=if disabling { "button danger-action" } else { "button primary-action" } type="submit" disabled=move || signals.command_pending.get()>{if disabling { "Disable and revoke" } else { "Enable account" }}</button></footer></form> }.into_any()
}

fn revoke_form(
    signals: Signals,
    drafts: Drafts,
    account: ServiceAccountResponse,
    credential_id: i64,
) -> AnyView {
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let reason = drafts.reason.get_untracked().trim().to_owned();
        if reason.is_empty() {
            signals
                .command_error
                .set(Some("Enter an attributed revocation reason.".into()));
            return;
        }
        dispatch(
            signals,
            PendingCommand::Revoke(
                account.service_account_id,
                credential_id,
                RevokeServiceAccountCredentialRequest {
                    expected_revision: account.revision,
                    reason,
                },
                api::new_idempotency_key(),
            ),
        );
    };
    view! { <form on:submit=submit><div class="secret-warning danger"><strong>"Revocation is immediate"</strong><span>"Requests using this credential will fail authentication. Revocation evidence is immutable."</span></div><label><span>"Reason"</span><textarea required maxlength="500" prop:value=move || drafts.reason.get() on:input=move |event| drafts.reason.set(event_target_value(&event))></textarea></label>{feedback(signals)}<footer><button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Cancel"</button><button class="button danger-action" type="submit" disabled=move || signals.command_pending.get()>"Revoke credential"</button></footer></form> }.into_any()
}

fn access_request(
    drafts: Drafts,
    access: StoredValue<AccessScopeWorkspace>,
) -> Result<ServiceAccountAccessRequest, String> {
    let mut facility_ids = drafts.facility_ids.get_untracked();
    facility_ids.sort_unstable();
    let mut inventory_owner_ids = drafts.owner_ids.get_untracked();
    inventory_owner_ids.sort_unstable();
    let mut permission_names = drafts.permissions.get_untracked();
    permission_names.sort();
    if !drafts.all_facilities.get_untracked() && facility_ids.is_empty() {
        return Err("Select at least one facility or all authorized facilities.".into());
    }
    if !drafts.all_owners.get_untracked() && inventory_owner_ids.is_empty() {
        return Err("Select at least one client or all authorized clients.".into());
    }
    if permission_names.is_empty() {
        return Err("Select at least one integration permission.".into());
    }
    if !drafts.all_facilities.get_untracked() && !drafts.all_owners.get_untracked() {
        let valid = access.with_value(|value| {
            explicit_owner_facility_pairs_are_valid(value, &facility_ids, &inventory_owner_ids)
        });
        if !valid {
            return Err(
                "Every selected client must operate in at least one selected facility.".into(),
            );
        }
    }
    Ok(ServiceAccountAccessRequest {
        all_facilities: drafts.all_facilities.get_untracked(),
        facility_ids,
        all_inventory_owners: drafts.all_owners.get_untracked(),
        inventory_owner_ids,
        permission_names,
    })
}

fn check_id(id: i64, label: String, selected: RwSignal<Vec<i64>>, all: RwSignal<bool>) -> AnyView {
    view! { <label><input type="checkbox" disabled=move || all.get() prop:checked=move || selected.get().contains(&id) on:change=move |event| selected.update(|ids| set_membership(ids,id,event_target_checked(&event)))/><span>{label}</span></label> }.into_any()
}

fn check_text(value: String, label: String, selected: RwSignal<Vec<String>>) -> AnyView {
    let checked_value = value.clone();
    let change_value = value;
    view! { <label><input type="checkbox" prop:checked=move || selected.get().contains(&checked_value) on:change=move |event| selected.update(|values| set_membership(values,change_value.clone(),event_target_checked(&event)))/><span>{label}</span></label> }.into_any()
}

fn set_membership<T: PartialEq>(values: &mut Vec<T>, value: T, checked: bool) {
    if checked && !values.contains(&value) {
        values.push(value);
    } else if !checked {
        values.retain(|candidate| candidate != &value);
    }
}

fn nonblank(value: String) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.trim().to_owned())
}

fn explicit_owner_facility_pairs_are_valid(
    access: &AccessScopeWorkspace,
    facility_ids: &[i64],
    owner_ids: &[i64],
) -> bool {
    owner_ids.iter().all(|owner_id| {
        facility_ids.iter().any(|facility_id| {
            access.owner_facilities.iter().any(|link| {
                link.inventory_owner_id == *owner_id && link.facility_id == *facility_id
            })
        })
    })
}

fn feedback(signals: Signals) -> AnyView {
    view! { <Show when=move || signals.command_error.get().is_some()><p class="inline-command-error" role="alert">{move || signals.command_error.get().unwrap_or_default()}</p></Show> }.into_any()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::web::access::AccessOwnerFacility;

    #[test]
    fn explicit_client_scope_requires_a_selected_facility_link() {
        let access = AccessScopeWorkspace {
            owner_facilities: vec![
                AccessOwnerFacility {
                    inventory_owner_id: 7,
                    facility_id: 3,
                },
                AccessOwnerFacility {
                    inventory_owner_id: 8,
                    facility_id: 4,
                },
            ],
            ..AccessScopeWorkspace::default()
        };
        assert!(explicit_owner_facility_pairs_are_valid(
            &access,
            &[3, 4],
            &[7, 8]
        ));
        assert!(!explicit_owner_facility_pairs_are_valid(
            &access,
            &[3],
            &[7, 8]
        ));
    }

    #[test]
    fn scope_membership_never_duplicates_values() {
        let mut values = vec![3];
        set_membership(&mut values, 3, true);
        set_membership(&mut values, 4, true);
        set_membership(&mut values, 3, false);
        assert_eq!(values, vec![4]);
    }
}
