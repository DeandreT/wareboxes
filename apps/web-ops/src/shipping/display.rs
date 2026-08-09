use wareboxes_api_contract::v1::ShipmentStatus;

pub(super) fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

pub(super) const fn shipment_status_label(status: ShipmentStatus) -> &'static str {
    match status {
        ShipmentStatus::AwaitingManifest => "Awaiting manifest",
        ShipmentStatus::Manifested => "Manifested",
        ShipmentStatus::PartiallyDeparted => "Partially departed",
        ShipmentStatus::Departed => "Departed",
        ShipmentStatus::Cancelled => "Cancelled",
    }
}

pub(super) fn departure_action_label(count: usize) -> String {
    match count {
        0 => "Depart scanned cartons".to_owned(),
        1 => "Depart 1 carton".to_owned(),
        _ => format!("Depart {count} cartons"),
    }
}

pub(super) fn dimensions_label(
    length: Option<i64>,
    width: Option<i64>,
    height: Option<i64>,
) -> String {
    match (length, width, height) {
        (Some(length), Some(width), Some(height)) => format!("{length}×{width}×{height} mm"),
        _ => "—".to_owned(),
    }
}

pub(super) fn compact_timestamp(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_manifest_service_is_trimmed() {
        assert_eq!(optional_text("  GROUND  ").as_deref(), Some("GROUND"));
        assert_eq!(optional_text("  "), None);
    }

    #[test]
    fn partial_departure_labels_are_explicit() {
        assert_eq!(
            shipment_status_label(ShipmentStatus::PartiallyDeparted),
            "Partially departed"
        );
        assert_eq!(departure_action_label(0), "Depart scanned cartons");
        assert_eq!(departure_action_label(1), "Depart 1 carton");
        assert_eq!(departure_action_label(2), "Depart 2 cartons");
    }

    #[test]
    fn dimensions_require_a_complete_triplet_for_display() {
        assert_eq!(
            dimensions_label(Some(10), Some(20), Some(30)),
            "10×20×30 mm"
        );
        assert_eq!(dimensions_label(Some(10), None, Some(30)), "—");
    }

    #[test]
    fn timestamp_uses_a_dense_minute_label() {
        assert_eq!(
            compact_timestamp("2026-08-08T16:06:59.386043+00:00"),
            "2026-08-08 16:06"
        );
        assert_eq!(compact_timestamp("not-a-timestamp"), "not-a-timestamp");
    }
}
