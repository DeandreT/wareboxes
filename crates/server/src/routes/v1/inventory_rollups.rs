use axum::extract::{Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    InventoryFacilityRollupPage, InventoryFacilityRollupResponse, InventoryItemRollupPage,
    InventoryItemRollupResponse, InventoryLocationRollupPage, InventoryLocationRollupResponse,
    InventoryQuantity, InventoryRollupPageRequest, InventoryRollupQuantity, OpaqueCursor,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::repo;
use crate::repo::inventory_rollup_v1::{
    FacilityRollupCursor, InventoryRollupQuantityColumns, ItemRollupCursor, LocationRollupCursor,
};
use crate::state::AppState;

const PERMISSION: &str = "wms";
const LOCATION_CURSOR_PREFIX: &str = "irl1.";
const FACILITY_CURSOR_PREFIX: &str = "irf1.";
const ITEM_CURSOR_PREFIX: &str = "iri1.";

pub async fn list_by_location(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<InventoryRollupPageRequest>,
) -> V1Result<Json<InventoryLocationRollupPage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let after = query
        .cursor
        .as_ref()
        .map(decode_location_cursor)
        .transpose()?;
    let page = repo::inventory_rollup_v1::get_inventory_location_rollup_page(
        &state.db,
        &user.tenant,
        after,
        query.limit.get(),
    )
    .await?;
    let items = page
        .rows
        .into_iter()
        .map(|row| {
            Ok(InventoryLocationRollupResponse {
                inventory_owner_id: row.inventory_owner_id,
                inventory_owner_name: row.inventory_owner_name,
                item_id: row.item_id,
                item_description: row.item_description,
                primary_sku: row.primary_sku,
                facility_id: row.facility_id,
                facility_name: row.facility_name,
                location_id: row.location_id,
                location_name: row.location_name,
                location_barcode: row.location_barcode,
                quantities: map_quantities(row.quantities)?,
                balance_count: validate_count(row.balance_count)?,
                batch_count: validate_count(row.batch_count)?,
            })
        })
        .collect::<V1Result<Vec<_>>>()?;
    let next_cursor = page.next_cursor.map(encode_location_cursor).transpose()?;

    Ok(Json(InventoryLocationRollupPage::new(items, next_cursor)))
}

pub async fn list_by_facility(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<InventoryRollupPageRequest>,
) -> V1Result<Json<InventoryFacilityRollupPage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let after = query
        .cursor
        .as_ref()
        .map(decode_facility_cursor)
        .transpose()?;
    let page = repo::inventory_rollup_v1::get_inventory_facility_rollup_page(
        &state.db,
        &user.tenant,
        after,
        query.limit.get(),
    )
    .await?;
    let items = page
        .rows
        .into_iter()
        .map(|row| {
            Ok(InventoryFacilityRollupResponse {
                inventory_owner_id: row.inventory_owner_id,
                inventory_owner_name: row.inventory_owner_name,
                item_id: row.item_id,
                item_description: row.item_description,
                primary_sku: row.primary_sku,
                facility_id: row.facility_id,
                facility_name: row.facility_name,
                quantities: map_quantities(row.quantities)?,
                balance_count: validate_count(row.balance_count)?,
                batch_count: validate_count(row.batch_count)?,
                location_count: validate_count(row.location_count)?,
            })
        })
        .collect::<V1Result<Vec<_>>>()?;
    let next_cursor = page.next_cursor.map(encode_facility_cursor).transpose()?;

    Ok(Json(InventoryFacilityRollupPage::new(items, next_cursor)))
}

pub async fn list_by_item(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<InventoryRollupPageRequest>,
) -> V1Result<Json<InventoryItemRollupPage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let after = query.cursor.as_ref().map(decode_item_cursor).transpose()?;
    let page = repo::inventory_rollup_v1::get_inventory_item_rollup_page(
        &state.db,
        &user.tenant,
        after,
        query.limit.get(),
    )
    .await?;
    let items = page
        .rows
        .into_iter()
        .map(|row| {
            Ok(InventoryItemRollupResponse {
                inventory_owner_id: row.inventory_owner_id,
                inventory_owner_name: row.inventory_owner_name,
                item_id: row.item_id,
                item_description: row.item_description,
                primary_sku: row.primary_sku,
                quantities: map_quantities(row.quantities)?,
                balance_count: validate_count(row.balance_count)?,
                batch_count: validate_count(row.batch_count)?,
                location_count: validate_count(row.location_count)?,
                facility_count: validate_count(row.facility_count)?,
            })
        })
        .collect::<V1Result<Vec<_>>>()?;
    let next_cursor = page.next_cursor.map(encode_item_cursor).transpose()?;

    Ok(Json(InventoryItemRollupPage::new(items, next_cursor)))
}

