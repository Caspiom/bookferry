#!/usr/bin/env bash
# Uso: spike/scripts/build-linux.sh   (de qualquer diretorio)
set -euo pipefail
cd "$(dirname "$0")/../app"
exec nix develop .. --command bash -c 'cd src-tauri && cargo tauri build --no-bundle'
