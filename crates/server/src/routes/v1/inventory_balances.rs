use axum::extract::{Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    InventoryBalancePage, InventoryBalancePageRequest, InventoryBalanceResponse,
    InventoryBalanceStatus, InventoryQuantity, OpaqueCursor,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::repo;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const CURSOR_PREFIX: &str = "ib1.";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<InventoryBalancePageRequest>,
) -> V1Result<Json<InventoryBalancePage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let after_id = query.cursor.as_ref().map(decode_cursor).transpose()?;
    let page = repo::inventory_v1::get_inventory_balance_page(
        &state.db,
        &user.tenant,
        after_id,
        query.limit.get(),
    )
    .await?;
    let items = page
        .rows
        .into_iter()
        .map(map_balance)
        .collect::<V1Result<Vec<_>>>()?;
    let next_cursor = page.next_after_id.map(encode_cursor).transpose()?;

    Ok(Json(InventoryBalancePage::new(items, next_cursor)))
}

fn decode_cursor(cursor: &OpaqueCursor) -> V1Result<i64> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .filter(|encoded| encoded.len() == 16)
        .ok_or_else(V1Error::invalid_cursor)?;
    let id = i64::from_str_radix(encoded, 16).map_err(|_| V1Error::invalid_cursor())?;
    if id <= 0 {
        return Err(V1Error::invalid_cursor());
    }
    Ok(id)
}

fn encode_cursor(id: i64) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!("{CURSOR_PREFIX}{id:016x}"))
        .map_err(|_| V1Error::internal("generated an invalid inventory balance cursor"))
}

fn map_balance(
    row: repo::inventory_v1::InventoryBalancePageRow,
) -> V1Result<InventoryBalanceResponse> {
    let status = match row.status.as_str() {
        "available" => InventoryBalanceStatus::Available,
        "hold" => InventoryBalanceStatus::Hold,
        "damaged" => InventoryBalanceStatus::Damaged,
        "quarantine" => InventoryBalanceStatus::Quarantine,
        _ => return Err(V1Error::internal("unknown inventory balance status")),
    };
    let available = row
        .qty_on_hand
        .checked_sub(row.qty_reserved)
        .filter(|quantity| *quantity >= 0)
        .ok_or_else(|| V1Error::internal("invalid inventory balance quantities"))?;

    Ok(InventoryBalanceResponse {
        id: row.id,
        inventory_owner_id: row.inventory_owner_id,
        facility_id: row.facility_id,
        facility_name: row.facility_name,
        location_id: row.location_id,
        license_plate_id: row.license_plate_id,
        item_batch_id: row.item_batch_id,
        item_id: row.item_id,
        uom: row.uom,
        status,
        quantity: InventoryQuantity {
            on_hand: row.qty_on_hand,
            reserved: row.qty_reserved,
            available,
        },
    })
}
