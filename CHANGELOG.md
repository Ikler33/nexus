# Changelog

Все значимые изменения проекта документируются в этом файле.
Формат основан на [Keep a Changelog](https://keepachangelog.com/ru/1.1.0/);
проект придерживается [Semantic Versioning](https://semver.org/lang/ru/).

## [Unreleased]

### Агент · SANDBOX-6c-3d-1 — образ +git, `ai.git_worktree` (owner-gated) + Tier-2 live-runbook/скрипт

Разблокировка live-валидации Tier-2 на .28 + предпосылка для реального exec-GitOp отката. Малый CI-mergeable срез (entrypoint `--sandbox-undo` + прод `SandboxUndoExecDriver` — 6c-3d-2).

- **Dockerfile +git** (~+15МБ): без git-бинаря в образе `git.op` И реальный `git reset --hard` (6c-3e undo) в контейнере СТРУКТУРНО невозможны. Валидируется существующим docker-smoke (paths-gated, срабатывает от касания Dockerfile).
- **`ai.git_worktree: Option<String>` (config, OWNER-GATED, default None)**: опц. ПЕРСИСТЕНТНЫЙ writable git-worktree для реального отката exec-GitOp. `None` (дефолт) → откат остаётся `Deferred` (vault `:ro`, scratch эфемерен — кросс-прогонный reset невозможен). `Some` → ОТДЕЛЬНЫЙ rw-mount (НИКОГДА не vault) в undo-контейнер. Включает ТОЛЬКО владелец; vault всегда `:ro`.
- **`docs/runbooks/sandbox-tier2.md` + `scripts/sandbox-tier2.sh`**: точный live-рецепт (.28, выделенный TEST-vault, НИКОГДА `~/.nexus/vault`; podman build образа → gated `cargo test exec_it -- --ignored` → reaper/undo/--sandbox-run шаги). Скрипт fail-closed отказывается целиться в живой `~/.nexus`.

Tier-1: `git_worktree_default_none_and_parses` (config round-trip). clippy 0, fmt + node-lints зелёные. Образ с git строит docker-smoke. Live containment-матрица (6c-3a/b/c `#[ignore]`) теперь прогоняема на .28. `--sandbox-undo` entrypoint + реальный container-spin undo-драйвер — 6c-3d-2.

### Агент · SANDBOX-6c-3b/c — Tier-2 containment-матрица + always-CI edge-тесты exec-раннера

Доказывает, что то, что Tier-1 мокал, реально enforced'ится — частью на УРОВНЕ ЯДРА контейнера (Tier-2, `#[ignore]` на .28), частью на реальном процессе БЕЗ podman (always-CI host-тесты).

- **Always-CI host-тесты `RealExecRunner` (`exec_child.rs`, `#[cfg(unix)]`, НЕ `#[ignore]` — гоняются в CI/локально)**:
  - `real_forking_grandchild_holds_pipe_returns_at_timeout` — форк-демон-кейс: родитель бэкграундит внука, держащего stdout fd1, и сразу выходит; `read_capped_tail` не получит EOF, но внешний `tokio::timeout`+`kill_on_drop` ОБЯЗАН вернуть `run()` ~таймаут (не виснуть до выхода внука) — **podman-free durable-гарантия**.
  - `real_large_output_capped` — вывод ≫ cap (200КБ через `head -c /dev/zero`, портируемо macOS+linux) ⇒ `stdout_truncated=true`, хвост ограничен cap (ring), exit 0, без OOM.
- **Tier-2 containment-матрица (`exec_it::tier2`, `#[ignore]`, только .28)**: `real_vault_write_is_erofs` (запись в `:ro`-vault → kernel EROFS, файл не создан) · `no_network_route_inside_exec` (`--network=none` → `/proc/net/route` без маршрутов). Хелпер `hardened_podman_run` (зеркало sandbox_run_plan-флагов). env-allowlist/no-shell/timeout уже доказаны host-level Tier-1 (`real_env_clear_proven`/`real_*`), не дублируются.

Tier-1 (+2 always-CI → 869 nexus-core, 0 failed) + 2 `#[ignore]` (→ check-ignored 28→30, gated). clippy 0, fmt + node-lints зелёные. Остаток 6c-3: 6c-3d (Dockerfile +git, `--sandbox-undo` entrypoint + прод undo-драйвер + `ai.exec.git_worktree` owner-gated + runbook) + live .28.

### Агент · SANDBOX-6c-3e — реальный exec-GitOp откат: `UndoExecDriver` seam (gated/ledgered, не raw-host)

Превращает `UndoStatus::Deferred(ExecGitRef)` (6c-2h surfacing) в РЕАЛЬНУЮ обратимость — через шов, а не привилегированный путь. **CI-mergeable seam + Tier-1** (реальный sandboxed `git reset` гоняет прод-драйвер 6c-3d на .28). Default-inert: пока композиционный корень не подставил драйвер, поведение БАЙТ-в-байт как 6c-2h.

- **`UndoExecDriver` trait** (`undo.rs`) + **`undo_run_with_driver(.., driver: Option<&dyn UndoExecDriver>)`**; `undo_run` — тонкая обёртка `(.., None)` ⇒ ВСЕ существующие vault-only вызыватели (handler/session/desktop/тесты) НЕ тронуты (INV-DEFAULT-INERT). ЦЕНТРАЛЬНЫЙ дизайн: откат GitOp — САМ мутирующий GitOp, поэтому RE-ENTER'ит тот же host/exec гейт (classify→decide→approve→in-container execute→report), НЕ ungated спец-путь.
- **ExecGitRef-арм `undo_run`**: ре-валидирует ref host-side (`is_git_sha`) ПЕРЕД вызовом драйвера (defense-in-depth — ledger мог быть подменён; инъекц/мусор-ref ⇒ `Failed`, драйвер НЕ зовётся, никакого `git reset --hard <garbage>`); валидный ref + `Some(driver)` ⇒ реальный откат (`Restored` ⇒ исходную строку помечают executed→undone ТОЛЬКО после успеха; `Deferred`/`Failed` ⇒ строка остаётся, retry-safe); `None` ⇒ Deferred surfacing.
- **Инварианты**: INV-UNDO-GATED (re-enter гейт, нет raw-host exec) · INV-UNDO-NO-HOST-GIT (reset ВНУТРИ контейнера — check-sandbox-exec зелёный) · INV-UNDO-HOST-AUTHORITY (ref из ledger, не от модели; ре-валидирован) · INV-UNDO-NEVER-AUTO (синтезированный reset Confirm-never-Auto ⇒ агент не само-апрувит свой undo) · INV-UNDO-MARKED-ON-VERIFY (undone только после reset EXECUTED) · INV-EXEC-IRREVERSIBLE (shell/process без ExecGitRef-хэндла ⇒ драйвер не зовётся).

Tier-1 (+7, итого 867 nexus-core, 0 failed): no-driver-still-deferred · driver-restored-marks-undone · driver-rejected-stays-deferred · driver-failed-not-undone · invalid-ref-never-calls-driver (host-authority над ref) · shell-exec-has-no-undo-handle · undo-reset-action-gated-never-auto (git reset --hard под PolicyDefault → Rejected). clippy 0, fmt + node-lints зелёные. Прод `SandboxUndoExecDriver` (реальный container-spin) + `ai.exec.git_worktree` (owner-gated rw worktree, default OFF) + `--sandbox-undo` entrypoint — 6c-3d; реальный `git reset` round-trip — Tier-2 .28.

### Дизайн · Hermes-6 MASTHEAD-1 — editorial-шапка + адаптивная буквица в reading-view редактора

Net-new render-фича превью/чтения (`editor.jsx`/`dropcap.js`/`app.css`): в `MarkdownPreview` появилась editorial-шапка (kicker из тегов · display-title Cormorant · mono-byline время/слова/чтение) и буквица ведущего абзаца.

- **Масthead** (`apps/desktop/src/lib/editor/masthead.ts` — чистый `deriveMasthead`): title = frontmatter `title` → текст ведущего H1 → имя файла; kicker = frontmatter-теги; byline берёт `mtime` (живёт в GroupPane) + слова/время чтения. Ведущий H1 **обнуляется** в теле (строка → пустая, перевод строки сохранён) — не дублируется в заголовке И номера строк не сдвигаются (тоггл тасков EDIT-5 / переход по оглавлению EDIT-7 целы). `title`/`tags` исключаются из Properties-таблицы (они уже в шапке).
- **Буквица** (порт `dropcap.js` как `useLayoutEffect`): штампует первую букву абзаца-зачина в `data-cap`, CSS тюнит оптический зазор по глифу (широкие М/Ш жмутся, узкие Г/Т дышат, круглые О/С крупнее без смены слива). Только абзац-зачин (список/заголовок первым блоком → буквицы нет). Reading-режим: центрированная шапка + буквица 3.9em.
- Рендерится ТОЛЬКО у top-level превью редактора (`GroupPane` передаёт prop `masthead`); embed/peek/доска не передают → шапки/буквицы нет. Прежняя строка `docMeta` свёрнута в byline. CSP-инвариант цел (`data-*`-атрибут с одной буквой — не вектор инъекции; без raw-HTML/inline-style). Только токены `--color-*`/`--font-*`.
- Adversarial-ревью (2 линзы: корректность + CSP/CSS/fidelity, вердикт SHIP). Фиксы по ревью: эффект на примитивных deps (не на свежем объекте `masthead`), снятие закрывающей ATX-`#`, снятие inline-`*`/`` ` `` из заголовка, перенос slug-id ведущего H1 на заголовок шапки (HEADANCHOR-1 якорь/дедуп целы). tsc·eslint·vitest 926·build·coverage·node-чеки — зелёные; скриншот-верификация light+dark + reading-режим.

### Настройки · Тогглы автономного (headless) агента в «AI / Модели» ⚠️ OWNER-GATED (Hermes-6/SYNC-NOTE)

Вывод трёх backend-флагов headless-агента (`nexus-agentd`) тумблерами в Настройки→ИИ, новый блок **«Автономный (серверный) агент»** (default-OFF/confirm, fail-closed, consent-предупреждения). Флаги читает ТОЛЬКО агентд из `.nexus/local.json`; десктоп их рантаймом не применяет (автономия прогона — per-run в UI; web — отдельный `websearch.json`) — честная рамка в интро блока.
- **Контролы:** `ai.agent_autonomy` (сегмент confirm|auto, дефолт confirm) · `ai.sandbox_enabled` (Linux-only) · `ai.shell_enable` (host-exec, всегда Confirm/никогда Auto; disabled пока нет sandbox+Linux) · `ai.web.allow_public_fetch` (публичный egress). sandbox/shell — disabled на не-Linux (бэк отдаёт `shellSupported = cfg!(linux)`); consent-warn на auto/shell-on/public-fetch-on (зеркало WebSearchBlock).
- **Бэк:** `AiConfigDto` расширен (camelCase) + новая команда `set_agent_flags` (мгновенный персист, БЕЗ hot-apply/egress-ресинка — это конфиг агентд, не десктопа); чистая `apply_agent_flags` сохраняет все прочие ключи local.json. Когерентность shell↔sandbox форсится на trust-boundary (нельзя записать `shell=true` при `sandbox=false`), не только в UI.
- **Safety:** `WebConfig.url` получил `#[serde(default)]` — частичный `ai.web` (только `allow_public_fetch`, без url) больше не валит парс всего local.json (был data-loss-класс: терялся chat/embedding-конфиг). Mock зеркалит контракт (нормализация autonomy, когерентность shell↔sandbox). UI persist — через ref+seq-гард (быстрый тоггл двух контролов не затирает друг друга стейлом).
- Тесты: nexus-core (partial-web парс/инертность, agent-флаги дефолты) · nexus-desktop (`apply_agent_flags`: сохранение ключей/web-merge/когерентность/round-trip) · vitest (рендер блока, autonomy+consent-warn, sandbox/shell disabled, public-fetch, ref-регрессия). Adversarial-ревью пройдено; превью-DOM-верификация. **Мерж — за владельцем (новая security-поверхность).**

### Дизайн · Hermes-6 PR-C — app-shell (Titlebar): чат-пузырь AI-тоггл + `/`-разделитель языка

Финальный хром-полиш по `app.jsx`: AI-панель тоггл `PanelRight` → `MessageCircle` (чат-пузырь, по брифу); разделитель RU/EN `·` → `/`. Прочий app-shell (ActivityBar/StatusBar/grid/splits/tabs/reading/scrim) уже совпадал с хэндоффом — не трогался. BrandMark-спутник остаётся `var(--color-accent)` (тема-реактивный, НЕ хардкод-hex). tsc·eslint·vitest — зелёные; DOM-верификация тоггла/разделителя.

### Дизайн · Hermes-6 фан-аут PR-B — рескин read/nav-вью (Home/News/Today/Board/Sidebar/Graph/Палитра/Plugins)

In-place CSS-рескин 8 поверхностей под `home.jsx`/`news.jsx`/`screens.jsx`(Today/Board)/`sidebar.jsx`/`graph.jsx`/`palette.jsx`/`plugins.jsx`:
- **Home:** greeting → `--display-lg` headline, continue-card headline-title/serif-snippet, плоский фон, приглушённые иконки.
- **News:** ADD **drop cap** (`::first-letter` Cormorant 3.5em accent на первом абзаце), editorial-reader (kicker 0.2em, lede 20px, paragraph 18px/1.8), CTA → headline/display-md.
- **Today/Board:** carded-list идиом (плоские ряды → elevated 14px-карточки, status-dots, mono-секции, mono-бейджи); Board DnD/list/task-peek/properties сохранены.
- **Sidebar/Graph/Palette/Plugins:** точечный полиш (active-state, glass-палитра, plugin-карточки/audit).

Рескин-на-месте — 0 регрессий (adversarial-ревью 4 линзы). Только per-component CSS (+ аддитивные спаны/иконо-боксы); shared-файлы не тронуты.

### Дизайн · Hermes-6 фан-аут PR-A — рескин модалок/инсайтов (Память/Эпизоды/Дайджест/Цели/Противоречия/Входящие/DeadJobs/Sync+Conflict)

In-place CSS-рескин 5 поверхностей под `screens.jsx`/`insights.css`/`sync.css`/`conflict.css`: заголовки → `--font-headline` 19px, мета → `--font-mono`, soft-accent пилюли, ember-акцент вместо cool-`--color-ai`, теги → `--color-tag`, soft-square чипы, entry-motion (m-fade/m-pop), empty-state иконо-боксы. Рескин-на-месте — 0 регрессий фич/i18n/a11y (adversarial-ревью 3 линзы, вердикт MERGE). Только per-component `.module.css` (+ аддитивные `emptyIcoBox`/`headIcon` спаны); shared-файлы не тронуты.

### Дизайн · AI-панель Castor (Hermes-6, Фаза B срез 1) — 2 вкладки + икон-композер + релокация «Связей»

Рескин AI/чат-панели под hi-fi макет (`ai-panel.jsx`). Рескин-на-месте — вся проводка чата сохранена.

- Шапка **«Castor»** (орбита-глиф + провайдер-бейдж + история/новая + развернуть-в-раздел Агента + закрыть); **2 вкладки: Чат · Castor** (`AiTab: 'chat'|'agent'`), вкладка Castor = `AgentTab`-лаунчер.
- **Релокация «Связи» (SuggestView) → инспектор-рейл редактора** (секция `suggest` + `pendingInspectorSection` по паттерну `pendingTagFilter`; команда палитры `view.suggest` перенацелена; reading-режим сбрасывается). «Похожие» уже в рейле.
- Композер по `ai.css`: scope-чип «По заметкам/Общий» (клик циклит) + Web/Pin икон-тогглы + круглый send/stop; empty «Спросите Castor».
- Adversarial-ревью (4 линзы, 0 блокеров): фиксы M1 (токен stopBtn), M2 (reading:false + тест), чистка осиротевших i18n-ключей. M3 (панельная RelatedView со слайдером) — осознанная минор-потеря (вставка-ссылки в SuggestView).

tsc · eslint · vitest 899/899 · build · node-чеки — зелёные; скриншот-верификация light+dark.

### Агент · SANDBOX-6c-3a — Tier-2 фундамент: podman-gate + crash-recovery reaper зависших exec

Старт финального exec-слайса (Tier-2 live на Podman .28). Два куска фундамента, оба **pure-code/CI-green** (podman НЕТ ни локально, ни в CI → live-тесты `ignore`-гейтятся и гоняются только на .28).

- **`sandbox::exec_it` (test-only модуль)** — ЕДИНЫЙ gate-предикат `podman_it_enabled()` (тройной fail-closed lock: `cfg(target_os="linux")` + `ignore`-атрибут + `NEXUS_SANDBOX_IT=1` И реальный `podman --version`); чистый комбинатор `it_gate` (юнит-тестируем без бинаря) + `is_safe_test_vault` (структурный отказ работать под `$HOME/.nexus` — Tier-2 только на TempDir/тест-vault, НИКОГДА прод). Один `ignore`-смоук `podman run --network=none <image> true`. Будущие Tier-2 матрицы (6c-3b/c/e) лягут сюда за тем же gate.
- **`audit::reconcile_stale_executing` (crash-recovery reaper, §6 TTL)** — финализирует `FAILED` строки `agent_actions`, застрявшие в `EXECUTING` (`outcome IS NULL`) дольше `EXEC_STALE_TTL_SECS=600` (5× 120с exec-кэп): контейнер исчез ПОСЛЕ redeem но ДО report, in-memory `in_flight`-карта потеряна на рестарте. **MARK FAILED, НЕ requeue** (exec не replay-safe: одноразовый токен консьюмнут, частичный эффект мог случиться); reaped-строка без undo-хэндла ⇒ вне `actions_for_undo` (необратима); фенс `outcome IS NULL` ⇒ взаимоисключение с `finish` CAS (first-terminal-wins). Прокинут в agentd crash-recovery рядом с `requeue_stale_running` (старт = момент потери in_flight).

Tier-1 (+8, итого 860 nexus-core, 0 failed): reaper — marks-stale-failed/skips-fresh/ignores-terminal-and-non-executing/idempotent-vs-finish (first-terminal-wins)/reaped-not-undoable; gate — requires-both-env-and-binary/disabled-without-env/test-vault-guard. CI-линты: `check-sandbox-exec` (podman-probe помечен маркером, exec_it НЕ whole-file-exempt) + `check-ignored` EXPECTED 27→28. Live killed-container reaper + containment-матрица — 6c-3b/c/f (gated). clippy 0, fmt зелёные.

### Агент · SANDBOX-6c-2h — обратимость exec: GitOp pre-op-ref undo (§5.5, data-plumbing)

Завершает 6c-2 (exec в коде). Обратимость exec ТАМ, ГДЕ ВОЗМОЖНА (только GitOp): мутирующая git-операция несёт восстановимый pre-op git-ref в ledger ДО финализации; shell/process НЕОБРАТИМЫ (и classify их не Auto). Узкий скоуп: data-plumbing pre-op-ref + персист — реальный `git reset` отложен в 6c-3 (Tier-2 live) за документированным seam.

- **`ProxyExecDispatcher.dispatch_exec` (in-container)**: для GitOp ПЕРЕД мутацией снимает pre-op ref read-only-инвокацией `git rev-parse HEAD` через ТОТ ЖЕ confined [`ExecRunner`] (тот же cwd/env/кэпы). **Best-effort**: ненулевой код / пустой / не-hex вывод (не git-репо, detached) → `undo_ref=None` (необратимо, не падаем). Передаётся в `report`. shell/process — probe не запускается.
- **`exec_host.report` (host, host-authority над обратимостью)**: `InFlightExec.undo_eligible` вычислен из СОХРАНЁННОГО действия на execute (= это GitOp) — НЕ из claim контейнера. `undo_ref` персистится как `UndoCols{kind:exec_gitref}` ТОЛЬКО при `undo_eligible` И валидном ref → контейнер не сделает shell/process «обратимым», подсунув undo_ref.
- **HOST-AUTHORITY над СОДЕРЖИМЫМ ref (adversarial-ревью MAJOR)**: host НЕ доверяет in-container probe (тот бежит на НЕдоверенной стороне) — РЕ-валидирует `undo_ref` сам через `is_git_sha` (непустой, ≤64 hex) ПЕРЕД персистом; мусор/инъекц-строка (`HEAD; rm -rf ~`) → `undo=None` (необратимо, fail-closed). Тот же предикат переиспользует probe (единый источник). Закрывает persist-seam, который 6c-3 скормил бы в `git reset --hard`.
- **`UndoHandle::ExecGitRef{reference}`** + `to_cols`/`from_cols` (`exec_gitref`); `undo_run` surfacing-арм → `UndoStatus::Deferred` (ref зафиксирован, реальный `git reset --hard <ref>` — 6c-3; строка НЕ помечается undone, чтобы 6c-3 завершил). Новый `UndoOutcome::deferred()`; `failed()` теперь СЧИТАЕТ явно (PathEscape|Failed), deferred под него не маскируется; `fully_undone()` = ни провала, ни отложенного.

Tier-1 (+9, итого 852 nexus-core, 0 failed): `gitop_captures_pre_op_ref` (rev-parse ПЕРВЫМ, sha→undo_ref) · `gitop_rev_parse_failure_irreversible` (probe fail→None, не падает) · `non_gitop_skips_pre_op_probe` · `gitop_report_persists_gitref` (ledger exec_gitref+sha, round-trip) · `non_gitop_report_ignores_undo_ref` (host-authority над обратимостью) · `gitop_report_rejects_nonhex_undo_ref` (host-authority над содержимым: инъекц-строка отвергнута) · `is_git_sha_validates` · `exec_gitref_cols_roundtrip` · `exec_gitref_undo_is_deferred` (Deferred, не undone, не провал). 2-линзовый adversarial-ревью (security+корректность) → 1 MAJOR (host доверял контейнерному ref) ЗАКРЫТ в срезе host-side ре-валидацией. agentd компилируется, clippy 0, fmt + node-lints зелёные. Реальный git-reset undo-apply — 6c-3 (Tier-2 live podman .28).

### Агент · SANDBOX-6c-2g — события exec (`ExecProposal`/`ExecResult`) + `ChangeKind::Exec` + wire-маппинг

Наблюдаемость + долговечный вид exec. Аддитивно поверх рабочего exec-ядра (6c-2a..f): host эмитит структурные события exec-намерения и исхода; UI/лог/десктоп-релей видят exec-активность БЕЗ сырого содержимого команды/вывода (редакция-дисциплина §5.6, зеркало `DiffSummary`).

- **`ChangeKind::Exec`** (`audit.rs`, токен `"exec"`): `orchestrate::change_kind` exec-таргетов → `Exec` (корректность; по exec-пути `diff_summary` в журнал НЕ пишется — exec вне vault-diff). `FileStatus` НЕ расширяется (exec не порождает changeset-файл).
- **`AgentEvent::ExecProposal { run_id, action_id, summary }` + `AgentEvent::ExecResult { run_id, action_id, exit_code, finalized }`** (`agent/event.rs`) — **STRUCT-варианты** (НЕ newtype: serde-internal-tag `tag="type"` молча терял бы newtype-варианты — задокументировано в event.rs/sandbox/event.rs); явный camelCase (`runId`/`actionId`/`exitCode`). `summary` — редакция-безопасный силуэт (имя инструмента + `op`-токен git + счётчик argv), `ExecResult` СОДЕРЖИМОЕ-СВОБОДЕН (нет stdout-поля by-design).
- **Зеркальные `AgentStreamEvent::ExecProposal`/`ExecResult` + `map_agent_event`-рукава** (`wire.rs`): экзаустивный матч компилит-форсит wire-решение; `EventForwardServer` релеит exec-события контейнер→host→десктоп без доп. проводки (через `event_notification`→`map_agent_event`).
- **Эмиссия host-side через `ctx.events`**: `ExecProposal` в `dispatch_exec_decision` (+параметр `events: &dyn EventSink`) ПОСЛЕ proposed-строки и ДО запроса решения; `ExecResult` в `DispatchExecBackend.report` после финализации ledger. `TracingEventSink` логирует оба (headless-наблюдаемость, паритет с Proposal).
- **`UNDO_EXEC_GITREF="exec_gitref"`** (`actuator/mod.rs`) — стабильный ledger-дискриминант отмены exec-GitOp объявлен здесь; персист `undo_ref`+exec-undo-ветка — 6c-2h.

Tier-1 (+8, итого 843 nexus-core): `change_kind_classifies_exec` · `exec_proposal_summary_is_content_free` (плантованный секрет ОТСУТСТВУЕТ в summary) · `exec_proposal/result_is_struct_variant_roundtrip` (newtype-loss регресс-гард) · `map_agent_event_covers_exec_variants` · `exec_decide_emits_exec_proposal`/`exec_report_emits_exec_result` (CollectingSink) · `event_forward_relays_exec_result` (сквозной релей до десктопа). agentd компилируется, clippy 0, fmt + node-lints зелёные. Остаётся undo (6c-2h), live-валидация (6c-3).
### Дизайн · Orvin/Castor foundation (Hermes-6) — бренд-знаки + ребренд + токен

Фаза A эпика переноса дизайна Hermes-6 (бренд Qasr → **Orvin** app / **Castor** agent): серийный фундамент бренд-примитивов до фан-аута рескина вью. Лейаут вью НЕ меняется (рескин панелей/вью — отдельные срезы Фазы B).

- **BrandMark** → Orvin «узел-на-орбите» (tile-less: кольцо `currentColor` + ember-точка `var(--color-accent)`, разрыв под спутником; README §6). **BrandThinking** + `motion.css` → орбита-спиннер (`bt-orbit/bt-ring/bt-sat` + `.idle` + reduced-motion) — атомарная пара (13 потребителей не трогаются).
- **Бренд-глифы** `OrbitIcon`/`CometIcon` (drop-in для lucide): свопы `sparkles`→орбита (11 AI-афордансов) и `bot`→комета Castor (нав агента); Titlebar 'midnight'-тема → нейтральный `MoonStar` (трёхуровневый язык: орбита = AI-слой Orvin, комета = Castor, lucide = прочее).
- **Ребренд** i18n Qasr→Orvin (`app.name` + onboarding/tree/chat/news, 7×2) + Castor (`agent.title`, `view.agent` в нав/палитре/шпаргалке, `who.agent`); окно Tauri + `<title>` + favicon (ember-плитка + белый Orbit-O). **БЕЗ** `s/nexus/` — тех-идентификаторы (`.nexus/`, `app.nexus.desktop`, `nexus.locale`) целы.
- Аддитивный токен `--color-warning-soft` (`color-mix`, авто-адаптация ко всем 13 темам); mock home Qasr→Orvin.

tsc · eslint · vitest 898/898 · vite build · node-чеки `test-all` — зелёные. Скриншот-верификация знаков (light+dark, @24/@76). Adversarial-ревью (5 линз) — 0 блокеров после фиксов.

### Агент · SANDBOX-6c-2f-3 — host-проводка exec end-to-end (serve_host в SandboxRunner + agentd CLI/харнесс)

Завершает 6c-2f: host теперь ОТВЕЧАЕТ на host/exec вживую (под `shell_enable`), замыкая полный exec-путь decide→execute→exec_child→report. Default-OFF сохранён двухуровнево (registry + serve_host). Завершает Фазу-3 host/exec в коде; остаются события (6c-2g), undo (6c-2h), live-валидация (6c-3).

- **`SandboxRunner::run<Eb,Ab,Xb>`** +параметр `exec_server: Option<HostExecServer<Xb>>`: accept-таск act.sock зовёт **`serve_host`** (host/act+host/exec по методу на одном peer-gated соединении) вместо `serve_act`; `None` → host/exec `method_not_found` (fail-closed). serve_act остаётся (тест/act-only).
- **`SandboxChildArgs.shell_enable`** + `to_argv` 6-й позиционный (`"true"`/`"false"`); agentd `--sandbox-child` парсит 6 аргументов (строгий bool-парс, fail-closed) → `SandboxChildSpec.shell_enable`.
- **`run_sandbox_host` (`--sandbox-run`)**: строит `DispatchExecBackend`+`HostExecServer` ТОЛЬКО при `cfg.ai.shell_enable` (иначе `None`), деля ТОТ ЖЕ `GatedToolCtx` (общий ledger/policy/kill-switch `agent_paused` через `Clone`) с act-бэкендом → exec и vault под единым гейтом; `SandboxChildArgs.shell_enable` зеркалит host-гейт в argv контейнера.

Tier-1: `child_argv_is_positional_and_safe` обновлён (6 аргументов вкл. shell_enable); serve_host-тесты (6c-2f-1) покрывают роутинг. 836 nexus-core, agentd компилируется, clippy 0, fmt + node-lints зелёные. Прод default-OFF (`ai.shell_enable=false` → exec не зарегистрирован И host/exec method_not_found). Live exec — 6c-3 (podman .28).

### Агент · SANDBOX-6c-2f-2 — регистрация exec-инструментов в песочном реестре (default-OFF gating, in-container)

In-container проводка exec: контейнерный реестр получает 3 exec-инструмента ТОЛЬКО при `shell_enable`, деля act.sock с note-инструментами через `Arc`-транспорт. Host-сторона (serve_host в SandboxRunner.run + agentd CLI-арг) — 6c-2f-3.

- **`child::build_sandbox_registry<A: Transport>(act: Arc<A>, shell_enable)`**: note-инструменты (всегда, host/act) + при `shell_enable` — `ShellRunTool`/`ProcessSpawnTool`/`GitOpTool` поверх `ProxyExecDispatcher`(`RealExecRunner`, cwd-базы `CONTAINER_SCRATCH`/`CONTAINER_VAULT`). **DEFAULT-OFF**: при `false` exec-инструменты СТРУКТУРНО отсутствуют (агент их не назовёт). `act`-транспорт ОБЩИЙ (`Arc`-клоны) для host/act + host/exec → одно соединение (host-side `serve_host` роутит по методу, 6c-2f-1).
- `SandboxChildSpec.shell_enable` (новое поле); `run_sandbox_child_session` оборачивает act в `Arc` и зовёт `build_sandbox_registry`. agentd `--sandbox-child` пока хардкодит `shell_enable=false` (6-й CLI-арг — 6c-2f-3).
- INV-CMD-SITE цел: child.rs РЕФЕРЕНСИТ `RealExecRunner` (unit-структуру), но НЕ конструирует Command (реальный спавн — exec_child.rs) — `check-sandbox-exec.mjs` зелёный.

Tier-1: `registry_gates_exec_tools_on_shell_enable` (shell_enable=false → exec-инструментов НЕТ, note есть; =true → все 4 есть). clippy 0, fmt + node-lints зелёные. Инертно для прод-пути (agentd shell_enable=false; serve_host-swap — 6c-2f-3).

### Агент · SANDBOX-6c-2f-1 — `serve_host` (host/act + host/exec на одном соединении) + `Arc<T>: Transport`

Первый из двух суб-срезов проводки exec: host-side примитивы. `serve_host` — security-keystone (обе RPC на одном peer-gated канале) + sharing-примитив для контейнерных шимов. Сама проводка (runner.run/child.rs/agentd CLI) — 6c-2f-2.

- **`runner::serve_host<T,Ab,Eb>`**: обслуживает act.sock, маршрутизируя `host/act`→`HostActServer` + `host/exec`→`HostExecServer` ПО МЕТОДУ на ОДНОМ соединении. `exec_server=None` (когда `shell_enable=false`) → `host/exec`→`method_not_found` (fail-closed); неизвестный метод→`method_not_found`. Заменит `serve_act` на act.sock когда подключён exec (6c-2f-2). SO_PEERCRED-гейт — тот же accept-цикл (act/egress/event), отдельного 4-го сокета/гейта НЕТ.
- **`impl<T: Transport + ?Sized> Transport for Arc<T>`** (connect): делегирует send/recv → ДВА `Arc`-клона делят ОДНО act.sock-соединение (контейнерные `ProxyActuator` host/act + `ProxyExecDispatcher` host/exec). Безопасно: tool-вызовы СЕРИАЛИЗОВАНЫ (`run_agent_loop` — один инструмент за раз).

4 Tier-1 теста (channel_pair): serve_host роутит обе RPC на одном соединении · exec-disabled→method_not_found · unknown-method→method_not_found; arc-transport-делит-соединение. clippy 0, fmt + node-lints зелёные. Инертно (serve_host/Arc-impl зовутся проводкой 6c-2f-2; serve_host unix-only как остальной runner).

### Агент · SANDBOX-6c-2e-2 — 3 exec-инструмента (`shell.run`/`process.spawn`/`git.op`) поверх `ExecDispatcher`

Шестой суб-срез: сами exec-инструменты агента (логика разбора+свёртки), транспорт-агностичные через шов `ExecDispatcher` (6c-2e-1). Регистрация в `child.rs` при `shell_enable` + проводка `ProxyExecDispatcher` поверх shared act.sock + `serve_host` (host отвечает host/act+host/exec на одном соединении) — 6c-2f.

- **`nexus-core::sandbox::exec_tools`**: `ShellRunTool`/`ProcessSpawnTool`/`GitOpTool` (impl `Tool`), держат `Arc<dyn ExecDispatcher>`. `invoke`: строгий разбор args (`deny_unknown_fields`, I-4 fail-closed; пустой argv/program → `BadArgs`) → типизированный exec-`Action` → `dispatch_exec` → свёртка `ExecToolOutcome` в текст. **Decision-исходы (Rejected/HardBlocked) — `Ok`-обратная связь** агенту (не ошибка инструмента); ошибка — лишь транспорт/протокол/разбор. `Executed` (любой exit) — результат команды (exit + усечённые хвосты + метки усечения/таймаута).
- Зеркало note-инструментов (`actuator::tools`), но для exec; ProxyExec/exec_child реальный спавн — НЕ здесь (Command только в `exec_child.rs`, INV-CMD-SITE цел).

9 Tier-1 тестов (MockExecDispatcher без транспорта/podman): для каждого инструмента — args→правильный `ActionTarget` (argv/program/op + cwd_rel) · BadArgs (пустой argv/program, unknown-field) · форматирование Executed (exit/stdout/stderr/усечение/таймаут) · Rejected/HardBlocked→Ok-feedback. clippy 0, fmt + node-lints зелёные. Инертно (регистрация — 6c-2f; default-OFF `shell_enable`).

### Агент · SANDBOX-6c-2e-1 — `ProxyExecDispatcher` (in-sandbox клиент host/exec) + `ExecDispatcher`-шов

Пятый суб-срез исполнительной половины Фазы-3: IN-SANDBOX клиент, оркеструющий полный host/exec цикл `decide→execute→ЛОКАЛЬНОЕ исполнение→report`. Зеркало `ProxyActuator` для exec-таргетов. 3 exec-инструмента + регистрация при `shell_enable` — 6c-2e-2; `serve_host`-проводка — 6c-2f.

- **`nexus-core::sandbox::exec_proxy`**: `ExecDispatcher`-шов (`dispatch_exec(action) -> ExecToolOutcome`) — exec-инструменты (6c-2e-2) держат `Arc<dyn ExecDispatcher>` → Tier-1-мок без транспорта/раннера. `ProxyExecDispatcher<T: Transport>` (in-sandbox реализация): поверх act.sock-транспорта + `Arc<dyn ExecRunner>` шлёт **decide** (Rejected/HardBlocked → `ExecToolOutcome`, агент видит отказ не ошибку; Approved → одноразовый токен) → **execute** (redeem host-side → host-authority `WireExecGo`, argv НЕ переподаём) → **ЛОКАЛЬНО `ExecRunner::run`** (in-container, ЕДИНСТВЕННОЕ место Command — `exec_child`) → **report** (host финализирует ledger). `ExecToolOutcome::{Executed,Rejected,HardBlocked}`.
- **INV-CMD-SITE цел**: `exec_proxy.rs` НЕ конструирует Command (зовёт `ExecRunner`-трейт; реальный спавн — только в `exec_child.rs`) — `check-sandbox-exec.mjs` зелёный.

6 Tier-1 тестов (channel_pair + mock-host + MockExecRunner): full-cycle-executed (раннер получил host-authority argv; report донёс exit+token, без action) · rejected-no-run · hardblocked-no-run · vault-target→ToolError (не уходит на провод) · dead-transport→ToolError · wire-exec-kind. clippy 0, fmt + node-lints зелёные. Инертен (вызыватель — exec-инструменты 6c-2e-2).

### Агент · SANDBOX-6c-2d — `host/exec` фаза report (финализация ledger EXECUTING→EXECUTED|FAILED + приватность вывода)

Четвёртый суб-срез исполнительной половины Фазы-3: host-СТОРОНА фазы `report` — финализация ledger по исходу исполнения, с приватным (структурным) outcome. Замыкает 3-актный host-цикл `decide→execute→report`. Контейнерный исполнитель + ProxyExec-шим + `serve_host`-проводка — 6c-2e/2f.

- **`ExecBackend::report(exec_token, exit_code, stdout_tail, stderr_tail, undo_ref) -> WireExecResult`** (default `invalid_params`) + `HostExecServer` роутит фазу `Report` (кросс-фазовый fail-closed: `action`→отказ); все 3 фазы (decide/execute/report) теперь маршрутизируются.
- **`DispatchExecBackend::report`**: КОНСЬЮМИТ `in_flight[token]` (one-shot финализация; отсутствует → `invalid_params`: нет execute / двойной report) → ledger `audit::finish` `EXECUTING→EXECUTED` (exit==0) | `FAILED` (CAS `outcome IS NULL` — replay/гонка → ошибка).
- **ПРИВАТНОСТЬ (review hard-gate)**: в долговечный ledger outcome пишется ТОЛЬКО структурное резюме (`exec exit=N (stdout NB, stderr MB)`) — сырой stdout/stderr НЕ персистится (зеркало diff_summary; сырой вывод — в транзитное ExecResult-событие 6c-2g). `undo_ref` принимается на проводе, но НЕ персистится (→6c-2h GitOp pre-op-ref + exec-undo-handler атомарно).

27 Tier-1 тестов exec_host (+5: report-finalizes-**executed**/**failed** + ledger-state; **report-does-not-persist-raw-tails** (секрет в хвостах НЕ в ledger outcome, undo None); report-without-execute→err; report-replay→err one-shot). clippy 0, fmt + node-lints зелёные. Инертен для контейнера (ProxyExec — 6c-2e).

### Агент · SANDBOX-6c-2c — `host/exec` фаза execute (redeem одноразового токена → ledger EXECUTING + WireExecGo)

Третий суб-срез исполнительной половины Фазы-3: host-СТОРОНА фазы `execute` — redeem одноразового `exec_token` в host-authority сигнал «исполни», под write-before-act ledger-переходом + kill-switch. Контейнерный исполнитель + report-финализация — 6c-2d/2e.

- **`ExecBackend::execute(exec_token) -> Result<WireExecGo, RpcError>`** (default `invalid_params` → мок/6c-1 инертны) + `HostExecServer` роутит фазу `Execute` (кросс-фазовый fail-closed: execute несёт ТОЛЬКО `exec_token`, decide/report-поля → отказ); `Report` остаётся `invalid_params` (6c-2d).
- **`DispatchExecBackend::execute`** (security-критичный порядок): (0) anti-runaway кэп на `in_flight` (симметрично `pending` [`MAX_PENDING_EXEC`]); (1) **consume под локом** (`pending.remove`) — одноразовость by-construction (повтор/гонка → токен отсутствует → `invalid_params`; TOCTOU-замок: argv из СОХРАНЁННОГО действия, не из wire); (2) **kill-switch LAST-MOMENT re-check** — ПОСЛЕ consume, НЕПОСРЕДСТВЕННО перед write-before-act (зеркало `apply_now` «сужение TOCTOU»): под паузой НЕ пишем и ВОЗВРАЩАЕМ токен в `pending` (un-pause retry); (3) ledger CAS `APPROVED→EXECUTING` (`audit::transition` фенсит `state=approved AND outcome IS NULL`) — не promoted ⇒ ошибка (токен консьюмнут, fail-closed); (4) `in_flight`-store для report-финализации (6c-2d) + `build_exec_go` (6c-2b).
  - **Закалка по 2-линзовому adversarial-ревью (1 MAJOR закрыт):** kill-switch перенесён ДО→ПОСЛЕ consume (last-moment, зеркало `apply_now`): прежний под-локовый re-check оставлял await-окно (transition — DB-await), где флип паузы пропускал запуск EXECUTING — теперь окно сужено, токен возвращается; + `in_flight` anti-runaway кэп; + module/struct-doc и тест-имя обновлены (decide+execute роутятся).
- `is_paused()` → `pub(crate)` (единый источник семантики паузы для exec-redeem, не дубль чтения Arc).

22 Tier-1 тестов exec_host (+4: **redeem-approved-token** consume+EXECUTING+argv-host-authority / **unknown-token→err** / **replay→err** one-shot / **paused-refuses+keeps-token** + un-pause-retry). 6c-2c инертен для контейнера (ProxyExec — 6c-2e). clippy 0, fmt + node-lints зелёные.

### Агент · SANDBOX-6c-2b — host env-build (allow-list §5.4) + `WireExecGo`-builder + `propose_key` в ledger-цепочке

Второй суб-срез исполнительной половины Фазы-3: чистая host-side плита под redeem (6c-2c) — собирает окружение и сигнал «исполни» из СОХРАНЁННОГО действия, БЕЗ исполнения и без ledger-переходов. Плюс load-bearing изменение модели данных для будущего ledger-фенса.

- **`exec_host::build_exec_env`** (§5.4 fail-CLOSED): env exec-команды из ПУСТОГО + фикс. безопасный набор (`PATH`/`LANG` + `HOME=/tmp`) + явный per-skill `skill_passthrough`. **НИКОГДА не читает `std::env` хоста** (структурно fail-closed, не best-effort-скруб). **Denylist ЗАПРЕЩЁН** (fail-OPEN). Зарезервированные `PATH`/`LANG`/`HOME` skill-passthrough НЕ переопределяет (скилл не подменит PATH на writable-каталог). `skill_passthrough` дефолт пуст (SKILL.md→env_passthrough — отдельный срез).
- **`exec_host::build_exec_go`**: строит `WireExecGo` host-side из СОХРАНЁННОГО `Action` (argv — host-authority, контейнер не переподаёт → TOCTOU-замок). Exhaustive по 3 exec-таргетам (ShellRun→argv; ProcessSpawn→[program]+args; GitOp→["git", op]+args; vault-арм fail-closed пустой). cwd=`ScratchTmpfs{cwd_rel}` (VaultRo отложен — live 6c-3); env=allow-list; таймаут/кэп — дефолты.
- **`propose_key` в ledger-цепочке**: `ExecDecision::Approved` теперь несёт `{ledger_action_id, propose_key}` (идемпотентность-ключ ledger-строки из `dispatch_exec_decision` — ЕДИНЫЙ источник, не пересчёт во избежание дрейфа); `DispatchExecBackend` сохраняет его в `PendingExec` → redeem/finalize (6c-2c/2d) фенсят `approved→executing→executed|failed` по нему.

18 Tier-1 тестов exec_host (+7: env allowlist-only/HOME=scratch/passthrough/**reserved-not-overridable**; go argv-from-action/defaults/no-cwd; + approve-test пинит сохранённый propose_key). Инертно (build_* зовёт redeem 6c-2c). clippy 0, fmt + node-lints зелёные.

### Агент · SANDBOX-6c-2a — exec-исполнитель `exec_child` (in-container) + `ExecRunner`-шов + CI-линт INV-CMD-SITE

Первый суб-срез исполнительной половины Фазы-3 (`docs/specs/agent-sandbox.md §5.2`; план — мультиагент-Workflow «plan-sandbox-6c2», 8 суб-срезов). Реализует **ЕДИНСТВЕННОЕ место реального исполнения exec-команды агента** + структурно лочит инверсию «host РЕШАЕТ, контейнер ИСПОЛНЯЕТ». Инертен (вызывающих нет) до 6c-2e.

- **`nexus-core::sandbox::exec_child`** — `RealExecRunner` (ЕДИНСТВЕННАЯ во всём core/host конструкция `process::Command` для команды агента; бежит ВНУТРИ `--network=none` контейнера): `Command::new(argv[0]).args(argv[1..])` **БЕЗ шелла** (INV-NO-SHELL — метасимволы безвредны), `env_clear()` ВСЕГДА перед `envs(go.env)` (INV-ENV-FAILCLOSED — команда видит РОВНО host-собранный allow-list, не наследует даже окружение agentd), `current_dir` под scratch/vault-`:ro` (INV-CWD-CONFINE — резолв через `resolve_cwd` единым правилом `classify::path_confinement`, побег→команда не запускается), wall-clock `timeout`→kill+`timed_out`, output-cap потоковым кольцевым хвостом (`go.output_cap_bytes`, без безлимитного `read_to_end` — анти-OOM «болтливой»/джейлбрейк-команды; `*_truncated`-флаги). `ExecResult`. `ExecRunner`-трейт-шов (инструменты 6c-2e держат `Arc<dyn ExecRunner>`) + `MockExecRunner` (Tier-1-без-podman: security-ассерты на ЛЮБОМ хосте).
- **CI-линт `scripts/check-sandbox-exec.mjs`** (zero-dep, самотест фейк-нарушением) — **INV-CMD-SITE**: `process::Command`/`Command::new` в host-sandbox-модулях (exec_host/act/runner/child/event/provider/proxy/mod) → красный CI. WHITELIST: `exec_child.rs` целиком + marked podman-launch в `runner.rs` (маркер `sandbox-exec-lint: allow podman-launch`). Wired в `test-all.sh` + `ci.yml`. Лочит инверсию С ПЕРВОГО ДНЯ появления Command.
- `classify::path_confinement` → `pub(crate)` (единый источник правила конфайнмента, не копия); консты `CONTAINER_SCRATCH=/tmp` (переиспользует рендеримый `--tmpfs /tmp`, без хрупкого nested-mount) / `DEFAULT_EXEC_TIMEOUT_MS=120_000` / `DEFAULT_EXEC_OUTPUT_CAP=65_536`; tokio `+io-util` (capped-чтение).

13 Tier-1 тестов (resolve_cwd scratch/vault/empty/**reject-escape** · capped-tail last-bytes/under-cap/zero-cap · mock-capture; unix-gated на CI-хосте: trivial-argv · nonzero-exit · **env_clear-proven** (host-секрет НЕ утёк) · **timeout-kills** · missing-binary→127). Полный EROFS/ENETUNREACH/cap-deny — 6c-3 (podman-gated). clippy 0, fmt + node-lints (вкл. новый check-sandbox-exec) зелёные.

### Агент · SANDBOX-6c-1 — `host/exec` контракт + host-РЕШЕНИЕ по exec-таргетам (Tier-1, БЕЗ исполнения)

Первый срез завершающей Фазы-3 (`docs/specs/agent-sandbox.md §5.2`; дизайн — мультиагент-Workflow «design-sandbox-6c»: гибрид no-4th-socket + executor-rigor). Host-СТОРОНА `host/exec`: классификация + решение + ledger + минт токена. **БЕЗ единого исполнения** (контейнерный executor + runner-routing + ProxyExec — 6c-2; live на .28 — 6c-3).

- **Wire-контракт** (`nexus-core::sandbox::exec_host`): `host/exec` — ВТОРОЙ метод на act.sock (НЕ 4-й сокет). `WireExecAction` (exec-only, `TryFrom<&Action>` vault→Err — зеркало-противоположность `WireAction`; `WireExecKind` знает лишь 3 exec-вида → forge невозможен в обе стороны). 3-актный `WireExecRequest{phase: decide|execute|report}` (`deny_unknown_fields`) + `WireExecDecision` (Approved{exec_token,ledger_action_id}|Rejected|HardBlocked) + `WireExecGo`/`WireExecResult`/`ExecCwd` (контракт для 6c-2). `HostExecServer` маршрутизирует `decide`; `execute`/`report` зарезервированы → `invalid_params` (6c-2).
- **host-РЕШЕНИЕ** (`actuator::dispatch_exec_decision` + `ExecDecision`): зеркалит decision-часть `propose_and_decide` БЕЗ apply — `classify_exec` (НИКОГДА Auto) → HardBlocked / под Confirm спрашивает `DecisionSource` (PolicyDefault=DENY headless; ChannelDecision=человек) → на Approve ledger PROPOSED→APPROVED (write-before-act intent) + kill-switch re-check → возвращает `ledger_action_id`. **Vault-apply (`apply_action`/`apply_now`) НЕ зовётся** (exec там fail-closed РУБЕЖ-0). НЕТ UI-Proposal (ExecProposal-событие — 6c-2).
- **`DispatchExecBackend`** (real `ExecBackend`): держит per-run `GatedToolCtx` (единый policy-путь) + token-store. На Approve минтит ОДНОРАЗОВЫЙ `exec_token` = blake3(run_id|action_id|fingerprint|RANDOM-nonce) + СОХРАНЯЕТ действие (контейнер на execute предъявит ТОЛЬКО токен — argv не переподаёт → TOCTOU-замок approve-`ls`-run-`rm`; redeem/finalize — 6c-2).

**Закалка по 2-линзовому adversarial-ревью (0 CRITICAL/MAJOR):** `dispatch_exec_decision` получил РАНТАЙМ fail-closed-guard `!is_exec()→Rejected` (был лишь `debug_assert`, компилируемый прочь в release — паритет с РУБЕЖ-0 `apply_now`); `host/exec decide` отвергает кросс-фазовые поля (`exec_token`/`exit`/`tails` в decide-запросе → `invalid_params`); `exec_token` несёт 16-байт RANDOM-nonce (`getrandom`) — непрогнозируем БЕЗ опоры на секретность run_id/action_id; `pending`-store получил fail-closed soft-cap `MAX_PENDING_EXEC`=64 (anti-рост до 6c-2 redeem); 3 обязательных 6c-2-инварианта (redeem-консьюм токена / env-allowlist-only / `serve_exec` SO_PEERCRED) зафиксированы в module-doc.

11 Tier-1 тестов (wire round-trip / vault-not-on-host-exec / unknown-field / cross-phase-field-reject / decide-маппинг / execute-зарезервирован; + DispatchExecBackend на НАСТОЯЩЕМ vault+ledger: shell_enable=false→HardBlocked-без-токена / PolicyDefault→Rejected-без-токена / Approve→Approved+токен-сохранён / soft-cap→Rejected-без-добавления). 786 nexus-core тестов, clippy 0, fmt/egress/tooluse/dangling/ignored зелёные. Следом 6c-2: ProxyExec-шим + `exec_child` исполнитель ВНУТРИ контейнера + exec-инструменты + `serve_host` routing + adversarial-ревью.

### Агент · SANDBOX — `SO_PEERCRED` peer-uid гейт на per-run сокетах (закрывает §4.3.6 / §10.1 T8)

Спека `docs/specs/agent-sandbox.md §4.3` инвариант 6 / §10.1 (T8) требовали `SO_PEERCRED`/uid-check на per-run AF_UNIX-листенерах прогона (egress/act/event, и будущий exec), чтобы egress/* и host/* RPC мог драйвить **ТОЛЬКО** спавненный контейнер (его run_as-uid). Инвариант был СПЕЦИФИЦИРОВАН, но НЕ реализован — защита держалась лишь на 0600-правах сокета + 0700 per-run каталога. Этот срез закрывает разрыв spec↔код.

- **`peer_uid(stream) -> Option<u32>`** (`agent/connect/afunix.rs`, `pub(crate)`) — читает uid пира соединённого AF_UNIX-сокета через `getsockopt(SO_PEERCRED)` (Linux). Это **ядро-достоверный** credential: клиент не подделает его (в отличие от прикладных полей RPC). **Fail-closed:** сбой syscall / усечённый `ucred` → `None`. Под `#[cfg(target_os = "linux")]`; на иных ОС — стаб → `None` (sandbox — Linux-host-only, §9). Дека `libc` ТОЛЬКО под Linux (уже транзитивно в Cargo.lock — в дерево ничего не приходит).
- **Гейт в accept-LOOP раннера** (`sandbox/runner.rs run()`): на КАЖДОМ из egress/act/event-сокетов соединение обслуживается, ТОЛЬКО если `peer_authorized(&stream, expected_uid)` — иначе дропается с `warn` И **слушаем дальше** (accept-loop, не одиночный accept: отвергнутый импостор НЕ лишает легитимный контейнер сокета; после обслуживания валидного пира — выход). `expected_uid` = host-видимый uid контейнера; при rootless-Podman + `--userns=keep-id` процесс контейнера виден хост-ядру под host-uid. Defense-in-depth ПОВЕРХ 0600/0700. **Тем же гейтом помечен к обёртыванию будущий `serve_exec` (exec.sock).**
- **Fail-closed-матрица** (`uid_matches`): авторизуем ТОЛЬКО при достоверном равенстве; неизвестный ожидаемый ИЛИ нечитаемый peer-cred → отказ.

**Adversarial-ревью (скептик-агент, security-линза) → 3 фикса в срезе:** (1) **`expected_uid` — единый источник истины с рендером `--user`**: дерим строго из `config.run_as` (тот же, что рендерит план), БЕЗ тихого фолбэка на дир-владельца при непарсящемся uid — иначе мисконфиг `run_as` гейтил бы против ДРУГОГО uid, чем реально в `--user` (теперь нечисловой → `None` → fail-closed, не «угадал чужой»); (2) **accept-loop вместо одиночного accept** (отвергнутый по peer-uid не вызывает self-DoS легитимного прогона); (3) **раздельный лог** «cred НЕЧИТАЕМ» (в `peer_uid`) vs «uid НЕ СОВПАЛ» (call-site) для аудит-следа. Трекнутый хвост (контрол-сокет коннектора `serve_unix` — второй 0600-листенер, который T8 тоже называет; иной транспорт/семантика — uid оператора, не run_as) **закрыт отдельным follow-up-срезом** (см. ниже).

**Тесты:** `uid_matches_is_fail_closed` (Tier-1, любая ОС — матрица отказа); `peer_authorized_accepts_same_uid_rejects_mismatch` (Tier-1, Linux — РЕАЛЬНАЯ пара `UnixListener`↔`UnixStream`: same-uid авторизован, заведомо-чужой `expected` и `None` отвергнуты). Кросс-uid РЕАЛЬНЫМ вторым пользователем — Tier-2 (нужны привилегии, §8.2 podman-gated). FFI-байндинг сверен с `libc` 0.2.186 (ucred/`SO_PEERCRED`=17/`SOL_SOCKET`=1 на arch/generic). clippy 0, fmt зелёный, workspace-тесты зелёные (nexus-core 775, agentd 7, cli 26, desktop 179) на macOS (Linux-путь — ubuntu-CI + Tier-2 .28).

### Агент · AGENT-CONNECT — `SO_PEERCRED` peer-uid гейт на контрол-сокете коннектора (закрывает хвост T8 / MAJOR-2 ревью #398)

ВТОРОЙ 0600 AF_UNIX-листенер, названный T8 (`agent-sandbox.md §10.1` / `agent-connect.md §6`): контрол-сокет коннектора `serve_unix`/`serve_unix_at` (`agent/connect/afunix.rs`), которым `nexus-agentd` хостит `ConnectAgentHandler` при `NEXUS_AGENTD_CONNECT_SOCKET`. До сих пор гейтился ТОЛЬКО 0600-правами — без ядро-достоверного peer-uid-чека. Трекнутый follow-up из adversarial-ревью PR #398 (MAJOR-2).

- **Ожидаемый peer = ОПЕРАТОР, не контейнер.** В отличие от per-run sandbox-сокетов (peer = run_as-uid контейнера, `sandbox/runner.rs::peer_authorized`), контрол-сокет драйвит сам оператор → ожидаемый uid = uid процесса `agentd`. Новый **`operator_uid() -> Option<u32>`** (`afunix.rs`, `pub`): Linux → `Some(getuid())`, не-Linux → `None`. `getuid()` инфаллибелен (POSIX).
- **Гейт в accept-loop `serve_unix`** (`connector_peer_authorized`): **Linux** — fail-closed ПОВЕРХ 0600 (переиспользует `peer_uid()` из предыдущего среза), пускаем ТОЛЬКО при достоверном равенстве `peer_uid == expected`; нечитаемый cred / mismatch / неизвестный ожидаемый → дроп + `warn` И слушаем дальше (импостор не лишает оператора сервиса — как accept-loop sandbox-раннера; `stream` дропается выходом из scope → FIN пиру). **Не-Linux → perms-only fallback** (accept): `SO_PEERCRED` там нет, а контрол-сокет КРОСС-ПЛАТФОРМЕННЫЙ (`#[cfg(unix)]`, dev/CI на macOS, E2E-тест `serve_unix_drives_run_over_socket`) — strict-fail-closed оборвал бы коннектор на macOS. Sandbox так НЕ делает (Linux-host-only, §9).
- **Сигнатура `serve_unix`/`serve_unix_at` + `expected_uid: Option<u32>`** — протянута до вызывающих: `nexus-agentd` (`main.rs`) передаёт `operator_uid()`; E2E-тест `serve_unix_drives_run_over_socket` (`handler.rs`) — тоже (клиент = наш uid → пропускается, доказывает что гейт не ломает легитимного оператора).

**Тесты:** `connector_peer_authorized_accepts_same_uid_rejects_mismatch` (Tier-1, Linux — РЕАЛЬНАЯ пара `UnixListener`↔`UnixStream`: `operator_uid()==getuid()`, same-uid авторизован, заведомо-чужой `expected` и `None` отвергнуты, аналог sandbox-теста); E2E по реальному сокету остаётся зелёным. Кросс-uid вторым пользователем — Tier-2 (привилегии). clippy 0, fmt зелёный, workspace-тесты зелёные на macOS (Linux-путь — ubuntu-CI).

### Агент · SANDBOX-6b — Фаза-3 exec-таргеты: `ShellRun`/`ProcessSpawn`/`GitOp` + classify never-Auto

Расширение типизированной алгебры действий (`docs/specs/agent-sandbox.md §5.1/§5.3`, дизайн — мультиагентный Workflow «design-sandbox-6b»: гибрид Variant-A footprint + Variant-B chokepoints). 3 НОВЫХ `ActionTarget`: `ShellRun{argv,cwd_rel}`, `ProcessSpawn{program,args,cwd_rel}`, `GitOp{op,args}`. **БЕЗ нового рантайм-действия** (исполнение — host/exec, 6c): только типы + classify + все exhaustive-армы fail-closed + инвариант-тесты.

**classify (§5.3, НИКОГДА Auto):** `ClassifyCtx` + `shell_enable`/`sandbox_available` (предвычислено корнем, classify остаётся чистой). `classify_exec`: precedence — `shell_enable=false` → `HardBlocked(ShellDisabled)`; иначе `sandbox_available=false` → `HardBlocked(SandboxUnavailable)` (block by-construction §9, не-Linux/sandbox-off); иначе → `Confirm(ExecRequiresApproval)`. exec-ячейки Auto СТРУКТУРНО нет. `ConfirmReason::ExecRequiresApproval` (unit).

**Два chokepoint'а fail-closed (не per-arm, а by-construction):** (1) `WireAction::From`→`TryFrom`: exec-таргет → `Err` (`host/act` несёт ТОЛЬКО vault; `WireKind` знает лишь 3 vault-вида → контейнер СТРУКТУРНО не протолкнёт exec через host/act); (2) `apply_action` top-guard `is_exec → Failed` (apply — единственный путь к диску; делает exec-армы в WRITE/success_summary ПРОВАБЛИ-МЁРТВЫМИ `unreachable!`, а не молчаливой псевдо-записью). `apply_now` + `proposed_content`/`file_status`/`change_kind`/`proposal_key`/`action_payload` — инертные exec-армы. `DispatchPolicy` + `with_exec_flags` (default false); `dispatch_action` пропускает read-vault для exec + питает ClassifyCtx из политики. agentd `--sandbox-run` проводит флаги (`shell_enable` из конфига; sandbox_available=true).

**Тесты:** `exec_targets_never_auto` (KEYSTONE — вся сетка ×3 варианта) + precedence/sandbox-unavailable/confirm + `exec_action_not_representable_on_host_act` + `adding_variant_breaks_match` расширен до 6 арм (canary). `is_exec` (action.rs).

**Мультиагент-ревью (2 линзы: never-Auto-инвариант / correctness, 0 CRITICAL/MAJOR после перепроверки):** обе линзы поймали ОДНУ реальную (MINOR) дыру — комменты ссылались на тест `exec_apply_is_fail_closed`, которого НЕ БЫЛО (apply-level RUBEZH-0 chokepoint был покрыт только на classify/wire-уровне). Исправлено: добавлен `exec_apply_is_fail_closed` — `apply_action` для всех 3 exec-ctor → `Failed`, БЕЗ файла/ledger-строки (пинит top-guard, от которого зависят `unreachable!()`-армы). 2 NIT приняты как есть (sandbox_available=true корректно ВНУТРИ host-раннера; `GitOp{op,args}` — намеренное обогащение спека-скетча `GitOp{op}`). 774 nexus-core теста зелёные, clippy 0, fmt/egress/tooluse/dangling/ignored зелёные. Следом 6c: `host/exec` RPC + исполнение ВНУТРИ песочницы (Tier-2).

### Агент · SANDBOX-6a — фундамент Фазы-3: env-scrub fail-closed + `ai.shell_enable` (default-OFF)

ПЕРВЫЙ срез Фазы-3 host-actuator (owner-greenlit, `docs/specs/agent-sandbox.md §5/§11`) — БЕЗ единого нового опасного действия (exec-таргеты вводит 6b, исполнение — 6c). Только фундамент-вокабуляр, всё SAFE-by-default:

- **`ai.shell_enable: bool`** (default false) — гейт исполнения host exec-таргетов. `false` → exec-таргеты будут `HardBlocked(ShellDisabled)`; `true` (+ `sandbox_enabled` + Linux) → `Confirm` (НИКОГДА `Auto`). На этом срезе ещё не рождает exec-таргеты — декларирован + питает classify/env-scrub 6b/6c.
- **`BlockReason::ShellDisabled` / `SandboxUnavailable`** + фенсенные сообщения (`block_message`) — вокабуляр для 6b: shell выключен / песочница недоступна структурно (не-Linux / sandbox-off → block by-construction §9). Никаких других exhaustive-match по `BlockReason` не сломалось (только `block_message`, арм добавлены).
- **env-scrub fail-closed (§5.4):** `SandboxConfig.env_allowlist: Vec<(K,V)>` → рендер `--env K=V`. **ДЕФОЛТ ПУСТ → ни одной `--env`** (host-окружение НЕ пробрасывается ни на одном срезе — Фаза-2 байт-в-байт прежняя). Это НЕ denylist: контейнер увидит РОВНО allow-list (Фаза-3 наполнит минимумом PATH + per-skill `env_passthrough`).

Render-тесты: пустой allowlist→нет `--env` (инвариант Фазы-2 сохранён) / allowlist→ровно `--env K=V` ДО образа; block_message покрывает новые reasons. clippy 0, fmt/egress/dangling/ignored зелёные, 56 sandbox-тестов. Следом 6b: ActionTarget `ShellRun`/`ProcessSpawn`/`GitOp` + classify (Confirm/HardBlocked, never Auto) + `exec_targets_never_auto`-инвариант.

### Агент · SANDBOX-5b — LIVE Tier-2 валидация каркаса песочницы на Podman .28 ✅ + fix `--user`

**Каркас Фазы-2 РАБОТАЕТ end-to-end против реального Qwen** (live на .28, podman 5.7.0). `nexus-agentd --sandbox-run` на тест-vault: хардненный контейнер (`--network=none`) → egress.sock→GuardedProxy→GuardedClient→Qwen:8080 (агент получил контекст, стримил) → **Qwen вызвал `note.create`** (Notes/Hello.md) → act.sock→HostActServer→`dispatch_action`→auto-тир **ПРИМЕНИЛ** (toolResult «создана заметка», isError=false) → event.sock релеил toolCall/toolResult/final → `Final`, exit 0 → **файл реально записан в host-vault** с точным содержимым. Полный путь (podman+3 сокета+LLM+гейт+vault-запись) подтверждён вживую.

**Live-bug пойман и исправлен** (`--user`): образ имеет `USER nexus` (uid 10001); при `--userns=keep-id` процесс контейнера (10001) НЕ открывал host-owned 0600-сокеты/`:ro`-vault → `connect egress.sock: Permission denied (EACCES)`. Фикс: `SandboxConfig.run_as` (Option, None по умолчанию — Tier-1 render-тесты не зависят от uid хоста) → рендер `--user <uid>:<gid>` ДО образа; `SandboxRunner` берёт uid:gid из metadata только что созданного `host_run_dir` (без libc/getuid) → контейнер бежит под host-uid → владеет сокетами + читает vault (бинарь world-rx, /tmp tmpfs rw). Render-тест `run_as_renders_user_before_image`. 54 sandbox-теста. **SANDBOX-1..5 ЗАВЕРШЕНЫ — каркас Фазы-2 собран И live-валидирован.** Следом: Фаза-3 host-actuator (6a-c, owner-greenlit) + прод-проводка коннектора (`ai.sandbox_enabled` маршрутизирует прогоны через песочницу).

### Агент · SANDBOX-5a — agentd `--sandbox-run`: host-харнесс one-shot песочного прогона

Прод-путь ВЫЗОВА `SandboxRunner` (его до сих пор никто не звал — `ai.sandbox_enabled` был инертен). `nexus-agentd --sandbox-run <vault> <task>` — композиционный корень host-стороны: открывает vault-БД, собирает egress-границу (policy+audit+allowlist из `.nexus/local.json`), РЕАЛЬНЫЕ backend'ы — `GuardedProxy` поверх `GuardedClient::for_chat` (egress.sock, chokepoint цел), `HostActServer` поверх `DispatchActuatorBackend(GatedToolCtx)` (act.sock, auto-тир + PolicyDefault + TracingEventSink), event-лог-транспорт (события агента в `tracing`) — и гонит ОДНУ задачу в хардненном контейнере через `SandboxRunner::run`. `run_id` — реальная строка `agent_runs` (ledger-корреляция). Unix-only (`#[cfg(unix)]`), default-OFF (только по флагу). Это ТОТ ЖЕ композиционный корень, что позже подключит коннектор при `ai.sandbox_enabled`; сейчас — для **live Tier-2 валидации каркаса на Podman .28** (образ `nexus-agentd:local`). clippy 0, fmt/egress/tooluse зелёные; CI гейтит компиляцию (полный путь — live, нужен Podman).

### Агент · SANDBOX-4b-2b-2 — host `SandboxRunner` + agentd `--sandbox-child` (завершение каркаса Фазы-2)

ЗАМЫКАЕТ рантайм песочницы (`docs/specs/agent-sandbox.md §2/§5`): host-оркестратор + in-container точка входа собраны в работающий путь.

**Host `SandboxRunner`** (`nexus-core::sandbox::runner`) — зеркало `run_sandbox_child_session` на хосте: биндит 3 AF_UNIX-сокета в per-run каталоге (`host_run_dir`, НЕ под `:ro`-vault), спавнит хардненный `podman run` (`sandbox_run_plan_with_cmd` + `--sandbox-child …`) и обслуживает каждый сокет РЕАЛЬНЫМ backend'ом: egress.sock → `GuardedProxy` (→ `GuardedClient`, единственный сетевой путь), act.sock → `HostActServer` (→ `dispatch_action`, authoritative-гейт host-side), event.sock → `EventForwardServer` (релей в коннектор/десктоп). Контейнер (`--network=none`) коннектится клиентом; host держит authoritative-решения. Lifecycle: `run()` ждёт выхода контейнера; отмена — `podman kill <container_name>` (проводка к `agent/cancel` — последующий срез). serve-хелперы (`serve_egress`/`serve_act`) — Tier-1 (через `ChannelTransport`); полный `run()` — Tier-2 (Podman + образ, live на .28).

**agentd `--sandbox-child`** (`nexus-agentd::main`) — перехват ДО `run()`: argv `<run_id> <base_url> <model> <ctx_window> <task>` (позиционно, ARGV не шелл → task с спецсимволами безопасен), коннект к 3 сокетам по ФИКСИРОВАННЫМ путям (`/run/nexus/{egress,act,event}.sock`), `run_sandbox_child_session` → код выхода (0=Final, 1=иначе). agentd остаётся serde_json-free (композиция в nexus-core).

Инфраструктура: `sandbox_run_plan_with_cmd` (CMD после образа = аргументы ENTRYPOINT `nexus-agentd`; ENV хоста по-прежнему НЕ пробрасывается); консты имён сокетов `SOCKET_{EGRESS,ACT,EVENT}` (единый источник host↔контейнер); nexus-core tokio `process`+`rt` (спавн podman + serve-таски). Раннер + `--sandbox-child` — Unix-only (`#[cfg(unix)]`, как `connect::afunix`; rootless-podman — Linux-host фича).

**Мультиагент-ревью (2 линзы: security+isolation / correctness+lifecycle): 1 MAJOR исправлен.** MAJOR — сокеты биндились сырым `UnixListener::bind` БЕЗ 0600-хардненинга (нарушение спеки §4.2/§4.3: per-run сокеты owner-only; egress.sock = guarded-эгресс, act.sock = host-гейт записи в vault — единственный bind-сайт, уронивший защиту относительно `serve_unix_at`). Фикс: `harden_socket_perms`/`prepare_socket_path` подняты до `pub(crate)`, раннер их ПЕРЕИСПОЛЬЗУЕТ (`bind_hardened`: prepare→bind→0600); + каталог сокетов 0700; + `prepare_socket_path` отказывается тереть НЕ-сокет (не чужой файл); + cleanup при частичном сбое bind; + serve-таски ДОТЕКАЮТ (bounded await вместо мгновенного abort — event-релей не теряет хвост). 2 Tier-1 регресс-теста (сокет 0600, отказ на не-сокете). Прочие находки перепроверкой → NIT (argv-инъекции нет — argv не шелл, CMD строго после образа; single-accept верен по контракту child; exit-code Paused/Cancelled невозможен в контейнере — статус решает host по событиям). clippy 0, egress-chokepoint цел.

3 Tier-1 теста serve-glue + 2 хардненинга. fmt/egress/tooluse/dangling/ignored зелёные. **Каркас Фазы-2 (SANDBOX-1..4b) собран end-to-end**; остаётся podman-gated Tier-2 live-валидация на .28 (SANDBOX-5) + Фаза-3 host-actuator (6a-c, owner-greenlit).

### Агент · SANDBOX-4b-2b-1 — `run_sandbox_child_session`: драйвер in-container loop'а (Tier-1)

Composition-root песочницы (`docs/specs/agent-sandbox.md §2`): in-container прогон (`nexus-agentd --sandbox-child` в 4b-2b-2) НЕ держит коннектора и НЕ строит host-side гейт — он крутит `run_agent_loop` поверх ТРЁХ прокси, замкнутых на host через AF_UNIX (`--network=none`): провайдер `ProxyToolProvider` (egress.sock → host `GuardedProxy` → `GuardedClient`, chokepoint цел), актуатор `ProxyActuator` как `Arc<dyn ActionDispatcher>` (act.sock → host `dispatch_action`, authoritative — ШОВ 4b-2a), форвардер `ProxyEventForwarder`+`drain_events` (event.sock → host релей в десктоп). Реестр — те же файловые инструменты, что in-process (после 4b-2a транспорт-агностичны), но диспетчер — `ProxyActuator`. Lifecycle host-side (НЕТ control-сокета в контейнер): in-container `cancel`/`paused` — локальные `AtomicBool(false)`, взводит их только хост (podman pause/kill). Контекст пока минимальный (преамбула + задача); recall/скиллы/web-в-песочнице — последующие срезы.

**СКВОЗНОЙ Tier-1 тест** (всё на `ChannelTransport`, без podman): mock-LLM (1-й POST → tool_call `note.create`, 2-й → Final) + capture-актуатор за `HostActServer` + сбор `agent/event` → `run_sandbox_child_session` сводит провайдер/актуатор/форвардер в РАБОЧИЙ tool-loop: модель зовёт `note.create` → `ProxyActuator` → act.sock → host применяет → Final("готово"); проверено, что действие применено с верным путём (`Notes/Sbx.md`), модель вызвана дважды, события (toolCall/final) доехали до event.sock.

**Мультиагент-ревью (2 линзы: concurrency+lifecycle / chokepoint+correctness): 0 CRITICAL/MAJOR** (2 MAJOR-тревоги перепроверкой понижены до NIT — kill-switch остаётся корректным: host `dispatch_action` перечитывает `agent_paused` per-step → запись fail-safe под паузой даже без знания контейнера; утечки drain-таска нет). Применены NIT/MINOR (no-tails): задокументирована дивергенция от спеки §6 (control-сокет сознательно опущен, in-container `cancel`/`paused` — плейсхолдеры под host-kill-семантику); `bounds`-док (host podman — авторитетный wall-clock); `drain.await` логирует JoinError при панике (drain структурно инфаллибелен → паника = регрессия); тест укреплён (прямой `await` host-тасков вместо timeout-обёртки — убрано окно недетерминизма); **+ негативный тест**: мёртвый egress.sock мид-прогон → `LoopOutcome::Error` за конечное время (graceful, не виснет). clippy 0, fmt/egress/tooluse/dangling зелёные. Следом 4b-2b-2: host `SandboxRunner` (podman + bind 3 AF_UNIX) + agentd `--sandbox-child` CLI → Tier-2 live на Podman .28.

### Агент · SANDBOX-4b-2a — ШОВ `ActionDispatcher`: транспорт-агностичные актуатор-инструменты (Tier-1)

Подготовка in-sandbox реестра (`docs/specs/agent-sandbox.md §2/§5`): файловые инструменты (`note.create`/`note.edit`/`note.set_frontmatter`) отвязаны от транспорта применения. Новый трейт `actuator::ActionDispatcher` (`apply(Action) -> Result<String, ToolError>`) с двумя реализациями: **in-process** `GatedToolCtx` (→ host-side `dispatch_action` напрямую) и **in-sandbox** `sandbox::act::ProxyActuator` (→ `host/act` RPC, который на хосте применяет тем же `dispatch_action`). Инструмент теперь держит `Arc<dyn ActionDispatcher>` и НЕ знает транспорт — реестр ОДИН, выбор делает композиционный корень (`run_agent_session` → `GatedToolCtx`; будущий `--sandbox-child` → `ProxyActuator`).

**Инвариант «нет ungated-пути» СОХРАНЁН** (3e hard-gate #1): шов не вводит обхода — ОБЕ реализации сводятся к ОДНОМУ host-side `dispatch_action` (classify/RiskTier×autonomy/decision/ledger/undo), песочница лишь меняет МЕСТО вызова инструмента (контейнер), authoritative-применение остаётся host-side. Чисто рефакторинг — поведение in-process пути байт-в-байт прежнее (`run_agent_session` оборачивает `GatedToolCtx` в `Arc<dyn ActionDispatcher>`). Затронуты только `actuator/tools.rs` + `agent/session.rs` (единственные сайты построения инструментов) + `sandbox/act.rs` (impl для `ProxyActuator`).

Новый Tier-1 тест ПОЛНОЙ ЦЕПИ песочного актуатора через `Tool`-трейт: in-sandbox `NoteCreateTool`(`Arc<ProxyActuator>`) → `invoke` → `host/act` → `HostActServer` → `DispatchActuatorBackend` → `dispatch_action` → запись на диск (тот же `NoteCreateTool`, что in-process — инструмент транспорт-агностичен). 8 actuator-tools тестов (обновлены под `Arc<dyn ActionDispatcher>`) + 46 sandbox-тестов. clippy 0, fmt/egress/tooluse зелёные. Следом 4b-2b: host `SandboxRunner` (podman + 3 сокета) + agentd `--sandbox-child`.

### Агент · SANDBOX-4b-1 — реальный act-backend + OUTWARD-форвардер событий (Tier-1)

Два host-side примитива рантайма песочницы (`docs/specs/agent-sandbox.md §2/§5`), оба Tier-1-тестируемы без Podman:

**1. `DispatchActuatorBackend`** (`nexus-core::sandbox::act`) — РЕАЛЬНЫЙ `ActuatorBackend` (host-сторона `host/act`): держит per-run `GatedToolCtx` (ВСЕ deps `dispatch_action` — canon_root/ledger/run_id/policy/decision_source/events) и применяет действие через НЕИЗМЕНЁННЫЙ `dispatch_action`. **Ключевой инвариант — нет второго policy-пути**: `GatedToolCtx` РОВНО тот же, что несут in-process актуатор-инструменты, поэтому classify/RiskTier×autonomy/TokenBucket/kill-switch/ledger/undo/blast-radius у песочного прогона ИДЕНТИЧНЫ in-process пути (песочница лишь добавляет OS-изоляцию вокруг loop'а; authoritative-решение — в ОДНОМ `dispatch_action`). Закрывает мок-заглушку `ActuatorBackend` из SANDBOX-3. 3 Tier-1 end-to-end теста на НАСТОЯЩЕМ vault+БД: auto-политика → `note.create` пишется на диск; полный host-путь `WireAction→HostActServer→DispatchActuatorBackend→dispatch_action→диск`; confirm+`PolicyDefault` → НЕ пишет (kill-path: контейнер не форсирует запись, host решает).

**2. event-форвардер** (`nexus-core::sandbox::event`) — OUTWARD-поток событий хода из контейнера наружу. In-sandbox `ProxyEventForwarder` (impl `AgentEventForwarder`) маппит `AgentEvent` → `agent/event`-нотификацию через `event_notification` (ТОТ ЖЕ wire-контракт, что у коннектора) → `try_send` в ОГРАНИЧЕННЫЙ канал (анти-leak, НЕ блокирует loop) → `drain_events` шлёт в event.sock → host `EventForwardServer` РЕЛЕИТ `agent/event` вербатим в исходящий транспорт коннектора (десктоп). **Критично — маппинг через `event_notification`, а НЕ `to_value(AgentEvent)`**: `AgentEvent` — `#[serde(tag="type")]` с newtype-вариантами (`AssistantToken(String)`/`Final(String)`), которые НЕсовместимы с serde-internal-tag → наивная сериализация ТЕРЯЕТ их (поймано тестом: 1 из 3 событий доходило). Маппинг идёт через struct-вариантный wire-DTO `AgentStreamEvent` (единый источник). События — ТОЛЬКО для отображения (authoritative-решения от них не зависят: Proposal/Diff порождает host-side `dispatch_action`, Approve валидируется host по реальному ledger); host релеит лишь метод `agent/event` (чужие сообщения на event.sock игнорируются — контейнер не диктует host'у иное). 4 теста: newtype-вариант форвардится (регресс), переполнение канала без паники, сквозной sandbox→десктоп (3 события вкл. newtype в порядке), чужой метод не релеится.

**Мультиагент-ревью (2 линзы: correctness+concurrency / security+инварианты): 0 CRITICAL/MAJOR** — подтверждены оба ключевых инварианта (нет второго policy-пути: `DispatchActuatorBackend` прокидывает РОВНО те 7 аргументов `dispatch_action`, что in-process `dispatch_via_gate`; event.sock не пробивает write-инвариант: форгнутый Proposal — display-only, Approve валидируется host по ledger). Применены 3 MINOR + NIT (no-tails): **(C1)** маппинг в wire перенесён из `forward` в `drain_events` (канал держит сырой `AgentEvent`) — serde убран с горячего пути loop'а, вытесняемые при переполнении события не сериализуются, и дока «зеркалит TransportForwarder» стала буквально верной; **(S1)** host `serve` теперь РЕ-ВАЛИДИРУЕТ `params` в `AgentStreamEvent` перед релеем (контейнер недоверенный — приводим форму на проводе к десктопу к классу in-process пути, закрывает cosmetic-spoof changeset-ленты); **(C2)** `EVENT_CHANNEL_CAP` вынесен в единый `connect::EVENT_CHANNEL_CAP` (был дубль event.rs/handler.rs → дрейф backpressure); + `tracing::debug!` на дроп чужих/кривых сообщений и обрыв drain (наблюдаемость adversarial-поведения); + 2 теста (FIFO-вытеснение хвоста при переполнении; host дропает невалидный `AgentStreamEvent`).

clippy 0, fmt/egress/tooluse/dangling/ignored — зелёные; 45 sandbox-тестов + 35 connect-тестов. Следом 4b-2: `SandboxRunner` (JobHandler: спавн podman + 3 сокета egress/act/event + эти backend'ы) + agentd `--sandbox-child` mode → Tier-2 live на Podman .28.

### Агент · SANDBOX-4a — `ProxyToolProvider`: in-sandbox tool-провайдер через GuardedProxy (Tier-1)

Провайдер для прогона ВНУТРИ песочницы (`docs/specs/agent-sandbox.md §2/§4`). Контейнер `--network=none` → нельзя `reqwest` к LLM; `nexus-core::sandbox::provider::ProxyToolProvider` (impl `ToolCapableProvider`) шлёт тот же OpenAI-запрос, но **`stream:false`** (буферизованный единый JSON), через `ProxyGuardedClient` (SANDBOX-2) поверх AF_UNIX → host `GuardedProxy` ре-эмитит через настоящий `GuardedClient` (chokepoint цел). Парс не-стрим `choices[0].message.{content,tool_calls}` → `ToolTurn`. **Дизайн-девиация от плана** (там был `ChatEgress`-трейт с рефактором горячего стрим-пути): выбран ОТДЕЛЬНЫЙ буферизованный провайдер, чтобы НЕ дестабилизировать прод-стрим chat-путь — host `OpenAiToolProvider` не тронут. `tool_spec_to_json` → `pub(crate)` (переиспользование схемы tools без дублирования). Буфер-путь: токены не инкрементальны (эмитятся разом) — приемлемо для каркаса. **Мультиагент-ревью (parse+design, 0 CRITICAL; 1 MAJOR+2 MINOR закрыты)**: `parse_completion` приведён к КОНТРАКТУ стрим-провайдера (`finalize`) — ловит HTTP-200 `{error:{...}}` (llama.cpp/vLLM так возвращают context-overflow/MTP-баг → раньше молча Final("")) → `BadResponse`; валидирует JSON-аргументы tool_call (раньше кривые args долетали до инструмента вместо одного чистого re-ask); пустое имя → BadResponse, пустой id → синтез `call_{i}`, object-args → сериализация. 12 Tier-1 тестов (Final/ToolCalls/multiple/error-object/no-choices/invalid-args/missing-name/synth-id+object-args/empty-body/cancel). Дизайн-девиация (отдельный провайдер vs ChatEgress-трейт) признана ревью «строго безопаснее» (горячий стрим-путь не тронут). Гейт зелёный, clippy 0, egress-chokepoint цел. Следом 4b: `SandboxRunner` + agentd `--sandbox-child` + 3-сокетный рантайм (Tier-2 на Podman .28).

### Агент · SANDBOX-3 — host/act RPC: vault-запись песочницы через host-side gate (Tier-1)

Третий срез Фазы-2 каркаса (`docs/specs/agent-sandbox.md §5.1`; архитектура — мультиагентный design-Workflow «design-sandbox-runtime»: тонкий in-container loop, host-authoritative gate, 3 сокета). Модуль `nexus-core::sandbox::act`: vault в контейнере `:ro` → in-sandbox актуатор-инструменты пишут НЕ локально, а через typed JSON-RPC **`host/act`** → ХОСТ исполняет НЕИЗМЕНЁННЫЙ `dispatch_action` (classify/RiskTier/decision/ledger/undo — authoritative). Зеркалит SANDBOX-2: `HostActServer` (host) + `ProxyActuator` (in-sandbox шим) + `ActuatorBackend`-трейт (Tier-1-тестируемо без vault/гейта; реальный backend с per-run контекстом — SANDBOX-4). `Action`/`ActionTarget` НЕ сериализуются (security-keystone) — wire-DTO `WireAction`{kind,rel,key?,content?,value?} (`deny_unknown_fields`) + EXHAUSTIVE fail-closed конверсия `From<&Action>` (Phase-3 ShellRun/… сломает компиляцию → осознанное решение, представим ли таргет в host/act). `DispatchOutcome`↔`WireDispatchOutcome` 1:1; tool-граница сохранена (Failed→Err). Шим id-матчит ответ (как ProxyGuardedClient). 8 Tier-1 тестов (round-trip/unknown-field-reject/frontmatter-key/outcome-mapping/proxy↔server/Failed-fold). **Мультиагент-ревью: 0 дефектов** (exhaustive-keystone цел, gate не обходится — SANDBOX-3 без fs-ops, ошибки не текут — vault-rel пути не секрет). Гейт зелёный, clippy 0, egress-chokepoint цел.

### Агент · WEB-FETCH-PUBLIC — произвольный публичный `web.fetch` (default-OFF, guarded)

`web.fetch` мог тянуть только allowlist-хосты (SearXNG). Owner-gated 2026-06-22 (decision #4): новый флаг политики `EgressPolicy.web_allow_public` (`AtomicBool`, default false) + конфиг `ai.web.allow_public_fetch` (default false) → `enable_web_tools` → `set_web_allow_public`. Когда ВКЛ, фича `Web` допускает ЛЮБОЙ ПУБЛИЧНЫЙ хост без allowlist (для deep-research). **ВСЕ остальные рубежи сохранены**: cloud-metadata-блок (шаг 1), офлайн-kill (шаг 2), per-feature opt-in (шаг 3), `deny_private` (шаг 4а — приватные/LAN режутся) + DNS-rebind/SSRF-гард на РЕЗОЛВНУТЫХ IP в `authorize` (публичное имя → приватный IP всё равно режется, т.к. `Web.denies_private()` неизменно true) + redirect=none + IP-пин + durable audit. Касается **ТОЛЬКО `Web`** (`matches!(feature, Web)`), не `NewsFeed`/Chat/Embed. Существующие деплои без флага — байт-в-байт прежние (allowlist-only). Тесты: публичный-разрешён / приватный+metadata+offline-режутся / NewsFeed-не-расширен / default-off. **Мультиагент-ревью (SSRF+correctness): 0 CRITICAL/MAJOR.** По находке-MINOR добавлен регресс-замок: full-`authorize()` тест — публичное имя, резолвящееся в приватный IP (DNS-rebind), режется ДАЖЕ при `web_allow_public` (пинит, что снятие string-allowlist не снимает резолв-гард; упадёт, если `Web.denies_private()` когда-то станет false). NIT-ы: порядок мутаций в `enable_web_tools` (флаги до включения фичи) + doc-кросс-рефы. Гейт зелёный, clippy 0, egress-chokepoint цел. Включение на .28 + live web.fetch публичного URL — следующим шагом.

### Агент · AGENT-AUTO — конфигурируемая автономия коннектора (headless-сервер авто-применяет Auto-тир)

Автономия прогонов коннектора стала конфигурируемой (была хардкод `"confirm"`). Новый `ConnectDeps.autonomy: String` + конфиг `ai.agent_autonomy` (`"confirm"`|`"auto"`, default `confirm`). agentd валидирует **fail-safe**: ТОЛЬКО точная строка `"auto"` поднимает автономию, всё прочее (опечатки/регистр/пусто) → `confirm` (+ warn на неизвестное значение). Owner-gated решение 2026-06-22: headless-сервер на .28 ставит `auto` → агент САМ авто-применяет НИЗКО-рисковые vault-записи (Auto-тир: blast-cap + snapshot + обратимый undo + durable audit); Confirm-тир (риск/крупная перезапись/HardBlock) НЕ авто-применяется — он ПРЕДЛАГАЕТСЯ по проводу (Proposal-событие) и пишется лишь по явному `agent/approve` (decision_source = `ChannelDecision`, fail-closed reject_all при дисконнекте; **NB: это не «auto-DENY», а человек-в-петле через провод** — risky-записи апрувятся интерактивно, напр. из десктопа). Defense-in-depth: эффект ТОЛЬКО при `agent_actuator_enabled=true` (два независимых флага); ядро нормализует автономию exact-match (`auto = autonomy == Some("auto")`). Десктоп (in-process путь, не ConnectDeps) и scheduler-`AgentRunHandler` (автономия из run-строки, PolicyDefault) — НЕ затронуты.

**Мультиагент-ревью (2 линзы, security+correctness)**: линзы РАЗОШЛИСЬ по ключевому инварианту (Confirm-тир под auto: auto-DENY vs wire-approve) → разрешено чтением кода — коннектор использует `ChannelDecision` (не `PolicyDefault`), значит Confirm-тир = wire-approve. Исправлена doc-неточность во всех местах (был ошибочный «auto-DENY»). Добавлен keystone-тест границы коннектора: `autonomy=auto`+actuator → Auto-тир `note.create` авто-применён БЕЗ approve (файл записан, без proposal). Тесты: propagation + keystone. Гейт зелёный, clippy 0, egress/tooluse целы. Включение на .28 (actuator+auto) + live apply→undo на копии заметок — следующим шагом. (Опц. хардненинг — типизировать autonomy enum'ом — отложен: fail-safe уже на двух уровнях.)

### Агент · SANDBOX-2 — GuardedProxy: единственный сетевой путь песочницы (default-OFF)

Второй срез Фазы-2 каркаса (`docs/specs/agent-sandbox.md §4`). Модуль `nexus-core::sandbox::proxy`: песочница бежит `--network=none` (нет NIC) → сетевую capability даёт ТОЛЬКО host-side **`GuardedProxy`** поверх AF_UNIX. In-sandbox-шим **`ProxyGuardedClient`** фреймит каждый запрос как typed JSON-RPC (`egress/get`/`egress/post`, framing AGENT-CONNECT `RpcMessage`), хост ре-эмитит через СУЩЕСТВУЮЩИЙ `GuardedClient` (chokepoint: allowlist → SSRF/DNS-rebind → durable audit с `run_id`). Второго не-guarded пути нет физически.

Fail-closed инварианты (§4.3): **`run_id` отсутствует в wire-DTO** — клиент физически не может задать корреляцию, хост всегда штампует свой `RunCtx::run` (сильнее «игнорирования поля», by-construction); **хост назначения — только из `url`** (парсит `GuardedClient`, typed-верба не HTTP-forward-proxy → нет request-smuggling/desync SSRF); **deny-not-clamp** — фича матчится строкой против `Display` allow-set прогона, нет совпадения (неизвестная/`probe`/`news_feed`/`web`-когда-не-разрешён) → отказ, НЕ тихий выбор мягкой фичи (Chat/Embed допускают LAN → кламп открыл бы LAN-SSRF); **per-run egress-бюджет** (`EgressBudget`: кэпы запросов + исходящих байт тела POST = вектор эксфильтрации) — превышение → отказ ДО сети; ошибки бэкенда **санитизированы** в `RpcError` (без host/url/кредов). Бэкенд абстрагирован `EgressBackend` (реальный = `GuardedClientBackend` поверх `GuardedClient` — единственное, что его зовёт) ради Tier-1-тестируемости логики без сети. `ProxyGuardedClient` НЕ конструирует `reqwest` — уже гарантируется существующим `check-egress.mjs` (билдеры только в `net/`); отдельный `check-sandbox-egress.mjs` избыточен пока нет отдельного sandbox-build-таргета (отложен до SANDBOX-4). Шим возвращает `EgressResponse`-DTO; адаптация провайдеров chat/embed под него — SANDBOX-4.

10 Tier-1 тестов (мок-бэкенд): success+host-run_id-штамп+host-из-url, unknown/probe/news_feed→deny (бэкенд не тронут), over-broad web→deny-not-clamp, web-в-allow-set→ok, budget req-cap/byte-cap→deny, method-not-found, shim↔proxy round-trip через `ChannelTransport`, RpcError-проброс, id-mismatch→fail-closed. **Adversarial-ревью (security-линза): keystone держится, 0 CRITICAL/MAJOR** (подтверждены все §4.3-инварианты: bypass невозможен, run_id неподделываем, deny-not-clamp звучен, нет host-smuggling, бюджет без overflow/обхода, ошибки не текут); закрыт 1 MINOR — `ProxyGuardedClient::call` теперь сверяет `id` ответа с запросом (fail-closed на случай будущего конкурентного шим-вызова). Гейт зелёный, clippy 0, egress-chokepoint цел. default-OFF (рантайм песочницы — SANDBOX-4; Tier-2 Podman-live — SANDBOX-5).

### Агент · SANDBOX-1 — каркас песочницы: рендер хардненного `podman run` (default-OFF)

Первый срез Фазы-2 каркаса (`docs/specs/agent-sandbox.md §11`). Новый модуль `nexus-core::sandbox`: `SandboxConfig`/`SandboxPlan`/`sandbox_run_plan` — ЧИСТЫЙ рендер argv `podman run` (в ядре, т.к. будущий `SandboxRunner`/SANDBOX-4 его зовёт; `nexus-cli` зависит от ядра, не наоборот). Хардненинг: `--network=none` (нет NIC — egress только через GuardedProxy, SANDBOX-2), `--read-only`+`--tmpfs /tmp`, `--cap-drop=ALL`, `--security-opt no-new-privileges`, `--userns=keep-id`, ресурс-кэпы (`ResourceCaps` дефолт pids=512/mem=2g/cpus=2); vault bind **`:ro`**; per-run каталог сокетов — ОТДЕЛЬНЫЙ mount в `/run/nexus`, структурно вынесен из-под vault (`SandboxConfig::for_run` деривит из `runtime_base` + отвергает путь внутри vault, §4.4); хост-окружение НЕ пробрасывается (нет `-e/--env` — секреты не утекают в argv; полный env-scrub fail-closed — SANDBOX-6a). Флаг `ai.sandbox_enabled` (default-false) добавлен в `AiConfig` — на этом срезе ещё инертен (рантайма/GuardedProxy/runner нет). Tier-1 render-тесты (6): все хардненинг-флаги, vault `:ro`, сокеты-distinct-mount-вне-vault, no-host-env, `validate_run_id` (podman-формат), POSIX-регресс-гард (без бэкслешей). Cross-platform: путь каталога сокетов строится POSIX-join (`/`), не `PathBuf::join` — песочница Linux-host-only, `join` дал бы `\` на Windows-CI (тот же класс, что фикс в `deploy remote`). Гейт зелёный, clippy 0, egress-chokepoint цел (модуль НЕ конструирует reqwest). default-OFF/CI-verifiable; рантайм-enforcement `--network=none` — Tier-2 (Podman-gated, SANDBOX-5).

### Спека · AGENT-SANDBOX — дизайн OS-песочницы прогона агента (Фаза 2/3, decision-complete)

Новая спека `docs/specs/agent-sandbox.md` (v1.0, RU): decision-complete дизайн песочницы агента — следующий greenlit-roadmap-шаг (owner-gated security, всё default-OFF/fail-closed/CI-verifiable). Синтез мультиагентного Workflow (3 дизайна: ephemeral-podman / persistent-sandbox / bubblewrap-ns → adversarial-критика по каждому → судья-синтез). Выбран **эфемерный per-run rootless-Podman** (база — репозиторный `Dockerfile`) c `--network=none` + host-side `GuardedProxy` поверх СУЩЕСТВУЮЩЕГО `GuardedClient` (egress физически замкнут на chokepoint, второго не-guarded пути нет). **Ключевая инверсия** (из критики): Фаза-3 shell исполняется ВНУТРИ песочницы, host только РЕШАЕТ (classify→approval) — иначе exec бежал бы с ambient-правами agentd в обход guard. Чёткое разделение Фаза-2 каркас (SANDBOX-1..5, строится автономно, БЕЗ новых ActionTarget) vs Фаза-3 host-actuator (SANDBOX-6a..c, owner-gated). Закрывает дизайн-долг THREAT_MODEL §T7. Anti-footgun из критик: deny-not-clamp для over-broad EgressFeature (Chat/Embed допускают LAN → кламп открыл бы LAN-SSRF), env-scrub fail-closed (пустое+allowlist, не denylist), proxy-сокет НЕ под `:ro`-vault, SO_PEERCRED, Web OFF + egress-бюджет против read-then-exfil, `exec_targets_never_auto`-тест-инвариант, честный Tier-2 (Podman-gated = единственное реальное доказательство `--network=none`). Все цитируемые код-якоря провалидированы (actuator/*, net/mod.rs EgressFeature/denies_private, run_store, connect). 12 секций + роадмап + 5 owner-вопросов. Реализация SANDBOX-1.. — следующими срезами.

### Деплой · `nexus undeploy remote` — симметричное снятие удалённого сервиса (DEPLOY-5)

Закрыта асимметрия деплой-CLI: был `deploy remote`, не было чистого снятия. **`nexus undeploy remote --host user@host [--remote-home P] [--apply]`** — ssh `systemctl --user disable --now` + `rm` юнита + `daemon-reload` (`remote_undeploy_plan` в `service.rs`). Все шаги **best-effort** (снятие отсутствующего сервиса — норма, как у локального `undeploy`/`undeploy docker`). Бинарь и vault НЕ трогает (паритет с локальным `undeploy` — убираем сервис, не данные). Переиспользует валидаторы `validate_remote_user/host`/path. **Safe default — печать ПЛАНА**; ssh под `--apply`. Тесты: 26 (1 новый: disable→rm→reload best-effort + не-трогает-бинарь/vault). Гейт зелёный, clippy 0. Теперь поверхность CLI симметрична: local/remote/docker × deploy/undeploy.

### CI · docker-build-smoke — автоматическая валидация образа agentd (DEPLOY-4)

Новый workflow `.github/workflows/docker-smoke.yml`: собирает образ agentd из корневого `Dockerfile` (DEPLOY-3) и гоняет **runtime-смоук** (`docker run … /nonexistent-vault-smoke` → ожидаем ненулевой выход + ошибку «vault path …» = бинарь стартовал в slim-рантайме и все `.so` резолвятся, не просто «собралось»). Закрывает follow-up DEPLOY-3 (Dockerfile нельзя собрать локально — нет Docker на dev-маке). **Self-validating**: PR трогает workflow-файл → смоук бежит на самом PR. **Paths-gated** (Dockerfile/.dockerignore/nexus-agentd/nexus-core/Cargo.*/rust-toolchain.toml/сам workflow) — дорогой (~6-10 мин) образ собирается ТОЛЬКО на причастных PR; + еженедельный `schedule`-cron (дрейф плавающего `rust:1-bookworm`) + `workflow_dispatch`.

**Хардненинг по adversarial-ревью (2 линзы)**: smoke-семантика подтверждена здоровой (позиционный vault-арг приоритетнее ENV; `tracing::error!` печатает до `exit(1)`; нет false-pass/fail). Закрыты находки: **paths-trigger landmine** — комментарий-предупреждение НЕ вешать `docker-build` в required checks (иначе непричастные PR зависнут; ветка сейчас не protected → риск только будущий); **дрейф плавающего базового образа** — weekly `schedule`; **`rust-toolchain.toml`** добавлен в paths (бамп тулчейна влияет на in-container build); **push-триггер убран** (PR валидирует merge-результат на squash-репо — не собираем образ дважды); `permissions: contents: read` (least-priv, сборка untrusted PR-контекста); `timeout-minutes: 25`; grep ужат до `«vault path»` (привязка к коду canonicalize-ошибки). `DOCKER_BUILDKIT=1` (Dockerfile несёт `# syntax=`).

### Деплой · контейнеризация agentd — `Dockerfile` + `nexus deploy docker` (DEPLOY-3)

Агент-сервис теперь запускается в Docker-контейнере. **Multi-stage `Dockerfile`** (корень репо): builder `rust:1-bookworm` собирает ТОЛЬКО `cargo build --release -p nexus-agentd` (десктоп/Tauri и его webkit/gtk НЕ компилируются) → runtime `debian:bookworm-slim` (ABI-совпадает с builder) + `ca-certificates`, непривилегированный пользователь `nexus` (uid 10001), vault как том `/vault`, `ENTRYPOINT nexus-agentd`. Рантайм минимален: дерево зависимостей agentd чистое — **rustls-tls + bundled webpki-roots** (без openssl), нет git2/native-tls → slim-образу хватает libc (rusqlite — bundled SQLite статикой). `.dockerignore` исключает target/node_modules/.git.

**`nexus deploy docker --vault P [--image N] [--name N] [--build [--context P]] [--apply]`** + **`undeploy docker [--name N] [--apply]`** (stop+rm). Чистый рендер плана (`service.rs`: `DockerConfig`/`DockerPlan`/`docker_plan`/`docker_undeploy_plan`, argv-векторы `docker build`/`docker run` — БЕЗ шелла) отделён от actuation под `--apply`. `docker run -d --restart unless-stopped -v <vault>:/vault -e NEXUS_AGENTD_CONNECT_SOCKET=/vault/.nexus/agentd.sock` → коннектор по AF_UNIX на bind-mount, хост-сокет `<vault>/.nexus/agentd.sock` (для `nexus status`). **Safe default — печать ПЛАНА.** Без `--build` — предупреждение, что образ должен существовать; build-шаг проверяет наличие `Dockerfile` в контексте; `--apply` через `run_cmds_strict` (провал build не ведёт к run устаревшего образа). На **macOS Docker Desktop** — предупреждение: AF_UNIX-сокет через virtiofs не пробрасывается, контейнер-деплой рассчитан на Linux-хост (риг/VPS).

Безопасность: docker-аргументы идут argv-векторами через `Command` (без шелла → нет инъекции); валидаторы `validate_image_name` (allowlist `[A-Za-z0-9._:/-]`, не с `-`) + `validate_container_name` (docker-формат `[A-Za-z0-9][A-Za-z0-9_.-]*`) + `validate_docker_user`. Диспетч `undeploy docker` ДО общего `undeploy` (иначе "docker" утёк бы во флаги launchd/systemd-выгрузки).

**Хардненинг по adversarial-ревью** (build/run-путь подтверждён здоровым — 0 CRITICAL/MAJOR; 4 MINOR закрыты): **`--vault` теперь ОБЯЗАТЕЛЕН** (без него `resolve_vault` смонтировал бы cwd как vault — footgun); **`--user uid:gid`** проброшен в `docker run --user` (контейнер пишет bind-mount vault + сокет, доступный хосту; иначе uid 10001 образа может не иметь прав) + ⓘ-нота о владельце vault; **macOS `--apply` заблокирован без `--force`** (как `deploy local` не ставит нерабочий сервис — virtiofs не пробрасывает AF_UNIX); **подсказка при провале `--apply`** (контейнер с именем уже существует → `nexus undeploy docker`). Тесты: 25 (8 новых: docker run-only/build-then-run/run-user/undeploy-stop-rm + валидация image/container/user). Гейт зелёный, clippy 0, egress-chokepoint цел, `#[ignore]`=27. Образ — база Фазы-2 Podman-песочницы (`--network=none`) + закрывает P0-дыру #6 (контейнер-деплой). LIVE docker-build (нет Docker на маке; CI docker-build-smoke) — follow-up.

### Деплой · `nexus deploy remote` — развёртывание agentd на удалённый Linux-хост (DEPLOY-2)

Новая сабкоманда `nexus deploy remote --host user@host --binary <linux-agentd> [--remote-vault P] [--remote-socket P] [--remote-home P] [--apply]` — деплой агент-сервиса на удалённый хост с `systemd --user` через `ssh`/`scp`. Цель — **риг 192.168.0.31** (на нём локальный LLM, естественный «агент на сервере»; VPS отпал — нет доступа к LLM). Чистый рендер плана (`service.rs`: `RemoteConfig`/`RemoteStep`/`RemotePlan`/`remote_plan`, переиспользует `render_systemd_unit` с УДАЛЁННЫМИ абсолютными путями) отделён от актуации. Шаги: `mkdir -p` (bin/unit/log/vault) → `scp` бинаря → `chmod +x` → `scp` юнита → `loginctl enable-linger` (best-effort) → `systemctl --user daemon-reload` → `enable --now`. systemctl-команды несут `XDG_RUNTIME_DIR=/run/user/$(id -u)` (ssh без логин-сессии). **Safe default — печать ПЛАНА**; ssh/scp только под `--apply` (наследуют stdio → интерактивный ввод пароля/passphrase; аутентификация на стороне ssh). Удалённый домашний каталог по соглашению `root→/root`, иначе `/home/<user>` (override `--remote-home`).

Удалённые пути/user/host встраиваются в `ssh <cmd>`-строки БЕЗ shell-экранирования, поэтому безопасность держится на валидаторах: `validate_remote_path` (абсолютный + отказ всех shell-метасимволов/контрол-символов), `validate_remote_user` (allowlist `[A-Za-z0-9._-]`), `validate_remote_host` (строгий allowlist `[A-Za-z0-9.-]`). Тесты: 7 (`default_remote_home`, рендер юнита на удалённые пути, порядок/полнота шагов, mkdir-покрытие+XDG, валидация path/user/host).

**Хардненинг по adversarial-ревью** (3 находки MAJOR/MINOR): **host-allowlist** вместо blocklist закрыл тихие мис-таргеты (`a@b@c` → `split_once('@')` оставлял `@` в хосте; `:`/`,`/`%`/`=` тоже проходили) → теперь `@`/`:`/`,` отвергаются; **подсказка на фейл `systemctl --user`** «свежего хоста» (нет user-bus → `sudo loginctl enable-linger` с правами + повтор `--apply`, бинарь/юнит уже доставлены) — закрывает непрозрачный «Failed to connect to bus»; **symlink-safe temp-юнит** (`create_new` O_EXCL + предварительный unlink вместо `fs::write` по предсказуемому имени в общем `/tmp`). **Cross-platform-фикс** (поймал Windows-CI): удалённые POSIX-пути строятся `posix_join` (`/`), а не `PathBuf::join` — иначе деплой С Windows на Linux-риг рендерил бы юнит/`mkdir` с `\` (сломанный сервис); регресс-гард в тестах (юнит без бэкслешей). Гейт зелёный, clippy 0, egress-chokepoint цел (ssh/scp — process-spawn, не HTTP-эгресс). LIVE-деплой на риг (нужен cross-compiled linux-бинарь) — отдельная валидация.

### Агент · EGR-AGENT-2: активация веб-инструментов в проде (конфиг → деплоенный агент, LIVE ✓)

Веб-инструменты из EGR-AGENT теперь РЕАЛЬНО работают в развёрнутом агенте (были построены, но дремали). Конфиг `ai.web {url, enabled}` (`WebConfig` в `AiConfig`, default-OFF) → `nexus-core::agent::enable_web_tools(policy, audit, url, timeout)` включает `EgressFeature::Web` + allowlist хоста SearXNG в скоупе `"web"` (не трогая скоуп `"ai"`) и строит `WebToolsConfig` (клиент `GuardedClient::for_web`, redirect=none). Проброшен в ОБА пути прогона: `AgentRunHandler.web` (scheduler) + `ConnectDeps.web` (коннектор) → `run_agent_session` регистрирует `web.search`/`web.fetch`. `desktop` пока `None` (web в десктоп-UI — отдельный срез).

**LIVE-проверено через демон**: `nexus-agentd` с `ai.web.enabled=true` (SearXNG VPS :8888) + AF_UNIX-коннектор → `nc -U` `agent/run` → агент сделал `toolCall web.search` → результаты с «Париж/Paris» → финал. Лог демона: «EGR-AGENT: веб-инструменты ВКЛ». Конфиг→активация→инструмент→ответ — end-to-end через сокет.

Adversarial-ревью активации (egress-фокус): default-OFF сохранён (web absent/`enabled=false` → фича Web выключена, allowlist пуст), scope-изоляция (`set_scoped_allowlist("web",…)` не клоберит `"ai"`), без over-broadening (только хост SearXNG; прочие URL режутся на step-4b ДО сети), threading корректен (borrow→move), redirect=none сохранён → **0 дефектов**. Гейт зелёный, clippy 0, egress-chokepoint цел.

### Агент · EGR-AGENT: веб-инструменты агента — `web.search` + `web.fetch` (LIVE ✓)

Агент научился ИССЛЕДОВАТЬ интернет. Новый `nexus-core::agent::web_tools`: **`web.search`** (мета-поиск через SearXNG: build/parse портированы в ядро) + **`web.fetch`** (HTTP GET публичного URL, HTML→текст). Весь эгресс — через `GuardedClient` с **`EgressFeature::Web`** (web-класс: `deny_private=true` → SSRF/DNS-rebind-гард + allowlist хостов + redirect=none + durable-аудит + per-call `RunCtx`). Результат — НЕДОВЕРЕННЫЕ ДАННЫЕ: `run_agent_loop` фенсит КАЖДЫЙ tool-результат (`fence_observation` + per-request `injection_marker`) → веб-контент не может инъектировать инструкции. `WebToolsConfig` (RUN-независимый клиент+SearXNG-URL) строит композиционный корень; `run_agent_session` получил `web: Option<&WebToolsConfig>` и регистрирует read-only веб-инструменты (НЕ требует actuator-флага). Активация в проде (agentd/connect-конфиг) — следующий срез EGR-AGENT-2 (сейчас все прод-вызывающие передают `None`; capability построена + проверена).

**LIVE-проверено**: реальная Qwen3.6-27B на риге 192.168.0.31:8080 через `web.search("столица Франции")` → SearXNG на VPS :8888 → 8 реальных результатов (Википедия) → ответ «Столица Франции — **Париж**» (45 c). Полный стек: модель → web-инструмент → guarded-эгресс → SearXNG → фенсенные результаты → ответ.

**Хардненинг по adversarial-ревью** (3 линзы, 22 агента): гигиена URL-кредов — `web.fetch` ОТКЛОНЯЕТ URL с `user:pass@` + secret-чек URL; `looks_secretish` ловит basic-auth-в-URL; `net::authorize` на нераспарсенном URL пишет в durable-аудит безопасный плейсхолдер (не сырую строку с возможными кредами); **`html_to_text` сделан UTF-8-корректным** (был баг byte-as-char → мусор на кириллице). Корректные-by-design (redirect=none уже в ядре, фенсинг циклом, allowlist-гейт) подтверждены. Тесты: 6 (build_search_url/parse_searx/html-strip/**html-non-ASCII**/secretish+creds/arg-reject) + 1 LIVE `#[ignore]`. `EXPECTED` #[ignore] 26→27. Гейт зелёный, clippy 0, egress-chokepoint цел. Лёгкая `looks_secretish` (vs полный `git::scan_secrets`) + arbitrary-public web.fetch (сейчас allowlist-gated) — в EGR-AGENT-2.

### Деплой · `nexus deploy local` — CLI развёртывания агент-сервиса (PROD-v1 item 4, LIVE ✓)

Новый крейт `crates/nexus-cli` (бинарь `nexus`) — делает `nexus-agentd` устанавливаемым локальным сервисом. Команды: **`deploy local`** (bootstrap `.nexus` + рендер сервис-юнита — **launchd** на macOS / **systemd --user** на Linux — который запускает `nexus-agentd <vault>` с `NEXUS_AGENTD_CONNECT_SOCKET`; **safe default — печать ПЛАНА**, установка только под `--apply`) · **`status`** (подключиться к AF_UNIX-сокету + `initialize` → доступность + версия протокола) · **`undeploy`** (остановить + удалить юнит). Чистый рендер (`service.rs`: `render_launchd_plist`/`render_systemd_unit`/`plan`) отделён от актуации (запись файла + `launchctl`/`systemctl` через argv-векторы **без шелла** → нет инъекции). Сокет по умолчанию `<vault>/.nexus/agentd.sock` (дискаверится приложением по vault). Без clap (ручной разбор, как agentd); сетевого egress нет.

**LIVE-проверено end-to-end на macOS**: `deploy local --apply` → launchd-сервис стартовал → сокет связан → **`nexus status` → «✓ агент ДОСТУПЕН, протокол v1.0»** (CLI подключился к launchd-управляемому agentd по сокету) → `undeploy --apply` чисто. Эмпирически найдена **macOS TCC-грабля**: launchd-агент без Full Disk Access НЕ создаёт сокет/логи в privacy-каталогах (`~/Documents`/`~/Desktop`/`/tmp`) — тихий сбой; перенос в обычный home-каталог лечит. Добавлено **TCC-предупреждение** в `deploy` (детектит рискованный путь vault/бинаря на macOS). Тесты: 11 (рендер plist/systemd + экранирование кавычек, план путей/команд, undeploy_plan, разбор флагов + reject-flag-value, default/relative-сокет, контрол-символы, TCC-детект). **Хардненинг по adversarial-ревью** (3 линзы, 21 агент): systemd-экранирование путей с `"`/пробелом (ExecStart + Environment в кавычках), отказ на **relative-пути agentd** (launchd/systemd не резолвят PATH в ExecStart → был бы мёртвый сервис), валидация путей (без `\n`/NUL), `--socket` обязан быть абсолютным, создание родителя сокета, `--apply` не ставит сервис при отсутствующем бинаре, status-диагностика (нет-файла vs не-сокет), egress-линт расширен на `nexus-cli`. Гейт зелёный, clippy 0. git-sync мост — следующий срез.

### Агент · LIVE actuator-валидация: реальная модель пишет заметку через гейт → undo (на риге + real-vault)

Валидация ПОЛНОГО стека актуатора вживую (owner-запрос «проводи лайф тесты»). Коммитнут `#[ignore]` live-тест `agent::session::tests::live_actuator_create_and_undo_on_rig` (env `NEXUS_LIVE_CHAT=1`): реальный `OpenAiToolProvider`→риг, `actuator_enabled=true` + `autonomy=auto`, модель вызывает `note.create` → `dispatch_action` гейт (Auto-тир apply) → файл РЕАЛЬНО записан в temp-vault → `actuator::undo_run` восстанавливает (restored≥1, файл удалён). **ПРОЙДЕН на риге 192.168.0.31:8080** (Qwen3.6-27B-MTP создала `Notes/AgentLiveTest.md` = «привет от агента», 37 c): доказан стек модель → tool-call → гейт → apply на диск → undo. `EXPECTED` #[ignore] 25→26.

**Real-vault-валидация** (на рабочей копии реального волта владельца — 231 заметка, app-созданный `.nexus/nexus.db`): `nexus-agentd` ОТКРЫЛ волт, применил ВСЕ миграции чисто (1→22, forward-compat с боевой БД подтверждён), actuator+cycle smokes прошли. Замечание (не баг): dev-smoke `NEXUS_AGENTD_SMOKE` калиброван под offline (8-c дедлайн lifecycle), а у боевого волта в `local.json` сконфигурирован живой LLM → прогон реально позвал модель (~105 c, статус `done`) → 8-c дедлайн смока сработал ложно. Смок не предназначен для real-LLM волта (CI гоняет его на temp-vault); продового дефекта нет.

### Агент · AGENT-CONNECT P0b-2c — agentd ХОСТИТ коннектор по AF_UNIX (агент = сервис, LIVE ✓)

`nexus-agentd` стал **подключаемым агент-сервисом**: приложение/CLI говорит с демоном по протоколу AGENT-CONNECT через локальный сокет. Новый `nexus-core::agent::connect::afunix` (Unix-only): **`AfUnixTransport`** (impl `Transport` поверх `UnixStream`, кадрирование = line-delimited JSON, read/write-половины за отдельными мьютексами — конкурентные `send` сериализуются, единственный `recv` читает строки по очереди; парс-сбой строки НЕ роняет соединение, EOF→`recv`=None) · **`connect_unix(path)`** (клиент) · **`serve_unix_at(path, deps)`** (удаляет stale-сокет → bind → `harden_socket_perms` 0600 → accept-loop) · **`serve_unix`** (на каждое подключение — СВЕЖИЙ `ConnectAgentHandler` с изолированным реестром сессий + dispatch-loop до EOF; `ConnectDeps` шарятся). **Защита-в-глубину: сокет 0600** (owner-only — коннектор привилегированный peer: драйвит агента/читает vault через tools/тратит токены). agentd: `maybe_spawn_connect_server` — **default-OFF**, включается env `NEXUS_AGENTD_CONNECT_SOCKET=<путь>`, строит `ConnectDeps` из ТЕХ ЖЕ зависимостей, что `AgentRunHandler` (провайдер/память/актуатор-конфиг/скиллы клонируются), без провайдера → не стартует; автономия коннектора = `confirm` (запись актуатора требует `agent/approve`). AF_UNIX = локальный IPC, НЕ сетевой egress.

**Хардненинг по мультиагентному adversarial-ревью** (3 линзы, 32 агента — AF_UNIX = новая поверхность атаки, поэтому ревью нашёл реальные дыры): (1) **анти-OOM** — `recv` читает кадр через `fill_buf`/`consume` с капом длины (`MAX_LINE_BYTES`=1 MiB; раньше `read_line` рос безгранично на потоке без `\n`); (2) **анти data-loss** — `prepare_socket_path` удаляет stale-сокет ТОЛЬКО если это реально сокет (мисконфиг env на обычный файл → отказ, чужой файл не трогаем); (3) **анти-leak** — канал событий стал ОГРАНИЧЕННЫМ (`try_send`, дроп при мёртвом drain — раньше unbounded рос при отвале клиента мид-ран); (4) **kill-switch** — `ConnectDeps.agent_paused` = ТОТ ЖЕ Arc, что у `AgentRunHandler` (SIGUSR1/agent.json пауза демона честится прогонами коннектора; `agent/control` ставит её); (5) анти-spin backoff в accept-loop + кап подряд-malformed строк. Корректные-by-design (undo vault-scoped, autonomy=confirm, 0600) оставлены.

**LIVE-проверено end-to-end**: запущен РЕАЛЬНЫЙ бинарь `nexus-agentd` над temp-vault + AF_UNIX-сокетом, `nc -U`-клиент сделал `initialize`→`{version:1.0}` + `agent/run`→`{runId:1}` + получил поток `agent/event` (`contextUsage`→`toolCall(debug.echo)`→`toolResult`→стрим `assistantToken`→`final`) — **реальная Qwen3.6-27B на риге 192.168.0.31:8080 проехала цикл через демон по сокету**. Тесты: 5 в `afunix` (round-trip + malformed-skip, EOF→None, oversized→close, prepare-path-refuses-non-socket, перм 0600) + `serve_unix_drives_run_over_socket` (e2e на реальном сокете) + `agent_control_sets_global_pause`. Гейт зелёный, clippy 0.

### Агент · AGENT-CONNECT P0b-2b — ConnectAgentHandler: коннектор ДРАЙВИТ цикл (+ LIVE на риге ✓)

**Замыкает коннектор «агент = сервис».** `nexus-core::agent::connect::handler::ConnectAgentHandler` реализует трейт `ConnectHandler` (P0a) поверх `run_agent_session` (P0b-2a): протокол JSON-RPC + wire-DTO + единая композиция → РАБОЧИЙ агент-сервис за `Transport`. `agent/run` создаёт прогон, спавнит цикл и **стримит его события клиенту как `agent/event`-нотификации** через `event_notification` (тот же wire-контракт, что у desktop UI-1b — без расхождения). Синхронный `AgentEventForwarder` мостится в асинхронный транспорт через unbounded-mpsc + drain-таск (`TransportForwarder`).

- **Сессии + контроль**: реестр `session_id → SessionHandle` (run_id + decision-sender + per-session `paused`/`cancel`). ОДИН активный прогон на `session_id` (реестр под локом через `create_run` — анти-TOCTOU; 2-й `agent/run` на активную сессию → `invalid_params`); дерегистрация на finish с guard по `run_id` (defense-in-depth при переиспользовании сессии). `agent/approve` кормит `ChannelDecision` (человек-в-петле, fail-closed reject_all); `agent/control` — per-session пауза; `agent/cancel` — кооперативно (idempotent no-op для неактивной/чужой сессии); `agent/undo` — `actuator::undo_run` (идемпотентно).
- **Безопасно по умолчанию**: автономия прогонов коннектора ЖЁСТКО `confirm` — запись актуатора требует ЯВНОГО `agent/approve` (повышение до `auto` — owner-gated). Ошибки sanitized (`RpcError::internal` логирует detail, клиенту — общий текст; T3/THREAT_MODEL). Новых egress-путей нет (тот же `GuardedClient`).
- **Тесты**: 5 offline (e2e `initialize`→`agent/run`→стрим `toolCall`→`final`; **approve-over-wire применяет note.create через гейт**; version-incompatible; cancel-неизвестной-сессии idempotent; один-прогон-на-сессию) + **1 LIVE `#[ignore]`** (`live_connect_tool_loop_on_rig`). **ЖИВОЙ ПРОГОН ПРОЙДЕН на риге 192.168.0.31:8080** (реальная Qwen3.6-27B-MTP): модель вызвала `echo`-инструмент через протокол коннектора и завершила ход — real tool-calling end-to-end, 32.8 c. Гейт зелёный, clippy 0 warnings, мультиагентный adversarial-ревью (3 линзы: concurrency-lifecycle / security-failclosed / protocol-correctness → верификация). `EXPECTED` #[ignore]-гейта 24→25.

### Агент · AGENT-CONNECT P0b-2a — ЕДИНАЯ композиция прогона `run_agent_session` (DRY)

Подготовка к ConnectHandler: композиция прогона агента жила в ТРЁХ копиях (headless `AgentRunHandler::drive`, desktop `drive_run`, намечался agentd-коннектор) — они расходились по контракту. Выделен единый транспорт-агностичный источник истины **`nexus-core::agent::session`**: `run_agent_session(...)` собирает начальный контекст ([system преамбул] + [recall памяти] + [меню скиллов] + [задача]), выбирает реестр (стабы при actuator-OFF → vault не трогается; ВКЛ → гейтнутые актуаторы за `dispatch_action`), регистрирует tier-2/3 инструменты скиллов и крутит `run_agent_loop`; финализацию в `run_store` оставляет вызывающему. Куда уходят события — решает переданный **`AgentEventForwarder`** (sync `forward(&AgentEvent)`): два потока (события цикла через `on_event` + Proposal/Diff гейта через `ForwardingEventSink`) сводятся в ОДИН форвардер (потоки непересекающиеся — без дублей). Решает давний `FIXME(UI-1)` (связка gate-EventSink ↔ on_event-стрим). **Конвергенция обоих существующих вызывающих** (без изменения внешнего поведения): `drive` → `HeadlessForwarder` (счёт `ToolResult`-шагов + `tracing`-лог Proposal/Diff, как прежний `TracingEventSink`; wrapper идемпотентности/паузы/финала не тронут); desktop `drive_run` → `ChannelForwarder` (маппинг в wire-DTO → Channel; заменяет прежние `ChannelEventSink`+`on_event`, минус дубль). Тесты: 2 новых в `session` (порядок ToolCall→ToolResult→Final через единый форвардер; тривиальный прогон), все прежние зелёные (nexus-core agent::job, desktop commands::agent ×4, agentd ×7 вкл. live_actuator_gate). Гейт зелёный, clippy 0 warnings, мультиагентный adversarial-ревью (3 линзы behavior-equivalence: headless/desktop/seam → верификация).

### Агент · AGENT-CONNECT P0b-1 — wire-DTO унификация (ЕДИНЫЙ контракт desktop↔agentd)

Закрывает отложенный из P0a EventSink-маппинг и устраняет риск расхождения контракта «бэкенд→клиент». Контракт стрима событий агента (`AgentStreamEvent` + `map_agent_event`) **вынесен в `nexus-core::agent::connect::wire`** — единый источник истины для ОБОИХ потребителей: desktop UI-1b (`Channel<AgentStreamEvent>`) и будущий agentd-коннектор (`agent/event`-нотификация). `desktop::commands::agent` теперь **ре-экспортит** эти имена из ядра (минус ~271 строка: убраны локальная копия DTO и 8 дублирующих map_*-юнитов — их покрытие переехало в `wire.rs`). Новое `event_notification(&AgentEvent) -> Option<RpcMessage>` оборачивает событие в JSON-RPC `agent/event` через wire-DTO (а НЕ `to_value(AgentEvent)` — у ядра newtype-варианты `Final(String)`/`AssistantToken(String)` несовместимы с serde-internal-tag; регрессия-тест `agent_event_newtype_is_not_directly_serializable` зафиксирован). Матч `map_agent_event` сделан **намеренно экзаустивным** (без `_`-рукава): `wire.rs` живёт В `nexus-core`, поэтому новый вариант `AgentEvent` теперь ВЫЗОВЕТ ошибку компиляции здесь и заставит явно решить его wire-маппинг — гарантия, что контракт desktop↔agentd не разъедется молча при росте ядра. JSON на проводе НЕ изменился (теги/`isError`/`runId`/`actionId`/camelCase идентичны). Тесты: 4 в `wire.rs` (newtype/struct-варианты, proposal/diff с явным runId, round-trip) + `event_notification_wraps_via_wire_dto`. Гейт зелёный, clippy 0 warnings, мультиагентный adversarial-ревью (3 линзы: wire-identity/behavior-preservation/frontend-contract → верификация).

### Агент · AGENT-CONNECT P0a — протокол-фундамент коннектора (agent=service)

Первый срез эпика «агент как сервис» (план: `docs/AGENT-PROD-PLAN.md`, спека: `docs/specs/agent-connect.md`). Новый модуль `nexus-core::agent::connect` — транспорт-агностичный JSON-RPC 2.0 слой коннектора app↔`nexus-agentd`: framing (`RpcMessage`/`RpcEnvelope` request/notification/response, классификация по `method`/`id`), подключаемый `Transport`-трейт + in-process `ChannelTransport` (tokio-mpsc дуплекс), `dispatch` метод→`ConnectHandler` (initialize/agent.run/undo/cancel + approve/control-notifications), version-negotiate, **sanitized-ошибки** (`internal()` не утекает detail клиенту — T3/THREAT_MODEL), ACP-tool-kind маппинг. Чистая граница без LLM. 12 unit-тестов (framing round-trip, version, error-sanitize, transport дуплекс, dispatch req/notification/unknown/bad-params/incompatible-version). **Отложено в P0b** (EventSink-адаптер): маппинг `AgentEvent`→`agent/event` (newtype-варианты несовместимы с serde-internal-tag → нужен явный wire-DTO) + привязка к `run_agent_loop` + **live tool-loop на риг** (tool-calling рига подтверждён пробой 2026-06-20).

### Доки · Агент-прод план приземлён в репо (агент = сервис + коннектор)

По итогам мультиагентного анализа конкурентов (hermes-agent/odysseus) + аудита полноты + Stage-2 design-спек: план разработки агента-как-сервиса теперь в репо как источник истины. Новое: **`docs/AGENT-PROD-PLAN.md`** (3-слойная модель base/agent-service/connector · роадмап PROD-v1→DEPLOY-2→owner-gated Фазы 2/3 · per-срез DoD с тестами/доками/ревью + **live-тест-слой агента против рига 24/7** · 6 P0-дыр · порт-backlog · деплой-автоматизация first-class), **`docs/THREAT_MODEL.md`** (P0-гейт, реализовано-vs-план размечено), **`docs/specs/agent-connect.md`** (ACP-совместимый протокол коннектора, decision-complete). `docs/BACKLOG.md` — раздел «Агент-прод». Подтверждено live: риг `192.168.0.31:8080` (Qwen3.6-27B-MTP, llama.cpp) отдаёт OpenAI tool-calling → агент-цикл работает на текущем железе, V100 не нужен.

### Плагины · Менеджер: включить/выключить + удалить (в корзину)

Бэкенд-управление плагинами по макету `plugins.jsx`. **Включить/выключить** — персист `plugins.<dir>.enabled` в `settings` (паттерн `episodic.enabled`, дефолт ВКЛ для обратной совместимости); `list_plugins` обогащает `PluginInfo.enabled`, выключенный плагин **не открывает сессию** (`plugin_open_session` отказывает). **Удалить** — каталог `.nexus/plugins/<dir>` → в корзину (`.nexus/.trash`, ОБРАТИМО, реюз `vault::move_to_trash`, не hard rm) + очистка настроек. Anti-traversal: имя каталога из IPC валидируется (один компонент, без `..`/разделителей). **Аудит** — уже был (broker `AuditLog` + permission-chips манифеста). Команды `set_plugin_enabled`/`remove_plugin`; фронт — тоггл-switch + trash в `PluginsPanel` (выключенная карточка приглушена, launch заблокирован), мок зеркалит контракт. **Marketplace отложен** — требует публичного egress (owner-gated). Тесты: `plugin::manage` (Rust), мок-контракт enable/disable/remove (фронт). Превью-верификация: toggle гасит/включает + launch-disable, remove убирает карточку.

### Редактор · Inspector-rail: «Похожие» + «Резюме» (живой контент вместо заглушек)

Заполняет отложенные секции инспектора (`editor.jsx`). **«Похожие»** (`RelatedNotes`) — семантически близкие заметки через существующий `get_related_notes` (max-sim по готовым векторам, дискавери-режим, без egress), с %-скором + сниппет-причиной, клик открывает заметку. **«Резюме»** (`NoteSummary`) — краткое LLM-резюме ТЕКУЩЕГО текста заметки: новая команда `get_note_summary` (one-shot, утилитарная модель `ai.fast`/`chat` через `GuardedClient`, текст заметки — ДАННЫЕ в анти-инъекционных маркерах AC-SEC-7, паттерн дайджеста). Запрос — по открытию секции и при смене заметки (НЕ на keystroke: текст из ref + race-guard); кнопка «Обновить» перегенерирует. Пустой ответ / нет модели → честная заглушка; ошибки → «не удалось». Мок зеркалит контракт. Тесты: `build_note_summary_messages` (Rust), `RelatedNotes`/`NoteSummary`/`InspectorRail` (фронт). Превью-верификация: обе секции рендерят живой контент. Убран плейсхолдер `inspector.aiSoon`.

### Редактор · InlineAI ⌘/ prompt-box — свободный note-grounded AI-запрос → вставка

Закрывает дельту gap-анализа: ghost-текст (`inlineGhost.ts`: continue/rewrite/summarize по выделению/курсору) уже был; не хватало **свободного prompt-box** из дизайна Qasr (`editor.jsx` InlineAI). Триггеры **⌘/** (в редакторе; шпаргалка хоткеев остаётся на ⌘/ вне редактора + в палитре) и слэш-команда **`/ai`**. Фазы ask→thinking→streaming→done, кнопки Вставить/Заново/Отмена, вставка блоком в позицию курсора активного редактора. **Бэкенд**: `InlineMode::Prompt` + `build_inline_prompt_messages` (запрос пользователя — доверенная инструкция БЕЗ маркера; текущая заметка-контекст — ДАННЫЕ в случайных маркерах, анти-инъекция AC-SEC-7); `inline_complete` получил опциональный `prompt`-параметр; утилитарная модель `ai.fast`, без RAG (D2). Мок зеркалит контракт. **Фронт**: `InlineAIBar` (+CSS-модуль, порт `app.css .inline-ai`), стор `inlineAI` (одна активная группа), ⌘/-keymap в `Editor` (новый `groupId`-prop), `/ai` в slash-реестре, i18n ru/en, закрытие на смене вкладки. Тесты: parse/builder (Rust), `InlineAIBar` flow + `/ai` слэш (фронт). Превью-верификация end-to-end: ⌘/→стрим→Вставить вставляет в курсор и закрывает бар.

### Дизайн · QASR-finishing: theme-completion (Mermaid/граф по всем 13 темам) + cleanup

Закрывает отложенные из QASR-0 пункты + QASR-cleanup. **Канон `DARK_THEMES`/`isDarkTheme`** в `stores/theme.ts` (9 тёмных: dark/midnight/platinum/mocha/nord/tokyo/rose/contrast/bronze; синхрон с `color-scheme` в tokens.css). **MermaidDiagram** теперь рендерит `dark`-mermaid на ВСЕХ тёмных темах (было только dark/midnight → 7 новых тёмных тем Qasr давали светлый mermaid). **graph.css** `--g-tag-l: 0.72` распространён на все 9 тёмных тем (было 3 → теги-узлы были тусклыми на новых тёмных). Drift-guard-тест: каждая из 13 тем классифицирована ровно один раз (новая тема обязана попасть в dark/light). **QASR-cleanup**: dead-old-design скан — миграция заменяла токены/стили in-place (не аккретила), осязаемого мёртвого старого дизайна НЕТ (нет old-theme-имён/legacy-vars/orphan-классов). test-all зелёный. **Дизайн-миграция Qasr ПОЛНОСТЬЮ завершена** (foundation+shell+все вью+theme-completion+cleanup).

### Редактор · editor-chrome: back/forward-кнопки + AppendLine + Inspector-rail (фичи макета Qasr, фронт)

Дотянут редактор до макета `editor.jsx` (фронт, без LLM/бэкенда; переиспользует существующую инфру). **Back/forward-кнопки** в таб-стрипе (ChevronLeft/Right) — привязаны к СУЩЕСТВУЮЩИМ `navBack`/`navForward` (NAV-3), disabled на границах истории (`navIndex`), хинты ⌘[/⌘] (только UI, nav-логика не дублирована). **AppendLine** (`components/editor/AppendLine.tsx`) — однострочный quick-add внизу превью (edit-режим): Enter дописывает строку через СУЩЕСТВУЮЩИЙ буфер (`updateBufferDoc`→dirty→autosave, без нового бэкенда); `[[` → автокомплит вики-ссылок через тот же `vault.listNotes`, что и CM6-редактор. **Inspector-rail** (`InspectorRail.tsx`) — правый вертикальный rail с 4 тогглами: outline→существующий `OutlineBar`, backlinks→`BacklinksBar` (обёрнуты, не переписаны), related/summary — структура + плейсхолдер «Нужен AI — скоро» (БЕЗ LLM — контент в InlineAI-срезе; тест запрещает suggest-вызов). **Behavior-change** (дизайн-faithful): outline/backlinks больше не всегда-видимые нижние бары — переехали под тогглы rail (collapsed по умолчанию), как в макете. i18n ru/en. **883 теста** (+16: back/forward-границы+клики, AppendLine append+[[автокомплит, rail-тогглы+no-suggest), 0 хардкод-цветов, test-all зелёный. Превью-подтверждено: back/forward слева от табов, Inspector-rail (4 иконки), AppendLine скрыт в reading-режиме. InlineAI(⌘/) + related/summary-LLM — следующий срез.

### Агент · UI-1b: фронт вкладки Агента (AgentView) — UI-1 ЗАВЕРШЁН

Фронт-половина UI-1 на контракте UI-1a → **вкладка Агента готова end-to-end** (бэкенд+фронт). `components/agent/AgentView.tsx`+`.module.css` по дизайну Qasr (`agent-view.jsx`, токены, 0 хардкод-цветов): шапка (session/model/autonomy/perms + контекст-бар из `contextUsage`) · лента шагов (assistant-токены стримом + раскрываемые tool-call/result + дифы) · **Changeset** (из `proposal`/`diff`: per-file +/−/status, apply/reject + bulk «Применить все»/«Отклонить» → `decisions[]`→`agent_approve`; auto-режим — бейдж без аппрува) · композер (`agent_run`) · правый dock (Plan/ResearchGraph — демо-структура с пометкой «события позже»; Report из `final`) + rail. **Store** `stores/agent.ts` (zustand: run-state + стрим-аккумулятор по `Channel.onmessage` + epoch-guard поздних событий; экшены run/approve/pause/resume/cancel/undo). **decisions fail-closed**: rejected И undecided → `approve:false` (зеркалит backend «missing=Reject»). Навигация: activity-bar Bot-иконка + команда `view.agent` + i18n (ru/en). `lib/tauri-api.ts` agent.*-обёртки + типы. **Браузер-мок** `lib/mock/agent.ts` ТОЧНО зеркалит контракт UI-1a (порядок/формы событий, id-корреляция, confirm-блок на approve = fail-closed, auto без proposal) + dedicated mock-mirror-гейт. **867 фронт-тестов** (+10 agent: рендер на мок-стриме, changeset-decisions, fail-closed, мок-зеркало); i18n паритет; найден+починен a11y-баг (per-file vs bulk reject одинаковый aria-name). Превью-подтверждено вживую: композер→мок-стрим→лента (fs.read)→changeset (3 файла +15−1, apply/reject+bulk)→Plan-dock; рендерится в дизайне Qasr. Бэкенд/actuator/egress НЕ тронуты. **UI-1 (a+b) ЗАВЕРШЁН.**

### Агент · UI-1a: бэкенд вкладки Агента в desktop (стрим + tauri-команды + UI-DecisionSource)

Подключён bounded agent-loop (AGENT-1..6) в desktop-приложение — БЭКЕНД-половина UI-1 (фронт AgentView — UI-1b). **`commands/agent.rs`**: контракт **`Channel<AgentStreamEvent>`** (serde, tag `type`, camelCase — все 8 `AgentEvent`-вариантов: assistantToken/toolCall/toolResult/contextUsage/proposal{files+actionId,runId}/diff/final/error; `_=>None` форвард-совместимость) + 6 tauri-команд: `agent_run(task,autonomy,channel)→run_id` (спавнит `run_agent_loop`, форвардит события в Channel в реалтайме — **закрывает FIXME(UI-1)**, где agentd только логировал), `agent_approve(run_id,decisions)`, `agent_pause/resume/cancel`, `agent_undo(run_id)→count`. **UI-driven `DecisionSource`** (mpsc, кормится `agent_approve`) вместо headless `PolicyDefault`: гейт эмитит Proposal/Diff → фронт видит changeset → блок на `decide()` → аппрув; **fail-closed** (нет ответа → `reject_all`). **State-реестр** прогонов (`run_id→{decision-sender, paused, cancel}`, регистрация до spawn, очистка на терминале). Композиция зеркалит agentd (build_agent_tool_provider, tokenizer/budget, recall, RunCtx); **actuator DEFAULT-OFF сохранён** (флаг отсутствует/false → стабы, vault не тронут), **новых egress-путей нет** (GuardedClient::for_chat переиспользован), gate не обходим (`apply_action` остаётся `pub(in crate::actuator)`). Вынесен **общий `nexus_core::ai::tools::build_agent_tool_provider`** (реальное переиспользование, не дубль; I-5/check-tooluse соблюдён). **Adversarial-ревью (3 линзы: actuator-safety / contract-lifecycle / egress-reuse) — 0 блокеров**; fold: `AGENT_PREAMBLE`+`RECALL_BUDGET_TOKENS` экспортированы из ядра pub (единый источник, убрана drift-копия). 185+ desktop-тестов (смоук-цикл против стаб-провайдера, маппинг 8 вариантов, approve-applies/no-approve-fail-closed), test-all зелёный.

### Дизайн · QASR-views (Conflict): перестройка резолвера конфликтов под макет Qasr

Перестройка ConflictResolver под макет (`conflict.jsx`) — design-layout, логика разрешения сохранена. **Grid** (`1fr 288px`; areas header/body/rail/foot). **Правый рейл** (288px, скролл): **stats-боксы** (local-edits accent / remote-edits link — счёт «changed lines per side» через set-diff ours/theirs vs base из реальных `GitConflictFile`-данных, не плейсхолдер), **навигатор** (jump-список «Конфликт N» + путь + status-dot, клик→скролл+flash к секции), **bulk-кнопки** (перенесены из body в рейл, вертикально, цветные свотчи). **Футер** с прогресс-баром (resolved/total) + Cancel/Apply (перенесены из body; Apply gated на allResolved). **Header** с warning-иконкой в 40px-боксе + subtitle на 2-й строке. **Логика разрешения БАЙТ-ИДЕНТИЧНА** (per-file ours/theirs/both/manual, pickAll, apply→`git.resolveConflicts`, merge-preview-фазы) — файловая модель сохранена (document-segment-модель макета НЕ внедрена). Tokens-only (искл. `#fff` на warning-иконке из макета). 4 теста (+1: рейл/stats/навигатор), i18n ru/en паритет, test-all зелёный. **Завершает чистый дизайн-ре-скин вью** (graph/ai-panel/sidebar уже совпали; home/news/sync/insights/worklists/palette полишены; settings/plugins/conflict перестроены).

### Дизайн · QASR-views (Plugins): перестройка под макет Qasr (design-only)

Перестройка вида плагинов под макет (`plugins.jsx`). **Design-only** — бэкенд плагинов имеет лишь `list_plugins`+session/invoke (нет enable/remove/marketplace), поэтому строится ТОЛЬКО раскладка на существующих данных, фейковые контролы НЕ добавлены. **Left-nav** (220px: «Установленные» + «Журнал доступа» + privacy-nav-note с shield) вместо inline-табов; grid head/nav/main. **3-частная карточка**: glyph 44×44 + body (name/version/sandbox-badge/perm-chips/consent) + side (существующая launch-кнопка). **Sandbox-badge** (mono shield «sandbox») на каждой карточке (author опущен — нет поля в `PluginInfo`). **Audit** вынесен в отдельный нав-таб (те же broker-call данные, новое размещение). Sandbox-launch (iframe-mount, consent-sheet, onCall→audit) полностью сохранён — сменился только триггер (`tab==='sandbox'`→`running`). **ФЛАГНУТО как needs-backend** (НЕ построено, фейков нет): enable/disable toggle, remove, marketplace/browse, runtime-permission-toggle-panel. 4 plugins-теста обновлены под новую раскладку, 0 хардкод-цветов, test-all зелёный.

### Дизайн · QASR-views (Settings): перестройка модалки настроек под макет Qasr

Крупнейшая per-view перестройка (7 hi/med-дельт), вся функциональность сохранена (save AI / test-connection / theme/accent/density / hotkeys edit-reset / about). **Модалка → grid** (`210px 1fr` / `head head` / `nav main`) с полноширинной **шапкой** (иконка-бейдж accent-soft + заголовок + close — close вынесен из абсолюта). **Theme picker → grid карточек**: вместо ряда текст-кнопок — 13 `.themeCard` с визуальным свотч-превью (реальные bg/text/accent темы + dot) + название; selected = accent-border+glow; превью-карта `THEME_PREVIEW` зеркалит data-theme-токены (без переключения темы документа); выбор зовёт тот же `setTheme` (превью-подтверждено: 13 карточек, Светлая selected). **AI-эндпоинты → `.modelCard`** (head иконка+title+desc, поля, inline test-кнопка 32px + status-бейдж; TestBadge-состояния сохранены). **Section title+subtitle** (`SectionHeader`); вложенные egress/web с разделителем. **About → центр-колонка** (BrandMark 56 + headline-имя + mono-версия + vault-meta) вместо dt/dd. nav-title без uppercase; accent-свотч 50%→7px rounded-square; row/hotkey label → text-md. Badge/status-цвета переведены на токены (--color-ai/success/warning/danger). **854 теста** (+2: theme-card выбор, about), 0 хардкод-цветов (кроме 3 декоративных из макета). Опущено (нет данных): provider-badge, about-ссылки.

### Дизайн · QASR-views (2): CSS-полиш батч (7 вью к макету Qasr)

Точечный полиш существующих вью под макет Qasr (только restyle, без новых фич/перестроек; питается токенами QASR-0). Основан на параллельном per-view дифф-анализе (13 вью; graph/ai-panel/sidebar уже совпали — не тронуты). **home**: `.greeting` → Cormorant headline 600 (превью-подтверждено: computed Cormorant/600/30px), heat-legend 3-частная раскладка. **news reader**: `.readerBar` sticky, `.readerTitle` Cormorant clamp(40–54px), новый mono-аптркейс `.readerKicker` (источник, акцент). **sync**: бордюр списка изменений, `.remote` dashed-box, единый `.syncStatus` с состояниями (success/warning/danger токены). **insights** (digest/goals/contradictions): `.iconBox` 34px у заголовка (3 модалки), унифицированный pill-`genBtn` 30px (goals — read-only, без кнопки). **worklists** (tasks/inbox): priority-бейджи с тинт-фоном (danger-soft/warning-mix/surface), inbox action-кнопки fade-in на hover/focus-within. **palette**: ширины 620/560px, паддинги input-row. **graph**: `flex:0 0 auto` для colorby/search. 15 файлов, 0 хардкод-цветов (только var(--…)/color-mix), test-all зелёный, 0 сломанных тестов.

### Дизайн · QASR-views (1): brand-completeness sweep + дискавери вью

Старт фазы QASR-views. **Дискавери** (превью :1432, demo-vault): после QASR-0 (токены) + QASR-shell (бренд) приложение УЖЕ полностью рендерится в дизайне Qasr — титлбар «Qasr» + лого-крепость, кремовый сайдбар/activity-bar, Cormorant-заголовки («Доброе утро»), статусбар; вью питаются токенами → дальнейший QASR-views = per-view ПОЛИШ против макета, не пересборка. **Brand-completeness** (добиты пропуски бренда вне титульного среза): `index.html` `<title>` Nexus→Qasr (doc-title вебвью — QASR-shell менял только нативный заголовок окна); футер командной палитры `Nexus`→`t('app.name')`; demo-данные Home (`Nexus MVP`→`Qasr MVP`, путь проекта) — Qasr-приложение больше не показывает «Nexus»-проект в демо. Код-идентификатор `remarkNexus` оставлен (внутреннее имя, как crate-имена). Превью: shell + Home рендерятся как Qasr корректно.

### Дизайн · QASR-shell: ребренд Nexus → Qasr (user-facing бренд)

Второй срез эпика QASR — **user-facing ребренд** на «Qasr» (قصر — дворец/цитадель). **Логотип**: марк Nexus («созвездие» из 4 узлов) → Qasr **«крепость из узлов»** (треугольная цитадель + узлы графа) — портирован SVG из дизайн-хендоффа в `BrandMark.tsx`. **Вордмарк** «Nexus» → «Qasr» через единый источник `app.name` (i18n) + теперь в display-шрифте бренда (`--font-display`; Cinzel + просторный трекинг на антик-темах bronze/marble). Все user-facing «Nexus» → «Qasr» (i18n en/ru — 7 строк/файл вкл. welcome/enter/welcomeBody/digest; onboarding `<h1>` и About → `t('app.name')`). **App-иконки**: полный Tauri-набор перегенерирован из Qasr-1024 (`tauri icon`: 32/128/128@2x/icns/ico + Square*/Store/android/ios). **Заголовок окна** → «Qasr». **СОХРАНЕНО** (анализ-решение, как repo/crate-имена): `productName`/`identifier` = `Nexus`/`app.nexus.desktop` (смена ломала бы путь appdata + CI bundle-smoke = инфра, не визуал); технические `.nexus/`-пути конфиг-каталога не тронуты (флипнут только заглавный бренд). Тесты-ассерты бренда обновлены (App.test enter-кнопка, ChatView ask1-пилюля). Превью: онбординг рендерит Qasr-логотип + вордмарк корректно. Хром (Titlebar/ActivityBar/StatusBar) структурно уже совпадал (DP-эпик) + питается токенами QASR-0 → авто-актуален.

### Дизайн · QASR-0: фундамент новой дизайн-системы (токены 13 тем + Cormorant/Cinzel + motion)

Первый срез эпика **QASR** (миграция дизайн-системы на новый бренд Qasr; этот срез — ТОЛЬКО фундамент, ребренд/вью — следующие срезы). Текущий фронт уже нёс прошлый Hermes-дизайн на ТОЙ ЖЕ системе переменных → миграция аддитивна. **Шрифты**: + **Cormorant** (`--font-headline`, ≥24px) + **Cinzel** (`--font-display`, антик-темы) через `@fontsource` (pnpm; Onest/Source-Serif/JetBrains уже были); подтверждено в бандле (30 cormorant + 8 cinzel файлов, вкл. кириллицу). **Токены** (`styles.css`): **13 тем** (light cream — основная, dark, midnight, platinum, paper, mocha, nord, tokyo, rose, sepia, contrast/AMOLED, bronze, marble) + 4 акцент-пресета (amber/teal/sage/clay), новые `:root`-vars (`--font-headline/-display/-serif`, `--display-*`, `--motion-*`), grain-оверлеи (paper/marble/bronze), радиусы 8/8/14, oklch-цвета. App-специфика СОХРАНЕНА (`#root`, `html.theme-anim` кроссфейд, `.nexus-print-root`+`@media print`). **Theme-store** расширен 4→13 (cycle/persist покрывают все; `Titlebar.themeIcon` получил `default`-фолбэк, иначе кнопка пустела на новых темах). **Motion** (`motion.css`): новые eases/keyframes (m-rise/pop/fade/slide/tabin) + tactile-press/hover-lift — применены ТОЛЬКО к реально-глобальным классам (`.gt-chip`, `.graph-view`, `.brand-thinking`); модульные пропущены (без мёртвого CSS, их доберут вью-срезы). i18n-метки 13 тем (ru/en паритет). **852 фронт-теста**, tsc/eslint/vite зелёные. Превью-верификация (родитель): light/dark/bronze отрендерены корректно — cream/charcoal/obsidian фоны, Ember/bronze акценты, Cormorant-заголовок, bronze grain-glow, 0 ошибок консоли.

### Агент/Инференс · engine-agnostic инференс через конфиг + cold-start-таймауты (INFER-CFG)

Смена LLM-движка (llama.cpp → 1Cat-vLLM Qwen3.6-27B-AWQ на V100 → любой OpenAI-совместимый) — теперь **правка `.nexus/local.json`, без кода**. **Cold-start-таймаут расщеплён** (`ai/tools.rs`+`ai/chat.rs`): хардкод `STREAM_IDLE_TIMEOUT=90s` применялся и к инициации, и к каждому чанку → убивал прогрев V100 (первый токен через 1–3 мин). Теперь `first_token_timeout` (дефолт **300с**) действует на `send` + чанки **до первого байта** (стейт `got_first_byte`), затем `idle_timeout` (дефолт **90с**) — на разрывы в steady-state; билдеры `with_first_token_timeout`/`with_idle_timeout`. **Context-fallback** 8192 → **32768** + warn (256K НЕ хардкодится — только из `context_window`). **Конфигурируемо** (опц. поля + геттеры с дефолтами, zero-config совместим): `ChatConfig{first_token_timeout_secs, idle_timeout_secs, connect_timeout_secs(30), retry_attempts(3), temperature(0.3), reserve_output_tokens}`, `EmbeddingConfig{timeout_secs(60)}`. `GuardedClient::for_chat/for_embedding` берут таймаут параметром (раньше хардкод 15/60с). **Проводка во ВСЕ composition-roots**: headless `nexus-agentd` (tool+chat+fast+embed) и desktop (`build_chat`/`build_util_chat` + хот-апплай `set_ai_config`) — единый `apply_chat_cfg`-паттерн + `temperature` из конфига. tool-call wire-формат (OpenAI `tools`/`finish_reason`) НЕ тронут (совместим с `qwen3_coder`); `api_base()` `/v1` сохранён. **659 тестов core / 7 agentd** (+1). Дока: `docs/dev/settings.md` — пример свап-профиля на целевой сервер. (Стрим-стейт-машина покрыта code-review + live-тестами `#[ignore]`; чистого юнита нет из-за async/reqwest-связки.)

### CI · Windows-гейт снова осмысленный — nexus-desktop исключён из тест-прогона на Windows

Windows-нога матрицы `Rust (build · clippy · test)` падала на КАЖДОМ коммите в `main` (пред-существующий инфра-долг, не регрессия среза): `cargo test --locked --workspace` проходил `nexus-core` (631 тест) и `nexus-agentd` (7), затем **умирал на загрузке** lib-тест-бинаря `nexus-desktop` с `0xC0000139 STATUS_ENTRYPOINT_NOT_FOUND` — Tauri-app крейт линкует `WebView2Loader`/webkit-нативщину, и одна из DLL на headless-раннере присутствует, но не экспортирует импортируемую точку входа (сбой ДО запуска любого теста, не логики-баг). Фикс: на `windows-latest` тест-прогон сужен до портативных крейтов (`cargo test --workspace --exclude nexus-desktop`), при этом `cargo build --workspace` + `cargo clippy --workspace -D warnings` на Windows **остаются полными** (desktop-крейт по-прежнему компилируется и линтуется на Windows — пропускается ТОЛЬКО исполнение его незагружаемого тест-бинаря). ubuntu/macOS гоняют ПОЛНЫЙ набор воркспейса, включая ~217 desktop-тестов. Зелёный Windows-чек снова что-то значит.
### Агент · Фаза 1: вендоринг kepano + capability/trust-gate (SKILL-3)

Связывает **declared-capabilities** скилла с run-policy и поставляет MIT-набор kepano-скиллов hash-pinned. Ключевой инвариант: **декларация ЗАПРАШИВАЕТ, не ГРАНТИТ.** **Capability-модель** (`skills/capability.rs`): типизированный `enum Capability {VaultRead,VaultWrite,WebFetch,WebPost,Shell,HostProcess,Unknown}` (case-insensitive parse + алиасы; нераспознанное → `Unknown`, не ошибка), `resolve_capabilities(declared, _tier, run_policy)` → `{granted, inert:[(cap,reason)]}` где **`granted = forced_base() ∩ run_policy`** (`forced_base`=`{VaultRead,VaultWrite}` хардкод; `declared` НИКОГДА не расширяет granted — только формирует `inert`). Инвариант (тест-параметризация по обоим tier + злонамеренная all-dangerous policy): Shell/Web*/Host **НИКОГДА** не granted в Фазе C. **Trust-tier** `{TrustedLocal, Vendor}` из rel_path (`vendor/` префикс) — **advisory**: захвачен + сюрфейсится, но **НЕ проведён** в classify/decision/orchestrate (актуатор байт-идентичен; `approval_default` — чистый Phase-3-seam, не вызывается живым гейтом, инвариант destructive/egress/host→никогда-Auto). **Discovery** расширен на вендоренную раскладку `vendor/<bundle>/<skill>/SKILL.md` (BOUNDED, без рекурсии; path-scope SKILL-1 сохранён на каждом уровне: symlink-skip + canonicalize-starts_with бэкстоп; имена компонентов валидируются как traversal-free в источнике). **Вендоринг-валидация** (`validate_vendored`, serde_json, НЕ serde_yaml): vendored-скилл грузится ТОЛЬКО если (a) `<bundle>/vendor.lock` парсится, (b) bundle-`license` непуст, (c) sha256(SKILL.md)==пин И запись существует — иначе жёсткий `SkillError` (в `errors`, НЕ загружен); хэш по уже-прочитанному контенту (без TOCTOU); TrustedLocal не hash-pinned. **activate_skill** доклеивает к фенсу advisory «Доступно … / Заявлено-но-ИНЕРТНО … (причина)» (AC#2 — не молчаливый no-op). **Вендоренный bundle**: `_skills/vendor/kepano/` (obsidian-markdown + json-canvas, MIT, pin commit `a1dc48e`, LICENSE+PROVENANCE+vendor.lock). **658 тестов core / 7 agentd** (+34). Adversarial-ревью (3 линзы: capability-escalation / vendoring-integrity / path-scope-регрессия) — 0 эксплойтов: granted доказуемо vault-only, tier не влияет на гейт, актуатор не тронут, вендоринг fail-closed; folds (defense-in-depth): валидация имён компонентов + fail-safe-тест регистра `Vendor/`. Без обхода capability (shell/web/host структурно инертны — нет ActionTarget), без skill→fact propose (owner-gated OFF), без UI (UI-1). **Cross-platform**: `.gitattributes` помечает `_skills/vendor/** -text` — вендоренные файлы байт-идентичны на всех ОС (иначе Windows-checkout `core.autocrlf` LF→CRLF ломал бы hash-pin; поймано вернувшимся Windows-CI).

### Агент · Фаза 1: активация скиллов — 3-tier disclosure (SKILL-2)

Проводка каталога SKILL-1 в агента по схеме **progressive disclosure** (меню → инструкции → ресурсы), весь контент скилла — **недоверенные ДАННЫЕ (I-5)**: фенсен per-request `injection_marker`, роль `user`/`tool`, НИКОГДА `system`. **Tier 1** — `SkillCatalog::catalog_block`: фенсенное меню (только `name`+`description`, НЕ тело), бюджет `CATALOG_MAX_ENTRIES`=50 / `CATALOG_DESC_MAX_CHARS`=200 (UTF-8-усечение + явная «…ещё N скиллов»), description сворачивается в одну строку (control→пробел, анти-разметка-спуфинг); инжектится в `drive()` как `ChatMessage::user` после recall, до задачи. **Tier 2** — `activate_skill` (`agent/skill_tools.rs`): `spec().parameters.enum` = live-имена каталога, `invoke` строго парсит `{skill}` (`deny_unknown_fields`) + **re-валидирует** через `catalog.get` → off-enum → fail-closed `UnknownTool`; возвращает ТЕЛО фенсенным. **Tier 3** — `read_skill_resource`: `resolve_skill_resource` конфайнит в подкаталог скилла (зеркало `vault::resolve_vault_path` — reject absolute/root + двойной `canonicalize`+`starts_with` на skill_dir и на resource → ловит symlink-наружу/`..`/sibling; backstop skill_dir⊆skills_root), read-only, cap 64 KiB, фенсен. Проводка: `ai.agent_skills_dir` (Option, default None; relative→от vault); `SkillContext{catalog,skills_root}` в `AgentRunHandler` — `Some` → меню+2 тула в per-run registry (независимо от actuator-флага, скиллы read-only), `None` → без регрессии (AGENT-2/MEM-1). **Capability-инертность**: активация = ТОЛЬКО текст-инструкция, НЕ регистрирует тулзы и не даёт прав (`capabilities` остаются захвачены, но инертны → enforcement в SKILL-3); имя скилла не становится ключом registry (нет hijack). **624 теста core / 7 agentd** (+26). Adversarial-ревью (3 линзы: path-escape / I-5-инъекция / корректность) — 0 блокеров: маркер 96-бит per-request (нельзя предугадать/закрыть фенс), 0 путей в `system`, конфайн ресурсов VERIFIED. Без вендоринга kepano + без capability-ENFORCEMENT (SKILL-3), без UI (UI-1).

### Агент · Фаза 1: SKILL.md загрузчик (SKILL-1)

`nexus-core::skills` — discovery + parse + validate + каталог SKILL.md (open-standard agentskills.io / kepano). `parse_skill`: frontmatter `name`/`description` **БЕЗ serde_yaml** (зеркало parser-edge-stripper; **kepano-совместимо** — `metadata.nexus.*` не требуется); malformed (нет name/description / битый frontmatter / небезопасное имя [`/`,`\`,`..`,control,>128]) → **HARD `SkillError`** (не проглатывается). `discover_skills` → `SkillCatalog`: **path-scope dual-layer** (`symlink_metadata`-skip + `canonicalize`-`starts_with`-backstop → НЕТ загрузки вне skills_dir; 3 path-escape теста вкл. real-dir-symlinked-out), single-def (duplicate name → `DuplicateName` видим), malformed-visible (per-skill ошибки в `catalog.errors`, не молча; dir без SKILL.md = «не скилл», единственный тихий skip). Capabilities (`capabilities`/`allowed-tools`, list-форма) захватываются БЕЗ enforcement. **598 тестов** (+32). Скептик-ревью: SHIP, 0 блокеров (path-scope secure / fail-closed / no-serde_yaml / kepano / boundary VERIFIED). Без активации/инъекции/тулз (SKILL-2), без вендоринга/capability-gate (SKILL-3), без agent-loop проводки.

### Агент · Фаза 1: приватность — content-free diff_summary + Windows-hardlink-защита (AGENT-6)

Две приватность/safety-обвязки актуатор-аудита. **Content-free `diff_summary` + редакция-гвард.** Долговечная колонка `agent_actions.diff_summary` теперь ЗАПОЛНЯЕТСЯ — но ИСКЛЮЧИТЕЛЬНО структурной формой `"+N -M (new|edit)"` через новый тип `audit::DiffSummary` (поля — ТОЛЬКО `u32`-счётчики + enum `ChangeKind`; **нет ни одного String-поля → передать сырой текст заметки через него НЕВОЗМОЖНО по построению**). Единый источник `orchestrate::diff_summary_for` (счётчики от `line_diff`, как в 3d-Diff) переиспользуют ОБА писателя — auto-apply (`apply.rs`, был `None`) и propose-путь (`orchestrate.rs`, был сырой `format!`); ни один не пишет тело/значения frontmatter/хунки. **Аудит ВСЕХ TEXT-колонок** долговечной строки: `diff_summary` (структурный), `outcome` (статус-сообщение `success_summary` — путь + ИМЯ ключа frontmatter, НЕ значение/контент), `target_rel` (путь — нужен для undo/аудита, не контент), `idempotency_key`/`content_hash` (blake3, one-way). Mandatory-тест `secret_content_never_lands_in_durable_ledger`: применяем NoteEdit с `SECRET-TOKEN-123` → сырой SELECT ВСЕХ десяти TEXT-колонок → ни одна не содержит секрет; `diff_summary == "+N -M (edit)"`. **Windows hardlink-защита (ДОБАВЛЕНА).** `confine_for_overwrite` получил `#[cfg(windows)]` рубеж 3 — зеркало unix `nlink>1`: `nNumberOfLinks > 1` через `GetFileInformationByHandle` (std не даёт переносимого link-count) → `PathEscape`. Закрывает Windows info-leak-щель (раньше check был ТОЛЬКО под unix → пред-существующий хардлинк наружу читался бы в снапшот/дифф). `windows-sys` 0.61 — `[target.'cfg(windows)'.dependencies]`, УЖЕ транзитивно в Cargo.lock (без build-script, на `windows-link`) → новой/тяжёлой деки в дерево нет; features `Win32_Foundation`+`Win32_Storage_FileSystem`. `#[cfg(windows)]` тест-зеркало unix-кейса. leaf-симлинк-reject + `resolve_vault_path_for_write` + rename-семантика atomic_write остаются.
- UI-1: **live-editor dirty-buffer → CONFIRM** — когда агент правит заметку, открытую в десктоп-редакторе с НЕсохранёнными изменениями, действие обязано ПРЕДЛОЖИТЬ (не auto-apply), чтобы не затереть несохранённый буфер. Это UI-1/desktop-скоуп: headless agentd НЕ имеет живого редактора, а решение требует состояния dirty-буфера десктоп-редактора (его в ядре/agentd нет). Кода в AGENT-6 нет — фиксируем границу.

### Агент · Фаза 1: kill-switch + anti-fatigue token-bucket (AGENT-5)

Две safety-обвязки агента. **Kill-switch `agent_paused`** (`Arc<AtomicBool>`) — fail-safe на 3 слоях: `handle()` до старта (paused → прогон остаётся `queued`, delayed re-enqueue → возобновление на un-pause), `run_agent_loop` (проверка КАЖДЫЙ шаг рядом с `cancel` → `BudgetKind::Paused` → requeue, НЕ terminal), actuator dispatch (оба write-пути под `!is_paused()` + **last-moment guard в `apply_now` перед atomic_write** → paused НИКОГДА не пишет; единственный agent-write-путь покрыт). Persisted `agent.json` (ядро `agent::control`, config-dir, atomic, default not-paused fail-safe) + SIGUSR1-тоггл (unix, без deps). **Token-bucket anti-fatigue** (заменил `BlastRadius`): capacity (=`blast_radius_cap`) + time-refill, **claim-before-apply `compare_exchange`** (concurrency-safe: 16×20 на cap 50 → РОВНО 50; refill без double-credit; refund capped at capacity), Clock-seam (Manual/Monotonic — без `Instant::now()` в тестах) — за бакетом Auto-тир форсирует propose. bundle-id config-dir → ОБЩИЙ const (egress+agent, против десинхрона headless↔desktop). + фикс флейка `recall_drops_low_priority` (детерминированный gap, ассерты сохранены/усилены, 5× green). **563 core + 7 agentd тестов** (+24). Adversarial-ревью (3 скептика→судья, спот-чек): SHIP, 0 блокеров (paused-no-write на всех слоях / token-bucket hard-equality под конкуренцией / persist fail-safe / flake-детерминизм — VERIFIED).
- UI-1: пауза-кнопка + runtime-swappable `DecisionSource`; gate undo-триггера под `agent_paused` (`undo.rs` — user-initiated write, НЕ под автономией агента; решение при UI-1). TOCTOU одобренного пути сжат last-moment guard'ом (суб-мс остаток присущ любому флаг-свитчу).

### Агент · Фаза 1: обратимость — undo-движок (AGENT-4)

`actuator::undo::undo_run(run_id)` откатывает прогон агента: проходит applied-действия в ОБРАТНОМ порядке (newest-first) и восстанавливает каждое по `UndoHandle` — `Snapshot{rel,ts}`→restore из `.nexus/history` (revert edit/frontmatter), `Trash`→`move_to_trash` (un-create). **НЕразрушающий**: перед каждой restore-перезаписью снапшотит ТЕКУЩИЙ контент (`history::snapshot manual=true`) → пост-прогонная правка человека всегда восстановима из history; **fail-closed** (не перезаписывает без recovery-point). Restore идёт через ОБЩИЙ rampart `confine_for_overwrite` (извлечён из apply, переиспользуется undo: canonicalize + leaf-symlink-reject + hardlink `nlink>1`-reject) — restore НИКОГДА не пишет вне vault. Идемпотентно (новый `ActionState::Undone`, `executed→undone` fenced; **без миграции** — state TEXT; re-undo = no-op через `actions_for_undo` state-фильтр + fenced `mark_undone`), partial-tolerant (per-action `UndoStatus`), reverse-order корректен (v0→v1→v2 → undo → **v0**). `drifted`-флаг в outcome для UI-1. restore-хелперы `pub(crate)` под UI-1. **541 тест** (+14). Adversarial-ревью (3 скептика→судья, спот-чек): SHIP, 0 блокеров (restore-path-safety / идемпотентность+reverse / extraction без регресса apply 3c-рубежей — VERIFIED).
- UI-1: триггер undo (tauri-команда/кнопка) + drift-confirm перед undo (движок safe-by-default — overwrite восстановим, но UX-warn о правке человека желателен); `read_snapshot` требует pre-confined `rel` (контракт задокументирован).

### Агент · Фаза 1: актуатор GO-LIVE в agentd (safe-default) — AGENT-3e

Актуатор подключён к ЖИВому headless-агенту — но **БЕЗОПАСНО ПО УМОЛЧАНИЮ**. **Завершает эпик AGENT-3 (актуатор) end-to-end** — агент может действовать в реальном vault. Тулзы (`note.create`/`note.edit`/`note.set_frontmatter`) маршрутизируются ТОЛЬКО через autonomy-гейт `orchestrate::dispatch_action` — ungated 3c-путь удалён, `apply_action` сужен до `pub(in crate::actuator)` (**compile-time no-bypass**: попытка обхода = E0603, ноль раскрытых вызовов). `AgentRunHandler` строит реестр актуатора per-run (DispatchPolicy из autonomy прогона + overwrite_threshold/blast_cap из конфига + свежий per-run BlastRadius) ТОЛЬКО за флагом `agent_actuator_enabled` (`#[serde(default)]` → **false**; agentd `unwrap_or(false)`) — иначе stub-реестр, реальный vault не тронут. Headless agentd = `PolicyDefault` (**auto-DENY** всего Confirm; unattended не само-одобряет). EventSink = `TracingEventSink` (headless-лог; UI-стрим = UI-1, FIXME оставлены). **`egress.json` kill-switch восстановлен** (CORE-2a tail закрыт: agentd грузит `net::load_egress_state` из config-dir [`NEXUS_CONFIG_DIR`/`app.nexus.desktop`, контракт задокументирован] и применяет offline+per-feature ДО любого egress). Replay → AlreadyDone (без двойного apply). **Live-путь покрыт CI** (`#[tokio::test] live_actuator_gate_applies_via_gate`: flag-on + `autonomy=auto` → note.create через гейт → файл записан + ledger executed, офлайн). **527 core + 5 agentd тестов.** Adversarial-ревью (3 скептика→судья, спот-чек + живой прогон smoke, **go-live bar**): SHIP, 0 блокеров (no-bypass / флаг-off+PolicyDefault / live-safety-матрица / egress-restore — все VERIFIED); 4 recommended сведены инлайн.
- Поведение по умолчанию: agentd из коробки = stubs (флаг ВЫКЛ), реальный vault не трогается; при opt-in + `autonomy=auto` headless применяет ТОЛЬКО Auto-тир, Confirm всегда auto-DENY. UI-стрим предложений + runtime-DecisionSource → UI-1; kill-switch/anti-fatigue token-bucket → AGENT-5; undo-UX → AGENT-4; diff-redaction + Windows-hardlink → AGENT-6.

### Агент · Фаза 1: гейт автономии + предложения (Proposal/Diff + DecisionSource) (AGENT-3d)

Гейт, решающий APPLY vs PROPOSE по матрице `(RiskTier × autonomy)`. **Всё ещё НЕ проведено в живого agentd** (3e) → реальный vault не тронут (дисковые записи только в temp-vault тестов; `dispatch_action`/`DecisionSource` конструируются и тестируются здесь). Состав:
- **`AgentEvent::Proposal{run_id, files}` + `Diff{path, add, del, status}`** (`agent/event.rs`, `#[non_exhaustive]` сохранён) + `FileStatus{New,Edit}` + `ProposedFile{path, add, del, status, action_id}` → маппинг на CONTRACT-NOTES §«Changeset / предложения» (`{path, add:int, del:int, status:new|edit}`); сериализация тегированная (`type:proposal|diff`), составные имена явно camelCase (`runId`/`actionId`; `rename_all` контейнера НЕ каскадирует в struct-варианты enum); `state(pending|applied|rejected)` НЕ дублируется в событии (берётся из ledger-строки).
- **`DecisionSource`** (`actuator/decision.rs`): `PolicyDefault` — **fail-closed Reject-ВСЕХ** (unattended agentd НИКОГДА не само-одобряет Confirm) + `ChannelDecision` (mpsc-канал: тест/будущий UI/контрол-плейн кормит `BatchDecision`; закрыт/пуст → reject_all). `BatchDecision` fail-closed на ВТОРОМ рубеже: отсутствующий `action_id` → `Reject` (частичный ответ ничего не «протащит»).
- **Матрица диспетча** (`actuator/orchestrate.rs`): HardBlocked→`ToolError::Exec` всегда; Auto+auto-run+под-кэпом→apply-сразу (bump blast-radius); Auto+auto-run+ЗА-кэпом→**форс-предложение** (анти-усталость); Auto+confirm-run→предложить; **Confirm-тир при ЛЮБОЙ автономии (вкл. auto)→предложить+ждать решения — auto НЕ перекрывает Confirm**. Эмиссия Proposal+Diff (`EventSink`), `proposed→approved→apply` / `proposed→rejected` ledger-флоу (новые state-константы + `audit::transition` fail-closed на `state=from AND outcome IS NULL`; ключ предложения `propose:`-префикс ≠ apply-ключ → нет UNIQUE-коллизии/ложного CrashedMidExecute).
- **`classify_hash` ОБЯЗАТЕЛЕН** на ОБОИХ применяющих путях (auto-apply и approved-Confirm) → `apply_action(…, Some(classify_hash))` (3c hard-gate: drift Рубежа 3 ловится ДО снапшота); конвенция значения зеркалит apply (`content_hash(current)` / `""` для отсутствующего create). **`overwrite_threshold` ИЗ КОНФИГА** (`DispatchPolicy`), не 64KiB-константа.

**519 тестов** (+24): каждая ячейка матрицы (вкл. auto-не-перекрывает-Confirm + blast-cap→propose) + PolicyDefault-never-applies-Confirm + **drift-между-propose-и-approve→Failed без клоббера** + Proposal/Diff-shape↔CONTRACT-NOTES + config-threshold-respected + propose→decide→apply/reject ledger + transition fail-closed.
- Honest: единственные `apply_action(…, None)` вне тестов — 3c-tools.rs (не-живой шов, тулзы не зарегистрированы) → на ЖИВОМ 3d-пути hash всегда `Some`. Остаточно: `AlreadyDone`-replay в auto-режиме считается в blast-radius (минорная анти-усталость-бухгалтерия — полный token-bucket/TTL/kill-switch = AGENT-5); diff — простой line-count по multiset (хунки/усечение/redact = AGENT-6); один айтем на диспетч (батч-из-одного — мульти-айтемный батч UI собирает на стороне фронта).
- **⚠ HARD-GATES для 3e/AGENT-5 (из adversarial-ревью):** (3e) живая проводка ОБЯЗАНА маршрутизировать тулзы через `orchestrate::dispatch_action` (гейт), а НЕ через legacy `tools::dispatch` (его Auto-путь применяет БЕЗ autonomy/blast-radius → обход гейта) — удалить/перенаправить `tools::dispatch` direct-apply + e2e-тест «confirm-run Auto → предлагает, не пишет»; `EventSink` — нестабилен до 3e (там подключается к loop `on_event`). (AGENT-5) blast-radius check-then-bump атомарен лишь при single-writer → при конкуренции claim-before-apply (compare-exchange); + kill-switch `agent_paused` + token-bucket/TTL. **ЧЕКПОИНТ ВЛАДЕЛЬЦУ перед 3e** (включение актуатора на реальном vault).

### Агент · Фаза 1: file-актуатор — apply-механизм + тулзы (AGENT-3c)

Механизм записи актуатора в vault + первые side-effect-тулзы (`note.create`/`note.edit`/`note.set_frontmatter`, impl `agent::Tool`). **Диск-записи только в тестах** (temp-vault); актуатор НЕ зарегистрирован в живого агента (3e), autonomy/proposals — 3d → реальный vault не тронут. `apply_action` — строгий порядок гейтов: (1) **symlink/canonicalize-рубеж**: `resolve_vault_path_for_write` (канонизация родителя) + leaf-symlink-reject (`symlink_metadata`) + **symlink-safe `create_dir_all`** (walk-reject симлинк-компонентов → нельзя создать папки НАРУЖУ) + **hardlink `nlink>1` reject** (cfg unix) → пишем ТОЛЬКО по канон-`abs`, никогда `canon_root.join(rel)`; (2) read-current; (3) optimistic-concurrency drift vs `classify_hash`; (4) **ledger write-before-act** (`record_before` Executing/outcome=NULL, UNIQUE-key fence → `replay_decision`: AlreadyDone / CrashedMidExecute-hash-recheck / Fresh); (5) **snapshot-before-act** `history::snapshot(manual=true)` (обход 90с-троттла) → `UndoHandle` (abort если snapshot None/Err — никогда не-восстановимый undo); **(5b) re-read-before-write TOCTOU-fence** (drift→Failed без клоббера, безусловно — даже при classify_hash=None); (6) `atomic_write` / `set_frontmatter_field`; (7) `finish` absorbing. Тулзы: strict `deny_unknown_fields`→BadArgs (I-4); HardBlocked→ToolError, Auto→apply, **Confirm→proposed-НЕ-применено** (3d-seam). **495 тестов** (+21). Adversarial-ревью (3 скептика→судья, спот-чек, production-bar): SHIP, 0 блокеров (escape/no-bypass **CLEAN** — оба симлинка+hardlink+create_dir_all отклоняются; write-before-act+обратимость CORRECT; scope SEALED); все 4 recommended hard-gates сведены инлайн (+mutation-verified).
- Honest: Windows hardlink-info-leak gap на overwrite (`nlink` unix-only; mitig.: leaf-symlink+resolve+re-read+atomic-rename). **HARD-GATE 3d:** `classify_hash` обязателен на live changeset-пути; `overwrite_threshold` из конфига; diff_summary-redact → AGENT-6.

### Агент · Фаза 1: ядро актуатора — машина состояний + classify + ledger (AGENT-3b)

Логика + персист актуатора (**БЕЗ записей в vault / тулз / apply** — те в AGENT-3c+). `nexus-core::actuator`: типизированная action-алгебра (`ActionTarget` NoteCreate/NoteEdit/Frontmatter; shell/процессы/egress НЕвыразимы → HardBlocked by-construction), **ЧИСТЫЙ fail-closed `classify()`** (исчерпывающий match без catch-all-downgrade; path-confinement лексический: `../`/абсолют/backslash[фикс реального Unix-bypass `a\..\..\secret`]/любой dot-компонент → HardBlocked; NoteEdit>порога → Confirm; иначе Auto), 5-state машина (`can_transition_to` исчерпывающий, Audited терминал) + `UndoHandle`-scaffold, idempotency-ledger (мигр.**022** `agent_actions`, UNIQUE `idempotency_key`=blake3(run_id,tool,args,target_hash), `finish` absorbing fenced на `outcome IS NULL`, `replay_decision` ветвится по НАЛИЧИЮ outcome а не ключа → Fresh/AlreadyDone/CrashedMidExecute). **474 теста** (+28). Adversarial-ревью (3 скептика→судья, спот-чек): SHIP, 0 блокеров (no-downgrade / replay-on-outcome / стейт-машина CONFIRMED).
- **⚠ HARD-GATE для AGENT-3c (первые записи в vault):** classify лексический (symlink внутри vault наружу не видит) → `apply()` ОБЯЗАН прогонять КАЖДУЮ запись через `resolve_vault_path_for_write` (canonicalize/symlink-рубеж; `root` уже канонизирован — добавлено в docstring) ДО диска + тест «symlink-внутри→наружу = PathEscape»; `overwrite_threshold` брать из конфига (sensible default ~10KB, не 0/безлимит); diff_summary-redaction → AGENT-6.

### Агент · Фаза 1: RunCtx — per-call корреляция egress (AGENT-3a, гейт актуатора)

Закрыт блокирующий гейт AGENT-2: процесс-глобальный `EgressAudit.run_id`-слот (`set_run` + RAII `RunScope`) заменён на ЯВНО-ПРОБРАСЫВАЕМЫЙ `RunCtx { run_id: Option<i64> }` (`Copy`; конструкторы ТОЛЬКО `NONE`/`run`, без `Default` — выбор «без run-context» всегда явный/grep-аудируемый). `record`/`authorize`/`get`/`post_json` несут ОБЯЗАТЕЛЬНЫЙ `ctx: RunCtx` (компилятор-enforced — egress нельзя сделать без корреляции; `record`/`authorize` приватны). Протяжка end-to-end: `AgentRunHandler::drive` → `run_agent_loop` → `ToolCapableProvider::stream_chat_tools` → `post_json`. Non-run egress (chat/embed/probe/news/websearch) → `RunCtx::NONE`. **Конкурентные прогоны теперь аудит-корректны** (каждый run_id в своём стеке вызова, общего изменяемого слота нет). check-egress расширен (`checkRunCtxParams`). **Concurrent-тест доказан** (ре-эмуляция старого слота → кросс-тегирование/упал → на RunCtx проходит). Заодно убрано мёртвое поле `audit` из `AgentRunHandler`. **446 тестов**. Adversarial-ревью (3 скептика→судья, независимый прогон): SHIP, 0 блокеров (no-bypass / gate-rigor / non-breakage VERIFIED).

### Агент · Фаза 1: мост памяти — `AgentMemory` (recall 3 слоёв + Add-only) (AGENT-MEM-1)

Агент подключён к ТРЁМ слоям памяти Nexus. `nexus-core::agent::memory`: трейт `AgentMemory` (mockable) + `VaultAgentMemory`. `recall(query, budget)` собирает факты (MEM `context_facts`) + переписку (N4b `chat_log::search_memory`) + эпизоды (EP `search_episodes`) в ФЕНСЕНные user-блоки (`build_*_block` + per-request `injection_marker`, I-5), с `exclude_session` (без само-recall), бюджетом (`pack_within_budget` роняет слой ЦЕЛИКОМ по приоритету chat<episodes<facts, никогда не превышает окно/не рвёт фенс), graceful degrade (нет AI/векторов → пусто, не падает). `remember(text)` — **Add-only** (`memory::add` source="agent"; ни update/delete — консолидация это gated MEM-эпик). Проводка в `AgentRunHandler`: контекст = [system] + recall(task,1500) + [user] (memory=None → поведение AGENT-2, без регресса). agentd `build_rag_min` открывает все 4 вектор-индекса + гонит ядро-публичный `reconcile_embedding_model` (**закрывает CORE-2a follow-up #2**: stale `.usearch` под другой моделью/dim сбрасывается ДО открытия; change-detection не даёт ложный wipe; `.usearch` — производные артефакты → ноль потери исходных данных; нет flip-flop на общем desktop+agentd vault). Write-funnel grep-линт `check-agent-memory.mjs` (memory-мутаторы под agent/ только в адаптере; self-test; `\w*` намеренно широк — упреждает backdoor-хелперы). **445 тестов** (+10). Adversarial-ревью (3 скептика→судья, спот-чек): SHIP, 0 блокеров.
- Follow-up (gated на срез run↔chat-session-linking): протянуть resolved session_id в `exclude_session` (канал работает + протестирован, сегодня None безопасно — прогон не пишет chat/episodes).

### Агент · Фаза 1: долговечные прогоны — scheduler-джоба + `agent_runs` (AGENT-2)

Цикл AGENT-1 стал ДОЛГОВЕЧНОЙ запланированной джобой. Миграция **021 `agent_runs`** (`id` i64 = run_id, session_id, task, status[queued|running|done|error|cancelled], model, autonomy, outcome, step, ts) + run-store (create/mark_running/bump_step/finish_run/get/`requeue_stale_running`; absorbing «first-terminal-wins», terminal-guard на всех мутаторах). `AgentRunHandler` (kind `agent_run`): terminal→Ok-noop (replay-safe) → mark_running + `RunScope` (RAII `set_run(Some)`→Drop `set_run(None)`, reset на success/error/**panic**) → `run_agent_loop` (стабы) → finish_run. **Корреляция egress→прогон** через P0-b-scaffold `set_run` (audit-строки прогона несут run_id). Краш-recovery: `requeue_stale_running(TTL=30мин)` в старте agentd (двухслойно с job-level requeue, корректный порядок). Backpressure: `defer_under_interactive()` (глобальный гейт `run_due` откладывает, не дропает). agentd регистрирует хендлер + smoke (graceful degrade без AI). **435 тестов** (+14). Adversarial-ревью (3 скептика→судья, спот-чек): SHIP, 0 блокеров; «CRITICAL TOCTOU» опровергнут (атомарный гейт — job-layer `claim_next`+write-actor; agentd строго последователен).
- **⚠ БЛОКИРУЮЩИЙ ГЕЙТ AGENT-3:** `EgressAudit.run_id` — процесс-глобальный слот, корректен ТОЛЬКО при строго последовательных прогонах. До параллельных прогонов / actuator-web-egress заменить на явно-проброшенный `RunCtx` (ADR-009 §P0-b) + concurrent-run тест. Инвариант задокументирован на месте регистрации (agentd main).

### Агент · Фаза 1: tool-граница + bounded event-stream цикл (AGENT-1)

Фундамент нативного агента (ADR-009 D3) — «вторая половина действия». Новый крейт-модуль `nexus-core::agent`: `Tool` трейт + `ToolRegistry` (fail-closed на unknown/bad-args, I-4), bounded `run_agent_loop` (потолки max_steps / wall_clock / token-через-`ContextBudget`, эмиттер потока `AgentEvent`), безопасные стабы (echo/noop — ноль side-effect; актуатор/skills/approval/sandbox — следующие срезы). `AgentEvent` (`#[non_exhaustive]`: AssistantToken / ToolCall / ToolResult / ContextUsage / Final / Error) — поток под вкладку Агента (контракт UI→бэкенд из дизайн-хэндофа). Типобезопасная граница: `OpenAiToolProvider` — **ОТДЕЛЬНЫЙ тип** (I-5: не протекает в chat/web, grep-линт `check-tooluse.mjs`), `ToolCapableProvider` ≠ `ChatProvider` (chat-путь не тронут). SSE tool_calls — index-keyed аккумулятор, финал по `finish_reason`, невалидный JSON → ошибка (никогда не mis-execute). Строгий OpenAI-протокол: `ChatMessage` + опц. `tool_calls`/`tool_call_id` (serde skip-if-None → существующие сообщения байт-идентичны, eval-гейты целы), цикл шлёт `assistant{tool_calls}`→`tool{tool_call_id}` (корреляция multi-call). Egress цикла — только Chat к LLM. `AIClient.agent_tools: Option` (None десктоп / Some agentd). **421 тест** (+30); agentd-smoke цикла (execute→feed-back→Final, офлайн); **live-тест полного цикла против развёрнутой Qwen3.6-27B :8080 ПРОЙДЕН** (outcome=Final, сервер принял строгую форму сообщений). Adversarial-ревью (3 скептика + live-проба протокола→судья): SHIP, 0 блокеров; все 5 recommended сведены инлайн (строгий протокол+корреляция, PER_MESSAGE_OVERHEAD-dedup, live tool-call тест, malformed-args/re-ask edge).

### Корректность бюджета: реальный Qwen3.6-27B токенайзер + ContextBudget (P0-c)

Плейсхолдер `WordTokenizer` (whitespace, недосчитывал кириллицу ~1.85×) заменён реальным BPE-токенайзером **развёрнутой** модели. `ai/tokenizer.rs`: `QwenTokenizer` (крейт `tokenizers` 0.23, `default-features=false`+`onig` — БЕЗ http→reqwest/hyper, egress-chokepoint цел) грузит вшитый gz-ассет (`tokenizer.json` `Qwen/Qwen3.6-27B`, vocab 248 044; 3.3 МБ gz, распаковка `flate2` раз через `OnceLock`) и считает `encode(text,false).len()` ровно как `llama.cpp /tokenize`. Configurable `ai.tokenizer_path` (дефолт = вшитый) — смена модели = файл+конфиг, без кода. Fail-closed: ассет не парсится → `warn!` + скрипт-aware эвристика (латиница÷4/кириллица÷3/CJK, консервативно завышает). `ContextBudget` (окно из `ChatConfig.context_window`, не хардкод; 32k→256k = строчка конфига) `fit()` сохраняет все system + самые свежие не-system; контракт system-overflow задокументирован + `warn!` (без молчаливого оверфлоу). **Гейт (CI, офлайн): embedded `count()` == живой `/tokenize` развёрнутой модели** на golden EN17/RU24/CODE40/MIX27 + `#[ignore]` live-кросс-чек vs :8080. WordTokenizer заменён в `indexer/mod.rs`+`indexer/rag.rs` (трейт+WordTokenizer оставлены для тестов). **391 тест** (+11). Adversarial-ревью (3 скептика→судья, спот-чек): SHIP, 0 блокеров; 5 recommended сведены инлайн (контракт-docstring+warn+тест system-overflow `fit()`, per-message overhead +8, NOTICE-атрибуция Qwen Apache-2.0, onig→fancy-regex заметка).

### Безопасность: чокпоинт фенсинга наблюдений `fence_observation` + паритет rerank (P0-e)

Единый примитив `ai::chat::fence_observation(label, body, marker)` — оборачивает НЕДОВЕРЕННЫЙ текст наблюдения (будущие tool-результаты AGENT-1, web-выдача, фрагмент файла) в ограждённый блок ДАННЫХ с per-request маркером ([`injection_marker`], 96 бит getrandom()), size-cap **12 KiB** с усечением по границе UTF-8-символа (кириллица не рвётся) + явное `…[усечено N байт]`. Контракт I-5: результат — ДАННЫЕ для роли `user`/`tool`, **НИКОГДА** не `system`. Defense-in-depth: вхождения маркера внутри тела структурно нейтрализуются (`⟨marker⟩`), чтобы недоверенный текст не подделал закрывающий разделитель (гард на пустой маркер). No-tails аудит: все 17+ продюсеров внешнего контента уже фенсятся в user-роли (RAG/web/news/episode/digest/insights/contradictions/memory/…) — незафенсенных инъекций не осталось.
- **Паритет rerank с RAG:** `search/rerank.rs::llm_rerank` теперь фенсит сниппеты заметок per-fragment маркером (delimiter-only, контент байт-идентичен). **DRY:** общий `build_rerank_messages` зовут и прод, и live-eval — eval мерит РЕАЛЬНЫЙ прод-промпт (убран ручной copy/drift). Live-eval (gemma-e4b :8084): rerank nDCG=1.000 MRR=1.000, регресса нет.
- **380 тестов** (+ neutralizes-marker-in-body, build_rerank_messages_structure, fence-helper юниты). Adversarial-ревью (3 скептика→судья, спот-чек): SHIP, 0 блокеров; оба recommended (marker-strip, prompt-shape тест) сведены инлайн.

### Безопасность: durable egress-аудит — write-before-act (агент P0-b)

`EgressAudit` стал **durable**: каждый egress-вердикт пишется в БД **до фактического сокет-вызова** (write-before-act), а не только в in-memory `Vec` (терялся при крэше — плохо для accountability always-on агента). Миграция **020** `egress_audit` (append-only: feature/host/bytes_out/allowed/denied_reason/run_id/created_at, индекс по created_at DESC). `record()` теперь async: пушит in-memory → если установлен writer, INSERT'ит строку и **дожидается commit перед возвратом** из `authorize()` (на всех 5 сайтах — успех + 4 отказа). Sink (`Mutex<Option<WriteActor>>`) ставится через `set_writer` ПОСЛЕ `Database::open` (в app `open_vault` и agentd main) — заменяем при пере-открытии vault; `record()` перечитывает writer per-call (без stale-writer на старую БД). Хост хранится реальный (локальная БД владельца; `Redacted` гасит только Debug/логи — warn при сбое durable-записи хост не светит). `run_id: Option<i64>` — scaffold под будущую AgentRun-корреляцию (пока None). Durable-сбой best-effort: логируется, egress не блокирует (in-memory слой сохраняет запись). Дедлок/reentrancy исключены: writer клонируется из Mutex (guard дропается до `.await`), WriteActor — однопоточный mpsc с catch_unwind, authorize не зовётся изнутри write-замыкания. **374 теста** (+4 net: durable-persist, write-before-act на denial И success-путях, своп writer при пере-открытии vault). Adversarial-ревью (3 скептика→судья, спот-чек): SHIP, 0 блокеров.
- Pre-vault окно (между `AppState::new` и `open_vault`) — аудит in-memory-only by-design (БД аудита живёт в ещё-не-открытом vault); задокументировано. **Follow-up (BACKLOG):** durable broker `AuditLog` (plugin/broker.rs, тот же accountability-слой) — вне scope P0-b.

### Архитектура: headless `nexus-agentd` — скелет (агент CORE-2a)

Новый крейт-бинарь `crates/nexus-agentd` (3-й член workspace) доказывает **топологию A**: открывает vault и крутит планировщик **headless, на одном `nexus-core`, без Tauri/десктопа**. Composition-root: `Database::open` (миграции) → `LocalConfig` из `.nexus/local.json` → `EgressPolicy` (fail-closed allowlist `ai.*`) + `EgressAudit` → `AIClient` (chat/fast/util) + эмбеддер через `GuardedClient` → `Indexer` → `scheduler::worker_loop` с no-op-хуками + тривиальный `health`-handler. **Smoke (`NEXUS_AGENTD_SMOKE=1`) пройден**: БД создана (19 миграций), планировщик отработал health-джобу (`jobs: health|done`), exit 0. Изоляция: зависит ТОЛЬКО от nexus-core (ноль tauri/app/raw-HTTP; весь egress через core-`GuardedClient`; `check-egress.mjs` расширен на `crates/nexus-agentd/src`). Зависимости только из lockfile (без скачиваний). `cargo test --workspace` 542. Adversarial-ревью (3 скептика→судья): SHIP, 0 блокеров.
- **Follow-up (ДО egress-способного / RAG-запрашивающего agentd — BACKLOG, не блокеры скелета, который ни egress, ни RAG-запросов не делает):** (1) восстановить persisted `egress.json` (offline + per-feature opt-out) через `net::persist::load`+apply, иначе headless-agentd проигнорирует kill-switch владельца; (2) прогнать `reconcile_embedding_model`/dim-model-гард перед чтением `vectors.usearch` (иначе `DimMismatch` на рассинхроне модели); (3) egress-lint: заскоупить `net/`-whitelist на nexus-core-srcRoot (defense-in-depth по мере роста SRC_ROOTS).

### Архитектура: memory/engine-кластер в `nexus-core` — CORE-1 ЗАВЕРШЁН (агент CORE-1c-2)

Финальный под-срез CORE-1: `episode, chat_log, contradictions, relation_reasons, starting_questions, memory, eval` (+ data-фикстуры `eval/*.json`) перенесены в `crates/nexus-core` (git mv). Память Nexus (факты MEM / переписка N4b / эпизоды EP) + консолидация (MEM-8c) + eval-харнесс теперь в ядре — agent-service получит их напрямую. `include_str!("../../eval/*.json")` разрешаются (data-dir переехал вместе; `eval_fixture_meets_baseline` + consolidation DELETE-precision гейты проходят, фикстуры SHA-идентичны — golden не трогали; гейты не вакуумны — `golden_parses_and_is_well_formed` требует ≥30 consolidation/≥20 episode кейсов). Релокация байт-идентична (порог 0.30, fail-closed=ADD, §4.3 защита explicit-фактов, op_group — целы; 139 тестов). Единственная не-rename правка — `note_snippet pub(crate)→pub` (кросс-крейт из commands/suggest). `cargo test --workspace` 542 (369 core + 173 app). Adversarial-ревью (3 скептика→судья): SHIP, 0 блокеров.

**CORE-1 ЗАВЕРШЁН:** `nexus-core` = полный data/engine-слой (vault/db/net/ai/scheduler/indexer/search/memory/episode/chat_log/graph/relation_reasons/contradictions/suggest/tagger/tags/watcher/vector/parser/chunker/plugin/redact/eval). В app остались: UI (home/board/goals/properties), tauri-команды, state, git, home-завязанные фичи (digest/news/websearch). Следующее — CORE-2 (headless `nexus-agentd` + P0-b durable аудит).

### Архитектура: index/retrieval-кластер вынесен в `nexus-core` (агент CORE-1c-1)

Под-срез CORE-1: `watcher, tags, tagger, indexer, graph, suggest, search` перенесены в `crates/nexus-core` (git mv). Развязка `indexer` от Tauri: `events.rs` принимает инъектируемые `IndexerHooks` (on_progress/on_vault_changed/on_file_changed, Arc-замыкания) вместо `AppHandle`; десктоп строит emit-замыкания в `commands/vault.rs::indexer_hooks` — **имена событий (`vault:index-progress`/`vault:changed`/`vault:file-changed`) и camelCase-payload (`IndexProgress`/`FileChanged`) байт-идентичны → фронт не тронут** (SAFE-3 echo-suppression + watcher-debounce целы). Ядро теперь индексирует + гибридный поиск headless. Релокация байт-идентична (M1-дедуп + оба теста переехали в core; search = 100% rename). `cargo test --workspace` 542 (239 core + 303 app). Adversarial-ревью (3 скептика→судья): SHIP. Подчищена мёртвая прямая зависимость `notify-debouncer-full` из app (теперь транзитивно через core).

### Архитектура: scheduler-движок вынесен в `nexus-core` (агент CORE-1b)

Под-срез CORE-1: генерик-движок планировщика (`Registry`/`JobHandler`/`WorkerHooks`/`worker_loop`/`tick_once`/`run_due`/`enqueue`/watchdog/crash-recovery/backoff) расщеплён из app в `crates/nexus-core/src/scheduler.rs` (tauri-free, dep только `db`) — будущий agent-service сможет гонять джобы headless. APP-glue (`WorkerSpawner`+`AppHandle`, `start()`/супервизор, `emit_jobs_changed`, `GcHandler`+`KIND_GC` с `contradictions`/`relation_reasons`, `default_registry`) остался в app и реэкспортит движок (`pub use nexus_core::scheduler::*`) → call-sites без правок. Семантика **байт-идентична** (все тайминг-константы + watchdog/requeue/анти-старвейшн неизменны; adversarial-ревью 3 скептика→судья подтвердил, GC через границу крейтов гоняется). `cargo test --workspace` 542 (173 core + 369 app); egress/CI-инварианты целы.

### Архитектура: выделен крейт `nexus-core` (агент CORE-1, ADR-009 D1)

Фундамент под headless agent-service: agent-нужное ядро вынесено из Tauri-приложения в библиотечный крейт `crates/nexus-core`, переиспользуемый и десктопом, и будущим `nexus-agentd`. Слайс 1 — замкнутый набор из 9 модулей (`redact, db, parser, vector, plugin, vault, chunker, net, ai` — tauri-свободные, зависимости внутри набора), перенесён через `git mv` (история цела).
- App переэкспортирует (`pub use nexus_core::{…}`) → существующие `crate::X` пути работают без правок call-site (минимум churn).
- **Egress-чокпоинт сохранён**: `net`/`core_client_builder`/`is_private_host` переехали → `check-egress.mjs` (+ `check-ignored`/`check-traceability`/`check-dangling`) расширены на оба src-корня; покрытие доказано инъекцией raw-reqwest в nexus-core → линт ловит. CI и `test-all.sh` переведены на `--workspace` (тесты/линты nexus-core реально гоняются).
- `test-util`-фича отдаёт тест-фикстуры (MockEmbedder/`unchecked`) только в dev (прод-бинарь не тянет). `cargo build/clippy/test --workspace` зелёный; **542 теста сохранены** (nexus-core 154 + app 388). Adversarial-ревью (3 скептика→судья): SHIP, 0 блокеров.
- Дальше: развязать `scheduler/indexer/home` от `tauri::AppHandle` (через хуки) → довнести `search/memory/chat_log/episode/scheduler` в ядро.

### RAG: дедуп чанков по реальному пересечению текста, не по соседству индекса (M1 — recall-фикс)

Баг из аудита (батч B, 🔴 MAJOR). `resolve_and_dedup` схлопывал чанки одного файла по `|Δchunk_index|≤1`, но `chunk_index` сквозной через секции → последний чанк секции A и первый секции B (0 пересечения текста) ложно схлопывались, второй молча выбрасывался → потеря recall на многосекционных заметках. Теперь дедуп по РЕАЛЬНОМУ пересечению `[char_start,char_end)` (персистятся, мигр.002): перекрывающиеся по тексту (overlap чанкера) схлопываются, на стыке секций — оба сохраняются. Регрессионный тест (две секции, соседний индекс, непересекающиеся диапазоны → оба сохранены) + офлайн eval-гейт `eval_fixture_meets_baseline` зелёный (без регрессии recall/nDCG). `cargo test --lib` 542/0. (Хвост n3 — multi-relevant golden для МЕТРИКИ выигрыша — остаётся, нужен живой bge-m3.)

### Inference: retry/backoff на инициации LLM-вызова (агент P0-d, ADR-009 Фаза 0)

Флейк локальной LLM больше не валит весь вызов: ограниченный экспоненциальный ретрай оборачивает ТОЛЬКО инициацию запроса (`post_json` + проверка статуса) ДО первого чанка стрима. После старта стрима ретрая нет (структурно — chunk-цикл вне ретраемого замыкания, тело ответа не тронуто). Ретрай на транспортных ошибках + 408/429/500/502/503/504; egress-deny и не-ретраебельные 4xx — fatal (не ретраятся). Cancel-aware (Stop отзывчив даже на максимальном backoff — sleep с тиком ≤50мс). Дефолт: 3 попытки, base 300мс / cap 2с (<1с добавленной латентности worst-case-success). 9 тестов (классификатор, backoff-математика, исчерпание, cancel до/во время сна, real-socket: первый коннект рвётся → второй стримит). Adversarial-ревью (скептик, 6 свойств): SHIP, 0 блокеров. `cargo test --lib` 541/0.

### Web-агент: consent-URL SearXNG за subpath (m9)

`build_search_url` достраивал `/search` через `ends_with("search")` — для consent-URL за subpath (`/research`/`/websearch`/`/metasearch`) ложно считал путь готовым эндпоинтом → запрос уходил не туда (404). Теперь сравнивается ПОСЛЕДНИЙ сегмент пути. Egress-safe (тот же хост/эндпоинт, без новых хостов — owner-flag из бэклога). +тест subpath/уже-полный путь; `cargo test --lib websearch` 14/0.

### Frontmatter: writer `set_frontmatter_field` стал value-aware (m8 — защита от тихой порчи)

Фикс бага из аудита LLM-путей (батч D, owner-deferred → взят с приоритетом как задевающий агента): writer валидировал только КЛЮЧ, не текущее ЗНАЧЕНИЕ — перезапись ключа, чьё значение список/блок, молча портила данные. Будущий actuator/skill-writer агента — ровно новый вызыватель, доходящий до этой ветки.
- `is_non_scalar_target` (симметрично ридеру `frontmatter_fields` через общий `read_scalar` — НЕ второй парсер) отказывает `Err(FmWriteError::NonScalarTarget)` (round-trip-reject, файл НЕ трогаем) при перезаписи ключа, чьё значение: инлайн-список/объект `[…]`/`{…}`; ЛИБО пустое/block-scalar-индикатор `|`/`>` + ниже отступной дочерний блок (вложенный маппинг/литерал) или элемент `- …`. Пустое без блока ниже — заполняется как раньше.
- Покрывает inline list/object, блок-список, **блок-скаляр `|`/`>`** (достижим через PropertiesEditor) и **вложенный блок-маппинг** — оба HIGH-находки adversarial-ревью (изначальный гард ловил только `- …`).
- Мок `vault.ts` зеркалит контракт (отказ ДО мутации CONTENT — файл байт-в-байт цел; MEM-5). 4 backend-теста + 4 мок-теста; `cargo test --lib parser` 16/0, мок 28/28, fmt/clippy/egress чисто.
- Остаток (BACKLOG m8-хвост): блок, отделённый ПУСТОЙ строкой — не ловим (симметрично читателю); проброс actionable-сообщения NonScalarTarget в тосты PropertiesEditor/BoardView.

### Egress: DNS-rebind/SSRF-гард в ядре GuardedClient (агент P0-a, ADR-009 Фаза 0)

Первый срез фундамента самообучающегося агента. Закрыта дыра: `EgressPolicy::check` валидировал только СТРОКУ хоста, после чего `reqwest` сам резолвил DNS и коннектился — публичный домен, резолвящийся в cloud-metadata (`169.254.169.254` / IMDS-v6 `fd00:ec2::254`) или (web-класс) приватный IP, проходил host-string-гейт и уходил на сокет (TOCTOU между check и connect). Гард resolve→check-all-IPs→pin был лишь в фетчерах и отсутствовал на core-пути chat/embed/probe.
- **`net/resolve.rs` (новый)** — единый источник истины: трейт `Resolver` (боевой `SystemResolver` на tokio + мок для офлайн-тестов), `check_resolved_ips(ips, deny_private)` — БЕЗУСЛОВНО режет cloud-metadata (вкл. IPv4-mapped/NAT64/6to4/IPv4-compat-туннелирование), IMDS-v6 `fd00:ec2::254` и link-local; при `deny_private` (web-класс) — также приватные/loopback/ULA/CGNAT; пустой резолв → fail-closed отказ.
- **`GuardedClient::authorize` теперь async**: host-string-гейт → резолв → `check_resolved_ips` → ровно одна audit-запись → per-request клиент с пином проверенного IP (`resolve_to_addrs`), коннект гарантированно на проверенный адрес. `redirect=none` сохранён. chat/embed/probe: LAN/приватные живут (local-first), metadata/link-local/IMDS-v6 — нет.
- **Дедуп**: дублированный guard в `news/fetch.rs`, `websearch/search.rs`, `commands/plugin.rs` сведён к `net::check_resolved_ips` (одна логика).
- 16 adversarial IP-тестов (metadata/IMDS-v6/mapped/NAT64/6to4/multi-A/empty/link-local) + 4 async-интеграционных (chat→metadata denied до коннекта и аудитится; chat→loopback/LAN allowed; web→private denied). Полный `cargo test --lib` 530/0; fmt/clippy/egress-lint чисто.
- Adversarial-ревью диффа (3 скептика→судья): TOCTOU-пин подтверждён корректным; блокер «IMDS-v6 на chat-пути» закрыт безусловным блоком. Остаток (authorize-owns-resolve: убрать двойной резолв на web-классе, аудит web-rebind-отказов, connection-pooling, мульти-IP-пин, дедуп предиката) → отложен в BACKLOG отдельным follow-up-срезом.

### Тогглы «Инсайты» и «Поиск противоречий» — owner-gated фоновые ИИ-фичи (дефолт OFF)

Два фоновых LLM-виджета вынесены за тогглы в Настройки→ИИ с возможностью отключить, не выпиливая функционал (решение владельца на real-test 2026-06-18: на reference/MOC-vault'ах инсайты дают пусто, противоречия точны но нишевы и затратны → дефолт **OFF**, opt-in). Источник истины — БД vault (зеркало `episodic.enabled`).
- **Бэкенд-гейт** (persisted `insights.enabled` / `contradictions.enabled`): `is_enabled`/`set_enabled` в `contradictions/mod.rs` + `insights_enabled`/`set_insights_enabled` в `home/insights.rs`. В `commands/vault.rs` (`open_vault`) флагами `insights_on`/`contra_on` гейтятся ВСЕ пути enqueue — recurring-регистрации (open_questions/context_drift/stale → insights, contradictions → contra) И on-open seed'ы. Ручные триггеры тоже гейтятся: `refresh_widget` (insight-ключи), `refresh_stale_radar`, `generate_contradictions`. `ContradictionHandler::handle` рано выходит NOOP при OFF (защита от stale-recurring при выключении в работающем приложении).
- **Kick при включении** (контракт MAJOR-2, урок EP-1): `contradictions_set_enabled`/`insights_set_enabled` при ВКЛючении enqueue'ят немедленные джобы доступных виджетов (recurring регистрируется лишь на открытии vault → без kick фича мертва до перезапуска). Дедуп `has_ready_job`.
- **Фронт**: стор `useAiFeaturesStore` (НЕ localStorage — грузится от бэка при открытии vault, `App.tsx`; иначе дефолт-OFF на новой машине разошёлся бы с включённым в БД — privacy-урок EP-3). Два тоггла в Настройки→ИИ. Home-карточки инсайтов и панель противоречий при OFF показывают честную подсказку «включите в настройках» вместо мёртвой кнопки «Обновить». Stale-radar deterministic-скан НЕ гейтится (graceful degradation: список устаревших виден без LLM-обогащения). Мок зеркалит контракт (дефолт OFF, round-trip).
- 4 backend-теста (дефолт-OFF+round-trip обоих тогглов · handler-NOOP-при-OFF противоречий) + полный фронт-сьют 845/0; tsc/eslint/i18n-паритет/egress/dangling зелёные.

### Утренний экран «Сегодня» — сводка дня из существующих данных (TODAY-1)

Новый top-level вид «Сегодня» (соседи Home/Новости/Доска): один экран собирает весь день из УЖЕ существующих данных — никакого нового бэкенда/БД/LLM/egress (чистый фронт, read-only компоновка). Заменяет утренний ритуал из 4 остановок (Home → ⌘⇧D дневник → Доска → Задачи) одним взглядом. Выбран мультиагентным роадмап-анализом R2 (12 кандидатов → судья) как единственный net-new кандидат (не перекрыт списком/панелью Задач/Home), ready-now и autonomy-safe; agenda-вид доски отклонён как перекрытый VIEW-1.
- **Пять секций** (`TodayView`): задачи доски просрочено+сегодня (через ОБЩИЙ `sortTasks(due,asc)`+`isOverdue` — без дрейфа ранжирования с VIEW-1/планом дня) · чек-задачи заметок просрочено+сегодня (`collectTasks`+`bucketOf`) · превью тела заметки дня · счётчик quick-capture Входящих (`parseInbox`) · недавние эпизоды (`episode.list`). Каждая секция изолирована: сбой загрузки → пустое состояние, а не падение всего экрана (fail-safe, `Promise.all` независимых веток).
- **READ-ONLY инвариант:** заметка дня проверяется через `file_hash` (→ null = нет файла) и НЕ создаётся на рендере; явное создание — только по клику «Открыть заметку дня» (`openOrCreateDaily`). Экран ничего не пишет в vault при открытии.
- **Стейт-машина (главный риск, по плану):** `todayOpen` — 4-я взаимоисключающая примарная вью; протянут во ВСЕ сайты гашения/возврата в `ui.ts` (`openHome/News/Board` + тоглы + `openChat` + ОБЕ ветки `toggleChat` с re-surface-проверкой) + `App.tsx` `aiVisible`/тернарий + `ActivityBar` (кнопка «Сегодня» + Files-active). Закрывает класс «мёртвой кнопки чата» (баг 2026-06-11): открытие чата из «Сегодня» гасит `todayOpen` → панель видна. Палитра: команда `view.today`.
- **Превью-верификация** (мок-vault): вид рендерит 5 секций, задачи доски — просроченная карточка, эпизоды из мока, пустые состояния для отсутствующих данных; **dead-button-гард ПРОВЕРЕН live** (чат из «Сегодня» открывается, не мёртвая кнопка); взаимоисключение «Сегодня»↔Home; 0 ошибок консоли; темы по токенам.
- **Adversarial-ревью (3 скептика → судья):** state-machine (полнота гашения 4-го флага) / read-only-инвариант / данные+layout+i18n.
- 6 тестов стейт-машины `ui.test.ts` (гашение/re-surface/dead-button) + 8 тестов `TodayView` (порядок доски, фильтры чек-задач, заметка-дня exists/absent+НЕ-пишет, счётчик Входящих, эпизоды, fail-safe, клик). i18n `today.*` + `view.today` ru+en (паритет). Полный фронт-сьют 844/0, tsc/eslint чисто, Rust не тронут.

### Доска: представление «Список» — плотная сортируемая/фильтруемая таблица задач (VIEW-1)

У доски появилось второе представление: переключатель «Канбан / Список» в шапке. «Список» — read-only плоская таблица поверх ТЕХ ЖЕ задач (никакого нового бэкенда/БД/LLM/egress — чистый фронт). Закрывает повседневный сценарий «дедлайн-стена»: за 3 секунды просмотреть все просроченные/сегодняшние задачи по всем проектам и отфильтровать под фокус, вместо горизонтального скролла канбан-колонок. Дополняет «План дня» (AI-подборка) полной прозрачной выдачей. Выбор среза — мультиагентный роадмап-анализ (15 кандидатов → судья) на границе завершённых эпиков EP+канбан.
- **Чистая модель** (`board-model.ts`): `sortTasks(cards, key, dir)` (ключи due/priority/status/title) + `filterTasks(cards, f)` (статус/приоритет/проект/тег/текст, комбинация по И). Инвариант «пустое тонет»: `null`-срок и неизвестный приоритет всегда В КОНЦЕ — НЕЗАВИСИМО от направления (desc их не поднимает). Приоритет ранжируется ОБЩИМ `priorityRank` (тот же, что у `planDay` — анти-дрейф двух ранжирований). Стабильный тай-брейк по пути, без влияния `dir`. Функции НЕ мутируют вход (`[...cards].sort` / `filter`) — критично: `BoardView` выводит канбан-колонки из ТОГО ЖЕ массива `data.cards`, мутирующий сорт испортил бы колонки/DnD.
- **`ListBoardView`** (новый): сортируемые заголовки (стрелка направления), фильтр-тулбар (опции только из присутствующих карточек), клик по строке → существующий `TaskPeek`. Статус в ячейке и в фильтре локализуется через `columnLabel` (кастом/переименованные/«Прочее» не теряются и не путаются). Строки — read-only `<button>` (без DnD-хэндлеров).
- **Тоггл вида** в `BoardView`: `viewMode` `columns`|`list` из localStorage (`nexus.board.viewMode.v1`, try/catch-гард для node25/test → fail-safe `columns`). `TaskPeek` и AI-панели (план/застрявшие) доступны в ОБОИХ режимах; DnD — только в канбане.
- **Превью-верификация** (мок-доска): дефолт-сорт due asc (просрочено первым, null-срок последним), сорт по приоритету (срочно→высокий→средний→низкий), фильтр по проекту (Nexus → 3 задачи), кастом-статус «ожидание», клик-строки→peek, строки не draggable, тоггл-туда-обратно сохраняет канбан, персист в localStorage, 0 ошибок консоли, темы по токенам.
- **Adversarial-ревью (3 скептика → судья): verdict SHIP, 0 MAJOR** — все 10 измерений чисты (чистота сорта/фильтра, null-last в обе стороны, round-trip статусов, общий priorityRank, изоляция стейт-машины тоггла от busyRef/peek, fail-safe localStorage, i18n-паритет, read-only/без egress). Остаток (MINOR a11y `aria-sort` на кнопке vs `role=columnheader`; nit отдельный `searchLabel`) — в BACKLOG, не блокеры (стрелка передаёт состояние зрячим).
- 22 юнит-теста модели (sort/filter + не-мутация) + 7 тестов `ListBoardView` + 5 тестов тоггла `BoardView` (переключение/персист/peek/нет-DnD). i18n `board.list.*` ru+en (паритет). tsc/eslint зелёные.

### Эпизодическая память: UI-панель + обратимость + тоггл (EP-3, завершение эпика)

Эпизодическая память становится управляемой: панель-таймлайн прошлых сессий + тоггл включения. Это закрывает эпик EP (фундамент EP-1 → ретривал EP-2 → UI EP-3).
- **Команды** `episode_list/dismiss/restore/purge/get_enabled/set_enabled` (`commands/episode.rs`). `dismiss`/`restore` — мягкое скрытие (обратимо, из ретривала). `purge` — жёсткое удаление: DELETE строки + `episode_vectors.remove` (реальный путь стереть саммари; CASCADE мёртв — команды удаления сессии нет). Первоисточник-сессия не трогается.
- **Тоггл** `episode_set_enabled(on)` персистит `episodic.enabled` И при ВКЛЮЧЕНИИ enqueue'ит `episode_rollup` kick — **закрывает контракт MAJOR-2** из ревью EP-1 (иначе фича «мертва до перезапуска vault»: seed гейтится `is_enabled`, recurring бутстрапится из успешного прогона).
- **Панель «Эпизоды»** (focus-trap по `MemoryPanel`, `TRAP_OVERLAYS_CLOSED`): таймлайн обратной хронологии — карточка = дата + заголовок сессии (клик грузит сессию) + саммари + чипы тем; «Скрыть» (undo-тост) / «Восстановить» / «Удалить навсегда» (подтверждение). Тоггл «Эпизодическая память» в Настройки→AI (пишет фронт-pref + бэк-настройку). i18n ru/en.
- **Превью-верификация** поймала реальный layout-баг (заголовок сессии схлопывался в 0 при узком окне) → флор `min-width` + `flex-wrap` в шапке карточки.
- **Adversarial-ревью поймал major (privacy):** тоггл писал И фронт-pref (localStorage), И бэк-настройку, но отображался из pref — при переносе vault на другую машину (DB `episodic.enabled`=ON едет с vault, localStorage дефолт OFF) тоггл показывал OFF, а фоновая генерация шла. Фикс: DB-настройка — источник истины, синхронизируем pref от неё при открытии vault (`App.tsx`).
- 4 backend-теста (list обр.-хрон / dismiss-restore / purge-удаляет-строку / set_enabled-persist) + 4 фронт-теста стора (мок зеркалит контракт: purge удаляет, dismiss обратим). Backend 503, фронт 801 зелёных.

### Эпизодическая память: ретривал + инъекция в чат под eval-гейтом faithfulness (EP-2)

Эпизоды (саммари прошлых сессий, EP-1) начинают подмешиваться в контекст чата — ассистент «помнит, о чём вы говорили раньше». За тогглом `aiEpisodicMemory` (**OFF** по умолчанию; UI-тоггл — EP-3). Чисто аддитивный канал — note-RAG не трогает.
- **Ретривал** `episode::search_episodes` (зеркало N4b `search_memory`): эмбеддинг вопроса → `episode_vectors` → отсечка по **отдельному** порогу `EPISODE_SIM_THRESHOLD=0.45` (длинное саммари ≠ короткий факт) → топ-`EPISODE_K=2`, фильтр скрытых (`dismissed`) + исключение текущей сессии.
- **Инъекция** `build_episode_block` (двойная анти-инъекция `injection_marker`: и при генерации саммари, и при инъекции), обрезка 400 симв. Стрим-событие `EpisodeSources` → плашка «Из прошлых сессий», клик грузит сессию.
- **Дедуп с N4b:** если разговор всплыл ЭПИЗОДОМ, его сырые реплики (`chat_vectors`) НЕ дублируются — `search_memory` получил `exclude_sessions`. Один разговор не пересказан И процитирован.
- **БЛОКИРУЮЩИЙ eval-гейт faithfulness** (`eval/episodes.rs`): главный риск эпизода — ложная память (саммари утверждает то, чего в диалоге не было). Гейт = доля «верных» саммари (не галлюцинируют + заземлены на якоря) ≥ **0.85** на ≥20 golden-кейсах. Ретривал не включается, пока live-точка `live_episode_summary_meets_gate` не зелёная. **Прогон на прод-модели саммари (gemma-e4b :8084): faithfulness=1.000 (22/22), 0 галлюцинаций.** При смене модели — рекалибровка.
- Приватность: эмбеддинг по существующему консентнутому каналу, ноль нового egress. Фронт-плумбинг (тоггл-флаг, тип `EpisodeHit`, рендер) — пер-call, дефолт OFF.
- Backend 499 зелёных (+ eval/episode/chat_log тесты), фронт 797 зелёных. **Отложено (BACKLOG):** общий токен-бюджет memory-prepend (пины→эпизоды→N4b) — пер-канальные капы уже ограничивают инъекцию; общий бюджет — оптимизация, отдельный срез (затрагивает зрелые MEM/N4b/PIN-пути).

### Эпизодическая память: фундамент — генерация саммари сессий (EP-1)

Третий слой памяти агента (после ФАКТОВ MEM и сырой памяти переписки N4b): **эпизод** = связное нарративное саммари ОДНОЙ завершённой чат-сессии («о чём был разговор и к чему пришли»). Спека `docs/specs/agent-episodic-memory.md` (decision-complete, 17 решений). EP-1 — только фундамент: генерация + хранение, БЕЗ ретривала/инъекции (EP-2) и UI (EP-3).
- **Схема (миграция 019)** `chat_episodes` (1:1 с `chat_sessions`, `session_id` UNIQUE): summary, topics, водяной знак `last_msg_id`/`msg_count`, время `started_at`/`ended_at`, `model`/`embed_model`, `dismissed`. Производна от `chat_messages` — дроп безопасен (пере-генерируется). Параллельный usearch-индекс `episode_vectors`.
- **Генерация — фоновая scheduler-джоба `episode_rollup`** (НЕ in-memory debounce: единственный писатель — воркер планировщика, гонка `UNIQUE(session_id)` исключена архитектурно). recurring scheduled-only (~6 ч) + seed run-if-overdue на открытии (`has_stale_episodes && !has_ready_job`). `defer_under_interactive` (уступает интерактивному чату).
- **Гейт «созревшей» сессии:** ≥4 сообщений, простой ≥2 ч, нет актуального эпизода (idempotency по `last_msg_id` — не жжём LLM на неизменном). Детерминированный SQL, юнит-тестируем.
- **Модель** `chat_util`→`chat_fast` фолбэк (паттерн `set_title`), транскрипт в `injection_marker()` (анти-инъекция). Best-effort: ошибка/пустое саммари → не пишем, джоба `Ok`.
- **Тоггл** `episodic.enabled` (persisted в `settings`, дефолт **OFF**: фоновая джоба не получает per-call флаг). OFF → ноль LLM-вызовов и записи. UI-тоггл — EP-3.
- **Реконсиляция эмбеддера (фикс orphan-вектора):** смена модели дропает `episode_vectors.usearch` + `UPDATE chat_episodes SET embed_model=NULL`; backfill на открытии переэмбеддит summary (как `chat_vectors`). Иначе запрос новой моделью против старых векторов → DimMismatch/мусор. (Пред-существующая orphan-дыра `chat_vectors`/`memory_vectors` — отдельная задача, не EP-1.)
- **Обратимость:** `ON CONFLICT(session_id) DO UPDATE` при пересжатии НЕ сбрасывает `dismissed` (фон не отменяет намерение скрыть). Полное удаление — `episode_purge` (EP-3, не CASCADE: команды удаления сессии в коде нет).
- 7 backend-тестов (гейт quiet/min-msgs, idempotency-не-перевызывает-LLM, пересжатие-сохраняет-dismissed, тоггл-OFF-NOOP, пустое-саммари-не-пишет, эмбеддинг-в-индекс, parse тем). Backend 491 зелёных.

### Память агента: авто-режим консолидации — «Предлагать ↔ Авто» (MEM-8c-b, завершение эпика)

Завершает эпик консолидации: под-режим **«Авто»** применяет слияния/замещения молча (без чипа), с защитой и обратимостью. За мастер-флагом `aiMemoryConsolidation`; под-тоггл `aiMemoryConsolidationMode` (`propose`|`auto`, дефолт **propose**).
- **Авто-применение:** в режиме «Авто» при подтверждении факта `update`/`supersede` на **auto-source** факт применяется сразу (без чипа). Тривиальные `add`/`noop` — как раньше.
- **Защита explicit-фактов (§4.3), fail-closed:** молча применяем **только** к цели с `targetSource==='auto'`; факт, введённый юзером ЯВНО (`explicit`), как и любой другой/неизвестный/пустой source (будущий imported/synced, регрессия бэка) → **всегда чип-предложение**, а не молчаливая мутация.
- **Обратимость + честный откат:** авто-консолидация показывает toast «Объединил/Заменил: было → стало» с кнопкой **«Отменить»** (откат группы `memory_consolidate_undo`, optimistic-безопасно). Тост честен: «Отменено» только если бэкенд реально откатил; если факт уже изменён руками / группа откачена — «Не удалось откатить», без лжи об успехе. Soft-supersede оставляет всё восстановимым.
- Под-тоггл «Предлагать ↔ Авто» в Настройках→AI (виден только при включённой консолидации). Тост авто-консолидации не всплывает на сменившейся ленте (presence-guard). 8 тестов авто-режима (применение/защита-explicit/fail-closed-unknown-source/undo-проброс-исхода/degraded-to-add/propose-сохранён) — фронт-зелёные.
- **Авто-DELETE разблокирован eval-гейтом** (MEM-8c-a): на gemma-26B DELETE-precision=1.0 (0 ложных). При смене модели на Qwen3 27B — повторить калибровку (`live_consolidation_meets_gate`).

### Память агента: бэкенд авто-режима — target_source + откат группы (MEM-8c-b-backend)

Фундамент авто-режима консолидации (фронт — следующим срезом). Две вещи, обе про БЕЗОПАСНОСТЬ авто-применения.
- **`targetSource` в `PlanOp::Update/Supersede`** (источник целевого факта 'explicit'|'auto') — для защиты explicit-фактов (§4.3): авто-режим НЕ переписывает/не супридит молча факт, введённый юзером явно (фронт покажет чип). `plan()` заполняет из `cands[idx].source`.
- **Откат группы** `memory_consolidate_undo(opGroup)` + `consolidate::undo` (§4.6 «откат последней консолидации») — реверсивность авто-действий: реверсит `update` (вернуть текст) / `supersede` (восстановить старый + удалить новый) / `add` группы, пишет компенсирующие `restore`/`delete` события, переиндексирует. **Optimistic-безопасно:** реверсим только если состояние = то, что группа оставила (текст не правили после, факт ещё супридён) — иначе пропуск (правка юзера цела); любая комбинация пропусков безопасна (max оба факта живы), идемпотентно.
- 5 тестов отката (supersede→восстановление+удаление, update→ревёрт текста, edited-new→пропуск, unknown→no-op, идемпотентность); 484 бэкенд-зелёных.

### Память агента: consolidation_eval — гейт доверия авто-режиму (MEM-8c-a)

Детерминированный eval-харнесс качества консолидации (план §4.5) — **предохранитель, разблокирующий авто-DELETE** (MEM-8c). По образцу `classify.rs` (EVAL-AI).
- **Гейт = именованные владельцем критерии БЕЗОПАСНОСТИ:** **DELETE-precision** ≥ 0.9 (доля предложенных DELETE, которые реально контрадикция по правильной цели; ложный DELETE = ошибочное устаревание факта) + **UPDATE-quality** ≥ 0.8 (UPDATE на верной цели И объединённый текст сохранил `mergeMustContain` И НЕ содержит `mergeMustNotContain` — прокси «не теряет деталь / не галлюцинирует») + анти-вырожденность (`predicted_delete > 0`). `op_accuracy`/`delete_recall` — ИНФОРМАЦИОННЫЕ (полезность, не безопасность): консервативная модель (пропуск контрадикции) безопасна, гейт меряет ложные удаления, а не безопасные промахи (находка adversarial-ревью).
- **Golden-набор** `eval/consolidation_eval.json` — 37 кейсов (контрадикция→DELETE / уточнение→UPDATE / парафраз→NOOP / новое→ADD), включая **мульти-кандидатные** (3 факта, НЕнулевая цель) и distractor (родина-vs-текущий-город) — иначе выбор правильной цели DELETE не проверялся бы (в проде модель видит до 6 кандидатов).
- **Гейт тестируем БЕЗ LLM:** «триггер-хэппи DELETE» проваливает (ловит опасный регресс), «всегда ADD» и «никогда не удаляет» валятся (UPDATE-quality / анти-вырожденность), идеальный проходит — 6 детерминированных тестов в CI.
- **Live-точка** `live_consolidation_meets_gate` (`#[ignore]`) — реальный `consolidate::decide` (основная модель, t=0). **Прогон на gemma-26B:** DELETE-precision **1.000** (0 ложных удалений на 37 кейсах вкл. distractor), UPDATE-quality **1.000** — **гейт пройден**; модель консервативна (поймала ~5/11 контрадикций, op-accuracy ~0.84 — пропуск безопаснее ложного удаления). Безопасность стабильна между прогонами (precision/update-quality = 1.0), тогда как op-accuracy флоат 0.78↔0.84 у границы → правильно, что он не в гейте. Это число доверия для разблокировки авто-режима (MEM-8c-b).

### Память агента: фронт консолидации — режим «Предлагать» (MEM-8b)

Подключает движок консолидации (MEM-8a) к UI за флагом `aiMemoryConsolidation` (OFF по умолчанию, тоггл в Настройках→AI, недоступен без включённой памяти агента). Режим «Предлагать»: каждое слияние/замещение — через ваш клик, **обратимо по построению** (ничего не применяется молча).
- **Поток:** при ✓ на чипе факта (флаг ON) считается `consolidate_plan`. Тривиальные операции (`add` / уже-покрыто `noop`) применяются сразу; `update` (дополнить) и `supersede` (заменить устаревший) показываются **чипом-предложением с diff «было … → станет …»** и кнопками **Объединить/Заменить** · **Оставить оба** · **×** (отклонить). Флаг OFF → поведение 1:1 со старым авто-захватом.
- **Безопасность UI:** `epoch-гард` (после async `plan` факт-чип мог смениться — не действуем устаревшим); `pendingConsolidation` сбрасывается на всех путях очистки ленты (новый обмен / clear / смена сессии); мок-зеркало контракта `op` (урок «Mock must match backend»); тоггл-гейт `disabled` без памяти агента.
- 9 тестов стейт-машины стора (флаг OFF/ON × add/noop/update/supersede, resolve accept/keepSeparate, dismiss, fail-safe при сбое plan, epoch-гонка) — 789 фронт-зелёных; превью-сверка тоггла + i18n. Авто-режим + eval-гейт — MEM-8c (owner-gated).

### Память агента: бэкенд консолидации ADD/UPDATE/DELETE→supersede/NOOP (MEM-8a)

Фундамент главного дифференциатора памяти (план [`docs/specs/agent-memory-mem0.md`](docs/specs/agent-memory-mem0.md) §4) — за мастер-флагом `aiMemoryConsolidation` (OFF по умолчанию, фронт подключается отдельным срезом MEM-8b). При записи факта семантически близкие существующие + **основная модель** (`ctx.ai.chat`, 27B — решение владельца) решают одну операцию; «дедлайн пятница» и «дедлайн среда» больше не сосуществуют, отравляя контекст.
- **Двухфазно:** `memory_consolidate_plan` (read-only — считает предложение, НИЧЕГО не пишет) → `memory_consolidate_apply` (применяет выбор `Accept`/`KeepSeparate` в ОДНОЙ транзакции). Точный дубль живых → `Noop` без LLM; пустой индекс / нет близких выше `MEM_CONSOLIDATE_THRESHOLD=0.55` → `Add` без LLM.
- **Безопасность структурная, не вероятностная:** **fail-closed = ADD** при любой неопределённости (битый JSON / нет модели / ошибка / id вне диапазона / неуверенность — LLM никогда не удаляет и не переписывает при сомнении); **DELETE = soft-supersede** (факт не удаляется физически, `superseded_by` + история + `op_group` → обратимо); **optimistic-чек** перечитывает целевой факт под writer-локом (текст изменился / исчез / уже супридён с момента plan → деградация в ADD — закрытие окна гонки через долгий LLM); временные числовые id существующих фактов (анти-галлюцинация).
- **Adversarial-ревью (5 линз) до мержа** — учтены: каждый факт приводится к ОДНОЙ строке (`one_line`) — текст с переносом не порождает фейковую пронумерованную строку → подмену id → ложный DELETE; supersede НЕ применяется, если новый факт не создан (кандидат совпал с другим живым фактом) — иначе target указал бы на несвязанный курированный факт; новый факт supersede-операции пишет `add`/`restore`-событие в ТУ ЖЕ `op_group`, что и supersede старого (полный групповой откат); `apply` тримит кандидата на входе (текст строки == текст вектора == ключ дедупа); UPDATE без реальной правки (new==old) → `Noop` (не плодим пустое событие/ре-эмбед); индексируем новый ДО снятия старого из ANN (over-recall, не дыра, при сбое).
- 22 теста стейт-машины (op-парсер fail-closed, plan-замыкания, UPDATE/supersede/NOOP/ADD, гонки, op_group, soft-supersede + восстановление, анти-инъекция). Авто-режим + чип-UI + eval-гейт `consolidation_eval` — следующими срезами (MEM-8b/8c).

### Память агента: извлечение N фактов за обмен через JSON (MEM-9)

Авто-предложение фактов больше не теряет данные и не ломается о форматирование модели.
- **N фактов вместо ≤1:** «быстрая» модель извлекает все стойкие факты обмена строгим JSON
  `{"facts":[...]}` (атомарные утверждения). Раньше — ≤1 факт голой строкой, второй/третий терялись.
- **Робастный парсинг:** `parse_facts` берёт JSON от первой `{` до последней `}` (терпит markdown-ограду
  и прозу вокруг), с **фолбэком** на старый одно-фактовый разбор при не-JSON-ответе; внутрипакетный дедуп
  без учёта регистра; кап `MAX_FACTS_PER_TURN=5`. Пустой `facts:[]` = «нечего» (не уходим в фолбэк).
- **UI:** факты предлагаются по одному чипу через очередь (`pendingFactQueue`) — подтверждение/отклонение
  продвигает к следующему; компонент-чип не менялся. Ноль молчаливых записей сохранён (D1): каждый факт —
  явный ✓/✗. Команда `memory_propose` теперь возвращает массив; явное «запомни …» берёт первый.

### Память агента: бюджет инъекции — кап пинов + обрезка длинного факта (MEM-10)

Гигиена контекста (снижение ШУМА, не token-saving — факты в БД/панели целы). План [`docs/specs/agent-memory-mem0.md`](docs/specs/agent-memory-mem0.md).
- **Кап пинов** `MEM_MAX_PINS=12` в `context_facts`: пины «всегда» (D2), но не десятками — иначе раздували
  бы промпт и вытесняли заметочный RAG. Берём свежайшие; релевантный «лишний» пин всё равно может всплыть
  через top-k. Остальные видны в панели.
- **Обрезка факта при инъекции** до `MEM_FACT_INJECT_MAX_CHARS=280` (UTF-8-безопасно, с «…») — длинный
  импортированный факт больше не засоряет контекст; в БД остаётся целым.
- Дедуп факт×реплика-переписки при инъекции (§4.7) — отложен в BACKLOG.

### Память агента: история/версии фактов (MEM-7, миграция 018)

Бэкенд-фундамент обратимости под консолидацию (MEM-8) — план [`docs/specs/agent-memory-mem0.md`](docs/specs/agent-memory-mem0.md).
- **Журнал событий факта** (`memory_fact_events`, мигр. 018): правка/удаление/замещение/восстановление с
  `old_text`/`new_text` и `op_group` (для группового отката составных операций MEM-8). Без FK-cascade —
  аудит переживает физическое удаление факта.
- **`edit`/`delete` пишут события** в той же транзакции (правка тем же текстом события не плодит).
- **Supersede-колонки** `superseded_by`/`superseded_at` + **инвариант** «факт жив ⟺ `superseded_by IS NULL`»:
  `list`/`count`/`context_facts` исключают супридённые ЗАРАНЕЕ (в MEM-7 ничто не супридит — фильтр
  установлен под MEM-8). Команда `memory_fact_history` для «истории факта» (UI — следующим срезом).

### Безопасность: git2 0.20 → 0.21 (RUSTSEC-2026-0183/0184)

Два свежих unsound-адвайзори против `git2 0.20.4` (потенциальный UB в `Remote::list()` и в `Signature`
из buffer-созданного `BlameHunk`) роняли `cargo-deny` репо-широко (на main и всех PR). Бамп до 0.21.0
закрывает оба. API-правки: `StatusEntry::path()` и `Remote::url()` теперь возвращают `Result` вместо
`Option` — поправлено в `git/mod.rs` (чтение статуса/URL origin), логика push/pull не затронута.
Vendored libgit2 + vendored openssl как были. 9 git-тестов зелёные.

### Память агента: порог близости в ретривале + честный score (MEM-6)

Первый срез эпика доработок памяти по мотивам Mem0 (план — [`docs/specs/agent-memory-mem0.md`](docs/specs/agent-memory-mem0.md)).
- **Порог близости фактов:** `context_facts` теперь отсекает не-пин-факты ниже `MEM_SIM_THRESHOLD` (косинус,
  bge-m3). Раньше top-k **всегда** добивался до `k` любыми хитами — в контекст ответа лезли нерелевантные
  факты при любом непустом индексе памяти. Пины инжектятся безусловно (D2). Порог консервативный (режет
  только near-orthogonal шум, recall не регрессирует); точное значение калибруется на dev-vault по
  наблюдаемым score под eval-гейтом. Фильтр вынесен в чистую `ids_above_threshold` + юнит-тест.
- **Честный score:** `chat_log::resolve_memory_hits` пробрасывает реальную similarity из ANN вместо
  хардкода `0.0` (плашка «Из прошлых разговоров» показывала 0%). `search_memory` тянет `(id, score)` пары.

### Команда «Скопировать заметку как Markdown» (COPY-AS-MARKDOWN)

Палитра → «Скопировать заметку как Markdown» кладёт исходный markdown активной заметки в буфер обмена
(полезно в режиме чтения, где нет редактора для select-all; и одним действием на весь документ). Берём
живой текст буфера (с несохранёнными правками), не читаем диск. Toast об успехе/сбое; нет активной
заметки → честная подсказка. +2 теста (успех/нет-заметки).

### Авто-рефреш Backlinks/Mentions по `vault:changed` (#301)

Бары обратных ссылок и незалинкованных упоминаний теперь пере-запрашиваются не только при смене файла,
но и при `vault:changed` (индексатор отработал) — связь, добавленная из **другой** заметки, появляется
без переоткрытия. Дебаунс 1500мс (как в статусбаре). Фоновый рефреш «тихий»: не дёргает loading и **не
обнуляет** список на транзиентной ошибке бэкенда (урок #296). Race-safe: монотонный токен запроса +
`alive`-флаг применяют только последний ответ. +4 теста.

### Команда «Переименовать активный файл» (F2) (#300)

`F2` (и палитра) раскрывает каталоги-предки активного файла и запускает инлайн-переименование его строки
в дереве — OS-стандартный жест. Сам rename флашит грязные буферы ДО переноса — несохранённое цело.
**useKeymap теперь пускает голые функц. клавиши F1–F12** (они не текст; `preventDefault` только при
совпавшей команде → F5-reload и пр. проходят насквозь). **Adversarial-ревью нашёл MAJOR:**
`window`-keydown ловит F2 даже из инпута (нативное всплытие сквозь `stopPropagation`) → F2 в самом
rename-input пере-сидил бы введённое имя. Фикс: голую F-клавишу не перехватываем из формового поля
(INPUT/TEXTAREA/SELECT); `contentEditable` (CM6) пропускаем намеренно — F2 переименовывает открытый файл.

### Команда «Показать файл в дереве» (reveal active file) (#299)

Палитра → «Показать файл в дереве» раскрывает каталоги-предки активного файла и скроллит/подсвечивает его
строку (открыв заметку через ⌘O/палитру/ссылку/граф, в дереве она раньше не подсвечивалась). Раскрытие
персистится; FileTree ищет строку в уже-раскрытом дереве (idx из пост-раскрытия, не stale) и скроллит
ровно один раз. +тесты vault/ui/FileTree.

### Поиск/замена в открытой заметке (⌘F) — `@codemirror/search`

Внутри заметки теперь есть поиск и замена (⌘F открывает панель CM6 с полями поиска/замены, навигацией по
совпадениям и подсветкой). Раньше поиск был только глобальный по vault в сайдбаре.
- Панель — стандартная CM6 (DOM-средствами; **CSP не трогаем** — CM6 льёт стили через `adoptedStyleSheets`,
  не `<style>`, тем же путём, что уже работающий редактор; проверено по исходникам). Стиль панели под токены приложения.
- Мультикурсор НЕ включаем (отдельный визуальный срез) → из `searchKeymap` убраны `Mod-d`/`Mod-Shift-l`
  (без `allowMultipleSelections` они тихо схлопывались).
- **Adversarial-ревью (по исходникам пакетов) нашёл MAJOR:** `⌘G` (найти дальше) всплывал в глобальный
  `useKeymap` и ВДОБАВОК тоглил граф (`view.graph`=mod+g) — `searchKeymap` не ставит `stopPropagation`.
  Фикс на уровне глобального хендлера: `if (e.defaultPrevented) return` (закрывает класс утечки для всех
  CM6-биндов). +3 теста useKeymap. CSP/Escape-приоритет/автосейв-replace/focus — подтверждено чистым.

### Свёрнутость дерева файлов переживает перезапуск (TREE-EXPANDED-PERSIST)

Раскрытые папки дерева теперь сохраняются между сессиями (localStorage, по образцу избранного, но
**с привязкой к vaultRoot** — иначе раскрытие одного vault протекло бы в другой с тем же путём). На
открытии vault грузятся дети раскрытых папок (иначе пометка expanded ничего не показала бы); исчезнувшие
снаружи папки отсеиваются. Свёрнутость чистится при удалении/переносе папки.
- Adversarial-ревью нашёл и закрыл 2 дефекта: **CRITICAL гонка** — `await` догрузки расширил окно
  re-entrant `openVault`, и быстрый A→B мог показать дерево A → epoch-токен отбивает устаревшую
  continuation; **MAJOR orphan** — сворачивание родителя не забывало потомков (`a/b` без `a` в персисте,
  невидимая загрузка на рестарте) → сворачивание теперь забывает поддерево. +4 теста (round-trip, prune,
  collapse-orphan, гонка). Грабля: node-тестовый localStorage не работает → Map-стаб в тесте.

### Клик по `[[wikilink]]` на несуществующую заметку создаёт её на лету

Раньше `openLink` при ненайденной заметке молча ничего не делал — мёртвый клик, ломавший базовый
Obsidian-workflow «пиши ссылку → наполнишь позже». Теперь (как в Obsidian) создаём `X.md` и открываем.
Разбор `[[folder/note]]` на каталог+имя, чистка недопустимых для ФС символов имени. Важно: различаем
«заметки нет» (resolveNote→null → создаём) и «резолв упал» (throw → НЕ плодим мусорный файл на
транзиентной ошибке бэкенда). Ошибка записи → toast. +3 теста.

### Фикс: триаж Inbox — вторая операция подряд больше не «съедается» сдвигом строк

GTD-разбор Inbox (INBOX-1) сверял строку по зафиксированному `item.line`, но после первого действия
(в задачу/в заметку/удалить) номера строк сдвигаются, а панель держит исходные номера → вторая операция
матчила ЧУЖУЮ строку и тихо превращалась в no-op (`consume` возвращал false). Теперь drift-guard
матчит по СТАБИЛЬНОМУ ключу `time+text` и режет ФАКТИЧЕСКУЮ текущую строку (`removeLine(doc, cur.line)`).
+регресс-тест (две операции подряд по разным элементам).

### Фикс: кириллические/Unicode `#теги` кликабельны в режиме чтения

`remarkNexus` подсвечивал/делал кликабельными только ASCII-теги (регэксп по устаревшему
`is_ascii_alphabetic`), а бэкенд-индексатор уже хранит Unicode-теги (`parser` `is_tag_char` =
`is_alphanumeric() | _-/` + хотя бы одна буква; есть тест `unicode_cyrillic_tags`). Итог — RU-теги `#идея`
в превью были мёртвым текстом, хотя в индексе/графе/панели тегов живые. Регэксп переведён на Unicode
(`\p{L}\p{N}_/-`, минимум одна `\p{L}`) — зеркалит бэкенд; клик нормализуется в lowercase на границе
(как `to_lowercase` бэкенда). +3 теста (кириллица, вложенный `#проект/идея`, `#123` не тег).

### Live Preview — frontmatter как Properties-таблица (эпик §13, срез 9)

Ведущий YAML-frontmatter в режиме чтения рендерится **Properties-таблицей** (ключ→значение, по образцу
Obsidian) вместо утечки сырого `---\nkey: val\n---` текстом + лишнего `<hr>`.
- `lib/markdown/remarkFrontmatter.ts` — убирает frontmatter из markdown-рендера **БЕЗ сдвига строк тела**:
  НЕ режет исходник (это сломало бы 1-based строки EDIT-5 тогл-тасков и EDIT-7 оглавления), а удаляет
  top-узлы целиком в строках `[1..endLine]` блока (тело строки > endLine сохраняет позиции). Удаление по
  ДИАПАЗОНУ строк, а не типам узлов — без remark-frontmatter `---\nk:v\n---` парсится неоднозначно.
- `lib/markdown/frontmatter.ts` — лёгкий разбор (`k: v` / `k: [a,b]` / `k:`+`  - item`, не полный YAML).
- `components/editor/PropertiesTable.tsx` — div-grid (не `<table>`, чтобы не конфликтовать с GFM-таблицами);
  поля-теги (tags/aliases) — кликабельные `#`-чипы (фильтр сайдбара, lowercase). У embed'ов frontmatter
  уже срезан (NoteEmbed) → таблицы нет.
- Тесты: 12 юнитов (extract/parse/remove-без-сдвига) + 5 рендер-тестов, **в т.ч. критический line-offset**:
  таск после frontmatter тоглит строку 5 ПОЛНОГО исходника. Adversarial-скептик сверил инвариант строк
  против РЕАЛЬНОГО remark-парсера (adjacency/setext/CRLF/пустой/body-`---`) — defects не найдены. Сюита 748 зелёная.

### Live Preview — комменты `%%…%%`, сноски `[^1]`, якоря заголовков (эпик §13, срез 8)

Три добивки режима чтения одним срезом (все autonomy-safe, без новых зависимостей/CSP):
- **COMMENT-1**: Obsidian-комментарии `%%скрыто%%` не показываются. `lib/markdown/remarkComments.ts` —
  remark-плагин на text-узлах (inlineCode/code-fence не трогает), ставится ПЕРВЫМ (закомментированные
  `[[ссылки]]`/`#теги`/callout-маркеры не обрабатываются). Неполный `%%` без пары — литерал. Покрывает
  инлайн и блок в одном абзаце; блок через пустую строку — задокументированная граница (редкий синтаксис).
- **FOOTNOTE-1**: сноски GFM `[^1]` (remark-gfm их парсил, не было стиля) — типографика `.footnotes`/
  надстрочников + плавный скролл по якорю `#id` В ПРЕДЕЛАХ превью (back-ref сносок и заголовки; не
  `target=_blank`, который ломал хеш-навигацию). `decodeURIComponent` под try/catch (находка ревью: `#50%`
  ронял клик).
- **HEADANCHOR-1**: заголовки получают slug-`id` (`lib/editor/slug.ts`, per-render дедуп `intro`/`intro-1`,
  Unicode сохраняется) → якоря для `#heading`-навигации и back-ref сносок. OutlineBar/транклюзия не задеты
  (они по `data-outline-line`/тексту).
- Тесты: 23 (8 slug + 7 remarkComments + 8 рендер: коммент скрыт/code-fence/slug-дедуп/footnote-рендер+скролл/
  `#50%`-гард). Adversarial-скептик: закрыт **MAJOR** (URIError на `#50%`); порядок плагинов закреплён
  комментарием (reorder remarkComments после embeds стёр бы вставки). Вся фронт-сюита 732 зелёная.

### Live Preview — кликабельные `#tag`-чипы → фильтр сайдбара (эпик §13, срез 7)

Клик (или Enter/Space) по `#tag`-чипу в режиме чтения открывает панель поиска сайдбара с ТОЧНЫМ
фильтром по этому тегу (как клик в панели «Теги»). Раньше чип был мёртвым `<span>`.
- `stores/ui.ts`: `pendingTagFilter` + `openTagFilter(tag)` (показывает сайдбар, выходит из reading) +
  `consumeTagFilter()`. `Sidebar` читает отложенный тег эффектом → `searchByTag` и сбрасывает.
- `MarkdownPreview`: проп `onOpenTag` (прокинут через embed-рекурсию и из `GroupPane`); чип —
  `<span role=button tabIndex=0>` (а не `<a>`, чтобы `.preview a` не перебивал стиль). Без `onOpenTag`
  (доска/peek) чип остаётся не-кликабельным — честно, без мёртвого клика.
- Adversarial-скептик нашёл **MAJOR**: бэкенд хранит теги в нижнем регистре (`parser.to_lowercase` +
  точный матч `notes_by_tag`), а клик слал тег как написано → `#TODO` давал пустую выдачу. Фикс:
  `.toLowerCase()` на границе (отображение чипа — как написано), +регресс-тест. Остальное (петли эффекта,
  unmount-тайминг, кодирование `#a/b`, a11y) — подтверждено чистым.
- Тесты: store-юниты (openTagFilter/consume) + 4 рендер-теста (клик/Enter/lowercase/не-кликабелен без проп).

### Live Preview — выделение `==текст==` (highlight) в режиме чтения (эпик §13, срез 6)

Obsidian-выделение `==текст==` рендерится `<mark>` (мягкая жёлтая плашка, читаемая во всех 4 темах).
- `lib/markdown/remarkHighlight.ts`: mdast-плагин на residual `text`-узлах (внутрь inlineCode/code-fence не
  лезет, как [[wikilink]]/#tag), эмитит нативный `<mark>` через data.hName — **без сырого HTML, CSP не трогаем**.
  Поставлен ДО remarkNexus → `==[[Note]]==` даёт mark с вложенной вики-ссылкой.
- Не путается с GFM `~~strike~~`/`**bold**` (разные узлы), setext-подчёркиванием `==` (блок-уровень) и `===`.
- Тесты: 12 юнитов (сплит/края: `===`/`==a=b==`/пробельное-внутри не выделение) + 3 рендер-теста (`<mark>`,
  соседство со strike/bold, code-fence не трогается). Adversarial-скептик нашёл **MAJOR ReDoS** — двойной
  ленивый квантор давал O(n²) на длинной строке без закрывашки (UI-фриз ~2 с на 100k); заменён на один
  ленивый + литерал `==` (линейно), добавлен перф-регресс-тест.

### Live Preview — Callouts/admonitions `> [!note]` в режиме чтения (эпик §13, срез 5)

Цитаты вида `> [!note] Заголовок` (Obsidian-callouts) теперь рендерятся цветными admonition-блоками с
иконкой по типу. Поддержаны типы note/abstract/info/todo/tip/success/question/warning/failure/danger/
bug/example/quote + алиасы (`hint`→tip, `error`→danger, `summary`→abstract, …); неизвестный тип → нейтральный
note. Сворачивание `[!note]-` (свёрнут) / `[!note]+` (развёрнут) — клик/Enter/Space по шапке.
- `lib/markdown/remarkCallouts.ts`: mdast-трансформер (чистые `parseCalloutMarker` + `splitInlineAtNewline`,
  тестируются) → кастомный узел `nexus-callout` (приём data.hName/hProperties, как у транклюзии); поставлен
  ДО `remarkGfm` — тело callout проходит обычный GFM/wikilink/math-конвейер.
- `components/editor/Callout.tsx`: иконка — инлайновый SVG (lucide, `currentColor`); цвет/тинт — классами +
  `data-callout`-селекторами (`color-mix` поверх поверхности + solid-фолбэк), **без inline-style**;
  сворачивание — React-state. Никакого `dangerouslySetInnerHTML`/rehype-raw — **CSP не трогаем**.
- Тесты: 20 юнитов (маркер/сплит/трансформ-дерево, в т.ч. **жёсткий перенос `break`-узлом**) + 6 рендер-тестов
  (data-callout, инлайн-SVG иконка, дефолтная подпись, сворачивание, обычная цитата не трогается, `[[wikilink]]`
  в теле). Adversarial-ревью (4 линзы): найден и закрыт MAJOR — hard-break после маркера поглощал тело в заголовок.
- Пиксельный скриншот не снят (welcome-экран без открытого vault) — путь рендера покрыт jsdom-тестами, CSS собран.

### Live Preview — math-шрифт (STIX Two Math) для MathML на Win/Linux (эпик §13, срез 4)

KaTeX в режиме чтения рендерит `output:'mathml'` (нативный `<math>` без inline-стилей — CSP не трогаем).
На macOS WebKit `<math>` берёт системный math-шрифт из коробки, а на **Chromium/WebView2 и WebKitGTK без
шрифта с OpenType-MATH-таблицей** ломаются растяжимые скобки/интегралы/большие операторы. Забандлен
**STIX Two Math** (self-hosted woff2 из `@fontsource/stix-two-math` — CSP `font-src 'self'` уже разрешает),
применён к `.preview math`.
- Грабля: пакет `@fontsource` тегает woff2 как «latin» (`unicode-range: U+0000-00FF`), что отрезало бы
  math-символы — НО сам файл содержит ПОЛНЫЙ шрифт (4605 глифов + MATH-таблица, проверено fonttools).
  Поэтому свой `@font-face` БЕЗ `unicode-range` (`src/math-font.css`), а не импорт его `index.css`.
- Без бинаря в репо (шрифт — npm-зависимость), без изменения CSP. Build бандлит woff2 (394K), `document.fonts`
  регистрирует+грузит STIX. **Визуальную правку скобок на Win/Linux с macOS не проверить** (там MathML и так
  ок) — механизм стандартный (font-family с MATH-таблицей).

### Live Preview — Mermaid-диаграммы под строгим CSP (эпик §13, срез 3)

Блоки ` ```mermaid ` теперь рендерятся диаграммами в режиме чтения. Владелец выбрал полосу **SVG-санитайз
без ослабления CSP** (vs CSP-nonce): mermaid эмитит SVG со встроенным `<style>` (под `style-src 'self'`
заблокирован), мы **переносим эти стили в SVG presentation-атрибуты** (`fill`/`stroke`/`font-*` — это
АТРИБУТЫ, не подчиняются `style-src`) и срезаем `<style>`/`style=`/`<script>`/`<foreignObject>`/`on*`/
`javascript:`-ссылки. Итог: полностью стилизованная диаграмма под строгим CSP, **0 нарушений CSP** (проверено
в превью: флоучарт + sequence рендерятся с темой mermaid, fill/border/стрелки на месте).
- `lib/markdown/mermaid.ts`: `parseCss` (лёгкий парсер, пропускает `@media`/`@keyframes`) + `cspSafeSvg`
  (строка→CSP-безопасная строка, тестируется на фикстуре) + ленивый `renderMermaid` (тяжёлый mermaid —
  отдельный async-чанк, `securityLevel:'strict'` + `htmlLabels:false` → `<text>` вместо `<foreignObject>`).
- `remarkMermaid` → узел `nexus-mermaid`; `MermaidDiagram.tsx` — ленивый рендер + `dangerouslySetInnerHTML`
  уже-санитизированного SVG (единственный осознанный, с XSS-гардом) + заглушки loading/error.
- Тесты: 10 транзформ-юнитов (presentation-перенос, strip `<style>`/`<script>`/`on*`, **XSS — `javascript:`
  вырезан**, невалидный вход) + 2 интеграционных (фенс→SVG / чужой язык остаётся кодом).
- **Дань:** mermaid тяжёлый (~100 транзит. пакетов) — поэтому ленивый импорт (вне основного бандла).

### Live Preview — картинки-вставки `![[pic.png]]` в режиме чтения (эпик §13, срез 2)

Следующий autonomy-safe срез после транклюзии (выбран скоуп-анализом 3 кандидатов: картинки · Ctrl+E ·
Mermaid — последний отложен владельцу как CSP-nonce-решение). Obsidian-синтаксис вставки картинок теперь
рендерится в reading-режиме: `![[diagram.png]]` и `![[pic.png|подпись|300]]` → `<img>` (alt + ширина
HTML-атрибутом, без inline-style — строгий CSP не трогаем).
- **Резолв по basename**: картинки НЕ в индексе (`files` — только `.md`), поэтому новая read-only команда
  `resolve_attachment` обходит vault за картинкой с таким именем — как basename-шорткат `[[ссылок]]`:
  КРАТЧАЙШИЙ путь, регистронезависимо, мимо служебных папок (`.nexus`/`.git`). Путь-с-сепаратором
  (`![[attachments/x.png]]`) — явный, через `resolve_vault_path` (анти-traversal). Рендер — существующим
  `read_attachment` → `data:`-URL (CSP уже разрешает `data:`).
- Маршрутизация в `remarkEmbeds`: image-расширение → узел `nexus-image` (картинка), иначе → `nexus-embed`
  (транклюзия заметки, срез 1). Потолок `MAX_EMBEDS_PER_NOTE` теперь общий для картинок и заметок.
- Мок (`mock/vault.ts`) зеркалит контракт обеих команд (basename-обход + placeholder-SVG) — превью без Tauri
  показывает картинку.
- **Adversarial-ревью (3 линзы × верификация): 1 MAJOR + нит'ы — все пофикшены.** MAJOR — `read_attachment`/
  `resolve_attachment` читали `.nexus`/`.git` (напрямую `![](.nexus/x.png)` И через симлинк `lnk.png→../.nexus/`,
  т.к. `resolve_vault_path` канонизирует, но не отвергает служебные папки): закрыто `is_ignored`-гардом на
  канон-пути в ОБОИХ путях чтения (паритет с `is_pinnable`/permission; +unix-тест на симлинк). Нит'ы:
  `![[img|0]]` больше не 0px-картинка (фолбэк на натуральный размер); `ico` добавлен в backend `mime_for_ext`
  (паритет мок↔бэкенд по набору расширений).
- Тесты: 6 Rust (TempDir-обход: подпапка/регистр/коллизия-кратчайший/`.nexus`-скип/не-найдено/не-картинка) +
  5 `parseImageParams` + 4 интеграционных (резолв→img / alt+width / не-найдено-заглушка / image-путь≠транклюзия).
Файлы: `commands/attachments.rs` (+`resolve_attachment`/`find_image_by_basename`), `lib/markdown/{embed,remarkEmbeds}.ts`,
`MarkdownPreview.tsx` (`EmbedImage`+`VaultImage` width), `tauri-api.ts`, `mock/vault.ts`, i18n `embed.imageMissing`.

### Live Preview — транклюзия `![[embed]]` в режиме чтения (эпик §13, срез 1)

После walk-through больших пост-v1 эпиков (мультиагентная оценка: Live Preview · Home · плагины/marketplace ·
auto-updater · мобилка) выбран самый ценный autonomy-safe срез с ежедневной пользой: **вставка заметок**.
Блок-вставка `![[Заметка]]` и секции `![[Заметка#Заголовок]]` теперь раскрываются прямо в reading-режиме —
карточка с заголовком-ссылкой (открывает исходник, как клик по `[[вики-ссылке]]`) и **рекурсивно**
отрендеренным телом (frontmatter срезан, секция по якорю извлечена, вложенные ссылки/теги/math работают).
- Резолв цели — той же командой `resolve_note`, что клик по вики-ссылке (индексаторная семантика + алиасы).
- Гард-цикл по множеству предков (`![[сама-себя]]`, A→B→A) + бэкстоп по глубине (`MAX_EMBED_DEPTH=4`).
- CSP не трогали (тот же безопасный конвейер react-markdown без raw-HTML/inline-стилей).
- Детект блок-вставки — по точному срезу исходника (`node.position` offsets), без зависимости от токенизации.
- Adversarial-ревью (4 линзы × верификация, 0 major): пофикшены ложные `#`-заголовки внутри код-фенсов
  (`extractSection` отслеживает fenced-state) и добавлен потолок fan-out `MAX_EMBEDS_PER_NOTE=50`.
- Отложено с честными заглушками: картинки `![[pic.png]]`, блок-ссылки `#^id`, setext-заголовки (ATX-only,
  как весь аппарат заголовков), инлайн-вставки, Mermaid, Dataview, live-edit (inline-правки).
Файлы: `lib/markdown/{embed,embed-context,remarkEmbeds}.ts`, `components/editor/NoteEmbed.tsx`,
`MarkdownPreview.tsx` (+CSS, i18n `embed.*`); проверено превью (скриншот: 2 карточки `ready`).

### Эпик «Канбан/задачи с AI» — старт (ADR + спека + PROP-1)

Владелец заказал top-level раздел канбан-доски для личных задач с тесной AI-интеграцией, Obsidian-паритетом
Properties/тегов и разбивкой по проектам. Проведён мультиагентный анализ (5 разведчиков: конкуренты
Trello/Notion/Linear/Todoist/Obsidian-Kanban/Tasks/Projects/Logseq/Anytype · Obsidian Properties+теги ·
инфра Nexus · AI-инструменты · модель данных) → синтез **decision-complete ADR + спека** (21 решение,
13 слайсов) → 3 adversarial-рецензента (данные/выполнимость · AI-измеримость/Obsidian · scope/UX, 2×
needs-changes — все находки вложены). Доки: `docs/adr/ADR-008-kanban-tasks.md`, `docs/specs/kanban-board.md`
(§14 — обязательные поправки ревью + пересортированный порядок слайсов).
Ключевые решения: задача = заметка с frontmatter `status`; доска = вью над frontmatter (колонка = значение
status); DnD = хирургическая правка одного ключа через `atomic_write` (serde_yaml архивирован); порядок —
в board JSON; индексация — переиспользовать `frontmatter_fields`+`file_tags` (без новой таблицы);
Properties-паритет (реестр типов + 5 виджетов); AI MVP детерминированный + closed-vocab (vision-фичи под
`vision→AC`). Критичная коллизия с существующей checklist-моделью (`commands/tasks.rs`) разрешена:
сосуществуют (чеклист=подзадачи-в-заметке, доска=заметки-задачи).

- **PROP-1** — Unicode/кириллица-теги (первый слайс, owner-critical: владелец русскоязычный, `#тег` сейчас
  режется). `is_tag_char` стал char-предикатом (`is_alphanumeric() || _-/`), инлайн-скан тела и `push_tag`
  переведены на `char_indices()`/`chars()` + `to_lowercase()` (было byte-based ASCII в 3 местах — ревью).
  Теперь валидны `#идея`, `#проект/важное`, frontmatter `tags: [Проект, идея]`. Обновлён зафиксированный
  тест (где `'тег'` отбрасывался) + новый `unicode_cyrillic_tags`. Полный Rust-сьют 397 зелёных (downstream
  граф-теги/tags/индекс не сломаны), fmt/clippy чисто.
- **BOARD-1** — примитив хирургической записи ОДНОГО плоского frontmatter-ключа (фундамент DnD-доски,
  Properties-панели; `status`/`project`/`priority`/`due`). `parser::set_frontmatter_field` правит/добавляет
  ключ, сохраняя остальной YAML и тело байт-в-байт (serde_yaml архивирован → ручной write-back); команда
  `set_frontmatter_field` → `atomic_write` (SAFE-1) + снапшот истории (SAFE-5, manual), возвращает новый
  контент+хеш для анти-эхо `baseHash` (SAFE-3); валидация ключа `value_key` (анти-инъекция). Adversarial-
  ревью (3 линзы) поймал 5 находок ДО мержа — все закрыты с регресс-тестами и подтверждены верификатором
  (компиляция обеих сторон + 28k фаззинг-входов, 0 расхождений): **F1 CRITICAL** — читатель
  `frontmatter_fields` тупой edge-stripper (`trim_matches(['"','\''])`), не YAML-парсер → значения с
  кавычкой портились; фикс — `fm_value_repr` симулирует читатель и ОТКЛОНЯЕТ значение без round-trip
  (перевод строки/краевые кавычки/инлайн-список) вместо тихой порчи; общий `read_scalar` — единый источник
  для чтения и проверки; **F2 MAJOR** — многострочное значение инжектило лишнюю строку; **F3 MAJOR** — мок
  не зеркалил `value_key` (урок MEM-5); **F4 MAJOR** — правилось ПЕРВОЕ дубль-вхождение, читатель
  last-key-wins → правим последнее; **F5** — mixed-EOL при добавлении в CRLF-файл. Браузер-мок
  `setFrontmatterField` зеркалит контракт байт-в-байт. UI-консьюмер отложен в BOARD-5 (DnD с baseHash-sync).
  Rust 400 зелёных + clippy, фронт tsc/eslint/vitest (23 мок-теста).
- **BOARD-2 + BOARD-4** — выборка задач + top-level раздел «Доска». Бэкенд `board::list_board` (клон
  `goals::list_goals`): INNER JOIN `frontmatter_fields` по `status` = «только задачи», LEFT JOIN
  project/priority/due, теги — коррелированный `group_concat` из `file_tags`; параметр `status_key`
  (умолч. `status`); сорт по пути (ручной порядок — BOARD-3). Команда `list_board`. Фронт: новый
  активити-бар-вход «Доска» (mutually-exclusive primary-view рядом с Home/News в `ui.ts`: `boardOpen` +
  `openBoard`/`toggleBoard`/`closeBoard`, гасит chat/home/news), `BoardView` с колонками по статусу
  (дефолт todo/doing/done + виртуальная «Прочее» для статусов вне набора, §12 — задачи не теряются),
  карточки (приоритет с цветом, дедлайн с overdue-подсветкой, проект, теги), состояния
  загрузка/ошибка(последняя доска цела)/пусто, refetch на фокус окна (§14.6). Чистая модель
  `board-model.ts` (группировка/overdue/basename — юнит-тест 8 кейсов) + i18n `board.*` в en+ru
  (parity-тест). Браузер-мок `mock/board.ts` зеркалит контракт (сид под превью). Клик по карточке
  открывает заметку (peek/side-panel — BOARD-6). DnD/конфиг/порядок — BOARD-5/3. Rust 402 + clippy/fmt,
  фронт 545 тестов; превью verified (4 колонки, overdue красным).
- **BOARD-3** — персист доски: конфиг `.nexus/boards/<id>.json` (колонки с переименованием без правки
  файлов, ручной порядок карточек — фундамент DnD-реордера BOARD-5, scope folder/project/tags, statusKey).
  Модуль `board::config` (load/save через `atomic_write_io`; id валидируется — анти-traversal; битый JSON →
  дефолт+`corrupt`-флаг, пользовательский файл НЕ перезаписывается). Команды `get_board`/`save_board`/
  `list_boards`. `get_board` = конфиг + карточки в scope (фильтр в Rust) + in-memory GC порядка. Фронт
  `BoardView` переключён на `get_board`: колонки и порядок из конфига (`applyOrder` — в-order первыми, новые
  стабильно по пути), label-фолбэк (метка→локализация дефолтных→raw id), пилюля битого конфига. Self-heal
  порядка ТОЧЕЧНЫЙ: rename-хук в `rename_path` (path from→to, ПОЗИЦИЯ сохранена §14.6) + delete-хук в
  `delete_path`. Adversarial-ревью (3 линзы) поймал **F1 CRITICAL** (класс MEM-5): self-heal-персист GC
  порядка НА ЧТЕНИИ при холодном/отстающем индексе (`list_board` пуст) СТЁР БЫ ручной порядок живых задач →
  фикс: НА ЧТЕНИИ не персистим вообще (GC только для отображения), чистим точечно по реальному
  удалению/реордеру; **F2** дедуп колонок по id (дубль из ручного JSON ломал группировку last-wins); **F4**
  имя файла — источник истины id. Регресс-тесты на все три. Rust 412 + clippy/fmt; превью verified
  (ручной порядок применён: «Релиз 0.9» выше «Дизайн доски» в колонке «В работе»).
- **BOARD-5** — drag-n-drop карточек (фронт-only, переиспользует `set_frontmatter_field` BOARD-1 +
  `save_board` BOARD-3). Перетаскивание между колонками → смена `status`; внутри колонки → реордер.
  Нативный HTML5-DnD (`CARD_MIME`). Чистая модель `planMove` (board-dnd.ts) + стейт-машина `performMove`
  (§14.6): optimistic-апдейт → persist (статус через `set_frontmatter_field` + анти-эхо `baseHash`-sync
  через новый `workspace.syncBufferAfterWrite` SAFE-3; порядок через `save_board`) → ОТКАТ на точный
  снапшот при ошибке; MalformedFrontmatter = карточка не двигается; в «Прочее» ронять нельзя; `busy`
  блокирует параллельный ход. Adversarial-ревью (3 линзы, MANDATORY §14.7) поймал 3 находки ДО мержа, все
  закрыты с регресс-тестами: **R1 CRITICAL data-loss** — `saveBuffer` ГЛОТАЕТ ошибку записи (буфер
  остаётся dirty); при провале флаша `set_frontmatter_field` прочитал бы старый диск без правок тела, а
  sync затёр бы их → фикс: после флаша проверяем dirty, не удалось — отменяем ход; **R2 MAJOR off-by-one**
  — реордер ВНИЗ внутри колонки мазал на 1 (фильтр пути сдвигал индексы) → корректировка индекса; **R3
  MAJOR гонка** — focus-рефетч не гейтился `busy` → `load()` посреди хода, откат затирал свежие данные →
  гейт через `busyRef`. Фронт 558 тестов (board-dnd 7 + BoardView-DnD 4), tsc/eslint. Реордер-индикатор
  между карточками / live-watcher-пересчёт — хвосты BOARD-5 в бэклоге.
- **BOARD-6** — превью задачи (peek, спека §9): клик по карточке открывает правый side-panel с рендером
  ТЕЛА заметки (`MarkdownPreview`, frontmatter срезан `stripFrontmatter`) + сводкой свойств
  (status/priority/project/due/tags) + «Открыть в редакторе»; НЕ модалка-трап — доска видна и
  интерактивна рядом (можно тащить карточки, переключать превью). `TaskPeek.tsx` читает `readFileMeta` с
  `alive`-cleanup от гонок; превью исчезает, если карточка удалена при рефетче. Клик по `[[ссылке]]` в теле
  → `openLink` (резолв вики-цели). Клик-vs-drag не путаются (браузер гасит click после dragstart).
  Adversarial-ревью: SHIP, 1 MINOR (`stripFrontmatter` мог бы съесть тело, начинающееся с `---`-разделителя
  БЕЗ frontmatter — недостижимо для карточек доски: у задачи всегда есть `status`-frontmatter; в бэклог как
  робастность хелпера). Фронт 563 теста (stripFrontmatter, TaskPeek 2, BoardView peek-клик), tsc/eslint;
  превью verified (панель + свойства + тело, доска видна).
- **PROP-2** — реестр типов свойств (Obsidian Properties, спека §7): фундамент Properties-панели (PROP-3).
  Модуль `properties` — тип свойства ГЛОБАЛЕН по ИМЕНИ в `.nexus/property-types.json` (явные), иначе
  эвристика по значению `infer_type` (порядок: forced-tags `tags/aliases/cssclasses` → bool→checkbox →
  ISO-datetime → ISO-date → number → `[…]`-list → text; CSV-текст НЕ список — убрана ложная ветка).
  `resolve_type` (реестр > эвристика), `note_properties` (frontmatter-скаляры заметки → типизированные).
  Команды `get_property_types`/`set_property_type`/`get_note_properties`. load/save через `atomic_write_io`,
  битый JSON → пустой реестр (fail-safe). Браузер-мок `mock/properties.ts` зеркалит эвристику (MEM-5).
  Rust 4 теста (эвристика/resolve/note_properties/round-trip) + clippy/fmt; фронт мок-тест + tsc/eslint.
  Properties-панель с виджетами + инлайн-правкой — PROP-3 (потребитель этого read-пути).
- **PROP-3** — Properties-панель (Obsidian, спека §7): в превью задачи (BOARD-6) типизированные виджеты
  frontmatter-свойств с ИНЛАЙН-правкой через `set_frontmatter_field`. Виджеты MVP: text/number/checkbox/
  date (datetime — как текст); список/теги — read-only (чип-правка PROP-4); значение не под типом → жёлтое
  invalid-поле + «Править в source». Общий безопасный путь записи `lib/frontmatter-edit.ts`
  (`writeFrontmatterField`) — инкапсулирует урок BOARD-5 R1 (флаш грязного буфера, не удалось →
  `FlushFailedError`, frontmatter не тронут) + анти-эхо `syncBufferAfterWrite` (SAFE-3) в ОДНОМ тестируемом
  месте. Чистый `prop-widgets.ts` (`isValidForType`/`isChecked`/`isCalendarDate`). Правка свойства →
  доска перечитывается (карточка едет в новую колонку). Adversarial-ревью (3 линзы, MANDATORY §14.7):
  data-safety цела (бэкенд режет пустое/`[…]`/перевод-строки/край-кавычки; флаш-гард airtight — НЕ
  data-loss). FIX-FIRST по 3 MAJOR валидации (корень — `isValidForType` была слабее виджета/бэкенда):
  R1 date — календарная валидация (`2026-02-30` → invalid, не пустой date-input с ложной ошибкой); R2
  list/tags — read-only ветка с escape-в-source (не текст-инпут, который бэкенд отвергнет); R3 number —
  грамматика (без `0x`/`0b`/`Infinity`). + хвосты hygiene (осиротевшие i18n/CSS). R5/R6/R7 (минорный UX —
  фокус/флеш/дрейф) — в бэклог. Фронт 575 тестов (prop-widgets/frontmatter-edit-флаш-гард/PropertiesEditor
  3); превью verified (date-пикеры, инлайн-правка, доска видна). PROP-4 — автокомплит тегов.
- **PROP-4** — автокомплит тегов в редакторе (Obsidian-паритет, спека §8): печатаешь `#` или значение в
  frontmatter `tags:`-инлайн-списке → выпадашка имён тегов vault (источник `list_tags`). Чистый матчер
  контекста `editor/tag-complete.ts` (`tagCompletionQuery`) — юнит-тестируем без CodeMirror: инлайн `#тег`
  (Unicode/кириллица, вложенность `#a/b`), но НЕ заголовок `# ` (после `#` нужен tag-символ) и НЕ внутри
  инлайн-code-span (нечётные `` ` ``); frontmatter `tags: [a, b|` / `aliases: […`. `tagSource`
  CompletionSource смонтирован в ЕДИНЫЙ `autocompletion({override:[wikilink, tag, slash]})` (два инстанса
  конфликтуют — урок EDIT-6); контексты взаимоисключающие по regex; `from = pos − len(префикс)` (заменяется
  только набранный хвост, `#`/`[` сохраняются), фильтр по подстроке, лимит 50, `validFor` держит выпадашку
  при доборе tag-символов. `fetchTags` проброшен через `Editor`-props → `cb`-ref → `nexusExtensions`
  (актуален без пересоздания view); `GroupPane` отдаёт `vault.listTags() → имена`. Фронт-сьют +5 (контекст-
  матчер: инлайн/заголовок/code-span/frontmatter/не-контекст), tsc/eslint чисто; превью — редактор
  открывается, набор не сломан. Завершает Properties-эпик (PROP-2/3/4).
- **AI-1 (A1)** — «На доску»/промоут заметки в задачу канбана (спека §10, БЕЗ LLM — чистый frontmatter-
  контракт). Заметка без `status` → задача в ПЕРВОЙ колонке дефолт-доски (`personal`); статус-ключ и набор
  колонок берём из её конфига (уважаем кастом-колонки владельца). Уже есть непустой `status` → «уже на доске»
  (НЕ перетираем колонку — иначе откат doing/done в первую = потеря состояния, §12). Точки входа: контекст-
  меню дерева (только для файлов, `menu.isDir`-гейт) + команда палитры `board.promote` (активная заметка).
  Чистый `lib/board-promote.ts` (`promoteToBoard`) — юнит-тестируем; orchestration (тост исхода с
  локализованной колонкой + открыть доску) в `commands-core`. Запись — общий безопасный путь
  `writeFrontmatterField`. **Adversarial-ревью (скептик-Agent) поймал MAJOR**: `forNote` (guard «уже
  задача») читает ДИСК, а флаша грязного буфера не было → несохранённый `status: doing` не виден guard'у,
  запись откатила бы его в первую колонку (data-loss класса BOARD-5 R1). Фикс: выделен общий
  `flushBufferIfDirty` (из `writeFrontmatterField`), `promoteToBoard` флашит ДО чтения status; сбой флаша →
  `FlushFailedError`, ничего не пишем. Фронт-сьют +6 (промоут/кастом-ключ/already/пустой-status + 2 dirty-
  buffer: флаш-до-guard и FlushFailed-abort); i18n ru/en (4 тоста + tree/команда); tsc/eslint/583 зелёные;
  превью verified end-to-end (пункт меню только у файлов → тост с локализованной колонкой → доска открыта,
  ветка «уже на доске» без перетирания). MINOR (отложен): промоут заметки ВНЕ scope кастом-доски ставит
  status, но карточка не покажется на ней — тост честно не отражает (дефолт-scope пуст = все, бьёт лишь при
  суженном scope) — в BACKLOG. Старт AI-набора (#127): далее AI-2a/2b → EVAL-AI → AI-2c.
- **AI-2a (A2)** — «застрявшие» задачи (спека §10 A2, ДЕТЕРМИНИРОВАННО — SQL по `edit_events`, БЕЗ LLM).
  Бэкенд `board::stale_tasks` (+команда `stale_tasks`): задачи (есть `status`), не правленные ≥ N дней
  (умолч. 14) — `last_edit = COALESCE(MAX(edit_events.ts), files.updated_at)` (честная ось правок P2,
  фолбэк mtime; touch/синк без смены контента НЕ «освежает»), `HAVING last_edit ≤ cutoff`, сорт «застряло
  дольше» сверху. Done-like здесь НЕ отсеивается (бэкенд не знает колонок) — фронт фильтрует по конфигу
  доски (`filterStuck`, чистый/тестируемый: терминальные колонки убраны, статус вне набора = в работе =
  застрял). UI: пилюля «Застрявшие · N» в шапке доски → раскрывает список (заголовок + «N дн. без правок»),
  клик открывает заметку. Запрос best-effort (его сбой НЕ рушит доску); рефетч на фокус/Обновить. Бэкенд
  +1 тест (порог + edit_events перебивает свежий mtime + не-задача/done-included/сорт); фронт +3 (filterStuck:
  done-like по `normalizeStatus`, «Прочее»=застрял, нет done-like→ничего); мок зеркалит контракт (включает
  done-задачу → превью реально гоняет фильтр). cargo fmt/clippy/test + tsc/eslint/586 зелёные; превью
  verified end-to-end (пилюля «· 2» из 3 stale, done отсеян, сорт, клик → заметка). Adversarial-ревью
  (скептик-Agent): 0 CRITICAL/MAJOR; применена MINOR-устойчивость (saturating-арифметика порога при прямом
  вызове lib). NIT (пилюля обновляется на след. фокус/рефреш после DnD-хода) — приемлемо (модель доски).
- **AI-2b (A3)** — «план на день» (спека §10 A3, ДЕТЕРМИНИРОВАННО, БЕЗ LLM, БЕЗ нового бэкенда). Чистый
  `planDay(cards, columns, today, limit=7)` отбирает активные (не done-like) задачи в фокус по причине-
  корзине: `overdue` (дедлайн в прошлом) → `today` (дедлайн сегодня) → `priority` (urgent/high без срочного
  дедлайна); внутри overdue/today — раньше-дата выше, затем приоритет; в priority — приоритет; всюду
  тай-брейк по пути; обрезка до `limit`. Задачи без причины (нет дедлайна и не высокий приоритет) в план НЕ
  попадают — он сфокусирован. Работает на УЖЕ загруженных карточках доски (без сети). UI: пилюля «План дня ·
  N» в шапке → панель с бейджем причины (просрочено/сегодня/приоритет), клик открывает заметку. Панели
  AI-2a/2b взаимоисключимы (единое состояние `aiPanel: 'stale'|'plan'|null`). Фронт +4 теста (корзины+сорт /
  без-причины-исключены / done-like-исключён / urgent>high+limit); tsc/eslint/590 зелёные; превью verified
  (пилюля «· 3»: 1 overdue + 2 priority, бейджи/сорт, взаимоисключимость стале↔план = max 1 панель → 0).
  Adversarial-ревью (скептик-Agent): 0 CRITICAL/MAJOR; исправлено по ревью — шэдоу локального `plan` в
  `performMove` (→ `movePlan`) + эффект сброса `aiPanel` при опустевшем списке (панель не «вспомнит»
  открытость). MINOR (дедлайн с тайм-компонентом вне `YYYY-MM-DD`-контракта молча выпадает — пред-существующая
  деградация, как у overdue-бейджа) — оставлено. Завершает детерминированную часть AI-набора; далее EVAL-AI
  (нулевой слайс) → AI-2c (авто-тег, eval-гейт).
- **EVAL-AI** — нулевой слайс ПЕРЕД AI-2c (спека §14.3, MAJOR): ДЕТЕРМИНИРОВАННЫЙ classification-харнесс
  качества closed-vocab авто-тега, БЕЗ LLM (чтобы сам гейт был тестируем). `eval/classify.rs`: МИКРО
  precision/recall/F1 на уровне (заметка → множество тегов) — TP/FP/FN суммируются по кейсам; closed-vocab
  hard-fail (`out_of_vocab`: предсказанный тег вне словаря = FP + проваленный инвариант, `suggested_new`
  ВЫКЛ); гейт `meets_thresholds` = `out_of_vocab==0 && precision≥0.8 && recall≥0.5` (спека §10 A4). Фикстура
  `eval/tag_golden.json` (6 заметок, мульти-лейбл + zero-gold-кейс, словарь из 6 тегов). **Фиктивный
  (без-LLM) классификатор в тестах ДОКАЗЫВАЕТ, что гейт ловит каждый регресс**: идеальный→проходит,
  ленивый→recall=0 валит, жадный→precision валит, тег-вне-словаря→hard-fail; + микро-агрегация (0.75/0.75).
  Adversarial-ревью (скептик-Agent): метрики-математика КОРРЕКТНА и false-pass-безопасна (вырожденный/
  читерский классификатор не пройдёт); поймал MAJOR — фикстура НЕ прогонялась через гейт end-to-end (нет
  аналога RAG-side `eval_fixture_meets_baseline`) → добавлен `fixture_runs_through_gate_and_discriminates`
  (эталон по зашитой фикстуре проходит, ленивый по ней же — падает) + точка подключения AI-2c; MINOR —
  zero-gold-кейс в фикстуру + документ предусловия уникальности путей. Бэкенд +7 тестов; cargo fmt/clippy/
  test зелёные. AI-2c подставит сюда реальный `chat_util`-классификатор и сверит его отчёт с порогами.
- **AI-2c (A4)** — **closed-vocabulary авто-тег** (спека §10 A4, ФИНАЛ AI-набора, mandatory-ревью §14.7).
  «Тесная интеграция AI», ради которой владелец и заказал канбан: по содержимому заметки `chat_util`
  (Qwen3-4B :8084) ПРЕДЛАГАЕТ теги ТОЛЬКО из существующего словаря vault. Спроектирован decision-complete
  через мультиагентный understand→design-Workflow (4 разведчика + синтез — ключевое решение write-back).
  **Бэкенд** `tagger/mod.rs`: `build_messages` (словарь — в system; тело — НЕДОВЕРЕННОЕ, между случайными
  `injection_marker()`, как `news/llm.rs`); `parse_and_filter` — ЧИСТЫЙ chokepoint closed-vocab: парсит
  JSON-объект `{"tags":[…]}` (терпит ```json```), нормализует (`trim`/снять `#`/lowercase — как
  `parser::push_tag`), оставляет ТОЛЬКО члены словаря (вне → `dropped`, `suggested_new` ВЫКЛ — owner-
  critical, гарантия на ВЫХОДЕ, модели НЕ доверяем); `classify_tags` (graceful-empty как
  `starting_questions`). Команда `suggest_tags(path)` (chat_util / topNvocab `list_tags` / `note_snippet`;
  НЕ пишет). Egress — переиспользует `EgressFeature::Chat` (без нового consent). **Write-back** — инлайн
  `#tag` в ТЕЛО (`lib/tag-apply.ts`): frontmatter-список невозможен (`set_frontmatter_field` режектит
  `[...]`; новый YAML-писатель вернул бы класс порчи, ради которого скаляр-примитив и сделан). `applyTags`
  флашит грязный буфер ДО записи (урок AI-1 R1), идемпотентен (уже-присутствующие теги не дублирует),
  атомарный `write_file` + анти-эхо `syncBufferAfterWrite` (SAFE-3). **UI** — «Предложить теги» в пейне
  редактора (по клику — LLM-вызов не на каждом открытии): чипы (отсев уже-в-теле) → «Применить». **Eval** —
  детерминир. мок-тесты (парс/closed-vocab-фильтр/инъекция-фенсинг) + live-гейт `live_classify_tags_meets_gate`
  (#[ignore], реальный chat_util по `tag_golden.json` → `evaluate_tags`/`meets_thresholds`). Мок зеркалит
  контракт (vocab-фильтр + dropped). Бэкенд +6 / фронт +7 тестов; cargo fmt/clippy/test + tsc/eslint/597
  зелёные; превью verified end-to-end (out-of-vocab отсеян, инлайн-запись + буфер-sync + тост).
  **Mandatory-ревью §14.7 (Workflow, 3 линзы: security/инъекция · data-safety/write-back · eval/closed-vocab
  → синтез):** security-линза ЧИСТА (фенсинг/output-chokepoint/char-safe-запись/egress подтверждены);
  вердикт FIX-FIRST на 1 MAJOR + 2 MINOR — **исправлено ПЕРЕД мержем:** MAJOR — гонка смены вкладки
  (`TagSuggest` без `key` → «Применить» после переключения заметки писал теги ЗАМЕТКИ-А в заметку-Б) →
  `key={active.path}` форсит ремоунт/сброс; MINOR — `existingInlineTags` regex ровняется на индексатор
  (нужна ≥1 буква, `#2024`≠тег); MINOR — мок-`suggestTags` зеркалит нормализацию+дедуп `parse_and_filter`;
  NIT — live-тест печатает реальный `dropped`. Frontmatter↔inline-дедуп тегов — в BACKLOG. **Завершает
  MVP канбана** (BOARD-1..6 · PROP-1..4 · AI-1/2a/2b/EVAL-AI/2c) — «тесная интеграция AI» закрыта.
- **Backlog cleanup (Batch A, frontend)** — добивание автономно-безопасных ✂️-хвостов adversarial-ревью
  (триаж — мультиагентный Workflow, 12 пунктов: 4 оказались уже сделаны → вычеркнуты, 5 фронтовых закрыты):
  (1) **trap-оверлеи vs Настройки** — `tweaksOpen: false` в `TRAP_OVERLAYS_CLOSED` → ни один focus-trap-
  оверлей не стэкается поверх Настроек (ревью MEM-4); (2) **GRAPH-4 поиск изолятов** — `hit` считается ДО
  скрытия сирот (`!showOrphans && n.ring && !hit`), `searchHits` включает изолят-совпадения (счётчик/Enter
  консистентны); (3) **kbd-aria палитры** — `aria-label="Escape"` + `↑↓`-глиф `aria-hidden`; (4) **AI-2c
  дедуп frontmatter** — `frontmatterTags()`/`existingTags()` (инлайн+блочный `tags:`-список), `appendInlineTags`
  и `TagSuggest` дедупят против тело∪frontmatter; (5) **AI-1 out-of-scope** — `inBoardScope()` (folder+project),
  `promoted.inScope=false` → честный тост `board.promote.outOfScope`. Фронт +3 теста (frontmatterTags / fm-дедуп
  / out-of-scope); tsc/eslint/600 зелёные; превью — приложение грузится, без console-ошибок.

### MEM-5 — захват факта в память прямо из чата (фидбэк владельца)

Отчёт владельца: чип «Запомнить» появлялся с задержкой (второй LLM-вызов ПОСЛЕ ответа), а явная команда
«сохрани в память X» уходила в LLM как обычная реплика — модель отвечала разговорно, факт не сохранялся;
в чате не было видимого пути к памяти (только панель в настройках). Решение владельца: **явная команда =
согласие** → сохранять сразу.

- **Распознавание явной команды** (`lib/memory-intent.ts`, `isExplicitSave`, без LLM, мгновенно): RU/EN-
  императивы «запомни / сохрани·добавь·занеси·запиши в память / remember that / save to memory / keep in
  mind». Уважает отрицание («не запоминай») и не путает «запоминай» (впредь) с «запомни» (это). ⚠️ JS-`\b`
  — ASCII-only (кириллицу не якорит) → границы слова строим вручную.
- **Явная команда → сохраняем СРАЗУ** (`source='explicit'`, без чипа): инлайн-индикатор «Сохраняю в
  память…» → тост «Запомнил: «…»» с «Отменить» (удаляет факт по id). Авто-детект без команды — прежний
  чип «Запомнить? ✓/✗» (D1: молчаливых записей нет).
- **Кнопка «🧠 В память»** в action-row под ответом (дискаверабельность): извлечь+сохранить факт из
  обмена (то же согласие). Дубль → честное «Это уже в памяти».
- **Фолбэк-срез команды** (`stripSaveCommand`): если LLM-извлечение пусто (редко), срезаем командный
  префикс, оставляя суть («запомни что дедлайн в пятницу» → «дедлайн в пятницу») — иначе команда была бы
  no-op. Поймал свой `\b`-ASCII-баг тестом (фолбакал весь префикс) → lookahead-границы.
- **Мок памяти** (`lib/mock/memory.ts`) для браузер-превью/vitest: in-memory факты (list/add/dedup/delete)
  — превью теперь правдиво показывает «Запомнил…», а не пустой no-op.

  **Adversarial-ревью диффа (2 рецензента, fix-first) поймал, в т.ч. КРИТИЧНОЕ data-loss:**
  - 🔴 **destructive-undo:** реальный `memory::add` на дубле возвращает СУЩЕСТВУЮЩИЙ id (а мок — null),
    значит «Отменить» на повторном сохранении удалил бы УЖЕ курированный факт юзера. Фикс: бэкенд
    `add → (id, inserted)`, команда `memory_add → {id, inserted}`; «Отменить» только при `inserted=true`,
    дубль → «Это уже в памяти» БЕЗ отмены. Мок выровнен по контракту.
  - 🔴 **сбой записи выдавался за «уже в памяти»** (`.catch(()=>null)`): теперь различаем saved/duplicate/
    **error**/nothing — статус-результат, error → честный «Не удалось сохранить».
  - 🔴 **гонки stale-toast:** epoch-гард в done и captureFromMessage (не вешаем тост старого обмена на
    очищенную/новую ленту); сброс `explicitSaving`/`capturingId` в clear/newSession/loadSession; стрим-гард
    в captureFromMessage.
  - 🟡 отрицание по стемам («не запомни/сохрани», `don't save`); пустая команда → «Нечего сохранять».
- Тесты: `memory-intent.test.ts` (детекция RU/EN, отрицание-перфектив, срез префикса) + chat-store
  (explicit+savedFact без чипа / дубль=duplicate без id / сбой=error / кнопка / undo) + бэкенд `add` inserted.
  Итого 529 фронт + 14 memory-Rust. Preview: «запомни …» → «Запомнил: «…»» + Отменить; ПОВТОР того же →
  «уже в памяти» БЕЗ Отменить (data-loss закрыт); кнопка «В память»; индикатор. i18n ru/en.

### Граф: когезия физики (эпик GRAPH, ресёрч конкурентов → синтез)

Мультиагентный ресёрч-Workflow (Obsidian / тюнинг d3-force / PKM-графы / ForceAtlas2-Louvain, 4 веб-источника)
→ decision-complete план: диагноз разлёта в терминах d3-force + 7 срезов GRAPH-1..7. Визуал (handoff-макет)
сохраняем — тюним физику/когезию/структуру.

- **GRAPH-1** — ретюн физики (фикс «граф размазан, изоляты разлетаются по углам»). Корень: центрирование
  было в ~7× слабее заряда (`forceX/Y` 0.012 vs d3-дефолт 0.1), степенной член заряда без капа (мега-хаб =
  «бомба»), `distanceMax=950` без отсечки, `distanceMin` не задан (старт-разлёт). Ретюн: gravity 0.012→**0.085**
  (глоб) / ×0.6 (лок), заряд `min(deg,8)*30`-кап, `distanceMax 340`/`distanceMin 14`, linkDist 62→46,
  link-iterations 2 (глоб), velocityDecay 0.45, кольцо изолятов 0.42→**0.30**·min(W,H) (плотный нимб у ядра,
  не у края). Физика вынесена в общие чистые хелперы `graph-sim.ts` (`chargeStrength`/`gravityStrength`/
  `clampNodePosition` + константы) — **устранён латентный баг дрейфа**: live-апдейт слайдеров раньше держал
  СТАРУЮ формулу (без капа/отсечки) и откатывал физику при движении ползунка. Ключ настроек `v2→v3` (старый
  персист разлёта не маскирует ретюн). Детерминированный snapshot-тест (d3-force на seed-графе хабы+спицы+
  изоляты) доказывает **сжатие связного ядра 0.8×** + no-NaN + клампы; preview-верификация когезии вживую.
  Adversarial-ревью диффа: без находок (3-сайтовый дрейф устранён, экстракция точна, v3-миграция чистая).
- **GRAPH-2** — граф открывается уже СОБРАННЫМ + остывает до полной остановки. Warmup: предрасчёт укладки
  headless (`WARMUP_TICKS`, без рендеров; кап тиков по размеру графа — на больших vault не блокируем UI) до
  первого кадра → нет старт-«прыжка». Cool-to-stop: `alphaTarget 0.02→0` (создание/drag-end/слайдер) — сим
  замирает, как в Obsidian, без вечного «дыхания»/CPU-churn; косметика активной ноты (halo/ripple/flow) —
  CSS-анимации, идут независимо от заморозки. Live-эффект настроек реогревает ТОЛЬКО при реальной смене
  слайдера (не на смене графа — иначе сбил бы warmup). Adversarial-ревью поймал MAJOR (рассинхрон
  `prevSettingsRef` при сдвиге слайдера без сим → ложный реогрев рушил warmup) — закрыт безусловной
  синхронизацией ref; + MINOR кап warmup-тиков по числу узлов. Preview-верификация: раскладка собрана к
  450мс, сим `frozen=true` после остывания.
- **GRAPH-3** — тоггл «показывать изоляты» (как «Show orphans» в Obsidian; zoom-to-fit уже был). Выкл →
  сироты (deg=0) не рисуются И не входят в кадр → камера стягивается по связному ядру (preview: 11→6 узлов,
  viewBox 956×829→**375×325**, ×2.5 плотнее). Прямо снимает «изоляты разлетаются»: кто не хочет нимб —
  прячет одним тумблером. Реогрев физики разнесён от перекадрирования: смена ФИЗИКИ (repel/linkDist/
  gravity/sizeScale/group) → reheat; тоггл отображения (showOrphans) → только re-fit без re-settle (ревью).
  Доп. поле `showOrphans` аддитивно (дефолт ВКЛ, ключ v3 не бампаем). i18n ru/en.
- **GRAPH-4** — поиск-в-графе: компактное поле в баре (иконка + clear-✕) подсвечивает узлы по
  вхождению запроса в заголовок (`title.includes`, регистронезависимо) и гасит прочие, как hover-dim.
  Совпадение — акцентный обвод узла + всегда видимый лейбл (на любом зуме). **Поиск доминирует** над
  фильтром тегов/hover-подсветкой/рёбрами: при активном запросе совпадения горят, всё остальное гаснет,
  а ребро видно только между двумя совпадениями. Adversarial-ревью поймал противоречивый полу-гашёный
  вид (узел был бы одновременно `.hit` + `.faded` при hover/тег-фейде поверх поиска) — снято правилом
  «hit никогда не faded; при активном поиске non-hit всегда faded». Render-тест: запрос → 1 `.hit` +
  гашение прочих, очистка сбрасывает; preview-верификация (запрос «meeting» → 1 узел горит, 5 гаснут,
  0 конфликтов hit+faded, рёбра к не-совпадениям тускнеют). i18n ru/en. Граничный кейс (поиск не находит
  изоляты при выкл. «Показывать изоляты») — в BACKLOG.
  Поиск → **quick-switcher**: счётчик совпадений в поле, **Enter** открывает верхнее совпадение (порядок
  отрисовки: full = самый связный сверху), **Esc** чистит запрос. `searchHits` уважает скрытые изоляты —
  Enter откроет ровно то, что подсвечено. Esc делает `stopPropagation` (на случай будущего глобального
  Esc-закрытия графа — чистка приоритетнее). Тест: счётчик «1», Enter→`openFile('A.md')`, Esc→пустой запрос;
  preview: Enter открыл «Meeting» и закрыл граф, Esc очистил и оставил граф открытым.
- **GRAPH-6** — детекция СООБЩЕСТВ (Louvain) + раскраска/группировка по кластерам (сигнатурная фича
  читаемости больших графов, как ForceAtlas2+Louvain в Gephi). Сегмент «Цвет: Теги | Сообщества» в баре
  (дискаверабельно, не закопан в ⚙️). Режим «Сообщества» красит узлы по id авто-кластера (`clusterColor`,
  hue = золотой угол 137.508° — макс. разнос соседних кластеров; та же oklch-семья и CSS-переменные, что
  у тег-цвета → ноль нового CSS, пер-тема). Группировка (`group`) при этом стягивает узлы к центроиду
  кластера, а не тега — лейбл тоггла зависит от режима («Группировка по сообществам»/«…по тегам», иначе
  врал бы). **Своя детерминированная реализация Louvain** (`louvain.ts`, ~250 строк, БЕЗ npm-зависимостей
  — graphology был бы слепой зоной supply-chain-CI; и БЕЗ генератора случайных чисел): фикс-порядок обхода
  по сортированным id, целочисленные веса, порядок-канонная агрегация супер-графа, канонизация меток по
  размеру (0 = крупнейшее, ничья — лексикограф. min-id) → цвета стабильны между перезагрузками. Модулярность
  на плоском графе (текстбук Q=Σ[L_c/m−(deg_c/2m)²], пин: две треугольника ≈0.357). Дизайн прогнан
  мультиагентным Workflow (3 разведчика → синтез → 2 adversarial-рецензента, оба needs-changes):
  все фиксы вложены — деление-на-0/NaN на пустом/без-рёбер графе (early-return + кап уровней),
  порядок-канонная агрегация, localeCompare для строковых id, comm через `commRef` (d3-эффект на `[graph]`,
  смена comm не реогревает warmup), reheat colorBy ТОЛЬКО при включённой группировке, `clusterColor(id<0)→null`
  (не путать «нет сообщества» с кластером 0), мод-аware лейбл группировки. Реализация поймала свой баг
  (`Map[key]` вместо `.get()` схлопывал всё в 1 кластер) тестом ДО ревью. Тесты: `louvain.test.ts` (7 —
  2×K5→2 кластера, детерминизм при перетасовке входа на ≥2-уровневом графе, канонизация, модулярность,
  вырожденные, строковые id) + `clusterColor` в graph-sim. Аддитивно: `colorBy:'tag'` дефолт, ключ
  настроек v3 НЕ бампаем. Preview: 6 узлов → 3 кластера (hue 0/138/275), тег↔сообщества переключение без
  скачка раскладки, тег-чип-фейд композится поверх кластер-цвета, console чисто. i18n ru/en.
  **Adversarial-ревью ДИФФА (после реализации, отдельный Workflow) поймал MAJOR**, пропущенный тестами:
  построение базового уровня СУММИРОВАЛО параллельные/реципрокные рёбра в вес (A↔B = вес 2), а
  модулярность считалась на дедуп-графе (unit-вес) → рассинхрон + смещение разбиения (взаимные вики-ссылки
  — типичная топология vault, бэкенд шлёт A→B и B→A двумя рёбрами). Фикс: базовый уровень тоже unit-вес
  (`pair.set(key,1)`), регресс-тест на реципрокные/параллельные рёбра (совпадает с графом без дублей).
  Доп. из ревью: `commRef` синк через `useLayoutEffect` (не useEffect) — иначе при персисте cluster+group
  первый warmup группировал бы по устаревшему comm; +интеграционный тест рендера кластер-цвета. Итог 515 тестов.

<!-- Эпик «второй мозг на каждый день»: продуктовый аудит (docs/reviews/PRODUCT_AUDIT_2026-06.md,
     99 находок) → decision-complete docs/IMPROVEMENT_PLAN.md (9 фаз) → автономное исполнение.
     Ниже — сводка по фазам (#181–228). Приёмка качества: adversarial-Workflow-ревью диффа ПЕРЕД
     мержем. Срезы — squash-PR от origin/main, мерж мимо Windows-флейка 0xc0000139 (ubuntu-Rust зелёный). -->

### Vision-фича: персистентная память агента (MEM, `docs/specs/agent-memory.md`)

Слой ЯВНЫХ ФАКТОВ о пользователе/проектах, отдельный от RAG-по-переписке (N4b). Решения D1–D6
зафиксированы владельцем (захват: явная команда + подтверждённое авто; инъекция: пины «всегда» + top-k
релевантных; хранение: таблица + параллельный usearch-индекс; UI: панель «Память ИИ»; флаг ВЫКЛ по
умолчанию; мягкий кап + ручная чистка). Срезы MEM-1..4.

- **MEM-1** — бэкенд-фундамент: миграция `017_memory_facts` (UNIQUE-дедуп по тексту) · модуль `memory/`
  (add/list/set_pinned/edit/delete/clear/count · `index_fact`/`unindex_fact` · `context_facts` = все пины +
  top-k не-пинов по близости с обновлением `used_at`) · per-vault индекс `memory_vectors.usearch` · 5 tauri-команд
  · 6 unit-тестов. Adversarial-ревью: пойман и закрыт рассинхрон индекса при пустом edit (команда не
  ре-эмбеддит пустой текст; `memory::edit` — no-op).
- **MEM-2** — инъекция фактов в чат (AC-MEM-5): `build_agent_memory_block` оборачивает факты (пины +
  top-k близких) в анти-инъекционные маркеры и префиксует к user-сообщению ЛЮБОГО режима — отдельный
  КАНАЛ (`memory_vectors`), не трогает note-RAG ранжирование (eval-гейт держится), как N4b. За флагом
  `aiAgentMemory` (ВЫКЛ по умолчанию, D5); пустая память → блок не добавляется. Плюс фронт-плумбинг
  (pref + проброс через `chat.ts`/`tauri-api`). Тумблер в Настройках и панель — MEM-4. Adversarial-ревью
  чистый (анти-инъекция/плумбинг/изоляция канала/порядок блоков — без находок).
- **MEM-3** — захват фактов (D1, AC-MEM-6): два пути, ноль молчаливых записей. (1) Подтверждённое **авто**:
  после обмена «быстрая» модель (`memory::extract::propose_fact`, анти-инъекция-маркеры, best-effort)
  предлагает ≤1 факт-кандидат → чип «Добавить в память: «…» ✓/✗» под последним ответом; ✓ → `memory_add`
  (`source='auto'`), ✗ → отбрасывается. Зовётся ТОЛЬКО при `aiAgentMemory`=on (нет лишних LLM-вызовов).
  Стейл-гард: чип привязан к id ответа, снимается на новом обмене/смене сессии. (2) **Явная команда**
  «Сохранить выделение в память ИИ» (палитра) → выделение редактора как `source='explicit'`. `memory_add`
  получил параметр `source` (валидация explicit|auto, прочее→explicit). i18n ru/en. Тесты: 8 Rust
  (extract) + 5 стора (propose/confirm/dismiss/стейл). Adversarial-ревью без находок.
- **MEM-4** — панель «Память ИИ» + тоггл (AC-MEM-7/8), **фича включена end-to-end**. Модалка
  `MemoryPanel` (focus-trap, «как Goals/Digest»): список фактов (пины сверху), пин/анпин, правка-на-месте,
  удаление (с подтверждением), ручное добавление; при переполнении мягкого капа `MEM_CAP=200` (D6)
  старые не-пины подсвечены «давно не использовался» (`staleFactIds` — LRU сверх капа, пины не считаются);
  пустое/загрузочное состояния. Тоггл `aiAgentMemory` в Настройки→AI (ВЫКЛ по умолчанию, D5) + кнопка
  «Память ИИ…»; команда палитры «Память ИИ». `memory`-неймспейс tauri-api (list/setPinned/edit/delete)
  + тип `MemoryFact`; стор `useMemoryStore` (reload-после-мутации с монотонным токеном против
  out-of-order); `memoryOpen` в `TRAP_OVERLAYS_CLOSED` (взаимоисключение focus-trap-оверлеев). i18n ru/en.
  Тесты: 6 стора + 6 рендер-панели. Мультиагентный adversarial-ревью диффа (4 измерения × adversarial-verify)
  → **4 находки, все исправлены**: (1 major) Esc во время правки-на-месте закрывал всю панель — React-onKeyDown
  `stopPropagation` НЕ помогает (нативный Esc-листенер focus-trap всплывает раньше; эмпирически подтверждено
  тестом) → фикс нативным листенером на самом input; (минорные) `memoryOpen` в reading-Esc-гейт, гонка
  reload→токен, `toggleMemory` гасит настройки. Пре-существующий класс «trap-оверлеи vs настройки» — в BACKLOG.

### Дотошный код-аудит (2026-06-15, `docs/reviews/CODE_AUDIT_2026-06.md`)

Мультиагентный аудит (132 агента, 88 уникальных находок) → фикс-батчи поверх #231–238. Каждый
батч — verified-spec → имплементация → полный CI-гейт → adversarial-ревью диффа ПЕРЕД мержем.

- **CRITICAL/MAJOR (#231–238, ранее):** SSRF плагин-egress + DNS-rebinding · path-traversal `..`/`.nexus` · атомарность конфигов/экспортов · перенос `.nexus/history` при rename · потери данных Home-захвата · zip-усечение векторов · graph_rank unbounded-IN · toggleTask нумерованные · watchdog-requeue.
- **B1 (#240)** — целостность: git pull dirty-guard (`ensure_clean_tree`) + history TOCTOU(O_EXCL)/traversal-валидация.
- **B2 (#241)** — БД-устойчивость: потолок версии миграций + `catch_unwind` writer/read-pool.
- **B3 (#242)** — contradictions: стабильный `hash_snippet` (blake3) + честная выдача list/relation-reasons (фильтры `is_deleted`, пустой-кэш).
- **B4 (#243)** — graph: self-loop исключён из беклинков, рёбер и степени.
- **B5 (#245)** — indexer/scheduler: wikilink-границы строки · честный скан (счётчик failed) · re-arm recurring dead-job.
- **B6 (#244)** — news/digest: lossy-декод не-UTF8 фидов + injection-маркеры контента в дайджесте.
- **B7 (#246)** — frontend-honesty: `data:image/`-only (анти-XSS) · ASCII-теги превью (как бэкенд) · знаковый `relTime`.
- **B8 (#248)** — сторы: персист recents при reset · ремап navHistory при moveTab · сброс suggest-dismissed при смене vault.
- **B9 (#247)** — starred переживает rename/delete заметок и каталогов.
- **B10 (#249)** — a11y: focus-trap модалок Digest/Contradictions/Settings/Conflict · Esc-приоритет reading-mode · roving-radiogroup · кламп FileTree-active.
- **B11 (#250)** — perf: отложенный парсинг оглавления (`useDeferredValue`) · кап кэша вопросов · очистка drag-слушателей (AbortController).
- **B12 (#251)** — чат epoch-гарды: onEvent (поздние токены после stop) · loadSession-recheck · disclosure LRU-кап вместо clear.
- **B13 (#252)** — фронт-гонки/honesty: epoch-гард news.load · видимость Home error/loading · честный сбой commit в SyncPanel.
- **B14 (#253)** — news: HN не фильтруется повторно `keyword_filter` (Algolia уже отфильтровал; чинит потерю совпавших по `story_text`).
- **Отложено в BACKLOG** (с обоснованием): orphan-history-GC · reconcile-orphans (риск usearch v2 API) · should_generate/candidate_pairs (нужна схема) · NewsView-offline-banner (нужен egress-event) · web-save/saveRemote honesty-хвосты. **Отклонено** (перф-регрессия): mtime-shortcut. **Отсеяно** (false-positive): backlinks-occurrences, atom-date-concat, hotkey-ghost, graphview-rerender (уже #148), trap-overlays-stack.

### Фаза 1 — Сохранность данных (P1, «нетеряемость мысли»)

- **SAFE-1 (#181)** — атомарная запись заметки (tmp в той же папке → fsync → rename), без частичных файлов.
- **SAFE-2 (#182)** — хеш-трекинг контента (blake3), `baseHash` буфера для детекта внешних правок.
- **SAFE-3 (#183)** — guard внешнего изменения: watcher эмитит `vault:file-changed`, баннер «Оставить мои / Загрузить с диска / Сравнить».
- **SAFE-4 (#184)** — debounced-автосейв + flush (blur / закрытие вкладки и окна) + `flushAllDirty`; честный статус «⚠ Не сохранено».
- **SAFE-5 (#185)** — история версий (бэкенд): снапшоты `.nexus/history`, дедуп по контенту, троттл/ретенция/GC.
- **SAFE-6 (#186)** — история версий (UI): список снапшотов + line-diff + восстановление + «Сравнить» в guard-баннере.

### Фаза 2 — Честная ось времени (P2)

- **EVT-1 (#188)** — таблица `edit_events` (миграция 015) на событиях индексатора; ts = когда Nexus УВИДЕЛ правку, не mtime.
- **ACT-1 (#198)** — активность Home (heatmap/тренд) переведена на `edit_events` с mtime-фолбэком для bootstrap — первый потребитель P2.

### Фаза 3 — Курация vault (P3)

- **CURATE-1 (#189)** — удаление заметки/папки в корзину `.nexus/.trash/` (atomic rename, vault-local).
- **CURATE-2 (#190)** — rename/move заметок и папок из UI (сохраняет file_id/беклинки, анти-overwrite).

### Фаза 4 — Захват / якорь (P4)

- **CAP-1 (#192)** — заметка дня ⌘⇧D (`Journal/YYYY-MM-DD.md`) + фокус курсора в редактор при открытии.
- **CAP-2 (#193)** — quick-capture ⌘⇧N → дозапись `- HH:MM …` в `Inbox.md` без открытия файла.
- **CAP-3 (#197)** — шаблоны `Templates/` + ⌘⇧T (плейсхолдеры `{{date}}/{{time}}/{{title}}`).
- **TOAST-1 (#194)** — глобальная toast-система (FIFO, auto-dismiss, reduced-motion) + подтверждение захвата.

### Фаза 5 — Навигация / всплывание знаний (P5)

- **NAV-1 (#191)** — контент-поиск в палитре (гибрид по телу + сниппеты + CSP-safe подсветка).
- **NAV-2 (#195)** — недавние заметки (MRU-кольцо) + ⌘O quick-switcher.
- **NAV-3 (#196)** — история навигации back/forward ⌘[ / ⌘] (с маршрутизацией в родную группу).
- **NAV-4 (#199)** — восстановление позиции курсора при возврате к заметке.

### Фаза 6 — AI-напарник (P6)

- **P6-AR (#200)** — действия под ответом ИИ: «Копировать» + «Вставить в заметку» (dispatch у курсора).
- **P6-PIN (#201)** — закрепить заметку в контекст чата (полное содержимое; гард `is_pinnable` .md-only; анти-инъекция).
- **P6-RGN (#202)** — регенерация последнего ответа (атомарно режет хвостовую пару из ленты, истории и векторов).
- **AIP-2 (#216)** — кликабельные цитаты `[n]` в ответах (открывают источник-заметку или web-URL).
- **AIP-3/6 (#217)** — мост «Разобрать с ИИ» на Home-инсайтах (открыть чат с правимым prefill + пин источника).
- **AIP-5 (#218)** — проактивные `open_questions` (генерация при открытии vault) + честный индикатор «генерирую…».
- **AIP-10 (#219)** — LLM-«причина связи» в «Связях»/«Похожих» (миграция 016 `relation_reasons`, ленивый кэш).

### Фаза 7 — Редактор / письмо (P7)

- **EDIT-1 (#203)** — формат-хоткеи жирный ⌘B / курсив ⌘⇧I.
- **EDIT-2 (#204)** — чекбоксы/таски по ⌘L (построчный тоггл `- [ ]`↔`- [x]`).
- **EDIT-3 (#205)** — умное продолжение списков/тасков/цитат по Enter (штатный `markdownKeymap`).
- **EDIT-4 (#206)** — вставка markdown-ссылки по ⌘K (умная по выделению/URL).
- **EDIT-5 (#207)** — кликабельные чекбоксы тасков в превью (флип исходной строки + автосейв).
- **EDIT-6 (#208)** — slash-команды «/» — попап быстрых вставок блоков (второй CompletionSource).
- **IMG-1 (#213)** — вставка/перетаскивание картинок (data-URL, CSP не трогаем; запись в `attachments/`).
- **KaTeX (#215)** — мат-формулы `$$…$$` в превью через MathML (нативный `<math>`, строгий CSP не трогаем).

### Фаза 8 — Эпики (P8: задачи, поток)

- **TASK-1 (#209)** — дашборд задач ⌘⇧K (скан всех `- [ ]` vault на лету, клик-тоггл + навигация).
- **TASK-2 (#210)** — дедлайны (`📅`/`@due()`) и приоритеты (`⏫🔼🔽`/`!pN`) + бакеты Просрочено/Сегодня/Неделя/Позже.
- **INBOX-1 (#211)** — GTD-разбор входящих: строки quick-capture → «В задачу / В заметку / Удалить».
- **FLOW (#220)** — связанные заметки vault в ридере новостей (RAG-мост лента→база).

### Фаза 9 — Полировка (P9)

- **P9 focus-trap (#212)** — Tab-цикл + Esc-close для модальных оверлеев (a11y); взаимоисключение оверлеев.
- **POLISH (#221)** — шпаргалка горячих клавиш ⌘/ (читает реестр команд, группирует по разделам).

### Owner-gated поток (по решению владельца)

- **RECUR-1 (#214)** — повторяющиеся задачи 🔁 (`daily/weekly/monthly/yearly`/`every N`; клемп дня/переполнения).
- (картинки IMG-1 #213 и KaTeX #215 — см. P7; git-sync оказался уже построен — строить было нечего.)

### AIP-поток «ИИ как ежедневный напарник» + роадмап-срезы (после исчерпания плана)

- **AIP-хвост (#222)** — проактивный stale-radar (забытые заметки всплывают) + мост «Разобрать с ИИ».
- **тег-фильтр (#223)** — точный фильтр по тегу (`notes_by_tag`) вместо шумного substring-поиска.
- **контент-поиск в сайдбаре (#224)** — режим «Везде» (поиск по телу со сниппетами), хвост NAV-1.
- **AIP-11 (#225)** — саджесты связанных заметок в контекст чата из открытого файла (мост редактор→чат).
- **EDIT-7 (#226)** — оглавление заметки: заголовки списком + переход к секции (source и preview).
- **AIP-SQ (#227)** — контекстные стартовые вопросы в пустом чате (LLM по активной заметке; деградация на статику).
- **UNLINK-1 (#228)** — незалинкованные упоминания: заметки, что упоминают имя/заголовок без `[[ссылки]]` (FTS5).

### News-ридер: кнопка «Обсуждение на HN» (хвост фидбэка владельца)

Завершает разбор отчёта по ридеру: для HN-айтема `url` = отправленная ссылка (у Show HN это
github-репо, корректный «Оригинал»), но ссылка на сам HN-тред терялась («копируется github, а не
сама новость»). Теперь HN-обсуждение хранится отдельно и показывается в ридере кнопкой
«Обсуждение на HN» рядом с «Оригинал» (открывается через тот же opener).

- Миграция 014: nullable `news_items.comments_url`. `parse_hn` проставляет
  `https://news.ycombinator.com/item?id={objectID}`, ТОЛЬКО когда `url` — внешний (у текстовых
  HN-постов `url` уже == обсуждение → `None`, без дубль-кнопки). Проброшено через
  `NewsEntry`→`NewRow`→`news_items`→DTO `NewsItem.commentsUrl`.
- Тесты: `parse_hn` (внешний url → тред / текстовый пост → None) + ридер показывает кнопку с
  href на `news.ycombinator.com`, «Оригинал» по-прежнему на github.

### Багфикс News-ридер: чужой хром в тексте + мёртвая кнопка «Оригинал» (отчёт владельца)

Отчёт по NF-6 ридеру на HN-новости (Show HN, url = github-репо). Два бага:

- **GitHub-хром протекал в текст статьи.** `extract_paragraphs` хватал ВСЕ `<p>` страницы, а у
  логаут-страницы репозитория это async-острова GitHub («There was an error while loading…»,
  повторялись), фидбэк-виджет, помощь по поиску — переводились локальной моделью и сыпались в
  ридер. Фикс: сносим `<script>/<style>` (там же лежит JSON-дубль README), сужаем до контейнера
  контента (`<article class="markdown-body">`/`<article>`/`<main>`; нет — весь документ, как
  раньше для блогов), блок-лист стабильных UI-фраз + дедуп повторов. Блок-лист сверяет ПРЕФИКС
  (strip_tags даёт «…page .» с пробелом перед точкой из-за вложенного `<a>` — adversarial-ревью
  поймало, что полная фраза не сматчилась бы).
- **Кнопка «Оригинал» не открывала браузер.** В Tauri-вебвью `<a target="_blank">` не уходит в
  системный браузер (строгий CSP глотает навигацию) — opener-плагина не было. Подключён
  `tauri-plugin-opener`; host-side команда `open_external` (схема-гард: только http/https,
  отсекает file:/кастомные схемы) — capability НЕ нужна. Хелпер `tauriApi.external.open` заменил
  мёртвый `target=_blank` в ТРЁХ местах: «Оригинал» ридера, web-источники чата, внешние ссылки в
  превью заметок. Открытие — НЕ эгресс приложения (фетчит ОС-браузер), `GuardedClient` не тронут.

- **Облако тем застилало экран при избытке** (у владельца 47 чипов). Свёрнуто по умолчанию: первые
  14 тем + «Ещё N», клик раскрывает/сворачивает (активный фильтр всегда виден). Тест на 20 темах.

Self-test на РЕАЛЬНОЙ странице владельца (`github.com/responsiblparty/cc-doubleteam`, фетч 200):
старый код хватал «There was an error while loading» ×3 + фидбэк-виджет; новый извлёк 6 чистых
README-абзацев («Three-phase project mode for Claude Code…», «Restart Claude Code…») и НИ ОДНОГО
куска хрома.

Примечание: для Show HN `url` = github-репо — это и есть «оригинал» (корректно). Отдельная
кнопка «Обсуждение на HN» — следующим PR (нужен проброс objectID через пайплайн).

### Перф (#19 cold-bench): индексация была O(N²) → почти линейна; + N4 live-проверка

Холодный бенч локального пайплайна на синтетическом vault (мок-эмбеддинг — изолирует локаль от
сети, `bench_local_pipeline_scale`, запуск через `NEXUS_BENCH_FILES`) вскрыл **суперлинейную
индексацию**: на nested+basename раскладе (как реальный Obsidian) 10k файлов индексировались
**124.7 с** (80 файлов/с против 613 на 1k — throughput коллапсировал, O(N²)).

Два места резолва ссылок без индекса:
- на каждый файл `UPDATE links … WHERE target_id IS NULL AND target_raw=?` — полный скан таблицы
  `links` (растёт ~3×N);
- `resolve_target` (3×/файл) для basename-шортката `[[Note]]` искал `path LIKE '%/' || ?` —
  **ведущий wildcard не индексируется** → полный скан `files` на каждую ссылку.

Фикс (миграция 013, без новых колонок): частичный индекс `idx_links_dangling` под dangling-UPDATE +
**индекс по выражению** `idx_files_basename` (последний сегмент пути); `resolve_target` и
`resolve_all_dangling` разбиты на индексируемые шаги (точный путь → basename-выражение → редкий
LIKE-суффикс только для `[[dir/Note]]` с `/`). Семантика резолва сохранена (юниты + новый
`basename_shortcut_resolves_into_subfolder`).

**До → после** (nested+basename, индексация полного скана):

| Файлов | До | После | Ускорение |
|---|---|---|---|
| 1 000 | 1.6 с (613 ф/с) | 0.7 с (1446 ф/с) | 2.3× |
| 10 000 | 124.7 с (80 ф/с) | 8.7 с (1148 ф/с) | **14×** |
| 50 000 | (O(N²), десятки минут) | 57.8 с (864 ф/с) | ~50× |

Поиск/граф/диск масштабируются линейно (50k: поиск p50 112 мс, db 73 МБ + usearch 202 МБ).

Также: `live_chat_memory_recall_end_to_end` — ЖИВАЯ сквозная проверка N4 (RAG по чат-сессиям): в
прошлой сессии зафиксирован факт, в новой сессии перефразированный вопрос → реальный bge-m3 достаёт
ту сессию, врезка памяти в промпт, живая gemma вспоминает факт; контроль без памяти не знает.
Прогон зелёный (40 с на 192.168.0.31).

### N1: ручной перезапуск планировщика из UI (развитие вотчдога #170)

- Кнопка «Перезапустить» в модалке «Фоновые задачи»: если воркер всё же завис, владелец
  поднимает его заново БЕЗ перезапуска приложения. Бэкенд `restart_scheduler` рвёт старый
  супервизор + дропает его shutdown-канал, поднимает свежий воркер тем же конфигом
  (`WorkerSpawner`); новый цикл на старте делает crash-recovery (running→pending) и тут же
  клеймит готовые джобы. Без переоткрытия vault.

### N4: RAG по чат-сессиям — переписка как «второй мозг» в выдаче LLM

Решение владельца 2026-06-12: переписка — часть «второго мозга», LLM должна иметь к ней доступ.
**Отдельный неймспейс**, чтобы чаты не глушили заметки в выдаче (eval-гейт держится).

Инфраструктура:
- Отдельный usearch-индекс `.nexus/chat_vectors.usearch` (тот же эмбеддер/dim, что заметки, но свои
  ключи = id сообщений — не пересекается с чанками заметок). Открывается в `build_rag`, поле
  `VaultContext::chat_vectors`.
- Каждый завершённый обмен индексируется в `chat_vectors` (оба сообщения, фоном). Бэкфилл на старте
  vault: сессии до N4 (или потерянные векторы) до-эмбеддятся (usearch — источник правды через
  `contains`).
- `chat_log::search_memory` — эмбеддит запрос, ищет в `chat_vectors`, резолвит в `MemoryHit`
  (сессия+сниппет), исключает текущую сессию, дедуплицирует по сессии.

Врезка в чат:
- `chat_rag` подмешивает топ-3 фрагмента прошлых диалогов как ФОН к user-сообщению любого режима
  (vault/общий/web) — отдельный канал, note-RAG ранжирование (`hybrid_search`) не тронуто, поэтому
  заметочный golden не падает (offline-гейт зелёный). Анти-инъекция: фрагменты обёрнуты случайным
  маркером (как RAG/web). Текущая сессия исключается по `sessionId`.
- Флаг `aiChatMemory` (ВКЛ по умолчанию) в настройках AI; событие `memorySources` → плашка «Из
  прошлых разговоров» в чате (по клику открывает ту сессию). Снапшот источников хранит и память.

### N2: live_real_vault_smoke стал vault-агностичным (self-retrieval)

- Прежние 4 хардкод-пробы («рецепт хлеба», ANN-поиск…) были написаны под старый личный vault и
  ложно падали на рабочем (SA-Vault: рабочие заметки). Теперь тест берёт выборку заметок самого
  vault, запрос — начало содержимого, и проверяет, что заметка находит САМУ СЕБЯ в топ-8 (порог
  80%). Работает на любом vault. Прогон на SA-Vault: 12/12 self-rank=0 (каждая заметка первая).

### N3: стоп-слова в лексической ветке поиска (eval-гейт пройден — и улучшил golden)

- Живой smoke на рабочем vault показал: служебные «на/без/the» лексически цепляли неродственные
  заметки (0.015–0.03) и через RRF теснили семантику. Добавлен консервативный RU/EN стоп-лист
  (`search::STOPWORDS`), чистящий ТОЛЬКО лексическую (FTS) ветку запроса; вектор не трогаем
  (эмбеддинг сам разбирается со стоп-словами); запрос целиком из стоп-слов → fallback на токены
  (не теряем лексику). Eval на golden (живой bge): **recall@8 1.000, nDCG@8 .883 → 1.000,
  MRR .848 → 1.000** — удаление низко-IDF шума не просто прошло гейт, а заострило ранжирование.

- Фикс: крестик закрытия вкладки выдавливался за рамку пилюли на длинных заголовках
  (имя вкладки стало сжимаемым flex-элементом с многоточием).

### Надёжность: вотчдог планировщика (инцидент 2026-06-12 — воркер «тихо умер»)

- Диагноз на живом процессе владельца: ready-джобы стояли в очереди 13 часов (attempts=0),
  все потоки tokio запаркованы, паник нет — воркер-задача исчезла без следа; «Обновить» в ленте
  дедупился об застрявшую джобу, «Собираю…» крутился вечно, чип статусбара застыл.
- **Супервизор**: worker_loop под наблюдением — неожиданное завершение (паника/return) →
  ERROR с причиной в журнал и рестарт через 5 с; штатный shutdown останавливает как раньше.
- **Вотчдог тика**: тело тика под timeout 15 мин — зависший await обрывается с ERROR,
  цикл продолжает жить (LLM-джобы исполняются внутри тика, потолок щедрый).
- **Телеметрия**: «scheduler worker started» + heartbeat в журнал раз в 10 мин — по файлу
  видно, жив ли планировщик; следующий инцидент объяснит сам себя.
- **Честное «Собираю…»**: фронт ленты поллит очередь — джоба pending дольше минуты без
  запуска или running дольше 20 мин → спиннер снимается с понятной ошибкой («планировщик
  не отвечает, перезапустите приложение, журнал там-то»).
- **Статусбар**: страховочный поллинг счётчиков раз в минуту — чип не застывает при мёртвом
  событийном канале.


### Чат-сессии: переписка — часть «второго мозга» (решения владельца, 2026-06-12)

- **Миграция 012**: `chat_sessions` + `chat_messages` в vault-БД — каждый завершённый обмен
  (вопрос+ответ+JSON-снапшот источников) пишется в текущую сессию; localStorage-история v1
  заменена. Ничего не удаляем: «Новая сессия» (вместо корзины) лишь начинает чистый лист,
  старые сессии остаются памятью.
- **Заголовок сессии** — суммарайз первого вопроса мелкой моделью (асинхронно, плейсхолдер —
  обрезанный вопрос).
- **История** (вариант А, Claude-style): кнопка-часы в шапке AI-панели → glass-дропдаун с
  группами «Сегодня/Вчера/На этой неделе/Ранее», активная сессия подсвечена, клик — загрузка
  ленты с восстановлением карточек источников.
- **«Сохранить в заметки»** (на ховере строки истории): сессия экспортируется в
  `Chats/<дата> <заголовок>.md` — индексируется обычным пайплайном и становится полноценной
  заметкой мозга. Авто-экспорта нет сознательно («куча диалогов недостойны отдельных заметок»).
- При открытии vault продолжается последняя сессия (преемственность прежнего поведения).
- 2-я очередь (BACKLOG): RAG-доступ к сессиям без экспорта (отдельный неймспейс).


### RAG: LLM-реранжирование источников (eval-гейт пройден; карт-бланш, ночь 2026-06-11)

- `search::rerank`: мелкая модель (`ai.fast`, no-think) переупорядочивает топ-24 кандидатов
  гибрида по релевантности вопросу, дальше в контекст идут лучшие k. **Eval на golden
  (живой bge+E4B): recall@8 1.000 (без изменений), nDCG@8 0.883 → 1.000, MRR 0.848 → 1.000** —
  идеальный порядок на всех 10 кейсах (эксперимент `live_eval_llm_rerank_experiment` оставлен
  в сьюте). Цена ~1–3 с на вопрос.
- Надёжность: ошибки вызова/мусор в ответе модели → исходный порядок гибрида (чат не ломается);
  номера вне диапазона/дубли отбрасываются, неупомянутые кандидаты добираются хвостом (реранк
  не может потерять источник). Сниппеты в промпте — данные (анти-инъекционная инструкция);
  модель возвращает только числа.
- Тумблер «Реранжирование источников (LLM)» в Настройках → AI (default ВКЛ при наличии
  утилитарной модели; без неё ретрив как раньше).


### Перф: cross-file батчинг эмбеддинга на полном скане (карт-бланш, ночь 2026-06-11)

- Скан шёл пофайлово: на реальных vault файл часто даёт 1–3 чанка → почти каждый чанк улетал
  отдельным HTTP-вызовом (бенч 500×1-чанк: 23 эмб/с). Теперь скан идёт группами по 256 файлов:
  префилл эмбеддит все чанки группы ПОЛНЫМИ батчами по 64 поперёк файлов (`Rag::scan_cache`),
  `index_file` берёт векторы из кэша (промах — добор по-старому; mtime-шорткат уважается, лишнего
  не эмбеддим). Замер на живом bge/.31: **21.7 с → 16.6 с (+30% throughput)**; остаток упирается
  в сам инференс CMP-карты.
- Хаускипинг BACKLOG: батчинг и «прогресс в UI» (#146) отмечены сделанными; web/tool-use-строка
  актуализирована (web-канал закрыт W-1..4); пометка про подсветку сниппетов (контент-сниппеты
  в UI поиска пока не выводятся).


### Хвосты волны 3 (2026-06-11, поздний вечер)

- **Источники в чате сворачивались при скролле**: react-virtual размонтирует сообщения, ушедшие
  из вьюпорта, и локальное состояние аккордеона сбрасывалось. Раскрытость теперь живёт в реестре
  при сторе (переживает ремаунт, чистится вместе с историей чата).
- **Этапы прогона ленты видны при «Обновить»**: раньше строка «Опрашиваю источники · i/N» жила
  только в первом прогоне без истории; кнопка «Собираю…» — лишь состояние кнопки. Теперь рубрика
  с этапом показывается и поверх существующей ленты.


### Фидбэк-волна 3: инсайты-меню (наконец-то), чат-полировка, живой статус ленты (2026-06-11)

- **БАГ «AI-инсайты не кликаются» найден журналом отладки**: в логе не было ни одного
  `digest:toggle` — клики не доходили. Причина: `backdrop-filter` титлбара создаёт стекинг-контекст
  с `z-index:auto` → дропдаун меню (z-80 ВНУТРИ него) перекрывался контентом, рисующимся позже
  (карточка «Сводка дня» ловила клики «сквозь» меню). Фикс: явный `z-index` на титлбаре.
- **Чат**: ответы рендерятся как **markdown** (заголовки/списки/код/таблицы — сырые `##` ушли);
  плавный «айфон-стайл» вывод стрима (свежий чанк проявляется с лёгким fade/blur, по завершении —
  переключение на md-рендер); **источники свернуты компактной плашкой** «Источники · N» с
  раскрытием (Sonnet-style), и vault-, и web-; типографика вопроса = типографике ответа;
  подсказка «Shift+↵ — новая строка» вместо дубля «отправить».
- **Лента: живой статус прогона** — `news:progress` с этапами: «Опрашиваю источники · 7/16» →
  «Анализирую записи · 24/60» (гранулярно по LLM-батчам) → «Пишу сводку дня…».
- Поле URL SearXNG в настройках больше не схлопывается до 36px (flex-фикс); ResizeObserver-шум
  отфильтрован из журнала отладки.


### Чат: Web — флаг поверх режима, ресайз панели, честные подписи (ревизия владельца, 2-я итерация)

- **Web — дополнительный флаг, не третий режим**: сегмент «По заметкам | Общий» всегда активен,
  глобус лишь разрешает модели сходить в интернет. Бэкенд: если web-агент решил «веб не нужен»
  (или выдача пуста), ответ идёт в ВЫБРАННОМ режиме (vault → RAG-ретрив, general → общий), а не
  принудительно в общем.
- **«web-агент не настроен» стал типизированным**: баннер «Web-поиск не настроен» с подсказкой
  и кнопкой «Открыть настройки» (ведёт в AI / Модели) вместо сырой красной строки.
- **Ресайз AI-панели**: тянем левую кромку (side) или верхнюю (bottom), 300–720 px /
  200–560 px, размер запоминается. Перемещение окна — отдельной итерацией (overlay-вариант
  в настройках уже даёт плавающую панель).
- **Нижняя строка композера** больше не пишет «Ищу по заметкам…» в «Общем»/Web — фраза
  по режиму и флагу (как у индикатора размышлений).

### Багфикс: статусбар «работал» вечно (отчёт владельца 2026-06-11)

- Анимированный индикатор «N задач» пульсировал постоянно, хотя ничего не выполнялось: `busy`
  считался по ВСЕМ `pending`, включая суточные recurring-джобы (дайджест/лента/противоречия/
  context_drift), которые после каждого прогона переназначаются на +24 ч и всегда висят в очереди.
- `JobCounts` получил поле `ready` (pending с наступившим `run_at`); `counts(reader, now)`.
  Пульс «N задач» — только при `running + ready > 0` (работа сейчас). Запланированные на будущее
  показываются статичным чипом-часами «Запланировано · N» (без анимации, кликабелен → модалка
  очереди со временем следующего запуска). Пусто → «Проиндексировано · N».

### UX-доводки по фидбэку владельца (вечер 2026-06-11)

- **Очередь задач: человеческое время** — «через 507 мин» у суточных джоб пугало; теперь
  мин → часы → дни («через 8 ч», «через 2 дн»).
- **Сводки размышлений стали конкретными** — промпт суммаризатора переписан: вместо пустых
  обобщений («Анализирую ваш запрос») — предмет текущего шага. Live-проверка на вопросе про
  GPU-риг: «Сравниваю производительность 4090 и P40 для LLM», «Сравниваю объём VRAM двух 3090
  и одной 4090».
- **Web — кнопка-тоггл, не третий режим**: сегмент «По заметкам | Общий» + отдельная кнопка
  с глобусом и aria-pressed («модель может искать в интернете»); при включённом Web сегмент
  приглушён, выключение возвращает прежний режим.

### Ридер: чтение офсайт-статей по явному per-host consent (ревизия NF-6, решение владельца)

- Раньше статья на хосте вне доверенных источников (HN-кейс) была недоступна без вариантов.
  Теперь в denied-баннере ридера — кнопка «Разрешить <хост> и загрузить»: хост (и только он)
  добавляется в `news.json::extra_hosts` (вне vault/git — consent не приезжает с pull) и сразу
  попадает в "news"-скоуп; статья перезагружается. Снять разрешение — в gear-меню ленты
  («Доверенные хосты статей»).
- Класс защиты НЕ ослаблен: приватные/LAN-хосты отвергаются на consent-команде И политикой
  web-класса с DNS-rebinding-гардом (defense-in-depth); капы/таймауты/анти-инъекционные маркеры
  без изменений; выключение ленты гасит и extra_hosts (fail-closed).


### Отчёт владельца #3: ридер Хабра, обзор очереди задач, журнал отладки (2026-06-11)

- **Ридер: «оригинал не загружен: error decoding response body» на статьях Хабра** — антибот
  (Qrator) душит запросы без User-Agent «капельницей» и рвёт соединение (замер: без UA 64 КБ +
  reset; с браузерным UA 176 КБ за 19 с целиком). News-фетчер (фиды + статьи) теперь ходит с
  браузерным UA. «Хост не разрешён политикой эгресса» на офсайт-статьях (HN и т.п.) — НЕ баг,
  а принятое решение NF-6 (fail-closed, без расширения allowlist по клику).
- **«N задач» в статусбаре кликабелен**: модалка фоновых задач теперь показывает и очередь
  (что выполняется / что ждёт и когда), и ошибки. Бэкенд: `scheduler::list_active` +
  `get_active_jobs`.
- **Режим отладки (журнал)**: файловый лог с ротацией по дням
  (`<data_local>/app.nexus.desktop/logs/nexus.log.*`; macOS — `~/Library/Application
  Support/app.nexus.desktop/logs/`) — всё, что в stdout, плюс UI-события фронта (`log_ui_event`):
  открытия панелей/вью, отправка чата (режим+длина, БЕЗ текста — принцип AC-SEC-6), JS-ошибки
  (`error`/`unhandledrejection`). Ловит отчёты «кликнул — ничего не произошло».

### Багфикс: «стрим размышлений» молчал — утилитарная модель думала над сводками (2026-06-11)

- R1-сводки CoT генерирует `ai.fast`; после замены Qwen3 на gemma12 (:8084) каждая 6-словная
  сводка занимала ~40 с (reasoning-модель думала сама) и приезжала после конца ответа. Теперь
  `chat_util` ВСЕГДА строится с `without_reasoning()` (примитивам CoT не нужен; замер на живом
  сервере: 39.8 с → 2.5 с). Для non-thinking моделей kwarg безвреден.
- Тот же флаг возвращён в хот-апплай настроек для `chat_fast` (#153 его забыл — после сохранения
  настроек дайджест становился «думающим» до переоткрытия vault).
- Фраза до первой сводки теперь честная по режиму: «Ищу по заметкам…» только в vault-режиме,
  в «Общем» — «Думаю…», в Web — «Ищу в интернете…».


### Багфикс: 6 из 16 источников новостей падали (отчёт владельца 2026-06-11)

- **«error decoding response body» (openai/raschka/gradient/vllm)** — это единый 20-секундный
  `timeout()`, срабатывавший ПОСРЕДИ тела у медленных-но-здоровых фидов (GitHub releases.atom
  отдаёт ~14 КБ/с → 421 КБ за ~27 с; reqwest маскирует обрыв под decode-ошибку). Теперь
  анти-зависание держат connect 10 с + read-inactivity 20 с, общий потолок 120 с — страховка.
- **301 Moved Permanently (google-ai/habr-ai)** — фиды переехали, а редиректы у `GuardedClient`
  отключены политикой (AC-EGR-7): URL обновлены на конечные
  (`blog.google/innovation-and-ai/…`, `habr.com/ru/rss/hubs/…/articles/all/`).
- **body-cap 2→4 МБ** — Substack-фид с полными текстами (raschka, 2.3 МБ) легитимно больше 2 МБ;
  потолок остаётся (анти-DoS). Все 6 источников прозвонены вживую с новой схемой — 200/тело целиком.
- Строка «llm-бюджет: обработано 60 из 1157» — не ошибка, а видимый кап LLM-этапа на первом прогоне
  (бэклог фидов разбирается по 60 свежих за прогон, no silent caps).

### Багфикс: AI-панель «не открывалась» с Home/News (отчёт владельца 2026-06-11, вторая часть)

- Панель чата живёт только в workspace-вью (DP-12, макет), но `openChat`/`toggleChat` не уводили
  с Home/News: флаг взводился, панель не показывалась — кнопка выглядела мёртвой (приложение
  стартует на Home → «чат и AI-функции не открываются»). Теперь открытие чата переключает в
  workspace; клик при открытой-но-скрытой панели возвращает её в поле зрения, а не закрывает.
  Инсайты (Дайджест/Цели/Противоречия) от вью не зависели и работали — проверено в превью.


### Web-агент: свежесть выдачи для time-sensitive вопросов (доводка по live-smoke)

- Live-smoke поймал: на «последнюю версию Python» агент честно цитировал выдачу, но SearXNG
  поднимал статьи 2023-го. Теперь планировщик помечает вопросы про ТЕКУЩЕЕ положение дел
  (версии, новости, цены, «сейчас/последний») префиксом `FRESH:` → `WebSearcher` ограничивает
  выдачу `time_range=year`. Контракт плана: `NONE` | `FRESH: <запрос>` | `<запрос>`
  (`WebQueryPlan{query,fresh}`); обычные запросы time_range не несут (recall не режем).


### Live-smoke LLM-этапов на прод-серверах (2026-06-11)

- Новый модуль `live_smoke` (все тесты `#[ignore]`, CI не трогают): прод-промпты и парсеры стадий
  на живых моделях — новостной LLM-этап (фильтр+RU-резюме AC-NF-3, сводка дня AC-NF-10) и web-агент
  целиком (план на быстрой модели → реальный SearXNG с W2-консентом/DNS-гардом → ответ большой
  модели), включая «веб не нужен». Запуск: `cargo test live_ -- --ignored --nocapture`;
  хосты — env `NEXUS_CHAT_URL`/`NEXUS_FAST_URL`/`NEXUS_EMBED_URL`/`NEXUS_SEARX_URL`.
- Старые live-тесты отвязаны от стёртого сервера 192.168.0.29: чат/эмбеддер/eval смотрят на
  актуальный 192.168.0.31 (env-оверрайды), embedder-smoke переведён с nomic(768) на прод bge-m3(1024),
  `eval/baseline.json` conditions.embedding_server обновлён (модель/dim не менялись).

### «⚠ N» в статусбаре кликабелен: модалка ошибок фоновых задач (отчёт владельца 2026-06-11)

- Раньше счётчик dead-джоб был пассивным бейджем — ошибки индексации/генерации нечем посмотреть.
  Теперь клик открывает модалку: какая задача упала (человеческое имя kind), почему (`last_error`),
  сколько попыток, когда; «Повторить» (после исправления причины — сброс attempts) и «Очистить все».
  Бэкенд: `scheduler::{list_dead,retry_dead,clear_dead}` + команды `get_dead_jobs`/`retry_dead_job`/
  `clear_dead_jobs` (ADR-007 S7: смерть джобы не только видима, но и разбираема).

### Багфикс: AI-функции не работали из-за двойного `/v1` + недоступной fast-модели (отчёт владельца 2026-06-11)

- **Корень №1 (двойной `/v1`)**: если в URL чат/embed-сервера был суффикс `/v1`
  (`http://host:8080/v1`), провайдер добавлял `/v1/chat/completions` → `…/v1/v1/chat/completions`
  → 404. Probe «Проверить связь» при этом показывал «Доступен» (принимал ЛЮБОЙ ответ, даже 404),
  вводя в заблуждение. **Фикс**: `ai::api_base()` снимает хвостовой `/v1` и `/` — провайдеры
  чата/эмбеддингов и probe терпят оба варианта ввода (с `/v1` и без). После пересборки чат
  чинится без изменения настроек.
- **Корень №2 (fast-модель не в UI)**: утилитарная модель `ai.fast` (Qwen3-4B — inline/судья/
  сводка reasoning/новости) не имела поля в Настройках → при смене сервера в `local.json`
  оставался старый мёртвый хост, и эти фичи падали (мёртвые джобы дайджеста/противоречий, лента
  «не собиралась»). **Фикс**: третий блок «Быстрая модель (примитивы)» в Настройки → AI / Модели;
  пустой URL → fallback на чат-модель. Горячее применение `chat_util` и `chat_fast` при сохранении
  (раньше пересобирался только `chat`).


### Web-агент W-3: режим «Web» в чате + источники + настройки SearXNG

- Чат-режим стал 3-кнопочным: **По заметкам / Общий / Web** (стор `mode` вместо `grounded`;
  vault→grounded, general→общий, web→web-агент). Переключение блокируется во время стрима.
- **Web-источники-цитаты**: карточки с заголовком, доменом и сниппетом; ссылка открывается во
  внешнем браузере (`target=_blank rel=noopener` — недоверенный web-контент в приложение не пускаем).
- **Настройки → AI / Модели → «Web-агент (поиск)»**: поле URL SearXNG + тоггл; при включённой
  фиче с непустым URL — consent-warning «запросы уйдут на {host}» (W2). API `tauriApi.websearch`.
- AC-EGR-14 + W4: отказ `secret` (секрет в запросе не отправлен) рендерится i18n-баннером.
- Мок-ветка web для браузер-превью (карточки-источники + ответ с цитатами).


### Web-агент W-2: agent-loop 3-го режима чата «Web» (decide→search→cite)

- **`websearch::agent`** (тестируемая оркестрация на трейтах `Searcher`/`ChatProvider`):
  планировщик (мелкая модель) решает «нужен ли интернет» и выдаёт ОДИН поисковый запрос
  (`NONE` → веб не нужен, деградация к общему чату); поиск через SearXNG; лимит **W3**
  `MAX_SEARCHES=3` на ход (v1 — один запрос, потолок назван явно).
- **Билдеры промптов** (`ai::chat`): `build_web_query_messages` (план: NONE | запрос),
  `build_web_answer_messages` (ответ по результатам — каждый обёрнут anti-injection маркером,
  цитаты [n]→URL), `parse_web_query_plan` (NONE/кавычки/многострочный шум).
- **Чат-команда**: режим `web` — план → поиск → `WebSources`-событие (title/url/snippet) →
  ответ с цитатами; **tool-use запрещён** (результаты только как недоверенный контекст);
  W4-секрет в запросе и отказы политики → типизированный `denied_kind` (offline|feature|host|secret).
- Фронт-контракт: `ChatStreamEvent.webSources`, `streamRag({web})`, мок-ветка для превью.
- Вид режима «Web» в чате + источники-цитаты + настройки SearXNG-URL — W-3.


### Web-агент W-1: EgressFeature::Web + SearXNG-клиент (egress срез 4, W1–W4)

- **`EgressFeature::Web`** — второй web-класс фундамента (как NewsFeed): `allow_private=false`,
  DNS-rebinding-гард обязателен, по умолчанию ВЫКЛ; не парсится из `set_egress_feature`
  (consent = URL SearXNG, не egress-настройки).
- **Модуль `websearch/`**:
  - `config` — consent-конфиг `websearch.json` в OS config-dir (URL SearXNG = явный consent;
    сохранение непустого URL + `enabled` → `sync_egress_policy` включает фичу и кладёт хост в
    allowlist скоупа "web"; fail-safe дефолты).
  - `search` — SearXNG JSON-клиент через `GuardedClient` с `EgressFeature::Web`: переиспользует
    DNS-гард ленты (resolve→проверка всех IP→пин), **W3** (таймаут 20 с, body-cap 2 МБ),
    **W4** — исходящий запрос сканируется `git::scan_secrets` ДО сети (секрет → `SecretInQuery`,
    запрос не уходит). Нормализация результатов (title/url/snippet), cap MAX_RESULTS.
- Команды `get_websearch_config`/`set_websearch_config`; восстановление consent на старте
  (setup-hook, как news).
- Agent-loop (3-й режим чата «Web»: decide→search→ответ с цитатами, anti-injection, ≤3/ход) — W-2.


### Ночной хаускипинг 2026-06-11: перф графа + актуализация доков

- **Граф, рендер-троттл «дыхания»**: физика тикает как прежде, но на остывшем симе (после
  укладки, вне drag) React-рендер — каждый 3-й тик (~20fps): на 600 узлах CPU втрое меньше,
  микро-движение визуально сохраняется.
- BACKLOG: сняты стейл-пометки «готово к реализации» у inline-LLM (реализован IL-1..4) и
  News Feed (реализован NF-1..6); docstring SyncPanel больше не утверждает, что конфликт
  «только сигналим» (resolver — DP-10/DP-14).
- NIGHT-PLAN: журнал ночи 2026-06-11 (6 срезов, статусы).
### Тесты: watcher 61.8→97.1%, eval 48.8→93.5% — цель 70% взята (ночь 2026-06-11)

- **watcher**: юниты `to_raw_changes` (Create/Remove/Modify + игнор-фильтр; ориентация
  rename-пары по существованию путей независимо от порядка notify; деградация несклеенного
  rename) + live-smoke `VaultWatcher::new` на реальном notify (мягкий выход по таймауту на
  медленных ФС — цель: путь инициализации).
- **eval**: ignored live/bench-тесты (нужен живой сервер/vault — в CI принципиально не
  исполняются и давили метрику) вынесены в `src/eval/live_tests.rs`; гейт покрытия меряет
  `eval/mod.rs`. Запуск live-тестов не изменился (`cargo test <имя> -- --ignored`).
- Floors ратчетнуты: watcher/eval 52/45 → **85/85**, global 68 → **75** (с запасом под
  macOS↔Linux-вариативность).


### Статусбар: реальный прогресс индексации «Индексация N/M» (ночь 2026-06-11)

- `Indexer::with_progress(hook)` — хук прогресса полного скана: старт (0, total), каждые 20
  файлов, финиш (total, total); watcher-петля эмитит `vault:index-progress` {done,total}.
- Статусбар: при активном скане — настоящий прогресс-бар с шириной по факту и текстом
  «Индексация N/M» (как в макете), по финишу — обратно «✓ Проиндексировано · N».
  Приоритет слота: скан → LLM-джобы → indexed.
- Выбор (зафиксирован как рекомендуемый): прогресс в файлах, не чанках — total известен до
  начала скана, бар честный с первой секунды.


### Граф: слой поверх тела + точный перенос вида и физики макета (отчёт владельца)

- **Слой**: GraphView рендерился 4-м ребёнком грида приложения → implicit-строка ПОСЛЕ
  статусбара; на реальном vault хром (статусбар/сайднав) торчал поверх графа. Теперь граф —
  absolute-слой внутри тела: строго между титлбаром и статусбаром, рейл живой (кнопка «Граф»
  закрывает его).
- **Вид и физика по `graph.jsx`**: радиусы макета (сирота — точка 3.5, узлы 5.5..15 по
  степени); **гало сирот** — кольцо с джиттером (radial-сила + жёсткий кламп полосы
  [0.78R, 1.18R], слабое взаимное отталкивание ×0.12); связанные узлы глобального графа не
  покидают ядро (coreMax-кламп 0.27·min(W,H)); сцена глобального — 1500×1300 (локальный
  900×620); компактные пружины (дефолт linkDist 62, мягкая гравитация 0.012, ядро ≥0.022);
  сим «дышит» (alphaTarget 0.02, не замерзает); **лейблы только у активной/hover-ноды и на
  среднем зуме 1.25–3.2** (Obsidian-like); зум-пределы макета 0.25…4. Ключ настроек физики
  поднят до v2 (новые дефолты не перекрываются старым персистом).
- **Группировка по тегам** — тогглер макета (gs-switch) в настройках графа: мягкое притяжение
  узлов к центроиду первого тега.
- **Поповер изолированной заметки** (макет orphan-pop): клик по сироте → «Изолированная
  заметка» + «Предложить связь» (топ-1 предложения Ф1-9, [[линк]] открывает заметку).
- Мок-vault: +2 заметки-сироты (превью гало).
### Фикс: вёрстка Home на реальном vault (отчёт владельца)

- Секции-гриды Home (`grid2`): `1fr` = `minmax(auto, 1fr)` — длинные nowrap-заголовки реальных
  заметок распирали min-content «Недавних» и воровали ширину у «Сводки дня» (та сжималась в
  столбик). Ячейкам грида задан `min-width: 0` — честные 50/50, ellipsis работает.
- Списки Home («Недавние», stale-radar) показывают имя заметки title-first без `.md`
  (та же семантика, что в дереве/табах после DP-15) — раньше для untitled светился «Untitled.md».


### Дизайн-паритет DP-12: AI-панель по макету + reasoning-рендер (закрывает PR #97)

- **Шапка панели** (макет ai-head): глиф + «AI-ассистент» + **бейдж провайдера (E9)** —
  «Локально» / «Офлайн» (kill-switch из `get_egress_state`; «Облако» появится со срезом 3);
  табы строкой ниже с ai-подчёркиванием активного.
- **Reasoning-рендер (из закрытого #97)**: фаза «думает» = анимированный BrandThinking +
  переливающийся label со стримом живой сводки CoT (`reasoningSummary`); сырой CoT принимается
  и не рендерится; сводка не персистится. Empty-state с suggestion-пилюлями, composer-foot
  («↵ отправить» / пульс при стриме).
- **RAG-источники в трёх стилях макета** (настройка «Источники в чате»): карточки (номер-плашка
  + заголовок + сниппет) / чипы / сноски `[N]`.
- **Расположение AI-панели** (настройка): side / bottom (полоса 280px под редактором) /
  overlay (скрим, клик мимо закрывает); панель живёт только в workspace-вью (как в макете).
- **AC-EGR-14**: `ChatStreamEvent::Error` несёт типизированный `denied_kind`
  (offline | feature | host) — фронт показывает i18n-баннер RU/EN вместо сырой строки ошибки.
- Honest-адаптация: Summary-таб макета не перенесён (суммаризация живёт в inline-LLM) — BACKLOG.


### Дизайн-паритет DP-15: имена без .md, ★-бейджи, «ФАЙЛЫ +», doc-meta с временем

- Расширение `.md` больше не показывается: дерево файлов, вкладки редактора, источники
  беклинков (title-first, открытие — по прежнему полному пути).
- Заголовок секции **«ФАЙЛЫ»** с кнопкой «+» (новая заметка) над деревом (макет side-head);
  ★-бейдж избранного в дереве был с DP-2 (залитая звезда) — подтверждён.
- **doc-meta превью** дополнена clock-чипом «N назад» (новая лёгкая команда `file_mtime`;
  относительное время — общий хелпер `lib/time.ts`, переиспользован из Home).


### Дизайн-паритет DP-14: статусбар по макету

- Слева — состояние синка: дот + «Синхронизировано» (чистое дерево) / «Изменения · N»
  (тултип — путь vault); затем индексация: при активных джобах — анимированный прогресс,
  иначе «✓ Проиндексировано · N» (новая лёгкая команда `notes_count`).
- Справа — **конфликт-пилюля** (merge-required живёт в сторе и после закрытия SyncPanel;
  клик открывает конфликт-резолвер напрямую, как в макете; гаснет после успешного apply),
  далее Локально · UTF-8 · Markdown. Лейбл темы убран (в макете его нет).
- Честная адаптация: прогресса «N/M чанков» на фронте нет (индексатор пишет его только в
  tracing) — полоска показывает активность очереди джоб; заметка в BACKLOG.


### Дизайн-паритет DP-13: вертикальный ActivityBar + титлбар по макету

- **ActivityBar** (макет `app.jsx`, Obsidian/VS Code-style): вертикальный рейл на левом краю —
  Home / Новости / Файлы (тоггл сайдбара) / Граф, снизу Синхронизация (git) и Настройки;
  активная кнопка с акцентной планкой слева.
- **Титлбар разгружен по макету**: бренд «лого + Nexus» (клик → Home), плейсхолдер
  «Поиск файлов и команд…», справа только AI-инсайты (sparkles▾) | режим чтения | RU/EN |
  тема | panel-right (AI-панель). Граф/новости/sync/настройки переехали в ActivityBar;
  плагины и «Открыть vault» — командами палитры (как в макете).
- Команда палитры «Свернуть боковую панель» (макет); сворачивание — кнопкой «Файлы» рейла.


### GC кэша «Поиска противоречий» (CT-3+ хвост)

- `gc_stale_cache`: пары `contradiction_cache`, у которых хотя бы один путь больше не живёт в
  `files` (заметка удалена/переименована), выметаются — раньше копились вечно. Зовётся встроенным
  kind «gc» планировщика вместе с чисткой done-джоб; дёшево и работает без сконфигурированного AI.
  Таблица `contradictions` в GC не нуждается — каждый прогон перезаписывает её целиком (AC-CT-4).

### Frontmatter-теги → file_tags (#35 хвост)

- **Парсер**: `frontmatter_tags` — ключи `tags:`/`tag:` (инлайн-список `[a, b]`, скаляр,
  блочный список `- a`) попадают в `parsed.tags` и далее в `file_tags` при индексации.
  Нормализация — как у инлайн-тегов тела: lowercase, ASCII-набор, срез ведущего `#`.
- **Следствия**: маркер целей работает через `tags: [goal]` (не только инлайн `#goal`);
  frontmatter-теги видны в панели «Теги», на узлах графа и в фильтр-чипах.

### Реиндекс vault — команда + quick action «Переиндексировать» (Home)

- **Бэкенд**: команда `rescan_vault` — ручной полный обход `scan_vault` через новый
  `VaultEvent::Rescan` в watcher-петле индексатора (сериализован с fs-событиями — без второго
  конкурентного сканера; mtime-шорткат делает повторный обход быстрым). По завершении —
  `vault:changed`, фронт перечитывает зависимые вьюхи.
- **Home**: пятая quick action «Переиндексировать» из макета `home.jsx` (иконка-спиннер до
  `vault:changed`) + команда «Переиндексировать vault» в палитре.
- Прогресс N/M в UI не показывается (скан пишет его только в tracing-лог) — заметка в BACKLOG.

### Граф: теги — цвет узлов и фильтр-чипы

- **Бэкенд**: `GraphNode.tags` — теги заметки в узлах локального и полного графа
  (JOIN `file_tags`, IN-чанки ≤ лимита SQL-переменных, как остальные граф-запросы).
- **Цвет узла по первому тегу**: стабильная хеш-палитра oklch (`tagHue`: FNV-1a → hue;
  светлота/хрома пер-тема через `--g-tag-l/--g-tag-c`) — как `nodeColor` макета `graph.jsx`,
  но без хардкод-словаря под демо-теги; узлы без тегов — прежний цвет из CSS.
- **Фильтр-чипы** `#tag` в баре графа (топ-8 тегов текущего графа, как в макете): выбранный
  тег гасит узлы и рёбра без него, повторный клик сбрасывает.

### Дизайн-паритет DP-11: настройки — 4 темы, density auto, chrome minimal, шрифт редактора

- **Оформление**: сегмент **4 тем** (Светлая / Тёмная / Midnight / Platinum), плотность
  compact/comfortable/**auto** (брейкпоинт 1180px с реакцией на резайз, `--density` 0.82/1),
  **рамки интерфейса** standard/**minimal** (`--chrome` 0|1 — гейт прозрачности `--color-border`),
  **шрифт редактора** гротеск/сериф/моно (`--editor-font`).
- **Основное**: **имя для приветствия Home** (поле prefs, обещано в DP-1) и **позиция палитры**
  top / center / **spotlight** (увеличенный инпут) — преф применяется к Command Palette.
- Все настройки персистятся в localStorage и применяются мгновенно без перезапуска.

### Дизайн-паритет DP-10: sync/conflict — сообщение коммита + выбор сторон

- **Сообщение коммита** (макет sync.jsx): textarea в панели синхронизации; бэкенд
  `git_commit`/`git_commit_paths` принимают опциональное `message` (пустое/пробельное →
  прежнее авто-саммари) — `commit_all_with_message`/`commit_paths_with_message` в git-ядре.
- **Conflict resolver по макету conflict.jsx**: стороны кликабельны (выбранная — accent-soft
  плашка с чек-маркой, другая тускнеет), кнопки Локально / На диске / **Оба**, статус-бейдж
  «не выбрано»/выбранного на каждый конфликт, **прогресс «Разрешено N из M»** в шапке,
  **bulk** «Везде локальные / Везде с диска», ручная правка результата = выбор «Вручную»;
  **«Применить и запушить» доступно только когда разрешены ВСЕ** (раньше дефолт = «наше» —
  теперь явное решение по каждому). Джамп-рейл макета опущен: типичный merge — 1–3 конфликта.
- 2 vitest резолвера (гейт apply + бейджи/прогресс; bulk).

### Дизайн-паритет DP-9: инсайт-модалки — серифный дайджест + thinking-знаки

- **Дайджест**: серифный текст 15px/1.7 с `**bold**`-рендером (общий хелпер `lib/render`,
  переиспользован из HOME), AI-бейдж в мета-строке, генерация — «думающий» бренд-знак с шиммером
  «Анализирую vault…», пустое состояние с глифом.
- **Цели**: загрузка — BrandThinking; пустое состояние с глифом.
- **Противоречия**: поиск — BrandThinking «Сверяю утверждения…»; пустое состояние с глифом.
  Карточки A↔B с типовыми бейджами уже соответствовали макету.

### Дизайн-паритет DP-8: плагины — perm-чипы + consent-sheet

- **`PluginInfo.permissions`** (бэкенд): сводка прав манифеста чипами с уровнями риска —
  `safe` (чтение/UI/эмбеддинги) · `caution` (запись, генерация local_only) · `sensitive`
  (сеть; генерация без local_only). Deny-all = пусто.
- **Менеджер плагинов** (макет plugins.jsx): вкладки «Установленные»/«Песочница»; карточки —
  glyph, имя+версия, бейдж несовместимости, **чипы прав по уровням** (amber/red рамки);
  **consent-sheet** перед запуском плагина с не-safe правами — строки прав с risk-бейджами
  (✓/~/!) и человеческими описаниями, Отмена/Разрешить, revocable-note; решение персистится
  (`nexus.plugin.consent.v1`) и **отзывается** из карточки. Песочница (demo-iframe + журнал
  брокер-вызовов) монтируется только на своей вкладке. Маркетплейс из макета — нет данных,
  не выдумываем (BACKLOG).
- Грабли: `:` в ключах прав конфликтует с nsSeparator i18next → ключи `vault_read` и т.п.
  2 vitest (чипы; consent-флоу до песочницы).

### Дизайн-паритет DP-7: онбординг — 4 шага

- **Welcome → vault → AI → индексация → вход** (макет onboarding.jsx): eyebrow + serif-заголовок,
  степпер с дотами/чек-марками, foot-hint «Около минуты · 3 шага». Повторные запуски (персист
  `nexus.onboarded.v1`) ведут с welcome сразу в диалог vault без шагов.
- **Шаг vault**: «Открыть папку» (системный диалог — новую папку можно создать в нём же; отмена
  диалога не двигает шаг) + Demo vault в браузерном превью.
- **Шаг AI** (честный): читает `.nexus/local.json` уже открытого vault, health-pill через
  `test_ai_connection` (проверка-BrandThinking / Готов / Недоступен / Не настроен), local-first
  note, «AI можно настроить позже». Выбора local/cloud нет — cloud появится со срезом 3 egress.
- **Шаг индексации**: идёт фоном — вход доступен сразу, «Готово» по первому `vault:changed`
  (вне Tauri — мок-таймер); indeterminate-прогресс с reduced-motion-гвардом.
- App держит онбординг-экран при активном flow и после открытия vault; тест полного прохода
  4 шагов + App-тесты на флаге onboardingDone.

### Дизайн-паритет DP-6: граф — пан/зум-камера + BrandThinking-лоадер

- **Камера (v2c из BACKLOG, теперь в макете)**: wheel-зум вокруг курсора (viewBox-камера,
  пределы ×8…÷3), **пан перетаскиванием фона** (ноды гасят всплытие — drag-pin не конфликтует),
  панель **+ / − / fit** (glass, по макету graph.css), **авто-fit по остыванию раскладки**;
  курсоры grab/grabbing. Drag-координаты учитывают камеру.
- **Лоадер** — «думающий» бренд-знак с шиммер-лейблом вместо текста (макет: brand mark, не спиннер).
- Halo/ripple/flow-рёбра/drag-pin/форс-панель уже были (срезы «Граф: интерактив» v2a/v2d) —
  обновлённый макет подтверждён против них. Тег-чипы фильтра — отдельный срез (нужны теги на
  узлах из БД; BACKLOG «Граф: теги»).

### Дизайн-паритет DP-5: палитра — файлы + команды

- **Секция «Файлы»** (макет palette.jsx): непустой запрос ищет и заметки (`search_vault`, top-8,
  debounce 120мс) — секции «Файлы»/«Команды» с заголовками, единая клавиатурная навигация по
  обоим спискам, Enter по файлу открывает его в редакторе; путь — подсказкой справа.
- **Анатомия по макету**: строка ввода с иконкой поиска и Esc-хинтом, иконки строк
  (file-text/command, актив — акцент), **футер с хинтами** «↑↓ навигация · ↵ открыть».
  Варианты позиционирования top/center/spotlight — преф в DP-11 (текущая позиция = top).
- Тест: запрос находит заметку в секции «Файлы», Enter открывает её.

### Дизайн-паритет DP-4: хром — AI-меню, 4 темы, статусбар

- **Титлбар**: Дайджест / Цели / Противоречия консолидированы в **sparkles-меню** (как в макете
  app.jsx — поповер bg-elevated + elevation-2, спринг-анимация, клик-вне закрывает); добавлен
  тоггл **режима чтения**; кнопка темы **циклит 4 темы** с анимацией смены иконки
  (sun → moon → sparkles → drive).
- **Темы Midnight Ink / Platinum Slate активированы**: theme store знает 4 темы (персист,
  системный дефолт прежний), токен-блоки DP-0 теперь достижимы из UI; выбор в настройках — DP-11.
- **Статусбар по макету**: статус-дот (ошибки джоб → danger), **анимированный прогресс-бар**
  при работе планировщика («N задач», reduced-motion-гвард), бейдж ошибок, right-блок
  Local · UTF-8 · Markdown. Конфликт-пилюля git — после DP-10 (BACKLOG: дешёвый статус-канал).

### Дизайн-паритет DP-3: редактор — DnD вкладок, mode-float, типографика превью

- **DnD вкладок между панами** (контракт `text/nexus-tab` макета): перенос без дублей (вкладка
  уже в цели → просто активируется), буфер жив (в отличие от closeTab), опустевшая группа
  схлопывается, цель подсвечивается акцентной рамкой; `workspace.moveTab` + 2 store-теста.
- **Mode-float пилюля** (⌘E): плавающий glass-тоггл Edit/Preview справа сверху заметки — иконка
  показывает ДЕЙСТВИЕ (книга в правке, карандаш в просмотре), анимация смены; режим переехал
  в стор (пер-группный) — команда палитры «Редактор: правка / просмотр». Кнопка из таб-бара убрана.
- **Вкладки по макету**: иконка файла, dirty-точка ВМЕСТО крестика (а не рядом), `+` новая
  заметка в группе, tab-tools sticky, m-tabin-анимация появления; в режиме чтения — без
  крестиков/плюса/беклинков.
- **Doc-meta** в превью: «N слов · M мин чтения». **Типографика превью** по `editor.jsx`:
  буллеты-акцент и faint-номера списков, цитата-плашка `accent-soft` со скруглением, инлайн-код
  в акценте, wikilink с мягким подчёркиванием/hover-плашкой и **видимыми `[[скобками]]`**,
  тег-пилюли. **Беклинк-бар** — сворачиваемый твист-шапкой.

### Дизайн-паритет DP-2: сайдбар — icon-rail + панели «Теги»/«Избранное»

- **Icon-rail** (макет `sidebar.jsx`): Файлы / Поиск / Теги / Избранное — переключение панелей,
  активная кнопка `--color-selected` + акцент; tactile-press. Поиск переехал из «всегда сверху»
  в свою панель (с подсказкой об охвате); side-nav (Home / Новая заметка) — над панелями.
- **Панель «Теги»**: новая команда `list_tags` (модуль `tags`, счёт по живым файлам, сортировка
  по частоте, пустые скрыты) — hash-иконки в `--color-tag`, количество; клик по тегу = поиск
  по нему. Frontmatter-теги — прежний хвост BACKLOG (#35).
- **Панель «Избранное»**: звезда на hover у файлов дерева (тогл, заливка `--color-warning`),
  стор `starred` (localStorage v1; синк между устройствами — BACKLOG), список с открытием.
- 2 новых vitest сайдбара (теги→поиск; пустое избранное) + Rust-тест `list_tags`.

### Дизайн-паритет DP-1: HOME-дашборд (бэкенд H6 + страница по `home.jsx`)

- **Бэкенд H6** — `get_home_activity` (`home::activity`): heatmap правок 17 недель × 7 дней по
  локальным дням пользователя (`tz_offset_min` с фронта), изменения сегодня, тренд недели
  (тек./пред. 7 дней), серия дней (+ лучшая в окне), заметки-сироты, «Продолжить» (последняя
  правленая заметка + сниппет с диска без frontmatter/заголовков). Всё из ТЕКУЩИХ `files.updated_at`
  — истории правок нет, ограничение честно задокументировано (BACKLOG). `HomeData.recent` теперь
  с метой (`updated_at`/`words`) для карточки «Недавние».
- **Страница `HomeView`** — лендинг-вью вместо редактора (стартовая после открытия vault; файл из
  дерева/поиска возвращает в редактор): serif-приветствие по времени суток (+имя из новой
  настройки), чипы (live-провайдер из AI-конфига, путь vault, серия), hero-поиск (⌘K),
  **«Продолжить»**-карта с градиентом, быстрые действия (новая/daily/быстрая мысль в Inbox/граф),
  секции: сводка дня (AI) + недавние · активность (метрики с трендом, goal-бар рекорда,
  **heatmap**) + **мини-граф** (спираль-виньетка по реальным связям, CTA в полный граф) ·
  цели + статистика 2×2 · stale radar + открытые вопросы (AI) · смещение фокуса (AI).
  AI-карты: teal-кант, бейдж, **thinking-оверлей** с `BrandThinking` (общий компонент, DP-0
  motion-слой); refresh — фоновые джобы H2, готовность по `home:widget-updated`.
- Side-nav в сайдбаре (Home + Новая заметка, активная балка-индикатор) — полный icon-rail в DP-2;
  команда палитры «Home»; стор `home`; мок `mock/home.ts` с контентом макета (превью ≡ дизайн);
  i18n RU/EN `home.*`; 3 vitest HomeView + 3 Rust-теста activity; App-тест обновлён под
  Home-лендинг.

### Дизайн-паритет DP-0: фундамент (решение владельца — приложение = макет целиком)

- **`docs/design/handoff/` обновлён** бандлом 2026-06-10 (макет, ранее приведённый к реальности
  приложения): новые экраны settings/sync/insights/news в прототипе, **две премиум-темы Midnight
  Ink / Platinum Slate** в tokens, обновлённые graph/home/plugins/editor.
- **`src/motion.css`** — глобальный motion-слой дизайн-системы: `--ease-spring/out/inout`,
  шкала `--dur-1..4`, классы «думающего» бренд-знака (`brand-thinking`, `mt-label` с шиммером)
  + reduced-motion гварды. Компонентные entrance-анимации остаются в модулях.
- **Темы `midnight`/`platinum`** в `styles.css` (полные токен-блоки; инертны до подключения в
  theme store — DP-4/11).
- **`docs/dev/DESIGN_PARITY_PLAN.md`** — нарезка эпика DP-0..DP-12 с протоколом визуальной сверки
  и границами (AI-панель — после дизайн-PR #97; news уже в main).

### News Feed NF-6: reader — полный RU-перевод статьи + «Сократить»

- **Reader in-app** (финальная итерация макета): клик по заголовку открывает статью вместо ухода
  в браузер; панель действий «К ленте / Сократить / В заметку / Оригинал» всегда видна над
  текстом (пожелание владельца); серифная типографика (заголовок, лид-курсив, абзацы), пометка
  «перевод AI», сноска о происхождении текста; Esc возвращает в ленту; открытие помечает
  прочитанным.
- **Бэкенд `news_article`**: кэш тела (миграция 011: `body_ru`/`body_fetched_at`/`body_truncated`)
  → guarded-фетч оригинала ЧЕРЕЗ ПОЛИТИКУ NF-4 КАК ЕСТЬ (хост вне news-allowlist — HN-ссылки на
  произвольные домены, офлайн — → честный `denied` БЕЗ расширения allowlist по клику; UI отдаёт
  резюме + «Оригинал») → извлечение абзацев (`<p>`-скан с фолбэком, мусор-фильтр, потолок 24k
  символов с видимым флагом усечения — no silent caps) → **полный перевод утилитарной моделью**
  (батчи ≤3k символов, injection-маркеры на недоверенный текст статьи, строгий JSON-массив строк;
  RU-источники — passthrough без LLM, D1) → кэш: повторное открытие мгновенно и без сети.
- **`news_summarize`** («Сократить»): 3–6 RU-тезисов по кэшированному телу (без кэша — по резюме),
  тот же маркер-контракт; панель «Кратко» поверх полного текста, закрывается крестиком.
- 7 новых Rust-тестов (извлечение/фолбэк/усечение; RU-passthrough без LLM-вызова; маркеры+JSON-
  контракт перевода; чанкование длинных статей; парс тезисов; roundtrip кэша тела) + 2 vitest
  reader-теста (полный поток и denied-кейс). Live-smoke перевода — по возвращении сервера.

### News Feed NF-5: страница «Новости» (AC-NF-10/12; дизайн-handoff 2026-06-10)

- **`NewsView`** — полная страница вместо редактора (вход: Rss-кнопка титлбара + палитра
  «Новости»): AI-карточка «Сводка дня» (серифный дайджест, мета «обновлено · новых · K из M
  источников», варнинг частичного прогона раскрывает список ошибок — no silent caps), чипы-фильтры
  тем + тоггл «Непрочитанные» (серверные фильтры NF-3), рубрики-кластеры в потоке, карточки
  (RU-заголовок → оригинал в браузере с пометкой прочитанного; пустое резюме → курсивная пометка
  «Резюме недоступно», AC-NF-10; источник · отн. время · язык EN/RU; hover-действия
  прочитано/в заметку с тостом пути).
- **Состояния макета целиком**: фича выключена → onboarding-CTA с **информированным согласием**
  (число и список доверенных источников из нового `news_sources`; «Включить» пишет `news.json` и
  сразу ставит первый прогон), первый прогон (скелетон-шиммер «Собираю новости…»), пустой день,
  ошибка прогона (прошлые данные целы, retry), офлайн-баннер (kill-switch эгресса). Шестерёнка
  страницы → «Выключить ленту».
- Фронт-слой: `tauriApi.news` (+ стейтфул-мок с контентом макета — превью сверяется с дизайном
  1:1), zustand-стор `news` (оптимистичный mark-read, refetch по `jobs:changed`), i18n RU/EN
  `news.*` (паритет-тест, AC-NF-12), 6 vitest-тестов NewsView. Встроенный reader с полным
  переводом и «Сократить» — срез NF-6 (добавлен владельцем на дизайн-итерации).

### News Feed NF-4: сетевой слой — NewsFeed-фича эгресса + DNS-гард (AC-NF-6/7/8)

- **`EgressFeature::NewsFeed`** — первый web-класс политики: по умолчанию **ВЫКЛЮЧЕНА**
  (consent: единственная истина — `news.json`, синхронизируется в политику на старте и в
  `set_news_config`; через `set_egress_feature` намеренно НЕ переключается);
  `allow_private=false` — приватные/LAN-хосты запрещены даже из allowlist (W-аддендум).
  Allowlist стал **скоуповым** ("ai" — хосты `local.json`, "news" — хосты активных источников;
  `check` смотрит объединение, скоупы не затирают друг друга).
- **`GuardedNewsFetcher`** — прод-реализация `FeedFetcher`: политика ДО DNS → **DNS-rebinding-гард**
  (резолв за трейтом `Resolver`, проверка ВСЕХ IP на приватность/metadata, затем **пин
  проверенного IP** в клиент через `reqwest resolve_to_addrs` — честный
  resolve-then-connect-check без TOCTOU-щели) → guarded-GET с лимитами W3 (таймаут 20 с,
  body-cap 2 МБ чанкованным чтением — превышение видимой ошибкой). Адреса не утекают в тексты
  ошибок (политика приватности как у `EgressDenied`).
- **Wiring (остаток AC-NF-6):** `open_vault` регистрирует `NewsFeedHandler` (фетчер + утилитарная
  модель) при наличии LLM; recurring раз/сутки (no-op при выключенной фиче); сид run-if-overdue
  «при первом открытии за день» (D3). Лента полностью рабочая с бэкенда: включение фичи в
  `news.json` → прогон при открытии vault. 4 новых теста (DNS-гард: приватный/metadata/смешанный
  резолв; политика до DNS; body-cap; web-класс политики). **AC-NF-6/7/8 → covered** — из
  бэкенд-AC ленты остался только UI-срез NF-5.

### News Feed NF-3: персист + пайплайн прогона + команды (AC-NF-4/5/9/11, частично 6)

- **Миграция 010**: `news_items` (дедуп `url UNIQUE`; `read_at`/`hidden`) + `news_runs`
  (RU-сводка дня + статистика «N из M источников, K не разобрано LLM» + видимые ошибки
  источников JSON'ом — no silent caps). `news::store`: вставка `ON CONFLICT DO NOTHING`
  (повторный прогон не перетирает прочитанность), листинг с фильтрами тема/непрочитанное и
  страницами по 50 (урок #22), темы по частоте, ретенция 30 дней (items+runs),
  `filter_new_urls` — префильтр против БД, чтобы НЕ жечь LLM на уже виденных записях
  (IN-чанки ≤500 — guard лимита переменных, урок V2.3).
- **`news::run`**: полный пайплайн fetch → parse → keyword → LLM → store → сводка → ретенция
  за трейтом `FeedFetcher` (реальный сетевой фетчер на `GuardedClient`+`NewsFeed`-фиче — NF-4;
  тесты — мок с фикстурами NF-1). Бюджеты видимые: cap 60 записей в LLM за прогон (излишек
  отрезается строкой в errors), HN — до 6 запросов по ключам (percent-encode) с дедупом.
  `NewsFeedHandler` (kind `newsfeed`): перечитывает конфиг на каждый прогон, выключенная фича →
  штатный no-op (consent, не сбой), `defer_under_interactive` (S5).
- **Команды** `get_news`/`news_mark_read`/`news_to_note`/`refresh_news` (дедуп через
  `has_ready_job`)/`get|set_news_config`; конфиг `news.json` в OS config-dir (consent-носитель,
  фича по умолчанию ВЫКЛ; переопределения источников; ключи). **«В заметку» (AC-NF-11)**:
  `News/<дата> <slug>.md` с фронтматтером `source`/`news_source`, RU-резюме и ссылкой;
  уникализация суффиксом; слаг чистится от ФС-символов (анти-traversal цел); дата —
  обратный Хиннант без chrono. 12 новых офлайн-тестов. Остаток AC-NF-6 (регистрация в
  open_vault + recurring) — NF-4 вместе с реальным фетчером.

### News Feed NF-2: LLM-этап — RU-резюме/темы + сводка дня (AC-NF-3, частично AC-NF-10)

- `news::llm`: `evaluate_entries` — батчи по 10 записей за вызов; недоверенный фид-контент в
  промпте ТОЛЬКО между injection-маркерами (AC-SEC-7-паттерн), системная инструкция «данные, не
  команды»; ответ — СТРОГИЙ JSON `{i, relevant, title_ru, summary_ru, topic}` (терпим
  ```json-ограждение, мусорные/дублирующие индексы игнорируем); невалидный ответ/пропущенная
  запись/relevant без полей → `failed`-счётчик в `EvalReport` (no silent caps), сбой батча не
  валит остальные. «Перевод» по D1: RU-заголовок+резюме пишутся самой моделью; для RU-источников
  инструкция запрещает переписывать заголовок.
- `daily_digest` — RU-сводка дня (5–8 строк) из оценённых записей; маркеры сохранены
  (defense-in-depth). Провайдера выбирает вызывающий (NF-3: примитив без reasoning).
- 6 офлайн-тестов на мок-`ChatProvider`, включая ассерт «инъекция из excerpt лежит строго между
  маркерами». Traceability: **AC-NF-3 → covered**, AC-NF-10 → partial (UI-состояния — NF-5).

### News Feed NF-1: парсеры фидов + keyword-фильтр (AC-NF-1/2)

- Новый модуль `news/`: нормализация RSS 2.0 / Atom / HF daily_papers / HN Algolia →
  `NewsEntry {source_id, url, title, published_at, excerpt}`. XML — через `quick-xml`
  (низкоуровневый токенизатор: CDATA/энтити/неймспейсы руками ненадёжно; новая workspace-зависимость,
  cargo-deny зелёный), нормализация/выжимка/даты — свои мини-парсеры (RFC 3339 + RFC 2822 без
  chrono, реюз `days_from_civil` из `home::stale`); HTML в выжимке срезается, энтити декодируются,
  ≤500 символов. HN-пост без url → ссылка на обсуждение. Битый фид → типизированная ошибка
  (источник пропустится прогоном с видимой пометкой — агрегация в NF-3).
- Реестр источников v1 из спеки (19 записей, прозвонены вживую; arxiv — `default_enabled=false`,
  HN — шаблон `{query}` под ключи) + пресет ключевых слов; **этап 1 фильтра**: keyword-фильтр
  только для high-volume источников (unicode lowercase, title+excerpt), малопоточные идут в
  LLM-этап целиком, пустые ключи → fail-closed.
- Тесты: 6 замороженных фикстур реальных фидов (RSS×2 вкл. кириллицу Хабра, Atom×2, JSON×2) +
  спот-чеки диалектов + даты + выжимка + фильтр + согласованность реестра со спекой.
  Traceability: **AC-NF-1/2 → covered**, AC-NF-3..12 — pending по срезам (реестр перенесён в
  ACCEPTANCE.md по правилу спеки).

### Спека News Feed (vision→AC сессия #2, doc-first)

- **`docs/specs/news-feed.md`** — «Лента новостей» переведена из vision в реализуемую спеку.
  Решения владельца D1–D7 (2026-06-10): 16 источников v1 (каждый фид прозвонен вживую; Anthropic
  без RSS → v1 через HN+Willison; arxiv выключен по умолчанию — вместо него HF Daily Papers),
  EN-контент не «переводится», а LLM сразу пишет RU-заголовок+резюме; keyword-фильтр только для
  high-volume источников; run-if-overdue раз/сутки + кнопка; формат «RU-сводка дня + карточки по
  темам»; «в заметку» в v1, семантическая vault-связь — v2; `news_items` в nexus.db, ретенция
  30 дней; `EgressFeature::NewsFeed` **дефолт ВЫКЛ** (consent при включении), лимиты W3 +
  DNS-rebinding-гард + anti-injection-маркеры для недоверенного фид-контента. 12 AC-NF
  (механика — фикстуры/моки; качество RU-резюме — human-eval) + нарезка NF-1..5 (NF-1..4
  офлайн; планировщик и egress-фундамент уже готовы, SearXNG не требуется).
- **`docs/design/NEWS_FEED_BRIEF.md`** — handoff дизайнеру: структура страницы (сводка дня →
  чипы тем → карточки), все состояния (вкл. «фича выключена»-onboarding с консентом и
  деградацию «фиды собраны, LLM упал»), контракт данных.

### Egress срез 2 «UI/контроль», часть 1: персист политики + тогглы в настройках (net.md)

- **Персист политики эгресса (E5):** новый `net::persist` — `egress.json` в **OS config-dir**
  (осознанно вне vault: `.nexus/` приходит git-pull'ом и не должен молча расширять сетевую
  границу; и вне keychain — политика не секрет). Грузится setup-хуком на старте (нет файла/битый
  → local-first-дефолты, fail-safe), пишется командами. Kill-switch «офлайн» и per-feature
  opt-in теперь переживают рестарт.
- **Команды:** `get_egress_state` / `set_egress_offline` / `set_egress_feature`
  (`chat|embed|probe`); включение офлайна по-прежнему дорезает активный стрим через существующий
  `chat_cancel` (E10) — теперь доступно с фронта.
- **Настройки → «AI / Модели» → блок «Сеть (egress)»:** тоггл «Офлайн-режим» (E2) + per-feature
  свитчи (E6) в существующем идиоме секции (сегмент Вкл/Выкл); применяется мгновенно, без Save;
  ошибка записи файла видима (no silent caps). i18n RU/EN; стейтфул-мок для браузерного dev;
  проверено в превью (правильный воркстри подтверждён по cwd процесса — см. NIGHT-PLAN про
  ловушку nexus-web→старый чекаут).
- **Остаток среза 2** (чат-бейдж local/offline E9 + i18n-рендер `EgressDenied`, AC-EGR-14) —
  после мержа PR #97: дизайн-ветка держит чат-файлы.

### Фикс «вечных воркеров» vault + паника-страховка планировщика (аудит багов 2026-06-10)

- **Жизненный цикл воркеров привязан к `VaultContext`** (новое поле `lifecycle`): воркер
  планировщика и watcher-петля индексатора раньше были бесконечными циклами — каждый повторный
  `open_vault` плодил ДУБЛИКАТЫ (два watcher'а на каталог, двойная индексация, LLM-джобы
  закрытого vault продолжали жечь модель). Теперь: watcher живёт в контексте (drop → mpsc-канал
  закрыт → петля выходит), воркер планировщика гаснет по `tokio::sync::watch`-shutdown (sender в
  контексте). `indexer::spawn` возвращает watcher (`#[must_use]`); `scheduler::worker_loop`
  вынесен из `spawn_worker` с хуками (тестируется без `AppHandle`). Тесты:
  `worker_loop_stops_when_shutdown_sender_dropped`, `event_loop_indexes_and_stops_when_sender_dropped`.
- **Паника `JobHandler` больше не валит воркер и не вешает джобу в `running`** (раньше — вечный
  requeue без backoff на каждом открытии): вызов изолирован в `tokio::spawn`, `JoinError`(паника)
  → штатный `fail()` (attempts++/backoff/dead). Тест `panicking_handler_fails_job_not_stuck_running`.
- **Чат: смена vault посреди стрима** — осиротевший стрим теперь дорезается в `hydrate` ДО смены
  ключа персиста (хвост финализируется в историю СТАРОГО vault, отмена уходит на бэкенд). Тест в
  `chat.test.ts`. Остальные заявки аудита проверены и отклонены как ложные/by-design — разбор в
  `NIGHT-PLAN.md` (журнал 2026-06-10). Сверка traceability: **AC-Б10-2 → covered** (сделан в #17,
  localStorage per-vault).

### CI: bundle-smoke `tauri build --debug` (кросс-план #3)

- Новая CI-джоба `bundle-debug` (только push в main, ubuntu): `tauri build --debug --bundles deb`
  + проверка артефакта — пайплайн бандлинга (иконки/ресурсы/desktop-файлы/deb) больше не может
  сломаться тихо (раньше CI гонял только `cargo build/test`). Локальный де-риск на macOS:
  `.app` собирается, dmg-шаг (`bundle_dmg.sh`, hdiutil+Finder) падает в headless-шелле —
  зафиксировано в BACKLOG (🔬 проверить на живом сеансе; подпись отложена, #29).

### Vault: префикс-запрос `list_notes` + бэкенд-резолв ссылок (кросс-план #22)

- **`list_notes(query?, limit?)`** вместо безлимитного SELECT всего vault: подстрочный фильтр по
  пути/заголовку в Rust (unicode-lowercase — кириллица, которую SQLite `LIKE` не умеет),
  префикс-совпадения basename/заголовка ранжируются выше, `limit` режет после ранжирования.
  Автокомплит `[[…` стал async-источником CM6 (топ-50 по запросу) — IPC-нагрузка ограничена
  топ-N, а не ~MB на целевых 50k файлов; стор vault больше НЕ грузит весь список заметок на
  открытие (payload не растёт с размером vault).
- **Новая команда `resolve_note(target)`**: клик по `[[ссылке]]` резолвится ТОЙ ЖЕ функцией, что
  индексатор links (`resolve_target` на `&Connection`, одна семантика: путь / +`.md` / basename,
  затем алиас V4.1) → **алиасные ссылки кликабельны** (фронт-резолвер алиасов не знал — починено
  заодно). Мок-слой зеркалит обе команды для браузерного dev-режима.

### Тест-инфра: линт висячих упоминаний снятых решений (AC-Q-6, хвост кросс-плана #5)

- Новый zero-dep гейт `scripts/check-dangling.mjs` (CI-job traceability + `test-all.sh`):
  ключевые слова снятых решений — `sqlite-vec` (ANN→usearch), `petgraph` (граф→SQLite, ADR-004),
  `wasmtime` (рантайм отложен), `currentFile` (→группы/вкладки, Б12). В **коде** термин вне
  комментария → красный CI; в **доках** — замороженный per-file инвентарь счётчиков (новое
  упоминание = осознанное обновление инвентаря в том же PR; паттерн EXPECTED из `check-ignored`).
  Исторические тексты (`docs/reviews/`, бэкап v1.0, журналы) — вне скоупа. Self-test
  фейк-нарушениями на каждом прогоне. Traceability: **AC-Q-6 → covered** (последний pending
  из AC-Q-блока).

### Egress-контроль ядра — фундамент `net::GuardedClient` (#16, ADR-005-ext, срез 1)

- **Новый модуль `net/`** — единственный chokepoint исходящего HTTP ядра (E1/AC-EGR-1):
  `GuardedClient` (оборачивает приватизированный `core_client_builder`, redirect=none сохранён —
  AC-EGR-7) + `EgressPolicy` с порядком проверки per-request: **metadata-блок** (новый предикат
  `blocks_cloud_metadata`, точный `169.254.169.254`, всегда — E7/AC-EGR-12) → **kill-switch «офлайн»**
  (рубит публичные хосты, LAN/loopback живут — E2/AC-EGR-3) → **per-feature opt-in**
  `Chat/Embed/Probe` (E6/AC-EGR-5) → **host ∈ allowlist ∨ `is_private_host`** (ре-экспорт, не копия —
  AC-EGR-8/2). Отказ — типизированный `EgressDenied` ДО сокета/DNS (бэкенд-половина AC-EGR-14).
- **Неотключаемый audit** `EgressAudit` (E8/AC-EGR-4): append-only (приватный `record()`), ось
  `{feature, host(Redacted), bytes_out?, decision}`; `bytes_out` — best-effort длина тела запроса
  (AC-EGR-10).
- **Провайдеры через guarded** (AC-EGR-6): `OpenAiChatProvider`/`OpenAiEmbedder`/`probe_dim`/
  `test_ai_connection` принимают `&GuardedClient` + feature-тег — «первый egress-вектор»
  (произвольный url с фронта в `test_ai_connection`) закрыт.
- **Composition-root** (AC-EGR-13): фасад `AIClient { chat, chat_fast, chat_util, embedder, policy }`
  заменяет четыре `Arc` в `VaultContext` (решение владельца 2026-06-10: все 4 провайдера); один
  `policy`/`audit` на приложение (`AppState`); hot-swap chat пересобирает уже-guarded клиент.
  **Авто-allowlist** хостов явных `ai.chat/ai.embedding/ai.fast` из `local.json` (E4); kill-switch —
  новый `Arc<AtomicBool>` в `AppState`, дефолт «не офлайн»; «офлайн» дорезает активный стрим через
  существующий `chat_cancel` (E10/AC-EGR-11).
- **CI-grep-линт `scripts/check-egress.mjs`** (AC-EGR-1): `reqwest::Client::builder`/
  `core_client_builder` вне `net/` → красный CI (self-test фейк-нарушением; whitelist — `net/` +
  `dispatch_net` с маркером-обоснованием) + контроль единственности `fn is_private_host` (AC-EGR-8);
  врезан в CI-job traceability и `test-all.sh`. Traceability: **AC-EGR-1..13 → covered**, AC-EGR-14
  pending (i18n-фронт, срез 2).

### ADR: egress-контроль ядра — web-аддендум + фундамент-спека (#16, doc-first)

- **ARCHITECTURE.md §0** (ADR-005-ext): добавлен **web-эгресс-аддендум** (решения владельца W1–W4,
  2026-06-10) — web-агент (3-й режим чата «Web» через self-hosted SearXNG с цитатами), News Feed
  (RSS/API → keyword→LLM → vault, scheduled), cloud-fallback как новые `EgressFeature::{Web,NewsFeed,
  CloudFallback}` поверх фундамента E1–E10: SearXNG-host — consent-on-save (W2), жёсткие лимиты v1 —
  ≤3 поиска/чат-ход, News Feed раз/сутки, body-cap ~2 MB, timeout 20 с (W3), outbound `scan_secrets`
  перед отправкой (W4), `allow_private=false` + DNS-rebinding-гард для web; tool-use заблокирован до
  untrusted-канала. Фикс §4.3-фантома `AIClient` (тонкий фасад `{chat,embedder,policy}`; cloud_fallback/
  guard_first_token помечены «план, вне ADR»).
- **AC-EGR-1..14** в `ACCEPTANCE.md` (+ traceability `pending`) — критерии egress-фундамента. Дев-дока со
  срезами — `docs/dev/net.md` (срез 1 фундамент `net::GuardedClient`, срез 2 UI/контроль, срез 3 cloud,
  срез 4 web/News Feed). Doc-first: реализация фундамента — следующим срезом.

### git-sync (#10): выборочный коммит (selective staging)

- Команда `git_commit_paths(paths)` + метод `GitSync::commit_paths` — коммитит **только выбранные пути**
  (из `git_status`), а не всё-или-ничего (`git_commit`/`commit_all` без изменений). Под капотом — общее
  ядро `commit_selected(Option<&[paths]>)`: при выборе индекс сбрасывается к HEAD, затем стейджатся
  только выбранные (`add_path` для новых/изменённых, `remove_path` для удалённых) → прочие изменения
  остаются не закоммиченными (видны в следующем `status`). **Secret-scan скоупится по коммитимым** —
  секрет в НЕвыбранном файле не блокирует; устаревший/пустой выбор → `nothing-to-commit`.
- Фронт — `tauriApi.git.commitPaths(paths)`. +юнит-тесты (только выбранное; удаление + скоуп secret-scan)
  и интеграционный (внешний потребитель `commit_paths`). UI-пикер файлов — фронт/дизайн-чат. `test-all.sh`
  зелёный.

### Тест-инфра (#18): per-module coverage-ратчет (TESTING_STRATEGY §6 / AC-Q-2)

- CI-джоба «Coverage (Rust)» теперь, помимо глобального floor, проверяет **per-path покрытие критичных
  модулей** (`indexer`/`chunker`/`search`/`plugin/broker`/`plugin/permission`/`watcher`/`eval`): новый
  скрипт `scripts/check-coverage.mjs` (zero-dep node) агрегирует строковое покрытие по путям из
  JSON-отчёта `cargo-llvm-cov` и сверяет с floor'ами `coverage-baseline.json` (ратчет «не ниже»,
  допуск `tolerance` п.п. под macOS↔Linux). Просело → красный CI с дельтой по модулям; печатает факт. %
  всех модулей (no silent caps). Локально — `bash scripts/coverage.sh`.
- Floor'ы зафиксированы по первому замеру (ратчет): chunker/permission ~98%, broker 93%, search 86%,
  indexer 70% — на/выше цели 70%; **watcher 62% / eval 49% — ниже цели**, floor по факту (растить
  тестами к 70%). Авто-bump baseline + PR-комментарий с дельтой — отложено (см. `docs/BACKLOG.md`).

### HOME-дашборд (бэкенд H5): Open questions + Context drift — закрывает HOME-бэкенд (H1–H5)

- Два LLM-виджета на фреймворке H2 (`home::insights`) — первые «настоящие» генераторы поверх кэша
  `home_widgets` (H3 шёл через зеркалирование дайджеста):
  - **Open questions** (зона 4, manual): LLM сканирует последние 20 изменённых заметок и извлекает
    НЕЗАКРЫТЫЕ вопросы (риторические/незавершённые/«надо разобраться»). Контент — JSON `[{question,
    path}]`; путь валидируется против поданных заметок (без галлюцинаций). На `chat_util`.
  - **Context drift** (зона 5, scheduled): LLM сравнивает текущий фокус (последние изменённые) с целями
    (`#goal`/`#priority`) → расхождение одним абзацем. recurring раз/сутки + on-open run-if-overdue
    (через H2 `is_overdue`); НЕ on-change (концепт: «чаще нет смысла»). На `chat_fast` (больше контекста).
- Доступ через готовый H2-API: `tauriApi.home.widget(key)`/`refresh(key)` + типизированные хелперы
  `home.openQuestions()` (парсит JSON) / `home.contextDrift()` + событие `onWidgetUpdated`. +юнит-тесты
  (валидация путей вопросов, фокус/цели, пустые выборки). `test-all.sh` зелёный.
- **HOME-бэкенд закрыт целиком (H1–H5):** статика/динамика (H1), кэш+фреймворк виджетов (H2), Daily
  brief (H3), Stale radar (H4), Open questions + Context drift (H5). Дальше — фронт-визуал зон 1–5
  (дизайн-чат) поверх `tauriApi.home.*`.

### HOME-дашборд (бэкенд H4): Stale radar — обнаружение устаревших заметок

- Двухслойный радар устаревания (зона 4 концепта `PKM_Home_Concepts.md`), модуль `home::stale`:
  - **Слой 1 — скоринг без LLM** (`get_stale_radar`, мгновенно on-open): балл устаревания из метаданных
    индекса — возраст без правок (главный сигнал), `draft`/`wip`, просроченный `due`, отсутствие
    беклинков добавляют баллы; `evergreen` режет (×0.2); папки `Templates/`/`Archives/` исключены.
    Severity `red`/`orange` по порогам, топ-50 по баллу, флаги-сигналы для UI. Чистый SQL + детермини-
    рованный скоринг (веса/пороги — именованные константы, легко тюнить). Дат-парсер `due` без chrono.
  - **Слой 2 — LLM-обогащение** (`refresh_stale_radar`, manual; kind `stale_radar`): для топ-10 LLM даёт
    причину устаревания + действие (`update`/`archive`/`split`/`delete`) + подсказку; кэш `stale_cache`
    (миграция 009) на 24ч, инвалидация по правкам файла (`source_mtime`). Уступает интерактиву (S5),
    дедуп активной джобы, событие `home:widget-updated`. Неизменённую заметку повторно не судит.
- Фронт: `tauriApi.home.staleRadar()` / `tauriApi.home.staleRefresh()` + событие `onWidgetUpdated`
  (ключ `'stale_radar'`). +юнит-тесты (скоринг/пороги/evergreen, дат-парсер, исключения/ранжирование,
  обогащение+кэш+событие, пропуск LLM по кэшу). `test-all.sh` зелёный.

### HOME-дашборд (бэкенд H3): Daily brief — дайджест как home-виджет

- «Сводка дня» (зона 2 концепта, LLM, on-open) поверх фундамента H2: существующий **дайджест изменений**
  экспонирован как HOME-виджет `daily_brief` без дублирования генерации — обработчик дайджеста после
  суммаризации **зеркалит результат в кэш `home_widgets`** (`source_hash = created_at` → виджет `stale`,
  если vault правился позже) и шлёт событие `home:widget-updated`. Одна генерация — обе поверхности
  (титлбар-панель дайджеста и HOME-виджет).
- Виджет бэкает существующий kind `digest`: `WidgetRegistry` теперь хранит `key → kind`, и
  `refresh_widget("daily_brief")` ставит/дедупит именно дайджест-джобу (ручной refresh = регенерация,
  делит дедуп с кнопкой панели дайджеста). on-open/recurring/on-change приходят даром от планировщика
  дайджеста; на открытии vault — бутстрап `mirror_latest_to_widget` (показать последнюю сводку сразу).
- Фронт: `tauriApi.home.widget('daily_brief')` / `tauriApi.home.refresh('daily_brief')` + событие
  `onWidgetUpdated`. +тесты зеркалирования (генерация→кэш+событие) и бутстрапа. `test-all.sh` зелёный.

### HOME-дашборд (бэкенд H2): кэш LLM-виджетов + refresh-режимы (фундамент)

- Таблица-кэш `home_widgets` (миграция 008, `key → content, generated_at, source_hash, status`): LLM-виджеты
  дашборда генерируются ФОНОМ (планировщик ADR-007) и читаются мгновенно из кэша — модель никогда не
  блокирует загрузку HOME (концепт `PKM_Home_Concepts.md` §«Принципы»). Инвалидация по правкам vault:
  `source_hash` = `max_file_mtime` на момент генерации; текущий mtime больше ⇒ виджет `stale`.
- Фреймворк `home::widgets`: трейт `WidgetGenerator` (конкретный виджет генерит контент) → обобщённый kind
  планировщика `WidgetHandler` (снять mtime → сгенерировать → положить в кэш → уведомить фронт) → трейт
  `WidgetSink` (Tauri-эмиттер `home:widget-updated`, в тестах — заглушка). Refresh-режимы поверх ADR-007:
  on-open (`is_overdue` run-if-overdue), scheduled (recurring), on-change, manual. Ошибка генерации
  помечает кэш `error` (прежний удачный контент сохраняется), Err проброшен планировщику (ретрай/dead, S7).
- Команды `get_widget(key)` (кэш, мгновенно; вне vault — `null`) и `refresh_widget(key)` (ручной refresh:
  ставит фон-джобу `home_widget:<key>`, дедуп активной, проверка зарегистрированного ключа). Фронт —
  `tauriApi.home.widget(key)` / `tauriApi.home.refresh(key)` + событие `tauriApi.events.onWidgetUpdated`.
- Фундамент: конкретные LLM-виджеты (Daily brief — H3, Stale radar — H4, Open questions/Context drift — H5)
  регистрируются в `open_vault` поверх этого слоя. +юнит-тесты (генерация/кэш/событие, stale-инвалидация,
  run-if-overdue, ошибка→статус/сохранение контента, реестр ключей).

### HOME-дашборд (бэкенд H1): статические/динамические виджеты

- Команда `get_home_data` → `{ stats, recent, goals }` для статических/динамических зон HOME (концепт
  `PKM_Home_Concepts.md`, зоны 2–3): счётчики базы (заметки/теги/связи/слова), недавние (топ-8 по
  `updated_at`), прогресс целей (`#goal`). Чистый SQL, без LLM/кэша. Фронт — `tauriApi.home.data()`.
- Визуал HOME собирается отдельно (дизайн-чат) поверх этого API; LLM-виджеты + кэш/refresh — H2+ (план
  `docs/dev/HOME_BACKEND_PLAN.md`). +тест агрегации.

### LLM R1 (backend): живая сводка размышлений reasoning-модели в чате

- gemma в RAG-чате — reasoning-модель: до ответа идёт долгий chain-of-thought, а UI всё это время молчал
  («зависло»). Теперь backend **прокидывает размышление** в стрим: `ChatProvider::stream_chat_reasoning`
  (дефолтный метод трейта → моки/inline/дайджест/судья НЕ затронуты; реальный провайдер парсит
  `delta.reasoning_content`). `chat_rag` шлёт сырые дельты `Reasoning` (для спойлера) и **живую короткую
  сводку** `ReasoningSummary`: параллельная задача каждые ~1.5с суммаризует накопленный CoT через мелкую
  модель (`chat_util`/Qwen) в одну фразу («💭 …»), плюс финальная сводка по завершении. Отмена чата гасит
  и стрим, и суммаризатор.
- Проверено живьём: английский CoT gemma → Qwen → «Проверяю арифметику» (чистая короткая фраза).
  +тест парсинга `reasoning_content`. clippy/`test-all.sh` зелёные. **Рендер 💭-блока во фронте — отдельным
  срезом (R1b).**

### LLM: утилитарная мелкая модель `ai.fast` для примитивов (inline/судья)

- Опциональный третий эндпоинт `ai.fast` в `.nexus/local.json` — **мелкая non-reasoning модель**
  (напр. Qwen3-4B на отдельном порту), для коротких латентность-чувствительных задач. Маршрутизация:
  **inline + судья противоречий → `ai.fast`** (если задан), **дайджест → gemma** (агрегирует до 40
  заметок → нужен большой контекст), **RAG-чат → gemma + reasoning**. Нет секции `ai.fast` → всё
  падает обратно на gemma-fast (ничего не ломается).
- `inline` контекст капится до 6000 симв (под 4k-контекст утилитарной модели): для `continue` —
  хвост у курсора, для rewrite/summarize — начало выделения.
- Проверено живьём (Qwen3-4B @ :8084): rewrite корректен, судья даёт верные вердикты с чистым JSON
  (temporal/none). `VaultContext` теперь `chat`/`chat_fast`/`chat_util`; +тест парсинга `ai.fast`.
  `test-all.sh` зелёный. Пример конфига — в `config.rs`/`docs`.

### LLM R2: быстрый режим без reasoning для примитивов (inline/дайджест/судья)

- **gemma — reasoning-модель**, и для коротких/структурных задач chain-of-thought только вредит:
  замер 2026-06-09 (rewrite, `max_tokens=250`) — с reasoning **6.9с и ПУСТОЙ ответ** (CoT съел бюджет),
  без него **3.8с и нормальный ответ**; на судье противоречий — без reasoning судит так же верно (даже
  точнее тип). На СЛОЖНОМ многошаговом выводе reasoning, наоборот, помогает (ON дал верную дату, OFF
  ошибся) → для RAG-чата оставлен. Подробности — `docs/reviews/LLM_FUNCTIONAL_REVIEW.md`.
- `OpenAiChatProvider::without_reasoning()` → в тело запроса добавляется `chat_template_kwargs.
  enable_thinking=false` (единственный рабочий способ глушения CoT у этой модели). `VaultContext` теперь
  держит пару: `chat` (с reasoning — RAG-чат) и `chat_fast` (без — примитивы). Дайджест, судья
  противоречий и inline переведены на `chat_fast`; RAG-чат остаётся на `chat`.
- Это убирает «долго генерирует»/пустой дайджест и ускоряет inline ~×2 без потери качества. Отдельная
  «быстрая модель» не нужна — тот же сервер/модель, только без CoT. +1 offline-тест
  (`request_body_toggles_reasoning`). clippy/`test-all.sh` зелёные.

### Тесты: интеграционный крейт git-sync (кросс-план #12, Wave B)

- **git-sync покрыт end-to-end как внешний потребитель.** Новый `tests/git_sync.rs` (отдельная
  cargo-цель, линкуется с `nexus_desktop_lib`) — 3 теста: (1) локальный flow commit/status + secret-scan
  блокирует коммит; (2) реальный сетевой round-trip `push` → клон → `pull` fast-forward; (3) расхождение
  историй → `MergeRequired`. Закрывает пробел unit-тестов `git/mod.rs`: `pull`/`push`/fast-forward/
  `MergeRequired` раньше не покрывались (нужен был remote).
- Remote — **локальный bare-репозиторий**: для local-транспорта libgit2 не дёргает credentials-callback,
  поэтому в CI **не нужны ни сеть, ни git-identity** (`GitSync::signature()` ставит дефолтную подпись).
  `git2` добавлен в `dev-dependencies` (теми же фичами → без второй сборки libgit2).

### Eval: CI-гейт на РЕАЛЬНОМ качестве bge-m3 без живого сервера (AC-EVAL-3)

- **Реальные эмбеддинги bge-m3 заморожены в фикстуру** `eval/fixture_bge_m3.json` (18 чанков + 10
  запросов golden-набора, dim 1024) — снято разовым живым прогоном на bge-m3 (`recall@8=1.000,
  nDCG@8=0.883, MRR=0.848`, совпало с baseline). Новый тест `eval_fixture_meets_baseline` (НЕ ignored)
  ВОСПРОИЗВОДИТ эти векторы (`ReplayEmbedder`, без сети) → `index_corpus`/`run_eval` → метрики ≥ baseline
  в обычном `cargo test`/CI. Раньше реальное качество гейтилось только `#[ignore]`-тестом с живым
  сервером (в CI не гонялся); офлайн-гейт `offline_eval_gate_on_fixed_vectors` (синтетика) проверял лишь
  логику ранжирования. Теперь CI ловит и регресс реального качества эмбеддингов.
- **Регенерация — `regen_eval_fixture`** (`ignored`, нужен живой bge-m3): пишет фикстуру ТОЛЬКО если
  прогон ≥ baseline. **Guard**: в фикстуре хранится blake3-хэш `golden.json` + модель + dim; гейт при
  расхождении (изменили golden / сменили модель / чанкер дал другие чанки) падает с подсказкой
  пере-генерировать — без молчаливого прохода на устаревших векторах.
- `baseline.json`: сервер `127.0.0.1:8083` → `192.168.0.29:8083` (модель переехала на LAN-хост; модель/
  dim/метрики те же). Живые eval-тесты принимают override `NEXUS_EMBED_URL`. `EXPECTED` в
  `check-ignored.mjs` 11→12 (+regen). Все гейты зелёные.

### Рефакторинг: декомпозиция `indexer/mod.rs` (кросс-план #28, Wave B)

- **Файл-монолит разрезан по швам.** `indexer/mod.rs` (1302 строки) → когезивные подмодули:
  `links` (резолв ссылок: `resolve_target`/`resolve_all_dangling`/`path_forms`), `fs` (обход vault +
  нормализация путей/времени), `events` (watcher-петля `spawn` + `vault:changed`), `rag` (механика
  эмбеддинга/reconcile/persist векторов как `impl Indexer`), `tests`. В `mod.rs` осталась оркестрация —
  `Indexer`/`Rag`, конструкторы и 4 ядровых метода (`index_file`/`remove_file`/`rename_file`/
  `scan_vault`): **1302 → 493 строки**.
- Подмодули используют доступ дочернего модуля к приватным полям родителя (`Indexer.{writer,reader,
  root,rag,force}`), методы — `pub(super)`; публичный контракт (`Indexer::*`, `indexer::spawn`)
  сохранён через `pub use events::spawn`. Чистый рефактор, поведение 1-в-1: те же 164 теста + clippy
  `-D warnings` + `test-all.sh` зелёные.

### Рефакторинг: типизированный `AppError` в командном слое (кросс-план #9, Wave B)

- **Конец stringly-typed ошибкам команд.** Раньше каждая из ~30 IPC-команд возвращала `Result<T,
  String>` и вручную звала `.map_err(|e| e.to_string())` на каждом шаге (≈100 мест в 14 модулях) —
  доменный тип ошибки (`DbError`/`AiError`/`VaultError`/…) терялся прямо на границе. Введён единый
  `error::AppError` (`thiserror`): доменные ошибки поднимаются через `?` (`#[from]` для Db/Ai/Vault/
  Vector/Git/Cred/Plugin/io), ad-hoc — `AppError::Msg`. **Контракт фронта не изменился**: `AppError`
  сериализуется в строку (`Serialize` → `Display`), JS по-прежнему получает `string` в reject —
  `tauri-api.ts` и сторы НЕ трогались, фронт-тесты без изменений.
- **Единый аксессор vault.** Добавлен `AppState::vault()` → `Result<RwLockReadGuard<VaultContext>,
  AppError>` (через `RwLockReadGuard::try_map`): заменил повторяющийся `let g = vault.read().await;
  match g.as_ref() { Some(ctx) => …, None => return Err("vault не открыт") }` в ~20 командах. Команды,
  что при отсутствии vault отдают значение по умолчанию (счётчики джоб, список противоречий), читают
  поле напрямую — для них `?`-семантика не нужна.
- Чистый рефактор: поведение идентично, диффом −68 строк. +4 теста на `AppError` (стабильное сообщение
  `NoVault`, `From<String>/<&str>`, подъём доменной ошибки через `?`, сериализация в строку). Все 164
  backend-теста + `clippy -D warnings` + `test-all.sh` зелёные.

### Исправление: зависание дайджеста/противоречий при сбойном LLM-сервере

- **Стрим больше не висит вечно.** На реальном тесте дайджест «завис на 15-20 минут» при обновлении:
  сервер модели подвисал/флапал (`error decoding response body`), а у `stream_chat` не было таймаута →
  LLM-вызов висел бесконечно, блокируя **весь воркер планировщика** (`run_due` ждёт `handle()`), и
  «Генерирую…» не гасло. Теперь у `OpenAiChatProvider` — idle-таймаут 90с **и на ответ сервера, и на
  каждый чанк** стрима (`tokio::time::timeout`): зависший сервер → понятная ошибка `Http(таймаут…)`,
  джоба честно падает (и пойдёт в backoff), воркер свободен. +1 тест (`stream_chat_times_out_on_hung_server`:
  сервер принял запрос и молчит → провайдер с таймаутом 250мс возвращает ошибку < 3с).
- **Индикатор не залипает при сбое.** Сторы дайджеста и противоречий теперь гасят «Генерирую…/Ищу…» не
  только при свежем результате, но и когда джоба **больше не активна** (упала/таймаут/no-op) — через
  новую команду `job_active(kind)` (`scheduler::is_kind_busy`: pending|running, в т.ч. отложенная).
  +1 тест (`is_kind_busy_counts_future_pending`). clippy/`tsc`/`eslint`/`test-all.sh` зелёные.

### Поиск противоречий — CT-3: кэш вердиктов

- **Не пере-судим неизменённое.** После slice 6/7 поиск противоречий гоняется часто (раз/сутки +
  по правкам), а раньше каждый прогон заново слал ВСЕ топ-пары в LLM. Теперь — кэш `contradiction_cache`
  (миграция 007): ключ — пара путей, хэши — от тех же сниппетов, что видит судья. Пара с неизменёнными
  сниппетами берёт вердикт из кэша (**без LLM-вызова**); изменился сниппет → хэш другой → пере-судим и
  обновляем кэш. Кэшируем и «нет противоречия» (и нераспознанный ответ судьи), чтобы не молотить мусор.
- +1 тест (`cache_skips_llm_on_unchanged_pair`: 2-й прогon по неизменённым заметкам не зовёт LLM, набор
  сохраняется). clippy/тесты зелёные. Стейл-строки кэша по удалённым заметкам — безвредны (BACKLOG: GC).
  Контракт: `docs/specs/contradictions.md`.

### Планировщик задач (ADR-007) — slice 7: on-change-триггер (S4)

- **Фоновые LLM-фичи реагируют на правки.** Дайджест и Поиск противоречий теперь перезапускаются не
  только по расписанию (24ч, slice 6) и вручную, но и **после редактирования заметок** — когда правки
  «улеглись». Воркер опрашивает `max_file_mtime` (макс. `updated_at` заметок) и через
  детектор-дебаунсер (`onchange_step`, 120с тишины после всплеска правок) перезапускает on-change-kind
  (готовый job, дедуп `has_ready_job` — поверх будущего recurring, не плодит). `spawn_worker` получает
  `reader` + список on-change-kind; `open_vault` наполняет его теми же LLM-kind, что и recurring.
- Детектор — **чистый стейт-машина** (без часов внутри) → детерминированно тестируем. +3 теста
  (`onchange_step` дебаунс/однократность/пере-взвод, `max_file_mtime` пусто). clippy/тесты/`test-all.sh`
  зелёные. Планировщик ADR-007 по триггерам закрыт (manual + run-if-overdue + recurring + on-change).
  Контракт: `docs/dev/scheduler.md`.

### Планировщик задач (ADR-007) — slice 6: расписание (recurring) + дедуп

- **Фоновые LLM-фичи «живут» сами.** Дайджест и Поиск противоречий теперь **авто-обновляются раз в
  сутки**, пока открыт vault: после успешного прогона kind сам переназначается на `now + интервал`
  (`run_due` + `reschedule_if_absent` — анти-стакинг: одна будущая периодическая за раз). `spawn_worker`
  получает карту `recurring` (kind→интервал), `open_vault` наполняет её для зарегистрированных
  LLM-kind. С backpressure (S5) фон не мешает интерактиву.
- **Дедуп ручного запуска.** `has_active` → **`has_ready_job(reader, kind, now)`** (готовая `run_at<=now`
  ИЛИ выполняется; будущая периодическая НЕ блокирует) → повторный клик «Сгенерировать»/«Найти» при уже
  идущем/готовом прогоне — no-op (без пачки одинаковых джоб). Применено к дайджесту И противоречиям.
- +3 теста (recurring-переназначение; идемпотентность `reschedule_if_absent`; `has_ready_job` игнорит
  будущее). clippy/тесты/`test-all.sh` зелёные. **Дальше (slice 7):** on-change-триггер (S4 — реагировать
  на правки vault). Контракт: `docs/dev/scheduler.md`.

### Поиск противоречий — CT-2: панель (#vision)

- **Фича видна и тестируется на живой модели.** Панель «Поиск противоречий» из титлбара (иконка-весы) +
  команда палитры `view.contradictions`: список найденных пар **«A ↔ B»** с типом (фактическое/мягкое/
  устарело) и объяснением; клик по заметке открывает её; кнопка **«Найти»** (ставит фоновую джобу).
  Поиск асинхронен — результат прилетает по `jobs:changed` (refetch, только когда панель открыта; общий
  слушатель с дайджестом). Контракт `tauriApi.contradictions.list()/generate()` + стор `contradictions`
  + мок для превью; i18n RU/EN. +2 теста (`ContradictionsPanel`). **Дальше:** CT-3 — кэш по контент-хэшу
  пар (не пере-судить неизменённое). Контракт: `docs/specs/contradictions.md`.

### Поиск противоречий — CT-1: бэкенд (#vision, спека `docs/specs/contradictions.md`)

- **Новая фоновая LLM-фича поверх планировщика.** Kind **`contradictions`**: пары-кандидаты по
  семантической близости (bge-m3/usearch, переиспользуем `suggest::get_related_notes`, порог 0.62,
  топ-24 пар за прогон) → **LLM-судья** даёт JSON-вердикт `{contradiction, type, explanation}` (типы
  **hard/soft/temporal**, D3) → таблица `contradictions` (миграция 006). Тексты заметок — данные в
  анти-инъекционных маркерах (AC-SEC-7). Прогон **заменяет** прошлый набор (CT-1 без кэша). Уступает
  интерактиву (S5: `defer_under_interactive`). Регистрируется только при **chat + векторах**.
- **Запуск (D1):** команда `generate_contradictions` (вручную, дедуп активной джобы через
  `scheduler::has_active`) + run-if-overdue seed на открытии. Команда `get_contradictions` (список).
- **Устойчивый парс вердикта:** срезает ```-фенсы/прозу, берёт первый `{…}`, нормализует тип (дефолт
  `soft`). +5 Rust-тестов (парс plain/fenced/мусор; обёртка маркерами; найденное/пустое противоречие на
  мок-эмбеддере+мок-судье). Качество вердиктов LLM — **human-eval, не автотест** (спека §3).
- **Дальше:** CT-2 — панель из титлбара (список + «Найти сейчас» + refetch по `jobs:changed`); CT-3 —
  кэш по контент-хэшу пар (не пере-судить неизменённое). Контракт: `docs/specs/contradictions.md`.

### Планировщик задач (ADR-007) — slice 5: backpressure чата (S5) + StatusBar

- **Интерактив важнее фона (S5).** Дайджест-джоба больше не делит локальную модель с тем, что ты делаешь
  руками: пока идёт **чат или inline-генерация**, фоновые LLM-джобы **уступают**. `JobHandler::
  defer_under_interactive()` (дайджест → `true`); `AppState` считает активные интерактивные LLM-операции
  (RAII-гард в `chat_rag`/`inline_complete`); воркер каждый тик читает «занят ли интерактив» и
  откладывает тяжёлые джобы (`defer`: `running→pending`, `run_at = now+тик`, **без** штрафа `attempts` —
  это уступка, а не сбой). Лёгкие (`gc`) идут всегда.
- **Индикатор задач в StatusBar.** Теперь видно состояние очереди: **⚙ running · ⏳ pending · ⚠ dead**
  (S7/S8 — видимость). `scheduler::counts` + команда `get_job_counts`, стор `jobs`, refetch по
  `jobs:changed` (без поллинга). Так заметно, как дайджест ждёт в очереди, пока ты в чате, и стартует,
  когда освободишься. +3 теста (backpressure-defer, counts, StatusBar-индикатор). clippy/тесты/
  `test-all.sh` зелёные. **Дальше (slice 6):** дедуп kind в очереди, on-change-триггер (S4), расписание.
  Контракт: `docs/dev/scheduler.md`.

### Inline-LLM в редакторе — slice IL-3: триггеры UX (#vision)

- **Тулбар по выделению (D4).** Над непустым выделением всплывает тулбар (CM6-tooltip) с действиями
  **Переписать / Сократить / Продолжить** → генерация по выделению (AC-IL-9). Скрыт, пока активно
  предложение. Клик на `mousedown` (не теряем выделение).
- **Команды палитры.** `editor.inline.continue/rewrite/summarize` через **реестр активного редактора**
  (`lib/editor/activeView.ts`) — inline доступен из палитры/хоткеев, а не только `Mod-i`.
- **Видимая ошибка у курсора (AC-IL-7).** Раньше сбой/ненастроенный chat были «тихими» — теперь
  красный `⚠`-виджет у курсора с локализованным сообщением (нет выделения / нет текста / ошибка
  бэкенда), авто-снятие через 6с или по Esc/правке. Без модала, редактор полностью рабочий.
- **a11y (AC-IL-10).** Подсказка «Tab — принять · Esc — отклонить» у готового предложения + индикатор
  «✦ генерирую…» до первого токена (AC-IL-1); `aria-live` (`InlineAria`) анонсирует статус скринридеру.
- +4 фронт-теста (тулбар, реестр активного view). **Отложено (no silent caps):** slash-меню `/` (D5;
  `/ask` нужен новый режим бэкенда) — IL-3b, в BACKLOG. clippy/тесты/`test-all.sh` зелёные. Контракт:
  `docs/dev/inline-llm.md`.

### Inline-LLM в редакторе — slice IL-2: ghost-text в CM6 (#vision)

- **Предложение модели прямо в тексте.** Ghost-text (серый курсив) у курсора: `Tab` — принять, `Esc` —
  отклонить. CM6-расширение `inlineGhost` (StateField + виджет-декорация + эффекты
  `setGhost`/`appendGhost`/`endGhostStream`/`clearGhost`): позиции маппятся через правки, **снятие при
  наборе** (как автокомплит). `acceptGhost` заменяет диапазон `from..to` (AC-IL-3); `rejectGhost`
  оставляет документ нетронутым (AC-IL-4); клавиатура `inlineKeymap` (`Prec.highest`) перехватывает
  `Tab`/`Esc` **только при активном ghost** (иначе штатное поведение, AC-IL-5).
- **Контроллер/стор `inline`**: `runInline(view, mode)` собирает контекст по D2 (`continue` — до курсора;
  `rewrite`/`summarize` — выделение), стримит результат с **rAF-троттлом** (≤~60 рендеров/с, AC-IL-2),
  один активный стрим за раз (AC-IL-8); `error` → тихий флаг (AC-IL-7). `cancelInline` гасит стрим
  (AC-IL-6). Контракт фронта `tauriApi.inline.complete()/cancel()` + мок `streamInline` для превью.
- **Триггер (минимальный):** `Mod-i` → продолжить у курсора. Полные триггеры (slash-меню D5, тулбар по
  выделению D4, команды палитры, a11y) — **IL-3**. +10 фронт-тестов (`inlineGhost`/`inline`, офлайн).
  Контракт: `docs/dev/inline-llm.md`.

### Inline-LLM в редакторе — slice IL-1: бэкенд (#vision, спека `docs/specs/inline-llm.md`)

- **Команда `inline_complete`** (поверх `ChatProvider`, ADR-005): стрим результата в редактор через
  `Channel<InlineStreamEvent>` (`token`/`done`/`error`), **без RAG** — контекст = текущая заметка (D2).
  Режимы `continue` / `rewrite` / `summarize` (`InlineMode`): `continue` работает с текстом до курсора,
  `rewrite`/`summarize` — с выделением (валидация пустого ввода/режима → понятная ошибка, AC-IL-7).
  Промпт по режиму требует вернуть **только результат**; контент заметки обёрнут случайным маркером
  (анти-инъекция AC-SEC-7, переиспользован из RAG). **Отмена** `inline_cancel` + независимый от чата
  токен `AppState::begin_inline` (один активный inline-стрим за раз, AC-IL-6/8). +4 Rust-теста (парс
  режима, обёртка payload маркером, различие режимов). **Дальше:** IL-2 (CM6 ghost-text + accept/reject +
  rAF-стрим), IL-3 (slash-меню + тулбар по выделению + a11y). Контракт: `docs/dev/inline-llm.md`.

### Планировщик задач (ADR-007) — slice 4: первый LLM-kind «Дайджест изменений» + UI (#35)

- **Первая настоящая фоновая LLM-фича (бэкенд, 4a).** Kind **`digest`**: собирает заметки, изменённые за окно (сутки,
  лимит 40 + сниппет 200 симв.), отдаёт chat-провайдеру (ADR-005) одним промптом → краткий дайджест →
  таблица `digests` (миграция 005). Регистрируется в реестре **только при сконфигурированном chat**
  (иначе kind отсутствует — джоба не зависнет в `dead`); пустой vault → успех без записи (нечего
  суммировать). Сидинг **run-if-overdue** (S2): на `open_vault` ставим джобу, только если за последнее
  окно дайджеста ещё не было. Команды `get_latest_digest` / `generate_digest` (последняя требует chat —
  понятная ошибка вместо тихого dead-letter). +2 офлайн-теста (FakeChat).
- **UI-панель дайджеста (фронт, 4b).** Модалка из титлбара (иконка-газета) + команда палитры
  `view.digest`: последний дайджест (контент + мета «когда · сколько заметок») и кнопка «Сгенерировать»
  (ставит джобу). Генерация **асинхронна** — готовый результат прилетает по событию `jobs:changed`
  (refetch, только когда панель открыта; без поллинга), кнопка показывает «Генерирую…» до прихода свежего
  дайджеста. Контракт `tauriApi.digest.latest()/generate()` + `events.onJobsChanged`, zustand-стор
  `digest`, i18n RU/EN, мок для превью. +2 теста (`DigestPanel`). Теперь фичу видно на живой модели.
- **Дальше:** backpressure чата (S5: приоритет интерактивного чата над дайджест-джобой), дедуп одинаковых
  kind в очереди, on-change-триггер (S4), StatusBar N/M по `jobs:changed` (slice 5). clippy/тесты/
  `test-all.sh` зелёные. Контракт: `docs/dev/scheduler.md`.

### Планировщик задач (ADR-007) — slice 3: live-спавн + первый kind

- **Очередь ожила end-to-end.** `open_vault` строит `default_registry` (встроенный kind **`gc`** —
  самоочистка завершённых джоб, S7), спавнит воркер (как индексатор: clone write-actor + crash-recovery
  на старте) и сидит gc-джобу на ближайший тик. Конвейер работает целиком: spawn → enqueue → claim →
  выполнение обработчика → `done` → событие `jobs:changed`. +1 тест (`gc_kind_registered_and_runs`).
  **Дальше (срез 4):** первый LLM-kind (Карта/Противоречия — на живых моделях), backpressure чата (S5),
  run-if-overdue-расписание + дедуп (S2), on-change-триггер (S4). Грабли (общие с индексатором): воркер
  спавнится на каждый open → дубли при переоткрытии vault (нужен shutdown-сигнал — BACKLOG). clippy/тесты
  зелёные. Контракт: `docs/dev/scheduler.md`.

### Планировщик задач (ADR-007) — slice 2: движок диспатча

- **Реестр обработчиков + воркер-луп.** `JobHandler`-трейт (`#[async_trait]`) + `Registry` (kind→handler);
  **`run_due`** — детерминированное ядро тика: `claim_next` → диспатч по `kind` → `complete`/`fail`
  (неизвестный kind → fail → видимый dead), не более 64 джоб/тик (анти-голодание); **`spawn_worker`** —
  воркер-луп (tokio-interval 5с, S1) с crash-recovery на старте, шлёт `jobs:changed` после продуктивного
  тика. Слой-1-функции переведены на `&WriteActor` (воркер держит клон, как индексатор). Пока **не
  спавнится из `open_vault`** (нет kind/энкьюеров — live-спавн в срезе 3, чтобы пустой воркер не
  dead-летил джобы); backpressure чата (S5) — с LLM-kind. +1 тест (`run_due_dispatches_by_kind`).
  clippy/тесты зелёные. Контракт: `docs/dev/scheduler.md`.

### Планировщик задач (ADR-007) — slice 1: очередь `jobs`

- **Слой данных очереди фоновых задач.** Первый срез планировщика (обе hard-dep — #13 rebuild-примитив
  и event-канал — закрыты). Миграция 004 `jobs(kind,payload,state,run_at,attempts,max_attempts,…)` +
  `idx_jobs_claim`; модуль `scheduler` с `enqueue` / `claim_next` / `complete` / `fail` / `requeue_running`
  / `gc_done`. Состояния **pending→running→done|dead** по решениям owner-codesign: claim **без гонок**
  (сериализован write-actor'ом, ADR-003), экспоненциальный backoff + `max_attempts` → **видимый `dead`**
  (S7, не тихий дроп), crash-recovery `running→pending` на старте (S8), offline-джобы ждут в `pending`
  (S10). Логически значимое время — явными параметрами → детерминированные офлайн-тесты (+3). Воркер-луп
  (tokio-interval S1, приоритет чата S5), триггеры (S2/S4) и первые kind (Карта/Противоречия S3) —
  следующие срезы; сетевые kind (News Feed) ждут egress (#16). Контракт: `docs/dev/scheduler.md`.
  clippy `-D warnings` + Rust-тесты зелёные.

### Фундамент — примитив пересборки FTS в раннере миграций (#13)

- **`rebuild_fts`-хук в раннере миграций** (`db/migrations.rs`). `Migration` получил флаг `rebuild_fts:
  bool`: миграция, инвалидирующая FTS5 (смена схемы/конфига `fts_chunks`), ставит его → после её SQL
  раннер пересобирает `fts_chunks` из `chunks` встроенной командой FTS5 `'rebuild'` (external-content,
  без переразбора файлов). **Примитив резюмируемости**: снимает sequencing-trap — будущие схемо-миграции
  (#14 re-chunk, `jobs`-таблица планировщика) **не заставят пользователя сносить `.nexus`**. usearch
  (смена размерности) этим не покрыт — реконсайл индексатора на открытии (нужен embedder), задокументировано
  (`docs/dev/db.md`). +2 теста (rebuild после рассинхрона; идемпотентность раннера). Это **последняя
  hard-dep планировщика** (ADR-007) — вместе с event-каналом он полностью разблокирован. clippy/тесты зелёные.

### Фундамент — Tauri event-канал + живые «Цели» (ADR-007 S8)

- **Event-канал backend→фронт (первый `.emit` в проекте).** По решению планировщика-ADR (S8) добавлен
  канал событий индексатора: после каждого реиндекса бэкенд шлёт `vault:changed` (`indexer::spawn` +
  `AppHandle`, проброшенный из `open_vault`). Фронт подписывается (`tauriApi.events.onVaultChanged` →
  `App.tsx`, дебаунс 800мс). **Первый потребитель — «Цели» (#35): AC-GP-3 закрыт** — список целей
  пересчитывается **живо** при правке любого `#goal`-файла (когда панель открыта), без планировщика.
  Фундамент для будущих live-фич и StatusBar-прогресса джоб (ADR-007). Rust compile/clippy + 118
  фронт-тестов зелёные.

### Архитектура — ADR-решения (codesign egress + планировщик)

- **Приняты owner-решения по egress (#16) и планировщику (#21)** из codesign-сессии (`docs/reviews/ADR_CODESIGN.md`).
  В `ARCHITECTURE.md §0`: **egress зафиксирован как расширение ADR-005** (единый `net::GuardedClient`-chokepoint:
  kill-switch «офлайн» рубит облако/web но не LAN, per-feature opt-in, allowlist из `local.json`, in-memory audit,
  metadata-блок, политика в OS config-dir; E1–E10) и **новый ADR-007 «Планировщик фоновых задач»**
  (`tokio::interval`-движок, run-if-overdue, жёсткий приоритет чата, backoff+видимый-dead+GC, кэш по `indexed_at`,
  Tauri event-канал как HARD-dep; S1–S10). `AC-SEC-4` уточнён (явные `ai.*.url` разрешены ядру, metadata reject
  всегда; плагинный `net.fetch` — под полным `is_private_host`). **Реализация** egress — после #5+#9; планировщика —
  после #13 (rebuild-примитив) + event-канал. Сетевой vision-класс (News Feed) разблокируется только обоими.

### Кросс-план — Wave B (точечные)

- **#35 vision: «Прогресс целей» (Goal Progress).** Вторая vision-фича, тоже **офлайн** (чистый SQL-read,
  без LLM/сети). Панель **«Цели»** из титлбара (🎯): vault-широкий дашборд заметок-целей с прогресс-барами.
  Маркер — **инлайн-тег `#goal`** (D5), прогресс — из frontmatter-поля `progress`: бэкенд `list_goals()`
  JOIN `tags`+`file_tags` + LEFT JOIN `frontmatter_fields`. Шкала **0–100** (D6: `0≤x≤1`→×100, срез `%`);
  битое/отсутствующее значение → **бейдж «нет прогресса»** (D7, не тихий 0%). Клик по цели открывает
  заметку; пустое состояние подсказывает конвенцию (AC-GP-5). Команда `view.goals` (палитра). **v1:**
  обновление по открытию панели/кнопке/смене файла — живой пересчёт по любому modify требует event-канала
  индексатора (BACKLOG, hard-dep #21). +3 теста (Rust парс+JOIN с D7; фронт-смоук). Спека:
  `docs/specs/related-and-goals.md` (AC-GP-1…7). Vision-волна (Related → Goals) завершена; дальше — inline-LLM.

- **#35 vision: «Похожие заметки» (Related Notes).** Первая «умная» дифференцирующая фича на готовом
  RAG-фундаменте, **офлайн** (без LLM/egress/планировщика). Новая вкладка **«Похожие»** в AI-панели:
  семантически близкие заметки для открытого файла из сохранённых usearch-векторов (max-sim, без
  embedder-сервера). В отличие от «Связей» — **дискавери**: показывает всё близкое, **включая уже
  связанные** (D2); «вставить связь» дописывает `[[wikilink]]` и **оставляет строку** (AC-RN-6); клик по
  заголовку открывает заметку. **Порог релевантности — слайдер** (настройка с v1, D4; персист). Бэкенд —
  общее ядро max-sim вынесено в `collect_related(exclude_linked)`: «Связи» (`exclude_linked=true` + порог
  0.55) и «Похожие» (`get_related_notes`, `exclude_linked=false`, порог в UI) делят один код. Решения
  зафиксированы codesign-сессией; спека — `docs/specs/related-and-goals.md` (AC-RN-1…7, AC-X-1).
  +4 теста (Rust `related_includes_linked_similar`; фронт стор+вью). Дальше — «Прогресс целей» (#goal).

- **#20 Markdown-preview (read-only render).** Раньше заметка показывалась только сырым исходником
  (`view.reading` лишь прятал чроме) — главный UX-долг против Obsidian. Добавлен переключатель
  **Исходник/Просмотр** в панели вкладок (`GroupPane`, кнопка-книга, для `.md`) и компонент
  `MarkdownPreview` (`react-markdown` + `remark-gfm`): заголовки, списки, **GFM** (таблицы, таск-листы,
  ~~strike~~), цитаты, код-блоки, картинки + Nexus-специфика — кликабельные `[[wikilink]]` (→ навигация)
  и `#tag`-чипы (remark-плагин `remarkNexus` на mdast-уровне → внутрь код-фенсов не лезет). **CSP-safe**:
  сырой HTML не рендерится (нет `rehype-raw`), `urlTransform` режет `javascript:`/`data:`. Превью уважает
  «читаемую ширину» (`--editor-max-width`). **Отложено** (требует inline-стилей, запрещённых строгим CSP):
  математика **KaTeX** и диаграммы **Mermaid**; Live-preview (inline-правки) — пост-v1 эпик; vault-локальные
  картинки (нужен asset-протокол). Всё — в `docs/BACKLOG.md`. +11 тестов (сплиттер + рендер + клик + анти-XSS),
  покрытие выше порогов (branches 79%). Контракт: `docs/dev/editor.md`.

- **#17 Персист истории чата (между сессиями).** Лента RAG-чата раньше жила только в памяти и терялась
  при перезапуске. Теперь сохраняется **на каждый vault** (`localStorage`, ключ `nexus.chat.v1:<root>`)
  и восстанавливается при открытии vault (`App.tsx` → `useChatStore.hydrate` по смене корня). Запись —
  на терминальных событиях (done/error/stop/clear), без записи на каждый токен; стрим-флаги при загрузке
  снимаются. Хвост ограничен **последними 100 сообщениями** (защита localStorage от разрастания — см.
  `docs/BACKLOG.md`, принцип «no silent caps»). Разные vault — раздельные истории. +3 теста стора
  (round-trip, изоляция по vault, clear→пусто). Frontend 102 теста.

### Раздел настроек (кросс-план #11, по образцу Obsidian)

- **Слайс 1 — оболочка раздела + «Оформление» + «О программе».** Разрозненная панель «Оформление»
  заменена полноценным **разделом настроек**: модалка с левым навом секций + контент-панель
  (`components/settings/SettingsView`). Секции v1: **Оформление** (тема/акцент/плотность — перенесено из
  бывшей `TweaksPanel`, та удалена), **О программе** (имя/версия/путь vault), **AI / Модели** и **Горячие
  клавиши** — заглушки (следующие слайсы). Команда `view.settings` (**Cmd/Ctrl+,**, палитра), шестерёнка
  в титлбаре. Состояние/активная секция — в ui-сторе (`settingsSection`, `openSettings`). Тест-смоук
  (нав + переключение секций). Frontend 91 тест, tsc/eslint/build зелёные.
  - Слайс 1 завершён (PR #60). Дальше — слайс 3 (переназначение горячих клавиш).

- **Слайс 2 — «AI / Модели» (chat + embedding из UI).** Секция-заглушка заменена рабочей формой
  (`SettingsView` → `AiSection`): поля **URL** и **Модель** для chat- и embedding-эндпоинтов, кнопка
  **«Проверить связь»** и **«Сохранить»**. Бэкенд — три IPC-команды (`commands/settings.rs`):
  `get_ai_config` (префилл из `.nexus/local.json`), `set_ai_config` (запись в `local.json` с
  **сохранением прочих ключей** через `serde_json::Value` + **горячее** применение chat-провайдера в
  state — без перезапуска) и `test_ai_connection` (пробный `GET /v1/models` через `core_client_builder`,
  redirect=none — анти-SSRF, AC-SEC-4). Смена **embedding** требует перезапуска (на нём висит индексатор;
  in-place hot-swap небезопасен) — UI явно сообщает об этом после сохранения (флаг `embeddingChanged`).
  Закрывает боль «чат/связи не работают без ручного `local.json`». Фронт: `tauriApi.settings.*` + мок для
  превью/тестов; 3 теста секции (форма, проверка связи, save + требование перезапуска). Rust unit-тест
  (`apply_ai`: мерж сохраняет ключи и детектит смену embedding). clippy/tsc/eslint/тесты зелёные,
  покрытие выше порогов. Контракт раздела: `docs/dev/settings.md`.

- **Слайс 3 — «Основное» + «Редактор».** Две новые секции. **Основное** (General): переключатель языка
  **RU/EN** (раньше — только мелкий тогл в титлбаре; он оставлен для быстрого доступа). **Редактор**:
  **«Читаемая ширина строки»** (как Obsidian «Readable line length») — ограничивает и центрирует колонку
  текста через CSS-переменную `--editor-max-width` (тема `.cm-content`), по умолчанию ВКЛ (~44rem). Новый
  `stores/prefs.ts` (персист localStorage + применение на старте — приём из theme-стора; **без**
  пересоздания CodeMirror-вью, чистый CSS-каскад). Нав раздела переупорядочен (Основное → Редактор →
  Оформление → AI → Горячие → О программе), дефолтная секция — «Основное». i18n ru/en, +2 теста
  (язык-секция; тогл ширины меняет prefs-стор и CSS-переменную). Frontend 96 тестов, tsc/eslint зелёные.

- **Слайс 4 — «Горячие клавиши» (переназначение).** Секция-заглушка заменена списком всех команд с их
  текущим хоткеем: **«Изменить»** → захват комбинации (capture-фаза `window` — перехватывает раньше
  глобального `useKeymap`, чтобы нажатие не сработало как команда; Esc — отмена; требуется модификатор),
  **сброс** к дефолту, подсветка **конфликтов** (одна комбинация у ≥2 команд). Движок ремапа уже был в
  реестре (`commands.ts`: `setUserKey`/`resolve`, приоритет **пользователь > плагин > ядро**) — добавлены
  **персист** в localStorage (`nexus.hotkeys.v1`, загрузка в конструкторе реестра), обратный поиск
  `userKeyFor`/`effectiveKey`, высокоуровневые `remap`/`resetKey`. i18n ru/en, +2 unit-теста реестра
  (ремап/сброс/эффективный ключ; персист) и +1 UI-тест (список, захват комбинации, сброс). Раздел
  настроек **полностью укомплектован**: Основное · Редактор · Оформление · AI · Горячие клавиши ·
  О программе. Frontend 99 тестов, tsc/eslint зелёные.

### Кросс-план — Wave A (quick-wins)

- **#23 Render-smoke `GraphView`.** Граф-вью исключён из coverage (view-слой) → крах при монтировании
  не ловился. Добавлен `GraphView.test.tsx`: мок `d3-force` (детерминированные x/y, без утечки таймера
  d3-timer) + мок `getFullGraph` → монтирование в режиме «весь vault» рисует узлы (`.g-dot`×2) без
  краха. Тест, прод-код не трогался. **#14 (токенайзер) и #18-Rust (per-module coverage) при разведке
  оказались НЕ «контейнерными»:** #14 меняет границы чанков → каскад на тесты/eval (место — перф-эпик
  #14→#15→#6); #18-Rust требует парсинга `llvm-cov --json` (нужен медленный прогон, чтобы сделать
  безопасно). Перенесены в Wave B с аккуратной проработкой (no silent caps).

- **#7 Гейт синхронизации версии.** `scripts/check-versions.mjs` сверяет версию приложения во всех
  **4 источниках** (`package.json` ×2, `Cargo.toml [workspace.package]`, `tauri.conf.json`) — бамп одного
  с забытыми остальными → красный CI (важно для crash-отчётов и updater). Добавлен в CI-job
  `traceability` и `test-all.sh`. Сейчас все консистентны на `0.0.0`; сам бамп номера — release-решение
  владельца (Wave C). Zero-dep.
- **#8 Единый разбор `local.json`.** Раньше `open_vault` читал и парсил `.nexus/local.json` **дважды**
  (`build_rag` + `build_chat`). Теперь `load_local_config` парсит ОДИН раз, передаёт `&LocalConfig` в оба;
  `build_rag(root, db, cfg)` / `build_chat(cfg)`. Меньше IO/парсинга на открытие vault, единая точка.
  Поведение не меняется (clippy/тесты зелёные).
- **#33 Конвенция `Redacted` — чеклист ревью** (`docs/dev/security.md`): что проверять при добавлении кода
  (новое Debug-поле с контентом/секретом → `Redacted`; новый эгресс → через будущий guarded_client;
  panic с данными → scrub). CI-автолинт «секрет-полей» — фуззи, отложен (BACKLOG); пока ревью-чеклист.
- **#5 Синк документации с кодом (дрейф-фиксы).** Убрана **стрей-строка** в `TESTING_STRATEGY.md` (утёкшая
  agent-наррация в строке 1). ARCHITECTURE §2 «Структура репозитория» помечена ИЛЛЮСТРАТИВНО-ЦЕЛЕВОЙ +
  описана реальная плоская раскладка (нет `ai/client.rs`/`schema.rs`/`wasm.rs`/`registry.rs`/`graph/store.rs`;
  граф в SQLite, схема в `migrations/*.sql`). (§4.3/§5.1 уже поправлены в docs-актуализации.) Авто-линт
  висячих упоминаний (sqlite-vec/petgraph/WASM) — отдельно (фуззи, нужен allowlist контекста).

- **#4 Гейты против ложной зелени.** (а) `check-traceability.mjs` теперь **проверяет существование имён**
  в `tests[]`: rust-тест-модуль реально есть (`mod tests`), фронт-тест-файл существует — мёртвая ссылка
  на тест ловится (раньше гейт верил на слово). (б) Новый `scripts/check-ignored.mjs` — **гейт числа
  `#[ignore]`** (=11): тихо отключённый тест → красный CI, нужно осознанно поднять `EXPECTED`. Оба
  добавлены в CI-job `traceability`. (г) `scripts/test-all.sh` — одна команда на все локальные проверки
  (preflight·traceability·#[ignore]·Rust fmt/clippy/test·фронт tsc/eslint/vitest/build). (в) `vitest
  --allowOnly` — **уже** false в CI по умолчанию (`!isCI`), отдельная правка не нужна (фактчек). Zero-dep.

- **#1 Команда «Новая заметка» + пустое состояние.** Раньше создать заметку было НЕЧЕМ
  (`write_file` есть, но команды/кнопки нет → пустой vault = тупик первого впечатления). Добавлены:
  `vault.createNote(dir, {baseName, content})` (уникальное имя `Untitled`/`Untitled N`, запись, обновление
  дерева+notes); команда **`file.new`** (`Cmd/Ctrl+N`, палитра); кнопка «+ Новая заметка» в сайдбаре;
  кнопка в **пустом состоянии дерева**, создающая `Welcome.md` с локализованным стартовым текстом (RU/EN)
  → открывает её. i18n RU/EN. Тест стора (уникальное имя + запись + рефреш). Frontend 90 тестов +
  coverage в пороге, tsc/eslint/build зелёные. (Визуал кнопок — простой; отзыв владельца приветствуется.)

- **#2 Гигиена дерева исходников.** Удалены 15 пустых теневых каталогов вида `<имя> 2`
  (`components 2/`, `db 2/`, `indexer 2/`…) — артефакты iCloud-синка, не отслеживались git, но
  загрязняли каждый `grep`/`find` (и, по выводам кросс-анализа, исказили сбор фактов линзами).
  `.gitignore` дополнен паттернами `* 2/` · `* 2.*` · `.nexus/`. Добавлен zero-dep `scripts/preflight.mjs`
  (скан теневых каталогов, `--fix` для пустых) — первый шаг любого среза. Только гигиена, код не трогался.
- **#6 Перф-PRAGMA БД.** В `configure_write`/`configure_read` (`db/mod.rs`) добавлены `mmap_size=256MB`
  (memory-map чтения), `cache_size` (64MB на writer / 16MB на каждый из 4 read-коннектов),
  `temp_store=MEMORY` — кратно ускоряет граф/беклинки/поиск/индексацию почти без кода. Поведение не
  меняется (5 db-тестов зелёные). **F32→F16-квантизация usearch — отложена в perf-эпик** (`#15→#6`),
  т.к. трогает recall и требует замера (no silent caps).

### Обзоры / процесс

- **Vision→AC сессия: Inline LLM в редакторе** (`docs/specs/inline-llm.md`). Первая из серии «vision→AC»
  (ревью §2/A2: у vision-фич нет AC + зашиты продуктовые решения → сперва спека, потом код). Vision-фича
  «Inline LLM» переведена в реализуемый контракт: **10 AC-IL** (Given/When/Then; триггер→стрим→ghost,
  Tab/Esc accept/reject + роутинг, отмена, ошибка-без-краша, один-стрим, a11y) + явное **«что тестируем
  (механика, детерминированно через мок) / что НЕ тестируем (качество вывода LLM — human-eval)»** (снимает
  риск «зелёных тестов на бессмысленный вывод»). **Продуктовые решения зафиксированы владельцем:** D1
  авто-ghost по паузе — ВЫКЛ по умолчанию (opt-in); D2 контекст = текущая заметка (RAG-грунтинг — отдельно);
  D3 логирование принятых/отклонённых — отложено (приватность). UX — DESIGN_BRIEF §4 (готов). Нарезка на
  4 среза IL-1..4. Реализация — отдельно; код не трогался.

- **Мульти-агентное ревью бэклога (2026-06-04).** 9 агентов (6 ревью-линз: архитектор · разработчик ·
  QA · аудитор автономности · безопасность · продукт + синтез + концепт тестов + дизайн-бриф), 61
  находка (6 blocker / 22 high / 9 противоречий) → `docs/reviews/BACKLOG_REVIEW.md` (§2 «Автономность
  разработки» — что не берётся автономным агентом и как разблокировать; §5 — рабочий список правок).
  - Концепт комплексного авто-тестирования (пирамида unit/integration/E2E + CI quality-gates +
    traceability «фича/AC ↔ тест» + coverage-ratchet) — `docs/dev/TESTING_STRATEGY.md`.
  - Дизайн-бриф для генератора UI (8 экранов: Home/настройки/чат-агент/inline-LLM/News-Feed/
    противоречия/карта-компетенций/reading-mode; токены Hermes) — `docs/design/DESIGN_BRIEF.md`.
  - BACKLOG: новая секция «🧱 Фундамент» (offline eval-гейт, планировщик джобов, frontmatter-parse,
    единый egress-контроль, AC-SEC-7 anti-injection, AC-SEC-6 redaction, CI security-job); плашка «не
    брать автономно» над vision-секцией; снят ярлык «(текущая)» с Ф1; auto-updater переописан
    (AC-DOD-Ф4, расщеплён на автономное ядро / ручную подпись-нотаризацию).

### Тестирование / CI (автономная очередь NIGHT-PLAN)

- **V1.1 — CI security-job** (ревью B6 / AC-Q-5). Отдельный job `security` в CI: supply-chain через
  `cargo-deny` (advisories RUSTSEC · лицензии · баны · источники — `deny.toml`) + secret-scan через
  `gitleaks` (`.gitleaks.toml` с allowlist плейсхолдеров доков/тестов). Проверки безопасности больше
  не тонут в общем `cargo test`. Лицензии выверены по фактическому дереву зависимостей (12 permissive,
  включая `CDLA-Permissive-2.0` для webpki-roots — данные CA-бандла Mozilla). `cargo-audit` покрыт
  advisories-срезом cargo-deny. Локально проверено: licenses/bans/sources/gitleaks — зелёные.
  - Отложено (no silent caps): выделенный прогон именно security-*тестов* (а не supply-chain) отдельным
    шагом требует конвенции тегирования тестов — записано в BACKLOG.
- **V1.2 — Coverage-ратчет** (TESTING_STRATEGY §6). Гейт покрытия «не ниже» на оба слоя:
  - **Frontend:** `@vitest/coverage-v8` + блок `coverage` в `vitest.config.ts` (provider v8, `all: true`
    по `src/**`); пороги lines/statements 63 · functions 60 · branches 75 (baseline 64.3/62.1/77.3%).
    CI-шаг `pnpm test:coverage`.
  - **Rust:** job `coverage-rust` (`cargo-llvm-cov --fail-under-lines 65`, baseline строк **71.8%**),
    параллельно rust-матрице — не добавляет wall-clock. Критичные модули сильны: parser 96.6%,
    permission 98%, broker 93%, search 85.5%, graph 95.8%, vault 92.6%.
  - Механика «тест на каждую новую функцию»: непокрытый код роняет % → CI краснеет.
  - Отложено (no silent caps): per-path пороги ≥70% критичных модулей, `coverage-baseline.json` +
    дельта-комментарий в PR, единый `scripts/test-all.sh` — BACKLOG «Coverage-доводка».
- **V1.3 — Traceability AC ↔ тест** (TESTING_STRATEGY §4). Матрица `docs/acceptance/traceability.json`:
  у КАЖДОГО из 77 AC (`ACCEPTANCE.md`) — статус (covered/partial/pending/manual/deferred) + ссылки на
  тесты. Гейт `scripts/check-traceability.mjs` (zero-dep, job `traceability`) падает, если: AC спеки нет
  в матрице (новый AC без записи о тесте), запись-сирота, неизвестный статус, или covered/partial без
  `tests[]`. Делает «тест на каждую фичу» проверяемым свойством сборки.
  - Стартовая картина (честная, по коду + ревью): **26 covered · 17 partial · 12 pending · 17 manual ·
    5 deferred** (43/77 с автотестами). pending-AC совпадают с очередью (Б9-1→V2.1, Б10-4→V2.4,
    SEC-6→V4.2, SEC-7→V4.3, EVAL-5, …) — матрица операционализирует автономность-находки ревью.
  - Отложено (no silent caps): рендер `traceability.md` для PR, проверка существования имён тестов в
    бинарях, конвенция `Closes AC-…` в PR-шаблоне — BACKLOG.

### Исправлено / безопасность (Волна 2 ночной очереди)

- **V2.2 — Rename-as-move: переименование сохраняет `file_id`** (AC-Б9-1 / ревью L6). Раньше move/rename
  приходил в индексатор как `Deleted(old)+Upsert(new)` → старый `file_id` умирал, новый путь получал
  свежий id, а беклинки/чанки на файл рвались молча. Теперь watcher склеивает пару From/To (по file-id
  через `notify-debouncer-full`) в одно событие `VaultEvent::Renamed{from,to}`, а `Indexer::rename_file`
  **переносит `files.path` с сохранением `file_id`**: входящие ссылки (беклинки) и чанки остаются
  привязаны к тому же id; ранее висячая `[[New]]` до-резолвится в файл; если rename совпал с правкой
  содержимого — финальный `index_file` обновит контент под тем же id (чистый rename → ранний выход).
  Краевые: rename в не-`.md` → удаление; перемещение из/в пределы vault → удаление/создание; замещение
  существующей цели — её строка/чанки убираются (UNIQUE(path) свободен). Тесты (offline): watcher
  `normalize` склейка/деградация перекрытого move; indexer — file_id+беклинки целы (`[[Old]]` по id,
  `[[New]]` до-резолвилась), чанки+векторы целы. Rust 117 зелёных. **Переписывание текста ссылок
  `[[Old]]`→`[[New]]` у источников — отдельная фича (BACKLOG, «no silent caps»).** Волна 2 закрыта.
- **«Вылет графа» РАЗГАДАН — это не краш** (диагностика с владельцем на реальном vault). Папка → клик
  на граф → «перезапуск приложения». Корень: **dev-only артефакт Vite** — граф лениво грузит
  `graphology`/`sigma`/`graphology-layout-forceatlas2`; при первом открытии Vite оптимизирует новую
  зависимость на лету и делает full-reload вебвью (`✨ new dependencies optimized … reloading`). В
  Rust-логе паники нет, процесс не рестартовал. Фикс: `optimizeDeps.include` граф-зависимостей в
  `vite.config.ts` (пребандл на старте dev). Прод-сборки не касалось. V2.3 (guard лимита SQLite-перем.)
  оставлен как полезное упрочнение.
- **V2.1 — Анти-SSRF для core-LLM-клиентов** (AC-SEC-4 / ревью C5). chat/embedding HTTP-клиенты ядра
  строятся через общий `ai::core_client_builder()` с `redirect(Policy::none())` — подменённый или
  скомпрометированный эндпоинт не уведёт запрос 30x-редиректом на внутренний/metadata-адрес. Закрывает
  core-половину AC-SEC-4 (плагинная `net.fetch` закрыта ранее). Тест `core_client_does_not_follow_redirects`
  (локальный 302-сервер на `std::net`, без новых зависимостей). `is_private_host` к ядру намеренно НЕ
  применяется — LLM-серверы локальные/LAN by design; consent на смену `base_url` из git-pull отнесён к
  «Единому egress-контролю ядра» (BACKLOG, Фундамент).
- **V2.3 — Граф: guard лимита SQLite-переменных** (ревью A9, защитно к багу вылета графа). `get_local_graph`
  и `get_full_graph` строили `IN (...)`-запросы с одним плейсхолдером на узел (и набор повторялся в
  `source AND/OR target`) — супер-хаб (узел с десятками тысяч связей) мог дать тысячи bind-переменных и
  словить `too many SQL variables`, уронив команду графа. Теперь все IN-запросы **чанкуются** через
  `collect_in_chunks` (≤900 на запрос; рёбра — одиночный `source IN (chunk)` + фильтр `target ∈ ids` в
  Rust вместо двойного IN). Результат полный, без обрезки. Тест `super_hub_does_not_exceed_sql_var_limit`
  (хаб с 1000 связей > чанка → много батчей; фикстура напрямую в БД через `WriteActor::transaction`).
  Снимает одну из 3 гипотез вылета графа; корень всё ещё ждёт артефакт владельца (BACKLOG).
- **V2.4 — Throttle рендера токенов чата** (AC-Б10-4 / ревью C9). Стор чата дописывал `content` на
  КАЖДЫЙ token-эвент → `set()` + ре-рендер на токен (O(токенов), кадровый бюджет под угрозой на 2000
  токенов). Теперь токены копятся в буфер и применяются одним `set()` на кадр через
  `requestAnimationFrame` (≤~60 ре-рендеров/сек). `done`/`error`/`stop` сбрасывают хвост буфера
  синхронно (токены не теряются). Тест `троттлит рендер токенов`: 200 токенов → **1** rAF-кадр (мок rAF
  считает вызовы), контент полный после кадра.

### Added — фундамент (ночная очередь / выбор владельца)

- **Граф v2a — физика на `d3-force`** (выбор владельца: «как в Obsidian»; их движок закрыт/проприетарен →
  переиспользовать нельзя, но d3-force — та же открытая основа). Ручная симуляция заменена на d3-force:
  `forceManyBody` (разлёт по площади), `forceLink` (пружины), `forceCenter` (мягкое центрирование),
  **`forceCollide`** (узлы не наезжают — убирает «мешанину»). **Drag через `fx/fy`**: тянем ноду — она
  пиннится к курсору, связанные подтягиваются с естественным сопротивлением (чем больше связей/инерции —
  тем больше сопротивление; `alphaTarget` разогревает на время drag). Рендер SVG + анимации (пульс/halo/
  kin/«поток») сохранены. Чистые помощники (подсветка/радиус) — `graph-sim.ts` (4 юнит-теста); раскладка —
  d3 + визуальная проверка человеком. **Часть «Граф v2»** — дальше: граф-во-вкладку, пан/зум-камера +
  авто-fit, панель настроек.
- **Граф v2d — панель настроек физики ⚙️** (отзыв владельца: «настройки узкой полосой и не
  регулируются» + цикл слепой подгонки сил → отдаём руль пользователю, как ⚙️ в Obsidian). Кнопка ⚙️
  в баре открывает плавающую панель со слайдерами **Отталкивание · Длина связей · Притяжение к центру ·
  Размер узлов**. Параметры применяются **вживую** без пересоздания симуляции (мутируем силы через refs +
  `alpha(0.5).restart()` → позиции узлов сохраняются) и хранятся в `localStorage` (`nexus.graph.settings.v1`).
  - **Доводка физики (каноничный фикс разлёта):** убран жёсткий `link.strength` (0.45) → d3 авто-масштабирует
    силу рёбер обратно степени (рёбра к хабам слабее) — именно это раздвигает хабы из центра; заряд теперь
    масштабируется по степени `−(repel + deg·30)`; `forceCenter` заменён на `forceX/forceY` к центру, чтобы
    «гравитация» стала регулируемым слайдером (выше = плотнее, ниже = разлёт). Drag-pin теперь **не навсегда**:
    новый захват ноды освобождает прежде закреплённые (как в Obsidian). Универсально «правильных» значений
    физики нет — поэтому их крутит сам пользователь под свой граф.
- **Граф: интерактив по дизайну** (хендофф `graph.jsx`). Граф переписан с sigma.js (WebGL, статичная
  раскладка) на **кастомный SVG force-directed** (по дизайну), что закрыло претензии владельца:
  - **drag узла** — перетаскиваемый пиннится к курсору, соседи подтягиваются пружинами (физика держится
    «тёплой» во время drag);
  - **hover** → подсветка узла+соседей+рёбер, остальное приглушается;
  - **активная нота** (открытый документ) → пульс-halo + ripple + дышащее кольцо + drop-shadow; её соседи
    («kin») — мягкие акцентные кольца; её рёбра — анимированный «поток»;
  - размер узла по степени, лейблы, режимы локальный (глубина 1–3, N-hop считает Rust) / единый (топ-600),
    счётчик узлов/рёбер, загрузка, баннер о неполноте.
  - Чистая математика (BFS/соседи/шаг-симуляции/радиус) вынесена в `graph-sim.ts` (**8 юнит-тестов**);
    визуал/drag — проверка человеком (view-слой `GraphView.tsx` исключён из coverage, как entry-point).
  - Удалены `sigma`/`graphology`/`graphology-layout-forceatlas2` + worker-раскладка (+ `optimizeDeps` от
    прошлого фикса — больше не нужен). **Отложено (no silent caps):** теги-цвета + фильтр-чипы (нужны теги
    на узлах из бэкенда) → отдельный срез; render-smoke-тест GraphView; перенос симуляции в Worker, если
    единый граф будет тормозить на огромных vault (сейчас main-thread rAF, узлов мало → ок).
- **V4.1 — Frontmatter `aliases` + резолв `[[Алиас]]`** (ревью H2, частично). Парсер извлекает алиасы
  из frontmatter (`aliases: [A, B]` инлайн · блочный список `- A` · скаляр `alias: A`) **минимальным
  line-парсером без YAML-либы** (serde_yaml архивирован → триггерил бы security-гейт; выбор владельца).
  Индексатор заполняет таблицу `aliases` (полная замена на файл; `OR REPLACE` на глобальном
  `UNIQUE(alias)`). Резолв ссылок `resolve_target`/`resolve_all_dangling` + обратный резолв расширены:
  `[[Алиас]]` находит файл и **forward**, и **backward** (out-of-order); путь имеет приоритет над
  алиасом. Тесты: парс 3 форм + резолв forward/backward + заполнение таблицы. Rust 113+9 зелёные.
  - Отложено (no silent caps): **полный typed-frontmatter** (`progress/draft/due/evergreen/goal` как
    структурированные поля) — упирается в выбор YAML-подхода (serde_yml / yaml-rust2), NEEDS-DECISION.
- **V4.4 — Общий чат без vault-грунтинга** (ревью правка 17, vision-critical). Раньше чат ВСЕГДА
  грунтился в vault (RAG) — расхождение с задумкой «ассистент». Теперь у чата два режима: **«По заметкам»**
  (как было — ретрив + источники) и **«Общий»** (ответ напрямую от модели, БЕЗ ретрива). Бэкенд: параметр
  `grounded` у `chat_rag` (дефолт `true`); при `false` — `hybrid_search` НЕ вызывается, источники пустые,
  промпт = новый `build_chat_messages` (system без vault-грунтинга). Фронт: переключатель-сегмент над
  композером (`ChatView`), `grounded`/`setGrounded` в сторе (на лету во время стрима не меняется),
  прокинут через `streamRag` + мок. Тесты (offline): Rust `build_chat_messages`; фронт — режим
  прокидывается в `streamRag`, общий режим → ответ без источников (ретрив не вызван), `setGrounded`
  заблокирован при стриме. Rust 127 + фронт 89 зелёные, coverage держит порог. i18n RU/EN.
  - Отложено (no silent caps): **web-search / tool-use** (LLM сам решает «нужен интернет» → поиск →
    ответ с цитатами) — требует ADR единого egress-контроля + self-hosted SearXNG (BLOCKED, решение
    владельца). Персист истории сессий (AC-Б10-2) — отдельно.
- **V4.3 — Анти-инъекция RAG-промпта (AC-SEC-7)** (ревью B2/A3; предусловие любых web-фич). Контент
  заметок попадает в LLM-промпт → заметка с «игнорируй инструкции» / поддельным `</note>` могла бы
  перехватить управление. Теперь каждый фрагмент контекста **обёрнут случайным маркером запроса**
  (`injection_marker` на `getrandom`, генерируется per-request → автор заметки его не знает и не может
  «закрыть» блок данных), а системная инструкция явно требует трактовать текст между маркерами как
  **ДАННЫЕ, а не инструкции**. `build_rag_messages` принимает маркер; команда `chat_rag` его генерирует.
  Тесты (offline): фрагменты обёрнуты маркером (≥2 раза), инъекция-текст лежит как данные внутри,
  система предупреждена; маркер случаен (две генерации различаются). Rust 120 зелёных.
  - **Вторая половина AC-SEC-7 («валидация JSON-ответа suggest») — N/A by-construction:** `suggest`
    (Ф1-9) считается из векторов usearch (max-sim), **LLM/JSON не использует** → инъекцией не
    управляем в принципе. Строгая JSON-валидация появится, ЕСЛИ/когда добавится LLM-suggest (BACKLOG).
- **V4.5 — Offline eval-гейт логики ранжирования (AC-EVAL-3/AC-Q-4)** (ревью A1; «делать раньше всех»).
  Раньше регресс-гейт качества был только ЖИВЫМ (`#[ignore]`, нужен embedding-сервер :8083) → CI зелёный
  без проверки ранжирования, RRF/метрики трогать боязно. Добавлен **детерминированный офлайн-тест**
  `offline_eval_gate_on_fixed_vectors` (обычный `cargo test`, без сервера): `FixedEmbedder` отдаёт
  **фиксированные синтетические векторы** (cosine-оси apple/banana/cherry), запросы находят релевантные по
  **векторной близости** (токены запроса не встречаются в телах → FTS пуст), прогон через настоящий
  `hybrid_search`→RRF→`run_eval` даёт точно посчитанные вручную метрики (recall@8=1.0, MRR=5/6,
  nDCG≈0.877; кейс QRYMIX: cherry@1 cos0.8 > apple@2 cos0.6 → RR=0.5). Регрессия RRF-слияния/метрик
  ломает тест в CI. Rust 121 зелёных. **AC-EVAL-3/AC-Q-4 → partial** (плумбинг автоматизирован).
  - Отложено (no silent caps): **гейт на РЕАЛЬНОМ качестве** (golden-фикстура реальных эмбеддингов
    bge-m3) — нужен живой :8083 один раз → BLOCKED, разовый шаг владельца.
- **V4.2 — Redaction-layer (AC-SEC-6)** (ревью H18). Чтобы контент заметок/пути не утекали в логи и
  crash-отчёты по неосторожности: новый тип **`Redacted<T>`** (модуль `redact`) — `Debug`/`Display`
  печатают `<redacted>`, значение достаётся только явно через `expose()` (видно на ревью). **Crash-scrub
  усилен**: помимо HOME→`~` теперь сворачивает и абсолютные пути ВНЕ дома (vault на другом диске/маунте)
  в `<path>/<basename>` — структура каталогов скрыта, имя файла оставлено для диагностики; относительные
  и `~/…` пути не трогаются. Аудит tracing: **ядро контент заметок НЕ логирует** (проверено) → `Redacted` —
  страховка от регрессий + инструмент для будущих web/import-фич. Отчёт локальный, отправка — opt-in.
  Тесты: `Redacted` скрывает значение в Debug/Display/интерполяции (`expose` возвращает оригинал);
  crash-scrub прячет пути вне дома, не трогает относительные/`~`. Rust 125 зелёных. **AC-SEC-6 → covered.**
  - Отложено (no silent caps): широкое оборачивание Debug-полей структур с контентом в `Redacted` —
    инкрементально, когда такие поля начнут попадать в логируемые пути (сейчас не попадают).
- **Typed-frontmatter — плоские поля** (ревью H2; продолжение V4.1, NEEDS-DECISION по YAML закрыт).
  Выбор владельца: **расширить мини-парсер** (без YAML-либы — `serde_yaml` архивирован → триггерил бы
  security-гейт). Парсер извлекает плоские скаляры верхнего уровня frontmatter (`progress/due/goal/
  evergreen/draft` и пр.) как `(key, value)` (кавычки сняты, ключи уникальны, порядок как в файле);
  инлайн-списки (`[…]`), вложенный YAML и блок-списки НЕ берутся (для них — свои таблицы/сырой
  `frontmatter`). Новая таблица **`frontmatter_fields`** (миграция 003, `UNIQUE(file_id,key)` +
  индекс по `key`) — индексатор наполняет её на каждый файл (полная замена, как теги/алиасы). Это
  разблокирует **кросс-файловые запросы** (цели/прогресс, stale-radar, Dataview, умные шаблоны).
  Тесты: парсер (только плоские скаляры, дубль→последний, кавычки/списки/вложенность) + индексатор
  (запись + замена при реиндексе) + миграция (таблица создаётся). Rust 128 зелёных.
  - Отложено (no silent caps): **типизация значений** (сейчас все значения — строки; даты/числа/bool
    парсят консьюмеры) + **query-API/команда** для кросс-файловых выборок — придут с первым
    консьюмером (Прогресс целей / Dataview); сложный вложенный YAML — fallback на сырой `frontmatter`.

### Added — Фаза 0

- **Ф0-1 — Каркас (monorepo + Tauri 2 + CI).**
  - pnpm-workspace + Cargo workspace по §2 ARCHITECTURE: `apps/desktop/{src, src-tauri}`,
    заготовки `packages/`, `plugins/`, `scripts/`.
  - Tauri 2-приложение `nexus-desktop` с первой сквозной IPC-командой `app_version`;
    единый IPC-шов фронта `src/lib/tauri-api.ts` (контракт §4.1) — весь `invoke` только здесь.
  - Фронт: React 19 + Vite 6 + TypeScript (strict); базовые design-токены (DESIGN §2, light/dark).
  - Тулчейн качества: `tsc --noEmit`, ESLint 9 (flat config), Vitest 3 (+ Testing Library),
    `cargo fmt` / `clippy -D warnings` / `cargo test`.
  - CI (GitHub Actions): job `frontend` (typecheck · lint · test · build) и job `rust`
    (matrix Win/Mac/Linux: fmt · build · clippy · test).
  - Placeholder app-иконки: `scripts/gen-icon.mjs` → `cargo tauri icon` (полный платформенный набор).
  - Стартовый CSP + минимальные capabilities (`core:default`); строгий аудит — в Ф0-12 (AC-SEC-5).

  Закрытые гейты: **AC-Q-1**, **AC-Q-2**, **AC-Q-3** (зелёные сборка/тесты/линтеры).

- **Ф0-2 — БД-слой (rusqlite + write-actor).**
  - `Database` (`src-tauri/src/db`): единственный поток-писатель `WriteActor` (синхронные
    транзакции, ADR-003) + пул read-коннектов `ReadPool` (WAL, `spawn_blocking`).
  - Раннер миграций: версионированные SQL (`include_str!`), версия в `PRAGMA user_version`
    (транзакционно, идемпотентно, резюмируемо). Схема v1: `files/links/tags/file_tags/aliases/settings`
    + индексы (ARCHITECTURE §5).
  - Тесты (на temp-файле, реальный WAL): атомарный rollback, конкурентные записи без `SQLITE_BUSY`,
    идемпотентность миграций, чтение во время записи.
  - Модульная дока: `docs/dev/db.md`.

  Закрытые гейты: **AC-Б7-1**, **AC-Б7-2**, **AC-PR-3**.

- **Ф0-3 — Vault + ленивое дерево файлов.**
  - Rust `vault`: `resolve_vault_path` (единая канонизация/анти-traversal — задел AC-SEC-1),
    ленивый `list_dir` (содержимое одного каталога, скрытие dotfiles/`.conflict`); команды
    `open_vault`/`list_dir`; managed state `AppState { vault }`; плагин `tauri-plugin-dialog`.
  - Фронт: IPC-шов расширен (`vault.*`) + мок-бэкенд для превью; Zustand-стор vault;
    виртуализированное дерево (`@tanstack/react-virtual`, flatten видимых узлов) с клавиатурной
    навигацией (`aria-activedescendant`); layout sidebar + main; иконки Lucide.
  - Тесты: Rust (листинг/ленивость/traversal), фронт (стор + FileTree). Дока: `docs/dev/vault.md`.

  Закрытые гейты: **AC-SEC-1** (vault-команды), задел **AC-PERF-7** (виртуализация).

- **Ф0-4 — Watcher + парсер + инкрементальная индексация.**
  - `parser` (pulldown-cmark): title, сырой frontmatter, ссылки (`[[wiki]]`/`![[embed]]`/markdown),
    `#tags`, word_count; матчи в коде исключаются.
  - `watcher` (notify-debouncer-full, 400 мс): `is_ignored` (`.nexus`/`.git`/`*.db*`/dotfiles),
    нормализация событий по пути (remove+create → один Upsert; шторм схлопывается).
  - `indexer`: UPSERT `files` по path (сохраняет `file_id` при atomic-save), полная замена
    `links`/`tags`, прямой+обратный резолв целей; soft-delete; начальный скан; обвязка
    watcher→index в `open_vault`.
  - Тесты: parser (5), watcher (3), indexer (3) — atomic-save/file_id+беклинки, обратный резолв,
    теги. Дока: `docs/dev/indexer.md`.

  Закрытые гейты: **AC-Б9-1**, **AC-Б9-2**, **AC-Б9-3**.

- **Ф0-5 — Редактор CodeMirror 6 (source-mode).**
  - CM6: markdown-подсветка, декорации `[[wikilink]]`/`![[embed]]`/`#tag` (токены цвета),
    клик по wikilink → навигация, автокомплит имён заметок внутри `[[…`.
  - Контракт CM6↔React: `EditorView` один раз; смена файла — `dispatch` (без пересоздания),
    помеченный аннотацией `externalSync` (нет ложного dirty); guard StrictMode; save по `Mod-s`.
  - Rust-команды `read_file`/`write_file` (write-safe canonicalize) + `list_notes`.
  - Стор vault: активный файл, dirty, заметки; `openFile`/`openLink`/`saveActiveFile`.
  - Тесты: 17 фронт (extensions/Editor+регресс/стор/FileTree), Rust 20. Дока: `docs/dev/editor.md`.

  Часть **AC-DOD-Ф0** (source-mode редактор, `[[wikilink]]` клик/автокомплит).

- **Ф0-6 — Беклинки из SQLite + backlinks-бар.**
  - Rust `graph::get_backlinks` (ADR-004): запрос по `idx_links_target` (без petgraph),
    `BacklinkEntry{sourcePath,sourceTitle,context,lineNumber}`; команда `get_backlinks`.
  - Фронт: `BacklinksBar` (слот editor-bottom) с loading/empty/списком, клик → переход к источнику.
  - Тесты: Rust (беклинки A,C→B + контекст + пусто), фронт (бар + пустое состояние). Дока: `docs/dev/graph.md`.

  Закрывает беклинки части **AC-DOD-Ф0** (беклинки из SQLite).

- **Ф0-7 — Поиск (title/path/tags).**
  - Rust `search::search_notes` (LIKE по path/title/tags, экранирование, LIMIT 100);
    команда `search_vault`. Допущение Ф0: метаданные; полнотекст по телу — Ф1 (FTS5 поверх chunks).
  - Фронт: `Sidebar` с полем поиска (debounce 150 мс) — дерево / результаты, клик → открыть.
  - Тесты: Rust (path/title/tag/пусто), фронт (дерево↔результаты/тег/пусто/очистка). Дока: `docs/dev/search.md`.

  Поиск части **AC-DOD-Ф0** (FTS-допущение зафиксировано).

- **Ф0-8 — Command Registry + Palette + keymap.**
  - Реестр `commands` (§4.6): register/run/dispose/subscribe; `Command{id,title,source,defaultKey,run}`;
    `resolve` с приоритетом пользователь>плагин>ядро; `normalizeCombo`/`eventToCombo`/`formatCombo`.
  - `CommandPalette` (Cmd/Ctrl+P): фильтр, ↑/↓/Enter/Esc, клик; `useKeymap` (window keydown → команда).
  - Команды ядра: `palette.open`/`vault.open`/`file.save`; `useUIStore`.
  - Тесты: реестр (приоритет/combo/dispose) + палитра (открытие/фильтр/Enter/Esc). Дока: `docs/dev/commands.md`.

  Закрывает command-registry часть **AC-DOD-Ф0** (база для плагинного registerCommand).

- **Ф0-9 — Workspace: вкладки/сплиты (Б12).**
  - `useWorkspaceStore`: буферы (один на путь), группы/вкладки, активная группа; openFile/openLink/
    setActiveTab/setActiveGroup/closeTab(+GC)/splitRight/updateBufferDoc/saveBuffer/reset; селекторы
    `activeBuffer`/`activePath`. Контекст AI/backlinks — из активной вкладки активной группы.
  - UI: `EditorArea` (сплиты в ряд) + `GroupPane` (вкладки + split + Editor[key=group] + BacklinksBar[path]).
  - Рефактор: vault-стор → только дерево/заметки; `BacklinksBar` принимает `path`; команды `file.save`/
    `view.splitRight`; открытие vault сбрасывает workspace.
  - Тесты: workspace (dirty при переключении — Б12-2; split+контекст — Б12-1; close/GC/openLink) + правки
    FileTree/Sidebar/App/BacklinksBar. Дока: `docs/dev/workspace.md`.

  Закрывает **AC-Б12-1**, **AC-Б12-2**.

- **Ф0-10 — i18n RU/EN.**
  - i18next + react-i18next; ru/en ресурсы; детекция локали (navigator.language), `changeLocale`
    с сохранением выбора; переключатель языка в шапке.
  - Плюралы `_one/_few/_many` (ru); `Intl.NumberFormat` (`formatNumber`); `Intl.Collator`
    (`compareEntries` — сортировка дерева: каталоги выше, кириллица).
  - Все UI-строки переведены в ключи (App/Sidebar/FileTree/Editor area/BacklinksBar/CommandPalette);
    команды через `titleKey`.
  - Тесты: AC-I18N-1 (паритет ключей), AC-I18N-2 (ru-плюралы), AC-I18N-3 (Intl-числа),
    AC-I18N-4 (Collator), AC-I18N-5 (детекция/смена). Дока: `docs/dev/i18n.md`.

  Закрывает **AC-I18N-1…5** (бэкенд-i18n AC-I18N-6 и плагины AC-I18N-7 — позже).

- **Ф0-11 — Граф (базовый).**
  - Rust `graph::get_local_graph` (BFS N-hop из SQLite, ADR-004); команда `get_local_graph`.
  - Фронт: `GraphView` (sigma.js + graphology, ленивый chunk §10); раскладка ForceAtlas2 в
    **Web Worker** (`layout.worker.ts`, AC-PERF-6); клик по узлу → открыть; команда `view.graph` (Cmd/Ctrl+G).
  - Тесты: Rust (N-hop по глубине, пустой центр), фронт (`computeLayout`, мок графа). Дока: `docs/dev/graph.md`.

  Закрывает граф-часть **AC-DOD-Ф0**; layout в Worker — **AC-PERF-6**.

- **Ф0-12 — Безопасность каркаса (CSP + capabilities).**
  - Строгий CSP без `unsafe-inline`/`unsafe-eval` (+ `object-src 'none'`, `base-uri 'self'`,
    `frame-ancestors 'none'`, `worker-src`).
  - Минимальные capabilities: `core:default` + `dialog:default`; нет `fs:`/`shell:`/`http:` —
    vault-доступ через собственные команды (`resolve_vault_path`).
  - Регресс-тест `csp_and_capabilities_are_hardened`. Дока: `docs/dev/security.md`.

  Закрывает каркасную часть **AC-SEC-5** (broker/iframe-изоляция — Ф2; рантайм-CSP — на упаковке).

- **Ф0-13 — Plugin loader (минимум).**
  - `plugin`: `ApiVersion`/`parse`, `PluginManifest`, `check_compatibility` (С-13: `min_api_version` —
    минимум ядра; `^1.0` отвергается), `load_manifest`, `scan_plugins` (`.nexus/plugins/*`);
    команда `list_plugins`. Без broker/исполнения (Ф2).
  - Тесты: совместимость/`TooNew`/`TooOld`/каретка-`BadVersion`/битый json/scan. Дока: `docs/dev/plugins.md`.

  Закрывает каркас плагинов части **AC-DOD-Ф0** (С-13).

### Added — Фаза 1 (AI Core)

- **Ф1-1 — Схема v2: chunks + FTS5 + триггеры.**
  - Миграция `002_chunks_fts.sql`: таблица `chunks` (+`idx_chunks_file`) + `fts_chunks` (FTS5
    external-content поверх `chunks.content`) + триггеры синхронизации `chunks_ai/ad/au` (§5).
  - Тест `fts_chunks_synced_via_triggers` (AC-Б8-1/8-2): текст находится сразу после вставки,
    исчезает после удаления чанка. Дока: `docs/dev/db.md` (schema v2).

  Закрывает **AC-Б8-1/8-2** (FTS-синхронизация), готовит почву под чанкер/эмбеддинги.

- **Ф1-2 — Чанкер (markdown-aware).**
  - `chunk_document`: frontmatter вырезан; разбиение по ATX-заголовкам (heading_path); sliding window
    с overlap ВНУТРИ окна (по словам), fenced-code атомарен; `char_start/end` — в исходном файле;
    `token_count` по тексту чанка. `Tokenizer` (placeholder `WordTokenizer`; реальный — Ф1-3).
  - Тесты: короткий/frontmatter/заголовки/overlap/код-атомарен. Дока: `docs/dev/chunker.md`.

  Готовит **AC-Б4-1** (эмбеддинг по чанкам — замкнётся в Ф1-5).

- **Ф1-3 — EmbeddingProvider + HTTP-клиент (ADR-005).**
  - `ai`: трейт `EmbeddingProvider` (embed_documents/embed_query, dim, model_id); `OpenAiEmbedder`
    (`/v1/embeddings`, task-префиксы nomic, L2-нормализация, проверка размерности); `MockEmbedder`
    (тесты без сервера); `LocalConfig` (`.nexus/local.json`: chat/embedding раздельно).
  - Зависимости: `reqwest` (rustls), `async-trait`. Сервер: nomic-embed-text :8081 (dim 768).
  - Тесты: l2/мок/конфиг + **живой smoke nomic** (`#[ignore]`) — dim 768, семантический ранкинг ✓.
    Зафиксирован риск ADR-005 (nomic англоцентричен; мультиязычный bge-m3/e5 — позже, §6.5). Дока: `docs/dev/ai.md`.

  Embedding-провайдер для RAG; chat — Ф1-7.

- **Ф1-4 — usearch ANN-индекс.**
  - `vector::VectorIndex` (usearch HNSW, Cos, sibling-файл `.nexus/vectors.usearch`): `open(path,dim)`
    (dim из эмбеддера), `upsert` (ключ=chunk_id, замена без дублей), `remove`, `search` → `VectorHit`,
    `save`/`len`/`contains`. Зависимость `usearch`.
  - Тесты: upsert+search+no-dup (AC-Б4-2), отказ при иной размерности (AC-Б5-1), remove чистит выдачу
    (AC-Б8-2), персистентность. Дока: `docs/dev/vector.md`.

  Закрывает (на уровне индекса) **AC-Б4-2 / AC-Б5-1 / AC-Б8-2**; интеграция в индексатор — Ф1-5.

- **Ф1-5 — Индексация с эмбеддингами (сборка RAG-индекса).**
  - `indexer`: на каждый `.md` (при включённом RAG) чанкинг → эмбеддинг батчами под семафором →
    в ОДНОЙ write-транзакции с file/links/tags полная замена `chunks` (+FTS5 триггерами) → после
    коммита usearch `remove` старых + `upsert(chunk_id, vec)` новых (1:1, без осиротевших векторов).
  - `Indexer::with_rag` (эмбеддер + `VectorIndex` + флаг `force`) vs `Indexer::new` (без AI);
    `spawn(indexer)` теперь принимает готовый индексатор. `remove_file` чистит chunks+FTS+векторы.
  - **Переэмбеддизация при смене модели (§6.5):** `embedding.model`/`dim` в `settings`;
    `reconcile_embedding_model` в `open_vault` при расхождении чистит chunks+файл векторов и поднимает
    `force` → полный перескан игнорирует mtime-шорткат. `dim` из конфига или `probe_dim` (не хардкод).
  - `open_vault` строит RAG из `.nexus/local.json`; нет конфига/сервер недоступен → vault без AI
    (local-first). `VaultContext.vectors` делится с поиском (Ф1-6). Прогресс/чекпойнт usearch в скане.
  - Добавлено: `DbError::External`, `OpenAiEmbedder::probe_dim`, `ai::default_prefixes` (nomic/e5).
  - Тесты (`MockEmbedder`): запись chunks+FTS+векторов, реиндексация без дублей, `remove`-чистка,
    `force`-перескан; реконсиляция модели. **Живой** end-to-end на nomic :8081 — семантический
    поиск находит нужный чанк. Дока: `docs/dev/indexer.md`.

  Закрывает **AC-Б4-1 / AC-Б8-1**; на уровне индексации — **AC-Б4-2 / AC-Б5-2 / AC-Б8-2 / AC-PERF-5**.

- **Ф1-6 — Hybrid search + RRF (§6.2).**
  - `search::hybrid_search`: вектор (usearch, семантика) **+** FTS5/BM25 (`fts_chunks`, лексика) → две
    независимые выдачи кандидатов (по 50) → **Reciprocal Rank Fusion** (`rrf_fuse`, k=60) → топ-`limit`
    с резолвом метаданных и сниппетом. Сливаем РАНГИ, не «сырые» score (cos vs BM25 — разные шкалы).
  - `fts_query`: санитизация ввода в MATCH (токены в кавычках через OR, юникод/кириллица; нет инъекции
    FTS-синтаксиса). Изящная деградация: нет эмбеддера → только FTS; пусто/без совпадений → пусто.
  - Команда `search_content(query, limit?)`; `VaultContext.embedder` (эмбеддинг запроса вне лока пула).
    `SearchHit` (camelCase). Контракт фронта `tauriApi.search.searchContent` + мок `mock/vault.ts`.
  - `rrf_fuse` принимает N списков → граф как **3-й ранг** (§6.2, REVIEW С-4: БЕЗ аддитивного `+0.2`)
    добавится третьим списком там, где есть центр-файл (чат/suggest, Ф1-7+).
  - Тесты: `rrf_fuse`, `fts_query`, FTS-only, сортировка+резолв, пустые случаи; **живой** на nomic :8081
    (запрос без лексических пересечений → семантический топ из вектора). Фронт: тест мока. Дока: `docs/dev/search.md`.

  Закрывает **AC-Б6-1** на уровне механизма (семантика через usearch HNSW, не линейный скан; перф на
  500k — AC-PERF-3 позже). НЕ закрывает **AC-Б6-2** (префильтр метаданных ДО KNN) — follow-up вместе с
  граф-рангом, dedup overlap-чанков и реранкером (jina :8082).

- **Ф1-7 — Chat-провайдер + стриминг (ADR-005, §4.1/§4.3).**
  - `ai::ChatProvider` (`stream_chat` с колбэком токенов + флагом отмены) и `OpenAiChatProvider`
    (`/v1/chat/completions`, `stream:true`, SSE через `Response::chunk()` — без новых зависимостей;
    парсер `parse_sse_delta`, `[DONE]`). `build_rag_messages` (system: только по контексту, цитаты [n],
    язык вопроса; пронумерованный контекст). `ChatMessage`.
  - Команда `chat_rag(channel, question, k?)`: поток `ChatStreamEvent` в Tauri `Channel` (§4.1) —
    `Sources` (гибрид-поиск Ф1-6) → `Token`… → `Done`/`Error`. Контекст = полное содержимое топ-k
    чанков (`search::fetch_chunk_contexts`). Лок vault снят до сетевых вызовов. Отмена — `chat_cancel`
    + `AppState::begin_chat` (один активный чат, новый стрим отменяет прежний).
  - `VaultContext.chat` (`build_chat` из `local.json → ai.chat`). Фронт: `ChatStreamEvent`,
    `tauriApi.chat.streamRag → cancelFn`, мок `streamChat`.
  - Тесты: `parse_sse_delta`, `build_rag_messages`; **живой** стрим Qwen :8080 (токены, «Париж»);
    фронт — мок streamChat (порядок событий, отмена). Дока: `docs/dev/chat.md`.

  Закрывает **AC-Б10** (стриминг через Channel + финализация в `Done` + отмена). UI чата — Ф1-8.

- **Ф1-6 доработка — префильтр (AC-Б6-2) + граф-ранг + dedup overlap (§6.2).**
  - **AC-Б6-2 (префильтр ДО KNN):** `SearchFilter { folder, tag }` → `allowed_chunk_ids`; вектор-ветвь
    через usearch `filtered_search` (фильтр ВНУТРИ обхода HNSW — `VectorIndex::search_filtered`, не
    пост-фильтр), FTS-ветвь через `JOIN files`, граф-ветвь — пересечением. Закрывает AC-Б6-2.
  - **Граф — 3-й ранг RRF (§6.2, REVIEW С-4):** `center` (открытый файл) → BFS по `links` (`GRAPH_HOPS=2`)
    → чанки соседей по (хоп, `chunk_index`) третьим списком в `rrf_fuse` — **в шкале RRF, БЕЗ `+0.2`**.
  - **Dedup overlap:** пере-выбор `limit×OVERFETCH` → схлоп соседних чанков одного файла (|Δindex|≤1).
  - `SearchOptions`; `search_content(query, limit?, folder?, tag?, center?)`; `chat_rag` передаёт `center`.
    Фронт: `searchContent(query, {limit,folder,tag,center})`, `streamRag(.., {center})`, мок учитывает `folder`.
  - Тесты: префильтр по папке, граф-ранг (изолированно), dedup overlap (+ живые зелёные). 63 Rust + 4 живых.

  Закрывает **AC-Б6-2**; граф-ранг — пункт DoD-Ф1 «hybrid+RRF без +0.2». Остаётся (осознанно): реранкер
  (опц., ADR-005, под eval-гейтом AC-EVAL-3 после Ф1-10), фильтр по дате, калибровка весов на eval.

- **Ф1-8 — Чат-UI (RAG, DESIGN §«AI Chat»).**
  - `stores/chat.ts` (`useChatStore`): сессия-лента `ChatMessage[]`, `send`/`stop`/`clear`; стрим через
    `tauriApi.chat.streamRag` (`sources`→`token`…→`done`/`error`), один стрим за раз, отмена.
  - `components/chat/ChatPanel.tsx` (+CSS-модуль): правая панель — пустое состояние, лента user/assistant,
    каретка стрима, **Стоп**/**Отправить** (Enter/Shift+Enter), кликабельные источники → `openFile`,
    бейдж «локально». Контекст retrieval = открытый файл (`activePath` → `center`, граф-ранг).
  - Интеграция: `ui.chatOpen` + команда `view.chat` (`mod+j`) + кнопка в шапке + 3-я колонка layout;
    i18n namespace `chat` (RU/EN). Удалены дубли доков (отдельный коммит).
  - Тесты: стор (стрим→ответ+источники, stop/clear/пустой ввод) и панель (пустое состояние, рендер
    ответа + клик источника → `openFile`, Enter-отправка, disabled). Фронт **57 тестов**.
    **Проверено в превью**: вопрос → стрим + источники → клик открывает файл. Дока: `docs/dev/chat.md`.

  Закрывает **AC-DOD-Ф1** (видимый поток «вопрос → ответ с источниками»). Виртуализация ленты,
  индикатор облака, персист сессий — в `docs/BACKLOG.md`.

- **Ф1-9 — Предложения связей (режим 1, max-sim).**
  - `suggest::get_link_suggestions`: на лету из готовых usearch-векторов (без embedder-сервера) — соседи
    каждого чанка файла → агрегация по целевому файлу по МАКСИМУМУ similarity → исключение уже связанных
    и самого файла → порог `MIN_SCORE` → топ-`limit`. `VectorIndex::get_vector`. `LinkSuggestion`.
    Команда `get_link_suggestions(path, limit?)`. Режим 1 — тихий (REVIEW С-8: на save LLM не дёргаем).
  - Фронт: `AiPanel` с вкладками **Чат**/**Связи** (рефактор правой панели; `ChatView`+`SuggestView`),
    `stores/suggest` (load/«пересчитать», dismiss-сессия, accept → дописывает `[[wikilink]]` в буфер),
    карточки score%/причина/Добавить/Скрыть. Команда `view.suggest`; i18n; `tauriApi.suggest.forFile`+мок.
  - **Фикс Ф0-5:** `Editor` теперь синкает внешнее изменение того же файла (accept/watcher), не только
    смену файла — `externalSync`, без ложного dirty; + регресс-тест.
  - Тесты: suggest (max-sim / исключение связанных / пусто) + **живой** nomic (топ — близкая заметка);
    стор+`SuggestView`+`Editor`-регресс. Фронт **64 теста**, Rust **+4** (incl. живой). Дока `docs/dev/suggest.md`.

  Закрывает Ф1-9 (suggest режим 1, max-sim — пункт AC-DOD-Ф1). Режим 2 (LLM), кэш `link_suggestions`,
  персист dismiss, калибровка порога — в `docs/BACKLOG.md`.

- **Ф1-10 — Eval-харнесс качества RAG (§6.6, AC-EVAL-1..6).**
  - `eval/golden.json` — корпус (RU/EN) + кейсы `query→файлы`, включая **кросс-язычные** (AC-EVAL-6);
    `eval/baseline.json` — пороги + условия (модель/сервер/k/набор, AC-EVAL-4).
  - `eval::{recall_at_k, reciprocal_rank, ndcg_at_k}` (чистые) + `run_eval` (через `hybrid_search`,
    файловая релевантность) + `EvalReport`/`CaseResult` + `index_corpus` + `load_golden/baseline`.
  - Раннер-гейт: `#[ignore]`-тест `live_eval_meets_baseline` — печатает отчёт и падает при метриках ниже
    baseline (**AC-EVAL-3**). Запуск: `cargo test live_eval_meets_baseline -- --ignored --nocapture`.
  - **Фактический baseline** (nomic @ :8081, k=8, 10 кейсов): recall@8 = nDCG@8 = MRR = **0.800**; 8/10
    идеальны, **2 промаха — кросс-язычные** → количественно подтверждён риск ADR-005 (AC-EVAL-6 ждёт
    мультиязычный эмбеддер). Тесты: математика метрик + парс + e2e на mock + живой ≥ baseline. Дока `docs/dev/eval.md`.

  Закрывает **AC-EVAL-1..5** (golden, метрики, baseline-гейт, условия в отчёте; suggest-порог per-model).
  **AC-EVAL-6** измерен и зафиксирован как недостигнутый на nomic (нужен мультиязычный эмбеддер — BACKLOG).
  **🏁 Фаза 1 (AI Core) завершена** — RAG end-to-end + видимый UI + suggest + измеримое качество.

### Added — после Фазы 1 (надёжность/доводка)

- **Crash-reconcile usearch (§5.1).** `indexer::reconcile_vectors` (в конце `scan_vault`): чанки, что
  есть в БД, но чьих векторов нет в usearch (крах между commit и `save`), переэмбеддятся батчами и
  доливаются; на force-скане no-op; best-effort при недоступном эмбеддере. Тест восстановления
  потерянного вектора. Закрывает рассинхрон, обещанный в `docs/dev/vector.md`.
- **condition-driven eval** (подготовка к Ф1-12): live-прогон читает модель/сервер/dim из
  `baseline.json` (`Conditions`) — AC-EVAL-4, прогон в зафиксированных условиях.

- **Ф1-12 — мультиязычный эмбеддер bge-m3 (закрыт AC-EVAL-6).** Подключён **bge-m3 Q4_K_M @ :8083**
  (dim 1024, мультиязычный) как основной RAG-эмбеддер вместо англоцентричного nomic. Переключение —
  через переэмбеддизацию (§6.5, dim 768→1024, код был готов с Ф1-5). `default_prefixes("bge-m3")` → без
  префиксов. Добавлен в `start_servers.sh` (:8083, персистентно).
  - **Eval на bge-m3: recall@8 = 1.000, nDCG@8 = 0.883, MRR = 0.848** (было 0.800/0.800/0.800 на nomic).
    Оба кросс-язычных кейса (EN→RU, RU→EN) теперь в recall@8 → **AC-EVAL-6 закрыт**; baseline поднят и
    перепроверен живым прогоном. Риск ADR-005 (англоцентричность) снят.
  - Доки: `ai.md`/`eval.md` обновлены; `docs/BACKLOG.md` — мультиязычный эмбеддер + AC-EVAL-6 в «Закрыто».

### Added — Фаза 2 (плагины / broker)

- **Ф2-1 — Модель прав плагина (capability-broker, security-ядро; ADR-002, §7.2/§7.4/§7.9).**
  - `plugin/permission.rs`: `Permissions` из `manifest.permissions` (vault:read/write — path-glob со
    scoped-правами; ai:embed; ai:complete `true`/`{local_only}`; net-allowlist; ui-точки). Манифест
    расширен полем `permissions` (отсутствие = deny-all, **fail-closed**).
  - `Permissions::check(ApiRequest) -> Result<(), Denied>` = §7.4 `check_scoped_permission`: метод→право,
    **path-scoped** (`path_in_scope`, `!`-deny перекрывает allow), анти-traversal в глубину
    (`..`/abs/`\`/пустой сегмент → `PathEscape`), net-allowlist, неизвестный метод → `UnknownMethod`.
    Сегментный `glob_match` (`**` 0..N сегментов, `*` внутри сегмента). Identity/токены — рантайм по порту (Ф2-2).
  - 13 security-тестов (glob, deny-override в любом порядке, read≠write, path-escape, ai/local_only,
    net, fail-closed). Rust 85 тестов зелёные. Дока `docs/dev/plugins.md`.

  Фундамент **AC-SEC-*** (path-scoped права, fail-closed). Рантайм-брокер (порты/токены/audit/iframe,
  исполнение JS/WASM) — Ф2-2+.

- **Ф2-2a — Capability-broker, host-сторона (§7.4).** `plugin/broker.rs`: `PluginBroker { sessions:
  HashMap<PortId, PluginSession>, audit }` — **identity по порту** (не из payload → закрывает
  confused-deputy/capability-laundering), `authorize(port, req)` = порт→сессия → `Permissions::check`
  → запись в **неотключаемый `AuditLog`** (и успех, и отказ), `revoke` (мгновенная ревокация),
  `handle(.., &mut dyn HostDispatch)` = authorize→dispatch. Реальный I/O — за трейтом `HostDispatch`
  (Ф2-2b). 6 тестов (unknown-port deny+audit, scope, confused-deputy по порту, ревокация, handle).
  Rust 91 тест. Дока `docs/dev/plugins.md`.

  Транспорт MessagePort/iframe + capability-токены + реальный dispatch — Ф2-2b (нужна фронт-сторона).

- **Ф2-2b (часть 1) — Capability-токены (§7.9).** Брокер переведён с порт-идентичности на
  **token-identity** (IPC-эквивалент порта): `open_session(session) -> CapToken` (32 случайных байта
  hex, `getrandom`, неугадываем), `authorize(&token, req)`, `revoke(&token)` (мгновенная инвалидация).
  Токен — источник identity на границе фронт↔Rust (порт-релей — на фронте). Закрывает confused-deputy
  по токену. Зависимость `getrandom`. 7 тестов брокера (уникальность токенов, ревокация, identity).
  Rust 92 теста. Транспорт MessagePort/iframe + `plugin_invoke` + реальный `HostDispatch` — далее.

- **Ф2-2b (часть 2) — Брокер live: Tauri-команды.** `AppState.plugins: Mutex<PluginBroker>`.
  `plugin_open_session(dir)` — манифест→совместимость→сессия с правами→**токен**; `plugin_invoke(token,
  method, path?)` — `authorize` (scoped + audit) → dispatch (`vault.readFile` через
  `vault::resolve_vault_path`, read-only). Лок брокера — только на синхронную авторизацию, async-I/O
  после освобождения. Зарегистрированы в `lib.rs`. Фронт-транспорт (iframe/MessagePort) + остальные
  методы (write/list/ai/net) — далее.

- **Ф2-2b (часть 3) — Расширение dispatch брокера: vault read/list/write.** `plugin_invoke` получил
  аргумент `content?` и возвращает JSON; реальный I/O вынесен в тестируемую `dispatch_vault`. Методы:
  `vault.readFile`/`vault.listFiles` (право `vault:read`), `vault.writeFile` (`vault:write`, через
  `resolve_vault_path_for_write`). Листинг/запись проходят ту же анти-traversal границу
  (defense-in-depth) уже ПОСЛЕ авторизации брокером по scope. +4 теста: read/list/write в пределах
  vault; path-escape (read+write) отклонён; unknown-метод / нет аргумента → ошибка; **E2E**
  «scope (broker) → dispatch I/O» с проверкой аудита (allow+deny). Rust 96 тестов. `ai.*`/`net.fetch`
  + фронт-транспорт — далее.

- **Ф2-2b (часть 4) — Фронт-транспорт плагинов: sandbox-iframe + MessagePort (§7.5, ADR-001/002).**
  Плагин крутится в `<iframe sandbox="allow-scripts">` (opaque origin — нет доступа к родителю/storage)
  и общается с хостом ТОЛЬКО через свой `MessagePort`. `lib/plugin-host.ts`: `attachPlugin` открывает
  сессию (токен **host-side**, плагину не передаётся), привязывает токен к ПОРТУ и обслуживает запросы
  через `tauriApi.plugins.invoke`; токен берётся из привязки порта, а НЕ из payload → confused-deputy
  закрыт и на фронте. `mountPlugin` — рукопожатие `nexus:ready`→`nexus:init` (порт через transfer, без
  гонки). Контракт `tauriApi.plugins` (`list`/`openSession`/`invoke`/`closeSession`) + мок-брокер
  (`lib/mock/plugins.ts`: токен→scope, glob с deny-override — зеркало Rust) → превью показывает РЕАЛЬНУЮ
  границу прав. Новая команда `plugin_close_session` (отзыв токена при размонтировании — без утечки
  сессий). UI: `PluginsPanel` (демо-плагин «Hello Reader» + лог брокерских вызовов ✓/✋), кнопка/команда
  `view.plugins`, i18n RU/EN. +11 фронт-тестов (транспорт, confused-deputy, scope, revoke). **Проверено
  в превью:** плагин в песочнице зовёт `vault.listFiles`/`readFile` через брокер, аудит фиксирует вызовы;
  console чистая. Реальная загрузка кода плагина из `.nexus/plugins/<id>/` + iframe-CSP — см. BACKLOG.

- **Ф2-3 — `registerCommand(source:'plugin')`: плагины расширяют палитру команд (§4.6).** Двунаправленный
  транспорт: плагин шлёт `ui.registerCommand {id,title}` → брокер авторизует (право **`ui:command`** в
  манифесте, иначе `NotGranted`) → фронт-релей регистрирует команду в реестре (`plugin:<dir>:<id>`,
  `source:'plugin'`). При запуске из палитры хост шлёт плагину событие `command` обратно по порту →
  плагин исполняет свой обработчик (host→plugin). `dispose()` снимает команды плагина из реестра;
  `plugin_invoke` для `ui.*` — только авторизация (host-I/O нет, регистрацию делает фронт). Демо-плагин
  регистрирует «Hello Reader: прочитать Inbox.md». +1 Rust-тест (ui:command), +1 фронт-тест (регистрация
  → событие → dispose). Rust 97 / фронт 76. **Проверено в превью:** команда плагина появляется в палитре,
  запуск → плагин читает Inbox.md через брокер (аудит фиксирует `vault.readFile`). Плагинные
  i18n-namespace (`plugin:<id>:<key>`) — далее.

- **Ф2-3 — Плагинные i18n-namespace `plugin:<id>:<key>` (закрыт AC-I18N-7).** Метод `ui.addTranslations`
  (`{локаль → {ключ → строка}}`): фронт-релей кладёт строки в i18next namespace `plugin` вложенно
  (`{<dir>:{<key>:value}}` → ключ `plugin:<dir>:<key>`, nsSeparator `:`). `registerCommand` принимает
  `titleKey` → команда получает локализованный `titleKey = plugin:<dir>:<key>` (палитра уже резолвит
  `titleKey` через `t()`, реагирует на смену языка). Брокер: `ui.*` теперь требует объявленной хотя бы
  одной ui-точки (fail-closed). Демо-плагин шлёт RU/EN-переводы и команду с `titleKey`. +1 Rust-тест
  (ui без точки → отказ), +1 фронт-тест (addTranslations резолвится, titleKey формируется). Rust 98 /
  фронт 77. **Проверено в превью:** заголовок команды плагина меняется EN↔RU при переключении языка.

- **Ф2-3 — AI host-API для плагинов: `ai.embed` + `ai.searchSemantic` (§7.2, право `ai:embed`).** Плагин
  получает RAG: `ai.embed` (текст → вектор) и `ai.searchSemantic` (запрос → гибридный поиск по vault,
  топ-8). Текст/запрос — в `content`. `plugin_invoke` снимает `reader/vectors/embedder` из `VaultContext`
  под read-локом и отпускает его ДО сети (как `search_content`); реальный вызов — в тестируемой
  `dispatch_ai`. Демо-плагин зовёт `ai.searchSemantic «roadmap»`. +1 Rust-тест (embed→вектор,
  search→выдача, на MockEmbedder+temp-индексе), +1 фронт-тест (мок ai). Rust 99 / фронт 78. **Проверено
  в превью:** аудит фиксирует `ai.searchSemantic` (полный host-API: vault read/list/write · ai
  embed/search · ui registerCommand/addTranslations). `ai.complete` (стрим) + `net.fetch` — см. BACKLOG.

- **Ф2-3 — `net.fetch` для плагинов: egress по allowlist + SSRF-гард (закрыт AC-SEC-4).** Плагин делает
  GET по URL (в `path`); хост извлекается и проверяется брокером против `net`-allowlist манифеста (нет в
  списке → `HostNotAllowed`). Поверх — **SSRF-гард** `is_private_host`: даже разрешённый хост не должен
  указывать на приватный/loopback/link-local/metadata-адрес (`127.*`, `10/172.16/192.168`,
  `169.254.169.254`, `::1`, `fc00::/7`, `fe80::/10`, `localhost`). Клиент без следования редиректам
  (анти-redirect-SSRF) + таймаут. Возвращает `{status, body}`. +1 Rust-тест (SSRF блокирует приватные,
  пропускает публичные), +1 фронт-тест (мок allowlist). Rust 100 / фронт 79. DNS-rebinding (резолв +
  проверка адреса) — доработка. `ai.complete` (стрим по порту) — остаётся (BACKLOG).

### Added — Фаза 3 (git-sync + производительность)

- **Ф3-1 — git-sync: фундамент (§8, core module).** vault как git-репозиторий (`src/git`, на `git2` /
  vendored libgit2 — кросс-платформенно, без системной зависимости). `GitSync::open_or_init` (open или
  `git init`); **управляемый `.gitignore`** (идемпотентный блок с маркером, не трогает пользовательские
  правила): `.nexus/*` исключён (индекс/векторы/БД, секреты `local.json`, **код плагинов** — фундамент
  **AC-Б3-1** и AC-SEC-3), но `!.nexus/config.json` синхронизируется (декларация плагинов); `status` —
  изменённые/новые/удалённые без игнорируемых. Весь libgit2-I/O синхронный → в Tauri вызывать в
  `spawn_blocking`. +3 теста (gitignore исключает секреты/плагины и оставляет config.json и заметки;
  идемпотентность + сохранение правил; open существующего). Rust 103. Коммит+secret-scan — Ф3-2;
  pull/push+конфликты — Ф3-3.

- **Ф3-2 — git-sync: выборочный коммит + secret-scan + авто-сообщение (AC-SEC-3).** `commit_all`:
  стейджит все не-игнорируемые изменения (`add_all` + `update_all` для удалений), **сканирует их
  содержимое на секреты** — при находке коммит НЕ делается (`BlockedBySecrets`), иначе коммит с
  авто-сообщением (`Vault sync: +N new, ~M changed, -K deleted`). `scan_secrets` — высокоточные форматы
  (PEM private key, `sk-…` OpenAI, `ghp_…`/`github_pat_` GitHub, `AKIA…` AWS, `xox…` Slack), без
  «high-entropy»-шума → мало ложных. Подпись из git-config, иначе дефолт `Nexus <nexus@local>`. +2 теста
  (детект форматов без ложных на URL/тексте; коммит→nothing→блокировка секрета). Rust 105. Команды/UI +
  sync-lock + pull/push — Ф3-3.

- **Ф3-3a — git-sync: команды + UI + sync-lock.** Tauri-команды `git_status`/`git_commit` (libgit2 в
  `spawn_blocking`, под **sync-локом** `AppState::git_lock` — один git-вызов за раз; репозиторий
  открывается per-вызов, т.к. git2 `!Send`). Фронт: контракт `tauriApi.git` (status/commit) + мок;
  панель **`SyncPanel`** (список изменений с бейджами A/M/D/R, кнопка коммита, исход —
  committed/nothing/**blocked-by-secrets** с файлами+строками), кнопка/команда `view.sync`, i18n RU/EN.
  +1 фронт-тест (мок status→commit→nothing). Rust 105 / фронт 80. **Проверено в превью:** изменения →
  коммит → «✓ Vault sync: ~2 changed», список очищен. pull/push + детект конфликтов — Ф3-3b.

- **Ф3-3b-1 — git-credentials в системном keychain (AC-SEC-3).** Токен доступа к remote хранится в
  keychain ОС (macOS Keychain / Windows Credential Manager / Linux Secret Service через `keyring` 3,
  zbus — pure-Rust, без системного libdbus при сборке), **на диск НЕ пишется** и не в git. `git/creds.rs`:
  `set_token`/`get_token`/`delete_token`/`has_token` (запись `service=nexus-git`, `account=<путь vault>`).
  Команды `git_set_token`/`git_clear_token`/`git_has_token` (keychain-I/O в `spawn_blocking`) +
  `tauriApi.git` + мок. +1 guarded Rust-тест (`#[ignore]`, реальный keychain) + 1 фронт-тест (мок-токен).
  Rust 105/8ign · фронт 81. Используется credentials-callback'ом git2 в pull/push — Ф3-3b-2.

- **Ф3-3b-2 — git-sync: remote + pull/push по https (§8).** git2 с `https` + **vendored-openssl**
  (компилит OpenSSL из исходников → кросс-платформенно без системных зависимостей; +время сборки CI).
  `GitSync`: `set_remote`/`get_remote` (origin), `push` (текущая ветка), `pull` (fetch + merge-analysis →
  `up-to-date` / `fast-forward` (применяется) / `merge-required`). credentials-callback берёт токен из
  keychain (Ф3-3b-1) как https-пароль. Команды `git_set_remote`/`git_get_remote`/`git_sync` (pull-ff →
  push, под sync-локом). `tauriApi.git` (setRemote/getRemote/sync) + мок. +1 Rust-тест (remote
  set/get/overwrite; push/pull — сеть, не юнит-тестятся) + 1 фронт-тест. Rust 106 / фронт 82. UI
  настройки remote + разрешение конфликтов (`merge-required`) + plugin pull → `needs-review` — Ф3-3b-3 (закроет AC-Б3).

- **Ф3-3b-3 — git-sync UI: настройка remote + sync (финиш git-sync).** Панель «Синхронизация»
  расширена: поле **Remote** (URL) + **Токен** (пароль → в системный keychain через `git_set_token`),
  индикатор подключения (`git_has_token`), кнопка **Синхр.** (`git_sync` = pull-ff → push) с исходом
  (`up-to-date` / `synced` / `merge-required` → «разрешите вручную» / ошибка). Контракт `tauriApi.git`
  (setRemote/getRemote/sync) + мок; i18n RU/EN. **Проверено в превью:** remote + токен → keychain
  («✓ токен в keychain») → sync → «↓↑ Синхронизировано». Фронт 82. **git-sync функционально готов**
  (локально + credentials + remote pull/push + UI). Полное разрешение конфликтов (`merge-required`) и
  plugin pull → `needs-review` — в BACKLOG (завязано на marketplace); git-exclusion кода плагинов
  (ядро AC-Б3) закрыт `.gitignore` ещё в Ф3-1.

- **Ф3-08 — единый граф всего vault (AC-DOD-Ф3 «единый граф»).** К локальному N-hop добавлен режим
  **«Весь vault»**: Rust `get_full_graph(limit)` отдаёт топ-`limit` файлов **по степени связности**
  (хабы наверх — осмысленный обзор на 50k без перегруза рендера) + рёбра между ними + мету
  (`totalFiles`, `truncated`). Раскладка по-прежнему в **Web Worker** (`layout.worker.ts`,
  main-thread не блокируется — закрывает AC-PERF-6 целиком). `GraphView` получил переключатель
  Локальный/Весь vault, узлы единого графа масштабируются по степени, подпись «показано N из M».
  Контракт `tauriApi.graph.getFullGraph` + мок (общий построитель смежности `buildAdjacency`);
  i18n RU/EN. +1 Rust-тест (все узлы → лимит обрезает по степени), +3 фронт-теста (контракт мока).
  Rust 107, Фронт 85.

- **Ф3-09 — нагрузочный бенчмарк полного пайплайна (AC-DOD-Ф3, верификация AC-PERF).**
  `eval::tests::bench_index_scale` (`#[ignore]`, не в CI): синтетический vault из `NEXUS_BENCH_FILES`
  заметок (RU+EN + вики-ссылки) индексируется **с живым bge-m3 :8083**, замеряет индексацию/поиск/граф
  и проецирует на 50k. **Реальные числа** (см. `docs/dev/perf.md`): граф единый на пределе кэпа
  (2000 узлов / 6000 рёбер) — **18 мс** + раскладка в Worker; гибридный поиск **20–100 мс** —
  AC-PERF-6 и отзывчивость поиска подтверждены. Старт UI индексацией **не блокируется** (фон, AC-PERF-1).
  Узкое место — эмбеддинг при первичном скане **~40 эмб/с → 50k ≈ 21 мин фоном** (последовательные
  одиночные запросы): рычаг = батч + конкурентность (BACKLOG, `perf.md`). Rust 107 (+1 `#[ignore]`).

- **Ф3-10 — конкурентный скан индексации (рычаг throughput, §10).** `scan_vault` держит до
  `SCAN_CONCURRENCY=16` файлов «в полёте» (`futures::buffer_unordered`) под семафором
  `EMBED_CONCURRENCY=8` — embed-/IO-ожидания перекрываются. **Кооперативно в ОДНОЙ задаче** (не
  параллелизм): между `.next()` ни одна future не поллится → синхронные секции usearch/БД не исполняются
  параллельно, **без гонок и без блокировок**. Починена латентная неэффективность: раньше скан был
  последовательным и семафор конкуренции при первичном наполнении **простаивал** (1 запрос в полёте).
  Бенч: **39 → 56 эмб/с (~1.4×), 50k ≈ 21 → 15 мин.** Прирост скромный — потолок упёрся в **инференс
  локального bge-m3** (~50 эмб/с на одиночных входах), не в клиента; на быстром/GPU-сервере конкуренция
  даст больше. Главный оставшийся рычаг — **cross-file батчинг** (чанки попёрек файлов в запросы по 64),
  в BACKLOG. Новая зависимость: `futures` (pure-Rust). Регрессий нет (Rust 107).

### Added — Фаза 4 (Polish — дизайн-система)

- **Ф4-0 — дизайн-система «Hermes»: токены + темы + self-hosted шрифты (фундамент, ADR-006).**
  Принят подготовленный дизайн-хендофф (вендорен в `docs/design/handoff/`). Порт **токенного слоя**
  в `src/styles.css`: **OKLCH**-палитра, тёплый hue, темы через `data-theme` (light «old paper» /
  dark «warm clay»), акцент через `data-accent` (amber/teal/sage/clay), elevation/focus-ring/моушн,
  новые семантические токены (`--color-chrome/selected/text-faint/tag/*-soft`, `--space-5..8`,
  `--radius-lg`, `--font-serif`). Имена совпадают с прежними → **весь существующий апп перекрасился
  когерентно** без правок компонентов. **Тема** (`stores/theme.ts`): тоггл свет/тёмная (кнопка
  sun/moon в шапке + команда `theme.toggle`), старт из `localStorage`/системной, применение до рендера
  (без вспышки), 320ms кросс-фейд (gated `prefers-reduced-motion`), персист. **Шрифты self-hosted**
  через `@fontsource` (Onest / JetBrains Mono / Source Serif 4 — offline/local-first, CSP уже
  разрешает). i18n RU/EN. **Проверено в превью:** обе темы (OKLCH), Onest загружен, тоггл + кросс-фейд.
  Док: ADR-006, `docs/dev/design.md`, §12. Фронт 85 (без регрессий). Дальше — порескринный рестайл.

- **Ф4-1 — chrome shell: titlebar + status bar + сетка (рестайл, вариант A).** Оболочка приложения
  перестроена под дизайн: сетка `38px titlebar / 1fr тело / 26px status bar`. **Titlebar**
  (Liquid-Glass — blur поверх chrome): бренд-марк «созвездие» (терракотовый squircle, инлайн-SVG) +
  имя vault, центральная **поисковая пилюля** (открывает Command Palette, ⌘K) и правая группа
  инструментов (чат/граф/плагины/sync · тема · **RU·EN** текст-тоггл · открыть vault) — **переехала
  из шапки сайдбара**. **Status bar**: путь vault + индикатор темы (богаче — отдельными срезами, без
  фейк-данных). Новые компоненты `components/chrome/{Titlebar,StatusBar,BrandMark}` + CSS-модули;
  шапка сайдбара убрана (поиск остаётся в сайдбаре). Вариант **A** — кастомный бар в обычном OS-окне
  (frameless + traffic-lights — отдельным шагом). i18n RU/EN (`chrome.search`). **Проверено в
  превью:** бренд/пилюля→палитра/тоггл темы/RU·EN, обе темы. Фронт 85, без регрессий.

- **Ф4-2 — рестайл сайдбара (file tree + поиск) под дизайн.** Строки дерева получили **фирменное
  выделение** открытого файла: фон `--color-selected` + **3px акцентная полоса слева** + акцентная
  иконка + вес 500; hover — `--color-surface-hover`; иконки в покое `--color-text-faint`; курсор
  клавиатуры — мягкая подсветка (визуально отделён от открытого файла). Поле поиска — `--radius-md` +
  мягкое акцентное focus-кольцо (`--color-accent-soft`), результаты hover — surface-hover. Чистый
  рестайл на токенах (логика не тронута). **Проверено в превью:** открытый README с акцент-полосой и
  акцентной иконкой, связи/теги в дизайн-цветах, обе темы. Фронт 85. **Rail** (files/search/tags/
  starred) — отдельным срезом (нужны панели tags/starred = новые фичи, не рестайл).

- **Ф4-3 — рестайл вкладок редактора (floating tabs) под дизайн.** Таб-стрип на `--color-chrome`;
  вкладки «плавающие» (скруглённый верх, gap 4px), активная **приподнимается** до цвета холста
  (`--color-bg`) с `--tab-shadow` + рамкой + **2px акцентной полосой сверху**; неактивная — прозрачная,
  hover `--color-surface-hover`. Полоса активной вкладки: **акцент в фокусной группе**, приглушённая
  (`--color-border-strong`) в неактивной — сохранён фокус-cue сплита. Кнопки close (18px, hover
  surface-hover) и split — в дизайн-стиле; dirty-точка `--color-text-muted`. Чистый рестайл (логика
  вкладок/DnD/сплитов не тронута). **Проверено в превью:** две вкладки, активная с акцент-полосой, обе
  темы. Фронт 85. Edit/Preview-pill и центр-measure редактора — отдельно (нет preview-режима в CM6
  source; это фича, не рестайл).

- **Ф4-4 — рестайл графа (цвета узлов/рёбер из токенов темы).** sigma рендерит в WebGL → цвета берём
  из токенов активной темы через **1×1-canvas readback** (`getComputedStyle` нынче отдаёт `oklch` как
  есть, а шейдер его не парсит → конвертируем реальный пиксель в rgb). Центр/активная нота —
  `--color-accent` (терракота, крупнее), соседи — `--color-text-muted`, рёбра — `--color-border-strong`,
  подписи — text-muted (шрифт Onest). Холст графа — радиальный градиент (`bg-elevated`→`bg`). **Проверено
  в превью:** центр терракотовый, соседи приглушённые, обе темы. Фронт 85. Пульс/halo активной ноты —
  отдельно (нужен кастомный node-renderer sigma).

- **Ф4-5 — рестайл Command Palette (glass-модал + staggered-раскрытие).** Скрим — затемнение +
  `blur(2px)`; палитра — «стекло» (`color-mix` bg-elevated 88% + `blur(28px) saturate`),
  `--color-border-strong`, radius-lg, elevation-2, pop-in. Активная строка — `--color-accent-soft`;
  плейсхолдер `--color-text-faint`; kbd-хинты в дизайн-стиле. **Staggered-раскрытие** первых строк
  (`animation-delay` по индексу `--cmd-i`, проброшен из компонента). Анимации gated
  `prefers-reduced-motion` (глобально). **Проверено в превью:** стекло поверх размытого фона, активная
  в accent-soft, kbd-хинты, обе темы. Фронт 85.

- **Ф4-6 — рестайл AI-панели (чат) под дизайн.** Шапка: провайдер-пилюля «local» (точка-индикатор
  `--color-success` + подпись, скруглённая 99px), кнопки действий — прозрачные (hover `surface-hover`).
  Чат: пузырь пользователя — `--color-accent-soft` + рамка (вместо сплошного акцента), ответ ассистента —
  прозрачная проза. **Источники — карточки** (surface + рамка, hover `--color-ai`) с нумерованным
  AI-бейджем (фон `--color-ai`, моно-цифра). **Композер — плавающий скруглённый бокс** (radius-lg,
  bg-elevated, elevation-1) с мягким акцентным focus-кольцом; send/stop — пилюли. Чистый рестайл на
  токенах. Сборка зелёная (Фронт 85); превью-проверка отложена — порт 1420 занят tauri-сборкой.

- **Ф4-7 — рестайл панели плагинов (демо + broker-аудит) под дизайн.** Бэкдроп — затемнение +
  `blur(2px)`; диалог — `bg-elevated` + `border-strong` + radius-lg + elevation-2; чип «песочница» —
  пилюля; close — прозрачная кнопка (hover surface-hover); аудит-лог на `--color-chrome`, строки hover
  surface-hover, вердикт allowed→`--color-success` / denied→`--color-danger`, метод — моно. Полноценный
  **менеджер плагинов** (карточки installed/marketplace + permission-chips + consent-sheet) — **post-v1**
  (завязан на marketplace, в BACKLOG). Чистый рестайл на токенах. Сборка зелёная (Фронт 85).
  **Рестайл существующих экранов завершён** (chrome · сайдбар · вкладки · граф · palette · ai · plugins).

- **Ф4-8a — conflict resolver: бэкенд 3-way merge (закрывает git-хвост Ф3).** libgit2 **in-memory**
  merge (`merge_commits`) — репозиторий и рабочее дерево НЕ трогаются до явного применения (атомарно,
  безопасно: при отмене ничего не изменилось). `merge_preview(token)` — fetch + merge →
  `up-to-date` / `clean` / `conflicts` (на каждый файл base/ours/theirs). `apply_merge(theirs,
  resolutions)` — накладывает резолвы (path→содержимое) на in-memory индекс, проверяет отсутствие
  остаточных конфликтов, создаёт **merge-коммит (2 родителя)**, двигает ветку + force-checkout, затем
  push. Команды `git_merge_preview` / `git_resolve_conflicts` (`spawn_blocking`, sync-лок, токен из
  keychain). Контракт `tauriApi.git.mergePreview/resolveConflicts` + типы (`GitMergePreview`/
  `GitConflictFile`) + мок. **+1 Rust-тест** (реальный конфликт base→ours/theirs → превью с 3 версиями
  → резолв → merge-коммит 2 родителя → повторно up-to-date). Rust 108. UI-панель resolver — Ф4-8b.

- **Ф4-8b — conflict resolver: UI 3-way панель (git-хвост Ф3 закрыт целиком).** `ConflictResolver`
  (overlay поверх SyncPanel): грузит `git.mergePreview` (in-memory, безопасно), на каждый конфликтный
  файл — колонки **НАШЕ / ИХ** (моно) + пилюли «Наше»/«Их» + **редактируемый результат** (дефолт —
  наше); «Применить и запушить» → `git.resolveConflicts` (merge-коммит + push). Состояния:
  up-to-date / clean (применить без конфликтов) / done («Слито и запушено» + oid) / error. Открывается
  из SyncPanel при `merge-required` (кнопка «Разрешить конфликты»). i18n RU/EN (`conflict.*`).
  **Проверено в превью** (скриншот; временно мокнув sync→merge-required, затем вернул): полный поток
  sync → merge-required → resolver (3-way) → правка → применить → «Слито и запушено». Фронт 85.
  **git-хвост Фазы 3 закрыт** (бэкенд Ф4-8a + UI Ф4-8b).

- **Ф4-9 — режим чтения (⌘R, distraction-free).** UI-флаг `reading` (`stores/ui`): прячет сайдбар и
  AI-панель, редактор — на всю ширину (сетка тела → `1fr`). Команда `view.reading` (палитра) + хоткей
  **⌘R** (toggle) + **Esc** на выход (если поверх нет оверлея — у них свой Esc). i18n RU/EN. **Проверено
  в превью:** переключение прячет сайдбар, редактор full-width. Центрирование документа узким measure
  (~62ch) — рефайнмент (CM6-контент), в BACKLOG. Фронт 85.

- **Ф4-10 — просмотр не-md вложений (картинки/PDF во вкладке).** `FileViewer` (`components/editor`):
  открытие картинки (png/jpg/gif/svg/webp/…) или PDF показывает её во вкладке вместо CM6 — URL через
  **asset-протокол Tauri** (`convertFileSrc`, CSP `img-src asset:`). Бинарь больше НЕ читается как текст
  (`openFile` пропускает `readFile` для viewable → нет ошибки UTF-8); backlinks-бар скрыт для вложений.
  Вне Tauri (браузер) — плейсхолдер «просмотр в приложении». Утилита `lib/file-kind`
  (isImage/isPdf/isViewable). i18n RU/EN. **Проверено в превью** (png → вьюер-плейсхолдер, не редактор).
  Фронт 85. **Inline-рендер в markdown** (`![[embeds]]`, **Mermaid**, **LaTeX**) — эпик **Live Preview**
  (§13, отдельный; в BACKLOG — нужен markdown-renderer + mermaid + katex).

- **Ф4-11 — онбординг (первый запуск).** При отсутствии открытого vault App показывает приветственный
  экран `Onboarding` (вместо авто-открытия): бренд-марк + серифный заголовок + интро + CTA «Открыть
  vault» (нативный диалог в Tauri / мок в браузере) + переключатели языка/темы, радиальный фон.
  Авто-открытие мок-vault убрано (теперь онбординг → клик). i18n RU/EN (`onboarding.*`). `App.test`
  обновлён под новый поток (онбординг → открыть → дерево). **Проверено в превью.** Фронт 85.
  Многошаговый flow (проверка LLM-сервера, прогресс индексации) — рефайнмент (BACKLOG). **Home-дашборд
  перенесён в BACKLOG** (концепция дозревает — решение владельца).

- **Ф4-12 — панель оформления (tweaks): тема / акцент / плотность.** `TweaksPanel` (оверлей; кнопка
  «слайдеры» в titlebar + команда `view.tweaks`): **тема** (light/dark), **акцент**
  (amber/teal/sage/clay → `data-accent`, весь апп перетинтовывается мгновенно), **плотность**
  (`--row-h`, comfortable/compact). Стор оформления расширен (`stores/theme`: accent/density + persist
  + кросс-фейд, применение до рендера без вспышки). i18n RU/EN (`tweaks.*`). **Проверено в превью:**
  смена акцента на teal → `data-accent=teal`, апп перетинтован; тема/плотность переключаются. Фронт 85.
  **Рестайл + новые экраны Фазы 4 завершены** (chrome · сайдбар · вкладки · граф · palette · ai ·
  plugins · conflict resolver · reading mode · вложения-вьюер · onboarding · tweaks). Home — в BACKLOG;
  дальше — инфра релиз-препа (C).

- **Ф4-13 — печать / экспорт PDF активной заметки (инфра C).** Команда `file.print` (палитра):
  `printActiveNote` рендерит заметку в чистый print-контейнер и вызывает системный диалог печати
  («Сохранить как PDF»); оболочка (titlebar/sidebar/…) скрыта через `@media print`. Печатает
  **исходник markdown** (отрендеренный HTML/Mermaid/LaTeX — эпик Live Preview). Контейнер чистится по
  `afterprint`. i18n RU/EN. Фронт 85.

- **Ф4-14 — локальный crash-reporter (panic-hook → scrubbed-лог, инфра C).** `crash::install_hook()`
  (в `run()` до всего): паника → отчёт (сообщение + место + время + версия) в `~/.nexus/crashes/`,
  **без сети и без контента заметок**; домашний путь вычищается на `~` (privacy by default). Прежний
  hook (stderr) сохраняется. **+1 Rust-тест** (scrub чистит HOME, отчёт без сырого пути). Rust 109.
  Поможет на фазе тестирования (точный `файл:строка` для багов вроде краша графа). **Отправка на
  бэкенд** — строго opt-in, отдельно (BACKLOG, нужен эндпоинт + согласие).

### Added — UI-доводка

- **Виртуализация ленты чата (DESIGN §«лента виртуализирована»).** `ChatView` рендерит сообщения через
  `@tanstack/react-virtual` (только видимые; переменная высота → `measureElement`, `initialRect` для
  jsdom). **Умный автоскролл**: следует за стримом только если пользователь у низа (`atBottom` по
  `onScroll`) — чтение истории не дёргается. Прозрачно (выглядит как было); проверено в превью. Фронт 64 теста.

### Fixed / Hardening

- **Кросс-платформенная анти-traversal граница (Windows; AC-SEC-1).** `resolve_vault_path` /
  `resolve_vault_path_for_write` блокировали абсолютные пути через `is_absolute()`, но на Windows
  `/etc/passwd` не абсолютен (нет диск-префикса) → `root.join("/etc/passwd")` даёт `C:\etc\passwd`
  (побег с диска), что ловилось лишь бэкстопом (canonicalize+`starts_with`) и возвращало `Io` вместо
  `PathEscape`. Добавлен явный отказ **root-anchored** путей (`rel.has_root()` — ловит `/x` и `\x` на
  обеих ОС). Поймано Windows-джобом CI (тест `resolve_blocks_traversal_and_absolute`). Поведение на
  Unix не изменилось; fail-closed усилен и сделан явным до файловых операций.

### Tooling

- **CI: динамическая матрица (экономия минут Actions).** На `pull_request` Rust-матрица гоняет только
  `ubuntu`+`windows`; полная ×3 (с **macOS — ×10 минут**) — лишь на `push` в `main`. Режет расход CI
  на PR ~в 4× (важно для приватного репо с квотой 2000 мин/мес); macOS-специфику ловим на main до релиза.
- **Живой smoke по реальному vault** (`eval::live_real_vault_smoke`, `#[ignore]`, env
  `NEXUS_TEST_VAULT`): индексирует произвольный vault во временные db+usearch (реальный `.nexus/` не
  трогается) и проверяет кросс-язычный гибридный поиск на живом контенте (bge-m3 :8083). Находка
  (стоп-слова запроса + слабый IDF на малом корпусе теснят кросс-язычную семантику) — в `docs/BACKLOG.md`.
