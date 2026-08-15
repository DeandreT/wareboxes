use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, Response};
use axum::Json;
use wareboxes_api_contract::v1::{
    CustomerPortalDocumentResponse, CustomerPortalDocumentType, CustomerPortalInventoryResponse,
    CustomerPortalOrderResponse, CustomerPortalOrderStatus, CustomerPortalShipmentResponse,
    CustomerPortalShipmentStatus, CustomerPortalWorkspaceRequest, CustomerPortalWorkspaceResponse,
    ErrorResponse,
};
use wareboxes_application::customer_portal::{
    CustomerPortalDocument, CustomerPortalInventoryLine, CustomerPortalOrder, CustomerPortalQuery,
    CustomerPortalShipment, CustomerPortalWorkspace, CUSTOMER_PORTAL_PERMISSION,
};
use wareboxes_domain::{OrderStatus, ShipmentDocumentType, ShipmentStatus};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::state::AppState;

/// Read the customer-facing inventory and fulfillment workspace.
#[cfg_attr(feature = "openapi", utoipa::path(
    get,
    path = "/api/v1/portal/workspace",
    operation_id = "getCustomerPortalWorkspace",
    tag = "Visibility",
    params(
        CustomerPortalWorkspaceRequest,
        ("x-wareboxes-tenant-id" = i64, Header, description = "Positive tenant context for the bearer credential.", minimum = 1)
    ),
    responses(
        (status = 200, description = "Owner- and facility-scoped customer visibility projection.", body = CustomerPortalWorkspaceResponse),
        (status = 400, description = "Invalid filter.", body = ErrorResponse),
        (status = 401, description = "Missing or invalid credential.", body = ErrorResponse),
        (status = 403, description = "The identity lacks customer portal permission or requested scope.", body = ErrorResponse)
    ),
    security(("bearerAuth" = []))
))]
pub async fn workspace(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<CustomerPortalWorkspaceRequest>,
) -> V1Result<Json<CustomerPortalWorkspaceResponse>> {
    user.require_permission(&state.db, CUSTOMER_PORTAL_PERMISSION)
        .await?;
    let query = application_query(query)?;
    let workspace = repo::customer_portal::workspace(&state.db, &user.tenant, &query).await?;
    Ok(Json(map_workspace(workspace)))
}

/// Download one immutable shipment document visible in the identity's scopes.
#[cfg_attr(feature = "openapi", utoipa::path(
    get,
    path = "/api/v1/portal/documents/{document_id}/content",
    operation_id = "downloadCustomerPortalDocument",
    tag = "Visibility",
    params(
        ("document_id" = i64, Path, description = "Shipment document identifier.", minimum = 1),
        ("x-wareboxes-tenant-id" = i64, Header, description = "Positive tenant context for the bearer credential.", minimum = 1)
    ),
    responses(
        (status = 200, description = "Immutable shipment document bytes.", content_type = "text/html"),
        (status = 401, description = "Missing or invalid credential.", body = ErrorResponse),
        (status = 403, description = "The identity lacks customer portal permission.", body = ErrorResponse),
        (status = 404, description = "The document is absent or outside the identity's owner/facility scopes.", body = ErrorResponse)
    ),
    security(("bearerAuth" = []))
))]
pub async fn download_document(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(document_id): Path<i64>,
) -> V1Result<Response<Body>> {
    user.require_permission(&state.db, CUSTOMER_PORTAL_PERMISSION)
        .await?;
    let result =
        repo::customer_portal::document_content(&state.db, &user.tenant, document_id).await?;
    let disposition = format!("attachment; filename=\"{}\"", result.document.file_name);
    let mut response = Response::new(Body::from(result.content));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&result.document.media_type)
            .map_err(|_| V1Error::internal("shipment document media type is invalid"))?,
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition)
            .map_err(|_| V1Error::internal("shipment document file name is invalid"))?,
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&result.document.content_length.to_string())
            .map_err(|_| V1Error::internal("shipment document length is invalid"))?,
    );
    Ok(response)
}

