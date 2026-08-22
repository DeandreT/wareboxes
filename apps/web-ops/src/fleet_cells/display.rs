use wareboxes_api_contract::v1::{DataCellMode, DataCellResponse, DataCellStatus};

pub(super) const fn status_label(status: DataCellStatus) -> &'static str {
    match status {
        DataCellStatus::Provisioning => "Provisioning",
        DataCellStatus::Active => "Active",
        DataCellStatus::Draining => "Draining",
        DataCellStatus::Retired => "Retired",
    }
}

pub(super) const fn status_class(status: DataCellStatus) -> &'static str {
    match status {
        DataCellStatus::Provisioning => "status-badge info",
        DataCellStatus::Active => "status-badge success",
        DataCellStatus::Draining => "status-badge warning",
        DataCellStatus::Retired => "status-badge neutral",
    }
}

pub(super) const fn mode_label(mode: DataCellMode) -> &'static str {
    match mode {
        DataCellMode::Shared => "Shared",
        DataCellMode::Dedicated => "Dedicated",
    }
}

pub(super) fn capacity(cell: &DataCellResponse) -> String {
    let reservations = cell.reserved_inbound_move_count + cell.reserved_rollback_move_count;
    format!(
        "{} / {} tenants · {} reserved · {} open",
        cell.placement_count, cell.max_tenants, reservations, cell.available_tenant_slots
    )
}

pub(super) fn short_timestamp(value: &str) -> String {
    value
        .replace('T', " ")
        .trim_end_matches('Z')
        .chars()
        .take(19)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_keep_draining_visually_distinct() {
        assert_eq!(status_label(DataCellStatus::Draining), "Draining");
        assert_eq!(mode_label(DataCellMode::Dedicated), "Dedicated");
    }
}
