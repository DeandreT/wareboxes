#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v docker >/dev/null 2>&1; then
  echo "docker not found. Install Docker before resetting the dev database." >&2
  exit 127
fi

if ! docker info >/dev/null 2>&1; then
  echo "docker is not available to this user. Start Docker and make sure your user can access /var/run/docker.sock." >&2
  echo "On Linux: sudo usermod -aG docker \"$USER\", then log out and back in." >&2
  exit 1
fi

echo "Resetting local Postgres data volume..."
docker compose down -v
docker compose up -d postgres

echo "waiting for postgres..."
for _ in $(seq 1 60); do
  if docker compose exec -T postgres pg_isready -U wareboxes_admin -d wareboxes >/dev/null 2>&1; then
    if ! role_flags="$(
      docker compose exec -T postgres \
        psql -U wareboxes_admin -d wareboxes -Atc \
        "SELECT rolcanlogin, rolsuper, rolinherit, rolcreaterole, rolcreatedb, rolreplication, rolbypassrls FROM pg_roles WHERE rolname = 'wareboxes_app';" \
        2>/dev/null
    )"; then
      echo "postgres initialized without the wareboxes_admin role" >&2
      exit 1
    fi
    if [ "$role_flags" = "t|f|f|f|f|f|f" ]; then
      echo "postgres ready with separate admin and runtime roles"
      exit 0
    fi
    echo "postgres initialized without the restricted wareboxes_app role" >&2
    exit 1
  fi
  sleep 1
done

echo "postgres did not come up" >&2
exit 1
