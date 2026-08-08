use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    ConfigureFacilityShippingOriginRequest, ConfigureFacilityShippingOriginResponse,
    FacilityShippingOriginResponse, Revision,
};
use wareboxes_application::facility_shipping_origin::{
    ConfigureFacilityShippingOriginCommand, ConfigureFacilityShippingOriginResult,
};
use wareboxes_domain::{FacilityId, FacilityRevision, FacilityShippingOrigin};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "admin";

pub async fn configure(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(facility_id): Path<i64>,
    Json(body): Json<ConfigureFacilityShippingOriginRequest>,
) -> V1Result<Json<ConfigureFacilityShippingOriginResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = command(facility_id, body)?;
    let context = user.command_context(&idempotency_key);
    let result = repo::facility_shipping_origin::configure_facility_shipping_origin(
        &state.db,
        &user.tenant,
        &context,
        &command,
    )
    .await?;
    Ok(Json(response(result)?))
}

fn command(
    facility_id: i64,
    request: ConfigureFacilityShippingOriginRequest,
) -> V1Result<ConfigureFacilityShippingOriginCommand> {
    let origin = FacilityShippingOrigin::new(
        request.name,
        request.company,
        request.line1,
        request.line2,
        request.city,
        request.state,
        request.postal_code,
        request.country,
        request.phone,
        request.email,
    )
    .map_err(invalid)?;
    Ok(ConfigureFacilityShippingOriginCommand::new(
        FacilityId::new(facility_id).map_err(invalid)?,
        FacilityRevision::new(request.expected_revision.get()).map_err(invalid)?,
        origin,
    ))
}

fn response(
    result: ConfigureFacilityShippingOriginResult,
) -> V1Result<ConfigureFacilityShippingOriginResponse> {
    let revision = Revision::new(result.revision.get())
        .map_err(|_| V1Error::internal("facility configuration produced an invalid revision"))?;
    Ok(ConfigureFacilityShippingOriginResponse {
        configuration_id: result.configuration_id.get(),
        facility_id: result.facility_id.get(),
        address_id: result.address_id.get(),
        revision,
        origin: FacilityShippingOriginResponse {
            name: result.origin.name().map(str::to_owned),
            company: result.origin.company().map(str::to_owned),
            line1: result.origin.line1().to_owned(),
            line2: result.origin.line2().map(str::to_owned),
            city: result.origin.city().to_owned(),
            state: result.origin.state().map(str::to_owned),
            postal_code: result.origin.postal_code().to_owned(),
            country: result.origin.country().to_owned(),
            phone: result.origin.phone().map(str::to_owned),
            email: result.origin.email().map(str::to_owned),
        },
        configured_by: result.configured_by.get(),
        configured_at: result.configured_at.to_rfc3339(),
    })
}

fn invalid(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructs_a_validated_path_derived_command() {
        let command = command(
            7,
            ConfigureFacilityShippingOriginRequest {
                expected_revision: Revision::new(3).unwrap(),
                name: None,
                company: Some("Wareboxes Fulfillment".into()),
                line1: "100 Distribution Way".into(),
                line2: None,
                city: "Reno".into(),
                state: None,
                postal_code: "89502".into(),
                country: "US".into(),
                phone: None,
                email: None,
            },
        )
        .unwrap();

        assert_eq!(command.facility_id().get(), 7);
        assert_eq!(command.expected_revision().get(), 3);
        assert_eq!(command.origin().company(), Some("Wareboxes Fulfillment"));
    }

    #[test]
    fn rejects_an_origin_without_a_name_or_company() {
        let result = command(
            7,
            ConfigureFacilityShippingOriginRequest {
                expected_revision: Revision::new(3).unwrap(),
                name: None,
                company: None,
                line1: "100 Distribution Way".into(),
                line2: None,
                city: "Reno".into(),
                state: None,
                postal_code: "89502".into(),
                country: "US".into(),
                phone: None,
                email: None,
            },
        );
        assert!(result.is_err());
    }
}
