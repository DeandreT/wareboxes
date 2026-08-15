use leptos::prelude::*;
use wareboxes_api_contract::v1::{ServiceAccountAccessRequest, ServiceAccountStatus};
use wareboxes_api_contract::web::access::{AccessScopeResource, AccessScopeWorkspace};

pub(super) const fn status_label(status: ServiceAccountStatus) -> &'static str {
    match status {
        ServiceAccountStatus::Active => "Active",
        ServiceAccountStatus::Disabled => "Disabled",
    }
}

pub(super) const fn status_class(status: ServiceAccountStatus) -> &'static str {
    match status {
        ServiceAccountStatus::Active => "status-badge success",
        ServiceAccountStatus::Disabled => "status-badge danger",
    }
}

pub(super) fn short_timestamp(value: &str) -> String {
    value
        .replace('T', " ")
        .trim_end_matches('Z')
        .chars()
        .take(19)
        .collect()
}

pub(super) fn scope_summary(access: &ServiceAccountAccessRequest) -> String {
    let facilities = if access.all_facilities {
        "all facilities".into()
    } else {
        format!("{} facilities", access.facility_ids.len())
    };
    let owners = if access.all_inventory_owners {
        "all clients".into()
    } else {
        format!("{} clients", access.inventory_owner_ids.len())
    };
    format!("{facilities} · {owners}")
}

pub(super) fn scope_names(resources: &[AccessScopeResource], ids: &[i64], all: bool) -> AnyView {
    if all {
        return view! {<span class="scope-pill">"All tenant scope"</span>}.into_any();
    }
    ids.iter()
        .map(|id| {
            let label = resources
                .iter()
                .find(|value| value.id == *id)
                .map(|value| value.name.clone())
                .unwrap_or_else(|| format!("#{}", id));
            view! {<span class="scope-pill">{label}</span>}
        })
        .collect_view()
        .into_any()
}

pub(super) fn account_scope(
    access: &AccessScopeWorkspace,
    policy: &ServiceAccountAccessRequest,
) -> AnyView {
    view! { <div class="service-account-scope-summary"><div><strong>"Facilities"</strong><span>{scope_names(&access.facilities,&policy.facility_ids,policy.all_facilities)}</span></div><div><strong>"Clients"</strong><span>{scope_names(&access.inventory_owners,&policy.inventory_owner_ids,policy.all_inventory_owners)}</span></div><div><strong>"Permissions"</strong><span>{policy.permission_names.iter().map(|name|view!{<span class="scope-pill permission">{name.clone()}</span>}).collect_view()}</span></div></div> }.into_any()
}

pub(super) fn event_label(action: &str) -> String {
    action.replace('_', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_account_labels_preserve_scope_meaning() {
        let exact = ServiceAccountAccessRequest {
            all_facilities: false,
            facility_ids: vec![1, 2],
            all_inventory_owners: false,
            inventory_owner_ids: vec![3],
            permission_names: vec!["orders".into()],
        };
        assert_eq!(scope_summary(&exact), "2 facilities · 1 clients");
        assert_eq!(status_label(ServiceAccountStatus::Disabled), "Disabled");
        assert_eq!(event_label("credential_revoked"), "credential revoked");
    }
}
