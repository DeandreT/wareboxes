use wareboxes_api_contract::v1::{
    CreateInventoryRecallRequest, InventoryAgingBucket, InventoryAgingPage, InventoryAgingSort,
    InventoryIntegrityIssueKind, InventoryIntegrityPage, InventoryIntegritySort,
    InventoryJournalPage, InventoryJournalSort, InventoryRecallPage, InventoryRecallResponse,
    InventoryRecallStatus, InventoryReconciliationStatusResponse, InventorySortDirection,
    OpaqueCursor, ReleaseInventoryRecallRequest,
};

use super::{internal_get, internal_post_idempotent, ApiError};

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

#[derive(Clone, Default)]
pub struct AgingFilters {
    pub query: Option<String>,
    pub facility_id: Option<i64>,
    pub inventory_owner_id: Option<i64>,
    pub item_id: Option<i64>,
    pub bucket: Option<InventoryAgingBucket>,
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

pub async fn inventory_reconciliation_status(
) -> Result<InventoryReconciliationStatusResponse, ApiError> {
    internal_get("/api/v1/inventory/reconciliation/status").await
}

pub async fn inventory_aging(
    filters: AgingFilters,
    sort: InventoryAgingSort,
    direction: InventorySortDirection,
    cursor: Option<&OpaqueCursor>,
) -> Result<InventoryAgingPage, ApiError> {
    internal_get(&aging_path(&filters, sort, direction, cursor)).await
}

pub async fn inventory_recalls(
    facility_id: Option<i64>,
    inventory_owner_id: Option<i64>,
    status: Option<InventoryRecallStatus>,
    cursor: Option<&OpaqueCursor>,
) -> Result<InventoryRecallPage, ApiError> {
    internal_get(&recall_path(
        facility_id,
        inventory_owner_id,
        status,
        cursor,
    ))
    .await
}

pub async fn create_inventory_recall(
    request: &CreateInventoryRecallRequest,
    idempotency_key: &str,
) -> Result<InventoryRecallResponse, ApiError> {
    internal_post_idempotent("/api/v1/inventory/recalls", request, idempotency_key).await
}

pub async fn release_inventory_recall(
    recall_id: i64,
    request: &ReleaseInventoryRecallRequest,
    idempotency_key: &str,
) -> Result<InventoryRecallResponse, ApiError> {
    internal_post_idempotent(
        &format!("/api/v1/inventory/recalls/{recall_id}/releases"),
        request,
        idempotency_key,
    )
    .await
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

fn aging_path(
    filters: &AgingFilters,
    sort: InventoryAgingSort,
    direction: InventorySortDirection,
    cursor: Option<&OpaqueCursor>,
) -> String {
    let mut params = vec![
        "limit=100".to_owned(),
        format!("sort={}", aging_sort_value(sort)),
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
    if let Some(bucket) = filters.bucket {
        params.push(format!("bucket={}", aging_bucket_value(bucket)));
    }
    if let Some(cursor) = cursor {
        params.push(format!("cursor={}", urlencoding::encode(cursor.as_str())));
    }
    format!("/api/v1/inventory/aging?{}", params.join("&"))
}

fn recall_path(
    facility_id: Option<i64>,
    inventory_owner_id: Option<i64>,
    status: Option<InventoryRecallStatus>,
    cursor: Option<&OpaqueCursor>,
) -> String {
    let mut params = vec!["limit=100".to_owned()];
    push_id(&mut params, "facility_id", facility_id);
    push_id(&mut params, "inventory_owner_id", inventory_owner_id);
    if let Some(status) = status {
        params.push(format!(
            "status={}",
            match status {
                InventoryRecallStatus::Active => "active",
                InventoryRecallStatus::Released => "released",
            }
        ));
    }
    if let Some(cursor) = cursor {
        params.push(format!("cursor={}", urlencoding::encode(cursor.as_str())));
    }
    format!("/api/v1/inventory/recalls?{}", params.join("&"))
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

fn aging_sort_value(value: InventoryAgingSort) -> &'static str {
    match value {
        InventoryAgingSort::Age => "age",
        InventoryAgingSort::Expiration => "expiration",
        InventoryAgingSort::Quantity => "quantity",
        InventoryAgingSort::Facility => "facility",
        InventoryAgingSort::Client => "client",
        InventoryAgingSort::Item => "item",
    }
}

fn aging_bucket_value(value: InventoryAgingBucket) -> &'static str {
    match value {
        InventoryAgingBucket::Expired => "expired",
        InventoryAgingBucket::DueWithin7Days => "due_within_7_days",
        InventoryAgingBucket::DueWithin30Days => "due_within_30_days",
        InventoryAgingBucket::DueWithin90Days => "due_within_90_days",
        InventoryAgingBucket::Beyond90Days => "beyond_90_days",
        InventoryAgingBucket::NoExpiration => "no_expiration",
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

    #[test]
    fn aging_paths_bind_risk_sort_and_search() {
        let path = aging_path(
            &AgingFilters {
                query: Some("LOT A/1".to_owned()),
                facility_id: Some(7),
                bucket: Some(InventoryAgingBucket::DueWithin30Days),
                ..Default::default()
            },
            InventoryAgingSort::Expiration,
            InventorySortDirection::Ascending,
            None,
        );
        assert!(path.contains("query=LOT%20A%2F1"));
        assert!(path.contains("facility_id=7"));
        assert!(path.contains("bucket=due_within_30_days"));
        assert!(path.contains("sort=expiration&direction=ascending"));
    }

    #[test]
    fn recall_paths_bind_scope_status_and_cursor() {
        let cursor = OpaqueCursor::new("ir1.cursor").unwrap();
        let path = recall_path(
            Some(7),
            Some(8),
            Some(InventoryRecallStatus::Active),
            Some(&cursor),
        );
        assert_eq!(
            path,
            "/api/v1/inventory/recalls?limit=100&facility_id=7&inventory_owner_id=8&status=active&cursor=ir1.cursor"
        );
    }
}
