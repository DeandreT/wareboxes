use axum::extract::{Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    InventoryAgingBucket as ApiAgingBucket, InventoryAgingPage as ApiAgingPage,
    InventoryAgingPageRequest, InventoryAgingResponse, InventoryAgingSort as ApiAgingSort,
    InventoryBalanceStatus as ApiInventoryBalanceStatus,
    InventoryIntegrityIssueKind as ApiIssueKind, InventoryIntegrityIssueResponse,
    InventoryIntegrityPage as ApiIntegrityPage, InventoryIntegrityPageRequest,
    InventoryIntegritySort as ApiIntegritySort, InventoryJournalEntryResponse,
    InventoryJournalPage as ApiJournalPage, InventoryJournalPageRequest,
    InventoryJournalSort as ApiJournalSort, InventoryJournalTransactionResponse,
    InventorySortDirection as ApiDirection, OpaqueCursor,
};
use wareboxes_application::inventory::InventoryBalanceStatus;
use wareboxes_application::inventory_integrity::{
    InventoryAgingBucket, InventoryAgingQuery, InventoryAgingReadModel, InventoryAgingSort,
    InventoryIntegrityIssueKind, InventoryIntegrityIssueReadModel, InventoryIntegrityQuery,
    InventoryIntegritySort, InventoryJournalEntryReadModel, InventoryJournalQuery,
    InventoryJournalSort, InventoryJournalTransactionReadModel, InventorySortDirection,
};
use wareboxes_domain::InventoryOwnerId;

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const JOURNAL_CURSOR_PREFIX: &str = "ij1.";
const INTEGRITY_CURSOR_PREFIX: &str = "ii1.";
const AGING_CURSOR_PREFIX: &str = "ia1.";

pub async fn journal(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<InventoryJournalPageRequest>,
) -> V1Result<Json<ApiJournalPage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let query = journal_query(&user, &request)?;
    let page = repo::inventory_integrity::journal_page(&state.db, &user.tenant, &query).await?;
    let next_cursor = page
        .next_offset
        .map(|offset| encode_journal_cursor(&request, offset))
        .transpose()?;
    Ok(Json(ApiJournalPage::new(
        page.items.into_iter().map(map_transaction).collect(),
        next_cursor,
    )))
}

pub async fn issues(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<InventoryIntegrityPageRequest>,
) -> V1Result<Json<ApiIntegrityPage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let query = integrity_query(&user, &request)?;
    let page = repo::inventory_integrity::integrity_page(&state.db, &user.tenant, &query).await?;
    let next_cursor = page
        .next_offset
        .map(|offset| encode_integrity_cursor(&request, offset))
        .transpose()?;
    Ok(Json(ApiIntegrityPage::new(
        page.items.into_iter().map(map_issue).collect(),
        next_cursor,
    )))
}

pub async fn aging(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<InventoryAgingPageRequest>,
) -> V1Result<Json<ApiAgingPage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let query = aging_query(&user, &request)?;
    let page = repo::inventory_integrity::aging_page(&state.db, &user.tenant, &query).await?;
    let next_cursor = page
        .next_offset
        .map(|offset| encode_aging_cursor(&request, offset))
        .transpose()?;
    Ok(Json(ApiAgingPage::new(
        page.items.into_iter().map(map_aging).collect(),
        next_cursor,
    )))
}

fn journal_query(
    user: &CurrentTenant,
    request: &InventoryJournalPageRequest,
) -> V1Result<InventoryJournalQuery> {
    let facility_id = request
        .facility_id
        .map(|id| user.require_facility(id))
        .transpose()?;
    let inventory_owner_id = request
        .inventory_owner_id
        .map(|id| {
            user.require_inventory_owner(id)?;
            InventoryOwnerId::new(id).map_err(|error| AppError::bad_request(error.to_string()))
        })
        .transpose()?;
    for id in [
        request.item_id,
        request.item_batch_id,
        request.license_plate_id,
        request.transaction_id,
    ] {
        if id.is_some_and(|value| value <= 0) {
            return Err(
                AppError::bad_request("inventory journal filter IDs must be positive").into(),
            );
        }
    }
    let offset = cursor_offset(
        request.cursor.as_ref(),
        JOURNAL_CURSOR_PREFIX,
        &journal_filter_key(request),
        "inventory journal",
    )?;
    Ok(InventoryJournalQuery {
        search: request
            .query
            .as_ref()
            .map(|value| value.as_str().to_owned()),
        facility_id,
        inventory_owner_id,
        item_id: request.item_id,
        item_batch_id: request.item_batch_id,
        license_plate_id: request.license_plate_id,
        transaction_id: request.transaction_id,
        sort: map_journal_sort(request.sort),
        direction: map_direction(request.direction),
        offset,
        limit: request.limit.get(),
    })
}

