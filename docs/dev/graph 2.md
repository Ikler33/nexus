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
- Тесты: Rust (N-hop расширяется по глубине, пустой центр), фронт (`computeLayout` назначает
  координаты; мок `getLocalGraph`).

## Дальше
- Глобальный граф (опц., с предупреждением о стоимости); фильтры; overlay-слот плагинов.
- Рефетч беклинков/графа после правок/реиндексации (сейчас — на смену активного файла).
- petgraph-кэш — только под тяжёлые алгоритмы (PageRank/кластеризация), не на старте (ADR-004).
