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

## Дальше
- `get_local_graph` (рекурсивный CTE по links, N-hop) + sigma.js — Ф0-11.
- Рефетч беклинков после правок/реиндексации (сейчас — на смену активного файла).
- petgraph-кэш — только под тяжёлые алгоритмы, не на старте (ADR-004).
