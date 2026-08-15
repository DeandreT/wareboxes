# Idempotency and retries

Warehouse integrations must assume ambiguous network failures. A caller can time
out after Wareboxes commits an order but before the response reaches the caller.
Every mutating public operation therefore requires an `Idempotency-Key`.

## Choosing keys

Use a stable identity derived from the source business operation, not from an HTTP
attempt. Keys must use visible ASCII and may be at most 200 bytes.

```text
northstar-order-SO-1001-v1
```

Good keys identify the command and remain unchanged across network retries. Avoid
timestamps, random values generated per attempt, credentials, customer PII, or a
single key reused for every order.

## Replay behavior

For order intake, Wareboxes binds the key to the tenant, source, external owner
scope, content type, and payload. The outcomes are:

| Retry | Result |
| --- | --- |
| Same key and identical submission | Original `202` outcome is returned |
| Same key with a different payload or scope | `409 Conflict` |
| New key with the same order | A new command; business uniqueness rules may reject or quarantine it |

Exact replay returns the original processing evidence even if mappings or
configuration have changed since the first attempt.

## Retry policy

- Retry connection failures and timeouts with the same key and payload.
- Retry `500` responses with bounded exponential backoff and jitter.
- Respect `429` and `Retry-After` when rate limiting is introduced.
- Do not automatically retry `400`, `401`, `403`, `404`, `409`, `413`, or `415`.
- A `202` quarantined result is a completed intake outcome, not a transport failure.

Persist the idempotency key and Wareboxes `request_id` with the source document so
operators can reconcile both systems.
