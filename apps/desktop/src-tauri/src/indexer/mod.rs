//! Инкрементальный индексатор (§4.2, §6.1): парсит .md и пишет `files/links/tags` через
//! write-actor. Стабильность `file_id` при atomic-save — за счёт UPSERT по `path` (AC-Б9-1).
//!
//! Ссылки резолвятся в обе стороны: прямо (исходящие ссылки файла → `target_id`) и обратно
//! (висячие ссылки, чья цель проиндексирована позже, до-резолвятся при появлении файла).
//!
//! **RAG (Ф1-5).** Если задан embedding-провайдер, на каждый .md дополнительно: чанкинг
//! (§6.1) → эмбеддинг по батчам → запись `chunks` (+FTS5 через триггеры) и upsert векторов в
//! usearch (ключ = `chunk_id`). SQLite-часть (file/links/tags/chunks) атомарна в одной транзакции;
//! usearch — sibling-файл, обновляется сразу после неё (полная атомарность с БД невозможна →
//! reconcile после краха, §5.1). Без провайдера RAG-шаги пропускаются (vault работает без AI).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::stream::{self, StreamExt};
use rusqlite::{params, OptionalExtension, Transaction};
use tokio::sync::Semaphore;

use crate::ai::EmbeddingProvider;
use crate::chunker::{self, ChunkOptions, WordTokenizer};
use crate::db::{Database, DbError, DbResult, ReadPool, WriteActor};
use crate::parser;
use crate::vector::VectorIndex;
use crate::watcher::{self, VaultEvent, VaultWatcher};

/// Максимум входов в одном запросе к embedding-серверу (страхует от слишком больших батчей).
const EMBED_BATCH: usize = 64;
/// Одновременные embedding-вызовы к серверу (семафор). Перекрытие сетевой латентности — главный
/// рычаг throughput первичного скана (бенч: последовательно ~40 эмб/с → ×N с конкуренцией).
const EMBED_CONCURRENCY: usize = 8;
/// Сколько файлов скана держим «в полёте» одновременно (`buffer_unordered`). Кооперативная
/// конкуренция в ОДНОЙ задаче (не параллелизм): перекрывает embed-/IO-ожидания, но синхронные
/// секции usearch/БД не исполняются параллельно → без гонок. Реальный потолок embed — `EMBED_CONCURRENCY`.
const SCAN_CONCURRENCY: usize = 16;
/// Как часто персистить usearch и логировать прогресс во время начального скана (в файлах).
const SCAN_CHECKPOINT: usize = 256;

/// RAG-подсистема индексатора: эмбеддер + векторный индекс + параметры чанкинга.
struct Rag {
    embedder: Arc<dyn EmbeddingProvider>,
    vectors: Arc<VectorIndex>,
    chunk_opts: ChunkOptions,
    embed_sem: Arc<Semaphore>,
}

/// Индексатор одного vault. Дёшево клонируемые writer/reader + корень. RAG — опционально.
pub struct Indexer {
    writer: WriteActor,
    reader: ReadPool,
    root: PathBuf,
    rag: Option<Rag>,
    /// Когда `true`, `index_file` игнорирует mtime/size-шорткат и переиндексирует принудительно
    /// (первичное наполнение чанков / переэмбеддизация после смены модели — §6.5).
    force: Arc<AtomicBool>,
}

