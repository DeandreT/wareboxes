# Integration Order Intake

Wareboxes accepts canonical JSON orders and a versioned X12 004010 940 profile
through the integration inbox. Both adapters retain the exact raw payload before
mapping or executing it. A successful HTTP response means the inbox processing and
canonical order command committed; it does not bypass order validation, allocation,
or warehouse policy.

## Partner setup

An integration administrator must configure and activate both mappings before a
partner submits demand:

- an inventory-owner mapping from the partner `source_key` and external owner key
  to one Wareboxes inventory owner; and
- item mappings from the partner item keys to owner-scoped items and their canonical
  order UOM.

Mappings are versioned, effective-dated, audited, and retained as evidence on every
processing attempt. Retiring or replacing a mapping affects a future processing
attempt, never the evidence of an earlier one.

## Canonical JSON intake

Submit a typed order to:

```text
POST /api/v1/integrations/order-intake/{source_key}/inventory-owners/{external_owner_key}/orders
```

Supply an `Idempotency-Key` and a unique external message key. An identical retry
returns the original result. Reusing either identity with a changed owner mapping or
payload is rejected.

## X12 940 profile

Submit one X12 transaction to:

```text
POST /api/v1/integrations/x12-940/{source_key}/inventory-owners/{external_owner_key}/orders
Content-Type: application/edi-x12
```

The Wareboxes v1 profile accepts one ISA/GS/ST transaction, `ST01=940`, `W0501=N`,
one ship-to `N1/N3/N4` loop, optional `N9*RU` routing and `G62*10` requested ship
date, and `LX/W01` lines identified by `SK` or `VP`. Envelope control numbers and
segment counts must reconcile. Unsupported purpose codes, unknown segments required
for meaning, unmapped items, or malformed envelopes are quarantined rather than
partially applied.

## Diagnose, correct, and replay

Use **Administration → Integrations → Inbound** to inspect the bounded preview,
download the retained payload, and review each immutable attempt and mapping
snapshot. For a quarantined message:

1. correct the owner/item mapping or submit an attributed canonical correction;
2. verify the new mapping revision and correction shown in the detail view; and
3. select **Reprocess** with the current processing revision.

Reprocessing creates a new immutable attempt and executes at most one canonical
order. Processed receipts are terminal. Scope loss deliberately makes a receipt and
its idempotent replay appear not found. Never update inbox, mapping-evidence, or
order rows directly.
