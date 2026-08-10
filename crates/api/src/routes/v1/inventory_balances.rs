use axum::extract::{Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    InventoryBalancePage, InventoryBalancePageRequest, InventoryBalanceResponse,
    InventoryBalanceSort, InventoryBalanceStatus, InventoryQuantity, InventorySortDirection,
    OpaqueCursor,
};
use wareboxes_application::inventory::{
    InventoryBalancePageQuery, InventoryBalanceReadModel,
    InventoryBalanceSort as ApplicationInventoryBalanceSort,
    InventoryBalanceSortDirection as ApplicationSortDirection,
    InventoryBalanceStatus as ApplicationInventoryBalanceStatus,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

const PERMISSION: &str = "wms";
const CURSOR_PREFIX: &str = "ib2.";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<InventoryBalancePageRequest>,
) -> V1Result<Json<InventoryBalancePage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let offset = decode_bound_cursor(&query)?;
    Ok(Json(
        page_for_access(
            &state,
            &user.tenant,
            &BalancePageOptions {
                offset,
                limit: query.limit.get(),
                query: query.query.as_ref().map(|query| query.as_str()),
                sort: query.sort,
                direction: query.direction,
                movable_only: query.movable_only,
            },
        )
        .await?,
    ))
}

pub(crate) struct BalancePageOptions<'a> {
    pub offset: u64,
    pub limit: u16,
    pub query: Option<&'a str>,
    pub sort: InventoryBalanceSort,
    pub direction: InventorySortDirection,
    pub movable_only: bool,
}

pub(crate) async fn page_for_access(
    state: &AppState,
    access: &wareboxes_core::models::TenantAccess,
    options: &BalancePageOptions<'_>,
) -> AppResult<InventoryBalancePage> {
    let page = wareboxes_persistence_postgres::inventory_balances::get_inventory_balance_page(
        &state.db,
        access.tenant_id,
        &access.site_scope,
        &access.owner_scope,
        &InventoryBalancePageQuery {
            offset: options.offset,
            limit: options.limit,
            query: options.query.map(str::to_owned),
            sort: map_sort(options.sort),
            direction: map_direction(options.direction),
            movable_only: options.movable_only,
        },
    )
    .await?;
    let items = page.items.into_iter().map(map_balance).collect();
    let next_cursor = page
        .next_offset
        .map(|offset| {
            encode_cursor(
                options.query,
                options.sort,
                options.direction,
                options.movable_only,
                offset,
            )
        })
        .transpose()?;

    Ok(InventoryBalancePage::new(items, next_cursor))
}

fn decode_bound_cursor(query: &InventoryBalancePageRequest) -> V1Result<u64> {
    let Some(cursor) = query.cursor.as_ref() else {
        return Ok(0);
    };
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(V1Error::invalid_cursor)?;
    let (filter, offset) = encoded
        .rsplit_once('.')
        .ok_or_else(V1Error::invalid_cursor)?;
    if filter
        != cursor_filter(
            query.query.as_ref().map(|value| value.as_str()),
            query.sort,
            query.direction,
            query.movable_only,
        )
        || offset.len() != 16
    {
        return Err(V1Error::invalid_cursor());
    }
    u64::from_str_radix(offset, 16).map_err(|_| V1Error::invalid_cursor())
}

fn encode_cursor(
    query: Option<&str>,
    sort: InventoryBalanceSort,
    direction: InventorySortDirection,
    movable_only: bool,
    offset: u64,
) -> AppResult<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{}.{offset:016x}",
        cursor_filter(query, sort, direction, movable_only)
    ))
    .map_err(|_| AppError::internal("generated an invalid inventory balance cursor"))
}

fn cursor_filter(
    query: Option<&str>,
    sort: InventoryBalanceSort,
    direction: InventorySortDirection,
    movable_only: bool,
) -> String {
    let query = query.map_or_else(
        || "-".to_owned(),
        |value| {
            value
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        },
    );
    format!(
        "{}.{}.{}.{}",
        sort_key(sort),
        direction_key(direction),
        u8::from(movable_only),
        query
    )
}