fn integrity_query(
    user: &CurrentTenant,
    request: &InventoryIntegrityPageRequest,
) -> V1Result<InventoryIntegrityQuery> {
    let facility_id = request
        .facility_id
        .map(|id| user.require_facility(id))
        .transpose()?;
    let inventory_owner_id = request
        .inventory_owner_id
        .map(|id| {
            user.require_inventory_owner(id)?;
            InventoryOwnerId::new(id).map_err(|error| AppError::bad_request(error.to_string()))
        })
        .transpose()?;
    if request.item_id.is_some_and(|value| value <= 0) {
        return Err(AppError::bad_request("inventory integrity item ID must be positive").into());
    }
    let offset = cursor_offset(
        request.cursor.as_ref(),
        INTEGRITY_CURSOR_PREFIX,
        &integrity_filter_key(request),
        "inventory integrity",
    )?;
    Ok(InventoryIntegrityQuery {
        kind: request.kind.map(map_issue_kind),
        facility_id,
        inventory_owner_id,
        item_id: request.item_id,
        sort: map_integrity_sort(request.sort),
        direction: map_direction(request.direction),
        offset,
        limit: request.limit.get(),
    })
}

fn aging_query(
    user: &CurrentTenant,
    request: &InventoryAgingPageRequest,
) -> V1Result<InventoryAgingQuery> {
    let facility_id = request
        .facility_id
        .map(|id| user.require_facility(id))
        .transpose()?;
    let inventory_owner_id = request
        .inventory_owner_id
        .map(|id| {
            user.require_inventory_owner(id)?;
            InventoryOwnerId::new(id).map_err(|error| AppError::bad_request(error.to_string()))
        })
        .transpose()?;
    if request.item_id.is_some_and(|value| value <= 0) {
        return Err(AppError::bad_request("inventory aging item ID must be positive").into());
    }
    let offset = cursor_offset(
        request.cursor.as_ref(),
        AGING_CURSOR_PREFIX,
        &aging_filter_key(request),
        "inventory aging",
    )?;
    Ok(InventoryAgingQuery {
        search: request
            .query
            .as_ref()
            .map(|value| value.as_str().to_owned()),
        facility_id,
        inventory_owner_id,
        item_id: request.item_id,
        bucket: request.bucket.map(map_aging_bucket),
        sort: map_aging_sort(request.sort),
        direction: map_direction(request.direction),
        offset,
        limit: request.limit.get(),
    })
}

fn cursor_offset(
    cursor: Option<&OpaqueCursor>,
    prefix: &str,
    expected_filter: &str,
    collection: &'static str,
) -> V1Result<u64> {
    let Some(cursor) = cursor else { return Ok(0) };
    let encoded = cursor
        .as_str()
        .strip_prefix(prefix)
        .ok_or_else(|| V1Error::invalid_cursor_for(collection))?;
    let (filter, offset) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for(collection))?;
    if filter != expected_filter || offset.len() != 16 {
        return Err(V1Error::invalid_cursor_for(collection));
    }
    u64::from_str_radix(offset, 16).map_err(|_| V1Error::invalid_cursor_for(collection))
}

fn encode_journal_cursor(
    request: &InventoryJournalPageRequest,
    offset: u64,
) -> AppResult<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{JOURNAL_CURSOR_PREFIX}{}.{offset:016x}",
        journal_filter_key(request)
    ))
    .map_err(|_| AppError::internal("generated an invalid inventory journal cursor"))
}

fn encode_integrity_cursor(
    request: &InventoryIntegrityPageRequest,
    offset: u64,
) -> AppResult<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{INTEGRITY_CURSOR_PREFIX}{}.{offset:016x}",
        integrity_filter_key(request)
    ))
    .map_err(|_| AppError::internal("generated an invalid inventory integrity cursor"))
}

fn encode_aging_cursor(
    request: &InventoryAgingPageRequest,
    offset: u64,
) -> AppResult<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{AGING_CURSOR_PREFIX}{}.{offset:016x}",
        aging_filter_key(request)
    ))
    .map_err(|_| AppError::internal("generated an invalid inventory aging cursor"))
}

