# Authentication

Wareboxes currently authenticates API integrations with an opaque bearer
credential provisioned during onboarding.

```http
Authorization: Bearer <opaque credential>
X-Wareboxes-Tenant-Id: 12
```

The bearer value is not a JWT. Treat it as a password: store it in a secret
manager, transmit it only over TLS, never place it in URLs or logs, and rotate it
through the onboarding process if it may have been exposed.

## Tenant context

An API credential identifies a principal, while `X-Wareboxes-Tenant-Id` selects
the tenant in which the request is evaluated. Wareboxes verifies membership and
fails closed when the header is missing, malformed, or names a tenant the identity
cannot access.

The tenant header does not select an inventory owner. Inventory-owner access is
evaluated separately using the identity's owner scope and the endpoint resource.

## Permissions and scopes

Order intake requires the `orders` permission and access to the inventory owner
resolved from the external owner key. Facility-scoped endpoints will additionally
require access to the relevant facility.

Dedicated service-account lifecycle management is not yet self-service. Do not
reuse an interactive warehouse operator's personal credential for an integration.
