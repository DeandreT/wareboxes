#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

check_forbidden() {
  local boundary="$1"
  local pattern="$2"
  shift 2

  if matches="$(rg -n "$pattern" "$@" || true)" && [ -n "$matches" ]; then
    printf 'forbidden dependency in %s:\n%s\n' "$boundary" "$matches" >&2
    return 1
  fi
}

check_forbidden \
  domain \
  'wareboxes_(application|core|api|persistence_postgres|worker)|^wareboxes-(application|core|api|persistence-postgres|worker)\s*=|\b(axum|sqlx|validator)(\s*=|::)' \
  crates/domain/Cargo.toml crates/domain/src

check_forbidden \
  application \
  'wareboxes_(core|api|persistence_postgres|worker)|^wareboxes-(core|api|persistence-postgres|worker)\s*=|\b(axum|sqlx|validator)(\s*=|::)' \
  crates/application/Cargo.toml crates/application/src

check_forbidden \
  api-contract \
  'wareboxes_(core|api|persistence_postgres|worker)|^wareboxes-(core|api|persistence-postgres|worker)\s*=|\b(axum|sqlx|validator)(\s*=|::)' \
  crates/api-contract/Cargo.toml crates/api-contract/src

check_forbidden \
  worker-engine \
  'wareboxes_(core|api|persistence_postgres)|^wareboxes-(core|api|persistence-postgres)\s*=|\b(axum|sqlx|validator)(\s*=|::)' \
  crates/worker/Cargo.toml crates/worker/src

check_forbidden \
  postgres-persistence \
  'wareboxes_api|^wareboxes-api\s*=|\baxum(\s*=|::)' \
  crates/persistence-postgres/Cargo.toml crates/persistence-postgres/src

echo "Workspace dependency boundaries are valid."
