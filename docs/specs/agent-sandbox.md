# AGENT-SANDBOX — OS-песочница прогона агента (Фаза 2 каркас + Фаза 3 host-actuator)

> Spec v1.0 (2026-06-21). Decision-complete. Owner-gated security (см. `feedback_full_autonomy_phase23`): всё **default-OFF, fail-closed, CI-verifiable**. Синтез трёх дизайнов (ephemeral-podman / persistent-sandbox / bubblewrap-ns) + adversarial-критики; выбран ephemeral-podman-каркас с КЛЮЧЕВОЙ инверсией исполнения (shell исполняется ВНУТРИ песочницы, решение остаётся на хосте). Зависимости-якоря: `crates/nexus-core/src/net/mod.rs` (egress-chokepoint), `crates/nexus-core/src/actuator/*` (классификатор/undo), `crates/nexus-core/src/agent/connect/*` (AF_UNIX-транспорт), `Dockerfile` (база образа), `docs/THREAT_MODEL.md` (§T6/T7/T8, Фаза-гейты), `docs/specs/agent-connect.md` (протокол коннектора).

---

## 1. Цель и не-цели

### 1.1 Что песочница защищает (цель)

OS-граница вокруг прогона агента — то, что `THREAT_MODEL §1/§9` прямо называет НЕ достижимым in-process-эвристиками (approval/redaction/fencing). Песочница — это **реальная изоляция против adversarial/джейлбрейкнутой LLM**, а не против кооперативно-ошибающейся (последнюю уже смягчают kill-switch + risk-tiers + audit Фазы C).

Защищаемые свойства (что становится **структурно невозможным** для кода внутри прогона, а не «проверяется и блокируется»):

1. **Эксфильтрация в обход chokepoint'а** — внутри песочницы НЕТ сетевого интерфейса (`--network=none`). Единственный сетевой путь — host-side `GuardedProxy` поверх СУЩЕСТВУЮЩЕГО `GuardedClient` (`net/mod.rs`). Любой байт эгресса проходит `EgressPolicy::check` → `check_resolved_ips` (SSRF/DNS-rebind) → durable `egress_audit` write-before-act с верным `run_id`. Второго не-guarded пути нет физически.
2. **Произвольное исполнение на хосте** (Фаза 3) — когда появятся host ActionTargets (shell/process/git), КОД команды исполняется ВНУТРИ песочницы (cap-drop, read-only rootfs, vault `:ro`, без NIC), а не процессом хоста с его ambient-полномочиями. Решение «исполнять ли» остаётся host-side (`classify`/`dispatch_action`/approval).
3. **Запись в произвольную ФС / порча vault** — vault смонтирован `:ro`; любая мутация (включая 3 vault-таргета Фазы C) проходит host-side `dispatch_action` (`resolve_vault_path_for_write` canonicalize/symlink-рубеж + snapshot-before-act + reversible `undo_run`). Внутри песочницы нет rw-доступа к vault, к `.nexus/nexus.db`, к ledger.
4. **Эскалация привилегий / персист компрометации** — `--cap-drop=ALL`, `--security-opt=no-new-privileges`, `--userns=keep-id`; контейнер ЭФЕМЕРНЫЙ (`--rm`), состояние не переживает прогон.
5. **Утечка секретов хоста в исполняемую команду** — окружение песочницы строится **fail-closed: пустое + явный allow-list** (НЕ denylist-скруб хост-env), см. §5.4.

### 1.2 Что песочница НЕ защищает (не-цели, честные остаточные риски)

- **Не контейнерит конфиденциальность READ-then-exfil логически.** Агенту дан vault `:ro` (он обязан читать заметки для контекста) И санкционированный эгресс (chat/web). Джейлбрейкнутая модель может прочитать заметку и закодировать её в ЛЕГИТИМНЫЙ POST к allowlisted-хосту. `GuardedClient` аудитит `{host, feature, bytes_out}`, не СОДЕРЖИМОЕ тела. Митигации (per-run egress-бюджет, Web OFF по умолчанию для песочных прогонов, subtree-mount vault) — снижают, не устраняют. См. §5.6 / §10.
- **Не контейнерит kernel/Podman/crun-escape.** Rootless-namespaces — shared-kernel-граница. Kernel-0day или userns-LPE = выход на uid оператора (на single-owner-боксе это владелец vault, kill-switch, секреты). Жёсткая изоляция (microVM Firecracker/gVisor) — будущий апгрейд (`THREAT_MODEL §3`), вне scope этой спеки.
- **Не контейнерит рассуждение модели** — только её ЭФФЕКТЫ. Модель может «решить» сделать вредное; песочница лишает её способности это вредное ИСПОЛНИТЬ.
- **Не on-device на macOS/Windows.** Песочница — Linux-host-only фича (см. §9). На не-Linux host-actuator остаётся HardBlock/disabled.
- **Не заменяет** durable agent_runs/agent_actions ledger, kill-switch, undo, audit — они остаются host-side и переиспользуются verbatim.

---

## 2. Фаза-2 (каркас) vs Фаза-3 (host-actuator) — чёткое разделение

Песочница вводится ДВУМЯ независимо-мерджабельными гейтами. Это намеренно: каркас (Фаза-2) можно построить, протестировать и смержить БЕЗ единого нового опасного действия, удовлетворив пререквизит `THREAT_MODEL §4` («Фаза 2: sandbox-каркас») до того, как `THREAT_MODEL §4` («Фаза 3: host-actuator») вообще станет на повестку.

### Фаза 2 — КАРКАС (`ai.sandbox_enabled`, default-false)

