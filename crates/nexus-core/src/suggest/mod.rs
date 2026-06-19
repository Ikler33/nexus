//! Предложения связей (§6, Ф1-9) — **режим 1 (max-sim)**: для открытого файла находим семантически
//! близкие ДРУГИЕ заметки, которых ещё нет в его ссылках, и предлагаем как кандидатов на `[[wikilink]]`.
//!
//! Считаем НА ЛЕТУ из уже сохранённых в usearch векторов чанков (эмбеддер-сервер не нужен): для
//! каждого чанка файла берём ближайших соседей (исключив чанки самого файла), агрегируем по целевому
//! файлу по МАКСИМУМУ similarity (max-sim), отбрасываем уже связанные файлы, сортируем, режем порог.
//! Кэш-таблица `link_suggestions` (чтобы не пересчитывать) и режим 2 (LLM) — позже (см. BACKLOG).

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::db::{DbResult, ReadPool};
use crate::vector::VectorIndex;

/// Сколько соседей запрашиваем на каждый чанк файла (до агрегации по файлам).
const NEIGHBORS_PER_CHUNK: usize = 10;
/// Порог max-sim: ниже — не предлагаем (калибровка — eval-харнесс Ф1-10, см. BACKLOG).
const MIN_SCORE: f32 = 0.55;
/// Длина «причины» (сниппета лучшего совпавшего чанка).
const REASON_CHARS: usize = 160;

/// Предложенная связь: близкий файл + max-sim score + «причина» (сниппет совпавшего чанка).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkSuggestion {
    pub path: String,
    pub title: Option<String>,
    pub score: f32,
    pub reason: String,
}

/// Кандидаты на связь для `path` (режим 1, max-sim). Пустой результат, если файл не проиндексирован,
/// нет чанков/векторов, или все близкие уже связаны.
pub async fn get_link_suggestions(
    reader: &ReadPool,
    vectors: &VectorIndex,
    path: String,
    limit: usize,
) -> DbResult<Vec<LinkSuggestion>> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let mut out: Vec<LinkSuggestion> = collect_related(reader, vectors, &path, true)
        .await?
        .into_iter()
        .filter(|s| s.score >= MIN_SCORE)
        .collect();
    sort_truncate(&mut out, limit);
    Ok(out)
}

/// «Похожие заметки» (#35, режим дискавери): семантически близкие заметки, **включая уже связанные**
/// (отличие от `get_link_suggestions`, который их вычитает). Порог релевантности — на стороне UI
/// (настройка с v1), поэтому здесь без жёсткой отсечки: отдаём топ-`limit` по max-sim. Пусто, если
/// файл не проиндексирован / нет векторов.
pub async fn get_related_notes(
    reader: &ReadPool,
    vectors: &VectorIndex,
    path: String,
    limit: usize,
) -> DbResult<Vec<LinkSuggestion>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut out = collect_related(reader, vectors, &path, false).await?;
    sort_truncate(&mut out, limit);
    Ok(out)
}

/// Сортировка по score (убыв.), тай-брейк по пути (детерминизм), усечение до `limit`.
fn sort_truncate(out: &mut Vec<LinkSuggestion>, limit: usize) {
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.path.cmp(&b.path))
    });
    out.truncate(limit);
}

/// Общее ядро max-sim для «Связей» (Ф1-9) и «Похожих» (#35): файлы-кандидаты с max-sim score и
/// «причиной» (сниппет лучшего чанка). `exclude_linked=true` — режим предложений связей (только
/// несвязанные); `false` — дискавери «Похожие» (include_linked). БЕЗ порога/сортировки/усечения —
/// это делают вызывающие. Сам файл исключается всегда.
async fn collect_related(
    reader: &ReadPool,
    vectors: &VectorIndex,
    path: &str,
    exclude_linked: bool,
) -> DbResult<Vec<LinkSuggestion>> {
    let Some((file_id, chunk_ids, linked)) = file_context(reader, path).await? else {
        return Ok(Vec::new());
    };
    if chunk_ids.is_empty() {
        return Ok(Vec::new());
    }
    let own: HashSet<u64> = chunk_ids.iter().map(|&id| id as u64).collect();

    // Для каждого чанка — ближайшие соседи (мимо чанков самого файла). usearch sync, быстрый.
    let mut candidates: Vec<(i64, f32)> = Vec::new();
    for &cid in &chunk_ids {
        let Some(vec) = vectors
            .get_vector(cid as u64)
            .map_err(|e| crate::db::DbError::External(e.to_string()))?
        else {
            continue;
        };
        let hits = vectors
            .search_filtered(&vec, NEIGHBORS_PER_CHUNK, |id| !own.contains(&id))
            .map_err(|e| crate::db::DbError::External(e.to_string()))?;
        for h in hits {
            candidates.push((h.chunk_id as i64, h.score));
        }
    }
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    // Агрегация по файлу (max-sim): лучший чанк определяет score и «причину».
    let cand_ids: Vec<i64> = candidates.iter().map(|(id, _)| *id).collect();
    let meta = chunk_file_meta(reader, &cand_ids).await?;

    // file_id -> (best_score, path, title, reason)
    let mut best: HashMap<i64, (f32, String, Option<String>, String)> = HashMap::new();
    for (chunk_id, score) in candidates {
        let Some((tgt_file, tgt_path, tgt_title, content)) = meta.get(&chunk_id) else {
            continue;
        };
        if *tgt_file == file_id || (exclude_linked && linked.contains(tgt_file)) {
            continue; // сам файл (всегда) или уже связан (только режим «Связи»)
        }
        let entry = best.entry(*tgt_file).or_insert((
            f32::MIN,
            tgt_path.clone(),
            tgt_title.clone(),
            String::new(),
        ));
        if score > entry.0 {
            *entry = (
                score,
                tgt_path.clone(),
                tgt_title.clone(),
                reason_of(content),
            );
        }
    }

    Ok(best
        .into_values()
        .map(|(score, path, title, reason)| LinkSuggestion {
            path,
            title,
            score,
            reason,
        })
        .collect())
}

