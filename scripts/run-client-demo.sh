#!/usr/bin/env bash
# Run the native client against the hosted demo environment.
set -euo pipefail
cd "$(dirname "$0")/.."

export WAREBOXES_API_URL="${WAREBOXES_API_URL:-https://api.88-198-229-62.sslip.io}"
export WAREBOXES_DEMO_EMAIL="${WAREBOXES_DEMO_EMAIL:-demo@wareboxes.app}"
export WAREBOXES_DEMO_PASSWORD="${WAREBOXES_DEMO_PASSWORD:-wareboxes-demo}"

exec cargo run -p wareboxes-client
