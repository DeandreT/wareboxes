# Quarantine and recovery

Quarantine separates durable receipt from successful warehouse processing. It is
designed for integrations where losing a document is worse than holding it for
controlled remediation.

## Quarantine reasons

| `error_code` | Meaning |
| --- | --- |
| `invalid_payload` | The retained bytes are not a valid v1 JSON order envelope |
| `mapping_validation_failed` | Parsed values violate the order or mapping contract |
| `item_mapping_not_found` | No active item/UOM mapping exists for at least one line |
| `business_rejected` | Current warehouse business rules rejected order creation |

A quarantined response has no `order_id` or `order_revision`. Do not create a new
idempotency key and resubmit automatically; that creates a second receipt and makes
reconciliation harder.

## Operator recovery

Wareboxes operators can review the retained payload and mapping evidence, repair
configuration, and reprocess the same receipt. If the source document itself is
wrong, an authorized operator can record a correction with an audit reason.

Reprocessing and correction endpoints are intentionally not part of the partner
API. They require operator authorization and preserve an internal attempt ledger.

## Partner behavior

When you receive `status: quarantined`:

1. Persist the receipt ID, payload hash, error code, and request ID.
2. Mark the source document as accepted-but-held, not failed transport.
3. Alert through the agreed integration support channel.
4. Wait for the same receipt to be recovered or explicitly rejected.

Outbound recovery notifications are planned; until they are published, recovery
status is coordinated operationally.
