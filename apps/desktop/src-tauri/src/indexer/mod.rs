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

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures::stream::{self, StreamExt};
use rusqlite::{params, OptionalExtension};
use tokio::sync::Semaphore;

use crate::ai::EmbeddingProvider;
use crate::chunker::{self, ChunkOptions, WordTokenizer};
use crate::db::{Database, DbError, DbResult, ReadPool, WriteActor};
use crate::parser;
use crate::vector::VectorIndex;

// Подмодули индексатора (декомпозиция #28): резолв ссылок, ФС-помощники, watcher-петля, RAG-механика.
mod events;
mod fs;
mod links;
mod rag;
#[cfg(test)]
mod tests;

pub use events::spawn;
/// Ре-экспорт для команды `resolve_note` (кросс-план #22): клик по `[[ссылке]]` резолвится той же
/// функцией, что и индексация links — одна семантика (путь/±.md/basename, затем алиас).
pub(crate) use links::resolve_target;

use fs::{collect_md, mtime_secs, now_secs};
use links::{path_forms, resolve_all_dangling};

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
}
