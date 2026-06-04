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
- Фронт: `GraphView` — **кастомный SVG force-directed** (по дизайн-хендоффу `graph.jsx`; sigma.js/graphology
  убраны). Физика (отталкивание + пружины рёбер + гравитация к центру, alpha-«остывание») — лёгкая
  main-thread rAF-петля; узлов мало (N-hop либо топ-600), кадровый бюджет соблюдается. Интерактив:
  **drag** (соседи подтягиваются пружинами), **hover** (подсветка связанных, остальное приглушается),
  **активная нота** (пульс-halo + ripple + дышащее кольцо), kin-кольца соседей, «поток» по рёбрам активной.
  Режимы local (глубина 1–3) / full (топ-600). Клик → `openFile`; открытие — `view.graph` (Cmd/Ctrl+G).
  - ⚠️ Раскладка теперь на **main-thread** (не Web Worker) — осознанный tradeoff ради живого drag-физикой
    (AC-PERF-6 «worker-layout» заменён). Узлы капнуты → ок; при тормозах единого графа на огромных vault —
    перенести симуляцию в Worker или снизить `FULL_LIMIT` (см. BACKLOG).
- **Чанкинг IN-запросов (V2.3, ревью A9):** узлы/рёбра/BFS-фронтир — `IN (...)`-запросы по одному
  плейсхолдеру на id. Супер-хаб превысил бы лимит bind-переменных SQLite (`too many SQL variables`) →
  чанкуем (`collect_in_chunks`, ≤900); рёбра — одиночный `source IN (chunk)` + фильтр `target ∈ ids` в
  Rust. Результат полный. Тот же приём в `get_full_graph`.
- Тесты: Rust (N-hop по глубине, пустой центр, **супер-хаб 1000 связей не валит лимит**); фронт —
  `graph-sim.ts` (BFS/соседи/kin/шаг-симуляции/радиус — 8 тестов); визуал/drag — проверка человеком.
- **Чисто-SVG (без тяжёлых deps):** граф больше не тянет sigma/graphology/forceatlas2 → пропала ленивая
  Vite-дооптимизация и full-reload вебвью (прежний ложный «вылет графа»); прежний `optimizeDeps` для них
  убран вместе с зависимостями.

## Дальше
- Глобальный граф (опц., с предупреждением о стоимости); фильтры; overlay-слот плагинов.
- Рефетч беклинков/графа после правок/реиндексации (сейчас — на смену активного файла).
- petgraph-кэш — только под тяжёлые алгоритмы (PageRank/кластеризация), не на старте (ADR-004).
