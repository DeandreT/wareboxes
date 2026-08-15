#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required for the durable-command archive acceptance test" >&2
  exit 127
fi

test_dir="$(mktemp -d)"
archive_database="wareboxes_archive_test_${RANDOM}_${RANDOM}"
database_created=false
cleanup() {
  if [ "$database_created" = true ]; then
    docker compose exec -T postgres \
      dropdb --force --if-exists --username wareboxes_admin "$archive_database" \
      >/dev/null 2>&1 || true
  fi
  rm -r "$test_dir"
}
trap cleanup EXIT

printf '%s\n' "wareboxes-command-archive-test-$RANDOM-$RANDOM" > "$test_dir/restic-password"
chmod 0600 "$test_dir/restic-password"
export RESTIC_REPOSITORY="$test_dir/repository"
export RESTIC_PASSWORD_FILE="$test_dir/restic-password"
export RESTIC_CACHE_DIR="$test_dir/cache"
export WAREBOXES_POSTGRES_COMPOSE_FILE="$PWD/docker-compose.yml"
export WAREBOXES_POSTGRES_DATABASE="$archive_database"
export WAREBOXES_BACKUP_HOST=wareboxes-command-archive-acceptance
export WAREBOXES_BACKUP_LOCK_FILE="$test_dir/archive.lock"
export WAREBOXES_COMMAND_ARCHIVE_AFTER_DAYS=1
export WAREBOXES_COMMAND_ARCHIVE_KEEP_MONTHLY=2
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
  echo "PostgreSQL did not become ready for the durable-command archive test" >&2
  exit 1
fi

docker compose exec -T postgres \
  createdb --username wareboxes_admin "$archive_database"
database_created=true
docker compose exec -T postgres \
  psql \
    --username wareboxes_admin \
    --dbname "$archive_database" \
    --set ON_ERROR_STOP=1 \
    --command "
      CREATE TABLE public._sqlx_migrations (
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
    --dbname "$archive_database" \
    --set ON_ERROR_STOP=1 \
  < migrations/postgres/0001_baseline.sql >/dev/null

docker compose exec -T postgres \
  psql \
    --username wareboxes_admin \
    --dbname "$archive_database" \
    --set ON_ERROR_STOP=1 \
    --command "
      WITH new_tenant AS (
          INSERT INTO tenants (slug, name)
          VALUES ('archive-acceptance', 'Archive Acceptance')
          RETURNING id
      ), new_user AS (
          INSERT INTO users (created, email)
          VALUES (CURRENT_TIMESTAMP, 'archive-acceptance@example.invalid')
          RETURNING id
      ), new_membership AS (
          INSERT INTO tenant_memberships (tenant_id, user_id, is_default)
          SELECT new_tenant.id, new_user.id, true
          FROM new_tenant CROSS JOIN new_user
          RETURNING tenant_id, user_id
      )
      INSERT INTO command_idempotency_records
          (tenant_id, created, operation, idempotency_key, request_hash,
           result_json, actor_user_id, request_id, result_schema_version)
      SELECT tenant_id, TIMESTAMPTZ '2000-01-01T00:00:00Z',
             'archive.acceptance.v1', 'archive-key', 'request-hash',
             '{\"accepted\":true}'::JSONB, user_id, 'archive-request', 1
      FROM new_membership
    " >/dev/null

deploy/wareboxes-archive-commands --check
deploy/wareboxes-archive-commands

canonical_record="$({
  docker compose exec -T postgres \
    psql \
      --username wareboxes_admin \
      --dbname "$archive_database" \
      --tuples-only \
      --no-align \
      --set ON_ERROR_STOP=1 \
      --command "
        SELECT COUNT(*) || ':' || BOOL_AND(actor_user_id IS NOT NULL) || ':' ||
               BOOL_AND(result_json = '{\"accepted\":true}'::JSONB)
        FROM command_idempotency_records
        WHERE operation = 'archive.acceptance.v1'
          AND idempotency_key = 'archive-key'
      "
} | tr -d '[:space:]')"
if [ "$canonical_record" != "1:true:true" ]; then
  echo "command archival changed or removed the canonical replay record" >&2
  exit 1
fi

if docker compose exec -T postgres \
  psql \
    --username wareboxes_admin \
    --dbname "$archive_database" \
    --set ON_ERROR_STOP=1 \
    --command "
      INSERT INTO command_idempotency_records
          (tenant_id, created, operation, idempotency_key, request_hash,
           result_json, actor_user_id)
      SELECT tenant_id, CURRENT_TIMESTAMP, operation, idempotency_key,
             'different-hash', '{\"accepted\":false}'::JSONB, actor_user_id
      FROM command_idempotency_records
      WHERE operation = 'archive.acceptance.v1'
    " >/dev/null 2>&1; then
  echo "partitioning admitted a duplicate durable idempotency identity" >&2
  exit 1
fi

if docker compose exec -T postgres \
  psql \
    --username wareboxes_admin \
    --dbname "$archive_database" \
    --set ON_ERROR_STOP=1 \
    --command "
      INSERT INTO command_idempotency_records
          (tenant_id, created, operation, idempotency_key, request_hash,
           result_json, actor_user_id)
      SELECT tenant_id, CURRENT_TIMESTAMP, 'archive.missing_actor.v1',
             'missing-actor', 'request-hash', '{}'::JSONB, NULL
      FROM command_idempotency_records
      WHERE operation = 'archive.acceptance.v1'
    " >/dev/null 2>&1; then
  echo "durable commands admitted a record without actor attribution" >&2
  exit 1
fi

restic snapshots \
  --host "$WAREBOXES_BACKUP_HOST" \
  --tag wareboxes-command-archive \
  --latest 1 >/dev/null
echo "durable-command archive acceptance test passed"
