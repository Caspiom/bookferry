#!/usr/bin/env bash
# Script de teste E2E real do Bookferry Desktop sob Xvfb (webkit2gtk).
# Executa:
# 1. Geração das fixtures sintéticas (EPUB 3 capítulos, PDF 5 páginas, CBZ 6 imagens PNG).
# 2. Build do binário com `BOOKFERRY_E2E=1` (habilita `cfg(bookferry_e2e)` via build.rs).
# 3. Execução sob `xvfb-run -a -s "-screen 0 1280x800x24"` pareando um segundo cliente
#    (`bookferry-client`) como "Celular S23 E2E", validando leitura nos 3 formatos,
#    propagação de posição via SSE, toast de posição remota e modal de retomada (`Ask`),
#    além de capturar screenshots reais em `app/tests/screenshots/`.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
SCREENSHOTS_DIR="$REPO_ROOT/app/tests/screenshots"

TMP_ROOT="$(mktemp -d -t bookferry-e2e-XXXXXX)"
trap 'rm -rf "$TMP_ROOT"' EXIT

VAULT_DIR="$TMP_ROOT/vault"
DATA_DIR="$TMP_ROOT/data"
REPORT_FILE="$TMP_ROOT/e2e-report.json"
LOG_FILE="$TMP_ROOT/e2e.log"

mkdir -p "$VAULT_DIR" "$DATA_DIR" "$SCREENSHOTS_DIR"
rm -f "$SCREENSHOTS_DIR"/*.png

echo "==> [1/3] Gerando fixtures sintéticas em $VAULT_DIR ..."
node "$REPO_ROOT/app/tests/fixtures-gen/generate-fixtures.mjs" "$VAULT_DIR"

echo "==> [2/3] Compilando bookferry-app com BOOKFERRY_E2E=1 ..."
cd "$REPO_ROOT"
BOOKFERRY_E2E=1 cargo build -p bookferry-app

echo "==> [3/3] Executando fluxo E2E sob xvfb-run (1280x800x24) ..."

BOOKFERRY_E2E=1 \
BOOKFERRY_E2E_VAULT="$VAULT_DIR" \
BOOKFERRY_E2E_DATA_DIR="$DATA_DIR" \
BOOKFERRY_SCREENSHOTS_DIR="$SCREENSHOTS_DIR" \
BOOKFERRY_E2E_OUT="$REPORT_FILE" \
WEBKIT_DISABLE_COMPOSITING_MODE=1 \
NO_AT_BRIDGE=1 \
GDK_BACKEND=x11 \
WAYLAND_DISPLAY= \
timeout 80s xvfb-run -a -s "-screen 0 1280x800x24" \
  "$REPO_ROOT/target/debug/bookferry" >"$LOG_FILE" 2>&1 || {
    echo "ERRO: Execução E2E falhou ou expirou. Log completo:"
    cat "$LOG_FILE"
    exit 1
  }

if [[ ! -f "$REPORT_FILE" ]]; then
  echo "ERRO: Relatório E2E não foi gerado. Log completo:"
  cat "$LOG_FILE"
  exit 1
fi

echo "==> Relatório JSON do E2E:"
node -e '
  const fs = require("node:fs");
  const r = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  console.log(JSON.stringify(r, null, 2));
  if (!r.ok || r.error) {
    console.error("E2E reportou erro:", r.error);
    process.exit(1);
  }
' "$REPORT_FILE"

echo "==> Screenshots capturados em $SCREENSHOTS_DIR:"
ls -lh "$SCREENSHOTS_DIR"/*.png
