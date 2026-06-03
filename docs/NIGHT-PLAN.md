# Ночной автономный план (carte blanche)

> Пользователь дал полную автономию на ночь. Работать без подтверждений, сколько хватит лимитов.
> Крон на 03:33 возобновляет работу после обновления лимитов. Этот документ — источник плана и
> журнал прогресса. **Каждая крон-сессия: прочитай этот файл + `CHANGELOG.md` + `docs/BACKLOG.md`,
> определи, что уже сделано, продолжи со следующего невыполненного пункта.**

## Жёсткие правила
1. **Дисциплина среза:** реализация → тесты зелёные → дока → коммит на ветке → следующий. Не копить.
   - Rust: `source "$HOME/.cargo/env"; cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings && cargo test` (из `apps/desktop/src-tauri`).
   - Front: `pnpm exec tsc --noEmit && pnpm exec eslint . && pnpm exec vitest run && pnpm exec vite build` (из `apps/desktop`).
2. **Коммиты — локально, на ветках. НЕ пушить** (push — зона пользователя: classifier + workflow-scope). Текущая ветка работы: `phase1/12-multilingual-embedder` (в неё подтягивается PR #1). Новые фазы — свои ветки.
3. **Отложил/спорно → пиши в `docs/BACKLOG.md`** и иди дальше («no silent caps», правило в `CLAUDE.md`).
4. **ADR не менять молча.** Если всплыл конфликт с ADR/спекой — записать в раздел «NEEDS-DECISION» ниже и переключиться на другой пункт (не блокироваться).
5. **Источник истины:** `docs/architecture/ARCHITECTURE.md` (§0 ADR), `docs/acceptance/ACCEPTANCE.md` (AC-*), `docs/design/DESIGN.md`. Не выдумывать — реализовывать по спеке.
6. **Живые серверы** (для `#[ignore]`-тестов): chat Qwen `192.168.0.172:8080`; embed nomic `127.0.0.1:8081`; jina-code `127.0.0.1:8082`; bge-m3 `127.0.0.1:8083` (поднять в Ф1-12). Если сервер недоступен — пропустить живой тест, не падать.
7. **В конце каждого пункта** — обнови «Журнал» внизу (что сделано, коммит-хэш).

## Очередь работ (по приоритету)

### 1. Ф1-12 — мультиязычный эмбеддер bge-m3 (закрыть AC-EVAL-6) [В РАБОТЕ]
Модель `bge-m3-Q4_K_M.gguf` качается в `~/Documents/llm-models/`. Когда докачается (~418 МБ):
- Поднять `llama-server` с bge-m3 на **:8083** (`--embedding -ngl 99 --ctx-size 8192`, по образцу `start_servers.sh`), проверить `/health` + что эмбеддит (dim 1024).
- Быстрая проверка кросс-язычности: cosine(RU-запрос, EN-док) высокий.
- Перевести `apps/desktop/src-tauri/eval/baseline.json` → conditions на bge-m3 (model `bge-m3`, server `http://127.0.0.1:8083`, dim 1024); metrics временно занизить.
- Перепрогнать `cargo test eval::tests::live_eval_meets_baseline -- --ignored --nocapture` → снять реальные recall@8/nDCG/MRR (ожидаю закрытие 2 кросс-язычных кейсов → ~1.0).
- Поднять baseline до фактических значений; обновить `_measured`.
- Добавить bge-m3-блок в `~/Documents/llm-models/start_servers.sh` (:8083, персистентно); обновить `.nexus/local.json`-пример в доке на bge-m3; обновить `docs/dev/ai.md` + `docs/dev/eval.md`; вычеркнуть AC-EVAL-6 из BACKLOG.
- `default_prefixes("bge-m3")` уже → None (корректно). Коммит Ф1-12.

### 2. Crash-reconcile usearch (§5.1, BACKLOG)
На `open_vault`: файлы, чьи `chunks` есть в БД, но векторов в usearch нет (крах между commit и save) → переэмбеддить эти чанки (или пометить файлы на реиндекс). Тест: вставить chunks без векторов → reconcile добирает. Аккуратно с тем, что embedder может быть down.

### 3. Чат: виртуализация ленты (BACKLOG, DESIGN §«лента виртуализирована»)
`@tanstack/react-virtual` для ленты сообщений в `ChatView` при длинной истории. Тест рендера.

### 4. Фаза 2 — плагины/broker (ADR-001/002, ARCHITECTURE §7, AC-SEC-*) [БОЛЬШАЯ]
Следующая фаза роадмапа. Резать по срезам (свои ветки `phase2/NN`), по §7:
- Ф2-1: capability-broker (скелет) — манифест прав, path-scoped permissions, host-сторона брокера.
- Ф2-2: изоляция исполнения плагина (iframe/worker), типизированный мост host↔plugin.
- Ф2-3: `registerCommand(source:'plugin')` через существующий реестр; плагинные i18n-namespace.
- Ф2-4: SDK-типы `docs/plugin-api/` + min_api_version.
- Сверяться с REVIEW (блокеры по безопасности плагинов). КАЖДЫЙ срез — тесты + дока.

### 5. Прочее из BACKLOG (самодостаточное)
Реальный токенайзер чанкера (если у сервера есть `/tokenize`), CI-eval без сервера (кэш golden-эмбеддингов), throttle токенов чата, dedup-улучшения. Брать только то, что не требует решений пользователя.

## NEEDS-DECISION (НЕ делать ночью — записать и пропустить)
- **Реранкер**: сервер :8082 — jina-эмбеддер кода, НЕ cross-encoder с `/rerank`. Нужен реальный reranker-эндпоинт/модель — решение пользователя. (ADR-005 опц., eval-гейт готов.)
- **Suggest режим 2 (LLM-обоснование)**: дизайн UX (Channel-стрим) — согласовать.
- Любая смена ADR / стека / крупного UX — стоп, записать сюда.

## Журнал прогресса (дописывать)
- (старт ночи) PR #1 «Phase 1: AI Core» открыт (`phase1/12` → `main`). Rust 71+6 live, front 64 — зелёные. Eval baseline 0.8 (nomic), AC-EVAL-6 открыт. bge-m3 качается.
- ✅ **Пункт #2 — Crash-reconcile usearch (§5.1)**: `reconcile_vectors` в `scan_vault` + тест (`27f5e0c`). Rust 72 теста зелёные.
- ✅ **Ф1-12 — bge-m3 (AC-EVAL-6 закрыт)**: bge-m3 Q4_K_M докачан, сервер :8083 поднят (dim 1024), `start_servers.sh` обновлён. baseline.json → bge-m3; **eval recall@8=1.0, nDCG@8=0.883, MRR=0.848** (оба кросс-язычных кейса найдены, было 0.8/провал на nomic). Доки/BACKLOG/CHANGELOG обновлены. Коммичу.
- (крон 03:33) Окружение проверено: git чист, серверы :8081/:8082/:8083 живы.
- ⏭️ **Пункт #3 (виртуализация ленты чата) ОТЛОЖЕН** → BACKLOG: UI-скролл/виртуализация требует визуальной проверки человеком, ненадёжно автономно (jsdom не верифицирует скролл). Записано в раздел NEEDS-DECISION/BACKLOG.
- ✅ **Ф2-1 — Модель прав плагина (capability-broker security-ядро, ADR-002)**: `plugin/permission.rs` — `Permissions` + `check_scoped_permission` (path-scoped glob с `!`-deny, анти-traversal, net-allowlist, fail-closed) + 13 security-тестов; манифест расширен `permissions`. Rust 85 тестов. Ветка `phase2/01-capability-model`. Доки/CHANGELOG обновлены. Коммичу.
- ⏭ Следующее: **Ф2-2** (рантайм-брокер: сессии/порты/токены/audit/dispatch — но iframe/MessagePort = фронт, хуже автономно тестируется; начать с Rust-стороны брокера + audit-log) ИЛИ другой самодостаточный backend-пункт из BACKLOG. Перед стартом крон-сессии: `curl :8083/health`, при недоступности — `bash ~/Documents/llm-models/start_servers.sh`.
