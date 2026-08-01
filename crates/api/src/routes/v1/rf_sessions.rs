use axum::extract::State;
use axum::Json;
use wareboxes_api_contract::v1::{
    CreateRfSessionRequest, CreateRfSessionResponse, RfSessionOwnerScope, RfSessionSiteScope,
    RfSessionTenant,
};
use wareboxes_application::{ApplicationError, ValidationIssue};

use super::error::{V1Error, V1Result};
use crate::auth;
use crate::error::AppError;
use crate::state::AppState;

const MAX_EMAIL_LENGTH: usize = 254;
const MAX_PASSWORD_LENGTH: usize = 1_024;

pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateRfSessionRequest>,
) -> V1Result<Json<CreateRfSessionResponse>> {
    validate_request(&body)?;
    let user = auth::verify_credentials(&state.db, &body.email, &body.password)
        .await?
        .ok_or_else(|| V1Error::from(AppError::unauthorized()))?;
    let token = auth::create_session(&state.db, user.id).await?;
    let response: V1Result<Json<CreateRfSessionResponse>> = async {
        let access = auth::default_tenant_for_session(&state.db, &token)
            .await?
            .ok_or_else(|| V1Error::from(AppError::forbidden()))?;

        Ok(Json(CreateRfSessionResponse {
            token: token.clone(),
            operator_id: user.id,
            tenant: RfSessionTenant {
                tenant_id: access.tenant_id.get(),
                name: access.name,
                site_scope: RfSessionSiteScope {
                    all_facilities: access.site_scope.all_facilities,
                    facility_ids: access
                        .site_scope
                        .facility_ids
                        .into_iter()
                        .map(|id| id.get())
                        .collect(),
                },
                owner_scope: RfSessionOwnerScope {
                    all_inventory_owners: access.owner_scope.all_inventory_owners,
                    inventory_owner_ids: access
                        .owner_scope
                        .inventory_owner_ids
                        .into_iter()
                        .map(|id| id.get())
                        .collect(),
                },
            },
        }))
    }
    .await;

    if response.is_err() {
        let _ = auth::destroy_session(&state.db, &token).await;
    }
    response
}

fn validate_request(body: &CreateRfSessionRequest) -> V1Result<()> {
    let mut violations = Vec::new();
    validate_credential_field(&mut violations, "email", &body.email, MAX_EMAIL_LENGTH);
    validate_credential_field(
        &mut violations,
        "password",
        &body.password,
        MAX_PASSWORD_LENGTH,
    );

    if violations.is_empty() {
        Ok(())
    } else {
        Err(AppError::Application(ApplicationError::Validation(violations)).into())
    }
}

fn validate_credential_field(
    violations: &mut Vec<ValidationIssue>,
    field: &str,
    value: &str,
    max_length: usize,
) {
    let message = if value.is_empty() {
        Some("must be nonempty".to_owned())
    } else if value.trim() != value {
        Some("must not have leading or trailing whitespace".to_owned())
    } else if value.chars().count() > max_length {
        Some(format!("must not exceed {max_length} characters"))
    } else {
        None
    };

    if let Some(message) = message {
        violations.push(ValidationIssue {
            field: field.to_owned(),
            message,
        });
    }
}
