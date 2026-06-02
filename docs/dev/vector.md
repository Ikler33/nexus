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

## Тесты
upsert+search+no-dup-growth (Б4-2), отказ при иной размерности (Б5-1), remove чистит выдачу (Б8-2),
персистентность (save → повторный open).

## Дальше
- Ф1-5: индексатор эмбеддит чанки (batch+семафор) и пишет `chunks`(+FTS)+usearch; переэмбеддизация
  при смене модели (§6.5).
- Ф1-6: hybrid search — usearch (vector) + FTS5 (BM25) → RRF; префильтр по метаданным до KNN.
