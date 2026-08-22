# Wareboxes

Wareboxes is a warehouse management system prototype.

The durable workspace and runtime boundaries are documented in
[`docs/architecture.md`](docs/architecture.md). Product delivery gates are tracked
in [`ROADMAP.md`](ROADMAP.md).

## Workspace

- `apps/server`: API and SSR process composition root
- `apps/worker`: background delivery process composition root
- `apps/edge-agent`: durable local bridge for automation and facility devices
- `apps/web-ops`: Leptos SSR operations web application
- `apps/rf-android`: native Android warehouse execution client
- `crates/api`: Axum routes, authentication, and Leptos SSR integration
- `crates/application`: transport-independent workflow contracts and orchestration
- `crates/persistence-postgres`: PostgreSQL connection, migrations, and tenant context
- `crates/worker`: persistence-independent background worker engine
- `crates/core`: shared models, DTOs, and errors
- `crates/barcodes`: barcode encoders
- `migrations/postgres`: PostgreSQL migrations
- `scripts`: local development and test helpers

## Requirements

- Rust stable
- Docker with Docker Compose
- `cargo-leptos` and the `wasm32-unknown-unknown` Rust target
- PostgreSQL is provided by `docker-compose.yml` for local development

## Development

Install the web build tools once:

```bash
cargo install cargo-leptos --locked
rustup target add wasm32-unknown-unknown
```

Start the local database and run the SSR web application:

```bash
scripts/dev.sh
```

Open `http://127.0.0.1:8080`. The development server rebuilds and reloads the
Leptos frontend and serves the API from the same origin. The operations web
application targets desktop workstations; scanner execution remains in the Android
RF application.

Run the API without the web frontend:

```bash
scripts/dev.sh server
```

Run the outbox worker with the explicit development publisher:

```bash
OUTBOX_PUBLISHER=stdout cargo run -p wareboxes-worker-process --bin wareboxes-worker
```

The stdout publisher acknowledges and consumes delivered events. For HTTP delivery,
set `OUTBOX_PUBLISHER=http`, `OUTBOX_PUBLISH_URL`, and
`OUTBOX_PUBLISH_BEARER_TOKEN`, plus a 32-byte-or-longer
`OUTBOX_WEBHOOK_SIGNING_SECRET`. HTTP endpoints must use HTTPS unless
`OUTBOX_ALLOW_INSECURE_HTTP=true` is explicitly set for local development. See the
[integration delivery runbook](docs/operations/integration-delivery.md) for webhook
verification and strict SFTP configuration.

Carrier manifesting is disabled unless the worker has a deployment carrier gateway.
Set `CARRIER_GATEWAY_URL`, `CARRIER_GATEWAY_BEARER_TOKEN`, and a 32-byte-or-longer
`CARRIER_GATEWAY_SIGNING_SECRET`; keep provider credentials in the gateway rather
than Wareboxes. See the [carrier gateway runbook](docs/operations/carrier-gateway.md).

## Android RF

The Rust Android app owns scanner-driven warehouse execution. Install `cargo-apk`
and the ARM64 Rust target once, then build its APK:

```bash
cargo install cargo-apk --locked
rustup target add aarch64-linux-android
scripts/build-rf-android.sh
```

The debug APK is written below `target/debug/apk/`.

Install and launch it on a connected device:

```bash
scripts/install-rf-android.sh
```

## Edge Agent

The optional Rust edge agent supplies typed PLC, conveyor, robotics, sortation,
printer, and scale adapter boundaries with a durable local command and recovery
store. It starts devices disabled and requires explicit safety confirmation before
automation can resume. See the [edge-agent runbook](docs/operations/edge-agent.md)
for adapter and operator procedures. The executable
[automation envelope](docs/operations/automation-envelope.md) measures durable
submission, execution, exact replay, ambiguous recovery, and manual fallback with
`scripts/test-automation-envelope.sh`.

The local Postgres container uses host port `5433`.

The server uses separate database identities. `MIGRATION_DATABASE_URL` is the
schema-owner connection used during startup migrations and bootstrap;
`DATABASE_URL` is the restricted runtime connection. Both URLs must resolve to the
same PostgreSQL database. Local volumes created before this role split must be
rebuilt with `scripts/reset-db.sh`.

The web operations console uses an HTTP-only, same-site cookie with server-held
tenant context. `WEB_SESSION_ABSOLUTE_TTL_SECONDS` and
`WEB_SESSION_IDLE_TTL_SECONDS` bound session lifetime. Local HTTP development uses
`SECURE_WEB_SESSION_COOKIE=false`; HTTPS deployments must set it to `true`.

## Tests

```bash
scripts/test-postgres.sh
```

Pass Cargo test arguments through the wrapper when narrowing or serializing a run:

