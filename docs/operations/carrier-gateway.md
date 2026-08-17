# Carrier Gateway Operations

Wareboxes sends immutable shipment snapshots to one deployment-owned HTTPS carrier
gateway. The gateway translates the canonical request into provider-specific APIs
and owns all provider credentials. PostgreSQL and the web application retain only
the non-secret `account_key` used to select credentials inside that gateway.

## Worker configuration

Add the following secrets and settings to `/etc/wareboxes/wareboxes.env`:

```text
CARRIER_GATEWAY_URL=https://carrier-gateway.example/manifests
CARRIER_GATEWAY_BEARER_TOKEN=<worker-to-gateway credential>
CARRIER_GATEWAY_SIGNING_SECRET=<at least 32 bytes from the secret manager>
```

The URL must use HTTPS. `CARRIER_GATEWAY_ALLOW_INSECURE_HTTP=true` exists only for
isolated local testing. When the URL is absent the carrier worker is disabled while
the outbox and reconciliation workers continue normally.

Optional bounded controls are `CARRIER_BATCH_SIZE` (default 20),
`CARRIER_TENANT_PAGE_SIZE` (100), `CARRIER_LEASE_SECONDS` (60),
`CARRIER_REQUEST_TIMEOUT_SECONDS` (20), `CARRIER_RETRY_DELAY_SECONDS` (5),
`CARRIER_RETRY_DELAY_CAP_SECONDS` (300), `CARRIER_MAX_ATTEMPTS` (10), and
`CARRIER_POLL_INTERVAL_SECONDS` (1). The request timeout must remain shorter than
the lease.

## Gateway contract

The worker sends JSON with schema version 1, a stable `request_key`, exact account,
carrier, service, origin, destination, and carton weight/dimension snapshots. Every
retry sends the same body and request key. Requests include:

- `Authorization: Bearer …`;
- `Idempotency-Key`: the stable request key;
- `X-Wareboxes-Carrier-Timestamp`: Unix seconds;
- `X-Wareboxes-Carrier-Signature`: `v1=` plus lowercase HMAC-SHA256 of
  `<timestamp>.<raw-body>`.

The gateway must authenticate before parsing, reject stale timestamps, compare the
signature in constant time, and deduplicate the request key before calling a carrier.
An identical retry returns the original manifest reference and carton tracking set.
A different payload under an existing key must be rejected.

A successful response repeats schema version 1 and the request key, supplies one
manifest reference, and contains exactly one unique tracking number for every input
carton. Wareboxes rejects missing, additional, duplicate, or mismatched cartons and
responses larger than 2 MiB. HTTP 408, 429, and 5xx are retryable; an integer
`Retry-After` value is honored within the configured cap. Other HTTP failures and
invalid response shapes require supervisor recovery.

## Operator workflow and recovery

An administrator configures a carrier account from Shipping for one exact client and
facility. The account key is a credential selector, never a password or API token.
Each reconfiguration, enable, or disable creates an immutable revision. A shipment
job freezes the selected revision and its exact request SHA-256.

Shipping shows queued, processing, retry-scheduled, failed, cancelled, and succeeded
attempts. Operators may cancel queued work. A supervisor may retry a failed job; the
retry preserves the original request key and snapshot even if the account was later
reconfigured. Manual tracking remains an explicit fallback and is blocked while an
automated job is active.

Do not edit carrier, shipment, manifest, tracking, attempt, or outbox rows. A failed
job should be corrected at the gateway or carrier-account configuration boundary and
then retried through Shipping. If a carrier accepted a request but the response was
lost, the gateway must replay its stored result for the same request key.

## Monitoring

Alert on failed jobs, repeated `retry_scheduled` transitions, jobs whose processing
lease repeatedly expires, oldest queued age, and gateway HTTP latency/error rate.
Correlate the job ID, request key, request SHA-256, attempt number, and worker ID.
Successful completion atomically records the shipment manifest, tracking packages,
attempt result, shipment event, and carrier-job event.