fn map_quantities(
    columns: InventoryRollupQuantityColumns,
) -> V1Result<Vec<InventoryRollupQuantity>> {
    let InventoryRollupQuantityColumns {
        uoms,
        on_hand,
        reserved,
        held,
        available,
    } = columns;
    let count = uoms.len();
    if count == 0
        || on_hand.len() != count
        || reserved.len() != count
        || held.len() != count
        || available.len() != count
    {
        return Err(V1Error::internal(
            "inventory rollup quantity columns are inconsistent",
        ));
    }

    uoms.into_iter()
        .zip(on_hand)
        .zip(reserved)
        .zip(held)
        .zip(available)
        .map(
            |((((uom, on_hand), reserved), held), available)| -> V1Result<_> {
                let quantities_are_valid = on_hand
                    .checked_sub(reserved)
                    .and_then(|quantity| quantity.checked_sub(held))
                    .is_some_and(|quantity| quantity >= available && available >= 0);
                if uom.trim().is_empty() || !quantities_are_valid {
                    return Err(V1Error::internal(
                        "inventory rollup quantities are inconsistent",
                    ));
                }
                Ok(InventoryRollupQuantity {
                    uom,
                    quantity: InventoryQuantity {
                        on_hand,
                        reserved,
                        held,
                        available,
                    },
                })
            },
        )
        .collect()
}

fn validate_count(count: i64) -> V1Result<i64> {
    if count <= 0 {
        Err(V1Error::internal("inventory rollup count is invalid"))
    } else {
        Ok(count)
    }
}

fn decode_location_cursor(cursor: &OpaqueCursor) -> V1Result<LocationRollupCursor> {
    let values = decode_cursor(cursor, LOCATION_CURSOR_PREFIX, 3)?;
    Ok(LocationRollupCursor {
        inventory_owner_id: values[0],
        item_id: values[1],
        location_id: values[2],
    })
}

fn encode_location_cursor(cursor: LocationRollupCursor) -> V1Result<OpaqueCursor> {
    encode_cursor(
        LOCATION_CURSOR_PREFIX,
        &[
            cursor.inventory_owner_id,
            cursor.item_id,
            cursor.location_id,
        ],
    )
}

fn decode_facility_cursor(cursor: &OpaqueCursor) -> V1Result<FacilityRollupCursor> {
    let values = decode_cursor(cursor, FACILITY_CURSOR_PREFIX, 3)?;
    Ok(FacilityRollupCursor {
        inventory_owner_id: values[0],
        item_id: values[1],
        facility_id: values[2],
    })
}

fn encode_facility_cursor(cursor: FacilityRollupCursor) -> V1Result<OpaqueCursor> {
    encode_cursor(
        FACILITY_CURSOR_PREFIX,
        &[
            cursor.inventory_owner_id,
            cursor.item_id,
            cursor.facility_id,
        ],
    )
}

fn decode_item_cursor(cursor: &OpaqueCursor) -> V1Result<ItemRollupCursor> {
    let values = decode_cursor(cursor, ITEM_CURSOR_PREFIX, 2)?;
    Ok(ItemRollupCursor {
        inventory_owner_id: values[0],
        item_id: values[1],
    })
}

fn encode_item_cursor(cursor: ItemRollupCursor) -> V1Result<OpaqueCursor> {
    encode_cursor(
        ITEM_CURSOR_PREFIX,
        &[cursor.inventory_owner_id, cursor.item_id],
    )
}

fn decode_cursor(
    cursor: &OpaqueCursor,
    prefix: &str,
    expected_values: usize,
) -> V1Result<Vec<i64>> {
    let encoded = cursor
        .as_str()
        .strip_prefix(prefix)
        .ok_or_else(V1Error::invalid_cursor)?;
    let values = encoded
        .split('.')
        .map(|part| {
            if part.len() != 16 {
                return Err(V1Error::invalid_cursor());
            }
            i64::from_str_radix(part, 16)
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(V1Error::invalid_cursor)
        })
        .collect::<V1Result<Vec<_>>>()?;
    if values.len() != expected_values {
        return Err(V1Error::invalid_cursor());
    }
    Ok(values)
}

fn encode_cursor(prefix: &str, values: &[i64]) -> V1Result<OpaqueCursor> {
    if values.iter().any(|value| *value <= 0) {
        return Err(V1Error::internal(
            "generated an invalid inventory rollup cursor",
        ));
    }
    let encoded = values
        .iter()
        .map(|value| format!("{value:016x}"))
        .collect::<Vec<_>>()
        .join(".");
    OpaqueCursor::new(format!("{prefix}{encoded}"))
        .map_err(|_| V1Error::internal("generated an invalid inventory rollup cursor"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollup_cursors_round_trip_and_reject_cross_dimension_reuse() {
        let location = LocationRollupCursor {
            inventory_owner_id: 11,
            item_id: 22,
            location_id: 33,
        };
        let encoded = encode_location_cursor(location).unwrap();
        assert_eq!(decode_location_cursor(&encoded).unwrap(), location);
        assert!(decode_facility_cursor(&encoded).is_err());
        assert!(decode_item_cursor(&encoded).is_err());
    }

    #[test]
    fn quantity_mapping_rejects_misaligned_columns() {
        let result = map_quantities(InventoryRollupQuantityColumns {
            uoms: vec!["each".to_owned()],
            on_hand: vec![5],
            reserved: vec![],
            held: vec![0],
            available: vec![5],
        });
        assert!(result.is_err());
    }
}