fn map_sort(value: InventoryBalanceSort) -> ApplicationInventoryBalanceSort {
    match value {
        InventoryBalanceSort::Position => ApplicationInventoryBalanceSort::Position,
        InventoryBalanceSort::Facility => ApplicationInventoryBalanceSort::Facility,
        InventoryBalanceSort::Client => ApplicationInventoryBalanceSort::Client,
        InventoryBalanceSort::Location => ApplicationInventoryBalanceSort::Location,
        InventoryBalanceSort::Item => ApplicationInventoryBalanceSort::Item,
        InventoryBalanceSort::Tracking => ApplicationInventoryBalanceSort::Tracking,
        InventoryBalanceSort::LicensePlate => ApplicationInventoryBalanceSort::LicensePlate,
        InventoryBalanceSort::Status => ApplicationInventoryBalanceSort::Status,
        InventoryBalanceSort::OnHand => ApplicationInventoryBalanceSort::OnHand,
        InventoryBalanceSort::Reserved => ApplicationInventoryBalanceSort::Reserved,
        InventoryBalanceSort::Held => ApplicationInventoryBalanceSort::Held,
        InventoryBalanceSort::Available => ApplicationInventoryBalanceSort::Available,
    }
}

fn map_direction(value: InventorySortDirection) -> ApplicationSortDirection {
    match value {
        InventorySortDirection::Ascending => ApplicationSortDirection::Ascending,
        InventorySortDirection::Descending => ApplicationSortDirection::Descending,
    }
}

fn sort_key(value: InventoryBalanceSort) -> &'static str {
    match value {
        InventoryBalanceSort::Position => "position",
        InventoryBalanceSort::Facility => "facility",
        InventoryBalanceSort::Client => "client",
        InventoryBalanceSort::Location => "location",
        InventoryBalanceSort::Item => "item",
        InventoryBalanceSort::Tracking => "tracking",
        InventoryBalanceSort::LicensePlate => "license_plate",
        InventoryBalanceSort::Status => "status",
        InventoryBalanceSort::OnHand => "on_hand",
        InventoryBalanceSort::Reserved => "reserved",
        InventoryBalanceSort::Held => "held",
        InventoryBalanceSort::Available => "available",
    }
}

fn direction_key(value: InventorySortDirection) -> &'static str {
    match value {
        InventorySortDirection::Ascending => "asc",
        InventorySortDirection::Descending => "desc",
    }
}

fn map_balance(row: InventoryBalanceReadModel) -> InventoryBalanceResponse {
    let status = match row.status {
        ApplicationInventoryBalanceStatus::Available => InventoryBalanceStatus::Available,
        ApplicationInventoryBalanceStatus::Hold => InventoryBalanceStatus::Hold,
        ApplicationInventoryBalanceStatus::Damaged => InventoryBalanceStatus::Damaged,
        ApplicationInventoryBalanceStatus::Quarantine => InventoryBalanceStatus::Quarantine,
    };
    let quantity = row.quantity;

    InventoryBalanceResponse {
        id: row.id,
        inventory_owner_id: row.inventory_owner_id.get(),
        inventory_owner_name: row.inventory_owner_name,
        facility_id: row.facility_id.get(),
        facility_name: row.facility_name,
        location_id: row.location_id,
        location_name: row.location_name,
        location_barcode: row.location_barcode,
        license_plate_id: row.license_plate_id,
        license_plate_barcode: row.license_plate_barcode,
        item_batch_id: row.item_batch_id,
        item_id: row.item_id,
        item_description: row.item_description,
        primary_sku: row.primary_sku,
        lot: row.lot,
        serial: row.serial,
        uom: row.uom,
        status,
        quantity: InventoryQuantity {
            on_hand: quantity.on_hand(),
            reserved: quantity.reserved(),
            held: quantity.held(),
            available: quantity.available(),
        },
    }
}