impl Indexer {
    /// Индексатор без RAG (только files/links/tags) — для vault без embedding-провайдера и тестов Ф0.
    pub fn new(db: &Database, root: PathBuf) -> Self {
        Self {
            writer: db.writer().clone(),
            reader: db.reader().clone(),
            root,
            rag: None,
            force: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Индексатор с RAG: чанкинг + эмбеддинг + usearch. `force_reindex` — принудительно
    /// переиндексировать все файлы при начальном скане (первое включение RAG / смена модели §6.5).
    pub fn with_rag(
        db: &Database,
        root: PathBuf,
        embedder: Arc<dyn EmbeddingProvider>,
        vectors: Arc<VectorIndex>,
        force_reindex: bool,
    ) -> Self {
        Self {
            writer: db.writer().clone(),
            reader: db.reader().clone(),
            root,
            rag: Some(Rag {
                embedder,
                vectors,
                chunk_opts: ChunkOptions::default(),
                embed_sem: Arc::new(Semaphore::new(EMBED_CONCURRENCY)),
            }),
            force: Arc::new(AtomicBool::new(force_reindex)),
        }
    }

    /// Индексирует один файл по относительному пути. Для не-.md — no-op. Пропускает
    /// неизменённые файлы по mtime+size (дешёвый шорткат — не читаем диск зря), если не `force`.
    pub async fn index_file(&self, rel: &str) -> DbResult<()> {
        if !rel.ends_with(".md") {
            return Ok(());
        }
        let abs = self.root.join(rel);
        let Ok(meta) = tokio::fs::metadata(&abs).await else {
            return Ok(()); // файла нет — обрабатывается как Deleted отдельно
        };
        let size = meta.len() as i64;
        let mtime = mtime_secs(&meta);

        let rel_owned = rel.to_string();
        if !self.force.load(Ordering::Relaxed) {
            let unchanged = self
                .reader
                .query({
                    let rel = rel_owned.clone();
                    move |c| {
                        c.query_row(
                            "SELECT updated_at, size_bytes FROM files WHERE path=?1 AND is_deleted=0",
                            [rel],
                            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
                        )
                        .optional()
                    }
                })
                .await?;
            if let Some((u, s)) = unchanged {
                if u == mtime && s == size {
                    return Ok(());
                }
            }
        }

        let content = tokio::fs::read_to_string(&abs).await.unwrap_or_default();
        let hash = blake3::hash(content.as_bytes()).to_hex().to_string();

        // Парсинг и (если RAG включён) чанкинг — оба CPU-bound, в одном spawn_blocking.
        let do_chunk = self.rag.is_some();
        let opts = self.rag.as_ref().map(|r| r.chunk_opts).unwrap_or_default();
        let (parsed, chunks) = tokio::task::spawn_blocking(move || {
            let parsed = parser::parse(&content);
            let chunks = if do_chunk {
                chunker::chunk_document(&content, &WordTokenizer, opts)
            } else {
                Vec::new()
            };
            (parsed, chunks)
        })
        .await
        .map_err(|_| DbError::Unavailable)?;

        // Эмбеддинг чанков (батчами, под семафором) — ДО транзакции (async, вне rusqlite).
        let vectors = if do_chunk && !chunks.is_empty() {
            self.embed_chunks(&chunks).await?
        } else {
            Vec::new()
        };

        let forms = path_forms(&rel_owned);
        let now = now_secs();

        let (old_chunk_ids, new_chunk_ids) = self
            .writer
            .transaction(move |tx| {
                let file_id: i64 = tx.query_row(
                    "INSERT INTO files \
                       (path,hash,title,created_at,updated_at,indexed_at,size_bytes,word_count,frontmatter) \
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9) \
                     ON CONFLICT(path) DO UPDATE SET \
                       hash=excluded.hash, title=excluded.title, updated_at=excluded.updated_at, \
                       indexed_at=excluded.indexed_at, size_bytes=excluded.size_bytes, \
                       word_count=excluded.word_count, frontmatter=excluded.frontmatter, is_deleted=0 \
                     RETURNING id",
                    params![
                        rel_owned,
                        hash,
                        parsed.title,
                        mtime,
                        mtime,
                        now,
                        size,
                        parsed.word_count as i64,
                        parsed.frontmatter,
                    ],
                    |r| r.get(0),
                )?;

                // Алиасы из frontmatter (V4.1): полная замена. UNIQUE(alias) глобальный →
                // OR REPLACE (последний проиндексированный файл выигрывает спорный алиас).
                tx.execute("DELETE FROM aliases WHERE file_id=?1", [file_id])?;
                for alias in &parsed.aliases {
                    tx.execute(
                        "INSERT OR REPLACE INTO aliases (file_id, alias) VALUES (?1, ?2)",
                        params![file_id, alias],
                    )?;
                }

                // Плоские поля frontmatter (typed-frontmatter): полная замена на файл; UNIQUE(file_id,key)
                // → OR REPLACE. Для кросс-файловых запросов (цели/stale-radar/Dataview).
                tx.execute("DELETE FROM frontmatter_fields WHERE file_id=?1", [file_id])?;
                for (key, value) in &parsed.fields {
                    tx.execute(
                        "INSERT OR REPLACE INTO frontmatter_fields (file_id, key, value) \
                         VALUES (?1, ?2, ?3)",
                        params![file_id, key, value],
                    )?;
                }

                // Исходящие ссылки: полная замена (DELETE + INSERT с прямым резолвом цели).
                tx.execute("DELETE FROM links WHERE source_id=?1", [file_id])?;
                for link in &parsed.links {
                    let target_id = resolve_target(tx, &link.target_raw)?;
                    tx.execute(
                        "INSERT INTO links (source_id,target_id,target_raw,link_type,context,line_number) \
                         VALUES (?1,?2,?3,?4,?5,?6)",
                        params![
                            file_id,
                            target_id,
                            link.target_raw,
                            link.link_type.as_str(),
                            link.context,
                            link.line_number as i64,
                        ],
                    )?;
                }

                // Теги: полная замена связей файла.
                tx.execute("DELETE FROM file_tags WHERE file_id=?1", [file_id])?;
                for tag in &parsed.tags {
                    tx.execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", [tag])?;
                    let tag_id: i64 =
                        tx.query_row("SELECT id FROM tags WHERE name=?1", [tag], |r| r.get(0))?;
                    tx.execute(
                        "INSERT OR IGNORE INTO file_tags (file_id,tag_id) VALUES (?1,?2)",
                        params![file_id, tag_id],
                    )?;
                }

                // Обратный резолв: висячие ссылки на этот файл получают target_id (путь + алиасы).
                for form in &forms {
                    tx.execute(
                        "UPDATE links SET target_id=?1 WHERE target_id IS NULL AND target_raw=?2",
                        params![file_id, form],
                    )?;
                }
                for alias in &parsed.aliases {
                    tx.execute(
                        "UPDATE links SET target_id=?1 WHERE target_id IS NULL AND target_raw=?2",
                        params![file_id, alias],
                    )?;
                }

                // RAG: полная замена чанков файла. Старые id нужны для чистки usearch (вне БД);
                // FTS5 синхронизируется триггерами chunks_ai/ad/au. Атомарно с file/links/tags.
                let mut old_ids: Vec<u64> = Vec::new();
                let mut new_ids: Vec<i64> = Vec::new();
                if do_chunk {
                    {
                        let mut stmt = tx.prepare("SELECT id FROM chunks WHERE file_id=?1")?;
                        old_ids = stmt
                            .query_map([file_id], |r| r.get::<_, i64>(0))?
                            .collect::<rusqlite::Result<Vec<_>>>()?
                            .into_iter()
                            .map(|id| id as u64)
                            .collect();
                    }
                    tx.execute("DELETE FROM chunks WHERE file_id=?1", [file_id])?;
                    for ch in &chunks {
                        let id: i64 = tx.query_row(
                            "INSERT INTO chunks \
                               (file_id,chunk_index,content,char_start,char_end,heading_path,token_count) \
                             VALUES (?1,?2,?3,?4,?5,?6,?7) RETURNING id",
                            params![
                                file_id,
                                ch.chunk_index as i64,
                                ch.content,
                                ch.char_start as i64,
                                ch.char_end as i64,
                                ch.heading_path,
                                ch.token_count as i64,
                            ],
                            |r| r.get(0),
                        )?;
                        new_ids.push(id);
                    }
                }
                Ok((old_ids, new_ids))
            })
            .await?;

        // usearch — вне SQLite-транзакции (отдельный файл). Снимаем старые векторы файла,
        // добавляем новые: ключ = chunk_id, вектор[i] 1:1 к чанку i (RETURNING сохранил порядок).
        if do_chunk {
            if let Some(rag) = &self.rag {
                for old in &old_chunk_ids {
                    let _ = rag.vectors.remove(*old);
                }
                for (id, vec) in new_chunk_ids.iter().zip(&vectors) {
                    rag.vectors
                        .upsert(*id as u64, vec)
                        .map_err(|e| DbError::External(e.to_string()))?;
                }
            }
        }
        Ok(())
    }

    /// Эмбеддит чанки батчами по [`EMBED_BATCH`] под семафором конкуренции. Возвращает векторы
    /// 1:1 ко входу. Только при включённом RAG (иначе вызывающий не дойдёт сюда).
    async fn embed_chunks(&self, chunks: &[chunker::Chunk]) -> DbResult<Vec<Vec<f32>>> {
        let rag = self.rag.as_ref().expect("embed_chunks без RAG");
        let mut out: Vec<Vec<f32>> = Vec::with_capacity(chunks.len());
        for batch in chunks.chunks(EMBED_BATCH) {
            let texts: Vec<&str> = batch.iter().map(|c| c.content.as_str()).collect();
            let _permit = rag
                .embed_sem
                .acquire()
                .await
                .map_err(|_| DbError::Unavailable)?;
            let vecs = rag
                .embedder
                .embed_documents(&texts)
                .await
                .map_err(|e| DbError::External(e.to_string()))?;
            out.extend(vecs);
        }
        Ok(out)
    }

    /// Soft-delete файла: помечает удалённым, обнуляет входящие ссылки, чистит исходящие/теги
    /// и чанки (+FTS через триггеры). Векторы удаляются из usearch после транзакции.
    pub async fn remove_file(&self, rel: &str) -> DbResult<()> {
        let rel = rel.to_string();
        let removed_chunk_ids = self
            .writer
            .transaction(move |tx| {
                let id: Option<i64> = tx
                    .query_row("SELECT id FROM files WHERE path=?1", [&rel], |r| r.get(0))
                    .optional()?;
                let mut chunk_ids: Vec<u64> = Vec::new();
                if let Some(id) = id {
                    {
                        let mut stmt = tx.prepare("SELECT id FROM chunks WHERE file_id=?1")?;
                        chunk_ids = stmt
                            .query_map([id], |r| r.get::<_, i64>(0))?
                            .collect::<rusqlite::Result<Vec<_>>>()?
                            .into_iter()
                            .map(|c| c as u64)
                            .collect();
                    }
                    tx.execute("UPDATE files SET is_deleted=1 WHERE id=?1", [id])?;
                    tx.execute("UPDATE links SET target_id=NULL WHERE target_id=?1", [id])?;
                    tx.execute("DELETE FROM links WHERE source_id=?1", [id])?;
                    tx.execute("DELETE FROM file_tags WHERE file_id=?1", [id])?;
                    tx.execute("DELETE FROM chunks WHERE file_id=?1", [id])?;
                }
                Ok(chunk_ids)
            })
            .await?;

        if let Some(rag) = &self.rag {
            for id in &removed_chunk_ids {
                let _ = rag.vectors.remove(*id);
            }
        }
        Ok(())
    }

    /// Переименование/перемещение `from_rel`→`to_rel` с СОХРАНЕНИЕМ `file_id` (AC-Б9, V2.2): переносит
    /// `files.path`, поэтому входящие ссылки (беклинки) и чанки остаются привязаны к тому же id —
    /// в отличие от delete+create, который рвёт их. Случаи: исходного файла нет в БД → индексируем цель
    /// как новую; на цели уже был файл → он замещается (строка+чанки убираются, UNIQUE(path) свободен);
    /// rename совпал с правкой содержимого → финальный `index_file` обновит контент под тем же id
    /// (UPSERT по новому пути), чистый rename → ранний выход. Текст ссылок-источников `[[Old]]`→`[[New]]`
    /// у ссылающихся файлов НЕ переписывается (отдельная фича Obsidian «update links on rename» — BACKLOG).
    pub async fn rename_file(&self, from_rel: &str, to_rel: &str) -> DbResult<()> {
        // Переименование в не-.md (смена расширения и т.п.) — цель не индексируется; убрать исходный.
        if !to_rel.ends_with(".md") {
            return self.remove_file(from_rel).await;
        }
        let from = from_rel.to_string();
        let to = to_rel.to_string();
        let to_forms = path_forms(to_rel);
        let now = now_secs();
        // `None` → исходного файла нет в БД (вызывающий проиндексирует цель как новую);
        // `Some(purged)` → перенесли, `purged` — чанки замещённого на цели файла (чистка usearch).
        let outcome: Option<Vec<u64>> = self
            .writer
            .transaction(move |tx| {
                let from_id: Option<i64> = tx
                    .query_row(
                        "SELECT id FROM files WHERE path=?1 AND is_deleted=0",
                        [&from],
                        |r| r.get(0),
                    )
                    .optional()?;
                let Some(from_id) = from_id else {
                    return Ok(None);
                };
                // Замещение любой строки на целевом пути (живой файл или tombstone), кроме самого
                // исходного, — иначе UPDATE на занятый UNIQUE(path) упадёт. Чанки вернём для usearch.
                let mut purged: Vec<u64> = Vec::new();
                let existing_to: Option<i64> = tx
                    .query_row("SELECT id FROM files WHERE path=?1", [&to], |r| r.get(0))
                    .optional()?;
                if let Some(to_id) = existing_to {
                    if to_id != from_id {
                        {
                            let mut stmt = tx.prepare("SELECT id FROM chunks WHERE file_id=?1")?;
                            purged = stmt
                                .query_map([to_id], |r| r.get::<_, i64>(0))?
                                .collect::<rusqlite::Result<Vec<_>>>()?
                                .into_iter()
                                .map(|c| c as u64)
                                .collect();
                        }
                        tx.execute(
                            "UPDATE links SET target_id=NULL WHERE target_id=?1",
                            [to_id],
                        )?;
                        tx.execute("DELETE FROM links WHERE source_id=?1", [to_id])?;
                        tx.execute("DELETE FROM file_tags WHERE file_id=?1", [to_id])?;
                        tx.execute("DELETE FROM chunks WHERE file_id=?1", [to_id])?;
                        tx.execute("DELETE FROM aliases WHERE file_id=?1", [to_id])?;
                        tx.execute("DELETE FROM files WHERE id=?1", [to_id])?;
                    }
                }
                // Перенос пути — file_id жив (входящие ссылки на from_id не трогаем → беклинки целы).
                tx.execute(
                    "UPDATE files SET path=?1, indexed_at=?2 WHERE id=?3",
                    params![to, now, from_id],
                )?;
                // Висячие ссылки на НОВОЕ имя ([[New]] до переименования) теперь резолвятся сюда.
                for form in &to_forms {
                    tx.execute(
                        "UPDATE links SET target_id=?1 WHERE target_id IS NULL AND target_raw=?2",
                        params![from_id, form],
                    )?;
                }
                Ok(Some(purged))
            })
            .await?;

        match outcome {
            None => self.index_file(to_rel).await,
            Some(purged) => {
                if let Some(rag) = &self.rag {
                    for id in &purged {
                        let _ = rag.vectors.remove(*id);
                    }
                }
                self.index_file(to_rel).await
            }
        }
    }

    /// Начальный обход vault: индексирует все .md, затем до-резолвит висячие ссылки.
    pub async fn scan_vault(&self) -> DbResult<()> {
        let root = self.root.clone();
        let rels = tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            collect_md(&root, &root, &mut out);
            out
        })
        .await
        .map_err(|_| DbError::Unavailable)?;

        let total = rels.len();
        let rag_on = self.rag.is_some();
        if rag_on && self.force.load(Ordering::Relaxed) {
            tracing::info!(
                files = total,
                "принудительная полная переиндексация (RAG / смена модели)"
            );
        }
        // Конкурентный скан (§10): держим до `SCAN_CONCURRENCY` файлов «в полёте» — embed-/IO-ожидания
        // перекрываются (потолок embed — семафор `EMBED_CONCURRENCY`). Кооперативно в ОДНОЙ задаче:
        // синхронные секции usearch/БД не исполняются параллельно (между `.next()` ни одна future не
        // поллится) → `persist_vectors()` в теле цикла и upsert'ы векторов без гонок.
        let mut done = 0usize;
        let mut stream = stream::iter(rels)
            .map(|rel| async move {
                if let Err(e) = self.index_file(&rel).await {
                    tracing::warn!(file = %rel, error = %e, "index_file failed during scan");
                }
            })
            .buffer_unordered(SCAN_CONCURRENCY);
        while stream.next().await.is_some() {
            done += 1;
            // Периодический чекпойнт usearch + прогресс N/M (AC-PERF-5).
            if rag_on && done % SCAN_CHECKPOINT == 0 {
                self.persist_vectors();
                tracing::info!(done, total, "indexing progress");
            }
        }
        drop(stream);
        self.writer.transaction(resolve_all_dangling).await?;
        // §5.1: дочинить векторы, потерянные при крахе между commit и save (на force-скане no-op).
        if let Err(e) = self.reconcile_vectors().await {
            tracing::warn!(error = %e, "reconcile усearch failed");
        }
        self.persist_vectors(); // финальный save усearch
        self.force.store(false, Ordering::Relaxed); // дальше — инкрементально, с mtime-шорткатом
        tracing::info!(files = total, "initial vault scan complete");
        Ok(())
    }

