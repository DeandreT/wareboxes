# Submit fulfillment orders

Order intake converts partner demand into a Wareboxes fulfillment order while
preserving the original document and the mappings used.

## Processing sequence

```text
authenticate and authorize
  → resolve external inventory owner
  → retain raw payload and payload hash
  → parse the v1 order envelope
  → resolve item and UOM mappings
  → create fulfillment demand atomically
  → return processing evidence
```

Once the payload is retained, document failures are quarantined rather than
discarded.

## Field semantics

- `order_key` is the partner's stable order identity inside the inventory owner.
- `line_key` correlates individual demand lines and must be unique within the order.
- `external_item_key` and `external_uom` are resolved as a pair under the source.
- `quantity` is a positive integer in the mapped requested UOM.
- `rush` is a prioritization signal; it does not bypass inventory or release policy.
- `ship_by` is an RFC 3339 deadline used by warehouse planning. It is not a carrier delivery guarantee.
- `destination` is captured with the order; unknown fields are not accepted by the v1 envelope.

## Understanding `processed`

`status: processed` means the external document produced one Wareboxes fulfillment
order. It does not imply any later warehouse state:

```text
order created
  ≠ inventory reserved
  ≠ stock allocated
  ≠ wave released
  ≠ picked
  ≠ packed
  ≠ manifested or shipped
```

Use the customer visibility workspace to read current order and shipment state.
Do not infer shipment promises from the intake response itself; webhook delivery
remains a separate contract.

## Response evidence

Retain these values with the source order:

- `receipt_id` for the immutable inbound document;
- `input_payload_sha256` for payload reconciliation;
- `revision` and `attempt_count` for processing history;
- `mapping_version` for the canonical intake adapter version (individual item
  mapping revisions remain in Wareboxes processing evidence);
- `order_id` and `order_revision` when processing succeeds;
- `error_code` and `error_message` when quarantined.
