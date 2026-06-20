# Nexus — Threat Model

> Статус: v0.1 (2026-06-20). Оператор-facing модель угроз. **Чётко разделяет РЕАЛИЗОВАНО (Фаза C, в коде сегодня) и ПЛАН (Фазы 2/3).** P0-гейт перед включением host-actuator (Фаза 3).

## 1. Допущения и границы доверия
- **Local-first, single-owner.** Один оператор владеет устройством и vault.
- **Vault = доверенный источник** (созданный/курируемый оператором). Содержимое заметок — данные, не код.
- **Plugins / skills / web / tool-output = НЕдоверенный вход.** Обрабатываются как данные (анти-инъекция).
- **Оператор доверяет своей LLM** (Фаза C, vault-only autonomous): kill-switch + risk-tiers + audit смягчают КООПЕРАТИВНЫЕ ошибки модели, НЕ адверсарную/джейлбрейкнутую LLM. Реальная изоляция против adversarial-LLM — OS-граница Фазы 3 (песочница), не in-process-эвристики.
- **TCB (доверенная база):** ядро (`nexus-core`), OS, локальный LLM-сервер. Approval-гейт, redaction, fencing — accident-prevention, НЕ containment.

## 2. Сценарии угроз × митигации

Легенда: ✅ реализовано (Фаза C) · 🔭 план (Фаза 2/3) · ⚠ остаточный риск.

### T1 — Confused-deputy через плагин/скилл (расширение крадёт права)
- ✅ Capability-broker: идентичность по токену, не по запросу; scoped-проверка fail-closed (`plugin/broker.rs`, `plugin/permission.rs`). Skills: declared∩granted, forced_base={VaultRead,VaultWrite}, shell/web СТРУКТУРНО инертны в Фазе C (`skills/capability.rs`).
- ✅ Path-scope + symlink-guards (`resolve_vault_path`, `resolve_plugin_dir` canonicalize+containment).
- ⚠ Plugin-audit пока in-memory (`plugin/broker.rs AuditLog`) → 🔭 durable перед Фазой 2.

### T2 — SSRF / DNS-rebind (эгресс на внутренние сервисы / metadata)
- ✅ Единственный outlet `GuardedClient`; `net/resolve.rs check_resolved_ips`: metadata (169.254.169.254 / IMDS-v6) + link-local (169.254/16, fe80::/10) блок **ВСЕГДА**; private/loopback/ULA/CGNAT блок при `deny_private` (web/agent-классы). LAN-LLM (192.168.0.31) жив при chat/embed (`deny_private=false`).
- ⚠ Chat-путь допускает LAN: при злонамеренной конфигурации LAN (rogue SLAAC/DHCP-metadata-сосед) chat мог бы достучаться. Митигация: оператор контролирует LAN; metadata всё равно заблокирован.

### T3 — Утечка секретов (в логи / эгресс / аудит)
- ✅ `Redacted<T>` + crash-scrub; content-free durable-аудит (`DiffSummary` без текстового поля by-construction, AGENT-6); egress-аудит не пишет тело.
- 🔭 Env-scrubbing для shell (Фаза 3): block-list KEY/TOKEN/SECRET/PASSWORD/CREDENTIAL/DSN/WEBHOOK + safe-prefix whitelist + `env_passthrough` в SKILL.md. **НЕ реализовано — блокер Фазы 3.**

### T4 — Потеря данных (актуатор перезаписал/удалил)
- ✅ Актуатор `classify` — ИСЧЕРПЫВАЮЩИЙ match по `ActionTarget` (3 vault-варианта: NoteCreate/NoteEdit/Frontmatter), без catch-all (compile-time fail-closed). RiskTier Auto/Confirm/HardBlock. Write-before-act durable intent (`net/mod.rs`, миграция 020) + snapshot-before-act + **обратимый undo** (AGENT-4). Удаление — в корзину (`move_to_trash`), не hard-rm.
- ✅ Актуатор **default-OFF** (`ai.agent_actuator_enabled=false`) → из коробки stubs.

### T5 — Sync / git-injection (multi-device vault)
- ✅ git-sync отдельный канал, оператор контролирует креды (keychain); агент НЕ вызывает git-sync, sync НЕ исполняет через актуатор. Конфликт-резолвер (3-way).
- 🔭 Agent-triggered sync (Фаза 2) — если появится, за capability-гейтом.

### T6 — Prompt-injection → актуатор (вредная инструкция в данных провоцирует запись)
- ✅ Недоверенный контент (web/skills/память/vault-fetch) обёрнут в user-role + per-request `injection_marker` + system-префикс «данные, не инструкции» (`fence_observation`). Defense-in-depth ВТОРИЧНА к approval-гейту.
- ✅ Любое Confirm-действие требует human-approval (aggregated diff) перед apply; headless = PolicyDefault auto-DENY.
- ⚠ Маркер-фенсинг — accident-prevention, НЕ защита от джейлбрейка. Реальная защита = approval + kill-switch + (Фаза 3) OS-изоляция.

### T7 — Agent-shell / host-execution (🔭 Фаза 3, СЕЙЧАС НЕ СУЩЕСТВУЕТ)
- ✅ Сегодня: shell/process/host **структурно отсутствуют** в `ActionTarget` (нет варианта → нечего исполнить).
- 🔭 Фаза 3 (за гейтом): новые ActionTargets ВНУТРИ rootless-Podman `--network=none`, vault `:ro`, записи через host-actuator (typed RPC GuardedProxy), env-scrub, `shell_enable` config default-false. **Гейт: этот THREAT_MODEL + env-scrub реализованы + явный owner-approval.**

### T8 — Connector remote-auth (app↔agentd по сети)
- ✅ Фаза C: in-process / loopback. 🔭 Фаза 2: AF_UNIX-сокет (`~/.nexus/agentd.sock`, mode 0600 — OS-изоляция юзеров). 🔭 Remote (WS): TLS обязателен + token (bearer, per-session scope, keychain/env), opt-in. Detail: `docs/specs/agent-connect.md`.

## 3. Известные пробелы (осознанные)
- In-process эвристики (approval/redaction/fencing) НЕ являются containment против adversarial-LLM — нужна OS-граница (Фаза 3 Podman; будущее — microVM Firecracker/gVisor для жёсткой изоляции).
- Plugin-audit + pending-proposals не durable (🔭 Фаза-2 пререквизиты).
- Env-scrubbing для shell не реализован (🔭 Фаза-3 блокер).

## 4. Фаза-гейты (security)
- **Фаза C (сейчас)**: vault-only актуатор, default-OFF, kill-switch, audit. Допущение: оператор доверяет LLM.
- **Фаза 2**: durable plugin-audit + proposals · AF_UNIX connector · sandbox-каркас. Перед: эти пререквизиты.
- **Фаза 3**: host-actuator. Перед: ЭТОТ THREAT_MODEL принят оператором + env-scrub реализован + `shell_enable` gate + sandbox работает.
