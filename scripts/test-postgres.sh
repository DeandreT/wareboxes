#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

if [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1090
  . "$HOME/.cargo/env"
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found. Install Rust with rustup or add ~/.cargo/bin to PATH." >&2
  exit 127
fi

if ! command -v docker >/dev/null 2>&1; then
  echo "docker not found. Install Docker before running the test environment." >&2
  exit 127
fi
if ! docker info >/dev/null 2>&1; then
  echo "docker is not available to this user. Start Docker and make sure your user can access /var/run/docker.sock." >&2
  echo "On Linux: sudo usermod -aG docker \"$USER\", then log out and back in." >&2
  exit 1
fi

export TEST_DATABASE_URL="${TEST_DATABASE_URL:-postgres://wareboxes_admin:wareboxes_admin@127.0.0.1:5433/wareboxes}"

echo "starting postgres test container..."
docker compose up -d postgres

echo "waiting for postgres at ${TEST_DATABASE_URL}..."
ready=false
for _ in $(seq 1 60); do
  if docker compose exec -T postgres pg_isready -U wareboxes_admin -d wareboxes >/dev/null 2>&1; then
    ready=true
    break
  fi
  sleep 1
done

if [ "$ready" != true ]; then
  echo "postgres did not become ready within 60 seconds." >&2
  echo "Check container logs with: docker compose logs postgres" >&2
  exit 1
fi

if ! docker compose exec -T postgres psql -U wareboxes_admin -d postgres -c "SELECT 1" >/dev/null 2>&1; then
  echo "postgres is accepting health checks but test credentials cannot connect to the admin database." >&2
  echo "TEST_DATABASE_URL=${TEST_DATABASE_URL}" >&2
  exit 1
fi

role_flags="$(
  docker compose exec -T postgres \
    psql -U wareboxes_admin -d wareboxes -Atc \
    "SELECT rolcanlogin, rolsuper, rolinherit, rolcreaterole, rolcreatedb, rolreplication, rolbypassrls FROM pg_roles WHERE rolname = 'wareboxes_app';"
)"
if [ "$role_flags" != "t|f|f|f|f|f|f" ]; then
  echo "postgres does not have the expected restricted wareboxes_app role." >&2
  echo "Reset the local database with: scripts/reset-db.sh" >&2
  exit 1
fi

echo "TEST_DATABASE_URL=${TEST_DATABASE_URL}"
export WAREBOXES_TEST_RUN_ID="${WAREBOXES_TEST_RUN_ID:-$$}"
if [[ ! "$WAREBOXES_TEST_RUN_ID" =~ ^[0-9]{1,16}$ ]]; then
  echo "WAREBOXES_TEST_RUN_ID must contain at most 16 ASCII digits." >&2
  exit 1
fi

cleanup_stale_test_databases() {
  local cleanup_jobs="${WAREBOXES_TEST_CLEANUP_JOBS:-4}"
  local container_id
  local current_prefix="wareboxes_test_${WAREBOXES_TEST_RUN_ID}_"
  if [[ ! "$cleanup_jobs" =~ ^[0-9]+$ ]] || ((cleanup_jobs < 1 || cleanup_jobs > 8)); then
    echo "WAREBOXES_TEST_CLEANUP_JOBS must be between 1 and 8." >&2
    return 1
  fi
  container_id="$(docker compose ps -q postgres)"
  if [ -z "$container_id" ]; then
    return
  fi

  docker exec -i "$container_id" bash -s -- "$current_prefix" "$cleanup_jobs" <<'EOF'
set -euo pipefail
current_prefix="$1"
cleanup_jobs="$2"
databases_to_remove=()
mapfile -t databases < <(
  PGOPTIONS='-c max_parallel_workers_per_gather=0' \
    psql -U wareboxes_admin -d postgres -Atc \
    "SELECT database.datname
     FROM pg_database database
     WHERE database.datname LIKE 'wareboxes_test_%'
       AND NOT EXISTS (
         SELECT 1 FROM pg_stat_activity activity
         WHERE activity.datname=database.datname)
     ORDER BY database.datname"
)
for database in "${databases[@]}"; do
  if [[ "$database" == "$current_prefix"* ]]; then
    continue
  fi
  if [[ ! "$database" =~ ^wareboxes_test_[0-9]+_[0-9]+(_[0-9]+)?$ ]]; then
    echo "refusing to remove unexpected stale test database name: $database" >&2
    exit 1
  fi
  databases_to_remove+=("$database")
done
if ((${#databases_to_remove[@]} > 0)); then
  printf '%s\0' "${databases_to_remove[@]}" | xargs -0 -P "$cleanup_jobs" -n 1 \
    bash -c 'dropdb -U wareboxes_admin --if-exists "$1" || echo "warning: could not remove active test database $1" >&2' _
fi
echo "removed ${#databases_to_remove[@]} stale test database(s)"
EOF
}

cleanup_test_databases() {
  local cleanup_jobs="${WAREBOXES_TEST_CLEANUP_JOBS:-4}"
  local container_id
  local prefix="wareboxes_test_${WAREBOXES_TEST_RUN_ID}_"
  if [[ ! "$cleanup_jobs" =~ ^[0-9]+$ ]] || ((cleanup_jobs < 1 || cleanup_jobs > 8)); then
    echo "WAREBOXES_TEST_CLEANUP_JOBS must be between 1 and 8." >&2
    return 1
  fi
  container_id="$(docker compose ps -q postgres)"
  if [ -z "$container_id" ]; then
    return
  fi

  docker exec -i "$container_id" bash -s -- "$prefix" "$cleanup_jobs" <<'EOF'
set -euo pipefail
prefix="$1"
cleanup_jobs="$2"
databases_to_remove=()
mapfile -t databases < <(
  psql -U wareboxes_admin -d postgres -Atc \
    "SELECT datname FROM pg_database WHERE datname LIKE 'wareboxes_test_%' ORDER BY datname"
)
for database in "${databases[@]}"; do
  if [[ "$database" != "$prefix"* ]]; then
    continue
  fi
  if [[ ! "$database" =~ ^wareboxes_test_[0-9]+_[0-9]+_[0-9]+$ ]]; then
    echo "refusing to remove unexpected test database name: $database" >&2
    exit 1
  fi
  databases_to_remove+=("$database")
done
if ((${#databases_to_remove[@]} > 0)); then
  printf '%s\0' "${databases_to_remove[@]}" | xargs -0 -P "$cleanup_jobs" -n 1 \
    bash -c 'dropdb -U wareboxes_admin --if-exists "$1" || echo "warning: could not remove active test database $1" >&2' _
fi
echo "removed ${#databases_to_remove[@]} run-scoped test database(s)"
EOF
}

cleanup_stale_test_databases
trap cleanup_test_databases EXIT
echo "running: cargo test --workspace $*"
cargo test --workspace "$@"
