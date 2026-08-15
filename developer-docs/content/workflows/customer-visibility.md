# Read customer visibility

The customer visibility API provides one owner- and facility-scoped projection
for inventory availability, order status, shipment status, tracking numbers, and
immutable shipment documents.

## Access boundary

The bearer identity must have the `customer_portal` permission. Results are also
intersected with its facility and inventory-owner scopes. Supplying an explicit
filter outside either scope fails with `403`; guessing a document identifier
outside scope returns `404`.

The projection intentionally omits internal locations, license plates, work
tasks, employee identities, and tenant metadata. Inventory is grouped by client,
facility, item, lot, expiration, UOM, and inventory status.

## Workspace and history

Call `GET /api/v1/portal/workspace`. Optional `inventory_owner_id`,
`facility_id`, and `search` filters narrow the response. By default, completed,
cancelled, and voided fulfillment history is omitted. Set
`include_history=true` when reconciling prior activity.

The response supplies stable download paths for each shipment document and the
inventory CSV report. Treat `generated_at` as the projection observation time;
it is not a warehouse event timestamp.

## Downloads

- `GET /api/v1/portal/documents/{document_id}/content` downloads a retained
  packing slip or carton-label set after rechecking current scopes.
- `GET /api/v1/portal/reports/inventory.csv` exports the same scoped inventory
  projection. CSV text fields are quoted and spreadsheet formula prefixes are
  neutralized.
