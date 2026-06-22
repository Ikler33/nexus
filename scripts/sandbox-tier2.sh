#!/usr/bin/env bash
# SANDBOX Tier-2 live-харнесс для .28 (rootless Podman). Идемпотентно собирает образ + гоняет gated
# #[ignore]-матрицу exec_it. Полный рецепт + reaper/undo/sandbox-run шаги — docs/runbooks/sandbox-tier2.md.
#
# БЕЗОПАСНОСТЬ: использует ТОЛЬКО выделенный TEST-vault (NEUER ~/.nexus/vault). Запускать НА .28, не в CI.
# Использование:  ./scripts/sandbox-tier2.sh [TEST_VAULT_DIR]
set -euo pipefail

REPO_DIR="${REPO_DIR:-$HOME/nexus}"
IMAGE="${NEXUS_SANDBOX_IMAGE:-nexus-agentd:local}"
TEST_VAULT="${1:-$HOME/sbx-test-vault}"

# Фейл-клоуз: не дать случайно нацелиться на живой vault.
case "$TEST_VAULT" in
  "$HOME/.nexus"|"$HOME/.nexus/"*)
    echo "ОТКАЗ: TEST_VAULT указывает на живой ~/.nexus — Tier-2 только на выделенном тест-vault." >&2
    exit 2 ;;
esac

command -v podman >/dev/null 2>&1 || { echo "podman не найден — Tier-2 требует rootless Podman (.28)." >&2; exit 1; }
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"

echo "==> [1/2] podman build ${IMAGE} (с git, из Dockerfile)"
podman build -t "${IMAGE}" -f "${REPO_DIR}/Dockerfile" "${REPO_DIR}"

echo "==> [2/2] gated Tier-2 матрица (exec_it --ignored)"
# shellcheck disable=SC1091
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
cd "${REPO_DIR}"
NEXUS_SANDBOX_IT=1 cargo test -p nexus-core exec_it -- --ignored --nocapture

echo "==> Готово. Reaper/undo/--sandbox-run шаги (4-6) — вручную по docs/runbooks/sandbox-tier2.md."
echo "    Очистка: podman ps -a --filter name=nexus-run --format '{{.Names}}' | xargs -r podman rm -f"
