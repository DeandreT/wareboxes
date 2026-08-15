# Baseline Operational Load Envelope

The Milestone 0 baseline is a single Wareboxes API process and PostgreSQL 16 on
the same low-latency network. The acceptance dataset contains 1,000 owner-scoped
inventory positions and 250 fulfillment orders. The harness uses a real release
build, restricted runtime database role, tenant-scoped authentication, and HTTP
serialization.

The current acceptance budgets are:

| Phase | Volume | Concurrency | p95 | p99 | Minimum throughput | Error budget |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Scoped inventory/order reads | 400 | 16 | 250 ms | 750 ms | 50 req/s | 0 |
| Durable mapped order commands | 100 | 8 | 1,000 ms | 2,000 ms | 10 req/s | 0 |
| Exact idempotent command replays | 100 | 8 | 500 ms | 1,000 ms | 20 req/s | 0 |

The command phase retains integration payloads, resolves versioned owner/item
mappings, creates fulfillment orders, records idempotency results, and produces
audit/outbox effects. The replay phase sends the identical commands concurrently
and requires byte-identical original responses.

Run the complete isolated acceptance test after building the release web artifact:

```bash
scripts/build-web.sh
scripts/test-load-envelope.sh
```

The script creates a uniquely named temporary database, starts the release server
on loopback, seeds the stated volume, executes the envelope, checks exported
metrics, and removes the temporary database. Override individual budgets through
the `LOAD_*` environment variables defined in
`apps/server/examples/load_envelope.rs` when validating a different documented
facility profile.

This is the initial supported envelope, not a fleet-scale claim. Before increasing
customer limits, add the relevant scanner-command mix, order-line distribution,
journal-entry rate, outbox destination latency, burst size, and soak duration, then
record the measured host class and revise these budgets.
