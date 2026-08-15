#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required for the backup/restore acceptance test" >&2
  exit 127
fi

test_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$test_dir"
}
trap cleanup EXIT

printf '%s\n' "wareboxes-backup-test-$RANDOM-$RANDOM" > "$test_dir/restic-password"
chmod 0600 "$test_dir/restic-password"
export RESTIC_REPOSITORY="$test_dir/repository"
export RESTIC_PASSWORD_FILE="$test_dir/restic-password"
export RESTIC_CACHE_DIR="$test_dir/cache"
export WAREBOXES_POSTGRES_COMPOSE_FILE="$PWD/docker-compose.yml"
export WAREBOXES_BACKUP_HOST=wareboxes-acceptance-test
export WAREBOXES_BACKUP_LOCK_FILE="$test_dir/backup.lock"
export WAREBOXES_RESTORE_MAX_SECONDS=300
export WAREBOXES_POSTGRES_INIT_SCRIPT="$PWD/deploy/postgres-init/001-create-app-role.sh"
if ! command -v restic >/dev/null 2>&1; then
  export RESTIC_DOCKER_MOUNT_ROOT="$test_dir"
  export PATH="$PWD/scripts/test-support:$PATH"
fi

restic init
docker compose up -d postgres

ready=false
for _ in $(seq 1 60); do
  if docker compose exec -T postgres \
    pg_isready --username wareboxes_admin --dbname wareboxes >/dev/null 2>&1; then
    ready=true
    break
  fi
  sleep 1
done
if [ "$ready" != true ]; then
  echo "PostgreSQL did not become ready for the backup/restore acceptance test" >&2
  exit 1
fi

schema_present="$(
  docker compose exec -T postgres \
    psql \
      --username wareboxes_admin \
      --dbname wareboxes \
      --tuples-only \
      --no-align \
      --command "SELECT to_regclass('public.inventory_transactions') IS NOT NULL" \
    | tr -d '[:space:]'
)"
if [ "$schema_present" != t ]; then
  docker compose exec -T postgres \
    psql \
      --username wareboxes_admin \
      --dbname wareboxes \
      --set ON_ERROR_STOP=1 \
      --command "
        CREATE TABLE IF NOT EXISTS public._sqlx_migrations (
            version BIGINT PRIMARY KEY,
            description TEXT NOT NULL,
            installed_on TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
            success BOOLEAN NOT NULL,
            checksum BYTEA NOT NULL,
            execution_time BIGINT NOT NULL
        )
      " >/dev/null
  docker compose exec -T postgres \
    psql \
      --username wareboxes_admin \
      --dbname wareboxes \
      --set ON_ERROR_STOP=1 \
    < migrations/postgres/0001_baseline.sql >/dev/null
fi

deploy/wareboxes-backup --check
deploy/wareboxes-backup
deploy/wareboxes-restore-postgres --check
deploy/wareboxes-restore-drill
restic snapshots \
  --host "$WAREBOXES_BACKUP_HOST" \
  --tag wareboxes-postgres \
  --latest 1 >/dev/null
echo "backup/restore acceptance test passed"
