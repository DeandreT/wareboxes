# Wareboxes

Wareboxes is a warehouse management system prototype.

## Workspace

- `crates/server`: Axum HTTP API backed by PostgreSQL and SQLx
- `crates/client`: egui/eframe desktop client
- `crates/core`: shared models, DTOs, and errors
- `crates/barcodes`: barcode encoders
- `migrations/postgres`: PostgreSQL migrations
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

The local Postgres container uses host port `5433`.

## Tests

```bash
scripts/test-postgres.sh
```

Or directly:

```bash
TEST_DATABASE_URL=postgres://wareboxes:wareboxes@127.0.0.1:5433/wareboxes \
  cargo test --workspace -- --test-threads=1
```

## Local Data

If migrations were changed during development, reset the local database with:

```bash
scripts/reset-db.sh
```
