#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

global_definitions="$({
  rg -o --no-filename -- '--[a-zA-Z0-9-]+[[:space:]]*:' \
    apps/web-ops/style/main.css \
    apps/web-ops/public/presentation.css
} | sed 's/[[:space:]]*:$//' | sort -u)"

used_tokens="$(
  rg -o --no-filename -- '--[a-zA-Z0-9-]+' \
    apps/web-ops/style \
    apps/web-ops/public \
    | sort -u
)"

local_tokens='--border-default
--portal-accent
--portal-accent-soft
--portal-canvas
--portal-card-shadow
--portal-panel-line
--split-master-width
--text-primary
--text-secondary
--text-tertiary'

missing="$(
  comm -23 \
    <(printf '%s\n' "$used_tokens") \
    <(printf '%s\n%s\n' "$global_definitions" "$local_tokens" | sort -u)
)"
if [ -n "$missing" ]; then
  printf 'web-ops CSS uses tokens that are neither global nor explicitly local:\n%s\n' "$missing" >&2
  exit 1
fi

while IFS= read -r stylesheet; do
  relative="${stylesheet#apps/web-ops/public/}"
  case "$relative" in
    presentation.css|workbench.css|workspace-layout.css) continue ;;
  esac
  if ! rg -q -F "url(\"/$relative\")" apps/web-ops/public/workbench.css; then
    printf 'web-ops feature stylesheet is missing from workbench.css: %s\n' "$relative" >&2
    exit 1
  fi
done < <(rg --files apps/web-ops/public -g '*.css' | sort)

if rg -n '<link rel="stylesheet" href="/(?!pkg/wareboxes-web\.css|workbench\.css|presentation\.css)' \
  apps/web-ops/src/app.rs --pcre2; then
  echo 'web-ops shell must load feature styles through workbench.css' >&2
  exit 1
fi

echo 'web-ops CSS contract is valid.'
