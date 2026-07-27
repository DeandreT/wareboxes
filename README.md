# Wareboxes

Wareboxes is a warehouse management system prototype.

## Workspace

- `crates/server`: Axum HTTP API backed by PostgreSQL and SQLx
- `crates/client`: egui/eframe operations and administration client
- `crates/core`: shared models, DTOs, and errors
- `crates/barcodes`: barcode encoders
- `migrations/postgres`: PostgreSQL migrations
- `apps/rf-android`: native Android warehouse execution client
- `scripts`: local development and test helpers

## Requirements

- Rust stable
- Docker with Docker Compose
- PostgreSQL is provided by `docker-compose.yml` for local development

## Development

Start the local database and run the server/client:

```bash
scripts/dev.sh
```

Or run pieces manually:

```bash
docker compose up -d postgres
cargo run -p wareboxes-server
cargo run -p wareboxes-client
```

To run the operations client against the hosted demo with its credentials prefilled:

```bash
scripts/run-client-demo.sh
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

## Website

The deployable website combines the static pages in `site/` with the real eframe
client compiled to WebAssembly. Install [Trunk](https://trunkrs.dev/) and the
`wasm32-unknown-unknown` Rust target, then build and preview the assembled site
from the repository root:

```bash
rustup target add wasm32-unknown-unknown
scripts/build-site.sh
python3 -m http.server 4173 --directory _site
```
