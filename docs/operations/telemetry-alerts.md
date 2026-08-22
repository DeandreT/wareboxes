# Telemetry and Alerts

The API emits structured JSON logs in production. HTTP spans include method, URI,
version, and the validated or generated `x-request-id`; command audit records,
integration attempts, and inventory transaction correlation IDs retain that request
identity where applicable. Keep logs in a restricted central sink and use request
IDs to correlate transport failures with durable workflow evidence.

Prometheus-format metrics are available only on the host-local `/metrics` endpoint.
Caddy intentionally returns 404 for public requests to that path. Configure one
scrape target per API process and load
`deploy/monitoring/wareboxes-alerts.yml` into the cell's Prometheus-compatible rule
evaluator. Preserve the `job="wareboxes-api"` label expected by the rules.

The private scrape also reports authoritative governed tenant-cell move state from
PostgreSQL. The collector enters a short, read-only transaction and uses a current
bootstrap-managed platform administrator as its transaction-local RLS context. It
exports only bounded lifecycle status labels; tenant IDs, move IDs, cell keys,
request IDs, and errors never become metric labels. A parameter-free,
`SECURITY DEFINER` database function supplies only the tenant-cell-move outbox count
and age aggregates. It validates the same platform actor and fails if its owner
cannot bypass tenant RLS, rather than returning a partial snapshot.

- `wareboxes_tenant_cell_moves_active{status}` reports the five active states:
  `planned`, `copying`, `frozen`, `validated`, and `cut_over`.
- `wareboxes_tenant_cell_move_oldest_active_age_seconds` reports elapsed time since
  the least recently revised active move last changed, using its request time only
  until the first revision.
- `wareboxes_tenant_write_fences_active` and
  `wareboxes_tenant_write_fence_max_age_seconds` report current write-fence risk.
- `wareboxes_tenant_write_fence_state_mismatches` reports missing, orphaned, or
  mismatched fences, including a fence epoch that does not exactly match the
  immutable `writes_frozen` event revision.
- `wareboxes_tenant_cell_moves_awaiting_post_cutover_verification` reports cut-over
  moves that cannot yet be completed safely.
- `wareboxes_tenant_cell_moves_awaiting_validation` reports write-frozen moves
  awaiting final validation.
- `wareboxes_tenant_cell_move_max_copy_replay_lag_bytes` reports the largest latest
  recorded source-to-target WAL checkpoint lag across copying and frozen moves.
- `wareboxes_tenant_cell_move_capacity_reservations{direction}` reports current
  target and source rollback capacity reservations without identifying a cell.
- `wareboxes_data_cells_exhausted_active` reports active shared cells whose placed
  tenants plus inbound and rollback reservations consume all configured capacity.
  Fully assigned dedicated cells are intentionally excluded.
- `wareboxes_tenant_cell_move_unpublished_outbox_events` and
  `wareboxes_tenant_cell_move_oldest_unpublished_outbox_age_seconds` report
  undiscarded move events still awaiting publication.
- `wareboxes_tenant_cell_move_outcomes_total{outcome}` counts accepted cutovers and
  terminal completion, rollback, and cancellation outcomes.
- `wareboxes_tenant_cell_move_command_rejections_total{command}` is a process-local
  counter for authorized `validate`, `cutover`, and `rollback` command attempts that
  returned an error. These are the only possible command label values.

The database snapshot has a two-second budget. If it fails or times out, `/metrics`
still returns process, HTTP, readiness, and pool metrics with HTTP 200,
`wareboxes_tenant_cell_move_metrics_collection_success` becomes `0`, and the
authoritative move series are omitted rather than published as false zeroes. The
same database snapshot appears on every API replica, so the supplied rules use
`max` or `min` by job instead of summing it. Process-local command rejection
counters do not depend on the database snapshot, remain available during collector
failure, and are summed by job only after applying `increase()` per replica.

The rules are a minimum baseline. Receiver routing and paging credentials belong in
the deployment secret manager, not this repository. Also alert from the service
manager or log platform when any of these events is absent or failed:

