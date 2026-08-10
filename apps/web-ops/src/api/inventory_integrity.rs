use wareboxes_api_contract::v1::{
    InventoryIntegrityIssueKind, InventoryIntegrityPage, InventoryIntegritySort,
    InventoryJournalPage, InventoryJournalSort, InventorySortDirection, OpaqueCursor,
};

use super::{internal_get, ApiError};

#[derive(Clone, Default)]
pub struct JournalFilters {
    pub query: Option<String>,
    pub facility_id: Option<i64>,
    pub inventory_owner_id: Option<i64>,
    pub item_id: Option<i64>,
    pub item_batch_id: Option<i64>,
    pub license_plate_id: Option<i64>,
    pub transaction_id: Option<i64>,
}

#[derive(Clone, Copy, Default)]
pub struct IntegrityFilters {
    pub kind: Option<InventoryIntegrityIssueKind>,
    pub facility_id: Option<i64>,
    pub inventory_owner_id: Option<i64>,
    pub item_id: Option<i64>,
}

pub async fn inventory_journal(
    filters: JournalFilters,
    sort: InventoryJournalSort,
    direction: InventorySortDirection,
    cursor: Option<&OpaqueCursor>,
) -> Result<InventoryJournalPage, ApiError> {
    internal_get(&journal_path(&filters, sort, direction, cursor)).await
}

pub async fn inventory_integrity_issues(
    filters: IntegrityFilters,
    sort: InventoryIntegritySort,
    direction: InventorySortDirection,
    cursor: Option<&OpaqueCursor>,
) -> Result<InventoryIntegrityPage, ApiError> {
    internal_get(&integrity_path(filters, sort, direction, cursor)).await
}

fn journal_path(
    filters: &JournalFilters,
    sort: InventoryJournalSort,
    direction: InventorySortDirection,
    cursor: Option<&OpaqueCursor>,
) -> String {
    let mut params = vec![
        "limit=100".to_owned(),
        format!("sort={}", journal_sort_value(sort)),
        format!("direction={}", direction_value(direction)),
    ];
    push_text(&mut params, "query", filters.query.as_deref());
    push_id(&mut params, "facility_id", filters.facility_id);
    push_id(
        &mut params,
        "inventory_owner_id",
        filters.inventory_owner_id,
    );
    push_id(&mut params, "item_id", filters.item_id);
    push_id(&mut params, "item_batch_id", filters.item_batch_id);
    push_id(&mut params, "license_plate_id", filters.license_plate_id);
    push_id(&mut params, "transaction_id", filters.transaction_id);
    if let Some(cursor) = cursor {
        params.push(format!("cursor={}", urlencoding::encode(cursor.as_str())));
    }
    format!("/api/v1/inventory/journal?{}", params.join("&"))
}

fn integrity_path(
    filters: IntegrityFilters,
    sort: InventoryIntegritySort,
    direction: InventorySortDirection,
    cursor: Option<&OpaqueCursor>,
) -> String {
    let mut params = vec![
        "limit=100".to_owned(),
        format!("sort={}", integrity_sort_value(sort)),
        format!("direction={}", direction_value(direction)),
    ];
    if let Some(kind) = filters.kind {
        params.push(format!("kind={}", issue_kind_value(kind)));
    }
    push_id(&mut params, "facility_id", filters.facility_id);
    push_id(
        &mut params,
        "inventory_owner_id",
        filters.inventory_owner_id,
    );
    push_id(&mut params, "item_id", filters.item_id);
    if let Some(cursor) = cursor {
        params.push(format!("cursor={}", urlencoding::encode(cursor.as_str())));
    }
    format!("/api/v1/inventory/integrity-issues?{}", params.join("&"))
}

fn push_text(params: &mut Vec<String>, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        params.push(format!("{key}={}", urlencoding::encode(value)));
    }
}

fn push_id(params: &mut Vec<String>, key: &str, value: Option<i64>) {
    if let Some(value) = value {
        params.push(format!("{key}={value}"));
    }
}

fn journal_sort_value(value: InventoryJournalSort) -> &'static str {
    match value {
        InventoryJournalSort::OccurredAt => "occurred_at",
        InventoryJournalSort::Transaction => "transaction",
        InventoryJournalSort::Type => "type",
        InventoryJournalSort::Client => "client",
        InventoryJournalSort::NetQuantity => "net_quantity",
    }
}

fn integrity_sort_value(value: InventoryIntegritySort) -> &'static str {
    match value {
        InventoryIntegritySort::Severity => "severity",
        InventoryIntegritySort::Facility => "facility",
        InventoryIntegritySort::Client => "client",
        InventoryIntegritySort::Item => "item",
    }
}

fn direction_value(value: InventorySortDirection) -> &'static str {
    match value {
        InventorySortDirection::Ascending => "ascending",
        InventorySortDirection::Descending => "descending",
    }
}

fn issue_kind_value(value: InventoryIntegrityIssueKind) -> &'static str {
    match value {
        InventoryIntegrityIssueKind::JournalProjection => "journal_projection",
        InventoryIntegrityIssueKind::Commitments => "commitments",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_paths_encode_trace_filters_sort_and_cursor() {
        let cursor = OpaqueCursor::new("ij1.cursor").unwrap();
        let path = journal_path(
            &JournalFilters {
                query: Some("LOT A/1".to_owned()),
                facility_id: Some(7),
                item_batch_id: Some(9),
                ..Default::default()
            },
            InventoryJournalSort::NetQuantity,
            InventorySortDirection::Ascending,
            Some(&cursor),
        );
        assert!(path.contains("query=LOT%20A%2F1"));
        assert!(path.contains("facility_id=7"));
        assert!(path.contains("item_batch_id=9"));
        assert!(path.contains("sort=net_quantity&direction=ascending"));
        assert!(path.contains("cursor=ij1.cursor"));
    }
}
