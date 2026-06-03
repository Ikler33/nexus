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