```bash
scripts/test-postgres.sh --locked -- --test-threads=1
```

The wrapper assigns a run identity and removes its cloned test databases on exit.

Validate crate dependency direction independently of compilation:

```bash
scripts/check-boundaries.sh
```

## Local Data

If migrations were changed during development, reset the local database with:

```bash
scripts/reset-db.sh
```

Seed a coherent operator dataset after the server has migrated the database and
created the bootstrap user:

```bash
scripts/seed-demo.sh --profile full
```

The `core` profile creates catalog, inventory, order, and legacy load volume. The
default `full` profile additionally executes real V1 commands to leave useful rows
in pick waves, packing, shipping, outbound loads, putaway, cycle counts, inventory
holds, replenishment, and the integration monitor. Both profiles are replay-safe.
The full profile also leaves draft, released, and completed value-added work and
draft, reserved, and shipped vendor returns with inventory and billing evidence; see
the [value-added work runbook](docs/operations/value-added-work.md) and
[vendor return runbook](docs/operations/vendor-returns.md).
Integration support procedures are documented in the
[order-intake runbook](docs/operations/integration-order-intake.md) and
[delivery runbook](docs/operations/integration-delivery.md).
Use `--verify-only` in CI or before a visual test to fail when a promised workspace
has no data. Counts can be adjusted without editing SQL:

```bash
scripts/seed-demo.sh --inventory-count 1000 --order-count 250 --load-count 250
```

Set `SEED_USER_EMAIL` when the target database has more than one administrator.
`DATABASE_URL` and `MIGRATION_DATABASE_URL` select a non-default database.

## Deployment

The deployment workflow verifies `deploy/runtime-version` before it builds or
activates a release. Run the current `deploy/provision.sh` on a host whenever that
version changes; normal application releases intentionally cannot rewrite root-owned
service, database, or secret configuration.

Provisioning installs the background worker service but leaves it disabled until a
publisher is configured. Set the HTTP publisher variables in
`/etc/wareboxes/wareboxes.env`, then activate it with:

```bash
sudo systemctl enable --now wareboxes-worker.service
```

Before the production-readiness gate, a host with an incompatible schema or database
role layout should be rebuilt rather than migrated for compatibility. After retaining
anything that matters, run these commands from a current repository checkout on the
host:

```bash
sudo systemctl stop wareboxes.service
sudo docker compose \
  -f /opt/wareboxes/runtime/postgres.compose.yml \
  down --volumes
sudo deploy/provision.sh '<deploy-public-key>' '<site-address>'
```

The next successful CI run can then deploy the application release. This procedure is
destructive and is not valid after the production-readiness gate.

Provisioned hosts take encrypted PostgreSQL snapshots daily and run an isolated
restore verification weekly. The provisioned local restic repository must be
replaced with an off-host backend before accepting production data. Configuration,
recovery objectives, alerts, drills, and the guarded disaster-restore command are
documented in [`docs/operations/backup-restore.md`](docs/operations/backup-restore.md).
Tenant-partitioned durable commands and their non-destructive encrypted archive are
documented in
[`docs/operations/command-archives.md`](docs/operations/command-archives.md).

The API exposes liveness at `/health/live`, database-and-schema readiness at
`/health/ready` (with `/health` retained as a readiness alias), and Prometheus text
metrics at `/metrics`. Caddy blocks the metrics endpoint from public traffic; scrape
it over the host-local server address. Production provisioning emits structured JSON
logs by default.
Baseline alert rules and response runbooks are documented in
[`docs/operations/telemetry-alerts.md`](docs/operations/telemetry-alerts.md).

Per-process concurrency, rate, login, and timeout controls are configured through
the production environment. Defaults and multi-replica boundaries are documented
in [`docs/operations/http-traffic-controls.md`](docs/operations/http-traffic-controls.md).

The repeatable single-node load gate and its latency, throughput, and error budgets
are documented in
[`docs/operations/load-envelope.md`](docs/operations/load-envelope.md). Run it with
`scripts/test-load-envelope.sh` after producing the release web build.

Regional cell registration, tenant placement and governed movement, residency,
capacity, draining, rollback, and retirement procedures are documented in the
[`data-cell operations runbook`](docs/operations/data-cells.md). Infrastructure
endpoints and secrets remain outside the application registry.

## Web Release

The release build produces the SSR server and its hydration assets together:

```bash
scripts/build-web.sh
```

The server binary is written to `target/release/wareboxes-server` and the browser
assets to `target/site`.

Build the non-root API and worker container images from those release artifacts
with `scripts/build-images.sh`. Image contracts and promotion requirements are in
[`docs/operations/container-images.md`](docs/operations/container-images.md).

## License

Wareboxes is licensed under the [MIT License](LICENSE).