Что входит:
- Запуск СУЩЕСТВУЮЩЕГО `run_agent_loop` (vault-only актуатор Фазы C, 3 таргета) ВНУТРИ эфемерного rootless-Podman-контейнера `--network=none`.
- `GuardedProxy` поверх AF_UNIX: единственный сетевой путь прогона (chat/embed/web эгресс модели — через host `GuardedClient`).
- Control-plane: `SandboxRunner` (sibling `JobHandler`), стрим событий через существующий AGENT-CONNECT.
- vault `:ro` bind; ВСЕ vault-записи (NoteCreate/NoteEdit/Frontmatter) — host-side через typed RPC `host/act` к НЕИЗМЕНЁННОМУ `dispatch_action`.
- Fail-closed: песочница ON-но-недоступна → прогон `error`, БЕЗ тихого in-process fallback.

Чего НЕТ в Фазе-2:
- **Никаких новых `ActionTarget`-вариантов.** `ActionTarget` остаётся `{NoteCreate, NoteEdit, Frontmatter}`. shell/process/git непредставимы (как сегодня).
- Никакого исполнения произвольных команд. Метод RPC `host/exec` НЕ существует в Фазе-2 (только `egress/*` и `host/act` для vault-таргетов).

Смысл Фазы-2: получить ВСЮ инфраструктуру изоляции и убедиться, что egress физически замкнут, ещё до появления опасной capability. Каркас при `sandbox_enabled=false` = байт-в-байт сегодняшнее поведение.

### Фаза 3 — HOST-ACTUATOR (`ai.shell_enable`, default-false, требует `sandbox_enabled` И Linux)

Что добавляется:
- Новые `ActionTarget`: `ShellRun { argv, cwd_rel }`, `ProcessSpawn { … }`, `GitOp { op }` (exhaustive-match ЗАСТАВИТ дописать ветки `classify`/`ActionState`/`undo`).
- Метод RPC `host/exec`: in-sandbox агент предлагает команду → host `dispatch_action` (`classify` → `(RiskTier×autonomy)` → approval/PolicyDefault) → **на Approve host шлёт обратно «исполни» → команда исполняется ВНУТРИ песочницы** (см. §5.2, КЛЮЧЕВАЯ инверсия vs наивный «shell на хосте»).
- env-scrub фактически реализован (пустое окружение + allow-list, §5.4) — `THREAT_MODEL §T7`-блокер закрыт.
- Новые таргеты дефолтят в `HardBlocked` (без `shell_enable`) / `Confirm` (с `shell_enable`), НИКОГДА не `Auto`; тест форсирует именно SAFE-решение (§5.3).

Пререквизиты Фазы-3 (гейт `THREAT_MODEL §4`): (1) Фаза-2 каркас смержена и работает; (2) этот THREAT_MODEL принят владельцем; (3) env-scrub реализован и протестирован; (4) `shell_enable`-гейт есть; (5) явный owner-approval (§12).

---

## 3. Выбранная архитектура + ПОЧЕМУ

### 3.1 Решение

**Эфемерный per-run rootless-Podman-контейнер** (база — репозиторный `Dockerfile`) с `--network=none`. Единственный сетевой путь — host-side `GuardedProxy` поверх bind-смонтированного AF_UNIX-сокета, который ре-эмитит каждый вызов через СУЩЕСТВУЮЩИЙ `GuardedClient`. Vault `:ro`. Все vault-записи и (Фаза-3) авторизация host-команд — typed RPC sandbox→host; `classify`/`RiskTier`/`PolicyDefault`/`undo`/audit остаются host-side и авторитетны. Фаза-3 shell **исполняется ВНУТРИ песочницы** после host-апрува.

Изоляционная единица — **per-run эфемерный контейнер** (`AgentRun` = граница изоляции, зеркалит `RunCtx::run(run_id)`). `podman run --rm` → ровно один `run_agent_loop` до терминала → контейнер исчезает.

### 3.2 ПОЧЕМУ — vs отвергнутые альтернативы

**Runtime: rootless Podman (выбран) vs Docker vs bubblewrap vs microVM.**
- Docker отвергнут: root-демон противоречит local-first single-owner и существующему non-root-образу (uid 10001).
- Podman выбран как ПЕРВИЧНЫЙ runtime, т.к. (а) `Dockerfile` уже задекларирован «база Фазы-2 Podman-песочницы» и Podman argv-совместим с Docker → `sandbox_run_plan` — близкий клон существующего `docker_plan` (`cli/service.rs`); (б) crun не требует демона.
- **bubblewrap (bwrap)** — серьёзная лёгкая альтернатива (нет демона/реестра/CNI, меньше DEPENDENCY-поверхность). Рассмотрена и **отложена в SANDBOX-7** (опциональный runtime-бэкенд), НЕ в v1: (1) экономит зависимость, но НЕ kernel-attack-поверхность (тот же userns); (2) требует hand-rolled seccomp (deny-list fail-open по природе — для adversarial-LLM нужен default-deny allow-list) + ручную fd-дисциплину (утечка rw-fd = тихий escape) + ручной `--clearenv`. Сначала используем vetted-дефолты Podman, потом (если ops-стоимость Podman неприемлема) добавляем bwrap-бэкенд за тем же `SandboxBackend`-трейтом.
- microVM (Firecracker/gVisor) — будущий шаг жёсткой изоляции (`THREAT_MODEL §3`), слишком тяжёл для десктоп-сайдкара сейчас.

