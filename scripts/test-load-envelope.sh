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
if [ ! -x target/release/wareboxes-worker ]; then
  echo "target/release/wareboxes-worker is missing; run scripts/build-web.sh first" >&2
  exit 1
fi

cargo build --locked --release -p wareboxes-server --example load_envelope

load_port="${WAREBOXES_LOAD_TEST_PORT:-18084}"
if [[ ! "$load_port" =~ ^[0-9]+$ ]] || [ "$load_port" -lt 1024 ] || [ "$load_port" -gt 65535 ]; then
  echo "WAREBOXES_LOAD_TEST_PORT must be between 1024 and 65535" >&2
  exit 2
fi
webhook_port="${WAREBOXES_LOAD_WEBHOOK_PORT:-$((load_port + 1))}"
if [[ ! "$webhook_port" =~ ^[0-9]+$ ]] \
  || [ "$webhook_port" -lt 1024 ] || [ "$webhook_port" -gt 65535 ] \
  || [ "$webhook_port" -eq "$load_port" ]; then
  echo "WAREBOXES_LOAD_WEBHOOK_PORT must be a distinct port between 1024 and 65535" >&2
  exit 2
fi
scanner_requests="${LOAD_SCANNER_REQUESTS:-100}"
command_requests="${LOAD_COMMAND_REQUESTS:-100}"
if [[ ! "$scanner_requests" =~ ^[0-9]+$ ]] || [ "$scanner_requests" -lt 1 ] \
  || [[ ! "$command_requests" =~ ^[0-9]+$ ]] || [ "$command_requests" -lt 1 ]; then
  echo "LOAD_SCANNER_REQUESTS and LOAD_COMMAND_REQUESTS must be positive integers" >&2
  exit 2
fi
load_database="wareboxes_load_$$_${RANDOM}"
if [[ ! "$load_database" =~ ^wareboxes_load_[0-9]+_[0-9]+$ ]]; then
  echo "could not construct a safe load-test database name" >&2
  exit 1
fi
load_dir="$(mktemp -d)"
server_pid=
receiver_pid=
worker_pid=
postgres_container=

