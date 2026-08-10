use axum::extract::{Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    InventoryFacilityRollupPage, InventoryFacilityRollupResponse, InventoryItemRollupPage,
    InventoryItemRollupResponse, InventoryLocationRollupPage, InventoryLocationRollupResponse,
    InventoryQuantity, InventoryRollupPageRequest, InventoryRollupQuantity, InventoryRollupSort,
    InventorySortDirection, OpaqueCursor,
};
use wareboxes_application::inventory::{
    InventoryRollupPageQuery, InventoryRollupQuantity as ApplicationInventoryRollupQuantity,
    InventoryRollupSort as ApplicationRollupSort,
    InventoryRollupSortDirection as ApplicationSortDirection,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const LOCATION_CURSOR_PREFIX: &str = "irl2.";
const FACILITY_CURSOR_PREFIX: &str = "irf2.";
const ITEM_CURSOR_PREFIX: &str = "iri2.";

pub async fn list_by_location(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<InventoryRollupPageRequest>,
) -> V1Result<Json<InventoryLocationRollupPage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    validate_rollup_sort(query.sort, false)?;
    let page_query = page_query(&query, LOCATION_CURSOR_PREFIX)?;
    let page =
        wareboxes_persistence_postgres::inventory_rollups::get_inventory_location_rollup_page(
            &state.db,
            user.tenant.tenant_id,
            &user.tenant.site_scope,
            &user.tenant.owner_scope,
            &page_query,
        )
        .await
        .map_err(AppError::from)?;
    let items = page
        .items
        .into_iter()
        .map(|row| InventoryLocationRollupResponse {
            inventory_owner_id: row.inventory_owner_id.get(),
            inventory_owner_name: row.inventory_owner_name,
            item_id: row.item_id,
            item_description: row.item_description,
            primary_sku: row.primary_sku,
            facility_id: row.facility_id.get(),
            facility_name: row.facility_name,
            location_id: row.location_id,
            location_name: row.location_name,
            location_barcode: row.location_barcode,
            quantities: map_quantities(row.quantities),
            balance_count: row.balance_count.get(),
            batch_count: row.batch_count.get(),
        })
        .collect();
    let next_cursor = page
        .next_offset
        .map(|offset| encode_cursor(&query, LOCATION_CURSOR_PREFIX, offset))
        .transpose()?;

    Ok(Json(InventoryLocationRollupPage::new(items, next_cursor)))
}

pub async fn list_by_facility(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<InventoryRollupPageRequest>,
) -> V1Result<Json<InventoryFacilityRollupPage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let page_query = page_query(&query, FACILITY_CURSOR_PREFIX)?;
    let page =
        wareboxes_persistence_postgres::inventory_rollups::get_inventory_facility_rollup_page(
            &state.db,
            user.tenant.tenant_id,
            &user.tenant.site_scope,
            &user.tenant.owner_scope,
            &page_query,
        )
        .await
        .map_err(AppError::from)?;
    let items = page
        .items
        .into_iter()
        .map(|row| InventoryFacilityRollupResponse {
            inventory_owner_id: row.inventory_owner_id.get(),
            inventory_owner_name: row.inventory_owner_name,
            item_id: row.item_id,
            item_description: row.item_description,
            primary_sku: row.primary_sku,
            facility_id: row.facility_id.get(),
            facility_name: row.facility_name,
            quantities: map_quantities(row.quantities),
            balance_count: row.balance_count.get(),
            batch_count: row.batch_count.get(),
            location_count: row.location_count.get(),
        })
        .collect();
    let next_cursor = page
        .next_offset
        .map(|offset| encode_cursor(&query, FACILITY_CURSOR_PREFIX, offset))
        .transpose()?;

    Ok(Json(InventoryFacilityRollupPage::new(items, next_cursor)))
}

pub async fn list_by_item(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<InventoryRollupPageRequest>,
) -> V1Result<Json<InventoryItemRollupPage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let page_query = page_query(&query, ITEM_CURSOR_PREFIX)?;
    let page = wareboxes_persistence_postgres::inventory_rollups::get_inventory_item_rollup_page(
        &state.db,
        user.tenant.tenant_id,
        &user.tenant.site_scope,
        &user.tenant.owner_scope,
        &page_query,
    )
    .await
    .map_err(AppError::from)?;
    let items = page
        .items
        .into_iter()
        .map(|row| InventoryItemRollupResponse {
            inventory_owner_id: row.inventory_owner_id.get(),
            inventory_owner_name: row.inventory_owner_name,
            item_id: row.item_id,
            item_description: row.item_description,
            primary_sku: row.primary_sku,
            quantities: map_quantities(row.quantities),
            balance_count: row.balance_count.get(),
            batch_count: row.batch_count.get(),
            location_count: row.location_count.get(),
            facility_count: row.facility_count.get(),
        })
        .collect();
    let next_cursor = page
        .next_offset
        .map(|offset| encode_cursor(&query, ITEM_CURSOR_PREFIX, offset))
        .transpose()?;

    Ok(Json(InventoryItemRollupPage::new(items, next_cursor)))
}

