use chrono::{DateTime, Utc};
use wareboxes_api_contract::v1::{SupportAccessResponse, SupportAccessStatus};

pub(super) const fn status_label(status: SupportAccessStatus) -> &'static str {
    match status {
        SupportAccessStatus::Pending => "Pending approval",
        SupportAccessStatus::Active => "Active",
        SupportAccessStatus::Rejected => "Rejected",
        SupportAccessStatus::Revoked => "Revoked",
        SupportAccessStatus::Expired => "Expired",
    }
}

pub(super) const fn status_class(status: SupportAccessStatus) -> &'static str {
    match status {
        SupportAccessStatus::Pending => "support-status pending",
        SupportAccessStatus::Active => "support-status active",
        SupportAccessStatus::Rejected => "support-status rejected",
        SupportAccessStatus::Revoked => "support-status revoked",
        SupportAccessStatus::Expired => "support-status expired",
    }
}

pub(super) fn short_timestamp(value: &str) -> String {
    DateTime::parse_from_rfc3339(value)
        .map(|value| {
            value
                .with_timezone(&Utc)
                .format("%Y-%m-%d %H:%M UTC")
                .to_string()
        })
        .unwrap_or_else(|_| value.to_owned())
}

pub(super) fn scope_summary(value: &SupportAccessResponse) -> String {
    let facilities = if value.access.all_facilities {
        "all facilities".to_owned()
    } else {
        format!("{} facilities", value.access.facility_ids.len())
    };
    let owners = if value.access.all_inventory_owners {
        "all clients".to_owned()
    } else {
        format!("{} clients", value.access.inventory_owner_ids.len())
    };
    format!("{facilities} · {owners}")
}

pub(super) fn permission_summary(value: &SupportAccessResponse) -> String {
    value.access.permission_names.join(", ")
}

pub(super) fn action_label(action: &str) -> String {
    match action {
        "requested" => "Access requested".to_owned(),
        "approved" => "Access approved".to_owned(),
        "rejected" => "Request rejected".to_owned(),
        "revoked" => "Access revoked".to_owned(),
        _ => action.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_has_an_operator_label() {
        for status in [
            SupportAccessStatus::Pending,
            SupportAccessStatus::Active,
            SupportAccessStatus::Rejected,
            SupportAccessStatus::Revoked,
            SupportAccessStatus::Expired,
        ] {
            assert!(!status_label(status).is_empty());
            assert!(status_class(status).starts_with("support-status "));
        }
    }
}
