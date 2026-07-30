# Wareboxes

Wareboxes is a warehouse management system prototype.

## Workspace

- `apps/server`: Axum HTTP API and Leptos SSR host backed by PostgreSQL and SQLx
- `apps/web-ops`: Leptos SSR operations web application
- `crates/core`: shared models, DTOs, and errors
- `crates/barcodes`: barcode encoders
- `migrations/postgres`: PostgreSQL migrations
- `apps/rf-android`: native Android warehouse execution client
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

Or directly:

```bash
TEST_DATABASE_URL=postgres://wareboxes_admin:wareboxes_admin@127.0.0.1:5433/wareboxes \
  cargo test --workspace -- --test-threads=1
```

## Local Data

If migrations were changed during development, reset the local database with:

```bash
scripts/reset-db.sh
```

## Deployment

The deployment workflow verifies `deploy/runtime-version` before it builds or
activates a release. Run the current `deploy/provision.sh` on a host whenever that
version changes; normal application releases intentionally cannot rewrite root-owned
service, database, or secret configuration.

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

## Web Release

The release build produces the SSR server and its hydration assets together:

```bash
scripts/build-web.sh
```

The server binary is written to `target/release/wareboxes-server` and the browser
assets to `target/site`.
