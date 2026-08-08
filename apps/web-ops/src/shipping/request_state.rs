#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ShipmentRequestToken {
    pub(super) generation: u64,
    pub(super) order_id: i64,
    pub(super) shipment_id: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ShipmentVersion {
    pub(super) shipment_id: i64,
    pub(super) shipment_revision: i64,
    pub(super) order_revision: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QueueRefreshAction {
    Keep,
    ClearSelection,
    ClearShipment,
    Load(ShipmentVersion),
}

pub(super) fn shipment_request_is_current(
    token: ShipmentRequestToken,
    current_generation: u64,
    selected_order_id: Option<i64>,
) -> bool {
    token.generation == current_generation && selected_order_id == Some(token.order_id)
}

pub(super) fn queue_response_is_current(
    request_generation: u64,
    request_facility_id: Option<i64>,
    current_generation: u64,
    current_facility_id: Option<i64>,
) -> bool {
    request_generation == current_generation && request_facility_id == current_facility_id
}

pub(super) fn queue_refresh_action(
    has_selection: bool,
    entry_exists: bool,
    queued: Option<ShipmentVersion>,
    current: Option<ShipmentVersion>,
) -> QueueRefreshAction {
    if !has_selection {
        return QueueRefreshAction::Keep;
    }
    if !entry_exists {
        return QueueRefreshAction::ClearSelection;
    }
    match (queued, current) {
        (Some(queued), Some(current)) if queued == current => QueueRefreshAction::Keep,
        (Some(queued), _) => QueueRefreshAction::Load(queued),
        (None, Some(_)) => QueueRefreshAction::ClearShipment,
        (None, None) => QueueRefreshAction::Keep,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(shipment_id: i64, shipment_revision: i64) -> ShipmentVersion {
        ShipmentVersion {
            shipment_id,
            shipment_revision,
            order_revision: 12,
        }
    }

    #[test]
    fn stale_shipment_response_is_rejected_after_a_new_selection() {
        let first = ShipmentRequestToken {
            generation: 4,
            order_id: 10,
            shipment_id: 20,
        };
        assert!(shipment_request_is_current(first, 4, Some(10)));
        assert!(!shipment_request_is_current(first, 5, Some(11)));
        assert!(!shipment_request_is_current(first, 4, None));
    }

    #[test]
    fn facility_filter_change_invalidates_an_in_flight_queue_response() {
        assert!(queue_response_is_current(7, Some(3), 7, Some(3)));
        assert!(!queue_response_is_current(7, Some(3), 8, Some(4)));
        assert!(!queue_response_is_current(7, Some(3), 7, Some(4)));
    }

    #[test]
    fn definitive_conflict_recovery_reloads_or_clears_authoritative_state() {
        assert_eq!(
            queue_refresh_action(true, true, Some(version(20, 1)), None),
            QueueRefreshAction::Load(version(20, 1)),
        );
        assert_eq!(
            queue_refresh_action(true, true, Some(version(20, 2)), Some(version(20, 1)),),
            QueueRefreshAction::Load(version(20, 2)),
        );
        assert_eq!(
            queue_refresh_action(true, false, None, Some(version(20, 2))),
            QueueRefreshAction::ClearSelection,
        );
        assert_eq!(
            queue_refresh_action(true, true, Some(version(20, 2)), Some(version(20, 2)),),
            QueueRefreshAction::Keep,
        );
    }
}