**Изоляционная единица: per-run эфемерная (выбрана) vs warm-pod (persistent).**
- warm-pod амортизирует cold-start, НО долгоживущий процесс = больше attack-residue (cross-run-утечка memory/tmpfs/env/fd), а главное — **per-run env-scrub в warm-pod бесполезен против /proc-чтения резидентного agentd той же uid в той же pid-ns** (критика persistent-sandbox, дыра #3). Per-run эфемерность даёт чистое состояние, тривиальный teardown (`--rm`), kill = `podman kill`, и 1:1-маппинг на `run_id`/`requeue_stale_running`. Cold-start-латентность (секунды) приемлема: она складывается с уже-медленным V100-warmup, а scheduler-джобы агента не интерактивны.

**Locus исполнения shell (Фаза-3): ВНУТРИ песочницы (выбран) vs на хосте.**
- Это КЛЮЧЕВАЯ правка относительно наивного ephemeral-podman-дизайна (критика, дыра #1). Если бы shell исполнялся НА ХОСТЕ через `host/exec` RPC, `--network=none`/cap-drop/read-only НЕ контейнерили бы его вовсе: `['sh','-c','cat ~/.ssh/id_rsa | curl …']` бежал бы с ambient-полномочиями agentd, в обход `GuardedClient`. Поэтому: **РЕШЕНИЕ** (classify→approval) host-side, **ИСПОЛНЕНИЕ** in-sandbox. Это единственный способ, которым OS-граница реально контейнерит ту самую capability, ради которой песочница существует, И это превращает env-scrub из fail-open denylist в fail-closed пустое окружение.

**Egress: `--network=none` + host-side GuardedProxy (выбран, единогласно во всех трёх дизайнах).** Делает chokepoint ФИЗИЧЕСКИ неизбежным (не lint-enforced). Переиспользует `GuardedClient`/`EgressPolicy`/`check_resolved_ips`/`RunCtx` verbatim host-side; in-sandbox — только транспорт-шим, не второй клиент.

---

## 4. Egress: `--network=none` + GuardedProxy

### 4.1 Backbone

Контейнер бежит `--network=none` → у него НЕТ NIC, нет маршрута, нет резолвера. `connect()` к любому реальному хосту внутри = ошибка ядра. Сетевая capability восстановлена ИСКЛЮЧИТЕЛЬНО через host-side `GuardedProxy`, достижимый по AF_UNIX-сокету, bind-смонтированному в контейнер.

In-sandbox процесс получает `ProxyGuardedClient` — шим, реализующий ТУ ЖЕ поверхность вызова, что нужна провайдерам (`get`/`post_json` с `url`+`EgressFeature`+`RunCtx`), но вместо открытия сокета фреймит запрос как typed RPC-строку в proxy-сокет. **`ProxyGuardedClient` НЕ конструирует реальный `reqwest::Client`** (CI-grep-линт, §8.3).

Host-side `GuardedProxy` — ЕДИНСТВЕННОЕ, что когда-либо зовёт реальный `GuardedClient::post_json/get`. Значит КАЖДЫЙ сетевой вызов песочницы проходит неизменённый chokepoint: `EgressPolicy::check` (metadata→offline→per-feature opt-in→allowlist/private) → `check_resolved_ips` (DNS-rebind/SSRF, pin проверенного IP) → durable `egress_audit` write-before-act с `run_id`.

### 4.2 Протокол AF_UNIX (переиспользует AGENT-CONNECT framing)

Отдельный сокет от control-сокета (см. §6). Кадрирование = **line-delimited JSON** (`agent/connect/afunix.rs`: `RpcMessage`, `MAX_LINE_BYTES = 1<<20`, `MAX_CONSECUTIVE_MALFORMED`, skip-malformed). Хардненинг сокета — `harden_socket_perms` (0600) + `prepare_socket_path` (non-socket refusal).

Методы egress-семейства (ровно два typed-верба — НЕ сырые сокеты/fd, туннель произвольного трафика невозможен):

```
→ egress/post  { feature: "chat"|"embed"|"web", url: string, body: <json> }
→ egress/get   { feature: "chat"|"embed"|"web", url: string }
← { status: u16, headers_subset: {…}, body: <bytes|chunked> }
← RpcError    { code, message }   // sanitized: без путей/хоста/токенов (agent-connect §13)
```

Стрим chat-ответа проксируется как chunked body обратно по UDS (отдельные кадры). `headers_subset` — белый список заголовков ответа (content-type, etc.), не всё.

### 4.3 Анти-подмена, fail-closed — инварианты протокола

1. **`run_id` штампует ХОСТ, игнорируя клиентское значение.** `GuardedProxy` сконструирован с `RunCtx::run(run_id)` этого прогона. In-sandbox сторона НЕ может подделать другой `run_id` (корреляция audit идентична сегодняшней).
2. **Хост назначения берётся ТОЛЬКО из `url`-поля, парсится host-side через `reqwest::Url`** (как в `authorize`). НЕ доверяем никаким in-pod Host-header / absolute-URI / CONNECT-target. `GuardedClient.check` ключуется на той же host-строке, что и реальный fetch (нет desync `check` vs `connect` — critique persistent-sandbox, дыра #1). Это намеренно НЕ HTTP-forward-proxy (где живёт SSRF), а typed JSON-RPC: хост детерминированно извлечён из `url`.
3. **`EgressFeature` валидируется host-side против allow-set ПРОГОНА.** Песочница может запросить лишь подмножество, разрешённое для run (см. §5.6 — по умолчанию `Chat`+`Embed` для модельного эгресса; `Web` — ВЫКЛ для песочных прогонов по умолчанию). `Probe` и `NewsFeed` НЕ запрашиваемы из песочницы (`Probe` — внутренняя проба ядра; `NewsFeed` — не агентская). `FromStr` уже отвергает неизвестные строки; неизвестная фича → `RpcError`.
4. **«Most-restrictive» при сомнении = deny, НЕ «самая мягкая фича».** Важно: `Chat`/`Embed`/`Probe` ДОПУСКАЮТ приватные/LAN-хосты (`denies_private()=false`, local-first). Поэтому over-broad/неизвестный feature-hint НЕ клампится в `Chat` (это открыло бы LAN-SSRF), а **отвергается** (`RpcError`). Песочный эгресс получает ровно объявленный run'ом feature и ничего шире (critique persistent-sandbox, дыра #2 / fail-closed-gap).
5. **Политика перечитывается на КАЖДЫЙ forwarded-запрос** (хост зовёт `GuardedClient::check` заново — он уже так делает). `GuardedProxy` НЕ кэширует per-connection grant'ы: kill-switch/allowlist-revoke/feature-off вступают немедленно (нет stale-grant fail-open).
6. **`SO_PEERCRED`/uid-check на proxy-listener:** только спавненный контейнер (его uid) может драйвить `egress/*` и `host/*`. Сокет в per-run-каталоге, 0600, teardown на `--rm`. **✅ Реализовано:** `agent/connect/afunix.rs::peer_uid` (`getsockopt(SO_PEERCRED)`, Linux, fail-closed) + `sandbox/runner.rs::peer_authorized`-гейт в accept-пути egress/act/event (будущий `serve_exec` обязан обернуться тем же гейтом). Defense-in-depth ПОВЕРХ 0600-сокета + 0700-каталога.
7. **Fail-closed-наследие `GuardedClient`:** poisoned policy-lock → deny; `BadUrl` → audited denial + `RpcError`; пустой резолв → typed denial; metadata/rebind → denied+audited. Всё verbatim из `net/mod.rs`.

### 4.4 Размещение сокета — анти-footgun

Существующий `CONTAINER_SOCKET = /vault/.nexus/agentd.sock` биндит control-сокет ВНУТРИ vault. Для песочницы это нельзя наследовать: vault теперь `:ro` bind. Proxy-сокет (и control-сокет) живут в **ОТДЕЛЬНОМ per-run host-каталоге** (`<runtime_dir>/nexus-run-<run_id>/`), bind-смонтированном в `/run/nexus/` (НЕ под vault). Render-тест ассертит, что proxy-bind — distinct mount, НЕ под `:ro`-vault-путём, и каталог writable только хостом (critique ephemeral-podman, дыра #4).

---

## 5. Host-actuator RPC + расширение ActionTarget/classify/RiskTier/undo (Фаза 3)

### 5.1 Vault-записи (Фаза 2, уже host-side)

Vault `:ro` → агент ЧИТАЕТ заметки/скиллы локально (быстро, без RPC). ВСЕ записи (3 таргета Фазы C) идут host-side как typed RPC:

```
→ host/act { action: <Action>, run_id }     // run_id штампует хост (§4.3)
← { state, undo_handle?, diff_summary? } | RpcError
```

Host запускает НЕИЗМЕНЁННЫЙ `dispatch_action` (`actuator/orchestrate.rs`): `classify` → `(RiskTier×autonomy)`-матрица → blast-radius `TokenBucket` → kill-switch (`agent_paused`) → `apply_action` (`resolve_vault_path_for_write` canonicalize/symlink-рубеж → drift → ledger write-before-act → snapshot → atomic_write) → производит `UndoHandle` (Snapshot/Trash). `undo_run(run_id)` работает идентично. Песочница НЕ пишет vault напрямую — confused-deputy-путь ограничен ровно сегодняшними lexical (`classify`) + canonicalize (`apply`) рубежами (контейнер ничего не добавляет к vault-WRITE-containment, и это ОК — vault-READ был не угрозой; угроза — exec, см. ниже).

### 5.2 Host-команды (Фаза 3) — `host/exec`, исполнение ВНУТРИ песочницы

```
→ host/exec { action: ShellRun{argv, cwd_rel} | ProcessSpawn{…} | GitOp{op}, run_id }
← phase 1: { decision: "approved" | "rejected" | "hardblocked", … }
   on "approved": host шлёт обратно сигнал «исполни» по тому же RPC →
← phase 2: { exit_code, stdout_tail, stderr_tail, undo_handle? } | RpcError
```

Поток (КЛЮЧЕВАЯ инверсия — решение host, исполнение sandbox):
1. In-sandbox агент эмитит tool-call → `host/exec { action }`.
2. **Host** запускает `dispatch_action`: `classify` (HardBlocked/Confirm — НИКОГДА Auto для exec) → `(RiskTier×autonomy)` → под headless `PolicyDefault` Confirm = auto-DENY; в режиме коннектора Confirm = `Proposal`-событие → человек `agent/approve`.
3. Только на **Approve**: host пишет ledger write-before-act (intent) + шлёт «исполни» обратно в песочницу. **Команда исполняется ВНУТРИ `--network=none` контейнера**: vault `:ro`, cap-drop, no-new-privileges, no NIC, **пустое+allow-list окружение** (§5.4), в per-run scratch-tmpfs (`cwd_rel` резолвится относительно него или vault-`:ro`).
4. exit/stdout-tail/stderr-tail возвращаются → host финализирует ledger + (где возможно) `UndoHandle`.

Почему так: джейлбрейкнутая модель, эмитнувшая `rm -rf` или reverse-shell, упирается в EROFS / ENETUNREACH / cap-deny на УРОВНЕ ЯДРА внутри песочницы, а не в Rust-if на хосте, который можно было бы «уговорить» и который бежал бы с полными правами хоста.

### 5.3 Расширение `ActionTarget` — exhaustive-match fail-closed сохранён И усилен

Новые варианты добавляются в `actuator/action.rs::ActionTarget` (ровно туда, куда указывает существующий комментарий: «когда появится сэндбокс под shell/web (Фаза 3), новые варианты добавятся ЗДЕСЬ»). Exhaustive-match БЕЗ `_ =>` (тест `adding_variant_breaks_match`) **компиляционно** заставит дописать ветки в `classify` (`actuator/classify.rs`), в `ActionState`/`can_transition_to` (`actuator/mod.rs`), в `tool_name`/`rel`, в `undo`.

**КРИТИЧНО (critique ephemeral-podman, fail-closed-gap):** exhaustive-match форсирует «ВЕТКА существует», но НЕ «ветка БЕЗОПАСНА» — контрибьютор мог бы написать `ShellRun{..} => RiskTier::Auto` и компилятор был бы доволен. Поэтому:
- Новые exec-таргеты `classify` в `Confirm`(с `shell_enable`) / `HardBlocked`(без него) — НИКОГДА `Auto`.
- Добавляется тест-инвариант `exec_targets_never_auto`: для ВСЕХ exec-вариантов при любом `ClassifyCtx` `classify != RiskTier::Auto`. Это форсирует SAFE-решение, не только НАЛИЧИЕ решения.
- Новый `BlockReason::SandboxUnavailable` (для не-Linux / песочница-выкл) и `BlockReason::ShellDisabled` (для `shell_enable=false`). На не-Linux exec-таргеты `classify` → `HardBlocked(SandboxUnavailable)` (block by-construction, не by-omission — §9).

**`GitOp` НЕ разбивается на «read-ops Auto».** git не партиционируется чисто на безопасный read vs write: `git -c core.pager=<cmd>`, `core.fsmonitor`, hooks, submodule-URL дают exec/write. Все `GitOp` дефолтят `Confirm`/`HardBlocked`, никогда `Auto` (critique bubblewrap, fail-closed-gap).

### 5.4 env-scrub — fail-closed пустое окружение (не denylist)

`THREAT_MODEL §T3/§T7` блокер. **Реализация — fail-CLOSED:** окружение исполняемой команды строится из ПУСТОГО + явный allow-list (`--clearenv`-эквивалент: контейнер получает только заданные `--env` переменные). НЕ denylist (`KEY/TOKEN/SECRET/…`) — denylist fail-OPEN (секрет в креативно-названной переменной / `HOME` утечёт).

Allow-list собирается из: (а) фиксированного безопасного набора (`PATH`, `LANG`, `HOME`=scratch-tmpfs); (б) per-skill `env_passthrough` из SKILL.md (явно объявленные скиллом). Поскольку исполнение ВНУТРИ песочницы (§5.2), хост-процесс agentd НЕ передаёт своё окружение в контейнер вообще — структурно fail-closed, а не best-effort-скруб хост-env. Render-тест ассертит, что `--env`-набор контейнера = только allow-list (НЕ содержит хост-секретов).

### 5.5 Undo

Vault-таргеты: `UndoHandle::{Snapshot, Trash}` как сегодня. Exec-таргеты: где обратимость возможна (`GitOp` — записать pre-op ref), иначе действие помечается «необратимым» в diff и НИКОГДА не `Auto` (только `Confirm` с явным флагом необратимости). `undo_run(run_id, canon_root, ledger)` walk'ает ledger по `run_id` — переиспользуется verbatim.

### 5.6 Per-run egress-бюджет + Web OFF по умолчанию (анти-эксфильтрация)

Guarded-путь сам по себе — exfil-канал для untrusted-агента с web-доступом (§1.2). Митигации (в Фазе-2 каркасе, т.к. relevant и без exec):
- **`Web` ВЫКЛ по умолчанию для песочных прогонов.** Allow-set прогона = `{Chat, Embed}` (модельный эгресс) если иное явно не включено. `Web` для песочного прогона — отдельный opt-in (`ai.sandbox_web_enabled`, default-false).
- **Per-run egress-бюджет**: байт/запрос-кэпы (`sandbox.egress_byte_cap`, `sandbox.egress_req_cap`). При превышении — `GuardedProxy` возвращает `RpcError` (fail-closed), прогон продолжается без эгресса. Кэпы в audit-логе.

---

## 6. Control-plane (переиспользование agentd + AGENT-CONNECT)

Host оркестрирует песочный прогон, переиспользуя композиционный корень agentd + AGENT-CONNECT. Новый host-компонент **`SandboxRunner`** — `scheduler::JobHandler`, аналог `AgentRunHandler` (`agent/job.rs`). На queued `agent_run`:

1. Строит run-spec (goal, autonomy, model cfg) — как `AgentRunHandler`.
2. Создаёт per-run host-каталог сокетов (`<runtime_dir>/nexus-run-<run_id>/`, НЕ под vault, §4.4).
3. Стартует host-side `GuardedProxy` + (Фаза-3) host-actuator RPC-сервер, забинденные на сокеты этого каталога, с ТЕМИ ЖЕ хендлами, что у `ConnectDeps`/`AgentRunHandler`: `GuardedClient`, `EgressAudit`/ledger, `DecisionSource`=`PolicyDefault` (headless), canon-root, `agent_paused` Arc, `RunCtx::run(run_id)`.
4. `podman run --rm` контейнер с bind-смонтированными сокетами + spec (через env/stdin) + vault `:ro`.
5. Стримит `AgentEvent` обратно: in-sandbox loop эмитит события на свой proxy-side транспорт → host форвардит на существующий connect event-stream (`agent/event` → ACP-chunk) → desktop UI видит Proposal/Diff/токены без изменений.

Внутри контейнера резидентный `nexus-agentd` хостит `ConnectAgentHandler` поверх control-сокета (тот же `serve_unix`-accept-loop, тот же `ConnectAgentHandler`/`ConnectDeps`/`dispatch`, те же `MAX_LINE_BYTES`/0600). Start/stop/cancel/approve/control — существующие AGENT-CONNECT-верба (`initialize`/`agent_run`/`agent_cancel`/`agent_approve`/`agent_control`):
- `agent_approve` кормит host-side `DecisionSource` (human-in-the-loop Confirm выживает).
- `agent_control(pause)` флипает shared `agent_paused` Arc (durable в `agent.json`, переживает рестарт), который `dispatch_action` перечитывает per-step; зеркалится в `podman pause` контейнера (OS-switch и in-process-switch согласованы).
- Cancel = `podman kill` + существующий cancel-AtomicBool по proxy.
- Crash-recovery: мёртвый контейнер оставляет `agent_runs`-строку `running` → stale → `requeue_stale_running` TTL → НОВЫЙ контейнер. Идемпотентность vault-write обеспечивает существующий drift/snapshot; для эгресса — idempotency не требуется (re-egress audited заново).

Композиция остаётся: при `sandbox_enabled=false` запускается `AgentRunHandler` (in-process, как сегодня); при `true` — `SandboxRunner`. Контейнер заменяет ТОЛЬКО call-site `run_agent_loop`; всё вокруг (ledger, decision, audit, kill-switch, undo) — host-side как построено.

---

## 7. default-OFF / fail-closed

### 7.1 Флаги конфига (все default-OFF, в `local.json`)

| Флаг | Default | Смысл |
|------|---------|-------|
| `ai.sandbox_enabled` | `false` | Мастер-свитч Фазы-2. OFF → агент бежит in-process через `AgentRunHandler`, ZERO изменений поведения. |
| `ai.shell_enable` | `false` | Гейт Фазы-3 host exec-таргетов. Требует `sandbox_enabled` И Linux. |
| `ai.sandbox_web_enabled` | `false` | Разрешить `EgressFeature::Web` песочным прогонам (§5.6). |
| `sandbox.egress_byte_cap` / `sandbox.egress_req_cap` | разумный кэп | Per-run egress-бюджет (§5.6). |
| `ai.agent_actuator_enabled` | `false` | Существующий — vault-актуатор включён вообще. |

### 7.2 Поведение

- **`sandbox_enabled=OFF`**: Podman не вызывается, proxy-сокет не создаётся. Поведение байт-в-байт сегодняшнее (vault-only in-process, default-OFF актуатор).
- **`sandbox_enabled=ON, но недоступна`** (Podman нет / не-Linux / образ не собран / `podman run` упал / сокет не забиндился): **FAIL-CLOSED** — прогон `error` с внятной причиной, БЕЗ тихого in-process fallback (silent fallback обнулил бы изоляцию, на которую владелец явно подписался). Это намеренно: ошибка ГРОМКАЯ.
- **`shell_enable=OFF`** (но sandbox ON): exec-таргеты `classify` → `HardBlocked(ShellDisabled)`; `host/exec` RPC инертен.
- **`GuardedProxy` fail-closed**: poisoned lock → deny; `BadUrl` → audited denial; неизвестная фича/метод → `RpcError`; over-broad feature → deny (НЕ кламп в мягкую); egress-бюджет исчерпан → `RpcError`.
- **`PolicyDefault` headless**: Confirm-тир auto-DENY → даже скомпрометированный in-sandbox агент ничего не применяет без host-side approval.

### 7.3 Не-Linux

`ai.sandbox_enabled` honored только на Linux (`#[cfg(target_os="linux")]` + runtime Podman-probe). На macOS/Windows флаг структурно инертен; exec-таргеты `classify` → `HardBlocked(SandboxUnavailable)`. Подробно §9.

---

## 8. CI-verifiability БЕЗ Podman

Два-три тира, зеркалят существующий стиль net/actuator-тестов (pure + tokio-integration, без реальной сети — `net/mod.rs` использует loopback `serve_once` + `FixedResolver`).

### 8.1 Tier-1 — Podman-free unit/render (бежит на КАЖДОМ CI-хосте, вкл. macOS/Windows)

- **`sandbox_run_plan(cfg)` → argv-вектор `podman run`** (клон стиля `docker_plan`-тестов в `service.rs`). Ассертим, что argv содержит: `--network=none`, `--read-only`, `--tmpfs /tmp`, `--cap-drop=ALL`, `--security-opt=no-new-privileges`, `--pids-limit`, `--memory`/`--cpus`-кэпы, `--userns=keep-id`, vault `:ro` bind, proxy-socket bind. **И ассертим, что proxy-socket bind — DISTINCT mount, НЕ под vault-путём** (§4.4). В SANDBOX-1 (нет ещё env-передачи) ассертим НЕГАТИВ: argv БЕЗ `-e`/`--env` (хост-окружение не пробрасывается, секреты не утекают). Позитивный ассерт «`--env`-набор = allow-list» (§5.4) приходит вместе с env-scrub в **SANDBOX-6a** (когда исполнение в песочнице реально передаёт env). NB: render-тест проверяет НАМЕРЕНИЕ (argv корректен), не runtime-enforcement флага — последнее только в Tier-2.
- **`GuardedProxy`-тесты поверх in-memory `ChannelTransport`** (существующий `channel_pair`) с `GuardedClient::unchecked()`/`FixedResolver`: `egress/post` к denied-хосту → typed `RpcError` И durable `egress_audit`-denial-строка с `run_id` (переиспользуем `temp_db`/`durable_rows`-хелперы); proxy штампует `RunCtx::run(run_id)`, игнорируя клиентский `run_id`; over-broad/неизвестный feature → deny (НЕ кламп); хост извлекается из `url`-поля, не из подделанного Host-header; egress-бюджет исчерпан → `RpcError`.
- **Host-actuator RPC-тесты**: `host/act` с HardBlocked-таргетом → error, файл НЕ записан (переиспользуем `orchestrate.rs` `setup()`/temp-vault); `host/exec` при `shell_enable=false` → HardBlocked, инертен.
- **classify-exhaustiveness**: расширить `adding_variant_breaks_match`-зеркало новыми exec-вариантами; `exec_targets_never_auto` (§5.3) ассертит, что НИ ОДИН exec-таргет не `Auto` ни при каком `ClassifyCtx`; не-Linux → `HardBlocked(SandboxUnavailable)`.

### 8.2 Tier-2 — Podman-gated integration (Linux-runner, `#[ignore]` если `NEXUS_SANDBOX_IT≠1`)

Паттерн DEPLOY-4 `docker-build-smoke`, расширенный rootless-podman+netns (НОВАЯ CI-инфра — GH ubuntu-runner с subuid/subgid). Собрать образ, запустить один реальный `--network=none` контейнер, ассертить:
- (a) прямой `TcpStream::connect` ВНУТРИ → ENETUNREACH; тот же fetch через proxy-сокет → успех.
- (b) запись в vault-путь ВНУТРИ → EROFS; typed `host/act` note_create→undo round-trip на temp-vault.
- (c) полный прогон → host-side ledger-строка + `egress_audit`-строки коррелированы по `run_id`; `--rm` не оставляет контейнера.
- (Фаза-3) (d) `host/exec` approved-команда исполняется ВНУТРИ, видит EROFS на vault, ENETUNREACH без proxy; `--env` не содержит хост-секретов.

**Честность:** «`--network=none` honored» — runtime-гарантия Podman, проверяется ТОЛЬКО в Tier-2 (trust-in-Podman); Tier-1 доказывает корректность ФЛАГА, не runtime-enforcement. Tier-2, **трогающий `nexus-sandbox`/`SandboxRunner`/proxy, делается БЛОКИРУЮЩИМ merge-гейтом** на Linux-runner (не nightly-best-effort) — иначе containment-свойства уезжают непроверенными.

### 8.3 Линт «нет второго egress-пути»

`scripts/check-sandbox-egress.mjs` (sibling к `scripts/check-egress.mjs`, AC-EGR-1): ассертит, что in-sandbox `ProxyGuardedClient` НИКОГДА не конструирует реальный `reqwest`-клиент и что в sandbox-build-таргете нет `core_client_builder`/сырого `reqwest::Client::builder`. Durability-замок против интродукции второго un-guarded пути. Существующий `check-egress.mjs` остаётся в силе для host-стороны.

---

## 9. Cross-platform posture

Песочница — **LINUX-HOST-ONLY** фича: rootless Podman / user-namespaces / seccomp / `--network=none` / AF_UNIX bind-mount — Linux-примитивы (на macOS Docker Desktop сокет через virtiofs не пробрасывается, что уже отмечено в `Dockerfile`-комментарии и `cli/service.rs`).

Posture:
- `SandboxRunner` + host-actuator exec-таргеты — `#[cfg(target_os="linux")]` + runtime Podman-probe (`podman --version` + dry `--unshare`-smoke на старте). На не-Linux — компилируется в stub, репортит `SandboxUnavailable`.
- `ai.sandbox_enabled` на macOS/Windows структурно инертен. exec-таргеты `classify` → `HardBlocked(BlockReason::SandboxUnavailable)` (block **by-construction** — ветка в classify ЕСТЬ и возвращает HardBlocked, а не by-omission через отсутствие варианта). Джейлбрейкнутая модель на Mac структурно не получит host exec — она получает fenced `ToolError`, который может прочитать и перепланировать.
- Десктоп на macOS/Windows продолжает гонять vault-only актуатор in-process как Фаза-C (default-OFF), ЛИБО подключается по AGENT-CONNECT к Linux-agentd, который ДЕЛАЕТ песочницу (рекомендованная multi-device-топология: Mac-десктоп ↔ Linux-риг с песочным agentd — зеркалит `nexus deploy remote`/риг 192.168.0.31).
- Никакого тихого capability-downgrade: на не-Linux новые таргеты НИКОГДА не достигают `Auto`/`Confirm` — они `HardBlocked` платформенным гейтом, и exhaustive-match форсирует явную ветку.

---

## 10. Дельты THREAT_MODEL

### 10.1 Что закрывается

**T7 (agent-shell/host-execution — сейчас 🔭 Фаза-3).** Песочница — реализация containment, который `T7` требует. Джейлбрейкнутая LLM, эмитнувшая вредный shell/process/git: (а) host-side `dispatch_action`→`classify`(HardBlock/Confirm)→`PolicyDefault` auto-DENY / human-approval решает, исполнять ли ВООБЩЕ; (б) даже approved-код исполняется ВНУТРИ `--network=none` rootless-Podman с cap-drop/no-new-privileges/read-only-rootfs/vault-`:ro`/пустым-env → не может эксфильтрировать (нет NIC кроме typed-proxy), эскалировать (нет caps/privs), персистить (эфемерный `--rm`), портить vault (read-only) или ledger/DB (не смонтированы). env-scrub реализован fail-closed (§5.4) — `§T7`-блокер закрыт.

**T6 (prompt-injection → актуатор).** Отравленная заметка/web/скилл, убедившая модель писать/исполнять, всё равно бьётся в host-side гейт + approval; marker-fencing остаётся defense-in-depth, но больше НЕ последняя линия — OS-граница теперь реальная линия.

**T2/SSRF/DNS-rebind.** Усилено: песочница не имеет резолвера вовсе; 100% её эгресса ре-проверяется host `GuardedClient` (`check_resolved_ips`, metadata-блок, pin-IP), хост извлечён из `url` host-side (нет request-smuggling/desync), нет альтернативного сокета.

**T8 (connector auth).** Control-сокет — 0600 AF_UNIX + `SO_PEERCRED`/uid-check (только спавненный контейнер драйвит). Согласуется с `agent-connect.md §11`.

### 10.2 Остаточные риски (честно, согласуется с `THREAT_MODEL §3`)

- ⚠ **kernel/Podman/crun/userns-escape** → выход на uid оператора (полные права владельца). Намеренный trade Фазы-2; жёсткая изоляция = будущий microVM (Firecracker/gVisor).
- ⚠ **READ-then-exfil через санкционированный эгресс** (§1.2). Митигировано (Web OFF по умолчанию, egress-бюджет), НЕ устранено: `GuardedClient` не инспектирует СОДЕРЖИМОЕ тела. Для confidential-vault — owner-aware (§12, вопрос про subtree-mount).
- ⚠ **Кооперативно-ошибающаяся модель** в пределах Auto-тир-бюджета (token-bucket) — by-design (владелец доверяет Auto-tier vault-записям). Песочница поднимает планку для adversarial/exec-кейса, не для этого.
- ⚠ **Новый trusted-surface**: `GuardedProxy` + host-actuator RPC-сервер. Баг (клиент-supplied run_id, over-broad `host/exec`, desync хоста) серьёзен → строго typed-верба (нет raw fd/socket), adversarial-ревью (§11), invariant-тесты (§8).

THREAT_MODEL обновляется: `T7` 🔭→ ✅(за `shell_enable`-гейтом) по завершении Фазы-3; `§3`-строка про env-scrub 🔭→✅; добавить `BlockReason::SandboxUnavailable`/`ShellDisabled` в перечень.

---

## 11. Фазовый роадмап SANDBOX-1..N

Каждый срез — мелкий, мерджабельный, default-OFF, CI-verifiable. После КАЖДОГО major — adversarial-ревью диффа перед мержем (`feedback_adversarial_review_after_major`).

**Фаза-2 каркас (host-actuator НЕТ):**

- **SANDBOX-1** [S, Tier-1] — `SandboxConfig` + `sandbox_run_plan(cfg)` (клон `docker_plan` + хардненинг-флаги + per-run-имя/сокеты). `ai.sandbox_enabled`-флаг (default-false, инертен). Render-тесты (§8.1, argv-ассерты вкл. distinct-socket-mount + env-allowlist). **Пререквизит:** —. Без рантайма.
- **SANDBOX-2** [M, Tier-1] — `GuardedProxy` host-side (`egress/get`/`egress/post`) поверх AF_UNIX-framing AGENT-CONNECT; `ProxyGuardedClient` in-sandbox-шим. run_id-штамп host-side, host-из-`url`, feature-allow-set, over-broad→deny, per-run egress-бюджет. `ChannelTransport`-тесты + durable-audit-ассерты + `check-sandbox-egress.mjs` линт. **Пререквизит:** SANDBOX-1.
- **SANDBOX-3** [M, Tier-1] — `host/act` RPC для 3 vault-таргетов Фазы-C → host-side `dispatch_action` (vault `:ro` в песочнице). RPC-тесты (HardBlocked→no-write, undo round-trip). **Пререквизит:** SANDBOX-2.
- **SANDBOX-4** [M, Tier-1] — `SandboxRunner` (`JobHandler`-sibling): proxy+control-сокеты, стрим `AgentEvent` через AGENT-CONNECT, cancel/pause/approve/requeue-маппинг. fail-closed при недоступности (НЕ in-process fallback). `#[cfg(target_os="linux")]` + Podman-probe; не-Linux stub. **Пререквизит:** SANDBOX-2, SANDBOX-3.
- **SANDBOX-5** [S, Tier-2] — Podman-gated integration-джоба (ENETUNREACH/EROFS/ledger+audit-корреляция/`--rm`), БЛОКИРУЮЩАЯ на Linux-runner для изменений `nexus-sandbox`. **Пререквизит:** SANDBOX-4. **Завершает Фазу-2 каркас** (THREAT_MODEL `§4 Фаза-2: sandbox-каркас` ✅).

**Фаза-3 host-actuator (OWNER-GATED, требует §12 greenlight):**

- **SANDBOX-6a** [M, Tier-1] — env-scrub fail-closed (пустое+allow-list, per-skill `env_passthrough`); `ai.shell_enable`-флаг (default-false); `BlockReason::ShellDisabled`/`SandboxUnavailable`. Render-тест env-allowlist. **Пререквизит:** SANDBOX-5 + owner-greenlight. **Закрывает `§T7`/env-scrub-блокер.**
- **SANDBOX-6b** [M, Tier-1] — новые `ActionTarget`: `ShellRun`/`ProcessSpawn`/`GitOp`; ветки `classify` (Confirm/HardBlocked, НИКОГДА Auto) + `ActionState`/`undo` + `exec_targets_never_auto`-инвариант + GitOp-не-Auto. **Пререквизит:** SANDBOX-6a.
- **SANDBOX-6c** [M, Tier-1+2] — `host/exec` RPC: host-решение (§5.2) + **исполнение ВНУТРИ песочницы** (per-run scratch-tmpfs, пустой env). Tier-2: approved-exec видит EROFS/ENETUNREACH, env без секретов. **Пререквизит:** SANDBOX-6a, SANDBOX-6b. **Завершает Фазу-3** (THREAT_MODEL `T7` ✅ за `shell_enable`).

**Опционально (после v1):**

- **SANDBOX-7** [L] — `bwrap`-runtime-бэкенд за `SandboxBackend`-трейтом (default-deny seccomp allow-list + `--clearenv`+setenv-allowlist + runtime fd-table-ассерт + vault read-via-RPC/subtree-mount). Для ops-окружений без Podman. **Пререквизит:** SANDBOX-5.
- **SANDBOX-8** [XL] — microVM (Firecracker/gVisor) для жёсткой изоляции против kernel-escape. **Пререквизит:** SANDBOX-6c. Owner-gated.

---

## 12. Открытые вопросы для владельца (owner-решения, НЕ изобретаем)

1. **Greenlight Фазы-3 (host-actuator).** Фаза-2 каркас (SANDBOX-1..5) строится автономно (default-OFF, в рамках мандата). SANDBOX-6a..6c (новые exec-таргеты) — НОВАЯ security-граница, требует явного owner-approval + принятия THREAT_MODEL (`THREAT_MODEL §4 гейт Фазы-3`). Без него Фаза-3 не начинается.
2. **Confidential-vault READ-exfil (§1.2 / §10.2).** Допустимо ли давать песочному прогону vault `:ro` ЦЕЛИКОМ, или нужен subtree-mount только run-relevant путей / read-via-RPC (с allowlist-путей)? Полный `:ro` проще и быстрее, но даёт agent'у read всей confidential-базы (BNPL/WB-смежные заметки). Owner-trade приватность vs производительность/простота.
3. **`Web`-эгресс для песочных прогонов.** По умолчанию `ai.sandbox_web_enabled=false` (§5.6). Подтвердить, что web-инструменты агента в песочнице — отдельный явный opt-in (а не наследуют общий web-тоггл), учитывая что web = самый широкий exfil-канал.
4. **Podman как ops-зависимость рига.** Память владельца отмечает, что риг 192.168.0.31 RAM-стартован и без Docker/Rust. Подтвердить целевой Linux-host для песочного agentd (риг с rootless-Podman? отдельный Linux-VPS? — последний пересекается с отложенным деплой-провиженингом). От этого зависит, где реально живёт Tier-2 CI-runner и прод-песочница.
5. **`pids-limit`/`memory`/`cpus`-кэпы** — конкретные значения по умолчанию. Предложение: консервативные (`--pids-limit=512`, `--memory=2g`, `--cpus=2`), но owner может знать профиль нагрузки целевого хоста.
