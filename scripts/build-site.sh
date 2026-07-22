#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_DIR="$ROOT_DIR/_site"

command -v trunk >/dev/null 2>&1 || {
  echo "trunk is required: https://trunkrs.dev/" >&2
  exit 1
}

cargo metadata --locked --format-version 1 --no-deps \
  --manifest-path "$ROOT_DIR/Cargo.toml" >/dev/null

(
  cd "$ROOT_DIR/crates/client"
  trunk build --release --public-url ./
)

mkdir -p "$OUTPUT_DIR/app"
cp -R "$ROOT_DIR/site/." "$OUTPUT_DIR/"
find "$OUTPUT_DIR/app" -maxdepth 1 -type f \
  \( -name 'wareboxes-client-*.js' -o -name 'wareboxes-client-*_bg.wasm' \) \
  -delete
cp -R "$ROOT_DIR/crates/client/dist/." "$OUTPUT_DIR/app/"

# Trunk 0.16 can render a relative public URL as /./, which breaks Pages subpaths.
sed -i 's|"/\./|"./|g; s|'"'"'/\./|'"'"'./|g' "$OUTPUT_DIR/app/index.html"

echo "Site assembled in $OUTPUT_DIR"
