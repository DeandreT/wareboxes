# Edge-agent operations

The Wareboxes edge agent is an optional local bridge for PLC, conveyor, robotics,
sortation, printer, and scale systems. The WMS remains the source of bounded work
instructions and inventory decisions; the edge agent correlates those instructions
to local equipment and never allocates or adjusts stock by itself.

## Safety and recovery contract

Every durable command includes tenant, facility, and device scope, a command ID, a
correlation ID, an idempotency key, a versioned typed payload, and exactly one
recovery policy:

- `device_deduplicated_replay` is accepted only when the adapter declares durable
  downstream duplicate protection. The same command and correlation IDs are sent
  on every attempt.
- `probe_then_retry` requires the adapter to query the controller by stable identity
  after an ambiguous result or process restart. The command is not sent again until
  that probe reports it absent.
- `manual_review` never guesses after an ambiguous result. The command and its
  device enter manual fallback for operator reconciliation.

Adapters must report an error as `retryable` only when the controller did not accept
the instruction, `permanent` only when it rejected the instruction without a
physical effect, and `ambiguous` whenever acceptance cannot be proven. Incorrectly
classifying an ambiguous outcome as retryable defeats duplicate protection.

The local SQLite store uses WAL mode, full synchronous writes, foreign keys, atomic
claims, and immutable attempt, command-transition, control, and heartbeat history.
An expired execution lease becomes a deduplicated retry, a recovery probe, or a
manual review according to the persisted policy. Command identity and payload are
immutable; an exact retry returns the original record, while reuse with changed
content fails.

## Control modes

New devices are always `disabled`. A device can be `automatic`, `disabled`, or in
`manual_fallback`. Disabling a device or entering manual fallback quarantines every
queued, retrying, recovering, or in-flight command as `manual_review`. Enabling the
device does not silently requeue those commands. Operators must resolve each one as
completed manually, cancel it, or explicitly retry it after inspecting the physical
system.

Resume automation only after isolating equipment, checking controller queues and
physical work, reconciling all ambiguous commands, clearing faults and guarding,
and validating a fresh heartbeat. The CLI requires the literal
`CONFIRM-SAFE-TO-RESUME` token to make that acknowledgement explicit.

## Local operator CLI

Set `EDGE_STORE_PATH` to the protected local database path. Reasons and notes that
contain spaces must be passed as one quoted argument.

```bash
export EDGE_STORE_PATH=/var/lib/wareboxes-edge/edge.sqlite3

wareboxes-edge-agent register scale-01 tenant-1 facility-1 scale \
  "Pack scale 01" operator-42 "initial commissioning"
wareboxes-edge-agent status
wareboxes-edge-agent command command-123
wareboxes-edge-agent attempts command-123
wareboxes-edge-agent command-events command-123
wareboxes-edge-agent control-events scale-01
wareboxes-edge-agent heartbeats scale-01
wareboxes-edge-agent manual-fallback scale-01 operator-42 \
  "unstable readings; use inspected backup scale"
wareboxes-edge-agent resolve command-123 operator-42 \
  "weight captured on backup scale and WMS exception completed"
wareboxes-edge-agent resume scale-01 operator-42 \
  "fault cleared and outstanding commands reconciled" CONFIRM-SAFE-TO-RESUME
```

Other controls are `disable`, `retry`, and `cancel`. The CLI deliberately has no
free-form command-enqueue operation: trusted WMS transport code must construct the
typed command and submit it through `EdgeEngine`, and a compiled vendor adapter must
be registered for the exact device scope.

## Vendor adapter implementation

Implement one of `PlcDriver`, `ConveyorDriver`, `RoboticsDriver`, `SortationDriver`,
`PrinterDriver`, or `ScaleDriver`, then wrap it with the corresponding typed bridge
before registering it. Keep protocol libraries, credentials, address maps, and
device-specific status translation inside that adapter. Propagate the envelope's
stable identities to the downstream controller and implement recovery against the
controller's durable command history when advertising that capability.

Use one local command runner per store. Size the execution lease above the adapter's
bounded controller timeout. A second process may safely inspect or change control
state; if it quarantines an in-flight command, the original execution can no longer
commit a success under its revoked lease and the physical outcome must be
reconciled.

## Heartbeats and incident response

The engine records a heartbeat before every attempt. `healthy` and `degraded` may
execute; `unknown` and `offline` retry without issuing the physical command;
`faulted` immediately enters manual fallback. Alert on stale heartbeats, growing
health-failure counts, manual-review commands, retry exhaustion, and any device in
manual fallback longer than the facility's recovery objective.

For an incident:

1. Physically stop or isolate affected equipment, then place the device in manual
   fallback.
2. Compare command, attempt, control, and heartbeat history with the controller's
   durable log using command and correlation IDs.
3. Complete safe warehouse work manually and resolve the command, or prove the
   controller never accepted it and explicitly retry it.
4. Reconcile affected WMS work and inventory before resuming automation.

Cloud command ingestion, outbound health publication, secrets provisioning, and
facility-specific vendor drivers remain deployment integrations. They must use
outbound authenticated connections and the typed engine boundary; they must not
bypass the durable store or permit stale edge state to create inventory decisions.

The repository's executable durable-command throughput and ambiguous-recovery
baseline is documented in
[`automation-envelope.md`](automation-envelope.md). Repeat that gate on each
qualified edge host and record the stricter facility/controller budget before
enabling automatic mode.