/// `(file_id, chunk_ids, linked_file_ids)` для пути. `None`, если файла нет (или удалён).
async fn file_context(
    reader: &ReadPool,
    path: &str,
) -> DbResult<Option<(i64, Vec<i64>, HashSet<i64>)>> {
    let path = path.to_string();
    reader
        .query(move |c| {
            let file_id: Option<i64> = c
                .query_row(
                    "SELECT id FROM files WHERE path = ?1 AND is_deleted = 0",
                    [&path],
                    |r| r.get(0),
                )
                .ok();
            let Some(file_id) = file_id else {
                return Ok(None);
            };

            let chunk_ids = c
                .prepare("SELECT id FROM chunks WHERE file_id = ?1")?
                .query_map([file_id], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            // Связанные в обе стороны (исходящие + входящие) — их не предлагаем.
            let linked = c
                .prepare(
                    "SELECT target_id FROM links WHERE source_id = ?1 AND target_id IS NOT NULL \
                     UNION SELECT source_id FROM links WHERE target_id = ?1",
                )?
                .query_map([file_id], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<HashSet<_>>>()?;

            Ok(Some((file_id, chunk_ids, linked)))
        })
        .await
}

/// Метаданные чанков-кандидатов: `chunk_id -> (file_id, path, title, content)`.
#[allow(clippy::type_complexity)]
async fn chunk_file_meta(
    reader: &ReadPool,
    ids: &[i64],
) -> DbResult<HashMap<i64, (i64, String, Option<String>, String)>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids = ids.to_vec();
    reader
        .query(move |c| {
            let ph = vec!["?"; ids.len()].join(",");
            let sql = format!(
                "SELECT ch.id, ch.file_id, f.path, f.title, ch.content \
                 FROM chunks ch JOIN files f ON f.id = ch.file_id \
                 WHERE f.is_deleted = 0 AND ch.id IN ({ph})"
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    (
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, String>(4)?,
                    ),
                ))
            })?;
            let mut map = HashMap::new();
            for row in rows {
                let (id, v) = row?;
                map.insert(id, v);
            }
            Ok(map)
        })
        .await
}

