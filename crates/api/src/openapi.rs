//! Explicitly allowlisted public API descriptions.

use axum::http::{header, HeaderValue};
use axum::response::IntoResponse;
use axum::Json;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::OpenApi as OpenApiDocument;
use utoipa::{Modify, OpenApi};

struct IntegrationSecurity;

impl Modify for IntegrationSecurity {
    fn modify(&self, openapi: &mut OpenApiDocument) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearerAuth",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .description(Some(
                            "Opaque bearer credential provisioned for the integration identity. It is not a JWT and must be stored as a secret.",
                        ))
                        .build(),
                ),
            );
        }
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Wareboxes Integration API",
        version = "1.0.0",
        description = "Version 1 partner integration surface for warehouse fulfillment workflows. Public operations are explicitly allowlisted; Wareboxes operator, RF, and administration endpoints are not part of this contract."
    ),
    paths(
        crate::routes::v1::integration_order_intake::receive_order,
        crate::routes::v1::integration_order_intake::receive_x12_940_order,
        crate::routes::v1::customer_portal::workspace,
        crate::routes::v1::customer_portal::download_document,
        crate::routes::v1::customer_portal::inventory_report
    ),
    modifiers(&IntegrationSecurity),
    tags((
        name = "Orders",
        description = "Submit external fulfillment demand using partner inventory-owner, item, and UOM identities."
    ),(
        name = "Visibility",
        description = "Read owner- and facility-scoped inventory, fulfillment status, reports, and documents."
    )),
    servers((
        url = "http://127.0.0.1:8080",
        description = "Local development. Hosted environment URLs are supplied during integration onboarding."
    ))
)]
struct IntegrationApiV1;

/// Builds the deterministic public Integration API v1 document.
pub fn integration_api_v1() -> OpenApiDocument {
    IntegrationApiV1::openapi()
}

/// Serves the same document exported into the Scalar developer portal.
pub async fn serve_integration_api_v1() -> impl IntoResponse {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    (headers, Json(integration_api_v1()))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use serde_json::Value;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use super::*;

    #[test]
    fn integration_contract_only_contains_allowlisted_partner_operations() {
        let document = serde_json::to_value(integration_api_v1())
            .expect("integration OpenAPI document should serialize");
        let paths = document["paths"]
            .as_object()
            .expect("integration OpenAPI paths should be an object");

        assert_eq!(paths.len(), 5);
        assert!(paths.contains_key(
            "/api/v1/integrations/order-intake/{source_key}/inventory-owners/{external_inventory_owner_key}/orders"
        ));
        assert!(paths.contains_key(
            "/api/v1/integrations/x12-940/{source_key}/inventory-owners/{external_inventory_owner_key}/orders"
        ));
        assert!(!document.to_string().contains("integration-monitor"));
        assert!(paths.contains_key("/api/v1/portal/workspace"));
        assert!(paths.contains_key("/api/v1/portal/documents/{document_id}/content"));
        assert!(paths.contains_key("/api/v1/portal/reports/inventory.csv"));
    }

    #[test]
    fn order_intake_contract_preserves_async_quarantine_semantics() {
        let document = serde_json::to_value(integration_api_v1())
            .expect("integration OpenAPI document should serialize");
        let operation = &document["paths"]
            ["/api/v1/integrations/order-intake/{source_key}/inventory-owners/{external_inventory_owner_key}/orders"]
            ["post"];

        assert_eq!(operation["operationId"], "submitIntegrationOrder");
        assert_eq!(
            operation["security"][0]["bearerAuth"],
            Value::from(Vec::<String>::new())
        );
        assert!(operation["responses"].get("202").is_some());
        assert!(operation["responses"].get("201").is_none());
        assert_eq!(document["openapi"], "3.1.0");
    }

    #[tokio::test]
    async fn public_document_is_served_without_authentication_or_database_access() {
        let db = PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .expect("lazy test database URL should parse");
        let response = crate::routes::app(crate::state::AppState::new(db))
            .oneshot(
                Request::builder()
                    .uri("/openapi/integrations/v1.json")
                    .body(Body::empty())
                    .expect("OpenAPI request should build"),
            )
            .await
            .expect("OpenAPI route should respond");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL),
            Some(&HeaderValue::from_static("public, max-age=300"))
        );
        let body = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("OpenAPI response should be readable");
        let document: Value =
            serde_json::from_slice(&body).expect("OpenAPI response should contain JSON");
        assert_eq!(document["info"]["title"], "Wareboxes Integration API");
    }
}