cleanup() {
  if [ -n "$worker_pid" ] && kill -0 "$worker_pid" >/dev/null 2>&1; then
    kill "$worker_pid" >/dev/null 2>&1 || true
    wait "$worker_pid" >/dev/null 2>&1 || true
  fi
  if [ -n "$receiver_pid" ] && kill -0 "$receiver_pid" >/dev/null 2>&1; then
    kill "$receiver_pid" >/dev/null 2>&1 || true
    wait "$receiver_pid" >/dev/null 2>&1 || true
  fi
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

MIGRATION_DATABASE_URL="$migration_url" scripts/seed-inventory.sh \
  --count 1000 --move-destinations "$scanner_requests"
MIGRATION_DATABASE_URL="$migration_url" scripts/seed-orders.sh --count 250

webhook_token="wareboxes-load-envelope-webhook"
webhook_secret="wareboxes-load-envelope-signing-secret-2026"
LOAD_WEBHOOK_PORT="$webhook_port" \
LOAD_WEBHOOK_DELAY_MILLIS="${LOAD_WEBHOOK_DELAY_MILLIS:-25}" \
LOAD_WEBHOOK_BEARER_TOKEN="$webhook_token" \
LOAD_WEBHOOK_SIGNING_SECRET="$webhook_secret" \
target/release/examples/load_envelope receiver > "$load_dir/receiver.log" 2>&1 &
receiver_pid="$!"

receiver_ready=false
for _ in $(seq 1 30); do
  if ! kill -0 "$receiver_pid" >/dev/null 2>&1; then
    echo "load webhook receiver stopped before becoming ready" >&2
    sed -n '1,240p' "$load_dir/receiver.log" >&2
    exit 1
  fi
  if curl --fail --silent --show-error \
    "http://127.0.0.1:$webhook_port/health" >/dev/null 2>&1; then
    receiver_ready=true
    break
  fi
  sleep 1
done
if [ "$receiver_ready" != true ]; then
  echo "load webhook receiver did not become ready within 30 seconds" >&2
  sed -n '1,240p' "$load_dir/receiver.log" >&2
  exit 1
fi

DATABASE_URL="$runtime_url" \
WORKER_ID="load-envelope-worker-$$" \
OUTBOX_PUBLISHER=http \
OUTBOX_PUBLISH_URL="http://127.0.0.1:$webhook_port/events" \
OUTBOX_PUBLISH_BEARER_TOKEN="$webhook_token" \
OUTBOX_WEBHOOK_SIGNING_SECRET="$webhook_secret" \
OUTBOX_ALLOW_INSECURE_HTTP=true \
OUTBOX_BATCH_SIZE="${LOAD_OUTBOX_BATCH_SIZE:-500}" \
OUTBOX_MAX_IN_FLIGHT="${LOAD_OUTBOX_MAX_IN_FLIGHT:-64}" \
OUTBOX_POLL_INTERVAL_SECONDS=1 \
LOG_FORMAT=json \
target/release/wareboxes-worker > "$load_dir/worker.log" 2>&1 &
worker_pid="$!"

drain_seconds="${LOAD_OUTBOX_DRAIN_SECONDS:-120}"
if [[ ! "$drain_seconds" =~ ^[0-9]+$ ]] || [ "$drain_seconds" -lt 1 ]; then
  echo "LOAD_OUTBOX_DRAIN_SECONDS must be a positive integer" >&2
  exit 2
fi
wait_for_outbox_drain() {
  local label="$1"
  local drained=false
  local pending
  for _ in $(seq 1 "$drain_seconds"); do
    if ! kill -0 "$worker_pid" >/dev/null 2>&1; then
      echo "load outbox worker stopped while draining $label" >&2
      sed -n '1,240p' "$load_dir/worker.log" >&2
      exit 1
    fi
    pending="$(psql "$migration_url" -Atqc \
      "SELECT COUNT(*) FROM outbox_events WHERE published_at IS NULL AND dead_lettered_at IS NULL")"
    if [ "$pending" = 0 ]; then
      drained=true
      break
    fi
    sleep 1
  done
  if [ "$drained" != true ]; then
    echo "$label did not drain within ${drain_seconds}s" >&2
    sed -n '1,240p' "$load_dir/worker.log" >&2
    exit 1
  fi
}

wait_for_outbox_drain "the seeded outbox burst"
load_started_at="$(psql "$migration_url" -Atqc 'SELECT clock_timestamp()')"

LOAD_BASE_URL="http://127.0.0.1:$load_port" \
LOAD_USER_EMAIL="$load_email" \
LOAD_USER_PASSWORD="$load_password" \
target/release/examples/load_envelope

wait_for_outbox_drain "the load-generated outbox burst"

dead_letters="$(psql "$migration_url" -Atqc \
  "SELECT COUNT(*) FROM outbox_events WHERE dead_lettered_at IS NOT NULL")"
if [ "$dead_letters" != 0 ]; then
  echo "load envelope produced $dead_letters dead-lettered outbox events" >&2
  exit 1
fi
published_since="$(
  psql "$migration_url" -v marker="$load_started_at" -Atq <<'SQL'
SELECT COUNT(*)
FROM outbox_events
WHERE created >= :'marker'::timestamptz
  AND published_at IS NOT NULL;
SQL
)"
minimum_published=$((scanner_requests + command_requests))
if [ "$published_since" -lt "$minimum_published" ]; then
  echo "expected at least $minimum_published load-generated outbox events, found $published_since" >&2
  exit 1
fi
outbox_p95_millis="$(
  psql "$migration_url" -v marker="$load_started_at" -Atq <<'SQL'
SELECT round((percentile_cont(0.95) WITHIN GROUP (
  ORDER BY EXTRACT(EPOCH FROM (published_at - created)) * 1000
))::numeric, 1)
FROM outbox_events
WHERE created >= :'marker'::timestamptz
  AND published_at IS NOT NULL;
SQL
)"
outbox_p95_budget="${LOAD_OUTBOX_P95_MILLIS:-5000}"
if ! [[ "$outbox_p95_budget" =~ ^[0-9]+$ ]] \
  || ! awk -v observed="$outbox_p95_millis" -v budget="$outbox_p95_budget" \
    'BEGIN { exit !(observed <= budget) }'; then
  echo "outbox p95 ${outbox_p95_millis}ms exceeded ${outbox_p95_budget}ms" >&2
  exit 1
fi
receiver_stats="$(curl --fail --silent --show-error \
  "http://127.0.0.1:$webhook_port/stats")"
receiver_duplicates="$(sed -nE 's/.*"duplicates":([0-9]+).*/\1/p' <<< "$receiver_stats")"
if [ -z "$receiver_duplicates" ] || [ "$receiver_duplicates" != 0 ]; then
  echo "load receiver reported invalid or duplicate delivery evidence: $receiver_stats" >&2
  exit 1
fi
echo "event=load_outbox_completed published=$published_since p95_millis=$outbox_p95_millis receiver=$receiver_stats"

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