/// «Причина»: схлопнутый и обрезанный по символам сниппет лучшего совпавшего чанка.
fn reason_of(content: &str) -> String {
    let collapsed = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= REASON_CHARS {
        return collapsed;
    }
    let cut: String = collapsed.chars().take(REASON_CHARS).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{EmbeddingProvider, MockEmbedder};
    use crate::db::Database;
    use crate::indexer::Indexer;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn index(root: &std::path::Path, files: &[(&str, &str)]) -> (Database, Arc<VectorIndex>) {
        for (name, body) in files {
            if let Some(parent) = std::path::Path::new(name).parent() {
                fs::create_dir_all(root.join(parent)).ok();
            }
            fs::write(root.join(name), body).unwrap();
        }
        let db = Database::open(root.join(".nexus/nexus.db")).await.unwrap();
        let vectors =
            Arc::new(VectorIndex::open(root.join(".nexus").join("vectors.usearch"), 16).unwrap());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder { dim: 16 });
        let idx = Indexer::with_rag(&db, root.to_path_buf(), embedder, vectors.clone(), true);
        for (name, _) in files {
            idx.index_file(name).await.unwrap();
        }
        (db, vectors)
    }

    /// max-sim: похожая (тем же содержимым) НЕсвязанная заметка предлагается; сам файл — нет.
    #[tokio::test]
    async fn suggests_similar_unlinked_note() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        // A и B — одинаковое тело (mock-векторы совпадут → max-sim ≈ 1), C — иное.
        let body = "# Note\n\nalpha beta gamma delta epsilon zeta semantic body text here\n";
        let (db, vectors) = index(
            &root,
            &[
                ("A.md", body),
                ("B.md", body),
                ("C.md", "# C\n\nполностью другой непохожий текст про борщ\n"),
            ],
        )
        .await;

        let sug = get_link_suggestions(db.reader(), &vectors, "A.md".into(), 5)
            .await
            .unwrap();
        assert!(sug.iter().any(|s| s.path == "B.md"), "похожая B предложена");
        assert!(
            sug.iter().all(|s| s.path != "A.md"),
            "сам файл не предлагается"
        );
        assert!(sug.iter().all(|s| s.score >= MIN_SCORE));
        let b = sug.iter().find(|s| s.path == "B.md").unwrap();
        assert!(!b.reason.is_empty(), "есть причина-сниппет");
    }

    /// Уже связанные файлы из предложений исключаются.
    #[tokio::test]
    async fn excludes_already_linked() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let body = "alpha beta gamma delta epsilon zeta semantic body text here";
        let (db, vectors) = index(
            &root,
            &[
                ("A.md", &format!("# A\n\n{body}\n\nсм. [[B]]\n")), // A уже ссылается на B
                ("B.md", &format!("# B\n\n{body}\n")),
            ],
        )
        .await;

        let sug = get_link_suggestions(db.reader(), &vectors, "A.md".into(), 5)
            .await
            .unwrap();
        assert!(
            sug.iter().all(|s| s.path != "B.md"),
            "уже связанная B не предлагается"
        );
    }

    /// «Похожие» (#35): в отличие от «Связей», уже связанная похожая заметка ПРИСУТСТВУЕТ
    /// (дискавери, include_linked — AC-RN-2); сам файл — никогда (AC-RN-3).
    #[tokio::test]
    async fn related_includes_linked_similar() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let body = "alpha beta gamma delta epsilon zeta semantic body text here";
        let (db, vectors) = index(
            &root,
            &[
                ("A.md", &format!("# A\n\n{body}\n\nсм. [[B]]\n")), // A УЖЕ ссылается на B
                ("B.md", &format!("# B\n\n{body}\n")),
            ],
        )
        .await;

        let rel = get_related_notes(db.reader(), &vectors, "A.md".into(), 10)
            .await
            .unwrap();
        assert!(
            rel.iter().any(|s| s.path == "B.md"),
            "связанная похожая B В выдаче (дискавери)"
        );
        assert!(rel.iter().all(|s| s.path != "A.md"), "сам файл не в выдаче");
    }

    /// Живой max-sim на nomic :8081: для заметки про кошку топовое предложение — ДРУГАЯ про кошек
    /// (ранжируется ВЫШЕ далёкой про физику). Абсолютный порог не проверяем: nomic англоцентричен и
    /// на коротких RU-текстах кучкует similarity высоко (зафиксированный риск ADR-005) — значима
    /// именно относительная близость. Векторы реальные (не mock).
    #[tokio::test]
    #[ignore = "нужен embedding-сервер (NEXUS_EMBED_URL, default 192.168.0.31:8083)"]
    async fn live_suggests_topically_similar() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(
            root.join("cat1.md"),
            "# Кошка\n\nМоя кошка любит спать на солнце и ловить мышей.\n",
        )
        .unwrap();
        fs::write(
            root.join("cat2.md"),
            "# Котёнок\n\nКотёнок играет с клубком и охотится на птиц во дворе.\n",
        )
        .unwrap();
        fs::write(
            root.join("phys.md"),
            "# Физика\n\nКвантовая хромодинамика и сильное взаимодействие кварков.\n",
        )
        .unwrap();

        let db = Database::open(root.join(".nexus/nexus.db")).await.unwrap();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::ai::live_test_embedder());
        let vectors = Arc::new(
            VectorIndex::open(
                root.join(".nexus").join("vectors.usearch"),
                crate::ai::LIVE_EMBED_DIM,
            )
            .unwrap(),
        );
        let idx = Indexer::with_rag(&db, root.clone(), embedder, vectors.clone(), true);
        for f in ["cat1.md", "cat2.md", "phys.md"] {
            idx.index_file(f).await.unwrap();
        }

        let sug = get_link_suggestions(db.reader(), &vectors, "cat1.md".into(), 5)
            .await
            .unwrap();
        assert!(!sug.is_empty(), "есть предложения");
        assert_eq!(sug[0].path, "cat2.md", "топовое предложение — близкая cat2");
        // Если phys прошла порог — она строго ниже cat2 по score (max-sim ранжирует ближе).
        if let Some(phys) = sug.iter().find(|s| s.path == "phys.md") {
            let cat2 = sug.iter().find(|s| s.path == "cat2.md").unwrap();
            assert!(cat2.score > phys.score, "cat2 ближе физики");
        }
    }

    /// Файл без чанков/индекса → пустой результат.
    #[tokio::test]
    async fn empty_for_unindexed_file() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let (db, vectors) = index(&root, &[("A.md", "# A\n\nтекст\n")]).await;
        let sug = get_link_suggestions(db.reader(), &vectors, "Nope.md".into(), 5)
            .await
            .unwrap();
        assert!(sug.is_empty());
    }
}
