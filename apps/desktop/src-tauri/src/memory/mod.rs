//! Персистентная память агента (MEM, vision-фича; спека `docs/specs/agent-memory.md`): слой ЯВНЫХ
//! ФАКТОВ о пользователе/проектах, отдельный от RAG-по-переписке (N4b/`chat_log`). Факты курирует
//! пользователь (D1/D4); инжектятся в контекст ответа ИИ — пины «всегда» + top-k семантически близких
//! (D2). Эмбеддинги — в параллельном usearch-индексе `memory_vectors` (ключ = `memory_facts.id`).
//!
//! DB-операции (этот слой) отделены от эмбеддинг-индекса (`index_fact`/`unindex_fact`): команда
//! оркестрирует «add → index_fact (если есть провайдер/индекс)», что упрощает тесты (БД без эмбеддера).

pub mod extract;

use rusqlite::params;
use serde::Serialize;

use crate::ai::EmbeddingProvider;
use crate::db::{DbError, DbResult, ReadPool, WriteActor};
use crate::scheduler::now_secs;
use crate::vector::VectorIndex;

/// Мягкий кап числа фактов (D6): не авто-эвикция, а подсветка старых для ручной чистки. Пины не считаются.
pub const MEM_CAP: usize = 200;

/// Источник факта (D1).
pub const SOURCE_EXPLICIT: &str = "explicit";
pub const SOURCE_AUTO: &str = "auto";

/// Факт памяти агента.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryFact {
    pub id: i64,
    pub text: String,
    pub pinned: bool,
    pub source: String,
    pub created_at: i64,
    pub used_at: i64,
}

fn row_to_fact(r: &rusqlite::Row) -> rusqlite::Result<MemoryFact> {
    Ok(MemoryFact {
        id: r.get(0)?,
        text: r.get(1)?,
        pinned: r.get::<_, i64>(2)? != 0,
        source: r.get(3)?,
        created_at: r.get(4)?,
        used_at: r.get(5)?,
    })
}

const SELECT_COLS: &str = "id, text, pinned, source, created_at, used_at";

/// AC-MEM-1: добавить факт в БД (дедуп по точному тексту — UNIQUE index). Пустой/whitespace отклоняется.
/// Возвращает id (нового или уже существующего дубля), либо `None` для пустого текста. Эмбеддинг — отдельно
/// (`index_fact`), чтобы DB-слой не зависел от провайдера.
/// Добавить факт. Возвращает `Some((id, inserted))`: `inserted=true` — НОВАЯ строка, `false` — дубль
/// (вернули существующий id, не плодим, AC-MEM-1). `None` — пустой текст. Флаг `inserted` критичен для
/// фронта (MEM-5): «Отменить» удаляет факт ТОЛЬКО если мы его реально создали — иначе undo стёр бы
/// уже существующий курированный факт пользователя (adversarial-ревью MEM-5, MAJOR).
pub async fn add(writer: &WriteActor, text: &str, source: &str) -> DbResult<Option<(i64, bool)>> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Ok(None);
    }
    let src = source.to_string();
    let now = now_secs();
    writer
        .transaction(move |tx| {
            let changed = tx.execute(
                "INSERT OR IGNORE INTO memory_facts(text,pinned,source,created_at,used_at) \
                 VALUES(?1,0,?2,?3,0)",
                params![text, src, now],
            )?;
            let id = if changed == 0 {
                // Дубль — вернём существующий id (не плодим, AC-MEM-1).
                tx.query_row("SELECT id FROM memory_facts WHERE text=?1", [&text], |r| {
                    r.get(0)
                })?
            } else {
                tx.last_insert_rowid()
            };
            Ok(Some((id, changed != 0)))
        })
        .await
}

/// AC-MEM-2: все факты — пины сверху, затем по `created_at` desc.
pub async fn list(reader: &ReadPool) -> DbResult<Vec<MemoryFact>> {
    reader
        .query(move |c| {
            let sql = format!(
                "SELECT {SELECT_COLS} FROM memory_facts ORDER BY pinned DESC, created_at DESC"
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt.query_map([], row_to_fact)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
}

/// AC-MEM-3: пин/анпин.
pub async fn set_pinned(writer: &WriteActor, id: i64, pinned: bool) -> DbResult<()> {
    writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE memory_facts SET pinned=?2 WHERE id=?1",
                params![id, pinned as i64],
            )?;
            Ok(())
        })
        .await
}

/// AC-MEM-3: правка текста факта (вызывающий ре-эмбеддит через `index_fact`). Пустой текст — no-op.
pub async fn edit(writer: &WriteActor, id: i64, text: &str) -> DbResult<()> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Ok(());
    }
    writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE memory_facts SET text=?2 WHERE id=?1",
                params![id, text],
            )?;
            Ok(())
        })
        .await
}

