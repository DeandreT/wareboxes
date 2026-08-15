# Receive Wareboxes events

Wareboxes publishes committed operational events at least once. Your consumer must
deduplicate `event_key` and tolerate a replay of an already accepted event.

## Verify an HTTPS webhook

Read the request body as bytes before parsing JSON. Parse
`X-Wareboxes-Webhook-Timestamp`, reject timestamps outside your replay window, and
compute HMAC-SHA256 over:

```text
<timestamp>.<exact request body bytes>
```

Compare `v1=<lowercase hex HMAC>` with `X-Wareboxes-Webhook-Signature` using a
constant-time comparison. Authenticate the bearer credential as a separate check.
Only after both checks pass should you claim `X-Wareboxes-Webhook-Id` in your
idempotency store and process the event.

Return a 2xx response after durable acceptance. Wareboxes retries request timeouts,
HTTP 408, 425, 429, and server errors. Other non-2xx responses are recorded as
permanent failures for operator review.

## Consume outbound SFTP files

Wareboxes can instead publish JSON envelopes over SFTP. Completed files use
`<sha256-of-event-key>.json`; hidden `.upload` files are incomplete and must be
ignored. Verify the `event_key` inside the envelope against your idempotency store,
because operator replay may replace a previously delivered filename.

SFTP accounts are key-only and the Wareboxes worker pins the server host key.
Coordinate key and directory changes with the warehouse operator before cutover.
