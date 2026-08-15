# Submit X12 940 warehouse shipping orders

Wareboxes supports a deliberately narrow X12 004010 940 profile for new
fulfillment demand. The raw interchange enters the same durable inbox as JSON
orders, with source-specific owner, item, and UOM mappings, quarantine evidence,
operator correction, and exact replay.

Send one transaction per interchange to:

```text
POST /api/v1/integrations/x12-940/{source_key}/inventory-owners/{external_owner_key}/orders
Content-Type: application/edi-x12
Authorization: Bearer <credential>
X-Wareboxes-Tenant-Id: <tenant>
Idempotency-Key: <stable interchange and transaction identity>
```

## Wareboxes v1 profile

The adapter requires:

- one fixed-width `ISA`/`IEA` interchange and one `GS`/`GE` functional group;
- `GS01=OW`, one `ST01=940` transaction, matching control numbers, and an exact
  `SE01` segment count;
- `W0501=N` and the partner order identity in `W0502`;
- one `N1*ST` ship-to loop followed by `N3` and `N4`; a `PER` contact is optional;
- each demand line as `LX` followed by `W01`, with whole quantity in `W0101`,
  external UOM in `W0102`, `SK` or `VP` in `W0104`, and external item identity in
  `W0105`;
- optional `N9*RU*Y` for rush demand and `G62*10*CCYYMMDD*HHMMSS` for ship-by time.

Change, replacement, and cancellation transactions are quarantined; they are not
silently interpreted as new demand. Coordinate a partner implementation guide
before production because X12 permits choices outside this supported profile.

## Outcomes and recovery

HTTP `202` means the interchange is durable. A `processed` response identifies the
created fulfillment order and reports adapter
`x12.940.warehouse_shipping_order` version `1`. A `quarantined` response identifies
the retained receipt and a stable error category. Operators can download the exact
raw interchange, correct mapping or canonical demand, and replay without database
access.

Use one idempotency key for the same raw interchange. Reusing it with changed bytes,
content type, source, or owner returns a conflict.
