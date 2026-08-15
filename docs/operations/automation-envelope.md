# Automation Throughput and Recovery Envelope

The Milestone 4 edge baseline validates the durable command boundary independently
of any vendor controller. It uses the production SQLite configuration (WAL,
`synchronous=FULL`, foreign keys, and a real file), typed device adapters, stable
command identities, and the normal execution/recovery engine.

The default acceptance mix is:

| Phase | Volume | Budget |
| --- | ---: | ---: |
| Durable command submissions | 11,000 | at least 100 commands/s; p99 at most 100 ms |
| Healthy automatic execution | 11,000 | at least 75 commands/s |
| Ambiguous outcome probes | 1,000 | all resolved within 5 seconds |
| Exact durable replays | 10,000 | at least 200 commands/s; p99 at most 50 ms |

The recovery devices deliberately lose their execution acknowledgements. The
engine must persist the ambiguous state, avoid blind replay, probe with the same
command and correlation identities, and reconcile every command to its durable
result. The gate also disables a device and proves that new work is quarantined in
manual review instead of reaching the adapter.

Run the isolated release-mode gate with:

```bash
scripts/test-automation-envelope.sh
```

The executable prints structured phase results and exits nonzero when a budget or
reconciliation invariant is missed. Volumes and limits can be overridden through
the `AUTOMATION_*` variables defined in
`apps/edge-agent/examples/automation_envelope.rs` when qualifying a facility host.

This is the repository's minimum single-agent envelope, not a controller or
facility throughput promise. A production commissioning record must repeat it on
the deployed edge hardware and separately measure the selected vendor adapter,
controller round-trip, physical equipment rate, cloud-link outage duration, and
manual recovery objective. The minimum gate must remain green when facility-specific
budgets are raised.
