#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

for command_name in cargo curl docker psql; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "$command_name is required for the load-envelope acceptance test" >&2
    exit 127
  fi
done
if [ ! -x target/release/wareboxes-server ]; then
  echo "target/release/wareboxes-server is missing; run scripts/build-web.sh first" >&2
  exit 1
fi

load_port="${WAREBOXES_LOAD_TEST_PORT:-18084}"
if [[ ! "$load_port" =~ ^[0-9]+$ ]] || [ "$load_port" -lt 1024 ] || [ "$load_port" -gt 65535 ]; then
  echo "WAREBOXES_LOAD_TEST_PORT must be between 1024 and 65535" >&2
  exit 2
fi
load_database="wareboxes_load_$$_${RANDOM}"
if [[ ! "$load_database" =~ ^wareboxes_load_[0-9]+_[0-9]+$ ]]; then
  echo "could not construct a safe load-test database name" >&2
  exit 1
fi
load_dir="$(mktemp -d)"
server_pid=
postgres_container=

cleanup() {
  if [ -n "$server_pid" ] && kill -0 "$server_pid" >/dev/null 2>&1; then
    kill "$server_pid" >/dev/null 2>&1 || true
    wait "$server_pid" >/dev/null 2>&1 || true
  fi
  if [ -n "$postgres_container" ] \
    && [[ "$load_database" =~ ^wareboxes_load_[0-9]+_[0-9]+$ ]]; then
    docker exec "$postgres_container" \
      dropdb --username wareboxes_admin --force --if-exists "$load_database" \
      >/dev/null 2>&1 || true
  fi
  rm -r "$load_dir"
}
trap cleanup EXIT

docker compose up -d postgres
postgres_container="$(docker compose ps -q postgres)"
if [ -z "$postgres_container" ]; then
  echo "could not resolve the load-test PostgreSQL container" >&2
  exit 1
fi
ready=false
for _ in $(seq 1 60); do
  if docker exec "$postgres_container" \
    pg_isready --username wareboxes_admin --dbname wareboxes >/dev/null 2>&1; then
    ready=true
    break
  fi
  sleep 1
done
if [ "$ready" != true ]; then
  echo "PostgreSQL did not become ready for the load-envelope test" >&2
  exit 1
fi
docker exec "$postgres_container" \
  createdb --username wareboxes_admin --owner wareboxes_admin "$load_database"

runtime_url="postgres://wareboxes_app:wareboxes_app@127.0.0.1:5433/$load_database"
migration_url="postgres://wareboxes_admin:wareboxes_admin@127.0.0.1:5433/$load_database"
load_email="load-envelope@example.test"
load_password="Wareboxes-load-envelope-2026!"

DATABASE_URL="$runtime_url" \
MIGRATION_DATABASE_URL="$migration_url" \
BIND_ADDR="127.0.0.1:$load_port" \
BOOTSTRAP_ADMIN_EMAIL="$load_email" \
BOOTSTRAP_ADMIN_PASSWORD="$load_password" \
ALLOW_PUBLIC_REGISTRATION=false \
SECURE_WEB_SESSION_COOKIE=false \
LOG_FORMAT=json \
LEPTOS_SITE_ROOT="$PWD/target/site" \
target/release/wareboxes-server > "$load_dir/server.log" 2>&1 &
server_pid="$!"

server_ready=false
for _ in $(seq 1 90); do
  if ! kill -0 "$server_pid" >/dev/null 2>&1; then
    echo "load-test server stopped before becoming ready" >&2
    sed -n '1,240p' "$load_dir/server.log" >&2
    exit 1
  fi
  if curl --fail --silent --show-error \
    "http://127.0.0.1:$load_port/health/ready" >/dev/null 2>&1; then
    server_ready=true
    break
  fi
  sleep 1
done
if [ "$server_ready" != true ]; then
  echo "load-test server did not become ready within 90 seconds" >&2
  sed -n '1,240p' "$load_dir/server.log" >&2
  exit 1
fi

MIGRATION_DATABASE_URL="$migration_url" scripts/seed-inventory.sh --count 1000
MIGRATION_DATABASE_URL="$migration_url" scripts/seed-orders.sh --count 250

LOAD_BASE_URL="http://127.0.0.1:$load_port" \
LOAD_USER_EMAIL="$load_email" \
LOAD_USER_PASSWORD="$load_password" \
cargo run --locked --release -p wareboxes-server --example load_envelope

metrics="$(curl --fail --silent --show-error "http://127.0.0.1:$load_port/metrics")"
for metric in \
  wareboxes_http_requests_total \
  wareboxes_http_request_duration_seconds_count \
  wareboxes_database_pool_connections; do
  if ! grep -q "^$metric" <<< "$metrics"; then
    echo "load-test metrics are missing $metric" >&2
    exit 1
  fi
done
echo "load-envelope acceptance test passed"
