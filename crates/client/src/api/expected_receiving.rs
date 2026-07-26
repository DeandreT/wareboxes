use wareboxes_api_contract::v1::{
    ConfirmExpectedReceiptRequest, ErrorResponse, ExpectedReceiptConfirmationResponse,
    ExpectedReceivingSessionResponse, IDEMPOTENCY_KEY_HEADER,
};

use super::{ApiClient, ApiEvent};

const REQUEST_ID_HEADER: &str = "x-request-id";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedReceivingCommand {
    LoadSession {
        load_id: i64,
    },
    Confirm {
        load_line_id: i64,
        body: ConfirmExpectedReceiptRequest,
        idempotency_key: String,
    },
}

impl ExpectedReceivingCommand {
    #[cfg(test)]
    pub fn idempotency_key(&self) -> Option<&str> {
        match self {
            Self::LoadSession { .. } => None,
            Self::Confirm {
                idempotency_key, ..
            } => Some(idempotency_key),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedReceivingRequest {
    pub request_id: String,
    pub command: ExpectedReceivingCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedReceivingTransportOutcome {
    Session(ExpectedReceivingSessionResponse),
    Confirmation(ExpectedReceiptConfirmationResponse),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedReceivingTransportFailure {
    pub status: Option<u16>,
    pub error: Option<ErrorResponse>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedReceivingTransportEvent {
    pub request: ExpectedReceivingRequest,
    pub outcome: Result<ExpectedReceivingTransportOutcome, ExpectedReceivingTransportFailure>,
}

impl ApiClient {
    pub fn execute_expected_receiving(&self, request: ExpectedReceivingRequest) {
        let mut http_request = match build_correlated_request(&self.base_url, &request) {
            Ok(request) => request,
            Err(message) => {
                let _ = self.tx.send(ApiEvent::ExpectedReceiving(
                    ExpectedReceivingTransportEvent {
                        request,
                        outcome: Err(ExpectedReceivingTransportFailure {
                            status: None,
                            error: None,
                            message,
                        }),
                    },
                ));
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
                Err(message) => Err(ExpectedReceivingTransportFailure {
                    status: None,
                    error: None,
                    message,
                }),
            };
            let _ = tx.send(ApiEvent::ExpectedReceiving(
                ExpectedReceivingTransportEvent { request, outcome },
            ));
            ctx.request_repaint();
        });
    }
}

fn build_correlated_request(
    base_url: &str,
    workflow_request: &ExpectedReceivingRequest,
) -> Result<ehttp::Request, String> {
    let mut request = build_request(base_url, &workflow_request.command)?;
    request
        .headers
        .insert(REQUEST_ID_HEADER, &workflow_request.request_id);
    Ok(request)
}

fn build_request(
    base_url: &str,
    command: &ExpectedReceivingCommand,
) -> Result<ehttp::Request, String> {
    let base_url = base_url.trim_end_matches('/');
    let (mut request, idempotency_key) = match command {
        ExpectedReceivingCommand::LoadSession { load_id } => (
            ehttp::Request::get(format!(
                "{base_url}/api/v1/expected-receiving/loads/{load_id}"
            )),
            None,
        ),
        ExpectedReceivingCommand::Confirm {
            load_line_id,
            body,
            idempotency_key,
        } => {
            let body = serde_json::to_vec(body).map_err(|error| {
                format!("could not encode expected receipt confirmation: {error}")
            })?;
            (
                ehttp::Request::post(
                    format!(
                        "{base_url}/api/v1/expected-receiving/lines/{load_line_id}/confirmations"
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
    command: &ExpectedReceivingCommand,
    response: ehttp::Response,
) -> Result<ExpectedReceivingTransportOutcome, ExpectedReceivingTransportFailure> {
    if !(200..300).contains(&response.status) {
        let error = serde_json::from_slice::<ErrorResponse>(&response.bytes).ok();
        let message = error
            .as_ref()
            .map(|error| format!("{} (request {})", error.message, error.request_id))
            .unwrap_or_else(|| String::from_utf8_lossy(&response.bytes).into_owned());
        return Err(ExpectedReceivingTransportFailure {
            status: Some(response.status),
            error,
            message,
        });
    }

    match command {
        ExpectedReceivingCommand::LoadSession { .. } => serde_json::from_slice(&response.bytes)
            .map(ExpectedReceivingTransportOutcome::Session)
            .map_err(|error| decode_failure(response.status, error)),
        ExpectedReceivingCommand::Confirm { .. } => serde_json::from_slice(&response.bytes)
            .map(ExpectedReceivingTransportOutcome::Confirmation)
            .map_err(|error| decode_failure(response.status, error)),
    }
}

fn decode_failure(status: u16, error: serde_json::Error) -> ExpectedReceivingTransportFailure {
    ExpectedReceivingTransportFailure {
        status: Some(status),
        error: None,
        message: format!("the expected receiving response was invalid: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use wareboxes_api_contract::v1::{
        ErrorReason, ExpectedReceiptDisposition, ExpectedReceiptExceptionReason,
        ExpectedReceiptLine, ExpectedReceiptLineStatus, ExpectedReceivingLoadStatus,
        ExpectedReceivingLocation,
    };

    use super::*;

    fn received_command() -> ExpectedReceivingCommand {
        ExpectedReceivingCommand::Confirm {
            load_line_id: 55,
            body: ConfirmExpectedReceiptRequest::Received {
                item_barcode: "0012345678905".into(),
                receiving_location_barcode: "DOCK-04".into(),
                quantity: 3,
                license_plate_barcode: Some("LP-0007".into()),
                lot: Some("LOT-07".into()),
                serial: None,
                expiration: Some("2027-07-26T00:00:00+00:00".into()),
            },
            idempotency_key: "receive-key-55".into(),
        }
    }

    fn session_response() -> ExpectedReceivingSessionResponse {
        ExpectedReceivingSessionResponse {
            load_id: 11,
            inventory_owner_id: 22,
            facility_id: 33,
            reference_number: Some("ASN-1001".into()),
            status: ExpectedReceivingLoadStatus::Receiving,
            receiving_location: ExpectedReceivingLocation {
                location_id: 44,
                barcode: "DOCK-04".into(),
                name: Some("Inbound Dock 4".into()),
            },
            lines: vec![ExpectedReceiptLine {
                load_line_id: 55,
                item_id: 66,
                item_description: Some("Case-picked item".into()),
                uom: "case".into(),
                item_barcodes: vec!["0012345678905".into()],
                expected_quantity: 12,
                received_quantity: 4,
                rejected_quantity: 1,
                missing_quantity: 0,
                remaining_quantity: 7,
                lot: Some("LOT-07".into()),
                serial: None,
                expiration: Some("2027-07-26T00:00:00+00:00".into()),
            }],
        }
    }

    fn confirmation_response() -> ExpectedReceiptConfirmationResponse {
        ExpectedReceiptConfirmationResponse {
            load_id: 11,
            load_line_id: 55,
            disposition: ExpectedReceiptDisposition::Received,
            quantity: 3,
            inventory_transaction_id: Some(71),
            inventory_balance_id: Some(72),
            item_batch_id: Some(73),
            license_plate_id: Some(74),
            line_status: ExpectedReceiptLineStatus::Partial,
            load_status: ExpectedReceivingLoadStatus::Receiving,
            cumulative_received_quantity: 7,
            cumulative_rejected_quantity: 1,
            cumulative_missing_quantity: 0,
            remaining_quantity: 4,
            receive_completed: false,
        }
    }

    fn response(status: u16, bytes: Vec<u8>) -> ehttp::Response {
        ehttp::Response {
            url: "https://wms.test/api/v1/expected-receiving".into(),
            ok: (200..300).contains(&status),
            status,
            status_text: String::new(),
            headers: ehttp::Headers::default(),
            bytes,
        }
    }

    #[test]
    fn load_session_is_an_exact_read_only_request() {
        let command = ExpectedReceivingCommand::LoadSession { load_id: 11 };

        let request = build_request("https://wms.test/", &command).unwrap();

        assert_eq!(request.method, "GET");
        assert_eq!(
            request.url,
            "https://wms.test/api/v1/expected-receiving/loads/11"
        );
        assert!(request.headers.get(IDEMPOTENCY_KEY_HEADER).is_none());
        assert!(request.body.is_empty());
    }

    #[test]
    fn request_id_is_forwarded_for_server_side_correlation() {
        let workflow_request = ExpectedReceivingRequest {
            request_id: "rf-receive-42".into(),
            command: ExpectedReceivingCommand::LoadSession { load_id: 11 },
        };

        let request = build_correlated_request("https://wms.test", &workflow_request).unwrap();

        assert_eq!(
            request.headers.get(REQUEST_ID_HEADER),
            Some("rf-receive-42")
        );
    }

    #[test]
    fn confirmation_has_exact_url_body_and_caller_key() {
        let command = received_command();

        let request = build_request("https://wms.test/", &command).unwrap();

        assert_eq!(request.method, "POST");
        assert_eq!(
            request.url,
            "https://wms.test/api/v1/expected-receiving/lines/55/confirmations"
        );
        assert_eq!(
            request.headers.get(IDEMPOTENCY_KEY_HEADER),
            Some("receive-key-55")
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&request.body).unwrap(),
            serde_json::json!({
                "disposition": "received",
                "item_barcode": "0012345678905",
                "receiving_location_barcode": "DOCK-04",
                "quantity": 3,
                "license_plate_barcode": "LP-0007",
                "lot": "LOT-07",
                "serial": null,
                "expiration": "2027-07-26T00:00:00+00:00"
            })
        );
    }

    #[test]
    fn confirmation_retry_reuses_exact_command_body_and_key() {
        let command = ExpectedReceivingCommand::Confirm {
            load_line_id: 55,
            body: ConfirmExpectedReceiptRequest::Rejected {
                item_barcode: "DAMAGED-66".into(),
                quantity: 2,
                reason: ExpectedReceiptExceptionReason::Damaged,
                note: Some("Crushed cases".into()),
            },
            idempotency_key: "reject-key-55".into(),
        };

        let first = build_request("https://wms.test", &command).unwrap();
        let retry = build_request("https://wms.test", &command).unwrap();

        assert_eq!(first.method, retry.method);
        assert_eq!(first.url, retry.url);
        assert_eq!(first.body, retry.body);
        assert_eq!(
            first.headers.get(IDEMPOTENCY_KEY_HEADER),
            retry.headers.get(IDEMPOTENCY_KEY_HEADER)
        );
        assert_eq!(command.idempotency_key(), Some("reject-key-55"));
    }

    #[test]
    fn successful_responses_decode_to_command_specific_outcomes() {
        let load_command = ExpectedReceivingCommand::LoadSession { load_id: 11 };
        let session = session_response();
        let session_bytes = serde_json::to_vec(&session).unwrap();

        assert_eq!(
            decode_response(&load_command, response(200, session_bytes)),
            Ok(ExpectedReceivingTransportOutcome::Session(session))
        );

        let confirmation = confirmation_response();
        let confirmation_bytes = serde_json::to_vec(&confirmation).unwrap();
        assert_eq!(
            decode_response(&received_command(), response(200, confirmation_bytes)),
            Ok(ExpectedReceivingTransportOutcome::Confirmation(
                confirmation
            ))
        );
    }

    #[test]
    fn stable_v1_errors_remain_typed() {
        let error = ErrorResponse::new(
            ErrorReason::IdempotencyKeyReused,
            "idempotency key was reused with a different request",
            "request-42",
        );
        let bytes = serde_json::to_vec(&error).unwrap();

        let failure = decode_response(&received_command(), response(409, bytes)).unwrap_err();

        assert_eq!(failure.status, Some(409));
        assert_eq!(failure.error, Some(error));
        assert_eq!(
            failure.message,
            "idempotency key was reused with a different request (request request-42)"
        );
    }

    #[test]
    fn invalid_success_payload_is_a_typed_transport_failure() {
        let failure = decode_response(
            &ExpectedReceivingCommand::LoadSession { load_id: 11 },
            response(200, br#"{"load_id":11}"#.to_vec()),
        )
        .unwrap_err();

        assert_eq!(failure.status, Some(200));
        assert!(failure.error.is_none());
        assert!(failure
            .message
            .starts_with("the expected receiving response was invalid:"));
    }
}
