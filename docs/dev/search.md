# Поиск (`src-tauri/src/search` + сайдбар)

> Ф0-7 — по **метаданным** (title/path/tags). Ф1-6 — **гибридный по ТЕЛУ** (вектор + FTS5 → RRF, §6.2).

## Метаданные (Ф0-7)
- `search_notes(reader, query) -> Vec<NoteRef>`: `files` LEFT JOIN `file_tags`/`tags`,
  `path/title/tag LIKE %q%` (спецсимволы LIKE экранируются, `ESCAPE '\'`), `LIMIT 100`. Команда `search_vault`.

## Гибридный поиск по телу (Ф1-6 + доработка, §6.2)
`hybrid_search(reader, vectors?, embedder?, query, opts: SearchOptions) -> Vec<SearchHit>`. До ТРЁХ
независимых выдач кандидатов (по `CANDIDATES=50`), затем слияние:
- **Вектор** (семантика): `embed_query` → `VectorIndex::search` (или `search_filtered` при префильтре) →
  ранжированные `chunk_id`. Эмбеддинг запроса — ВНЕ блокировки read-пула (лок снят в команде).
- **FTS5/BM25** (лексика): `fts_chunks MATCH … ORDER BY rank`. `fts_query` санитизирует ввод: токены по
  не-буквенно-цифровым границам (юникод → кириллица цела), в кавычках, через `OR` — спецсинтаксис не утекает.
- **Граф** (близость по ссылкам, `center` = открытый файл): BFS по `links` до `GRAPH_HOPS=2` → чанки
  соседей, упорядоченные по (дистанция хопа, `chunk_index`). **Третий РАНГ внутри RRF, НЕ аддитивный
  `+0.2`** (REVIEW С-4). Включается только при заданном `center`.
- **RRF** (`rrf_fuse`): score = `Σ 1/(k+rank)` (k=`RRF_K`=60, rank 1-based); сорт по score↓, тай-брейк
  `chunk_id↑`. Сливаем РАНГИ, не «сырые» score (cos/BM25/граф — разные шкалы).

**Префильтр метаданных ДО KNN (AC-Б6-2).** `SearchFilter { folder, tag }` → `allowed_chunk_ids` (SQL по
`chunks JOIN files` + тег-`EXISTS`). Вектор-ветвь: usearch `filtered_search(|id| allowed.contains)` —
фильтр ВНУТРИ обхода HNSW (не пост-фильтр, recall цел). FTS-ветвь: те же условия через `JOIN files`.
Граф-ветвь: пересечение с `allowed`. Пустой фильтр-результат → пустая выдача.

**Dedup overlap.** Пере-выбираем `limit×OVERFETCH` кандидатов RRF → резолв (с `file_id`/`chunk_index`) →
схлоп: в порядке RRF пропускаем чанк, если сосед того же файла (|Δchunk_index|≤1 — overlap чанкера) уже
взят → усечение до `limit`. Итог в порядке RRF минус перекрытия.

**Изящная деградация:** нет эмбеддера → FTS(+граф); нет `center` → без граф-ранга; всё пусто → пусто.
Команда `search_content(query, limit?, folder?, tag?, center?)` (потолок 50) тянет `vectors`/`embedder`
из `VaultContext`. Чат `chat_rag` передаёт `center` (открытый файл) → граф-ранг в RAG-retrieval.
Контракт фронта: `tauriApi.search.searchContent(query, {limit,folder,tag,center})` + мок `mock/vault.ts`.

## Фронт
- `Sidebar`: поле поиска по метаданным (debounce 150 мс) + дерево/результаты. Вне Tauri — мок.
- Поиск/чат по содержимому (UI поверх `searchContent`/`streamRag`) — Ф1-8.

## Ограничения / дальше
- Сниппет — обрез чанка без подсветки; FTS5 `snippet()`/`highlight` — refinement.
- Префильтр по **дате** (есть папка/тег) + UI фильтров; калибровка `GRAPH_HOPS`/весов рангов — на eval (Ф1-10).
- **Реранкер** (cross-encoder поверх топ-N гибрида) — ADR-005 опционально; :8082 сейчас jina-эмбеддер кода,
  не `/rerank`; включать только под eval-гейтом (AC-EVAL-3) после Ф1-10.

## Тесты
- Rust (метаданные): path/title/tag, пустой/пробельный запрос.
- Rust (гибрид): `rrf_fuse`, `fts_query`, FTS-only, сортировка+резолв, пустые случаи; **префильтр по папке**
  (AC-Б6-2 — выдача в подпапке), **граф-ранг** (изолированно: только граф даёт соседа центра, без центра —
  пусто), **dedup overlap** (соседние чанки схлопнуты, < общего числа). **Живой** (nomic :8081). ✓
- Фронт: дерево↔результаты (метаданные); мок `searchContent` (score↓, limit, пустые случаи).
