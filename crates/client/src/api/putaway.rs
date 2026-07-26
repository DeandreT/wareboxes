use wareboxes_api_contract::v1::{
    ClaimNextPutawayRequest, ClaimPutawayByIdRequest, ConfirmLicensePlatePutawayRequest,
    ConfirmPutawayRequest, ErrorResponse, LicensePlatePutawayConfirmationResponse,
    PutawayClaimResponse, PutawayConfirmationResponse, PutawayWorkflow, IDEMPOTENCY_KEY_HEADER,
};

use super::{ApiClient, ApiEvent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PutawayCommand {
    LoadCurrent,
    ClaimNext {
        workflow: PutawayWorkflow,
        idempotency_key: String,
    },
    ClaimById {
        task_id: i64,
        idempotency_key: String,
    },
    ConfirmLoose {
        task_id: i64,
        destination_location_barcode: String,
        idempotency_key: String,
    },
    ConfirmLicensePlate {
        task_id: i64,
        license_plate_barcode: String,
        destination_location_barcode: String,
        idempotency_key: String,
    },
}

impl PutawayCommand {
    #[cfg(test)]
    pub fn idempotency_key(&self) -> Option<&str> {
        match self {
            Self::LoadCurrent => None,
            Self::ClaimNext {
                idempotency_key, ..
            }
            | Self::ClaimById {
                idempotency_key, ..
            }
            | Self::ConfirmLoose {
                idempotency_key, ..
            }
            | Self::ConfirmLicensePlate {
                idempotency_key, ..
            } => Some(idempotency_key),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutawayRequest {
    pub request_id: String,
    pub command: PutawayCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PutawayTransportOutcome {
    Current(Option<PutawayClaimResponse>),
    Claimed(Option<PutawayClaimResponse>),
    LooseConfirmed(PutawayConfirmationResponse),
    LicensePlateConfirmed(LicensePlatePutawayConfirmationResponse),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutawayTransportFailure {
    pub status: Option<u16>,
    pub error: Option<ErrorResponse>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutawayTransportEvent {
    pub request: PutawayRequest,
    pub outcome: Result<PutawayTransportOutcome, PutawayTransportFailure>,
}

impl ApiClient {
    pub fn execute_putaway(&self, request: PutawayRequest) {
        let mut http_request = match build_request(&self.base_url, &request.command) {
            Ok(request) => request,
            Err(message) => {
                let _ = self.tx.send(ApiEvent::Putaway(PutawayTransportEvent {
                    request,
                    outcome: Err(PutawayTransportFailure {
                        status: None,
                        error: None,
                        message,
                    }),
                }));
                self.ctx.request_repaint();
                return;
            }
        };
        http_request.headers = self.authenticated_headers(http_request.headers);

        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        ehttp::fetch(http_request, move |response| {
            let outcome = match response {
                Ok(response) => decode_response(&request.command, response),
                Err(message) => Err(PutawayTransportFailure {
                    status: None,
                    error: None,
                    message,
                }),
            };
            let _ = tx.send(ApiEvent::Putaway(PutawayTransportEvent {
                request,
                outcome,
            }));
            ctx.request_repaint();
        });
    }
}

fn build_request(base_url: &str, command: &PutawayCommand) -> Result<ehttp::Request, String> {
    let base_url = base_url.trim_end_matches('/');
    let (mut request, idempotency_key) = match command {
        PutawayCommand::LoadCurrent => (
            ehttp::Request::get(format!("{base_url}/api/v1/putaway-claims/current")),
            None,
        ),
        PutawayCommand::ClaimNext {
            workflow,
            idempotency_key,
        } => {
            let body = serde_json::to_vec(&ClaimNextPutawayRequest {
                workflow: *workflow,
            })
            .map_err(|error| format!("could not encode putaway claim: {error}"))?;
            (
                ehttp::Request::post(format!("{base_url}/api/v1/putaway-claims/next"), body),
                Some(idempotency_key.as_str()),
            )
        }
        PutawayCommand::ClaimById {
            task_id,
            idempotency_key,
        } => {
            let body = serde_json::to_vec(&ClaimPutawayByIdRequest::default())
                .map_err(|error| format!("could not encode selected putaway claim: {error}"))?;
            (
                ehttp::Request::post(format!("{base_url}/api/v1/putaway-claims/{task_id}"), body),
                Some(idempotency_key.as_str()),
            )
        }
        PutawayCommand::ConfirmLoose {
            task_id,
            destination_location_barcode,
            idempotency_key,
        } => {
            let body = serde_json::to_vec(&ConfirmPutawayRequest {
                destination_location_barcode: destination_location_barcode.clone(),
            })
            .map_err(|error| format!("could not encode loose putaway confirmation: {error}"))?;
            (
                ehttp::Request::post(
                    format!("{base_url}/api/v1/putaway-tasks/{task_id}/confirmations"),
                    body,
                ),
                Some(idempotency_key.as_str()),
            )
        }
        PutawayCommand::ConfirmLicensePlate {
            task_id,
            license_plate_barcode,
            destination_location_barcode,
            idempotency_key,
        } => {
            let body = serde_json::to_vec(&ConfirmLicensePlatePutawayRequest {
                license_plate_barcode: license_plate_barcode.clone(),
                destination_location_barcode: destination_location_barcode.clone(),
            })
            .map_err(|error| {
                format!("could not encode license plate putaway confirmation: {error}")
            })?;
            (
                ehttp::Request::post(
                    format!(
                        "{base_url}/api/v1/license-plate-putaway-tasks/{task_id}/confirmations"
                    ),
                    body,
                ),
                Some(idempotency_key.as_str()),
            )
        }
    };

    if let Some(idempotency_key) = idempotency_key {
        request
            .headers
            .insert(IDEMPOTENCY_KEY_HEADER, idempotency_key);
    }
    Ok(request)
}

fn decode_response(
    command: &PutawayCommand,
    response: ehttp::Response,
) -> Result<PutawayTransportOutcome, PutawayTransportFailure> {
    if !(200..300).contains(&response.status) {
        let error = serde_json::from_slice::<ErrorResponse>(&response.bytes).ok();
        let message = error
            .as_ref()
            .map(|error| format!("{} (request {})", error.message, error.request_id))
            .unwrap_or_else(|| String::from_utf8_lossy(&response.bytes).into_owned());
        return Err(PutawayTransportFailure {
            status: Some(response.status),
            error,
            message,
        });
    }

    match command {
        PutawayCommand::LoadCurrent => serde_json::from_slice(&response.bytes)
            .map(PutawayTransportOutcome::Current)
            .map_err(|error| decode_failure(response.status, error)),
        PutawayCommand::ClaimNext { .. } => serde_json::from_slice(&response.bytes)
            .map(PutawayTransportOutcome::Claimed)
            .map_err(|error| decode_failure(response.status, error)),
        PutawayCommand::ClaimById { .. } => serde_json::from_slice(&response.bytes)
            .map(Some)
            .map(PutawayTransportOutcome::Claimed)
            .map_err(|error| decode_failure(response.status, error)),
        PutawayCommand::ConfirmLoose { .. } => serde_json::from_slice(&response.bytes)
            .map(PutawayTransportOutcome::LooseConfirmed)
            .map_err(|error| decode_failure(response.status, error)),
        PutawayCommand::ConfirmLicensePlate { .. } => serde_json::from_slice(&response.bytes)
            .map(PutawayTransportOutcome::LicensePlateConfirmed)
            .map_err(|error| decode_failure(response.status, error)),
    }
}

fn decode_failure(status: u16, error: serde_json::Error) -> PutawayTransportFailure {
    PutawayTransportFailure {
        status: Some(status),
        error: None,
        message: format!("the putaway response was invalid: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_request_has_exact_v1_body_and_caller_key() {
        let command = PutawayCommand::ClaimNext {
            workflow: PutawayWorkflow::LicensePlate,
            idempotency_key: "claim-key-1".into(),
        };

        let request = build_request("https://wms.test/", &command).unwrap();

        assert_eq!(request.url, "https://wms.test/api/v1/putaway-claims/next");
        assert_eq!(
            request.headers.get(IDEMPOTENCY_KEY_HEADER),
            Some("claim-key-1")
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&request.body).unwrap(),
            serde_json::json!({"workflow": "license_plate"})
        );
    }

    #[test]
    fn confirmation_retry_reuses_exact_key_and_body() {
        let command = PutawayCommand::ConfirmLicensePlate {
            task_id: 44,
            license_plate_barcode: "LP-44".into(),
            destination_location_barcode: "A-01-02".into(),
            idempotency_key: "confirm-key-44".into(),
        };

        let first = build_request("https://wms.test", &command).unwrap();
        let retry = build_request("https://wms.test", &command).unwrap();

        assert_eq!(first.url, retry.url);
        assert_eq!(first.body, retry.body);
        assert_eq!(
            retry.headers.get(IDEMPOTENCY_KEY_HEADER),
            Some("confirm-key-44")
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&retry.body).unwrap(),
            serde_json::json!({
                "license_plate_barcode": "LP-44",
                "destination_location_barcode": "A-01-02"
            })
        );
    }

    #[test]
    fn current_claim_is_read_only() {
        let request = build_request("https://wms.test", &PutawayCommand::LoadCurrent).unwrap();

        assert_eq!(
            request.url,
            "https://wms.test/api/v1/putaway-claims/current"
        );
        assert!(request.headers.get(IDEMPOTENCY_KEY_HEADER).is_none());
        assert!(request.body.is_empty());
    }

    #[test]
    fn selected_task_claim_uses_typed_empty_command() {
        let request = build_request(
            "https://wms.test",
            &PutawayCommand::ClaimById {
                task_id: 81,
                idempotency_key: "selected-key-81".into(),
            },
        )
        .unwrap();

        assert_eq!(request.url, "https://wms.test/api/v1/putaway-claims/81");
        assert_eq!(
            request.headers.get(IDEMPOTENCY_KEY_HEADER),
            Some("selected-key-81")
        );
        assert_eq!(request.body, b"{}");
    }
}
