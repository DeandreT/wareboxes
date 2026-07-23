#!/usr/bin/env bash
set -euo pipefail

if [ -n "${POSTGRES_APP_PASSWORD_FILE:-}" ]; then
  app_password="$(cat "$POSTGRES_APP_PASSWORD_FILE")"
else
  app_password="${POSTGRES_APP_PASSWORD:?POSTGRES_APP_PASSWORD or POSTGRES_APP_PASSWORD_FILE is required}"
fi

psql_admin=(
  psql
  --username "$POSTGRES_USER"
  --dbname "$POSTGRES_DB"
  --set ON_ERROR_STOP=1
)

if "${psql_admin[@]}" --tuples-only --no-align --command \
  "SELECT 1 FROM pg_roles WHERE rolname = 'wareboxes_app';" | grep -qx 1; then
  "${psql_admin[@]}" --set app_password="$app_password" <<'SQL'
ALTER ROLE wareboxes_app
  WITH LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS
  PASSWORD :'app_password';
SQL
else
  "${psql_admin[@]}" --set app_password="$app_password" <<'SQL'
CREATE ROLE wareboxes_app
  WITH LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS
  PASSWORD :'app_password';
SQL
fi

"${psql_admin[@]}" --set database_name="$POSTGRES_DB" <<'SQL'
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
REVOKE TEMPORARY ON DATABASE :"database_name" FROM PUBLIC;
GRANT CONNECT ON DATABASE :"database_name" TO wareboxes_app;
GRANT USAGE ON SCHEMA public TO wareboxes_app;
SQL
