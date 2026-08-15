# Wareboxes Integration API

Use the Wareboxes Integration API to exchange warehouse demand and operational
state without coupling your system to internal database identities or warehouse
execution details.

Version 1 includes fulfillment order intake and a customer-safe visibility
surface. Additional inbound document and event contracts are added as complete
partner workflows, rather than exposing internal operator endpoints one at a time.

## Design guarantees

- Every request executes inside one authenticated tenant boundary.
- Inventory-owner access is explicit and independently scoped.
- Mutating requests are replay-safe through caller-supplied idempotency keys.
- Partner identifiers are retained alongside the mapping versions used to process them.
- Invalid or unmapped business documents can be quarantined without losing the original payload.
- HTTP acceptance, warehouse processing, allocation, picking, packing, and shipping are distinct states.

Start with the [quickstart](/getting-started/quickstart), then read
[warehouse identities and scope](/concepts/warehouse-identities) and
[idempotency and retries](/concepts/idempotency) before sending production data.

## Current public surface

| Workflow | Availability | Meaning |
| --- | --- | --- |
| Fulfillment order intake | Available in v1 | Retain, map, and create outbound demand |
| X12 940 order intake | Available in v1 | Validated 004010 profile mapped through the same durable inbox |
| Inventory availability | Available in v1 | Owner/facility-scoped stock without internal positions or containers |
| Order and shipment status | Available in v1 | Partner-visible fulfillment, carrier, and tracking state |
| Shipment documents and inventory CSV | Available in v1 | Scope-safe downloads for customer operations |
| Webhooks | Available | HMAC-SHA256 signed, replayable outbound business events |
| Outbound SFTP exchange | Available | Host-verified, atomic JSON document delivery |

An endpoint is not part of the public contract unless it appears in the
[Integration API v1 reference](/api/v1).
