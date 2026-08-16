use wareboxes_api_contract::v1::{PickCartStatus, PickClusterStatus};

pub(super) const fn cart_status_label(status: PickCartStatus) -> &'static str {
    match status {
        PickCartStatus::Active => "Active",
        PickCartStatus::OutOfService => "Out of service",
        PickCartStatus::Retired => "Retired",
    }
}

pub(super) const fn cluster_status_label(status: PickClusterStatus) -> &'static str {
    match status {
        PickClusterStatus::Planned => "Planned",
        PickClusterStatus::InProgress => "In progress",
        PickClusterStatus::Completed => "Completed",
        PickClusterStatus::Cancelled => "Cancelled",
    }
}

pub(super) const fn next_cart_status(status: PickCartStatus) -> Option<PickCartStatus> {
    match status {
        PickCartStatus::Active => Some(PickCartStatus::OutOfService),
        PickCartStatus::OutOfService => Some(PickCartStatus::Active),
        PickCartStatus::Retired => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_carts_have_no_operational_toggle() {
        assert_eq!(next_cart_status(PickCartStatus::Retired), None);
        assert_eq!(
            next_cart_status(PickCartStatus::OutOfService),
            Some(PickCartStatus::Active)
        );
    }
}
