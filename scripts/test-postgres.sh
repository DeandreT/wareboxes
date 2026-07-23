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
echo "running: cargo test --workspace $*"
cargo test --workspace "$@"
