#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

command -v cargo-leptos >/dev/null 2>&1 || {
  echo "cargo-leptos is required: cargo install cargo-leptos --locked" >&2
  exit 1
}

cargo metadata --locked --format-version 1 --no-deps \
  --manifest-path "$ROOT_DIR/Cargo.toml" >/dev/null

cd "$ROOT_DIR"
cargo leptos build \
  --release \
  --project wareboxes-web \
  --bin-cargo-args=--locked \
  --lib-cargo-args=--locked

echo "Wareboxes SSR server and web assets built in target/release and target/site"