fn journal_filter_key(request: &InventoryJournalPageRequest) -> String {
    format!(
        "{}.{}.{}.{}.{}.{}.{}.{}.{}",
        optional_id(request.facility_id),
        optional_id(request.inventory_owner_id),
        optional_id(request.item_id),
        optional_id(request.item_batch_id),
        optional_id(request.license_plate_id),
        optional_id(request.transaction_id),
        journal_sort_key(request.sort),
        direction_key(request.direction),
        request
            .query
            .as_ref()
            .map_or_else(|| "-".to_owned(), |value| hex_encode(value.as_str()))
    )
}

fn integrity_filter_key(request: &InventoryIntegrityPageRequest) -> String {
    format!(
        "{}.{}.{}.{}.{}.{}",
        issue_kind_filter_key(request.kind),
        optional_id(request.facility_id),
        optional_id(request.inventory_owner_id),
        optional_id(request.item_id),
        integrity_sort_key(request.sort),
        direction_key(request.direction)
    )
}

fn aging_filter_key(request: &InventoryAgingPageRequest) -> String {
    format!(
        "{}.{}.{}.{}.{}.{}.{}",
        optional_id(request.facility_id),
        optional_id(request.inventory_owner_id),
        optional_id(request.item_id),
        aging_bucket_filter_key(request.bucket),
        aging_sort_key(request.sort),
        direction_key(request.direction),
        request
            .query
            .as_ref()
            .map_or_else(|| "-".to_owned(), |value| hex_encode(value.as_str()))
    )
}

fn optional_id(value: Option<i64>) -> String {
    value.map_or_else(|| "-".to_owned(), |id| format!("{id:016x}"))
}

fn hex_encode(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn journal_sort_key(value: ApiJournalSort) -> &'static str {
    match value {
        ApiJournalSort::OccurredAt => "time",
        ApiJournalSort::Transaction => "transaction",
        ApiJournalSort::Type => "type",
        ApiJournalSort::Client => "client",
        ApiJournalSort::NetQuantity => "net",
    }
}

fn integrity_sort_key(value: ApiIntegritySort) -> &'static str {
    match value {
        ApiIntegritySort::Severity => "severity",
        ApiIntegritySort::Facility => "facility",
        ApiIntegritySort::Client => "client",
        ApiIntegritySort::Item => "item",
    }
}

fn aging_sort_key(value: ApiAgingSort) -> &'static str {
    match value {
        ApiAgingSort::Age => "age",
        ApiAgingSort::Expiration => "expiration",
        ApiAgingSort::Quantity => "quantity",
        ApiAgingSort::Facility => "facility",
        ApiAgingSort::Client => "client",
        ApiAgingSort::Item => "item",
    }
}

fn direction_key(value: ApiDirection) -> &'static str {
    match value {
        ApiDirection::Ascending => "asc",
        ApiDirection::Descending => "desc",
    }
}

fn issue_kind_filter_key(value: Option<ApiIssueKind>) -> &'static str {
    match value {
        None => "all",
        Some(ApiIssueKind::JournalProjection) => "journal",
        Some(ApiIssueKind::Commitments) => "commitments",
    }
}

fn aging_bucket_filter_key(value: Option<ApiAgingBucket>) -> &'static str {
    match value {
        None => "all",
        Some(ApiAgingBucket::Expired) => "expired",
        Some(ApiAgingBucket::DueWithin7Days) => "due7",
        Some(ApiAgingBucket::DueWithin30Days) => "due30",
        Some(ApiAgingBucket::DueWithin90Days) => "due90",
        Some(ApiAgingBucket::Beyond90Days) => "beyond90",
        Some(ApiAgingBucket::NoExpiration) => "none",
    }
}

fn map_journal_sort(value: ApiJournalSort) -> InventoryJournalSort {
    match value {
        ApiJournalSort::OccurredAt => InventoryJournalSort::OccurredAt,
        ApiJournalSort::Transaction => InventoryJournalSort::Transaction,
        ApiJournalSort::Type => InventoryJournalSort::Type,
        ApiJournalSort::Client => InventoryJournalSort::Client,
        ApiJournalSort::NetQuantity => InventoryJournalSort::NetQuantity,
    }
}

fn map_integrity_sort(value: ApiIntegritySort) -> InventoryIntegritySort {
    match value {
        ApiIntegritySort::Severity => InventoryIntegritySort::Severity,
        ApiIntegritySort::Facility => InventoryIntegritySort::Facility,
        ApiIntegritySort::Client => InventoryIntegritySort::Client,
        ApiIntegritySort::Item => InventoryIntegritySort::Item,
    }
}

