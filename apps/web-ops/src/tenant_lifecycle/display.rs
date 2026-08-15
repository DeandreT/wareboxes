use wareboxes_api_contract::v1::{TenantLifecycleResponse, TenantStatus};

pub(super) const fn status_label(status: TenantStatus) -> &'static str {
    match status {
        TenantStatus::Active => "Active",
        TenantStatus::Suspended => "Suspended",
    }
}

pub(super) const fn status_class(status: TenantStatus) -> &'static str {
    match status {
        TenantStatus::Active => "status-badge success",
        TenantStatus::Suspended => "status-badge danger",
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

pub(super) fn footprint(tenant: &TenantLifecycleResponse) -> String {
    format!(
        "{} members · {} facilities · {} clients · {} integrations",
        tenant.active_member_count,
        tenant.active_facility_count,
        tenant.active_inventory_owner_count,
        tenant.active_service_account_count
    )
}

pub(super) fn action_label(value: &str) -> String {
    value.replace('_', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_labels_distinguish_suspension() {
        assert_eq!(status_label(TenantStatus::Suspended), "Suspended");
        assert_eq!(action_label("status_changed"), "status changed");
    }
}