- daily `event=backup_completed`;
- weekly `event=restore_drill_completed`;
- monthly `event=command_archive_completed`;
- worker dead-letter creation or repeated publisher failure;
- API or worker process restart loops.

## API target down

Confirm whether the process is stopped or merely unreachable from the scraper.
Check `systemctl status wareboxes.service`, recent structured logs, host capacity,
and `/health/live`. If liveness works, repair the private scrape route without
opening `/metrics` publicly.

## Readiness failure

Query `/health/ready` locally. Database failures and schema-contract failures are
logged separately. Check PostgreSQL availability, pool exhaustion, migration
version, runtime-role validation, disk capacity, and recent deploys. Remove the
replica from traffic until readiness is stable.

## High server error rate

Group structured error logs by request ID and route span, then correlate affected
commands with idempotency, audit, inbox, and outbox records. Do not retry mutating
requests without their original idempotency keys.

## Database pool saturation

Check query latency, locks, PostgreSQL connection count, and traffic-control
rejections before increasing pool capacity. More connections can worsen database
contention. Compare the event with the measured load envelope and scale replicas or
remove the blocking query only after identifying the constraint.

## Tenant cell move metrics unavailable

Check `wareboxes_tenant_cell_move_metrics_collection_success` across every API
replica, then inspect API logs for `tenant-cell move metrics collection failed` or
`timed out`. Confirm database reachability, pool availability, runtime-role grants,
the current migration, and that at least one active bootstrap-managed platform
administrator exists. Do not weaken row-level security or expose `/metrics`
publicly. Restore the collector before treating the absence of the move, fence, or
verification gauges as an all-clear.

## Tenant cell move stuck

Open **Platform operations → Cell moves**, filter to active moves, and identify the
oldest record and its current status. Correlate its immutable event history and
request IDs with the deployment change, copy tooling, PostgreSQL replay state, and
tenant-cell-move outbox delivery. Retry a timed-out mutation only with its original
idempotency key. If a pre-cutover copy will not continue, stop the deployment copy
safely and cancel through the governed workflow. If the move has cut over, complete
verification or roll back; never edit placement or move rows directly. The
six-hour threshold measures time since the latest recorded revision, not total copy
duration. It is an initial baseline and should be tuned to the measured cell copy
envelope without relaxing fence or verification thresholds.

## Tenant write fence state mismatch

Page immediately and stop tenant mutation traffic at the deployment boundary while
keeping routing fixed. In **Platform operations → Cell moves**, compare every active
`frozen`, `validated`, or `cut_over` move with its fence and immutable
`writes_frozen` event. The tenant, move, frozen actor/time, and exact event revision
must agree; no other move state may retain a fence. Preserve logs and a database
snapshot, then use the governed recovery or restore procedure. Never insert, delete,
or repair fence, move, placement, or event rows manually.

## Tenant cell move copy replay lag

Open **Platform operations → Cell moves**, filter to `copying` and `frozen`, and
compare each move's latest source WAL and target replay checkpoints. Check whether
the target replay LSN is advancing, then inspect the copy process, replication-slot
health, target storage and compute pressure, network throughput, and deployment
change logs. The metric reflects the latest recorded checkpoint, so confirm live
replication state before deciding that lag is still growing. If a copying move
cannot recover, stop the deployment copy safely and cancel through the governed
workflow. If the move is frozen, treat restoration as tenant-impacting and proceed
only after the target catches up and a new post-freeze checkpoint is recorded.
Never fabricate checkpoint LSNs or validate or cut over while replay is behind. The
one-GiB threshold is an initial baseline and should be tuned to the measured WAL
and copy envelope.

## Tenant cell move outbox lag

Treat the move's external audit and integration history as incomplete. Inspect the
outbox worker, ordering blockers, delivery attempts, dead letters, database locks,
and publisher connectivity. Correlate by request ID in restricted logs and use the
integration monitoring replay workflow for a replay-safe retry; do not update or
delete outbox rows directly. Keep the incident open until the unpublished count and
oldest age return to zero and downstream delivery is confirmed.

