use std::sync::mpsc::Sender;

use eframe::egui;
use thiserror::Error;
use url::Url;
use wareboxes_api_contract::v1::{HeartbeatCycleCountClaimRequest, IdempotencyKey};

use crate::command_store::{DispatchAttempt, DurableHttpResponse, ExecutionScope};
use crate::wire::{
    EXPECTED_RECEIVING_BARCODE_LOOKUP_PATH, build_cross_dock_heartbeat_request_parts,
    build_expected_receiving_session_path, build_movement_heartbeat_request_parts,
    build_pick_heartbeat_request_parts, build_replenishment_heartbeat_request_parts,
    normalize_expected_receiving_load_barcode,
};
use crate::workflow::{ClaimOperation, MovementOperation};

const ACCEPT_JSON: &str = "application/json";
const REQUEST_ID_HEADER: &str = "X-Request-Id";
const TENANT_ID_HEADER: &str = "X-Wareboxes-Tenant-Id";
const IDEMPOTENCY_KEY_HEADER: &str = "Idempotency-Key";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerEndpoint(Url);

impl ServerEndpoint {
    pub fn parse(value: &str) -> Result<Self, TransportBuildError> {
        let value = value.trim();
        let mut url = Url::parse(value).map_err(|_| TransportBuildError::InvalidServerUrl)?;
        if url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(TransportBuildError::InvalidServerUrl);
        }
        if !matches!(url.scheme(), "https" | "http") {
            return Err(TransportBuildError::InvalidServerUrl);
        }
        #[cfg(target_os = "android")]
        if url.scheme() != "https" {
            return Err(TransportBuildError::HttpsRequired);
        }

