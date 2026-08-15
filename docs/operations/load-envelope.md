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
| Scanner-originated inventory moves | 100 | 16 | 750 ms | 1,500 ms | 15 req/s | 0 |
| Durable mapped order commands | 100 | 8 | 1,000 ms | 2,000 ms | 10 req/s | 0 |
| Exact idempotent command replays | 100 | 8 | 500 ms | 1,000 ms | 20 req/s | 0 |
| Sustained scoped reads | 15 seconds | 8 | 500 ms | 1,000 ms | 25 req/s | 0 |
| Seeded outbox backlog | At least 1,000 | 64 in flight | — | — | Complete within 120 seconds | 0 dead letters or duplicate deliveries |
| Signed load-event delivery | At least 200 | 64 in flight | 5,000 ms | — | Complete within 120 seconds | 0 dead letters or duplicate deliveries |

The command phase retains integration payloads, resolves versioned owner/item
mappings, creates fulfillment orders, records idempotency results, and produces
audit/outbox effects. The replay phase sends the identical commands concurrently
and requires byte-identical original responses.

The scanner phase issues independent, scope-bound inventory moves against seeded
available positions. Every move writes the immutable journal and balance projection
atomically, emits its outbox event, and is then replayed with the exact body-level
idempotency identity. A 15-second read soak follows the mutation phases.

During the complete run, the production outbox worker first drains the seeded burst,
then continuously publishes load-generated events to a loopback HTTP receiver. The
receiver verifies the bearer identity, HMAC signature, event identity, event type,
tenant envelope, and exact payload before adding 25 ms of destination latency. The
gate requires no dead letters, no duplicate receiver effects, complete drain of both
phases within 120 seconds, at least one published event per scanner/order command,
and a 5-second p95 commit-to-publish latency for events created during the measured
run. Draining the seeded backlog before latency measurement prevents fixture creation
time from distorting command budgets while retaining an explicit recovery-burst gate.
Two-second settling intervals between mutation and replay groups prevent one named
phase from inheriting queued effects from the preceding phase; the worker remains
active, and each mutation phase is still measured while its own events publish.

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
customer limits, expand the relevant scanner-command mix, order-line distribution,
journal-entry rate, destination latency, burst size, and soak duration; record the
measured host class and revise these budgets together.