fn map_quantities(
    quantities: Vec<ApplicationInventoryRollupQuantity>,
) -> Vec<InventoryRollupQuantity> {
    quantities
        .into_iter()
        .map(|quantity| {
            let (uom, on_hand, reserved, held, available) = quantity.into_parts();
            InventoryRollupQuantity {
                uom,
                quantity: InventoryQuantity {
                    on_hand,
                    reserved,
                    held,
                    available,
                },
            }
        })
        .collect()
}

fn page_query(
    request: &InventoryRollupPageRequest,
    prefix: &str,
) -> V1Result<InventoryRollupPageQuery> {
    let query = validated_query(request.query.as_deref())?;
    Ok(InventoryRollupPageQuery {
        offset: decode_cursor(request, prefix)?,
        limit: request.limit.get(),
        query: query.map(str::to_owned),
        sort: map_sort(request.sort),
        direction: map_direction(request.direction),
    })
}

fn validated_query(query: Option<&str>) -> V1Result<Option<&str>> {
    if query.is_some_and(|value| {
        value.is_empty()
            || value.trim() != value
            || value.chars().count() > 200
            || value.chars().any(char::is_control)
    }) {
        return Err(AppError::bad_request("inventory rollup query is invalid").into());
    }
    Ok(query)
}

fn validate_rollup_sort(sort: InventoryRollupSort, locations_supported: bool) -> V1Result<()> {
    if sort == InventoryRollupSort::Locations && !locations_supported {
        return Err(AppError::bad_request(
            "location summary rows do not support location-count sorting",
        )
        .into());
    }
    Ok(())
}

const fn map_sort(sort: InventoryRollupSort) -> ApplicationRollupSort {
    match sort {
        InventoryRollupSort::Client => ApplicationRollupSort::Client,
        InventoryRollupSort::Item => ApplicationRollupSort::Item,
        InventoryRollupSort::Scope => ApplicationRollupSort::Scope,
        InventoryRollupSort::Balances => ApplicationRollupSort::Balances,
        InventoryRollupSort::Batches => ApplicationRollupSort::Batches,
        InventoryRollupSort::Locations => ApplicationRollupSort::Locations,
    }
}

const fn map_direction(direction: InventorySortDirection) -> ApplicationSortDirection {
    match direction {
        InventorySortDirection::Ascending => ApplicationSortDirection::Ascending,
        InventorySortDirection::Descending => ApplicationSortDirection::Descending,
    }
}

fn decode_cursor(request: &InventoryRollupPageRequest, prefix: &str) -> V1Result<u64> {
    let Some(cursor) = request.cursor.as_ref() else {
        return Ok(0);
    };
    let encoded = cursor
        .as_str()
        .strip_prefix(prefix)
        .ok_or_else(V1Error::invalid_cursor)?;
    let (filter, offset) = encoded
        .rsplit_once('.')
        .ok_or_else(V1Error::invalid_cursor)?;
    if filter != cursor_filter(request) || offset.len() != 16 {
        return Err(V1Error::invalid_cursor());
    }
    u64::from_str_radix(offset, 16).map_err(|_| V1Error::invalid_cursor())
}

fn encode_cursor(
    request: &InventoryRollupPageRequest,
    prefix: &str,
    offset: u64,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!("{prefix}{}.{offset:016x}", cursor_filter(request)))
        .map_err(|_| V1Error::internal("generated an invalid inventory rollup cursor"))
}

fn cursor_filter(request: &InventoryRollupPageRequest) -> String {
    let query = request.query.as_ref().map_or_else(
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
        "{}.{}.{}",
        sort_key(request.sort),
        direction_key(request.direction),
        query
    )
}

const fn sort_key(sort: InventoryRollupSort) -> &'static str {
    match sort {
        InventoryRollupSort::Client => "c",
        InventoryRollupSort::Item => "i",
        InventoryRollupSort::Scope => "s",
        InventoryRollupSort::Balances => "b",
        InventoryRollupSort::Batches => "t",
        InventoryRollupSort::Locations => "l",
    }
}

const fn direction_key(direction: InventorySortDirection) -> &'static str {
    match direction {
        InventorySortDirection::Ascending => "a",
        InventorySortDirection::Descending => "d",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollup_cursors_bind_dimension_filter_and_sort() {
        let request = InventoryRollupPageRequest {
            query: Some("widget".to_owned()),
            sort: InventoryRollupSort::Batches,
            direction: InventorySortDirection::Descending,
            ..InventoryRollupPageRequest::default()
        };
        let encoded = encode_cursor(&request, LOCATION_CURSOR_PREFIX, 250).unwrap();
        let mut bound = request.clone();
        bound.cursor = Some(encoded.clone());
        assert_eq!(decode_cursor(&bound, LOCATION_CURSOR_PREFIX).unwrap(), 250);
        assert!(decode_cursor(&bound, ITEM_CURSOR_PREFIX).is_err());

        bound.sort = InventoryRollupSort::Client;
        assert!(decode_cursor(&bound, LOCATION_CURSOR_PREFIX).is_err());
    }
}
