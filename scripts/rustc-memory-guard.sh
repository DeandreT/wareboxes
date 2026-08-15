#!/usr/bin/env bash
set -euo pipefail

# Cargo's jobs setting is per invocation. An editor check and a deliberate build
# can otherwise run large native and WebAssembly rustc processes concurrently.
# Serialize rustc itself across invocations on Linux; other platforms retain the
# per-invocation Cargo limit.
if command -v flock >/dev/null 2>&1; then
  guard_dir="${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}"
  guard_file="${guard_dir%/}/wareboxes-rustc-${UID}.lock"
  exec 9>"${guard_file}"
  flock 9
fi

exec "$@"
