# Errors and request IDs

HTTP errors use a stable JSON envelope:

```json
{
  "reason": "idempotency_key_reused",
  "message": "idempotency key was already used with a different request",
  "request_id": "northstar-order-SO-1001-attempt-1",
  "violations": []
}
```

`reason` is intended for program logic. `message` is operator-facing and may gain
clarity without changing the reason. `violations` is omitted when there are no
field-level failures.

## Request correlation

Send an optional `X-Request-Id` containing 1–128 letters, digits, `-`, `_`, `.`, or
`:`. Wareboxes echoes a valid value in the response header and body error envelope;
otherwise it assigns one.

Record request IDs in integration logs, but do not use them as idempotency keys.
Request IDs identify transport attempts; idempotency keys identify business
commands.

## HTTP errors versus quarantine

An HTTP error means Wareboxes could not safely accept the submission under the
requested identity and transport contract. A `202` response with
`status: quarantined` means the raw document is retained and can be investigated
without asking the partner to reconstruct it.

Malformed order JSON and body-level mapping or business validation failures are
currently represented as durable quarantine outcomes. Missing authentication,
invalid headers, unknown owner mappings, incompatible idempotent reuse, oversized
bodies, and unsupported media types remain HTTP errors.
