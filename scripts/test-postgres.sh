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

export TEST_DATABASE_URL="${TEST_DATABASE_URL:-postgres://wareboxes:wareboxes@127.0.0.1:5433/wareboxes}"

echo "starting postgres test container..."
docker compose up -d postgres

echo "waiting for postgres at ${TEST_DATABASE_URL}..."
ready=false
for _ in $(seq 1 60); do
  if docker compose exec -T postgres pg_isready -U wareboxes -d wareboxes >/dev/null 2>&1; then
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

if ! docker compose exec -T postgres psql -U wareboxes -d postgres -c "SELECT 1" >/dev/null 2>&1; then
  echo "postgres is accepting health checks but test credentials cannot connect to the admin database." >&2
  echo "TEST_DATABASE_URL=${TEST_DATABASE_URL}" >&2
  exit 1
fi

echo "TEST_DATABASE_URL=${TEST_DATABASE_URL}"
echo "running: cargo test --workspace $*"
cargo test --workspace "$@"
