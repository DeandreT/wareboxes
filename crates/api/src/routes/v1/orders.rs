use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use wareboxes_api_contract::v1::{
    AmendFulfillmentOrderRequest, AmendFulfillmentOrderResponse, AmendedFulfillmentOrderStatus,
    CreateFulfillmentOrderRequest, CreateFulfillmentOrderResponse, CreatedFulfillmentOrderLine,
    CreatedFulfillmentOrderStatus, FulfillmentOrderDestination, OrderEntryItemResponse,
    ReplaceFulfillmentOrderLinesRequest, ReplaceFulfillmentOrderLinesResponse,
    ReplacedFulfillmentOrderLineResponse, ReplacedFulfillmentOrderStatus, Revision,
};
use wareboxes_application::order_amendment::{
    AmendFulfillmentOrderCommand, AmendFulfillmentOrderResult,
};
use wareboxes_application::order_line_amendment::{
    ReplaceFulfillmentOrderLinesCommand, ReplaceFulfillmentOrderLinesResult, ReplacementOrderLine,
};
use wareboxes_domain::{
    CatalogItemId, FulfillmentOrderDemandLine, InventoryOwnerId, NewFulfillmentOrder, OrderId,
    OrderKey, OrderLineKey, OrderQuantity, OrderRevision, OrderStatus, RequestedUom,
    ShippingDestination, ShippingRecipient, Timestamp,
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

pub async fn amend(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(order_id): Path<i64>,
    Json(body): Json<AmendFulfillmentOrderRequest>,
) -> V1Result<Json<AmendFulfillmentOrderResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = amendment_command(order_id, body)?;
    let context = user.command_context(&idempotency_key);
    let result = repo::order_amendment::amend_fulfillment_order_header(
        &state.db,
        &user.tenant,
        &context,
        &command,
    )
    .await?;
    Ok(Json(amendment_response(result)?))
}

pub async fn replace_lines(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(order_id): Path<i64>,
    Json(body): Json<ReplaceFulfillmentOrderLinesRequest>,
) -> V1Result<Json<ReplaceFulfillmentOrderLinesResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = line_replacement_command(order_id, body)?;
    let context = user.command_context(&idempotency_key);
    let result = repo::order_line_amendment::replace_fulfillment_order_lines(
        &state.db,
        &user.tenant,
        &context,
        &command,
    )
    .await?;
    Ok(Json(line_replacement_response(result)?))
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

