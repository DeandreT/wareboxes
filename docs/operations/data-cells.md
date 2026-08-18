# Data-Cell Registry Operations

Wareboxes records each deployable data cell separately from the tenants placed in
it. The registry is control-plane evidence: it contains a stable key, display name,
region, residency jurisdiction, isolation mode, lifecycle state, and tenant
capacity. Database endpoints, credentials, encryption keys, and other secrets stay
in the deployment secret plane and must never be entered in the registry.

## Register and activate a cell

Only a platform administrator can use **Platform operations → Data cells**.
Register a cell after its database, application replicas, connection pool, object
storage, observability, backups, and recovery controls exist. A shared cell accepts
up to its configured tenant capacity. A dedicated cell always has capacity one.

New cells begin in `provisioning` and cannot receive tenants. Before activation,
verify:

- the region and residency code match the deployed infrastructure;
- restore, failover, encryption, monitoring, and alert tests passed;
- capacity reflects a measured operational envelope, not only tenant count;
- the deployment routing layer recognizes the permanent cell key.

Activation, reconfiguration, draining, reactivation, and retirement require the
cell's exact revision and an attributed reason. Exact retries return the original
result; reusing an idempotency key with another request is rejected. Every accepted
revision creates immutable evidence and a transactional outbox event.

## Place a tenant

Provisioning a tenant requires one active cell and an explicit residency
requirement. `GLOBAL` permits any cell jurisdiction; any other code must exactly
match the selected cell. Creation locks the cell while checking status, isolation,
and capacity, then creates the tenant and its immutable initial placement in the
same transaction. Concurrent requests cannot overfill a shared or dedicated cell.

The local development baseline includes an active `local-default` shared cell with
`GLOBAL` residency. It exists so direct development fixtures remain coherent. A
production deployment should register and activate its real cells before it accepts
tenant provisioning.

## Drain and retire

Draining immediately blocks new tenant placements while existing tenants continue
to use their current home. Do not change deployment routing by hand. Move each
tenant through the governed movement workflow, verify the source is empty, and only
then retire the cell. Retirement is terminal and is rejected while any placement
remains.

Never edit registry, placement, evidence, command-result, or outbox rows directly.
Use the platform controls so optimistic revision checks, audit attribution,
residency rules, and downstream publication remain atomic.

## Monitoring

Alert on cells stuck in provisioning or draining, capacity exhaustion, tenant
provisioning conflicts, rejected residency placement, and outbox delivery lag for
`data_cell` events. Capacity is only the placement admission limit; scanner volume,
journal throughput, database headroom, recovery objectives, and noisy-neighbor
signals must be monitored independently.

