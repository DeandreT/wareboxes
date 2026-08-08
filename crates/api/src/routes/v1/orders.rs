use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use wareboxes_api_contract::v1::{
    CreateFulfillmentOrderRequest, CreateFulfillmentOrderResponse, CreatedFulfillmentOrderLine,
    CreatedFulfillmentOrderStatus, OrderEntryItemResponse, Revision,
};
use wareboxes_domain::{
    CatalogItemId, FulfillmentOrderDemandLine, InventoryOwnerId, NewFulfillmentOrder, OrderKey,
    OrderLineKey, OrderQuantity, OrderStatus, RequestedUom, ShippingDestination, Timestamp,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "orders";
const DEFAULT_ITEM_LIMIT: i64 = 50;
const MAX_ITEM_LIMIT: i64 = 100;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderEntryItemQuery {
    pub search: Option<String>,
    pub limit: Option<i64>,
}

pub async fn create(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreateFulfillmentOrderRequest>,
) -> V1Result<Json<CreateFulfillmentOrderResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let order = new_fulfillment_order(body)?;
    let context = user.command_context(&idempotency_key);
    let result =
        repo::order_creation::create_fulfillment_order(&state.db, &user.tenant, &context, &order)
            .await?;

    let status = match result.status {
        OrderStatus::Open => CreatedFulfillmentOrderStatus::Open,
        _ => {
            return Err(V1Error::internal(
                "fulfillment order creation produced an invalid initial status",
            ));
        }
    };
    let revision = Revision::new(result.revision).map_err(|_| {
        V1Error::internal("fulfillment order creation produced an invalid revision")
    })?;
    Ok(Json(CreateFulfillmentOrderResponse {
        order_id: result.order_id,
        order_key: result.order_key,
        status,
        revision,
        lines: result
            .lines
            .into_iter()
            .map(|line| CreatedFulfillmentOrderLine {
                order_line_id: line.order_line_id,
                line_key: line.line_key,
            })
            .collect(),
    }))
}

pub async fn entry_items(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(inventory_owner_id): Path<i64>,
    Query(query): Query<OrderEntryItemQuery>,
) -> V1Result<Json<Vec<OrderEntryItemResponse>>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let inventory_owner_id =
        InventoryOwnerId::new(inventory_owner_id).map_err(|error| invalid(error.to_string()))?;
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if search.is_some_and(|value| value.chars().count() > 200) {
        return Err(invalid("item search cannot exceed 200 characters"));
    }
    let limit = query
        .limit
        .unwrap_or(DEFAULT_ITEM_LIMIT)
        .clamp(1, MAX_ITEM_LIMIT);
    let items = repo::order_creation::order_entry_items(
        &state.db,
        &user.tenant,
        inventory_owner_id,
        search,
        limit,
    )
    .await?
    .ok_or_else(|| AppError::not_found("inventory owner"))?;

    Ok(Json(
        items
            .into_iter()
            .map(|item| OrderEntryItemResponse {
                item_id: item.item_id,
                description: item.description,
                requested_uom: item.requested_uom,
            })
            .collect(),
    ))
}

fn new_fulfillment_order(request: CreateFulfillmentOrderRequest) -> V1Result<NewFulfillmentOrder> {
    let inventory_owner_id = InventoryOwnerId::new(request.inventory_owner_id)
        .map_err(|error| invalid(error.to_string()))?;
    let order_key = OrderKey::new(request.order_key).map_err(domain_validation)?;
    let ship_by = parse_timestamp(request.ship_by.as_deref(), "ship_by")?;
    let destination = ShippingDestination::new(
        request.destination.line1,
        request.destination.line2,
        request.destination.city,
        request.destination.region,
        request.destination.postal_code,
        request.destination.country,
    )
    .map_err(domain_validation)?;
    let lines = request
        .lines
        .into_iter()
        .map(|line| {
            Ok(FulfillmentOrderDemandLine::new(
                OrderLineKey::new(line.line_key).map_err(domain_validation)?,
                CatalogItemId::new(line.item_id).map_err(domain_validation)?,
                OrderQuantity::new(line.quantity).map_err(domain_validation)?,
                RequestedUom::new(line.requested_uom).map_err(domain_validation)?,
            ))
        })
        .collect::<V1Result<Vec<_>>>()?;

    NewFulfillmentOrder::new(
        inventory_owner_id,
        order_key,
        request.rush,
        ship_by,
        destination,
        lines,
    )
    .map_err(domain_validation)
}

fn parse_timestamp(value: Option<&str>, field: &str) -> V1Result<Option<Timestamp>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.trim() != value || value.is_empty() {
        return Err(invalid(format!(
            "{field} must be a nonempty RFC3339 timestamp"
        )));
    }
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| Some(timestamp.with_timezone(&Utc)))
        .map_err(|_| invalid(format!("{field} must be an RFC3339 timestamp")))
}

fn domain_validation(error: impl std::fmt::Display) -> V1Error {
    invalid(error.to_string())
}

fn invalid(message: impl Into<String>) -> V1Error {
    AppError::bad_request(message).into()
}

#[cfg(test)]
mod tests {
    use wareboxes_api_contract::v1::{
        CreateFulfillmentOrderLineRequest, FulfillmentOrderDestination,
    };

    use super::*;

    fn request() -> CreateFulfillmentOrderRequest {
        CreateFulfillmentOrderRequest {
            inventory_owner_id: 7,
            order_key: "SO-1001".into(),
            rush: true,
            ship_by: Some("2027-08-12T10:00:00-07:00".into()),
            destination: FulfillmentOrderDestination {
                line1: "125 Shipping Lane".into(),
                line2: Some("Dock 4".into()),
                city: "Reno".into(),
                region: "NV".into(),
                postal_code: "89502".into(),
                country: "US".into(),
            },
            lines: vec![CreateFulfillmentOrderLineRequest {
                line_key: "1".into(),
                item_id: 41,
                quantity: 12,
                requested_uom: "case".into(),
            }],
        }
    }

    #[test]
    fn converts_a_valid_request_to_the_domain_command() {
        let order = new_fulfillment_order(request()).expect("valid order");

        assert_eq!(order.inventory_owner_id().get(), 7);
        assert_eq!(order.order_key().as_str(), "SO-1001");
        assert!(order.rush());
        assert_eq!(
            order.ship_by().map(Timestamp::to_rfc3339).as_deref(),
            Some("2027-08-12T17:00:00+00:00")
        );
        assert_eq!(order.demand_lines()[0].item_id().get(), 41);
    }

    #[test]
    fn rejects_an_invalid_timestamp_before_persistence() {
        let mut request = request();
        request.ship_by = Some("2027-08-12 17:00:00".into());

        assert!(new_fulfillment_order(request).is_err());
    }

    #[test]
    fn rejects_duplicate_line_keys_before_persistence() {
        let mut request = request();
        request.lines.push(CreateFulfillmentOrderLineRequest {
            line_key: "1".into(),
            item_id: 42,
            quantity: 1,
            requested_uom: "each".into(),
        });

        assert!(new_fulfillment_order(request).is_err());
    }
}
