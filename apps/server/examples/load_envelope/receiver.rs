use std::collections::HashSet;
use std::env;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use hmac::{Hmac, Mac};
use serde::Serialize;
use serde_json::Value;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
struct ReceiverState {
    bearer_token: Arc<str>,
    signing_secret: Arc<[u8]>,
    delay: Duration,
    event_keys: Arc<Mutex<HashSet<String>>>,
    received: Arc<AtomicUsize>,
    duplicates: Arc<AtomicUsize>,
}

#[derive(Serialize)]
struct ReceiverStats {
    received: usize,
    unique: usize,
    duplicates: usize,
}

pub async fn serve() -> anyhow::Result<()> {
    let port = u16::try_from(integer_env("LOAD_WEBHOOK_PORT", 18_085, 1_024, 65_535)?)
        .context("LOAD_WEBHOOK_PORT does not fit in u16")?;
    let delay_millis = integer_env("LOAD_WEBHOOK_DELAY_MILLIS", 25, 0, 60_000)?;
    let bearer_token = required_env("LOAD_WEBHOOK_BEARER_TOKEN")?;
    let signing_secret = required_env("LOAD_WEBHOOK_SIGNING_SECRET")?;
    if signing_secret.len() < 32 {
        bail!("LOAD_WEBHOOK_SIGNING_SECRET must contain at least 32 bytes");
    }
    let state = ReceiverState {
        bearer_token: Arc::from(bearer_token),
        signing_secret: Arc::from(signing_secret.into_bytes()),
        delay: Duration::from_millis(
            u64::try_from(delay_millis).context("webhook delay does not fit in u64")?,
        ),
        event_keys: Arc::new(Mutex::new(HashSet::new())),
        received: Arc::new(AtomicUsize::new(0)),
        duplicates: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route("/health", get(|| async { StatusCode::NO_CONTENT }))
        .route("/events", post(receive))
        .route("/stats", get(stats))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .with_context(|| format!("binding load webhook receiver on port {port}"))?;
    println!("event=load_webhook_receiver_started port={port} delay_millis={delay_millis}");
    axum::serve(listener, app)
        .await
        .context("serving load webhook receiver")
}

async fn receive(
    State(state): State<ReceiverState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    match validate_request(&state, &headers, &body) {
        Ok(event_key) => {
            tokio::time::sleep(state.delay).await;
            let duplicate = match state.event_keys.lock() {
                Ok(mut event_keys) => !event_keys.insert(event_key),
                Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
            };
            state.received.fetch_add(1, Ordering::Relaxed);
            if duplicate {
                state.duplicates.fetch_add(1, Ordering::Relaxed);
            }
            StatusCode::NO_CONTENT
        }
        Err(status) => status,
    }
}

fn validate_request(
    state: &ReceiverState,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<String, StatusCode> {
    let authorization = header(headers, "authorization")?;
    if authorization != format!("Bearer {}", state.bearer_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let event_key = header(headers, "x-wareboxes-webhook-id")?.to_owned();
    if header(headers, "idempotency-key")? != event_key {
        return Err(StatusCode::BAD_REQUEST);
    }
    let timestamp = header(headers, "x-wareboxes-webhook-timestamp")?;
    timestamp
        .parse::<i64>()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let signature = header(headers, "x-wareboxes-webhook-signature")?
        .strip_prefix("v1=")
        .ok_or(StatusCode::BAD_REQUEST)?;
    let signature = hex::decode(signature).map_err(|_| StatusCode::BAD_REQUEST)?;
    let mut verifier = HmacSha256::new_from_slice(&state.signing_secret)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    verifier.update(timestamp.as_bytes());
    verifier.update(b".");
    verifier.update(body);
    verifier
        .verify_slice(&signature)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    let envelope: Value = serde_json::from_slice(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    if envelope.get("event_key").and_then(Value::as_str) != Some(&event_key)
        || envelope.get("event_type").and_then(Value::as_str)
            != Some(header(headers, "x-wareboxes-event-type")?)
        || envelope
            .get("tenant_id")
            .and_then(Value::as_i64)
            .map(|value| value.to_string())
            != Some(header(headers, "x-wareboxes-tenant-id")?.to_owned())
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(event_key)
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, StatusCode> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or(StatusCode::BAD_REQUEST)
}

async fn stats(State(state): State<ReceiverState>) -> Json<ReceiverStats> {
    let unique = state
        .event_keys
        .lock()
        .map_or(0, |event_keys| event_keys.len());
    Json(ReceiverStats {
        received: state.received.load(Ordering::Relaxed),
        unique,
        duplicates: state.duplicates.load(Ordering::Relaxed),
    })
}

fn required_env(name: &str) -> anyhow::Result<String> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => bail!("{name} is required and must not be empty"),
    }
}

fn integer_env(
    name: &str,
    default: usize,
    minimum: usize,
    maximum: usize,
) -> anyhow::Result<usize> {
    let value = env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .with_context(|| format!("{name} must be an integer"))
        })
        .transpose()?
        .unwrap_or(default);
    if !(minimum..=maximum).contains(&value) {
        bail!("{name} must be between {minimum} and {maximum}");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_receiver_rejects_payload_tampering() {
        let state = ReceiverState {
            bearer_token: Arc::from("load-token"),
            signing_secret: Arc::from(b"0123456789abcdef0123456789abcdef".as_slice()),
            delay: Duration::ZERO,
            event_keys: Arc::new(Mutex::new(HashSet::new())),
            received: Arc::new(AtomicUsize::new(0)),
            duplicates: Arc::new(AtomicUsize::new(0)),
        };
        let body = br#"{"event_key":"event-1","event_type":"inventory.moved.v1","tenant_id":1}"#;
        let mut signer = HmacSha256::new_from_slice(&state.signing_secret).unwrap();
        signer.update(b"1700000000.");
        signer.update(body);
        let signature = hex::encode(signer.finalize().into_bytes());
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer load-token".parse().unwrap());
        headers.insert("idempotency-key", "event-1".parse().unwrap());
        headers.insert("x-wareboxes-webhook-id", "event-1".parse().unwrap());
        headers.insert(
            "x-wareboxes-webhook-timestamp",
            "1700000000".parse().unwrap(),
        );
        headers.insert(
            "x-wareboxes-webhook-signature",
            format!("v1={signature}").parse().unwrap(),
        );
        headers.insert(
            "x-wareboxes-event-type",
            "inventory.moved.v1".parse().unwrap(),
        );
        headers.insert("x-wareboxes-tenant-id", "1".parse().unwrap());

        assert_eq!(
            validate_request(&state, &headers, body),
            Ok("event-1".into())
        );
        assert_eq!(
            validate_request(&state, &headers, br#"{"tampered":true}"#),
            Err(StatusCode::UNAUTHORIZED)
        );
    }
}