fn map_aging_sort(value: ApiAgingSort) -> InventoryAgingSort {
    match value {
        ApiAgingSort::Age => InventoryAgingSort::Age,
        ApiAgingSort::Expiration => InventoryAgingSort::Expiration,
        ApiAgingSort::Quantity => InventoryAgingSort::Quantity,
        ApiAgingSort::Facility => InventoryAgingSort::Facility,
        ApiAgingSort::Client => InventoryAgingSort::Client,
        ApiAgingSort::Item => InventoryAgingSort::Item,
    }
}

fn map_aging_bucket(value: ApiAgingBucket) -> InventoryAgingBucket {
    match value {
        ApiAgingBucket::Expired => InventoryAgingBucket::Expired,
        ApiAgingBucket::DueWithin7Days => InventoryAgingBucket::DueWithin7Days,
        ApiAgingBucket::DueWithin30Days => InventoryAgingBucket::DueWithin30Days,
        ApiAgingBucket::DueWithin90Days => InventoryAgingBucket::DueWithin90Days,
        ApiAgingBucket::Beyond90Days => InventoryAgingBucket::Beyond90Days,
        ApiAgingBucket::NoExpiration => InventoryAgingBucket::NoExpiration,
    }
}

fn map_direction(value: ApiDirection) -> InventorySortDirection {
    match value {
        ApiDirection::Ascending => InventorySortDirection::Ascending,
        ApiDirection::Descending => InventorySortDirection::Descending,
    }
}

fn map_issue_kind(value: ApiIssueKind) -> InventoryIntegrityIssueKind {
    match value {
        ApiIssueKind::JournalProjection => InventoryIntegrityIssueKind::JournalProjection,
        ApiIssueKind::Commitments => InventoryIntegrityIssueKind::Commitments,
    }
}

fn map_status(value: InventoryBalanceStatus) -> ApiInventoryBalanceStatus {
    match value {
        InventoryBalanceStatus::Available => ApiInventoryBalanceStatus::Available,
        InventoryBalanceStatus::Hold => ApiInventoryBalanceStatus::Hold,
        InventoryBalanceStatus::Damaged => ApiInventoryBalanceStatus::Damaged,
        InventoryBalanceStatus::Quarantine => ApiInventoryBalanceStatus::Quarantine,
    }
}

fn map_transaction(
    value: InventoryJournalTransactionReadModel,
) -> InventoryJournalTransactionResponse {
    InventoryJournalTransactionResponse {
        id: value.id,
        inventory_owner_id: value.inventory_owner_id.get(),
        inventory_owner_name: value.inventory_owner_name,
        occurred_at: value.occurred_at.to_rfc3339(),
        actor_user_id: value.actor_user_id,
        transaction_type: value.transaction_type,
        reason: value.reason,
        reference_type: value.reference_type,
        reference_id: value.reference_id,
        correlation_id: value.correlation_id,
        operation: value.operation,
        entry_count: value.entry_count,
        net_quantity: value.net_quantity,
        entries: value.entries.into_iter().map(map_entry).collect(),
    }
}

fn map_entry(value: InventoryJournalEntryReadModel) -> InventoryJournalEntryResponse {
    InventoryJournalEntryResponse {
        id: value.id,
        facility_id: value.facility_id.get(),
        facility_name: value.facility_name,
        location_id: value.location_id,
        location_name: value.location_name,
        location_barcode: value.location_barcode,
        license_plate_id: value.license_plate_id,
        license_plate_barcode: value.license_plate_barcode,
        item_batch_id: value.item_batch_id,
        item_id: value.item_id,
        primary_sku: value.primary_sku,
        item_description: value.item_description,
        uom: value.uom,
        lot: value.lot,
        expiration: value.expiration.map(|time| time.to_rfc3339()),
        serial: value.serial,
        status: map_status(value.status),
        quantity_delta: value.quantity_delta,
    }
}