    /// **Crash-reconcile usearch (§5.1).** Для чанков, что есть в БД, но чьих векторов нет в usearch
    /// (commit прошёл, `save` усearch — нет), переэмбеддит содержимое и доливает векторы. На force-скане
    /// обычно no-op (все чанки только что переиндексированы). Best-effort: при недоступном эмбеддере
    /// логирует и выходит — повторит при следующем открытии. Закрывает рассинхрон из docs/vector.md.
    async fn reconcile_vectors(&self) -> DbResult<()> {
        let Some(rag) = &self.rag else {
            return Ok(());
        };
        let all_ids: Vec<i64> = self
            .reader
            .query(|c| {
                c.prepare("SELECT id FROM chunks")?
                    .query_map([], |r| r.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .await?;
        let missing: Vec<i64> = all_ids
            .into_iter()
            .filter(|id| !rag.vectors.contains(*id as u64))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        tracing::info!(
            count = missing.len(),
            "reconcile: дочиняю потерянные векторы чанков (§5.1)"
        );

        let mut restored = 0usize;
        for batch in missing.chunks(EMBED_BATCH) {
            let ids = batch.to_vec();
            let rows: Vec<(i64, String)> = self
                .reader
                .query(move |c| {
                    let ph = vec!["?"; ids.len()].join(",");
                    let sql = format!("SELECT id, content FROM chunks WHERE id IN ({ph})");
                    c.prepare(&sql)?
                        .query_map(rusqlite::params_from_iter(ids.iter()), |r| {
                            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .await?;
            let texts: Vec<&str> = rows.iter().map(|(_, c)| c.as_str()).collect();
            let _permit = rag
                .embed_sem
                .acquire()
                .await
                .map_err(|_| DbError::Unavailable)?;
            match rag.embedder.embed_documents(&texts).await {
                Ok(vecs) => {
                    for ((id, _), v) in rows.iter().zip(&vecs) {
                        match rag.vectors.upsert(*id as u64, v) {
                            Ok(()) => restored += 1,
                            Err(e) => tracing::warn!(error = %e, "reconcile: upsert вектора"),
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "reconcile: эмбеддер недоступен — повтор при след. открытии");
                    break;
                }
            }
        }
        if restored > 0 {
            self.persist_vectors();
            tracing::info!(restored, "reconcile: векторы восстановлены");
        }
        Ok(())
    }

    /// Персистит usearch на диск (no-op без RAG). Ошибку логирует, скан не валит.
    fn persist_vectors(&self) {
        if let Some(rag) = &self.rag {
            if let Err(e) = rag.vectors.save() {
                tracing::warn!(error = %e, "usearch save failed");
            }
        }
    }
}

/// Запускает watcher + фоновый цикл индексации для готового `Indexer` (вызывается из `open_vault`,
/// который решает, с RAG или без). Watcher живёт внутри спавненной задачи; на завершении — стоп.
pub fn spawn(indexer: Indexer) {
    let root = indexer.root.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<VaultEvent>();
    let watcher = match VaultWatcher::new(&root, tx) {
        Ok(w) => w,
        Err(e) => {
            tracing::error!(error = %e, "vault watcher init failed");
            return;
        }
    };
    tokio::spawn(async move {
        let _watcher = watcher; // держим watcher живым на время задачи
        if let Err(e) = indexer.scan_vault().await {
            tracing::error!(error = %e, "initial vault scan failed");
        }
        while let Some(event) = rx.recv().await {
            let result = match event {
                VaultEvent::Upsert(abs) => match rel_of(&indexer.root, &abs) {
                    Some(rel) => indexer.index_file(&rel).await,
                    None => Ok(()),
                },
                VaultEvent::Deleted(abs) => match rel_of(&indexer.root, &abs) {
                    Some(rel) => indexer.remove_file(&rel).await,
                    None => Ok(()),
                },
                VaultEvent::Renamed { from, to } => {
                    match (rel_of(&indexer.root, &from), rel_of(&indexer.root, &to)) {
                        (Some(from_rel), Some(to_rel)) => {
                            indexer.rename_file(&from_rel, &to_rel).await
                        }
                        // Перемещение из/в пределы vault → как удаление/создание соответственно.
                        (None, Some(to_rel)) => indexer.index_file(&to_rel).await,
                        (Some(from_rel), None) => indexer.remove_file(&from_rel).await,
                        (None, None) => Ok(()),
                    }
                }
            };
            match result {
                // Персистим usearch после каждого инкрементального события (события дебаунсятся
                // watcher'ом, не на каждое нажатие). Дебаунс самого save — позже при росте индекса.
                Ok(()) => indexer.persist_vectors(),
                Err(e) => tracing::warn!(error = %e, "index event failed"),
            }
        }
    });
}

/// Резолвит цель ссылки в `file_id` (точный путь, путь+`.md`, basename ± `.md`; затем алиас).
/// Путь имеет приоритет над алиасом (реальный файл `X` важнее алиаса `X`).
fn resolve_target(tx: &Transaction, target_raw: &str) -> rusqlite::Result<Option<i64>> {
    let by_path = tx
        .query_row(
            "SELECT id FROM files WHERE is_deleted=0 AND ( \
               path = ?1 OR path = ?1 || '.md' \
               OR path LIKE '%/' || ?1 OR path LIKE '%/' || ?1 || '.md' \
             ) ORDER BY length(path) LIMIT 1",
            [target_raw],
            |r| r.get(0),
        )
        .optional()?;
    if by_path.is_some() {
        return Ok(by_path);
    }
    // Фолбэк: точное совпадение с алиасом (V4.1), файл не удалён.
    tx.query_row(
        "SELECT a.file_id FROM aliases a JOIN files f ON f.id = a.file_id \
         WHERE f.is_deleted=0 AND a.alias = ?1 LIMIT 1",
        [target_raw],
        |r| r.get(0),
    )
    .optional()
}

/// До-резолвит ВСЕ висячие ссылки (после начального скана — закрывает порядок индексации).
fn resolve_all_dangling(tx: &Transaction) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE links SET target_id = COALESCE( \
           ( SELECT f.id FROM files f WHERE f.is_deleted=0 AND ( \
               f.path = links.target_raw OR f.path = links.target_raw || '.md' \
               OR f.path LIKE '%/' || links.target_raw OR f.path LIKE '%/' || links.target_raw || '.md' \
             ) ORDER BY length(f.path) LIMIT 1 ), \
           ( SELECT a.file_id FROM aliases a JOIN files f ON f.id = a.file_id \
             WHERE f.is_deleted=0 AND a.alias = links.target_raw LIMIT 1 ) \
         ) WHERE target_id IS NULL",
        [],
    )?;
    Ok(())
}

/// Нормализованные формы относительного пути для обратного резолва ссылок.
fn path_forms(rel: &str) -> Vec<String> {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    let mut forms = vec![
        rel.to_string(),
        rel.strip_suffix(".md").unwrap_or(rel).to_string(),
        base.to_string(),
        base.strip_suffix(".md").unwrap_or(base).to_string(),
    ];
    forms.sort();
    forms.dedup();
    forms
}

fn collect_md(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if watcher::is_ignored(&path) {
            continue;
        }
        if path.is_dir() {
            collect_md(root, &path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Some(rel) = rel_of(root, &path) {
                out.push(rel);
            }
        }
    }
}

fn rel_of(root: &Path, abs: &Path) -> Option<String> {
    abs.strip_prefix(root)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
}

fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    async fn open(root: &Path) -> Database {
        Database::open(root.join(".nexus/nexus.db")).await.unwrap()
    }

    async fn file_id(db: &Database, path: &str) -> i64 {
        let path = path.to_string();
        db.reader()
            .query(move |c| c.query_row("SELECT id FROM files WHERE path=?1", [path], |r| r.get(0)))
            .await
            .unwrap()
    }

    /// Источники беклинков файла `target_id` (пути), отсортированы.
    async fn backlink_sources(db: &Database, target_id: i64) -> Vec<String> {
        db.reader()
            .query(move |c| {
                let mut stmt = c.prepare(
                    "SELECT f.path FROM links l JOIN files f ON f.id=l.source_id \
                     WHERE l.target_id=?1 ORDER BY f.path",
                )?;
                let rows = stmt
                    .query_map([target_id], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .await
            .unwrap()
    }

    /// Все теги, привязанные к файлам (отсортированы).
    async fn read_tags(db: &Database) -> Vec<String> {
        db.reader()
            .query(|c| {
                let mut s = c.prepare(
                    "SELECT t.name FROM tags t JOIN file_tags ft ON ft.tag_id=t.id ORDER BY t.name",
                )?;
                let v = s
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(v)
            })
            .await
            .unwrap()
    }

    /// Алиасы файла (отсортированы).
    async fn read_aliases(db: &Database, file_id: i64) -> Vec<String> {
        db.reader()
            .query(move |c| {
                let mut s =
                    c.prepare("SELECT alias FROM aliases WHERE file_id=?1 ORDER BY alias")?;
                let v = s
                    .query_map([file_id], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(v)
            })
            .await
            .unwrap()
    }

    /// Поля frontmatter файла как `(key, value)`, отсортированы по ключу.
    async fn read_fields(db: &Database, file_id: i64) -> Vec<(String, String)> {
        db.reader()
            .query(move |c| {
                let mut s = c.prepare(
                    "SELECT key, value FROM frontmatter_fields WHERE file_id=?1 ORDER BY key",
                )?;
                let v = s
                    .query_map([file_id], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(v)
            })
            .await
            .unwrap()
    }

    /// V4.1: `[[Алиас]]` резолвится в файл, объявивший алиас в frontmatter (forward и backward),
    /// таблица `aliases` заполняется.
    #[tokio::test]
    async fn aliases_resolve_links_and_populate_table() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(
            root.join("Target.md"),
            "---\naliases: [MyAlias, Second]\n---\n# Target\n",
        )
        .unwrap();
        fs::write(root.join("Fwd.md"), "see [[MyAlias]]\n").unwrap();
        fs::write(root.join("Bwd.md"), "see [[Second]]\n").unwrap();

        let db = open(&root).await;
        let idx = Indexer::new(&db, root.clone());

        // Backward: Bwd индексируется ДО Target (ссылка висячая) → резолв при индексации Target по алиасу.
        idx.index_file("Bwd.md").await.unwrap();
        idx.index_file("Target.md").await.unwrap();
        // Forward: Fwd индексируется ПОСЛЕ Target → резолв алиаса при вставке ссылки.
        idx.index_file("Fwd.md").await.unwrap();

        let target_id = file_id(&db, "Target.md").await;
        let mut bl = backlink_sources(&db, target_id).await;
        bl.sort();
        assert_eq!(
            bl,
            vec!["Bwd.md".to_string(), "Fwd.md".to_string()],
            "[[Алиас]] резолвится и forward, и backward"
        );
        assert_eq!(
            read_aliases(&db, target_id).await,
            vec!["MyAlias".to_string(), "Second".to_string()]
        );
    }

    /// AC-Б9-1: atomic-save (перезапись того же пути) сохраняет file_id, беклинки целы.
    #[tokio::test]
    async fn atomic_save_preserves_file_id_and_backlinks() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("A.md"), "# A\n\nlink to [[B]]\n").unwrap();
        fs::write(root.join("B.md"), "# B\n").unwrap();

        let db = open(&root).await;
        let idx = Indexer::new(&db, root.clone());
        idx.index_file("B.md").await.unwrap();
        idx.index_file("A.md").await.unwrap();

        let b_id = file_id(&db, "B.md").await;
        assert_eq!(backlink_sources(&db, b_id).await, vec!["A.md"]);

        // atomic-save B.md: тот же путь, новое содержимое.
        fs::write(root.join("B.md"), "# B\n\nmore text\n").unwrap();
        idx.index_file("B.md").await.unwrap();

        assert_eq!(
            file_id(&db, "B.md").await,
            b_id,
            "file_id должен сохраниться"
        );
        assert_eq!(
            backlink_sources(&db, b_id).await,
            vec!["A.md"],
            "беклинки B не должны пострадать"
        );
    }

    /// AC-Б9 (V2.2): rename/move сохраняет `file_id` → беклинки целы. `[[Old]]` остаётся
    /// зарезолвленной по сохранённому id, а ранее висячая `[[New]]` до-резолвится в этот файл.
    #[tokio::test]
    async fn rename_preserves_file_id_and_backlinks() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("Old.md"), "# Old\n").unwrap();
        fs::write(root.join("Ref.md"), "see [[Old]]\n").unwrap();
        fs::write(root.join("Fwd.md"), "see [[New]]\n").unwrap();

        let db = open(&root).await;
        let idx = Indexer::new(&db, root.clone());
        idx.index_file("Old.md").await.unwrap();
        idx.index_file("Ref.md").await.unwrap(); // [[Old]] → Old.md
        idx.index_file("Fwd.md").await.unwrap(); // [[New]] висячая (New.md ещё нет)

        let old_id = file_id(&db, "Old.md").await;
        assert_eq!(
            backlink_sources(&db, old_id).await,
            vec!["Ref.md"],
            "до rename зарезолвлена только [[Old]]"
        );

        // Переименование Old.md → New.md (как watcher после move на ФС).
        fs::rename(root.join("Old.md"), root.join("New.md")).unwrap();
        idx.rename_file("Old.md", "New.md").await.unwrap();

        assert_eq!(
            file_id(&db, "New.md").await,
            old_id,
            "file_id сохраняется под новым путём"
        );
        let mut bl = backlink_sources(&db, old_id).await;
        bl.sort();
        assert_eq!(
            bl,
            vec!["Fwd.md".to_string(), "Ref.md".to_string()],
            "[[Old]] цела (по id), [[New]] до-резолвилась"
        );

        // Старого пути в живых не осталось.
        let old_live: Option<i64> = db
            .reader()
            .query(|c| {
                c.query_row(
                    "SELECT id FROM files WHERE path='Old.md' AND is_deleted=0",
                    [],
                    |r| r.get(0),
                )
                .optional()
            })
            .await
            .unwrap();
        assert!(old_live.is_none(), "старый путь не должен оставаться живым");
    }

    /// Обратный резолв: ссылка, чья цель проиндексирована позже, до-резолвится.
    #[tokio::test]
    async fn back_resolves_links_indexed_out_of_order() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("A.md"), "[[B]]\n").unwrap();
        fs::write(root.join("B.md"), "# B\n").unwrap();

        let db = open(&root).await;
        let idx = Indexer::new(&db, root.clone());
        idx.index_file("A.md").await.unwrap(); // B ещё не в БД → ссылка висячая
        idx.index_file("B.md").await.unwrap(); // обратный резолв привяжет ссылку A→B

        let b_id = file_id(&db, "B.md").await;
        assert_eq!(backlink_sources(&db, b_id).await, vec!["A.md"]);
    }

    /// Индексация наполняет теги; повторная индексация заменяет их.
    #[tokio::test]
    async fn indexes_and_replaces_tags() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("N.md"), "body #project #area\n").unwrap();

        let db = open(&root).await;
        let idx = Indexer::new(&db, root.clone());
        idx.index_file("N.md").await.unwrap();

        assert_eq!(
            read_tags(&db).await,
            vec!["area".to_string(), "project".to_string()]
        );

        fs::write(root.join("N.md"), "body #area only\n").unwrap();
        idx.index_file("N.md").await.unwrap();
        assert_eq!(read_tags(&db).await, vec!["area".to_string()]);
    }

    /// typed-frontmatter: плоские поля индексируются в `frontmatter_fields` и заменяются при реиндексе.
    #[tokio::test]
    async fn indexes_and_replaces_frontmatter_fields() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(
            root.join("Goal.md"),
            "---\nprogress: 0.3\ndue: 2026-02-01\naliases: [G]\n---\nbody\n",
        )
        .unwrap();

        let db = open(&root).await;
        let idx = Indexer::new(&db, root.clone());
        idx.index_file("Goal.md").await.unwrap();
        let id = file_id(&db, "Goal.md").await;

        // Плоские скаляры записаны; список aliases в frontmatter_fields НЕ попал (у него своя таблица).
        assert_eq!(
            read_fields(&db, id).await,
            vec![
                ("due".to_string(), "2026-02-01".to_string()),
                ("progress".to_string(), "0.3".to_string()),
            ]
        );

        // Реиндекс с другими полями → полная замена (старое `due` ушло).
        fs::write(root.join("Goal.md"), "---\nprogress: 1.0\n---\nbody\n").unwrap();
        idx.index_file("Goal.md").await.unwrap();
        assert_eq!(
            read_fields(&db, id).await,
            vec![("progress".to_string(), "1.0".to_string())]
        );
        assert_eq!(
            file_id(&db, "Goal.md").await,
            id,
            "file_id стабилен (UPSERT по пути)"
        );
    }

