use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER};
use reqwest::{Client, StatusCode, Url};
use sha2::Sha256;
use wareboxes_application::carrier::{
    CarrierManifestAdapterRequest, CarrierManifestAdapterResponse,
};
use wareboxes_worker::{CarrierGateway, CarrierGatewayError};

type HmacSha256 = Hmac<Sha256>;

const MAX_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;

pub struct HttpCarrierGateway {
    client: Client,
    endpoint: Url,
    authorization: String,
    signing_secret: Vec<u8>,
}

impl HttpCarrierGateway {
    pub fn new(
        endpoint: Url,
        bearer_token: String,
        signing_secret: String,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            client: Client::builder().build()?,
            endpoint,
            authorization: format!("Bearer {bearer_token}"),
            signing_secret: signing_secret.into_bytes(),
        })
    }
}

#[async_trait]
impl CarrierGateway for HttpCarrierGateway {
    fn name(&self) -> &'static str {
        "wareboxes_http_v1"
    }

    async fn manifest(
        &self,
        request: &CarrierManifestAdapterRequest,
    ) -> Result<CarrierManifestAdapterResponse, CarrierGatewayError> {
        let body = serde_json::to_vec(request).map_err(|error| {
            CarrierGatewayError::permanent("serialize_request", error.to_string())
        })?;
        let timestamp = Utc::now().timestamp();
        let signature = signature(&self.signing_secret, timestamp, &body)
            .map_err(|error| CarrierGatewayError::permanent("sign_request", error.to_string()))?;
        let response = self
            .client
            .post(self.endpoint.clone())
            .header(AUTHORIZATION, &self.authorization)
            .header(CONTENT_TYPE, "application/json")
            .header("idempotency-key", &request.request_key)
            .header("x-wareboxes-carrier-timestamp", timestamp.to_string())
            .header("x-wareboxes-carrier-signature", format!("v1={signature}"))
            .body(body)
            .send()
            .await
            .map_err(|error| CarrierGatewayError::retryable("transport", error.to_string()))?;
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES)
        {
            return Err(CarrierGatewayError::permanent(
                "response_too_large",
                "carrier gateway response exceeds 2 MiB",
            ));
        }
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_secs);
        let bytes = response
            .bytes()
            .await
            .map_err(|error| CarrierGatewayError::retryable("response_body", error.to_string()))?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RESPONSE_BYTES {
            return Err(CarrierGatewayError::permanent(
                "response_too_large",
                "carrier gateway response exceeds 2 MiB",
            ));
        }
        if status.is_success() {
            return serde_json::from_slice(&bytes).map_err(|error| {
                CarrierGatewayError::permanent("invalid_json", error.to_string())
            });
        }
        let message = response_message(status, &bytes);
        if retryable(status) {
            let error = CarrierGatewayError::retryable("http_status", message);
            return Err(match retry_after {
                Some(delay) => error.with_retry_after(delay),
                None => error,
            });
        }
        Err(CarrierGatewayError::permanent("http_status", message))
    }
}

fn retryable(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

fn response_message(status: StatusCode, bytes: &[u8]) -> String {
    let body = String::from_utf8_lossy(bytes);
    let body = body.trim().chars().take(800).collect::<String>();
    if body.is_empty() {
        format!("carrier gateway returned HTTP {status}")
    } else {
        format!("carrier gateway returned HTTP {status}: {body}")
    }
}

fn signature(secret: &[u8], timestamp: i64, body: &[u8]) -> anyhow::Result<String> {
    let mut signer = HmacSha256::new_from_slice(secret)
        .map_err(|error| anyhow::anyhow!("initializing carrier HMAC: {error}"))?;
    signer.update(timestamp.to_string().as_bytes());
    signer.update(b".");
    signer.update(body);
    Ok(hex::encode(signer.finalize().into_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use wareboxes_application::carrier::{CarrierAddressSnapshot, CarrierPackageSnapshot};
    use wareboxes_domain::{CarrierAccountKey, CarrierCode, CartonId, ShipmentId, TenantId};

    #[test]
    fn signatures_are_deterministic_and_body_bound() {
        let first = signature(b"0123456789abcdef0123456789abcdef", 7, b"one").unwrap();
        assert_eq!(
            first,
            signature(b"0123456789abcdef0123456789abcdef", 7, b"one").unwrap()
        );
        assert_ne!(
            first,
            signature(b"0123456789abcdef0123456789abcdef", 7, b"two").unwrap()
        );
    }

    #[tokio::test]
    async fn gateway_sends_signed_replay_identity_and_decodes_exact_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0_u8; 4096];
            loop {
                let count = stream.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&chunk[..count]);
                let Some(header_end) = bytes.windows(4).position(|value| value == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|value| value.parse::<usize>().ok())
                    })
                    .unwrap();
                if bytes.len() >= header_end + 4 + content_length {
                    assert!(headers.contains("authorization: Bearer gateway-token"));
                    assert!(headers.contains("idempotency-key: stable-request"));
                    assert!(headers.contains("x-wareboxes-carrier-signature: v1="));
                    let request: serde_json::Value =
                        serde_json::from_slice(&bytes[header_end + 4..]).unwrap();
                    assert_eq!(request["request_key"], "stable-request");
                    let response = serde_json::json!({
                        "schema_version": 1,
                        "request_key": "stable-request",
                        "manifest_reference": "MANIFEST-1",
                        "packages": [{"carton_id": 3, "tracking_number": "TRACK-3"}]
                    })
                    .to_string();
                    stream
                        .write_all(
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                response.len(), response
                            )
                            .as_bytes(),
                        )
                        .await
                        .unwrap();
                    break;
                }
            }
        });
        let gateway = HttpCarrierGateway::new(
            Url::parse(&format!("http://{address}/manifests")).unwrap(),
            "gateway-token".into(),
            "0123456789abcdef0123456789abcdef".into(),
        )
        .unwrap();
        let address = CarrierAddressSnapshot {
            name: Some("Warehouse".into()),
            company: None,
            line1: "100 Dock Way".into(),
            line2: None,
            postal_code: "89501".into(),
            country: "US".into(),
            phone: None,
            email: None,
            state: Some("NV".into()),
            county: None,
            city: "Reno".into(),
            territory: None,
            district: None,
        };
        let response = gateway
            .manifest(&CarrierManifestAdapterRequest {
                schema_version: 1,
                request_key: "stable-request".into(),
                tenant_id: TenantId::new(1).unwrap(),
                account_key: CarrierAccountKey::new("account-1").unwrap(),
                carrier_code: CarrierCode::new("UPS").unwrap(),
                service_code: None,
                shipment_id: ShipmentId::new(2).unwrap(),
                origin: address.clone(),
                destination: address,
                packages: vec![CarrierPackageSnapshot {
                    carton_id: CartonId::new(3).unwrap(),
                    carton_barcode: "CARTON-3".into(),
                    weight_grams: 1200,
                    length_millimeters: None,
                    width_millimeters: None,
                    height_millimeters: None,
                }],
            })
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(response.request_key, "stable-request");
        assert_eq!(response.packages[0].tracking_number.as_str(), "TRACK-3");
    }
}
