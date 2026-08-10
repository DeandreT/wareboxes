use wareboxes_core::models::{LoadStatus, Order, OrderStatus, Timestamp};

pub(super) fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

pub(super) fn parse_optional_timestamp(value: &str) -> Result<Option<Timestamp>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }

    let normalized = if value.ends_with('Z') || value.contains('+') {
        value.to_owned()
    } else if value.len() == 16 {
        format!("{value}:00Z")
    } else {
        format!("{value}Z")
    };
    normalized
        .parse::<Timestamp>()
        .map(Some)
        .map_err(|_| "Enter a valid date and time.".to_owned())
}

pub(super) fn timestamp_input(value: Option<Timestamp>) -> String {
    value.map_or_else(String::new, |value| {
        value.format("%Y-%m-%dT%H:%M").to_string()
    })
}

pub(super) fn short_timestamp(value: Timestamp) -> String {
    value.format("%Y-%m-%d %H:%M").to_string()
}

pub(super) fn optional_timestamp(value: Option<Timestamp>) -> String {
    value.map_or_else(|| "Not set".to_owned(), short_timestamp)
}

pub(super) fn query_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

pub(super) fn order_destination(order: &Order) -> String {
    [
        order.city.as_deref(),
        order.state.as_deref(),
        order.postal_code.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.trim().is_empty())
    .collect::<Vec<_>>()
    .join(", ")
}

pub(super) fn order_status_class(status: OrderStatus) -> &'static str {
    match status {
        OrderStatus::Shipped => "status shipped",
        OrderStatus::Cancelled | OrderStatus::Void => "status muted",
        OrderStatus::Held => "status held",
        OrderStatus::Processing
        | OrderStatus::AwaitingPacking
        | OrderStatus::Packing
        | OrderStatus::AwaitingShipment => "status processing",
        OrderStatus::Open => "status open",
    }
}

pub(super) fn load_status_class(status: LoadStatus) -> &'static str {
    match status {
        LoadStatus::Received | LoadStatus::Closed => "status shipped",
        LoadStatus::Cancelled => "status muted",
        LoadStatus::Rejected => "status held",
        LoadStatus::Arrived | LoadStatus::Receiving => "status processing",
        LoadStatus::Planned | LoadStatus::Scheduled => "status open",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_encoding_preserves_url_safe_bytes() {
        assert_eq!(query_encode("PO 8/2"), "PO%208%2F2");
        assert_eq!(query_encode("ABC-_.~"), "ABC-_.~");
    }

    #[test]
    fn browser_datetime_is_interpreted_as_utc() {
        let parsed = parse_optional_timestamp("2026-07-29T14:05")
            .expect("valid timestamp")
            .expect("timestamp");
        assert_eq!(parsed.to_rfc3339(), "2026-07-29T14:05:00+00:00");
    }

    #[test]
    fn blank_optional_values_are_absent() {
        assert_eq!(optional_text("  "), None);
        assert_eq!(parse_optional_timestamp("  "), Ok(None));
    }
}
