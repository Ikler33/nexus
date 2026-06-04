# Граф ссылок (`src-tauri/src/graph` + backlinks-бар)

> Срез Ф0-6. **ADR-004**: источник истины графа — SQLite (НЕ petgraph). Локальный N-hop — Ф0-11.

## Беклинки (Rust)
- `get_backlinks(reader, path)` — кто ссылается на `path`. Запрос по индексу `idx_links_target`:
  `links JOIN files(source) WHERE target_id = (SELECT id FROM files WHERE path=?)`. Доли мс из
  page-cache, без petgraph/рассинхрона.
- `BacklinkEntry { sourcePath, sourceTitle, context, lineNumber }`; команда `get_backlinks(path)`.

## Бар (фронт)
- `BacklinksBar` (слот editor-bottom): фетчит беклинки активного файла при смене файла;
  состояния loading / empty / список; клик по источнику → `openFile`. Вне Tauri — мок.

## Тесты
- Rust: `backlinks_come_from_sqlite_with_context` (A,C → B; контекст; пустой случай).
- Фронт: бар показывает входящие ссылки / пустое состояние.

## Локальный граф (Ф0-11)
- Rust `get_local_graph(reader, center, hops)` — BFS по неориентированным связям до глубины
  `hops` из SQLite (без petgraph); рёбра — внутри полученного множества id. `GraphData {nodes, edges}`.
  Команда `get_local_graph(center, hops)`.
- Фронт: `GraphView` (sigma.js + graphology, **ленивый** chunk — §10). Раскладка — ForceAtlas2 в
  **Web Worker** (`layout.worker.ts` → `computeLayout`), не блокирует main-thread (**AC-PERF-6**).
  Клик по узлу → `openFile`. Открытие — команда `view.graph` (Cmd/Ctrl+G) / кнопка в шапке.
- **Чанкинг IN-запросов (V2.3, ревью A9):** узлы/рёбра/BFS-фронтир выбираются `IN (...)`-запросами с
  одним плейсхолдером на id. Супер-хаб (узел с десятками тысяч связей) превысил бы лимит bind-переменных
  SQLite (`too many SQL variables`) и уронил бы команду. Все IN-запросы чанкуются (`collect_in_chunks`,
  ≤900 на запрос); рёбра — одиночный `source IN (chunk)` + фильтр `target ∈ ids` в Rust вместо двойного
  IN. Результат полный (без обрезки). Тот же приём в `get_full_graph`.
- Тесты: Rust (N-hop расширяется по глубине, пустой центр, **супер-хаб 1000 связей не валит лимит**),
  фронт (`computeLayout` назначает координаты; мок `getLocalGraph`).
- **Dev-нюанс (vite optimizeDeps):** граф-зависимости (`graphology`/`sigma`/`graphology-layout-forceatlas2`)
  грузятся лениво (code-split). В dev Vite иначе оптимизирует их при ПЕРВОМ открытии графа на лету и
  делает full-reload вебвью (выглядело как «вылет графа»). Прописаны в `optimizeDeps.include`
  (`vite.config.ts`) → пребандл на старте, без reload. Прод-сборки не касается.

## Дальше
- Глобальный граф (опц., с предупреждением о стоимости); фильтры; overlay-слот плагинов.
- Рефетч беклинков/графа после правок/реиндексации (сейчас — на смену активного файла).
- petgraph-кэш — только под тяжёлые алгоритмы (PageRank/кластеризация), не на старте (ADR-004).
