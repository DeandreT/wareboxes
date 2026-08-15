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

web_pkg_dir="$ROOT_DIR/target/site/pkg"
web_js="$web_pkg_dir/wareboxes-web.js"
referenced_wasm="$(
  sed -nE "s/.*new URL\([\"']([^\"']+\.wasm)[\"'].*/\1/p" "$web_js" | head -n 1
)"
if [ -z "$referenced_wasm" ]; then
  echo "Unable to determine the WASM asset referenced by $web_js" >&2
  exit 1
fi
if [ ! -s "$web_pkg_dir/$referenced_wasm" ]; then
  generated_wasm="$web_pkg_dir/wareboxes-web.wasm"
  if [ ! -s "$generated_wasm" ]; then
    echo "Missing browser WASM assets in $web_pkg_dir" >&2
    exit 1
  fi
  cp "$generated_wasm" "$web_pkg_dir/$referenced_wasm"
fi

cargo build \
  --release \
  --locked \
  --package wareboxes-worker-process \
  --bin wareboxes-worker

echo "Wareboxes server, worker, and web assets built in target/release and target/site"
