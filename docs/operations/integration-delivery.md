# Integration Delivery

For inbound canonical JSON and X12 940 processing, mapping, quarantine, correction,
and replay procedures, see [Integration Order Intake](integration-order-intake.md).

The outbox worker delivers each committed domain event at least once. Receivers
must deduplicate the stable `event_key`; the worker retains every attempt, retry,
dead-letter transition, and operator replay in PostgreSQL.

## Signed HTTPS webhooks

Configure the worker with:

```text
OUTBOX_PUBLISHER=http
OUTBOX_PUBLISH_URL=https://partner.example/webhooks/wareboxes
OUTBOX_PUBLISH_BEARER_TOKEN=<partner-specific bearer credential>
OUTBOX_WEBHOOK_SIGNING_SECRET=<at least 32 bytes from the secret manager>
```

The worker signs the exact request body using HMAC-SHA256 over
`<timestamp>.<body>` and sends:

- `X-Wareboxes-Webhook-Id`: the stable outbox event key;
- `X-Wareboxes-Webhook-Timestamp`: Unix seconds used by the signature;
- `X-Wareboxes-Webhook-Signature`: `v1=<lowercase hex HMAC>`;
- `Idempotency-Key`: the same stable event key.

Receivers must reject stale timestamps, calculate the HMAC over the unmodified raw
body, compare signatures in constant time, and deduplicate the webhook ID before
applying a business effect. Rotate credentials through a coordinated overlap at the
partner endpoint; Wareboxes currently emits one active `v1` signature.

Plain HTTP is rejected. `OUTBOX_ALLOW_INSECURE_HTTP=true` exists only for isolated
local testing and must not be set in a deployed environment.

## Outbound SFTP exchange

The SFTP adapter requires OpenSSH `sftp`, key authentication, and a pinned host key:

```text
OUTBOX_PUBLISHER=sftp
OUTBOX_SFTP_HOST=sftp.partner.example
OUTBOX_SFTP_PORT=22
OUTBOX_SFTP_USERNAME=wareboxes
OUTBOX_SFTP_PRIVATE_KEY_FILE=/run/secrets/partner-sftp-key
OUTBOX_SFTP_KNOWN_HOSTS_FILE=/run/secrets/partner-known-hosts
OUTBOX_SFTP_REMOTE_DIRECTORY=/exchange/outbound
```

Both credential paths must be readable regular files when the worker starts. Build
the `known_hosts` secret from a host key obtained through an authenticated partner
channel; do not trust a key learned during the first connection.

Each event is serialized as the public outbound event envelope. Its remote filename
is the SHA-256 digest of `event_key` with a `.json` suffix, so retries address the
same object without exposing business identifiers in paths. The worker uploads to a
hidden `.upload` name and renames it only after the complete payload arrives. A
consumer should watch only `*.json`, archive or deduplicate filenames, and tolerate
at-least-once replacement during recovery.

SFTP transport failures are retryable and follow the same capped retry,
dead-letter, monitoring, and replay policies as HTTP delivery.

## Operations

Use the Integration console to inspect delivery history and bounded diagnostics,
then replay or discard a dead-lettered event with an attributed reason. Never edit
outbox rows or remote delivery files to make an event appear successful. Alert on
dead-letter creation, repeated transport failure, and growing oldest-event age as
defined in the telemetry runbook.