    // ── RAG (Ф1-5): чанки + эмбеддинги + usearch ──────────────────────────────────────────────

    use crate::ai::{default_prefixes, MockEmbedder, OpenAiEmbedder};

    /// Индексатор с RAG поверх детерминированного мок-эмбеддера и собственного usearch-файла.
    fn rag_indexer(
        db: &Database,
        root: &Path,
        dim: usize,
        force: bool,
    ) -> (Indexer, Arc<VectorIndex>) {
        let path = root.join(".nexus").join("vectors.usearch");
        let vectors = Arc::new(VectorIndex::open(path, dim).unwrap());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder { dim });
        let idx = Indexer::with_rag(db, root.to_path_buf(), embedder, vectors.clone(), force);
        (idx, vectors)
    }

    async fn chunk_count(db: &Database) -> i64 {
        db.reader()
            .query(|c| c.query_row("SELECT count(*) FROM chunks", [], |r| r.get(0)))
            .await
            .unwrap()
    }

    async fn fts_hits(db: &Database, term: &str) -> i64 {
        let term = term.to_string();
        db.reader()
            .query(move |c| {
                c.query_row(
                    "SELECT count(*) FROM fts_chunks WHERE fts_chunks MATCH ?1",
                    [term],
                    |r| r.get(0),
                )
            })
            .await
            .unwrap()
    }

    /// AC-Б4-1 / AC-Б8-1: индексация пишет чанки, наполняет FTS и кладёт по вектору на чанк.
    #[tokio::test]
    async fn rag_index_writes_chunks_fts_and_vectors() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(
            root.join("Note.md"),
            "# Heading\n\nalpha beta gamma vector search body text here\n",
        )
        .unwrap();

        let db = open(&root).await;
        let (idx, vectors) = rag_indexer(&db, &root, 16, false);
        idx.index_file("Note.md").await.unwrap();

        let n = chunk_count(&db).await;
        assert!(n >= 1, "должен появиться хотя бы один чанк");
        assert_eq!(vectors.len(), n as usize, "по вектору на чанк (AC-Б4-1)");
        assert_eq!(fts_hits(&db, "vector").await, 1, "FTS находит тело чанка");
    }

    /// AC-Б9 (V2.2): rename сохраняет чанки и векторы под тем же `file_id` (не пересоздаёт) —
    /// чистый rename проходит через ранний выход `index_file` (mtime/size не изменились).
    #[tokio::test]
    async fn rename_preserves_chunks_and_vectors() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(
            root.join("Old.md"),
            "# Heading\n\nalpha beta gamma vector search body text here\n",
        )
        .unwrap();

        let db = open(&root).await;
        let (idx, vectors) = rag_indexer(&db, &root, 16, false);
        idx.index_file("Old.md").await.unwrap();
        let before = chunk_count(&db).await;
        assert!(before >= 1, "должен появиться хотя бы один чанк");
        let old_id = file_id(&db, "Old.md").await;

        fs::rename(root.join("Old.md"), root.join("New.md")).unwrap();
        idx.rename_file("Old.md", "New.md").await.unwrap();

        assert_eq!(file_id(&db, "New.md").await, old_id, "file_id сохранён");
        assert_eq!(chunk_count(&db).await, before, "число чанков не изменилось");
        assert_eq!(
            vectors.len(),
            before as usize,
            "векторы целы (по одному на чанк)"
        );
        assert_eq!(
            fts_hits(&db, "vector").await,
            1,
            "FTS по-прежнему находит чанк переименованного файла"
        );
    }

    /// AC-Б4-2 (интеграция): реиндексация заменяет чанки и векторы без осиротевших — число
    /// векторов = числу чанков, старый текст уходит из FTS, новый появляется.
    #[tokio::test]
    async fn reindex_replaces_chunks_and_vectors_without_orphans() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(
            root.join("Note.md"),
            "# H\n\nalpha vector search body words\n",
        )
        .unwrap();

        let db = open(&root).await;
        let (idx, vectors) = rag_indexer(&db, &root, 16, false);
        idx.index_file("Note.md").await.unwrap();
        assert_eq!(fts_hits(&db, "vector").await, 1);

        // Иное содержимое (другой размер → mtime-шорткат не сработает) → полная замена.
        fs::write(root.join("Note.md"), "# H\n\ndelta epsilon zeta\n").unwrap();
        idx.index_file("Note.md").await.unwrap();

        assert_eq!(
            vectors.len(),
            chunk_count(&db).await as usize,
            "нет осиротевших векторов после реиндексации (AC-Б4-2)"
        );
        assert_eq!(fts_hits(&db, "vector").await, 0, "старый текст ушёл из FTS");
        assert_eq!(fts_hits(&db, "delta").await, 1, "новый текст попал в FTS");
    }

    /// AC-Б8-2 (интеграция): удаление файла чистит и чанки (+FTS), и векторы usearch.
    #[tokio::test]
    async fn remove_file_purges_chunks_and_vectors() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("Note.md"), "# H\n\nalpha vector beta gamma\n").unwrap();

        let db = open(&root).await;
        let (idx, vectors) = rag_indexer(&db, &root, 16, false);
        idx.index_file("Note.md").await.unwrap();
        assert!(!vectors.is_empty());

        idx.remove_file("Note.md").await.unwrap();
        assert_eq!(chunk_count(&db).await, 0, "чанки удалены");
        assert_eq!(vectors.len(), 0, "векторы удалены из usearch");
        assert_eq!(fts_hits(&db, "vector").await, 0, "FTS чист");
    }

    /// §6.5 (AC-Б5-2): `force` переиндексирует НЕизменённый файл (mtime/size те же) — так после
    /// смены модели чанки и векторы перестраиваются, хотя файлы на диске не трогали.
    #[tokio::test]
    async fn force_reindex_rebuilds_unchanged_file() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(
            root.join("Note.md"),
            "# H\n\nalpha vector beta gamma delta\n",
        )
        .unwrap();

        let db = open(&root).await;
        let (idx, _v1) = rag_indexer(&db, &root, 16, false);
        idx.index_file("Note.md").await.unwrap();
        let n = chunk_count(&db).await;
        assert!(n >= 1);

        // Имитируем смену модели: чанки очищены (как делает reconcile), usearch — новый файл.
        db.writer()
            .call(|c| c.execute("DELETE FROM chunks", []).map(|_| ()))
            .await
            .unwrap();
        assert_eq!(chunk_count(&db).await, 0);

        let vectors2 =
            Arc::new(VectorIndex::open(root.join(".nexus").join("vectors2.usearch"), 16).unwrap());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder { dim: 16 });
        let idx2 = Indexer::with_rag(&db, root.clone(), embedder, vectors2.clone(), true);
        idx2.index_file("Note.md").await.unwrap(); // файл НЕ менялся, но force обходит шорткат

        assert_eq!(
            chunk_count(&db).await,
            n,
            "force переиндексировал несмотря на mtime-шорткат (§6.5)"
        );
        assert_eq!(vectors2.len(), n as usize);
    }

    /// §5.1 crash-reconcile: потерянный вектор (chunks в БД есть, вектора в usearch нет) дочиняется.
    #[tokio::test]
    async fn reconcile_restores_lost_vectors() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(
            root.join("Note.md"),
            "# H\n\nalpha vector beta gamma delta\n",
        )
        .unwrap();

        let db = open(&root).await;
        let (idx, vectors) = rag_indexer(&db, &root, 16, false);
        idx.index_file("Note.md").await.unwrap();
        let n = vectors.len();
        assert!(n >= 1);

        // Имитируем крах: вектор пропал из usearch, но чанк в БД остался.
        let lost: i64 = db
            .reader()
            .query(|c| c.query_row("SELECT id FROM chunks LIMIT 1", [], |r| r.get(0)))
            .await
            .unwrap();
        vectors.remove(lost as u64).unwrap();
        assert!(!vectors.contains(lost as u64));
        assert_eq!(vectors.len(), n - 1);

        // reconcile переэмбеддит и возвращает потерянный вектор.
        idx.reconcile_vectors().await.unwrap();
        assert!(
            vectors.contains(lost as u64),
            "reconcile вернул потерянный вектор"
        );
        assert_eq!(vectors.len(), n);
    }

    /// Живой end-to-end против nomic на :8081 (`cargo test -- --ignored`): индексируем два файла,
    /// семантический запрос про кошку находит чанк именно из cat.md (а не из физики).
    #[tokio::test]
    #[ignore = "нужен embedding-сервер на 127.0.0.1:8081"]
    async fn live_rag_index_and_semantic_search() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(
            root.join("cat.md"),
            "# Кошка\n\nКошка сидит на тёплом коврике у окна и довольно мурлычет.\n",
        )
        .unwrap();
        fs::write(
            root.join("physics.md"),
            "# Физика\n\nКвантовая хромодинамика описывает сильное взаимодействие кварков.\n",
        )
        .unwrap();

        let db = open(&root).await;
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(
            OpenAiEmbedder::new(
                "http://127.0.0.1:8081",
                "nomic-embed-text",
                768,
                default_prefixes("nomic-embed-text"),
            )
            .unwrap(),
        );
        let vectors =
            Arc::new(VectorIndex::open(root.join(".nexus").join("vectors.usearch"), 768).unwrap());
        let idx = Indexer::with_rag(&db, root.clone(), embedder.clone(), vectors.clone(), true);
        idx.index_file("cat.md").await.unwrap();
        idx.index_file("physics.md").await.unwrap();
        assert!(vectors.len() >= 2, "оба файла дали векторы");

        let q = embedder.embed_query("где находится кошка?").await.unwrap();
        let hits = vectors.search(&q, 1).unwrap();
        let top = hits[0].chunk_id as i64;
        let path: String = db
            .reader()
            .query(move |c| {
                c.query_row(
                    "SELECT f.path FROM chunks ch JOIN files f ON f.id=ch.file_id WHERE ch.id=?1",
                    [top],
                    |r| r.get(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(
            path, "cat.md",
            "ближайший к запросу про кошку чанк — из cat.md"
        );
    }
}