pub(crate) fn new_fulfillment_order(
    request: CreateFulfillmentOrderRequest,
) -> V1Result<NewFulfillmentOrder> {
    let inventory_owner_id = InventoryOwnerId::new(request.inventory_owner_id)
        .map_err(|error| invalid(error.to_string()))?;
    let order_key = OrderKey::new(request.order_key).map_err(domain_validation)?;
    let ship_by = parse_timestamp(request.ship_by.as_deref(), "ship_by")?;
    let destination = shipping_destination(request.destination)?;
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

fn amendment_command(
    order_id: i64,
    request: AmendFulfillmentOrderRequest,
) -> V1Result<AmendFulfillmentOrderCommand> {
    Ok(AmendFulfillmentOrderCommand::new(
        wareboxes_domain::OrderId::new(order_id).map_err(domain_validation)?,
        wareboxes_domain::OrderRevision::new(request.expected_revision.get())
            .map_err(domain_validation)?,
        request.rush,
        parse_timestamp(request.ship_by.as_deref(), "ship_by")?,
        shipping_destination(request.destination)?,
    ))
}

fn amendment_response(
    result: AmendFulfillmentOrderResult,
) -> V1Result<AmendFulfillmentOrderResponse> {
    let status = match result.order_status {
        OrderStatus::Open => AmendedFulfillmentOrderStatus::Open,
        OrderStatus::Held => AmendedFulfillmentOrderStatus::Held,
        _ => {
            return Err(V1Error::internal(
                "order amendment produced an invalid order status",
            ));
        }
    };
    Ok(AmendFulfillmentOrderResponse {
        amendment_id: result.amendment_id.get(),
        order_id: result.order_id.get(),
        inventory_owner_id: result.inventory_owner_id.get(),
        status,
        revision: Revision::new(result.revision.get())
            .map_err(|_| V1Error::internal("order amendment produced an invalid revision"))?,
        rush: result.rush,
        ship_by: result.ship_by.map(|value| value.to_rfc3339()),
        destination: destination_response(&result.destination),
        amended_by: result.amended_by.get(),
        amended_at: result.amended_at.to_rfc3339(),
    })
}

fn line_replacement_command(
    order_id: i64,
    request: ReplaceFulfillmentOrderLinesRequest,
) -> V1Result<ReplaceFulfillmentOrderLinesCommand> {
    let lines = request
        .lines
        .into_iter()
        .map(|line| {
            ReplacementOrderLine::new(
                line.line_key,
                line.item_id,
                line.quantity,
                line.requested_uom,
            )
            .map_err(domain_validation)
        })
        .collect::<V1Result<Vec<_>>>()?;
    Ok(ReplaceFulfillmentOrderLinesCommand::new(
        OrderId::new(order_id).map_err(domain_validation)?,
        OrderRevision::new(request.expected_revision.get()).map_err(domain_validation)?,
        lines,
    ))
}

fn line_replacement_response(
    result: ReplaceFulfillmentOrderLinesResult,
) -> V1Result<ReplaceFulfillmentOrderLinesResponse> {
    let order_status = match result.order_status {
        OrderStatus::Open => ReplacedFulfillmentOrderStatus::Open,
        OrderStatus::Held => ReplacedFulfillmentOrderStatus::Held,
        _ => {
            return Err(V1Error::internal(
                "line replacement produced an invalid order status",
            ));
        }
    };
    Ok(ReplaceFulfillmentOrderLinesResponse {
        amendment_id: result.amendment_id.get(),
        order_id: result.order_id.get(),
        inventory_owner_id: result.inventory_owner_id.get(),
        order_status,
        previous_revision: Revision::new(result.previous_revision.get())
            .map_err(|_| V1Error::internal("line replacement produced an invalid revision"))?,
        revision: Revision::new(result.revision.get())
            .map_err(|_| V1Error::internal("line replacement produced an invalid revision"))?,
        previous_line_count: result.previous_line_count,
        previous_quantity: result.previous_quantity,
        resulting_quantity: result.resulting_quantity,
        released_reservation_count: result.released_reservation_count,
        released_allocation_count: result.released_allocation_count,
        released_quantity: result.released_quantity,
        lines: result
            .lines
            .into_iter()
            .map(|line| ReplacedFulfillmentOrderLineResponse {
                order_line_id: line.order_line_id.get(),
                line_key: line.line_key,
                line_number: line.line_number,
                item_id: line.item_id.get(),
                quantity: line.quantity,
                requested_uom: line.requested_uom,
            })
            .collect(),
        amended_by: result.amended_by.get(),
        amended_at: result.amended_at.to_rfc3339(),
    })
}

fn shipping_destination(destination: FulfillmentOrderDestination) -> V1Result<ShippingDestination> {
    let recipient = ShippingRecipient::new(
        destination.recipient_name,
        destination.company,
        destination.phone,
        destination.email,
    )
    .map_err(domain_validation)?;
    ShippingDestination::new(
        recipient,
        destination.line1,
        destination.line2,
        destination.city,
        destination.region,
        destination.postal_code,
        destination.country,
    )
    .map_err(domain_validation)
}

fn destination_response(destination: &ShippingDestination) -> FulfillmentOrderDestination {
    FulfillmentOrderDestination {
        recipient_name: destination.recipient().name().to_owned(),
        company: destination.recipient().company().map(str::to_owned),
        phone: destination.recipient().phone().map(str::to_owned),
        email: destination.recipient().email().map(str::to_owned),
        line1: destination.line1().to_owned(),
        line2: destination.line2().map(str::to_owned),
        city: destination.city().to_owned(),
        region: destination.region().to_owned(),
        postal_code: destination.postal_code().to_owned(),
        country: destination.country().to_owned(),
    }
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
    use wareboxes_api_contract::v1::CreateFulfillmentOrderLineRequest;

    use super::*;

    fn request() -> CreateFulfillmentOrderRequest {
        CreateFulfillmentOrderRequest {
            inventory_owner_id: 7,
            order_key: "SO-1001".into(),
            rush: true,
            ship_by: Some("2027-08-12T10:00:00-07:00".into()),
            destination: FulfillmentOrderDestination {
                recipient_name: "Receiving Team".into(),
                company: Some("Northstar Retail".into()),
                phone: Some("+1 775 555 0100".into()),
                email: Some("receiving@example.com".into()),
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
        assert_eq!(order.destination().recipient().name(), "Receiving Team");
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

    #[test]
    fn constructs_a_path_bound_amendment_that_can_clear_ship_by() {
        let command = amendment_command(
            17,
            AmendFulfillmentOrderRequest {
                expected_revision: Revision::new(3).unwrap(),
                rush: false,
                ship_by: None,
                destination: request().destination,
            },
        )
        .unwrap();
        assert_eq!(command.order_id().get(), 17);
        assert_eq!(command.expected_revision().get(), 3);
        assert_eq!(command.ship_by(), None);
        assert_eq!(command.destination().recipient().name(), "Receiving Team");
    }

    #[test]
    fn constructs_a_path_bound_exact_line_replacement() {
        let command = line_replacement_command(
            17,
            ReplaceFulfillmentOrderLinesRequest {
                expected_revision: Revision::new(3).unwrap(),
                lines: vec![
                    wareboxes_api_contract::v1::ReplaceFulfillmentOrderLineRequest {
                        line_key: "replacement-1".into(),
                        item_id: 41,
                        quantity: 9,
                        requested_uom: "case".into(),
                    },
                ],
            },
        )
        .unwrap();
        assert_eq!(command.order_id().get(), 17);
        assert_eq!(command.expected_revision().get(), 3);
        assert_eq!(command.lines()[0].line_key(), "replacement-1");
        assert_eq!(command.lines()[0].quantity(), 9);
    }
}
