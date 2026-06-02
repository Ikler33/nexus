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