fn map_issue(value: InventoryIntegrityIssueReadModel) -> InventoryIntegrityIssueResponse {
    InventoryIntegrityIssueResponse {
        issue_key: value.issue_key,
        kind: match value.kind {
            InventoryIntegrityIssueKind::JournalProjection => ApiIssueKind::JournalProjection,
            InventoryIntegrityIssueKind::Commitments => ApiIssueKind::Commitments,
        },
        inventory_owner_id: value.inventory_owner_id.get(),
        inventory_owner_name: value.inventory_owner_name,
        facility_id: value.facility_id.get(),
        facility_name: value.facility_name,
        location_id: value.location_id,
        location_name: value.location_name,
        location_barcode: value.location_barcode,
        license_plate_id: value.license_plate_id,
        license_plate_barcode: value.license_plate_barcode,
        item_batch_id: value.item_batch_id,
        item_id: value.item_id,
        primary_sku: value.primary_sku,
        item_description: value.item_description,
        lot: value.lot,
        serial: value.serial,
        uom: value.uom,
        status: map_status(value.status),
        journal_quantity: value.journal_quantity,
        projected_quantity: value.projected_quantity,
        variance_quantity: value.variance_quantity,
        on_hand_quantity: value.on_hand_quantity,
        reserved_quantity: value.reserved_quantity,
        allocated_quantity: value.allocated_quantity,
        held_quantity: value.held_quantity,
        hold_ledger_quantity: value.hold_ledger_quantity,
        overcommitted_quantity: value.overcommitted_quantity,
        severity_quantity: value.severity_quantity,
        issue_codes: value.issue_codes,
    }
}

fn map_aging(value: InventoryAgingReadModel) -> InventoryAgingResponse {
    InventoryAgingResponse {
        inventory_balance_id: value.inventory_balance_id,
        inventory_owner_id: value.inventory_owner_id.get(),
        inventory_owner_name: value.inventory_owner_name,
        facility_id: value.facility_id.get(),
        facility_name: value.facility_name,
        location_id: value.location_id,
        location_name: value.location_name,
        location_barcode: value.location_barcode,
        license_plate_id: value.license_plate_id,
        license_plate_barcode: value.license_plate_barcode,
        item_batch_id: value.item_batch_id,
        item_id: value.item_id,
        primary_sku: value.primary_sku,
        item_description: value.item_description,
        uom: value.uom,
        lot: value.lot,
        serial: value.serial,
        received_at: value.received_at.to_rfc3339(),
        age_days: value.age_days,
        expiration: value.expiration.map(|time| time.to_rfc3339()),
        days_to_expiration: value.days_to_expiration,
        bucket: match value.bucket {
            InventoryAgingBucket::Expired => ApiAgingBucket::Expired,
            InventoryAgingBucket::DueWithin7Days => ApiAgingBucket::DueWithin7Days,
            InventoryAgingBucket::DueWithin30Days => ApiAgingBucket::DueWithin30Days,
            InventoryAgingBucket::DueWithin90Days => ApiAgingBucket::DueWithin90Days,
            InventoryAgingBucket::Beyond90Days => ApiAgingBucket::Beyond90Days,
            InventoryAgingBucket::NoExpiration => ApiAgingBucket::NoExpiration,
        },
        status: map_status(value.status),
        on_hand_quantity: value.on_hand_quantity,
        reserved_quantity: value.reserved_quantity,
        held_quantity: value.held_quantity,
        available_quantity: value.available_quantity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_are_bound_to_every_filter_and_sort() {
        let request = InventoryJournalPageRequest {
            facility_id: Some(7),
            item_id: Some(9),
            sort: ApiJournalSort::Client,
            direction: ApiDirection::Ascending,
            ..Default::default()
        };
        let cursor = encode_journal_cursor(&request, 100).unwrap();
        assert_eq!(
            cursor_offset(
                Some(&cursor),
                JOURNAL_CURSOR_PREFIX,
                &journal_filter_key(&request),
                "inventory journal"
            )
            .unwrap(),
            100
        );
        let mut changed = request.clone();
        changed.item_id = Some(10);
        assert!(cursor_offset(
            Some(&cursor),
            JOURNAL_CURSOR_PREFIX,
            &journal_filter_key(&changed),
            "inventory journal"
        )
        .is_err());
    }

    #[test]
    fn aging_cursor_is_bound_to_bucket_search_and_sort() {
        let request = InventoryAgingPageRequest {
            bucket: Some(ApiAgingBucket::DueWithin30Days),
            sort: ApiAgingSort::Expiration,
            direction: ApiDirection::Ascending,
            ..Default::default()
        };
        let cursor = encode_aging_cursor(&request, 100).unwrap();
        assert_eq!(
            cursor_offset(
                Some(&cursor),
                AGING_CURSOR_PREFIX,
                &aging_filter_key(&request),
                "inventory aging"
            )
            .unwrap(),
            100
        );
        let mut changed = request.clone();
        changed.bucket = Some(ApiAgingBucket::Expired);
        assert!(cursor_offset(
            Some(&cursor),
            AGING_CURSOR_PREFIX,
            &aging_filter_key(&changed),
            "inventory aging"
        )
        .is_err());
    }
}