/// Export the currently visible inventory availability as CSV.
#[cfg_attr(feature = "openapi", utoipa::path(
    get,
    path = "/api/v1/portal/reports/inventory.csv",
    operation_id = "exportCustomerPortalInventory",
    tag = "Visibility",
    params(
        CustomerPortalWorkspaceRequest,
        ("x-wareboxes-tenant-id" = i64, Header, description = "Positive tenant context for the bearer credential.", minimum = 1)
    ),
    responses(
        (status = 200, description = "Scope-safe inventory availability CSV.", content_type = "text/csv"),
        (status = 401, description = "Missing or invalid credential.", body = ErrorResponse),
        (status = 403, description = "The identity lacks customer portal permission or requested scope.", body = ErrorResponse)
    ),
    security(("bearerAuth" = []))
))]
pub async fn inventory_report(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<CustomerPortalWorkspaceRequest>,
) -> V1Result<Response<Body>> {
    user.require_permission(&state.db, CUSTOMER_PORTAL_PERMISSION)
        .await?;
    let query = application_query(query)?;
    let workspace = repo::customer_portal::workspace(&state.db, &user.tenant, &query).await?;
    let mut response = Response::new(Body::from(inventory_csv(&workspace.inventory)));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"inventory-availability.csv\""),
    );
    Ok(response)
}

fn application_query(query: CustomerPortalWorkspaceRequest) -> V1Result<CustomerPortalQuery> {
    let search = query
        .search
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if search
        .as_ref()
        .is_some_and(|value| value.chars().count() > 100 || value.chars().any(char::is_control))
    {
        return Err(AppError::bad_request(
            "portal search cannot exceed 100 characters or contain control characters",
        )
        .into());
    }
    Ok(CustomerPortalQuery {
        inventory_owner_id: query.inventory_owner_id,
        facility_id: query.facility_id,
        search,
        include_history: query.include_history,
    })
}

fn map_workspace(workspace: CustomerPortalWorkspace) -> CustomerPortalWorkspaceResponse {
    CustomerPortalWorkspaceResponse {
        generated_at: chrono::Utc::now().to_rfc3339(),
        inventory: workspace.inventory.into_iter().map(map_inventory).collect(),
        orders: workspace.orders.into_iter().map(map_order).collect(),
        shipments: workspace.shipments.into_iter().map(map_shipment).collect(),
        documents: workspace.documents.into_iter().map(map_document).collect(),
        inventory_report_path: "/api/v1/portal/reports/inventory.csv".into(),
    }
}

fn map_inventory(value: CustomerPortalInventoryLine) -> CustomerPortalInventoryResponse {
    CustomerPortalInventoryResponse {
        inventory_owner_id: value.inventory_owner_id,
        inventory_owner_name: value.inventory_owner_name,
        facility_id: value.facility_id,
        facility_name: value.facility_name,
        item_id: value.item_id,
        item_description: value.item_description,
        primary_sku: value.primary_sku,
        lot: value.lot,
        expiration: value.expiration.map(|timestamp| timestamp.to_rfc3339()),
        uom: value.uom,
        status: value.status,
        on_hand: value.on_hand,
        reserved: value.reserved,
        held: value.held,
        available: value.available,
    }
}

fn map_order(value: CustomerPortalOrder) -> CustomerPortalOrderResponse {
    CustomerPortalOrderResponse {
        order_id: value.order_id,
        order_key: value.order_key,
        inventory_owner_id: value.inventory_owner_id,
        inventory_owner_name: value.inventory_owner_name,
        facility_id: value.facility_id,
        facility_name: value.facility_name,
        status: match value.status {
            OrderStatus::Open => CustomerPortalOrderStatus::Open,
            OrderStatus::Held => CustomerPortalOrderStatus::Held,
            OrderStatus::Processing => CustomerPortalOrderStatus::Processing,
            OrderStatus::AwaitingPacking => CustomerPortalOrderStatus::AwaitingPacking,
            OrderStatus::Packing => CustomerPortalOrderStatus::Packing,
            OrderStatus::AwaitingShipment => CustomerPortalOrderStatus::AwaitingShipment,
            OrderStatus::Shipped => CustomerPortalOrderStatus::Shipped,
            OrderStatus::Cancelled => CustomerPortalOrderStatus::Cancelled,
            OrderStatus::Void => CustomerPortalOrderStatus::Void,
        },
        rush: value.rush,
        ordered_quantity: value.ordered_quantity,
        ship_by: value.ship_by.map(|timestamp| timestamp.to_rfc3339()),
        created_at: value.created_at.to_rfc3339(),
        destination_company: value.destination_company,
        destination_city: value.destination_city,
        destination_region: value.destination_region,
        destination_country: value.destination_country,
    }
}

