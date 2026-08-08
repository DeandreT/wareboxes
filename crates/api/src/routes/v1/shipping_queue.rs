use axum::extract::{Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    OpaqueCursor, Revision, ShipmentStatus as ApiShipmentStatus, ShippingQueueEntryResponse,
    ShippingQueuePage as ApiShippingQueuePage, ShippingQueuePageRequest,
    ShippingQueueShipmentResponse,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{OrderId, ShipmentStatus};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const CURSOR_PREFIX: &str = "sq1.";

pub async fn queue(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<ShippingQueuePageRequest>,
) -> V1Result<Json<ApiShippingQueuePage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let after = query.cursor.as_ref().map(decode_cursor).transpose()?;
    let facility_id = query.facility_id.map(|id| id.get());
    if after
        .as_ref()
        .is_some_and(|cursor| cursor.facility_id != facility_id)
    {
        return Err(V1Error::invalid_cursor_for("shipping queue"));
    }
    Ok(Json(
        page_for_access(
            &state,
            &user.tenant,
            facility_id,
            after.as_ref(),
            query.limit.get(),
        )
        .await?,
    ))
}

pub(crate) async fn page_for_access(
    state: &AppState,
    access: &TenantAccess,
    facility_id: Option<i64>,
    after: Option<&repo::shipping::ShippingQueueCursor>,
    limit: u16,
) -> AppResult<ApiShippingQueuePage> {
    let page = repo::shipping::shipping_queue(&state.db, access, facility_id, after, limit).await?;
    let items = page
        .items
        .into_iter()
        .map(map_entry)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = page.next_cursor.map(encode_cursor).transpose()?;
    Ok(ApiShippingQueuePage::new(items, next_cursor))
}

fn map_entry(entry: repo::shipping::ShippingQueueEntry) -> AppResult<ShippingQueueEntryResponse> {
    let shipment = entry
        .shipment
        .map(|shipment| {
            Ok::<ShippingQueueShipmentResponse, AppError>(ShippingQueueShipmentResponse {
                shipment_id: shipment.shipment_id.get(),
                status: map_status(shipment.status),
                revision: Revision::new(shipment.revision.get())
                    .map_err(|error| AppError::internal(error.to_string()))?,
                carton_count: shipment.carton_count,
                shipped_quantity: shipment.shipped_quantity,
                carrier_code: shipment.carrier_code,
                service_code: shipment.service_code,
                created_at: shipment.created_at.to_rfc3339(),
                manifested_at: shipment.manifested_at.map(|value| value.to_rfc3339()),
            })
        })
        .transpose()?;
    Ok(ShippingQueueEntryResponse {
        order_id: entry.order_id.get(),
        order_key: entry.order_key,
        order_revision: Revision::new(entry.order_revision.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: entry.inventory_owner_id,
        inventory_owner_name: entry.inventory_owner_name,
        facility_id: entry.facility_id,
        facility_name: entry.facility_name,
        facility_revision: Revision::new(entry.facility_revision.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        packing_session_id: entry.packing_session_id.get(),
        rush: entry.rush,
        ship_by: entry.ship_by.map(|value| value.to_rfc3339()),
        origin_ready: entry.origin_ready,
        destination_ready: entry.destination_ready,
        shipment,
    })
}

fn map_status(status: ShipmentStatus) -> ApiShipmentStatus {
    match status {
        ShipmentStatus::AwaitingManifest => ApiShipmentStatus::AwaitingManifest,
        ShipmentStatus::Manifested => ApiShipmentStatus::Manifested,
        ShipmentStatus::Departed => ApiShipmentStatus::Departed,
    }
}

fn decode_cursor(cursor: &OpaqueCursor) -> V1Result<repo::shipping::ShippingQueueCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("shipping queue"))?;
    let parts = encoded.split('.').collect::<Vec<_>>();
    if parts.len() != 4 {
        return Err(V1Error::invalid_cursor_for("shipping queue"));
    }
    let facility_id = match parts[0] {
        "a" => None,
        encoded if encoded.len() == 16 => Some(
            i64::from_str_radix(encoded, 16)
                .ok()
                .filter(|id| *id > 0)
                .ok_or_else(|| V1Error::invalid_cursor_for("shipping queue"))?,
        ),
        _ => return Err(V1Error::invalid_cursor_for("shipping queue")),
    };
    let rush_rank = match parts[1] {
        "0" => 0,
        "1" => 1,
        _ => return Err(V1Error::invalid_cursor_for("shipping queue")),
    };
    let ship_by = match parts[2] {
        "n" => None,
        encoded if encoded.len() == 17 && encoded.starts_with('t') => {
            let sortable = u64::from_str_radix(&encoded[1..], 16)
                .map_err(|_| V1Error::invalid_cursor_for("shipping queue"))?;
            let micros = (sortable ^ (1_u64 << 63)) as i64;
            Some(
                chrono::DateTime::<chrono::Utc>::from_timestamp_micros(micros)
                    .ok_or_else(|| V1Error::invalid_cursor_for("shipping queue"))?,
            )
        }
        _ => return Err(V1Error::invalid_cursor_for("shipping queue")),
    };
    if parts[3].len() != 16 {
        return Err(V1Error::invalid_cursor_for("shipping queue"));
    }
    let order_id = i64::from_str_radix(parts[3], 16)
        .ok()
        .and_then(|id| OrderId::new(id).ok())
        .ok_or_else(|| V1Error::invalid_cursor_for("shipping queue"))?;
    Ok(repo::shipping::ShippingQueueCursor {
        facility_id,
        rush_rank,
        ship_by,
        order_id,
    })
}

fn encode_cursor(cursor: repo::shipping::ShippingQueueCursor) -> AppResult<OpaqueCursor> {
    if !matches!(cursor.rush_rank, 0 | 1) || cursor.facility_id.is_some_and(|id| id <= 0) {
        return Err(AppError::internal(
            "generated an invalid shipping queue cursor",
        ));
    }
    let ship_by = cursor.ship_by.map_or_else(
        || "n".to_owned(),
        |value| {
            let sortable = (value.timestamp_micros() as u64) ^ (1_u64 << 63);
            format!("t{sortable:016x}")
        },
    );
    let facility_id = cursor
        .facility_id
        .map_or_else(|| "a".to_owned(), |id| format!("{id:016x}"));
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{}.{}.{}.{:016x}",
        facility_id,
        cursor.rush_rank,
        ship_by,
        cursor.order_id.get()
    ))
    .map_err(|_| AppError::internal("generated an invalid shipping queue cursor"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipping_cursor_round_trips_scope_priority_due_date_and_identity() {
        let expected = repo::shipping::ShippingQueueCursor {
            facility_id: Some(8),
            rush_rank: 0,
            ship_by: Some("2026-08-09T04:00:00Z".parse().unwrap()),
            order_id: OrderId::new(42).unwrap(),
        };
        let encoded = encode_cursor(expected.clone()).unwrap();
        assert_eq!(decode_cursor(&encoded).unwrap(), expected);
    }

    #[test]
    fn shipping_cursor_rejects_other_resources_and_malformed_values() {
        for value in [
            "pq1.a.0.n.0000000000000001",
            "sq1.a.2.n.0000000000000001",
            "sq1.a.0.tffffffffffffffff.0000000000000000",
        ] {
            let cursor = OpaqueCursor::new(value).unwrap();
            assert!(decode_cursor(&cursor).is_err(), "{value}");
        }
    }
}
