//! RAG-механика индексатора (Ф1-5, §5.1): эмбеддинг чанков, crash-reconcile потерянных векторов и
//! персист usearch. Это методы [`Indexer`], вынесенные из оркестрации (`index_file`/`scan_vault`)
//! для когезии «векторной» части. Активны только при включённом RAG (`self.rag.is_some()`).

use crate::chunker;
use crate::db::{DbError, DbResult};

use super::{Indexer, EMBED_BATCH};

impl Indexer {
    /// Эмбеддит чанки батчами по [`EMBED_BATCH`] под семафором конкуренции. Возвращает векторы
    /// 1:1 ко входу. Только при включённом RAG (иначе вызывающий не дойдёт сюда).
    /// Во время полного скана сперва смотрит в кэш группы (cross-file батчинг — префилл уже
    /// сэмбеддил полными батчами): попадание = 0 сетевых вызовов; промахи (файл изменился между
    /// префиллом и индексом) добиваются сетью как раньше.
    pub(super) async fn embed_chunks(&self, chunks: &[chunker::Chunk]) -> DbResult<Vec<Vec<f32>>> {
        let rag = self.rag.as_ref().expect("embed_chunks без RAG");
        if let Some(cache) = rag.scan_cache.lock().expect("scan_cache lock").as_ref() {
            if let Some(hit) = chunks
                .iter()
                .map(|c| cache.get(c.content.as_str()).cloned())
                .collect::<Option<Vec<_>>>()
            {
                return Ok(hit);
            }
        }
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

    /// Префилл кэша группы (cross-file батчинг, perf.md): читает/чанкует файлы группы (тем же
    /// mtime+size-шорткатом, что `index_file` — неизменённые не эмбеддим зря) и эмбеддит ВСЕ их
    /// чанки полными батчами по [`EMBED_BATCH`] с конкуренцией семафора. Ошибки сети — не фатальны:
    /// промахи кэша доберёт пер-файловый путь (тот честно вернёт ошибку индексации файла).
    pub(super) async fn prefill_scan_cache(&self, group: &[String]) {
        use futures::stream::{self, StreamExt};
        let Some(rag) = &self.rag else { return };
        // 1) Собираем тексты чанков группы (дёшево: IO+чанкер, без сети).
        let mut texts: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for rel in group {
            if !rel.ends_with(".md") {
                continue;
            }
            let abs = self.root.join(rel);
            let Ok(meta) = tokio::fs::metadata(&abs).await else {
                continue;
            };
            if !self.force.load(std::sync::atomic::Ordering::Relaxed) {
                let unchanged = self
                    .reader
                    .query({
                        let rel = rel.clone();
                        move |c| {
                            use rusqlite::OptionalExtension;
                            c.query_row(
                                "SELECT updated_at, size_bytes FROM files WHERE path=?1 AND is_deleted=0",
                                [rel],
                                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
                            )
                            .optional()
                        }
                    })
                    .await
                    .ok()
                    .flatten();
                if let Some((u, s)) = unchanged {
                    if u == super::fs::mtime_secs(&meta) && s == meta.len() as i64 {
                        continue; // index_file всё равно скипнет — не жжём эмбеддинги
                    }
                }
            }
            let Ok(content) = tokio::fs::read_to_string(&abs).await else {
                continue;
            };
            for ch in
                chunker::chunk_document(&content, &crate::chunker::WordTokenizer, rag.chunk_opts)
            {
                if seen.insert(ch.content.clone()) {
                    texts.push(ch.content);
                }
            }
        }
        if texts.is_empty() {
            return;
        }
        // 2) Эмбеддим ПОЛНЫМИ батчами поперёк файлов, конкурентно под семафором.
        let batches: Vec<Vec<String>> = texts.chunks(EMBED_BATCH).map(|b| b.to_vec()).collect();
        /// Сэмбежженный батч: исходные тексты + их векторы 1:1.
        type EmbeddedBatch = (Vec<String>, Vec<Vec<f32>>);
        let results: Vec<Option<EmbeddedBatch>> = stream::iter(batches)
            .map(|batch| async move {
                let _permit = rag.embed_sem.acquire().await.ok()?;
                let refs: Vec<&str> = batch.iter().map(|s| s.as_str()).collect();
                match rag.embedder.embed_documents(&refs).await {
                    Ok(vecs) => Some((batch, vecs)),
                    Err(e) => {
                        tracing::warn!(error = %e, "префилл-батч не сэмбеддился — доберём пофайлово");
                        None
                    }
                }
            })
            .buffer_unordered(super::EMBED_CONCURRENCY)
            .collect()
            .await;
        let mut map = std::collections::HashMap::new();
        for (batch, vecs) in results.into_iter().flatten() {
            for (t, v) in batch.into_iter().zip(vecs) {
                map.insert(t, v);
            }
        }
        tracing::debug!(
            chunks = map.len(),
            "scan-cache группы заполнен (cross-file батчи)"
        );
        *rag.scan_cache.lock().expect("scan_cache lock") = Some(map);
    }

    /// Сбрасывает кэш группы (граница группы скана / конец скана).
    pub(super) fn clear_scan_cache(&self) {
        if let Some(rag) = &self.rag {
            *rag.scan_cache.lock().expect("scan_cache lock") = None;
        }
    }

    /// **Crash-reconcile usearch (§5.1).** Для чанков, что есть в БД, но чьих векторов нет в usearch
    /// (commit прошёл, `save` усearch — нет), переэмбеддит содержимое и доливает векторы. На force-скане
    /// обычно no-op (все чанки только что переиндексированы). Best-effort: при недоступном эмбеддере
    /// логирует и выходит — повторит при следующем открытии. Закрывает рассинхрон из docs/vector.md.
    pub(super) async fn reconcile_vectors(&self) -> DbResult<()> {
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
    pub(super) fn persist_vectors(&self) {
        if let Some(rag) = &self.rag {
            if let Err(e) = rag.vectors.save() {
                tracing::warn!(error = %e, "usearch save failed");
            }
        }
    }
}
