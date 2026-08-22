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

## Move a tenant

Tenant movement is a control-plane workflow around a deployment-plane copy. The
Wareboxes API never accepts database endpoints or credentials and does not run a
cross-cell copy itself. The approved deployment tool owns backup/restore or logical
replication, object copying, routing changes, and evidence collection. Wareboxes
reserves capacity, fences application writes, validates that evidence, changes the
home placement, and records every step atomically with an outbox event.

Before planning a move:

- switch the platform administrator's selected tenant to a stable control tenant;
- confirm the target is active, has the required residency, and has measured
  database, object-storage, and application headroom;
- confirm source and target backups, encryption, monitoring, and rollback routing;
- assign one deployment incident/change record to the copy and routing work.

Use **Platform operations → Cell moves** and follow the state machine without
skipping a step:

1. **Plan** the target against the tenant's current placement revision. Planning
   reserves a target slot, including dedicated-cell exclusivity.
2. **Start copy** with the non-secret deployment change or replication reference.
3. **Checkpoint** copy progress with source WAL LSN, target replay LSN, copied rows,
   and copied bytes. Values must move monotonically and a copying target may not
   claim replay ahead of the source.
4. **Freeze writes** only after a checkpoint. The command serializes with in-flight
   tenant mutations and installs a database-enforced tenant write fence. Reads and
   the move control plane remain available; ordinary tenant mutations fail closed.
5. Record a new **post-freeze checkpoint**, then **validate** exact row, data,
   schema, and object-manifest parity. The trusted validator must also prove
   inventory reconciliation, replay-safe command records, and outbox continuity.
6. **Cut over** using both the exact move revision and original placement revision.
   The placement change and immutable evidence commit together. The write fence
   remains installed, and the source slot becomes a rollback reservation.
7. Change deployment routing, then record **cutover verification**. The evidence
   binds the observed target cell and placement revision to a routing reference and
   proves target reads, the still-active write fence, inventory reconciliation,
   idempotency continuity, and outbox continuity.
8. **Complete** with the change record's closing reason. Completion is rejected
   without cutover verification and removes the fence atomically.

Each mutation requires a new idempotency key. Retry the identical request with the
same key after a timeout; Wareboxes returns the original result. Never reuse a key
for changed evidence. The operator page explains unavailable actions and stale
placement, target, fence, checkpoint, and validation conditions.

Before sending a move command, the operator page saves its exact body, idempotency
key, authenticated user, and selected control tenant in a dedicated durable browser
recovery record. Separate per-command records prevent concurrent tabs from
overwriting one another. The page blocks new commands while recovery is unavailable
or unresolved, retains records after a network failure, unreadable success response,
or HTTP 5xx, and only clears each record after a successful replay or a definitive
client error. After a reload, use **Retry exact command** for every retained record;
if the selected tenant changed, switch back to that record's control tenant first.

### Cancel and roll back

A planned, copying, frozen, or validated move may be cancelled. Cancellation of a
frozen or validated move removes the fence in the same transaction; retain or
remove deployment-plane copy artifacts according to the change record.

A cut-over move may be rolled back until it is completed, including after cutover
verification. While the write fence is still active, restore deployment routing to
the source and independently verify the routing reference, source reads, active
fence, inventory reconciliation, replay-safe command records, and outbox
continuity. The rollback request must identify the observed source cell and the
next placement revision. Wareboxes records that immutable proof, restores the
source placement, consumes the reserved source slot, records the new placement
revision, and only then releases the fence in one transaction. The source cannot
be retired or have its reserved capacity reassigned during this window. A
completed move cannot be rolled back through this workflow; use a newly planned
reverse move.

### Stuck move recovery

Do not edit placement, move, validation, event, or fence rows and do not delete a
fence manually. First retry the last identical command and inspect immutable event
history and outbox delivery. Then:

- cancel a pre-cutover move once the deployment copy has stopped safely;
- for a cut-over move whose target cannot pass verification, restore routing to
  the source while writes remain fenced, collect the required rollback safety
  proof, and invoke the rollback command;
- confirm live routing matches the placement recorded by the successful command
  before releasing incident control;
- escalate any database invariant failure rather than bypassing it. Deferred
  constraints require move phase, placement, write fence, and proof rows to agree.

The deployment change record must retain tool version, copy/routing references,
source and target LSNs, checksums, validation output, command request IDs,
idempotency keys, and the final move and placement revisions. Exercise completion
and rollback regularly against production-like cells.

## Monitoring

Alert on cells stuck in provisioning or draining, capacity exhaustion, tenant
provisioning conflicts, rejected residency placement, and outbox delivery lag for
`data_cell` events. Capacity is only the placement admission limit; scanner volume,
journal throughput, database headroom, recovery objectives, and noisy-neighbor
signals must be monitored independently.

Also alert on active moves with no recent revision, prolonged write fences, growing
copy replay lag, failed or stale validation, target or rollback reservation
exhaustion, rejected cutovers, rollbacks, and move-event outbox lag. Page an operator
immediately for a fenced tenant without an active frozen, validated, or cut-over
move. Reconcile routing against the recorded home placement after every cutover and
rollback.
