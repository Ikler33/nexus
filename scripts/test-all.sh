#!/usr/bin/env bash
# Единый прогон всех проверок локально (кросс-план #4г, TESTING_STRATEGY §7 шаг 1).
# Те же гейты, что в CI — «зелено локально ⇒ зелено в CI». Fail-fast.
#   bash scripts/test-all.sh
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck disable=SC1091
source "$HOME/.cargo/env" 2>/dev/null || true

echo "── preflight (гигиена дерева) ──"
node scripts/preflight.mjs

echo "── traceability (AC↔тест + имена) + #[ignore] + версия + egress-линт + AC-Q-6 ──"
node scripts/check-traceability.mjs
node scripts/check-ignored.mjs
node scripts/check-versions.mjs
node scripts/check-egress.mjs
node scripts/check-tooluse.mjs
node scripts/check-dangling.mjs

echo "── Rust: fmt · clippy · test ──"
(
  cd apps/desktop/src-tauri
  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
)

echo "── Frontend: tsc · eslint · vitest · build ──"
(
  cd apps/desktop
  pnpm exec tsc --noEmit
  pnpm exec eslint .
  pnpm exec vitest run
  pnpm exec vite build
)

echo "✅ Все проверки пройдены."
