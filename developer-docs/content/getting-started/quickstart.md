# Quickstart

This example submits one fulfillment order using partner-facing owner, item, and
UOM identities.

## Before you send an order

Wareboxes onboarding must provide:

- an API base URL and opaque bearer credential;
- your numeric tenant context;
- a provisioned `source_key` for the sending system;
- an external inventory-owner key mapped to the correct Wareboxes inventory owner;
- item and UOM mappings for each `(source, external item, external UOM)` combination.

## Submit the order

```bash
curl --request POST \
  "${WAREBOXES_API_URL}/api/v1/integrations/order-intake/partner-api/inventory-owners/NORTHSTAR/orders" \
  --header "Authorization: Bearer ${WAREBOXES_API_TOKEN}" \
  --header "X-Wareboxes-Tenant-Id: ${WAREBOXES_TENANT_ID}" \
  --header "Idempotency-Key: northstar-order-SO-1001-v1" \
  --header "X-Request-Id: northstar-order-SO-1001-attempt-1" \
  --header "Content-Type: application/json" \
  --data-binary @- <<'JSON'
{
  "order_key": "SO-1001",
  "rush": false,
  "ship_by": "2027-08-12T17:00:00Z",
  "destination": {
    "recipient_name": "Receiving Team",
    "company": "Northstar Retail",
    "phone": "+1 775 555 0100",
    "email": "receiving@example.com",
    "line1": "125 Shipping Lane",
    "line2": "Dock 4",
    "city": "Reno",
    "region": "NV",
    "postal_code": "89502",
    "country": "US"
  },
  "lines": [
    {
      "line_key": "1",
      "external_item_key": "CLIENT-CASE",
      "external_uom": "CS",
      "quantity": 4
    }
  ]
}
JSON
```

A successfully mapped order returns `202 Accepted` with `status: processed` and
an `order_id`. Acceptance means Wareboxes created fulfillment demand. It does
not mean inventory has been reserved or allocated, or that warehouse work has
been released.

```json
{
  "receipt_id": 501,
  "processing_id": 601,
  "processing_attempt_id": 701,
  "correction_id": null,
  "input_payload_sha256": "4cacc15b0023683e11cc4c371c585f8aefe1a12221edeb64290fbe35be4e4ccd",
  "inventory_owner_id": 42,
  "adapter_key": "wareboxes.fulfillment_order",
  "mapping_version": 2,
  "status": "processed",
  "revision": 1,
  "attempt_count": 1,
  "applied_mapping_count": 1,
  "order_id": 9001,
  "order_revision": 1,
  "error_code": null,
  "error_message": null,
  "attempted_by": 7,
  "attempted_at": "2026-08-11T19:30:00Z",
  "processed_at": "2026-08-11T19:30:00Z"
}
```

If the response instead has `status: quarantined`, the payload is durable but no
order was created. See [quarantine and recovery](/workflows/quarantine).

## Retry safely

If the connection closes or times out before you receive a response, resend the
same method, path, headers, body bytes, and idempotency key. An exact retry returns
the original outcome. Never create a second key merely because the first response
was lost.
