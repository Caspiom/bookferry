#!/usr/bin/env bash
# Uso: spike/scripts/build-android.sh   (APK debug, so aarch64)
set -euo pipefail
cd "$(dirname "$0")/../app"
nix develop .. --command bash -c '
  [ -d src-tauri/gen/android ] || cargo tauri android init
  cargo tauri android build --debug --target aarch64 --apk
'
ls -l src-tauri/gen/android/app/build/outputs/apk/universal/debug/*.apk