        url.set_query(None);
        url.set_fragment(None);
        if !url.path().ends_with('/') {
            let path = format!("{}/", url.path());
            url.set_path(&path);
        }
        Ok(Self(url))
    }

    pub fn display(&self) -> String {
        self.0.as_str().trim_end_matches('/').to_owned()
    }

    fn api_url(&self, path: &str) -> Result<String, TransportBuildError> {
        let path = path
            .strip_prefix('/')
            .ok_or(TransportBuildError::InvalidApiPath)?;
        self.0
            .join(path)
            .map(|url| url.into())
            .map_err(|_| TransportBuildError::InvalidApiPath)
    }

    fn api_url_with_segment(
        &self,
        path: &str,
        segment: &str,
    ) -> Result<String, TransportBuildError> {
        let path = path.trim_end_matches('/');
        let mut url =
            Url::parse(&self.api_url(path)?).map_err(|_| TransportBuildError::InvalidApiPath)?;
        url.path_segments_mut()
            .map_err(|_| TransportBuildError::InvalidApiPath)?
            .push(segment);
        Ok(url.into())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransportBuildError {
    #[error("Enter a valid server URL")]
    InvalidServerUrl,
    #[error("The Android app requires an HTTPS server")]
    HttpsRequired,
    #[error("The API request path is invalid")]
    InvalidApiPath,
    #[error("The stored command body does not match its integrity hash")]
    CorruptCommandBody,
    #[error("The putaway task ID must be positive")]
    InvalidTaskId,
    #[error("Enter a valid idempotency key")]
    InvalidIdempotencyKey,
    #[error("The heartbeat request could not be encoded")]
    InvalidHeartbeatRequest,
    #[error("The expected receiving load ID must be positive")]
    InvalidLoadId,
    #[error("Scan a valid load barcode")]
    InvalidLoadBarcode,
}

pub struct AuthenticatedTransport<'a> {
    pub endpoint: &'a ServerEndpoint,
    pub token: &'a str,
    pub scope: &'a ExecutionScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkEvent {
    Session {
        request_id: String,
        response: Result<NetworkResponse, String>,
    },
    CurrentClaim {
        operation: ClaimOperation,
        request_id: String,
        response: Result<NetworkResponse, String>,
    },
    Heartbeat {
        operation: ClaimOperation,
        task_id: i64,
        request_id: String,
        response: Result<NetworkResponse, String>,
    },
    ExpectedReceivingSession {
        load_id: i64,
        request_id: String,
        response: Result<NetworkResponse, String>,
    },
    ExpectedReceivingBarcodeLookup {
        barcode: String,
        request_id: String,
        response: Result<NetworkResponse, String>,
    },
    OutboundLoadLookup {
        barcode: String,
        request_id: String,
        response: Result<NetworkResponse, String>,
    },
    Command {
        record_id: i64,
        attempt_id: String,
        response: Result<DurableHttpResponse, String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub server_request_id: Option<String>,
}

pub fn build_session_request(
    endpoint: &ServerEndpoint,
    path: &str,
    request_id: &str,
    body: Vec<u8>,
) -> Result<ehttp::Request, TransportBuildError> {
    let mut request = ehttp::Request::post(endpoint.api_url(path)?, body);
    request.headers = ehttp::Headers::new(&[
        ("Accept", ACCEPT_JSON),
        ("Content-Type", ACCEPT_JSON),
        (REQUEST_ID_HEADER, request_id),
    ]);
    Ok(request)
}

pub fn build_current_claim_request(
    transport: &AuthenticatedTransport<'_>,
    operation: ClaimOperation,
    request_id: &str,
) -> Result<ehttp::Request, TransportBuildError> {
    let path = match operation {
        ClaimOperation::Putaway => "/api/v1/putaway-claims/current",
        ClaimOperation::InventoryRelocation => "/api/v1/inventory-relocation-claims/current",
        ClaimOperation::CycleCount => "/api/v1/cycle-count-claims/current",
        ClaimOperation::Picking => "/api/v1/picking-claims/current",
        ClaimOperation::Replenishment => "/api/v1/replenishment-claims/current",
        ClaimOperation::CrossDock => "/api/v1/cross-dock-claims/current",
    };
    let mut request = ehttp::Request::get(transport.endpoint.api_url(path)?);
    request.headers = authenticated_headers(transport, request_id);
    Ok(request)
}

pub fn build_movement_heartbeat_request(
    transport: &AuthenticatedTransport<'_>,
    operation: ClaimOperation,
    task_id: i64,
    request_id: &str,
    idempotency_key: &str,
) -> Result<ehttp::Request, TransportBuildError> {
    let (path, body) = match operation {
        ClaimOperation::Putaway | ClaimOperation::InventoryRelocation => {
            let movement = match operation {
                ClaimOperation::Putaway => MovementOperation::Putaway,
                ClaimOperation::InventoryRelocation => MovementOperation::InventoryRelocation,
                ClaimOperation::CycleCount
                | ClaimOperation::Picking
                | ClaimOperation::Replenishment
                | ClaimOperation::CrossDock => unreachable!(),
            };
            build_movement_heartbeat_request_parts(movement, task_id).map_err(
                |error| match error {
                    crate::wire::WireRequestError::InvalidTaskId => {
                        TransportBuildError::InvalidTaskId
                    }
                    _ => TransportBuildError::InvalidHeartbeatRequest,
                },
            )?
        }
        ClaimOperation::CycleCount => {
            if task_id <= 0 {
                return Err(TransportBuildError::InvalidTaskId);
            }
            (
                format!("/api/v1/cycle-count-claims/{task_id}/heartbeats"),
                serde_json::to_vec(&HeartbeatCycleCountClaimRequest::default())
                    .map_err(|_| TransportBuildError::InvalidHeartbeatRequest)?,
            )
        }
        ClaimOperation::Picking => {
            build_pick_heartbeat_request_parts(task_id).map_err(|error| match error {
                crate::wire::WireRequestError::InvalidTaskId => TransportBuildError::InvalidTaskId,
                _ => TransportBuildError::InvalidHeartbeatRequest,
            })?
        }
        ClaimOperation::Replenishment => build_replenishment_heartbeat_request_parts(task_id)
            .map_err(|error| match error {
                crate::wire::WireRequestError::InvalidTaskId => TransportBuildError::InvalidTaskId,
                _ => TransportBuildError::InvalidHeartbeatRequest,
            })?,
        ClaimOperation::CrossDock => {
            build_cross_dock_heartbeat_request_parts(task_id).map_err(|error| match error {
                crate::wire::WireRequestError::InvalidTaskId => TransportBuildError::InvalidTaskId,
                _ => TransportBuildError::InvalidHeartbeatRequest,
            })?
        }
    };
    let idempotency_key = IdempotencyKey::new(idempotency_key)
        .map_err(|_| TransportBuildError::InvalidIdempotencyKey)?;
    let mut request = ehttp::Request::post(transport.endpoint.api_url(&path)?, body);
    request.headers = authenticated_headers(transport, request_id);
    request.headers.insert("Content-Type", ACCEPT_JSON);
    request
        .headers
        .insert(IDEMPOTENCY_KEY_HEADER, idempotency_key.into_inner());
    Ok(request)
}

pub fn build_expected_receiving_session_request(
    transport: &AuthenticatedTransport<'_>,
    load_id: i64,
    request_id: &str,
) -> Result<ehttp::Request, TransportBuildError> {
    let path = build_expected_receiving_session_path(load_id).map_err(|error| match error {
        crate::wire::WireRequestError::InvalidLoadId => TransportBuildError::InvalidLoadId,
        _ => TransportBuildError::InvalidApiPath,
    })?;
    let mut request = ehttp::Request::get(transport.endpoint.api_url(&path)?);
    request.headers = authenticated_headers(transport, request_id);
    Ok(request)
}

pub fn build_expected_receiving_barcode_lookup_request(
    transport: &AuthenticatedTransport<'_>,
    barcode: &str,
    request_id: &str,
) -> Result<ehttp::Request, TransportBuildError> {
    let barcode = normalize_expected_receiving_load_barcode(barcode)
        .map_err(|_| TransportBuildError::InvalidLoadBarcode)?;
    let mut request = ehttp::Request::get(
        transport
            .endpoint
            .api_url_with_segment(EXPECTED_RECEIVING_BARCODE_LOOKUP_PATH, &barcode)?,
    );
    request.headers = authenticated_headers(transport, request_id);
    Ok(request)
}

pub fn build_outbound_load_lookup_request(
    transport: &AuthenticatedTransport<'_>,
    barcode: &str,
    request_id: &str,
) -> Result<ehttp::Request, TransportBuildError> {
    let barcode = barcode.trim();
    if barcode.is_empty() || barcode.len() > 200 || barcode.chars().any(char::is_control) {
        return Err(TransportBuildError::InvalidLoadBarcode);
    }
    let mut request = ehttp::Request::get(
        transport
            .endpoint
            .api_url_with_segment("/api/v1/outbound-loads/by-barcode", barcode)?,
    );
    request.headers = authenticated_headers(transport, request_id);
    Ok(request)
}

pub fn build_command_request(
    transport: &AuthenticatedTransport<'_>,
    attempt: &DispatchAttempt,
) -> Result<ehttp::Request, TransportBuildError> {
    if !attempt.command.request.verify_body() {
        return Err(TransportBuildError::CorruptCommandBody);
    }
    let mut request = ehttp::Request::post(
        transport.endpoint.api_url(&attempt.command.request.path)?,
        attempt.command.request.body.clone(),
    );
    request.method = "POST".into();
    request.headers = authenticated_headers(transport, &attempt.request_id);
    request
        .headers
        .insert("Content-Type", attempt.command.request.content_type.clone());
    request.headers.insert(
        IDEMPOTENCY_KEY_HEADER,
        attempt.command.draft.idempotency_key.clone(),
    );
    Ok(request)
}

fn authenticated_headers(
    transport: &AuthenticatedTransport<'_>,
    request_id: &str,
) -> ehttp::Headers {
    ehttp::Headers::new(&[
        ("Accept", ACCEPT_JSON),
        ("Authorization", &format!("Bearer {}", transport.token)),
        (TENANT_ID_HEADER, &transport.scope.tenant_id.to_string()),
        (REQUEST_ID_HEADER, request_id),
    ])
}

pub fn send_session(
    request: ehttp::Request,
    request_id: String,
    sender: Sender<NetworkEvent>,
    context: egui::Context,
) {
    ehttp::fetch(request, move |result| {
        let response = result
            .map(NetworkResponse::from)
            .map_err(|error| error.to_string());
        let _ = sender.send(NetworkEvent::Session {
            request_id,
            response,
        });
        context.request_repaint();
    });
}

pub fn send_current_claim(
    request: ehttp::Request,
    operation: ClaimOperation,
    request_id: String,
    sender: Sender<NetworkEvent>,
    context: egui::Context,
) {
    ehttp::fetch(request, move |result| {
        let response = result
            .map(NetworkResponse::from)
            .map_err(|error| error.to_string());
        let _ = sender.send(NetworkEvent::CurrentClaim {
            operation,
            request_id,
            response,
        });
        context.request_repaint();
    });
}

pub fn send_heartbeat(
    request: ehttp::Request,
    operation: ClaimOperation,
    task_id: i64,
    request_id: String,
    sender: Sender<NetworkEvent>,
    context: egui::Context,
) {
    ehttp::fetch(request, move |result| {
        let response = result
            .map(NetworkResponse::from)
            .map_err(|error| error.to_string());
        let _ = sender.send(NetworkEvent::Heartbeat {
            operation,
            task_id,
            request_id,
            response,
        });
        context.request_repaint();
    });
}

pub fn send_expected_receiving_session(
    request: ehttp::Request,
    load_id: i64,
    request_id: String,
    sender: Sender<NetworkEvent>,
    context: egui::Context,
) {
    ehttp::fetch(request, move |result| {
        let response = result
            .map(NetworkResponse::from)
            .map_err(|error| error.to_string());
        let _ = sender.send(NetworkEvent::ExpectedReceivingSession {
            load_id,
            request_id,
            response,
        });
        context.request_repaint();
    });
}

pub fn send_expected_receiving_barcode_lookup(
    request: ehttp::Request,
    barcode: String,
    request_id: String,
    sender: Sender<NetworkEvent>,
    context: egui::Context,
) {
    ehttp::fetch(request, move |result| {
        let response = result
            .map(NetworkResponse::from)
            .map_err(|error| error.to_string());
        let _ = sender.send(NetworkEvent::ExpectedReceivingBarcodeLookup {
            barcode,
            request_id,
            response,
        });
        context.request_repaint();
    });
}

pub fn send_outbound_load_lookup(
    request: ehttp::Request,
    barcode: String,
    request_id: String,
    sender: Sender<NetworkEvent>,
    context: egui::Context,
) {
    ehttp::fetch(request, move |result| {
        let response = result
            .map(NetworkResponse::from)
            .map_err(|error| error.to_string());
        let _ = sender.send(NetworkEvent::OutboundLoadLookup {
            barcode,
            request_id,
            response,
        });
        context.request_repaint();
    });
}

pub fn send_command(
    request: ehttp::Request,
    record_id: i64,
    attempt_id: String,
    sender: Sender<NetworkEvent>,
    context: egui::Context,
) {
    ehttp::fetch(request, move |result| {
        let response = result
            .map(|response| DurableHttpResponse {
                status: response.status,
                server_request_id: response.headers.get("x-request-id").map(str::to_owned),
                body: response.bytes,
            })
            .map_err(|error| error.to_string());
        let _ = sender.send(NetworkEvent::Command {
            record_id,
            attempt_id,
            response,
        });
        context.request_repaint();
    });
}

impl From<ehttp::Response> for NetworkResponse {
    fn from(response: ehttp::Response) -> Self {
        Self {
            status: response.status,
            server_request_id: response.headers.get("x-request-id").map(str::to_owned),
            body: response.bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_store::CommandStore;
    use crate::workflow::{DurableCommandDraft, MovementKind, PutawayCommand};

    fn attempt() -> DispatchAttempt {
        let mut store = CommandStore::open_in_memory().unwrap();
        let scope = ExecutionScope {
            tenant_id: 7,
            operator_id: 8,
            device_id: "rf-01".into(),
        };
        let record = store
            .persist(
                &scope,
                DurableCommandDraft {
                    schema_version: 1,
                    command_id: "command-1".into(),
                    idempotency_key: "putaway-key-1".into(),
                    command: PutawayCommand::ClaimNext {
                        workflow: MovementKind::Loose,
                    }
                    .into(),
                },
            )
            .unwrap();
        store.begin_attempt(&scope, record.record_id).unwrap()
    }

    #[test]
    fn endpoint_rejects_credentials_queries_and_non_http_schemes() {
        for invalid in [
            "",
            "api.example.com",
            "ftp://api.example.com",
            "https://user:secret@api.example.com",
            "https://api.example.com?tenant=1",
            "https://api.example.com#fragment",
        ] {
            assert_eq!(
                ServerEndpoint::parse(invalid),
                Err(TransportBuildError::InvalidServerUrl)
            );
        }
    }

    #[test]
    fn endpoint_preserves_an_optional_base_path() {
        let endpoint = ServerEndpoint::parse("https://example.com/wareboxes").unwrap();

        assert_eq!(
            endpoint.api_url("/api/v1/sessions").unwrap(),
            "https://example.com/wareboxes/api/v1/sessions"
        );
        assert_eq!(endpoint.display(), "https://example.com/wareboxes");
    }

    #[test]
    fn command_request_uses_only_the_durable_envelope_and_attempt_identity() {
        let attempt = attempt();
        let endpoint = ServerEndpoint::parse("https://example.com").unwrap();
        let scope = attempt.command.scope.clone();
        let request = build_command_request(
            &AuthenticatedTransport {
                endpoint: &endpoint,
                token: "session-secret",
                scope: &scope,
            },
            &attempt,
        )
        .unwrap();

        assert_eq!(request.method, "POST");
        assert_eq!(
            request.url,
            "https://example.com/api/v1/putaway-claims/next"
        );
        assert_eq!(request.body, attempt.command.request.body);
        assert_eq!(
            request.headers.get("Content-Type"),
            Some("application/json")
        );
        assert_eq!(
            request.headers.get(IDEMPOTENCY_KEY_HEADER),
            Some("putaway-key-1")
        );
        assert_eq!(
            request.headers.get("Authorization"),
            Some("Bearer session-secret")
        );
        assert_eq!(request.headers.get(TENANT_ID_HEADER), Some("7"));
        assert_eq!(
            request.headers.get(REQUEST_ID_HEADER),
            Some(attempt.request_id.as_str())
        );
    }

    #[test]
    fn heartbeat_request_uses_authenticated_replay_safe_headers_and_exact_body() {
        let endpoint = ServerEndpoint::parse("https://example.com").unwrap();
        let scope = ExecutionScope {
            tenant_id: 7,
            operator_id: 8,
            device_id: "rf-01".into(),
        };
        let request = build_movement_heartbeat_request(
            &AuthenticatedTransport {
                endpoint: &endpoint,
                token: "session-secret",
                scope: &scope,
            },
            MovementOperation::Putaway.into(),
            42,
            "rf-heartbeat-request-1",
            "putaway:heartbeat:42:1",
        )
        .unwrap();

        assert_eq!(request.method, "POST");
        assert_eq!(
            request.url,
            "https://example.com/api/v1/putaway-claims/42/heartbeats"
        );
        assert_eq!(request.body, b"{}");
        assert_eq!(
            request.headers.get("Content-Type"),
            Some("application/json")
        );
        assert_eq!(
            request.headers.get(IDEMPOTENCY_KEY_HEADER),
            Some("putaway:heartbeat:42:1")
        );
        assert_eq!(
            request.headers.get("Authorization"),
            Some("Bearer session-secret")
        );
        assert_eq!(request.headers.get(TENANT_ID_HEADER), Some("7"));
        assert_eq!(
            request.headers.get(REQUEST_ID_HEADER),
            Some("rf-heartbeat-request-1")
        );
    }

    #[test]
    fn heartbeat_request_rejects_invalid_task_and_idempotency_identity() {
        let endpoint = ServerEndpoint::parse("https://example.com").unwrap();
        let scope = ExecutionScope {
            tenant_id: 7,
            operator_id: 8,
            device_id: "rf-01".into(),
        };
        let transport = AuthenticatedTransport {
            endpoint: &endpoint,
            token: "session-secret",
            scope: &scope,
        };

        assert!(matches!(
            build_movement_heartbeat_request(
                &transport,
                MovementOperation::Putaway.into(),
                0,
                "request-1",
                "heartbeat-1"
            ),
            Err(TransportBuildError::InvalidTaskId)
        ));
        assert!(matches!(
            build_movement_heartbeat_request(
                &transport,
                MovementOperation::Putaway.into(),
                42,
                "request-1",
                "has spaces"
            ),
            Err(TransportBuildError::InvalidIdempotencyKey)
        ));
    }

    #[test]
    fn relocation_claim_and_heartbeat_use_relocation_endpoints() {
        let endpoint = ServerEndpoint::parse("https://example.com").unwrap();
        let scope = ExecutionScope {
            tenant_id: 7,
            operator_id: 8,
            device_id: "rf-01".into(),
        };
        let transport = AuthenticatedTransport {
            endpoint: &endpoint,
            token: "session-secret",
            scope: &scope,
        };

        let current = build_current_claim_request(
            &transport,
            MovementOperation::InventoryRelocation.into(),
            "request-1",
        )
        .unwrap();
        assert_eq!(
            current.url,
            "https://example.com/api/v1/inventory-relocation-claims/current"
        );

        let heartbeat = build_movement_heartbeat_request(
            &transport,
            MovementOperation::InventoryRelocation.into(),
            42,
            "request-2",
            "relocation:heartbeat:42:1",
        )
        .unwrap();
        assert_eq!(
            heartbeat.url,
            "https://example.com/api/v1/inventory-relocation-claims/42/heartbeats"
        );
        assert_eq!(heartbeat.body, b"{}");
    }

    #[test]
    fn cycle_count_claim_and_heartbeat_use_count_endpoints() {
        let endpoint = ServerEndpoint::parse("https://example.com").unwrap();
        let scope = ExecutionScope {
            tenant_id: 7,
            operator_id: 8,
            device_id: "rf-01".into(),
        };
        let transport = AuthenticatedTransport {
            endpoint: &endpoint,
            token: "session-secret",
            scope: &scope,
        };

        let current =
            build_current_claim_request(&transport, ClaimOperation::CycleCount, "request-1")
                .unwrap();
        assert_eq!(
            current.url,
            "https://example.com/api/v1/cycle-count-claims/current"
        );

        let heartbeat = build_movement_heartbeat_request(
            &transport,
            ClaimOperation::CycleCount,
            42,
            "request-2",
            "cycle-count:heartbeat:42:1",
        )
        .unwrap();
        assert_eq!(
            heartbeat.url,
            "https://example.com/api/v1/cycle-count-claims/42/heartbeats"
        );
        assert_eq!(heartbeat.body, b"{}");
    }

    #[test]
    fn expected_receiving_session_get_has_authenticated_headers_without_idempotency() {
        let endpoint = ServerEndpoint::parse("https://example.com/wareboxes").unwrap();
        let scope = ExecutionScope {
            tenant_id: 7,
            operator_id: 8,
            device_id: "rf-01".into(),
        };
        let request = build_expected_receiving_session_request(
            &AuthenticatedTransport {
                endpoint: &endpoint,
                token: "session-secret",
                scope: &scope,
            },
            11,
            "rf-receiving-session-1",
        )
        .unwrap();

        assert_eq!(request.method, "GET");
        assert_eq!(
            request.url,
            "https://example.com/wareboxes/api/v1/expected-receiving/loads/11"
        );
        assert!(request.body.is_empty());
        assert_eq!(
            request.headers.get("Authorization"),
            Some("Bearer session-secret")
        );
        assert_eq!(request.headers.get(TENANT_ID_HEADER), Some("7"));
        assert_eq!(
            request.headers.get(REQUEST_ID_HEADER),
            Some("rf-receiving-session-1")
        );
        assert_eq!(request.headers.get(IDEMPOTENCY_KEY_HEADER), None);
        assert_eq!(request.headers.get("Content-Type"), None);
    }

    #[test]
    fn expected_receiving_barcode_lookup_encodes_one_path_segment() {
        let endpoint = ServerEndpoint::parse("https://example.com/wareboxes").unwrap();
        let scope = ExecutionScope {
            tenant_id: 7,
            operator_id: 8,
            device_id: "rf-01".into(),
        };
        let transport = AuthenticatedTransport {
            endpoint: &endpoint,
            token: "session-secret",
            scope: &scope,
        };
        assert_eq!(
            endpoint
                .api_url_with_segment(EXPECTED_RECEIVING_BARCODE_LOOKUP_PATH, "ASN/50%?#東京")
                .unwrap(),
            "https://example.com/wareboxes/api/v1/expected-receiving/loads/by-barcode/ASN%2F50%25%3F%23%E6%9D%B1%E4%BA%AC"
        );
        let request = build_expected_receiving_barcode_lookup_request(
            &transport,
            " asn:50.1_2-3 ",
            "rf-receiving-barcode-1",
        )
        .unwrap();

        assert_eq!(request.method, "GET");
        assert_eq!(
            request.url,
            "https://example.com/wareboxes/api/v1/expected-receiving/loads/by-barcode/ASN:50.1_2-3"
        );
        assert_eq!(
            request.headers.get("Authorization"),
            Some("Bearer session-secret")
        );
        assert_eq!(request.headers.get(TENANT_ID_HEADER), Some("7"));
        assert_eq!(request.headers.get(IDEMPOTENCY_KEY_HEADER), None);

        for invalid in ["", "-ASN-1", "ASN/1", "ASN%1", "ASN?1", "ASN#1", "東京"] {
            assert!(matches!(
                build_expected_receiving_barcode_lookup_request(
                    &transport,
                    invalid,
                    "rf-receiving-barcode-2"
                ),
                Err(TransportBuildError::InvalidLoadBarcode)
            ));
        }
    }

    #[test]
    fn expected_receiving_transport_rejects_invalid_load_identity() {
        let endpoint = ServerEndpoint::parse("https://example.com").unwrap();
        let scope = ExecutionScope {
            tenant_id: 7,
            operator_id: 8,
            device_id: "rf-01".into(),
        };
        let transport = AuthenticatedTransport {
            endpoint: &endpoint,
            token: "session-secret",
            scope: &scope,
        };
        assert!(matches!(
            build_expected_receiving_session_request(&transport, 0, "request-1"),
            Err(TransportBuildError::InvalidLoadId)
        ));
    }

    #[test]
    fn retry_changes_only_transport_attempt_identity() {
        let mut store = CommandStore::open_in_memory().unwrap();
        let scope = ExecutionScope {
            tenant_id: 7,
            operator_id: 8,
            device_id: "rf-01".into(),
        };
        let record = store
            .persist(
                &scope,
                DurableCommandDraft {
                    schema_version: 1,
                    command_id: "command-2".into(),
                    idempotency_key: "putaway-key-2".into(),
                    command: PutawayCommand::ClaimNext {
                        workflow: MovementKind::Loose,
                    }
                    .into(),
                },
            )
            .unwrap();
        let first = store.begin_attempt(&scope, record.record_id).unwrap();
        store
            .mark_ambiguous(
                &scope,
                record.record_id,
                &first.attempt_id,
                "connection lost",
            )
            .unwrap();
        let second = store.begin_attempt(&scope, record.record_id).unwrap();

        assert_ne!(first.attempt_id, second.attempt_id);
        assert_ne!(first.request_id, second.request_id);
        assert_eq!(first.command.request, second.command.request);
        assert_eq!(
            first.command.draft.idempotency_key,
            second.command.draft.idempotency_key
        );
    }
}
