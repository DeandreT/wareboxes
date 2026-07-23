#!/usr/bin/env bash
# Local dev runner.
#   scripts/dev.sh          start the API, wait for it, then launch the client
#   scripts/dev.sh server   start only the API (foreground)
#   scripts/dev.sh client   start only the client (assumes API already running)
#
# Config comes from .env (DATABASE_URL, MIGRATION_DATABASE_URL, BIND_ADDR,
# BOOTSTRAP_ADMIN_*).
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1090
  . "$HOME/.cargo/env"
fi

# shellcheck disable=SC1091
[ -f .env ] && set -a && . ./.env && set +a
DATABASE_URL="${DATABASE_URL:-postgres://wareboxes_app:wareboxes_app@127.0.0.1:5433/wareboxes}"
MIGRATION_DATABASE_URL="${MIGRATION_DATABASE_URL:-postgres://wareboxes_admin:wareboxes_admin@127.0.0.1:5433/wareboxes}"
BIND_ADDR="${BIND_ADDR:-127.0.0.1:8080}"
export DATABASE_URL MIGRATION_DATABASE_URL
HEALTH="http://${BIND_ADDR}/health"

ensure_cargo() {
  if ! command -v cargo >/dev/null 2>&1; then
    echo "cargo not found. Install Rust with rustup or add ~/.cargo/bin to PATH." >&2
    exit 127
  fi
}

ensure_docker() {
  if ! command -v docker >/dev/null 2>&1; then
    echo "docker not found. Install Docker before running the dev environment." >&2
    exit 127
  fi
  if ! docker info >/dev/null 2>&1; then
    echo "docker is not available to this user. Start Docker and make sure your user can access /var/run/docker.sock." >&2
    echo "On Linux: sudo usermod -aG docker \"$USER\", then log out and back in." >&2
    exit 1
  fi
}

ensure_postgres() {
  ensure_docker
  docker compose up -d postgres
  for _ in $(seq 1 60); do
    if docker compose exec -T postgres pg_isready -U wareboxes_admin -d wareboxes >/dev/null 2>&1; then
      if ! role_flags="$(
        docker compose exec -T postgres \
          psql -U wareboxes_admin -d wareboxes -Atc \
          "SELECT rolcanlogin, rolsuper, rolinherit, rolcreaterole, rolcreatedb, rolreplication, rolbypassrls FROM pg_roles WHERE rolname = 'wareboxes_app';" \
          2>/dev/null
      )"; then
        echo "postgres is using an incompatible local data volume." >&2
        echo "Reset it with: scripts/reset-db.sh" >&2
        exit 1
      fi
      if [ "$role_flags" = "t|f|f|f|f|f|f" ]; then
        return 0
      fi
      echo "postgres is using an incompatible local data volume." >&2
      echo "Reset it with: scripts/reset-db.sh" >&2
      exit 1
    fi
    sleep 0.5
  done
  echo "postgres did not come up" >&2
  exit 1
}

run_server() {
  set +e
  output="$(cargo run -p wareboxes-server 2>&1)"
  status=$?
  set -e
  printf '%s\n' "$output"
  if [ "$status" -ne 0 ] && printf '%s\n' "$output" | grep -q "previously applied but has been modified"; then
    echo "" >&2
    echo "Database migrations changed after they were applied." >&2
    echo "Reset the local dev database with: scripts/reset-db.sh" >&2
  fi
  exit "$status"
}
run_client() { exec cargo run -p wareboxes-client; }

case "${1:-all}" in
  server) ensure_cargo; ensure_postgres; run_server ;;
  client) ensure_cargo; run_client ;;
  all)
    ensure_cargo
    ensure_postgres
    cargo build -p wareboxes-server -p wareboxes-client
    cargo run -p wareboxes-server &
    SERVER_PID=$!
    trap 'kill "$SERVER_PID" 2>/dev/null || true' EXIT
    echo "waiting for API at ${HEALTH} ..."
    for _ in $(seq 1 60); do
      if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        wait "$SERVER_PID" || true
        echo "server exited before becoming healthy" >&2
        echo "If migrations were modified locally, run: scripts/reset-db.sh" >&2
        exit 1
      fi
      if curl -fs "$HEALTH" >/dev/null 2>&1; then break; fi
      sleep 0.5
    done
    curl -fs "$HEALTH" >/dev/null 2>&1 || { echo "server did not come up" >&2; echo "If migrations were modified locally, run: scripts/reset-db.sh" >&2; exit 1; }
    echo "API up. Launching client (log in with ${BOOTSTRAP_ADMIN_EMAIL:-admin@example.com})."
    cargo run -p wareboxes-client
    ;;
  *) echo "usage: scripts/dev.sh [server|client|all]" >&2; exit 2 ;;
esac
