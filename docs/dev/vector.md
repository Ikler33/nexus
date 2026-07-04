# Векторный ANN-индекс (`src-tauri/src/vector`)

> Срез Ф1-4 (§3, §6.1–6.2). usearch HNSW, sibling-файл `.nexus/vectors.usearch`.
> Критерии **AC-Б4-2 / AC-Б5-1 / AC-Б8-2**.

## VectorIndex (usearch)
- `open(path, dim)` — загрузка существующего / создание нового под `dim` (= `embedder.dim()`,
  НЕ хардкод 1024 — §5/§6.5). Метрика **Cos** (векторы L2-нормализованы эмбеддером).
- `upsert(chunk_id, vec)` — ключ = `chunk_id` (u64). Замена снимает старый вектор → нет дублей
  при реиндексации (**AC-Б4-2**); проверка размерности → `DimMismatch` (**AC-Б5-1**); авто-`reserve`.
- `remove(chunk_id)` — чистка при удалении/реиндексации (**AC-Б8-2**), no-op если отсутствует.
- `search(query, k)` → `Vec<VectorHit { chunk_id, score = 1 − cos_dist }>`.
- `save()` (persist), `len`/`is_empty`/`contains`/`dim`. `usearch::Index` — Send+Sync (thread-safe).

## Транзакционность
usearch — отдельный файл, НЕ часть SQLite-транзакции. `replace_vectors` выполняется в том же job
write-actor сразу после SQLite-транзакции (сериализовано); полная атомарность с SQLite невозможна →
reconcile после краха (§5.1) подчистит рассинхрон. Подключение в индексатор — Ф1-5.

## Reconcile embedding-модели (§6.5, R-3d)
`reconcile_embedding_model` — КАНОН гарда совместимости производных vault с активной моделью/dim
(решение владельца §8.5, «полная чистка»; единственная реализация — реплики desktop/agentd удалены):
- смена модели/dim → `DELETE FROM chunks` (+FTS триггерами) + снос ВСЕХ `VECTOR_INDEX_FILES`
  (4 индекса, вкл. `chat_vectors`) + `chat_episodes.embed_model=NULL` + durable-маркер
  `files.size_bytes=-1` (ломает mtime+size-шорткат скана: chunks пересоздадутся, даже если
  `true`-возврат потребил процесс без индексатора заметок — agentd реконсилил первым, desktop
  открылся позже как no-op) + запись новых `settings`, возврат `true` (desktop передаёт его
  как `force` в `Indexer::with_rag`);
- та же модель/dim → СТРОГИЙ no-op, `false` (пользовательские индексы не пересобираются);
- первое включение → только запись `settings`, `true` (сноса нет — производных ещё нет).

Вызыватели: desktop `open_vault`/`build_rag`, agentd `build_rag_min` — оба ДО открытия индексов.

## Тесты
upsert+search+no-dup-growth (Б4-2), отказ при иной размерности (Б5-1), remove чистит выдачу (Б8-2),
персистентность (save → повторный open); reconcile-канон R-3d (first-run без сноса / строгий no-op /
полная чистка при смене модели и dim + durable-маркер; сценарий «agentd first → desktop no-op →
скан всё равно пересоздаёт chunks» — `indexer::tests::scan_after_foreign_reconcile_rebuilds_chunks_without_force`).

## Дальше
- Ф1-5: индексатор эмбеддит чанки (batch+семафор) и пишет `chunks`(+FTS)+usearch; переэмбеддизация
  при смене модели (§6.5).
- Ф1-6: hybrid search — usearch (vector) + FTS5 (BM25) → RRF; префильтр по метаданным до KNN.
