use std::sync::mpsc::Sender;

use eframe::egui;
use thiserror::Error;
use url::Url;

use crate::command_store::{DispatchAttempt, DurableHttpResponse, ExecutionScope};

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
    request_id: &str,
) -> Result<ehttp::Request, TransportBuildError> {
    let mut request = ehttp::Request::get(
        transport
            .endpoint
            .api_url("/api/v1/putaway-claims/current")?,
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
    request_id: String,
    sender: Sender<NetworkEvent>,
    context: egui::Context,
) {
    ehttp::fetch(request, move |result| {
        let response = result
            .map(NetworkResponse::from)
            .map_err(|error| error.to_string());
        let _ = sender.send(NetworkEvent::CurrentClaim {
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
    use crate::command_store::CommandStore;
    use crate::workflow::{DurableCommandDraft, PutawayCommand, PutawayKind};

    use super::*;

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
                        workflow: PutawayKind::Loose,
                    },
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
                        workflow: PutawayKind::Loose,
                    },
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
