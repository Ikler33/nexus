# CLAUDE.md — Nexus (правила проекта, грузятся каждую сессию)

Nexus — Obsidian-форк (Tauri 2 + Rust + React) с локальным LLM/RAG, local-first, плагинная экосистема, i18n RU/EN. **Не изобретай архитектуру — реализуй по спеке.** Полный метод и план Фазы 0 — в `prompts/DEV-PROMPT.md`.

## Источники истины (читай ПЕРЕД кодом)
1. `docs/architecture/ARCHITECTURE.md` — что/как; **раздел 0 (ADR) не пересматривать**.
2. `docs/acceptance/ACCEPTANCE.md` — критерии `AC-…` = основа тестов.
3. `docs/design/DESIGN.md` — UI/UX (токены, компоненты, состояния, a11y).
4. `docs/reviews/` — почему так и какие ошибки запрещены.

Перед любым срезом: прочитай релевантный раздел ARCHITECTURE + нужные `AC-…` + (для UI) DESIGN.

## Жёсткие правила (ADR — не менять молча)
- **ADR-001** Плагины — **JS-first** + host-broker; логика в Web Worker, редакторные расширения в main-контексте; WASM опц. для тяжёлых вычислений. **Не WASM-first.**
- **ADR-002** Безопасность — граница прав = **host-broker** (§7.4/§7.9), не iframe-sandbox; path-scoped permissions, identity по MessagePort, audit-log; **код плагинов не в git**; подпись с Фазы 2.
- **ADR-003** БД — **rusqlite + write-actor**, синхронные транзакции. Не sqlx.
- **ADR-004** Граф — источник истины **SQLite** (беклинки по `idx_links_target`); petgraph опц. кэш.
- **ADR-005** Провайдеры — раздельные **Chat/Embedding**; эмбеддер мультиязычный; cloud-fallback только chat и по opt-in.

Требуется отступить от ADR → **остановись и спроси человека**. Смена ADR = правка §0 + согласование.

## Антипаттерны (НИКОГДА)
- ❌ эмбеддинг файла целиком → ✅ по чанкам, 1:1.
- ❌ хардкод размерности `FLOAT[1024]` → ✅ из модели (`embedder.dim()`), переэмбеддизация при смене (§6.5).
- ❌ FTS5 поверх `files` → ✅ поверх `chunks` + триггеры; чистка usearch при удалении/реиндексации.
- ❌ `sqlx` async-замыкание-транзакция → ✅ rusqlite `conn.transaction` во write-actor.
- ❌ petgraph как источник истины → ✅ беклинки из SQLite.
- ❌ глобальный `listen('llm-stream')` → ✅ per-session `Channel`, финализация на `done`, `cancel`, ref-буфер + throttle.
- ❌ реакция только на `Modified` → ✅ `notify-debouncer-full` + ignore (`.nexus/.git/*.db*`) + reconcile по пути (сохранение `file_id`).
- ❌ `index.add_all(["*"])` → ✅ выборочный stage + secret-scan + sync-lock; git2 в `spawn_blocking`.
- ❌ доверять `pluginId` из payload / пускать vault-команды в iframe → ✅ identity по порту, capability-токен, `resolve_vault_path` (канонизация), `ai:complete {local_only}`.
- ❌ `tokio::timeout` как лимит WASM → ✅ `epoch_interruption`/fuel + `StoreLimits`.
- ❌ единый `currentFile` → ✅ модель групп/вкладок; контекст AI/backlinks из активной вкладки.
- ❌ `{{count}} files` без плюралов / строковая сортировка → ✅ плюралы `_one/_few/_many`, `Intl`, `Intl.Collator('ru')`.
- ❌ endpoints/ключи в git → ✅ `.nexus/local.json` (gitignore) + OS keychain; `*.url` валидируются (анти-SSRF).
- ❌ `invoke` вне `lib/tauri-api.ts` / хардкод цветов-отступов в UI → ✅ единая обёртка IPC; только design-токены; состояния empty/loading/streaming/error/offline.
- ❌ Live Preview в Фазе 0 → ✅ Фаза 0 — source-mode; Live Preview — отдельный эпик позже.

## Стек (зафиксирован)
Tauri 2 · Rust (rusqlite, usearch, git2, reqwest, tokio, tracing, pulldown-cmark, notify-debouncer-full) · React 19 + TS + Vite 6 · Zustand · CodeMirror 6 · sigma.js + graphology · i18next · CSS Modules + variables. Подмена компонентов — только с согласованием.

## Документация (на зелёном — обязательно)
Цикл среза: **реализация → тесты зелёные → обновить/написать доку → следующий срез**. Доку «вперёд» не пишем.
- `docs/architecture/` — спека (синхронизируй при уточнении; смена ADR — через §0).
- rustdoc (`///`) / TSDoc на публичных API; `docs/dev/<module>.md` на нетривиальные подсистемы (write-actor, indexer, watcher, broker, RAG).
- `docs/plugin-api/` синхронно с SDK; `CHANGELOG.md` (Keep a Changelog); `README.md` по процессу.
- **Отложил/урезал что-то осознанно → запиши в `docs/BACKLOG.md`** (единый реестр, не только в `## Дальше` доки и коммит). Закрыл пункт — вычеркни. Принцип «no silent caps».
- **Баг-фикс = регресс-тест (красный) → фикс → правка затронутой доки (и неверной доки!) → привязка к `AC-…`.**
- PR не готов, если код зелёный, а дока не обновлена.

## Верификация (каждый срез)
- Rust: `cargo build` · `cargo clippy -D warnings` · `cargo test`.
- Фронт: `tsc --noEmit` · `eslint` · `vitest`.
- UI: подними dev-сервер и проверь в превью (снимок/консоль/сеть) — не проси человека проверять руками.
- Блокирующие gate'ы: security-тесты (AC-SEC) и eval (AC-EVAL); perf-пороги (AC-PERF) где применимо.
- Отчитывайся фактами (что прошло/упало, вывод теста). «Готово» — только с доказательством.

## Рабочие конвенции
- Срез = маленький и вертикальный, привязан к `AC-…`. Сначала контракт + тест, потом реализация.
- IPC — только через `lib/tauri-api.ts`; фронт стартует на моках этих сигнатур (параллельно бэкенду).
- Ветка на срез; коммит/пуш — только по явной просьбе; не на дефолтной ветке.
- При двусмысленности — задай точечный вопрос с вариантами; не доделывай молча и не пересматривай ADR.
- DoD среза: релевантные `AC-…` + `AC-PR-1…6` + обновлённая дока.