## Active data cell capacity exhausted

Open **Platform operations → Data cells** and verify the shared cell's configured
capacity against placed tenants, inbound move reservations, and source rollback
reservations. Do not cancel or complete a move merely to clear the alert. Finish or
recover valid governed moves, or register and activate additional compliant
capacity before planning more placements. A fully assigned dedicated cell is normal
and does not contribute to this metric.

## Tenant cell move validation pending

Treat this as tenant-impacting because ordinary writes remain fenced. In
**Platform operations → Cell moves**, identify the `frozen` move and confirm its
latest checkpoint was recorded after the write-fence revision. Verify source and
target WAL convergence, row and checksum parity, schema and object-manifest parity,
inventory reconciliation, replay-safe command records, outbox continuity, target
health, and immutable event history. Record final validation with a new idempotency
key only when every proof is current; retry an ambiguous mutation only with its
original request and idempotency key. If validation cannot be proven, stop the copy
safely and cancel through the governed workflow. Never delete the fence or edit
move, checkpoint, or validation rows directly.

## Tenant cell move validation rejected

Find the failed request ID in structured logs and inspect the move's current
revision, write fence, final checkpoint, validation evidence, and immutable event
history. If the response was ambiguous, first read the durable move state and retry
only the identical request with its original idempotency key. For a definitive
evidence or revision rejection, correct the evidence and submit a new command with a
new key. Never bypass validation or release the fence manually.

## Tenant write fence prolonged

Treat this as tenant-impacting: ordinary writes remain blocked while the fence is
present. In **Platform operations → Cell moves**, verify that the fenced tenant has
exactly one `frozen`, `validated`, or `cut_over` move, then check the last event,
copy/validation evidence, routing, target health, and outbox delivery. Advance to
validation, cutover verification, and completion when their evidence is valid;
otherwise cancel before cutover or roll back after cutover. Never delete a fence or
change placement manually—the terminal command must release the fence atomically.

## Tenant cell move verification pending

Confirm routing sends the tenant to the recorded target placement and that target
reads, inventory reconciliation, replay-safe command history, and outbox continuity
match the cutover-verification evidence. Record verification with a new
idempotency key, then complete the move to release the fence. If target routing or
verification cannot be proven, restore routing and roll back through the governed
workflow. Keep the incident open until the recorded home placement, live routing,
and fence state agree.

## Tenant cell move cutover rejected

Treat this as tenant-impacting and leave the write fence in place. Correlate the
request ID with the durable command record, move revision, final validation,
placement revision, cell health, and routing change. Determine from the read model
and immutable events whether cutover committed before retrying: ambiguous attempts
use the identical request and original idempotency key; definitive rejections require
corrected state and a new key. Keep routing on the recorded source unless durable
placement proves that cutover committed, and never change placement manually.

## Tenant cell move rollback recorded

Open an incident even when rollback was operator-initiated. Confirm the immutable
rollback verification, resulting source placement, live source routing, source
reads, inventory reconciliation, idempotency history, outbox continuity, and fence
release all agree. Preserve the deployment change and request IDs, investigate the
cutover failure, and do not attempt another move until the cause and capacity impact
are understood.

## Verification

CI syntax-checks the production rule file and executes
`deploy/monitoring/wareboxes-alerts.test.yml` with `promtool test rules`. The unit
fixtures prove multi-replica aggregation and alert hold times, including `max` for
the replicated durable rollback counter and `sum` for process-local command
rejection counters.

After deployment, verify that liveness and readiness are independently scraped,
exercise the stuck-move, copy-lag, pending-validation, prolonged-fence, and
pending-verification rules in a non-production environment. Also inject a fence
state mismatch, stalled move outbox event, exhausted shared cell, rejected
validation and cutover, and accepted rollback. Confirm receiver delivery and record
the exercise. Repeat after monitoring topology, labels, routing, or move thresholds
change.