/// AC-MEM-3: удалить факт из БД (вектор — `unindex_fact`).
pub async fn delete(writer: &WriteActor, id: i64) -> DbResult<()> {
    writer
        .transaction(move |tx| {
            tx.execute("DELETE FROM memory_facts WHERE id=?1", [id])?;
            Ok(())
        })
        .await
}

/// AC-MEM-9 / D6: вычистить всю память — ручное действие «очистить всю память» (панель MEM-4); вызывающий
/// также пересоздаёт `memory_vectors` (как chat_vectors). Изоляция между vault'ами — структурная (свой
/// `.nexus` на vault), отдельной очистки не требует.
pub async fn clear(writer: &WriteActor) -> DbResult<()> {
    writer
        .transaction(move |tx| {
            tx.execute("DELETE FROM memory_facts", [])?;
            Ok(())
        })
        .await
}

/// Число фактов (для кап-подсветки D6).
pub async fn count(reader: &ReadPool) -> DbResult<usize> {
    reader
        .query(move |c| {
            let n: i64 = c.query_row("SELECT count(*) FROM memory_facts", [], |r| r.get(0))?;
            Ok(n as usize)
        })
        .await
}

/// Эмбеддит текст факта и кладёт в `memory_vectors` (ключ = id). Best-effort на уровне команды.
pub async fn index_fact(
    vectors: &VectorIndex,
    embedder: &dyn EmbeddingProvider,
    id: i64,
    text: &str,
) -> DbResult<()> {
    let vec = embedder
        .embed_query(text)
        .await
        .map_err(|e| DbError::External(e.to_string()))?;
    vectors
        .upsert(id as u64, &vec)
        .map_err(|e| DbError::External(e.to_string()))?;
    vectors
        .save()
        .map_err(|e| DbError::External(e.to_string()))?;
    Ok(())
}

/// Убирает факт из `memory_vectors` (после delete/перед ре-эмбеддингом edit).
pub fn unindex_fact(vectors: &VectorIndex, id: i64) -> DbResult<()> {
    vectors
        .remove(id as u64)
        .map_err(|e| DbError::External(e.to_string()))?;
    vectors
        .save()
        .map_err(|e| DbError::External(e.to_string()))?;
    Ok(())
}

/// Достаёт факты по id, сохраняя порядок `ids` (для ранжированной выдачи поиска).
async fn facts_by_ids(reader: &ReadPool, ids: Vec<i64>) -> DbResult<Vec<MemoryFact>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    reader
        .query(move |c| {
            let mut out = Vec::new();
            let sql = format!("SELECT {SELECT_COLS} FROM memory_facts WHERE id=?1");
            let mut stmt = c.prepare(&sql)?;
            for id in &ids {
                if let Ok(f) = stmt.query_row([id], row_to_fact) {
                    out.push(f);
                }
            }
            Ok(out)
        })
        .await
}

/// AC-MEM-4 (D2): факты для контекста ответа — ВСЕ пины + top-k не-пинов по близости к `query`.
/// Обновляет `used_at` возвращённых. Пустой query/k=0/пустой индекс → только пины (без поиска).
pub async fn context_facts(
    reader: &ReadPool,
    writer: &WriteActor,
    vectors: &VectorIndex,
    embedder: &dyn EmbeddingProvider,
    query: &str,
    k: usize,
) -> DbResult<Vec<MemoryFact>> {
    // Пины — всегда.
    let mut pinned: Vec<MemoryFact> = reader
        .query(move |c| {
            let sql = format!(
                "SELECT {SELECT_COLS} FROM memory_facts WHERE pinned=1 ORDER BY created_at DESC"
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt.query_map([], row_to_fact)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .await?;

    // top-k не-пинов по близости.
    let mut topk: Vec<MemoryFact> = Vec::new();
    if k > 0 && !query.trim().is_empty() && !vectors.is_empty() {
        let qvec = embedder
            .embed_query(query)
            .await
            .map_err(|e| DbError::External(e.to_string()))?;
        let hits = vectors
            .search(&qvec, (k * 4).max(8))
            .map_err(|e| DbError::External(e.to_string()))?;
        let pinned_ids: std::collections::HashSet<i64> = pinned.iter().map(|f| f.id).collect();
        let ranked: Vec<i64> = hits
            .into_iter()
            .map(|h| h.chunk_id as i64)
            .filter(|id| !pinned_ids.contains(id))
            .collect();
        topk = facts_by_ids(reader, ranked).await?;
        topk.truncate(k);
    }

    let now = now_secs();
    let used_ids: Vec<i64> = pinned.iter().chain(topk.iter()).map(|f| f.id).collect();
    if !used_ids.is_empty() {
        let ids = used_ids.clone();
        writer
            .transaction(move |tx| {
                for id in &ids {
                    tx.execute(
                        "UPDATE memory_facts SET used_at=?2 WHERE id=?1",
                        params![id, now],
                    )?;
                }
                Ok(())
            })
            .await?;
    }
    pinned.append(&mut topk);
    Ok(pinned)
}

#[cfg(test)]
mod tests;