fn map_shipment(value: CustomerPortalShipment) -> CustomerPortalShipmentResponse {
    CustomerPortalShipmentResponse {
        shipment_id: value.shipment_id,
        order_id: value.order_id,
        order_key: value.order_key,
        inventory_owner_id: value.inventory_owner_id,
        inventory_owner_name: value.inventory_owner_name,
        facility_id: value.facility_id,
        facility_name: value.facility_name,
        status: match value.status {
            ShipmentStatus::AwaitingManifest => CustomerPortalShipmentStatus::AwaitingManifest,
            ShipmentStatus::Manifested => CustomerPortalShipmentStatus::Manifested,
            ShipmentStatus::PartiallyDeparted => CustomerPortalShipmentStatus::PartiallyDeparted,
            ShipmentStatus::Departed => CustomerPortalShipmentStatus::Departed,
            ShipmentStatus::Cancelled => CustomerPortalShipmentStatus::Cancelled,
        },
        carton_count: value.carton_count,
        shipped_quantity: value.shipped_quantity,
        carrier: value.carrier,
        service: value.service,
        tracking_numbers: value.tracking_numbers,
        created_at: value.created_at.to_rfc3339(),
        manifested_at: value.manifested_at.map(|timestamp| timestamp.to_rfc3339()),
        departed_at: value.departed_at.map(|timestamp| timestamp.to_rfc3339()),
    }
}

fn map_document(value: CustomerPortalDocument) -> CustomerPortalDocumentResponse {
    let document_id = value.document_id;
    CustomerPortalDocumentResponse {
        document_id,
        shipment_id: value.shipment_id,
        order_id: value.order_id,
        order_key: value.order_key,
        inventory_owner_id: value.inventory_owner_id,
        facility_id: value.facility_id,
        document_type: match value.document_type {
            ShipmentDocumentType::PackingSlip => CustomerPortalDocumentType::PackingSlip,
            ShipmentDocumentType::CartonLabelSet => CustomerPortalDocumentType::CartonLabelSet,
        },
        file_name: value.file_name,
        media_type: value.media_type,
        content_length: value.content_length,
        content_sha256: value.content_sha256,
        generated_at: value.generated_at.to_rfc3339(),
        download_path: format!("/api/v1/portal/documents/{document_id}/content"),
    }
}

fn inventory_csv(lines: &[CustomerPortalInventoryLine]) -> String {
    let mut csv = String::from(
        "inventory_owner,facility,item_id,sku,description,lot,expiration,uom,status,on_hand,reserved,held,available\r\n",
    );
    for line in lines {
        let fields = [
            csv_field(&line.inventory_owner_name),
            csv_field(&line.facility_name),
            line.item_id.to_string(),
            csv_field(line.primary_sku.as_deref().unwrap_or_default()),
            csv_field(line.item_description.as_deref().unwrap_or_default()),
            csv_field(line.lot.as_deref().unwrap_or_default()),
            line.expiration
                .map(|timestamp| timestamp.to_rfc3339())
                .unwrap_or_default(),
            csv_field(&line.uom),
            csv_field(&line.status),
            line.on_hand.to_string(),
            line.reserved.to_string(),
            line.held.to_string(),
            line.available.to_string(),
        ];
        csv.push_str(&fields.join(","));
        csv.push_str("\r\n");
    }
    csv
}

fn csv_field(value: &str) -> String {
    let safe = if value.starts_with(['=', '+', '-', '@']) {
        format!("'{value}")
    } else {
        value.to_owned()
    };
    format!("\"{}\"", safe.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_fields_quote_delimiters_and_neutralize_formulas() {
        assert_eq!(csv_field("north,west"), "\"north,west\"");
        assert_eq!(csv_field("=1+1"), "\"'=1+1\"");
        assert_eq!(csv_field("a\"b"), "\"a\"\"b\"");
    }
}
