#!/usr/bin/env bash
# Локальный прогон Rust-coverage + per-module гейт (TESTING_STRATEGY §6). То же, что в CI-джобе
# «Coverage (Rust)». Медленно (инструментирует + гоняет все тесты) — НЕ входит в быстрый test-all.sh.
#   bash scripts/coverage.sh
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck disable=SC1091
source "$HOME/.cargo/env" 2>/dev/null || true

echo "── cargo-llvm-cov (Rust, per-file JSON) ──"
( cd apps/desktop/src-tauri && cargo llvm-cov --locked --workspace --json --output-path coverage.json )

echo "── per-module ратчет-гейт ──"
node scripts/check-coverage.mjs apps/desktop/src-tauri/coverage.json
