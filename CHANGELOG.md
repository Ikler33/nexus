# Changelog

Все значимые изменения проекта документируются в этом файле.
Формат основан на [Keep a Changelog](https://keepachangelog.com/ru/1.1.0/);
проект придерживается [Semantic Versioning](https://semver.org/lang/ru/).

## [Unreleased]

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
