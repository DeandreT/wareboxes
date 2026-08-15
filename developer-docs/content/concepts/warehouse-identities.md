# Warehouse identities and scope

Warehousing systems often use the word “account” for several unrelated concepts.
Wareboxes keeps them separate so that authorization and inventory ownership remain
unambiguous.

| Concept | Meaning | Selected by |
| --- | --- | --- |
| Tenant | SaaS customer organization and hard data-isolation boundary | Authenticated tenant context |
| Inventory owner | Client or legal owner of stock inside a tenant | Owner mapping or explicit owner resource |
| Facility | Physical warehouse operated within a tenant | Facility resource or workflow configuration |
| Source | Sending system or partner channel | Provisioned `source_key` |

## External identities

Order intake deliberately accepts `external_inventory_owner_key`,
`external_item_key`, and `external_uom` rather than Wareboxes database IDs.
Mappings are scoped to the tenant, source, and inventory owner.

An item mapping resolves this tuple:

```text
(tenant, inventory owner, source, external item key, external UOM)
```

to one active Wareboxes item and requested UOM. The submitted quantity remains the
quantity for that mapped UOM; the intake API does not silently perform unit
conversion.

## Mapping evidence

Wareboxes records the mapping revision used for every accepted processing attempt.
Changing a mapping later does not rewrite historical evidence or change an exact
idempotent replay.

If an owner is outside the caller's scope, the API avoids exposing whether it
exists. Integrations must not discover owners, facilities, or stock by guessing
identifiers.
