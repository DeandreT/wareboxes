#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"

if [[ ! -d "$ANDROID_HOME/ndk" && -d "$HOME/Android/Sdk/ndk" ]]; then
  export ANDROID_HOME="$HOME/Android/Sdk"
fi
unset ANDROID_SDK_ROOT

if [[ -z "${ANDROID_NDK_ROOT:-}" ]]; then
  ANDROID_NDK_ROOT="$(
    find "$ANDROID_HOME/ndk" -mindepth 1 -maxdepth 1 -type d -print |
      sort -V |
      tail -n 1
  )"
  export ANDROID_NDK_ROOT
fi

if [[ ! -d "$ANDROID_NDK_ROOT" ]]; then
  echo "Android NDK not found below $ANDROID_HOME/ndk" >&2
  exit 1
fi

command -v cargo-apk >/dev/null 2>&1 || {
  echo "cargo-apk is required: cargo install cargo-apk --locked" >&2
  exit 1
}

cargo apk build \
  --manifest-path "$ROOT_DIR/Cargo.toml" \
  --package wareboxes-rf-android \
  --lib \
  --target aarch64-linux-android \
  "$@"
