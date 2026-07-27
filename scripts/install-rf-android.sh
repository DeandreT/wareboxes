#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SDK_ROOT="${ANDROID_HOME:-$HOME/Android/Sdk}"
if [[ ! -x "$SDK_ROOT/platform-tools/adb" && -x "$HOME/Android/Sdk/platform-tools/adb" ]]; then
  SDK_ROOT="$HOME/Android/Sdk"
fi

ADB="$SDK_ROOT/platform-tools/adb"
if [[ ! -x "$ADB" ]]; then
  echo "adb not found below $SDK_ROOT/platform-tools" >&2
  exit 1
fi

"$ROOT_DIR/scripts/build-rf-android.sh"
"$ADB" install -r "$ROOT_DIR/target/debug/apk/wareboxes-rf.apk"
"$ADB" shell am start -n com.wareboxes.rf/android.app.NativeActivity
