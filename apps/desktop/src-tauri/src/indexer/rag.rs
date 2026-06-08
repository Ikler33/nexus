//! RAG-механика индексатора (Ф1-5, §5.1): эмбеддинг чанков, crash-reconcile потерянных векторов и
//! персист usearch. Это методы [`Indexer`], вынесенные из оркестрации (`index_file`/`scan_vault`)
//! для когезии «векторной» части. Активны только при включённом RAG (`self.rag.is_some()`).

use crate::chunker;
use crate::db::{DbError, DbResult};

use super::{Indexer, EMBED_BATCH};

impl Indexer {
    /// Эмбеддит чанки батчами по [`EMBED_BATCH`] под семафором конкуренции. Возвращает векторы
    /// 1:1 ко входу. Только при включённом RAG (иначе вызывающий не дойдёт сюда).
    pub(super) async fn embed_chunks(&self, chunks: &[chunker::Chunk]) -> DbResult<Vec<Vec<f32>>> {
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
