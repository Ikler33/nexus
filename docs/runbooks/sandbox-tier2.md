# Runbook — SANDBOX Tier-2 live-валидация (Podman, .28)

Tier-1 (без podman, в CI) доказывает контракт песочницы на моках/реальном процессе. **Tier-2** доказывает,
что РЕАЛЬНЫЙ `--network=none` контейнер enforce'ит kernel-инварианты (EROFS/ENETUNREACH/env/output-cap) и что
весь exec-путь (decide→approve→in-container execute→report→undo/reaper) работает вживую. Podman есть ТОЛЬКО на
риге **192.168.0.28** (в CI его нет), поэтому Tier-2-тесты помечены `#[ignore]` и гоняются здесь вручную.

> **CI-posture (§8.2):** Tier-2 — `#[ignore]` (CI-инертно). «Блокирующий гейт» удовлетворяется РУЧНЫМ прогоном
> на .28, чей вывод приложен к PR (как делалось для SANDBOX-5b). Self-hosted Linux-runner с rootless-podman —
> owner-gated follow-up.

## Предусловия на .28 (ssh greenlit)
- rootless Podman 5.7 (`podman --version`); `XDG_RUNTIME_DIR=/run/user/$(id -u)`.
- cargo на PATH: `source ~/.cargo/env`.
- Репозиторий в `~/nexus` на нужной ветке/`main`.
- Qwen на `http://localhost:8080` (для `--sandbox-run`-смоука).
- **ВЫДЕЛЕННЫЙ ТЕСТ-vault** (напр. `~/sbx-test-vault`) — **НИКОГДА `~/.nexus/vault`** (живой agentd-сервис).
- `shell_enable=true` — ТОЛЬКО в тест-vault `local.json`, НИКОГДА в любом не-тестовом конфиге.

## Шаги
1. **Образ с git** (6c-3d добавил git в Dockerfile): `cd ~/nexus && podman build -t nexus-agentd:local -f Dockerfile .`
   (бинарь собирается ВНУТРИ multi-stage-образа — не нативная тяжёлая сборка на прод-LLM-боксе, RAM-защита).
2. **Тест-vault + scratch git-repo**: `mkdir -p ~/sbx-test-vault/.nexus`; `local.json` с `ai.chat.url=http://localhost:8080`,
   `ai.shell_enable=true`, egress-allowlist incl. `localhost:8080`. Для git-undo: `git init ~/sbx-test-repo &&
   (cd ~/sbx-test-repo && git commit --allow-empty -m base)`; в конфиге `ai.exec.git_worktree=~/sbx-test-repo`.
3. **Gated Tier-2 матрица** (харнесс лёгкий, LLM удалённый):
   ```
   cd ~/nexus && source ~/.cargo/env && \
   XDG_RUNTIME_DIR=/run/user/$(id -u) NEXUS_SANDBOX_IT=1 \
   cargo test -p nexus-core exec_it -- --ignored --nocapture
   ```
   Покрывает: EROFS (`:ro`-vault) · no-route (`/proc/net/route` пуст) · env-allowlist-only · argv-no-shell ·
   output-cap · forking-pipe-timeout-in-container · ephemeral `--rm`.
4. **Reaper crash-injection** (6c-3a): запустить approved exec → `podman kill nexus-run-<id>` ДО report →
   строка `agent_actions` зависла EXECUTING/outcome NULL → `reconcile_stale_executing` → row=failed, без undo, без двойного exec.
5. **git-undo entrypoint** (6c-3d-2/6c-3e): мутировать HEAD approved-GitOp в scratch-repo, затем
   `XDG_RUNTIME_DIR=/run/user/$(id -u) nexus-agentd --sandbox-undo ~/sbx-test-vault <run_id> --approve` →
   in-container `git reset --hard <pre-op-ref>` под апрувом → HEAD восстановлен → исходная ledger-строка → undone.
6. **Прод-композиция (регресс, зеркало 5b)**: `nexus-agentd --sandbox-run ~/sbx-test-vault "run: echo hello via shell.run"`
   → в tracing видно host/exec decide→approve→in-container exec→report EXECUTED → ledger + egress_audit по run_id.
   `podman ps -a` — без остатков `nexus-run-*` (`--rm` чисто).
7. **Записать вывод** `cargo test exec_it -- --ignored` в PR как blocking-gate-evidence.

## Очистка
```
podman ps -a --filter name=nexus-run --format '{{.Names}}' | xargs -r podman rm -f
rm -rf ~/sbx-test-vault ~/sbx-test-repo   # если пересоздаёшь
```

## Правила
- Статус владельцу каждые ~5 мин на долгих операциях.
- FOREGROUND/serial — **никаких фоновых git-операций в общем .28-репо** (см. feedback_no_bg_git_in_shared_repo).
- **Никогда** не трогать `~/.nexus/vault`; **никогда** не оставлять `shell_enable=true` в не-тестовом конфиге.
- Скрипт-помощник: `scripts/sandbox-tier2.sh` (шаги 1+3, идемпотентно).

## Результаты live-прогона (2026-06-23, .28, rootless Podman 5.7.0)
ПЕРВЫЙ реальный прогон вскрыл 2 бага САМОГО Tier-2-харнесса (тесты `#[ignore]` живьём не гонялись) —
исправлено в `exec_it.rs` (PR #437):
1. образ несёт `ENTRYPOINT=nexus-agentd` → `podman run <image> true` уходил в `nexus-agentd true`
   («vault path true: No such file»); все 3 теста падали. Фикс: `--entrypoint ""`.
2. `real_vault_write_is_erofs`: rootless под `USER nexus` (не владелец mount'а) → `:ro`-запись падает
   РАНЬШЕ по EACCES, чем по EROFS, маскируя сигнатуру. Фикс: probe под `--user 0:0`.

**Шаг 3 (containment-матрица): 3/3 PASS** — `no_network_route_inside_exec` (/proc/net/route пуст),
`podman_smoke_runs_trivial_container`, `real_vault_write_is_erofs` (EROFS + файл НЕ на host).

**Шаг 6 (прод-композиция `--sandbox-run`): PASS вживую против Qwen** — контейнер поднят, агент достал Qwen
ЧЕРЕЗ egress-proxy (несмотря на `--network=none`), вызвал `shell.run` → host/exec → гейт классифицировал
exec-предложение → **fail-closed DENY** (Confirm never-Auto, headless = нет аппрувера) → агент корректно
отчитался, контейнер вышел `--rm` чисто. Доказан весь exec-composition end-to-end + fail-closed.

**Шаг 5 (`--sandbox-undo`): entrypoint живой и fail-closed** — `--sandbox-undo <vault> <run> [--approve]`
на прогоне без GitOp / на несуществующем run → `restored=0 deferred=0 failed=0`, без краша/двойного exec.

**Отложено (требует интерактивного аппрувера = десктоп-коннектор, НЕ headless):** approved-EXECUTED
ФОРВАРД-путь (shell.run реально выполнен) + живой git-reset round-trip с реальным pre-op-ref + reaper
crash-injection (шаг 4) — все требуют одобрения Confirm-тира, которое headless по дизайну отклоняет.
Покрыто Tier-1-тестами + доказано, что гейт корректно ТРЕБУЕТ одобрения. Догнать при прогоне
десктоп-флоу одобрения exec.
