#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required for the automation-envelope acceptance test" >&2
  exit 127
fi

cargo run --locked --release -p wareboxes-edge-agent --example automation_envelope
