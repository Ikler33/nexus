//! Поиск: по метаданным (Ф0: title/path/tags) и гибридный по ТЕЛУ (Ф1-6 + доработка §6.2).
//!
//! **Гибрид (§6.2):** до ТРЁХ независимых выдач кандидатов — вектор (usearch, семантика), FTS5/BM25
//! (`fts_chunks`, лексика) и граф (соседи открытого файла) — сливаются через **Reciprocal Rank
//! Fusion** (RRF), не по «сырым» score (разные шкалы). Граф входит ТРЕТЬИМ РАНГОМ в саму RRF-формулу,
//! а НЕ аддитивным `+0.2` (REVIEW С-4). Префильтр по метаданным (папка/тег) применяется ДО KNN
//! (usearch `filtered_search`, AC-Б6-2). Перекрывающиеся соседние чанки одного файла дедуплицируются.
//! Деградация изящная: нет эмбеддера → FTS(+граф); нет центра → без граф-ранга; всё пусто → пусто.

use std::collections::{HashMap, HashSet};

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::ai::EmbeddingProvider;
use crate::db::{DbError, DbResult, ReadPool};
use crate::vault::NoteRef;
use crate::vector::VectorIndex;

/// Сколько кандидатов берём из КАЖДОЙ выдачи до слияния (recall до RRF).
const CANDIDATES: usize = 50;
/// Константа RRF (сглаживание вклада ранга; классическое значение из литературы).
const RRF_K: f32 = 60.0;
/// Потолок длины сниппета (символы исходного чанка).
const SNIPPET_CHARS: usize = 240;
/// Глубина графового обхода для граф-ранга (соседи открытого файла).
const GRAPH_HOPS: u32 = 2;
/// Во сколько раз пере-выбираем кандидатов из RRF до dedup overlap (чтобы добрать `limit` после схлопа).
const OVERFETCH: usize = 3;

/// Результат поиска по содержимому: чанк + его файл + слитый RRF-score.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub chunk_id: i64,
    pub path: String,
    pub title: Option<String>,
    pub heading_path: Option<String>,
    pub snippet: String,
    pub score: f32,
}

/// Префильтр по метаданным (применяется ДО KNN — AC-Б6-2). Все поля опциональны (AND по заданным).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SearchFilter {
    /// Ограничить папкой (префикс пути, рекурсивно): `path == folder || path LIKE folder||'/%'`.
    pub folder: Option<String>,
    /// Ограничить заметками с этим тегом.
    pub tag: Option<String>,
}

impl SearchFilter {
    fn is_empty(&self) -> bool {
        self.folder.is_none() && self.tag.is_none()
    }
}

/// Параметры гибридного поиска. `center` (открытый файл) включает граф-ранг 3-м источником RRF.
#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    pub limit: usize,
    pub filter: Option<SearchFilter>,
    pub center: Option<String>,
}

/// Ищет заметки по подстроке в пути, заголовке или имени тега. Спецсимволы LIKE экранируются.
pub async fn search_notes(reader: &ReadPool, query: String) -> DbResult<Vec<NoteRef>> {
    let q = query.trim().to_string();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    reader
        .query(move |c| {
            let like = format!(
                "%{}%",
                q.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            );
            let mut stmt = c.prepare(
                "SELECT DISTINCT f.path, f.title FROM files f \
                 LEFT JOIN file_tags ft ON ft.file_id = f.id \
                 LEFT JOIN tags t ON t.id = ft.tag_id \
                 WHERE f.is_deleted = 0 AND ( \
                   f.path LIKE ?1 ESCAPE '\\' \
                   OR f.title LIKE ?1 ESCAPE '\\' \
                   OR t.name LIKE ?1 ESCAPE '\\' \
                 ) ORDER BY f.path LIMIT 100",
            )?;
            let rows = stmt
                .query_map([like], |r| {
                    Ok(NoteRef {
                        path: r.get(0)?,
                        title: r.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
}

/// Reciprocal Rank Fusion: сливает несколько ранжированных списков id в один. Вклад элемента —
/// `Σ 1/(k + rank)` (rank 1-based) по спискам, где он встретился. Сорт по score↓, тай-брейк id↑
/// (детерминизм). Не зависит от «сырых» score выдач — они в разных шкалах (cos vs BM25).
pub fn rrf_fuse(lists: &[Vec<i64>], k: f32) -> Vec<(i64, f32)> {
    let mut scores: HashMap<i64, f32> = HashMap::new();
    for list in lists {
        for (rank, &id) in list.iter().enumerate() {
            *scores.entry(id).or_insert(0.0) += 1.0 / (k + rank as f32 + 1.0);
        }
    }
    let mut fused: Vec<(i64, f32)> = scores.into_iter().collect();
    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    fused
}

/// Строит безопасный FTS5-MATCH из пользовательского ввода: токены (по не-буквенно-цифровым
/// границам, юникод — кириллица сохраняется) в кавычках (фразы, нейтрализуют спецсинтаксис),
/// через `OR` (recall). `None`, если значимых токенов нет.
fn fts_query(raw: &str) -> Option<String> {
    let terms: Vec<String> = raw
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

/// Гибридный поиск по телу заметок (§6.2): вектор + FTS5/BM25 (+ граф соседей центра) → RRF →
/// dedup overlap → топ-`limit` с резолвом метаданных и сниппетом. Запрос эмбеддится ВНЕ лока пула.
pub async fn hybrid_search(
    reader: &ReadPool,
    vectors: Option<&VectorIndex>,
    embedder: Option<&dyn EmbeddingProvider>,
    query: String,
    opts: SearchOptions,
) -> DbResult<Vec<SearchHit>> {
    let q = query.trim();
    if q.is_empty() || opts.limit == 0 {
        return Ok(Vec::new());
    }
    let filter = opts.filter.filter(|f| !f.is_empty());

    // Префильтр по метаданным (AC-Б6-2): множество допустимых chunk_id вычисляется ДО KNN.
    let allowed: Option<HashSet<i64>> = match &filter {
        Some(f) => Some(allowed_chunk_ids(reader, f).await?),
        None => None,
    };
    if matches!(&allowed, Some(a) if a.is_empty()) {
        return Ok(Vec::new()); // фильтр не оставил кандидатов
    }

    // Выдача 1 — векторная (семантика). Префильтр — ВНУТРИ обхода HNSW (filtered_search).
    let mut vec_ranked: Vec<i64> = Vec::new();
    if let (Some(index), Some(embedder)) = (vectors, embedder) {
        let qvec = embedder
            .embed_query(q)
            .await
            .map_err(|e| DbError::External(e.to_string()))?;
        let hits = match &allowed {
            Some(a) => index.search_filtered(&qvec, CANDIDATES, |id| a.contains(&(id as i64))),
            None => index.search(&qvec, CANDIDATES),
        }
        .map_err(|e| DbError::External(e.to_string()))?;
        vec_ranked = hits.into_iter().map(|h| h.chunk_id as i64).collect();
    }

    // Выдача 2 — лексическая (FTS5/BM25) с тем же префильтром (через JOIN files).
    let fts_ranked = match fts_query(q) {
        Some(match_q) => fts_candidates(reader, match_q, filter.clone()).await?,
        None => Vec::new(),
    };

    // Выдача 3 — графовая: чанки соседей открытого файла, ТРЕТИЙ РАНГ RRF (не +0.2, REVIEW С-4).
    let graph_ranked = match &opts.center {
        Some(center) if !center.is_empty() => {
            graph_rank(reader, center.clone(), GRAPH_HOPS, allowed.clone()).await?
        }
        _ => Vec::new(),
    };

    // Слияние трёх рангов; пере-выбор кандидатов под dedup; резолв + схлоп соседних + усечение.
    let fused = rrf_fuse(&[vec_ranked, fts_ranked, graph_ranked], RRF_K);
    if fused.is_empty() {
        return Ok(Vec::new());
    }
    let candidates: Vec<(i64, f32)> = fused.into_iter().take(opts.limit * OVERFETCH).collect();
    resolve_and_dedup(reader, candidates, opts.limit).await
}

/// Множество `chunk_id`, проходящих метаданный префильтр (папка-префикс и/или тег). Для AC-Б6-2.
async fn allowed_chunk_ids(reader: &ReadPool, filter: &SearchFilter) -> DbResult<HashSet<i64>> {
    let folder = filter.folder.clone();
    let tag = filter.tag.clone();
    reader
        .query(move |c| {
            let mut sql = String::from(
                "SELECT ch.id FROM chunks ch JOIN files f ON f.id = ch.file_id WHERE f.is_deleted = 0",
            );
            let mut params: Vec<String> = Vec::new();
            if let Some(folder) = &folder {
                sql.push_str(" AND (f.path = ? OR f.path LIKE ? || '/%')");
                params.push(folder.clone());
                params.push(folder.clone());
            }
            if let Some(tag) = &tag {
                sql.push_str(
                    " AND EXISTS (SELECT 1 FROM file_tags ft JOIN tags t ON t.id = ft.tag_id \
                       WHERE ft.file_id = f.id AND t.name = ?)",
                );
                params.push(tag.clone());
            }
            let mut stmt = c.prepare(&sql)?;
            let ids = stmt
                .query_map(rusqlite::params_from_iter(params.iter()), |r| {
                    r.get::<_, i64>(0)
                })?
                .collect::<rusqlite::Result<HashSet<_>>>()?;
            Ok(ids)
        })
        .await
}

/// FTS5/BM25-кандидаты (`chunk_id` по возрастанию `rank`) с метаданным префильтром через JOIN.
async fn fts_candidates(
    reader: &ReadPool,
    match_q: String,
    filter: Option<SearchFilter>,
) -> DbResult<Vec<i64>> {
    reader
        .query(move |c| {
            let mut sql = String::from(
                "SELECT ch.id FROM fts_chunks \
                 JOIN chunks ch ON ch.id = fts_chunks.rowid \
                 JOIN files f ON f.id = ch.file_id \
                 WHERE fts_chunks MATCH ? AND f.is_deleted = 0",
            );
            let mut params: Vec<String> = vec![match_q];
            if let Some(filter) = &filter {
                if let Some(folder) = &filter.folder {
                    sql.push_str(" AND (f.path = ? OR f.path LIKE ? || '/%')");
                    params.push(folder.clone());
                    params.push(folder.clone());
                }
                if let Some(tag) = &filter.tag {
                    sql.push_str(
                        " AND EXISTS (SELECT 1 FROM file_tags ft JOIN tags t ON t.id = ft.tag_id \
                           WHERE ft.file_id = f.id AND t.name = ?)",
                    );
                    params.push(tag.clone());
                }
            }
            sql.push_str(&format!(" ORDER BY fts_chunks.rank LIMIT {CANDIDATES}"));
            let mut stmt = c.prepare(&sql)?;
            let ids = stmt
                .query_map(rusqlite::params_from_iter(params.iter()), |r| {
                    r.get::<_, i64>(0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(ids)
        })
        .await
}

/// Граф-ранг: чанки файлов-соседей `center` (BFS по `links` до `hops`), упорядоченные по (дистанция
/// хопа, `chunk_index`). Третий источник RRF — близость по графу ссылок (§6.2). Центр исключён.
async fn graph_rank(
    reader: &ReadPool,
    center: String,
    hops: u32,
    allowed: Option<HashSet<i64>>,
) -> DbResult<Vec<i64>> {
    reader
        .query(move |c| {
            let center_id: Option<i64> = c
                .query_row(
                    "SELECT id FROM files WHERE path = ?1 AND is_deleted = 0",
                    [&center],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(center_id) = center_id else {
                return Ok(Vec::new());
            };

            // BFS: соседние file_id в порядке расширения (ближе по хопам — раньше), центр исключаем.
            let mut seen: HashSet<i64> = HashSet::from([center_id]);
            let mut frontier = vec![center_id];
            let mut ordered_files: Vec<i64> = Vec::new();
            for _ in 0..hops {
                if frontier.is_empty() {
                    break;
                }
                let ph = vec!["?"; frontier.len()].join(",");
                let sql = format!(
                    "SELECT source_id, target_id FROM links \
                     WHERE target_id IS NOT NULL AND (source_id IN ({ph}) OR target_id IN ({ph}))"
                );
                let mut stmt = c.prepare(&sql)?;
                let params: Vec<&dyn rusqlite::ToSql> = frontier
                    .iter()
                    .chain(frontier.iter())
                    .map(|x| x as &dyn rusqlite::ToSql)
                    .collect();
                let pairs = stmt
                    .query_map(params.as_slice(), |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let mut next = Vec::new();
                for (s, t) in pairs {
                    for n in [s, t] {
                        if seen.insert(n) {
                            next.push(n);
                            ordered_files.push(n);
                        }
                    }
                }
                frontier = next;
            }
            if ordered_files.is_empty() {
                return Ok(Vec::new());
            }

            let file_rank: HashMap<i64, usize> = ordered_files
                .iter()
                .enumerate()
                .map(|(i, &f)| (f, i))
                .collect();
            let ph = vec!["?"; ordered_files.len()].join(",");
            let sql =
                format!("SELECT id, file_id, chunk_index FROM chunks WHERE file_id IN ({ph})");
            let mut stmt = c.prepare(&sql)?;
            let params: Vec<&dyn rusqlite::ToSql> = ordered_files
                .iter()
                .map(|x| x as &dyn rusqlite::ToSql)
                .collect();
            let mut rows: Vec<(i64, i64, i64)> = stmt
                .query_map(params.as_slice(), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if let Some(a) = &allowed {
                rows.retain(|(id, _, _)| a.contains(id));
            }
            rows.sort_by_key(|(_, fid, cidx)| {
                (file_rank.get(fid).copied().unwrap_or(usize::MAX), *cidx)
            });
            Ok(rows.into_iter().map(|(id, _, _)| id).collect())
        })
        .await
}

/// Строка резолва (внутренняя): несёт `file_id`/`chunk_index` для dedup overlap.
struct RawHit {
    chunk_id: i64,
    file_id: i64,
    chunk_index: i64,
    path: String,
    title: Option<String>,
    heading_path: Option<String>,
    content: String,
}

/// Резолвит метаданные кандидатов (в порядке RRF), схлопывает перекрывающиеся соседние чанки одного
/// файла (|Δchunk_index| ≤ 1 — overlap чанкера) и обрезает до `limit`. Порядок RRF сохраняется.
async fn resolve_and_dedup(
    reader: &ReadPool,
    candidates: Vec<(i64, f32)>,
    limit: usize,
) -> DbResult<Vec<SearchHit>> {
    let score_of: HashMap<i64, f32> = candidates.iter().copied().collect();
    let order: Vec<i64> = candidates.iter().map(|(id, _)| *id).collect();
    let ids = order.clone();

    let rows = reader
        .query(move |c| {
            let ph = vec!["?"; ids.len()].join(",");
            let sql = format!(
                "SELECT ch.id, ch.file_id, ch.chunk_index, f.path, f.title, ch.heading_path, ch.content \
                 FROM chunks ch JOIN files f ON f.id = ch.file_id \
                 WHERE f.is_deleted = 0 AND ch.id IN ({ph})"
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(ids.iter()), |r| {
                    Ok(RawHit {
                        chunk_id: r.get(0)?,
                        file_id: r.get(1)?,
                        chunk_index: r.get(2)?,
                        path: r.get(3)?,
                        title: r.get(4)?,
                        heading_path: r.get(5)?,
                        content: r.get(6)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await?;

    let mut by_id: HashMap<i64, RawHit> = rows.into_iter().map(|h| (h.chunk_id, h)).collect();
    let mut kept_idx_by_file: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut out: Vec<SearchHit> = Vec::with_capacity(limit);
    for id in order {
        let Some(h) = by_id.remove(&id) else { continue };
        let overlaps = kept_idx_by_file
            .get(&h.file_id)
            .is_some_and(|idxs| idxs.iter().any(|&ci| (ci - h.chunk_index).abs() <= 1));
        if overlaps {
            continue; // соседний чанк того же файла уже взят — это overlap чанкера
        }
        kept_idx_by_file
            .entry(h.file_id)
            .or_default()
            .push(h.chunk_index);
        out.push(SearchHit {
            score: score_of.get(&h.chunk_id).copied().unwrap_or(0.0),
            chunk_id: h.chunk_id,
            path: h.path,
            title: h.title,
            heading_path: h.heading_path,
            snippet: snippet_of(&h.content),
        });
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

/// Полное содержимое чанков по id (для сборки RAG-контекста чата). Ключ — `chunk_id`, значение —
/// `(метка-источник = путь [> heading], содержимое)`. Отсутствующие/удалённые id просто опускаются.
pub async fn fetch_chunk_contexts(
    reader: &ReadPool,
    ids: &[i64],
) -> DbResult<HashMap<i64, (String, String)>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids = ids.to_vec();
    reader
        .query(move |c| {
            let placeholders = vec!["?"; ids.len()].join(",");
            let sql = format!(
                "SELECT ch.id, f.path, ch.heading_path, ch.content \
                 FROM chunks ch JOIN files f ON f.id = ch.file_id \
                 WHERE f.is_deleted = 0 AND ch.id IN ({placeholders})"
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
                let id: i64 = r.get(0)?;
                let path: String = r.get(1)?;
                let heading: Option<String> = r.get(2)?;
                let content: String = r.get(3)?;
                let label = match heading {
                    Some(h) => format!("{path} > {h}"),
                    None => path,
                };
                Ok((id, (label, content)))
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

/// Сниппет из содержимого чанка: схлопывает пробелы, режет по границе символа до `SNIPPET_CHARS`.
fn snippet_of(content: &str) -> String {
    let collapsed = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= SNIPPET_CHARS {
        return collapsed;
    }
    let cut: String = collapsed.chars().take(SNIPPET_CHARS).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::indexer::Indexer;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn searches_by_path_title_and_tag() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("Roadmap.md"), "# Project Roadmap\n\n#planning\n").unwrap();
        fs::write(root.join("Idea.md"), "# Bright Idea\n\n#planning\n").unwrap();
        fs::write(root.join("Other.md"), "# Other\n").unwrap();

        let db = Database::open(root.join(".nexus/nexus.db")).await.unwrap();
        let idx = Indexer::new(&db, root.clone());
        for f in ["Roadmap.md", "Idea.md", "Other.md"] {
            idx.index_file(f).await.unwrap();
        }

        let by_path = search_notes(db.reader(), "Roadmap".into()).await.unwrap();
        assert!(by_path.iter().any(|n| n.path == "Roadmap.md"));

        let by_title = search_notes(db.reader(), "Bright".into()).await.unwrap();
        assert!(by_title.iter().any(|n| n.path == "Idea.md"));

        let mut by_tag = search_notes(db.reader(), "planning".into()).await.unwrap();
        by_tag.sort_by(|a, b| a.path.cmp(&b.path));
        let paths: Vec<_> = by_tag.iter().map(|n| n.path.as_str()).collect();
        assert_eq!(paths, vec!["Idea.md", "Roadmap.md"]);

        assert!(search_notes(db.reader(), "zzz-nope".into())
            .await
            .unwrap()
            .is_empty());
        assert!(search_notes(db.reader(), "  ".into())
            .await
            .unwrap()
            .is_empty());
    }

    // ── Гибридный поиск + RRF (Ф1-6) ──────────────────────────────────────────────────────────

    use crate::ai::MockEmbedder;
    use std::sync::Arc;

    async fn open_db(root: &std::path::Path) -> Database {
        Database::open(root.join(".nexus/nexus.db")).await.unwrap()
    }

    /// Опции поиска только с лимитом (без фильтра/центра).
    fn opts(limit: usize) -> SearchOptions {
        SearchOptions {
            limit,
            ..Default::default()
        }
    }

    /// Индексирует файлы с RAG (mock-эмбеддер) и возвращает эмбеддер + векторный индекс.
    async fn index_rag(
        db: &Database,
        root: &std::path::Path,
        files: &[(&str, &str)],
        dim: usize,
    ) -> (Arc<dyn EmbeddingProvider>, Arc<VectorIndex>) {
        for (name, body) in files {
            fs::write(root.join(name), body).unwrap();
        }
        let vectors =
            Arc::new(VectorIndex::open(root.join(".nexus").join("vectors.usearch"), dim).unwrap());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder { dim });
        let idx = Indexer::with_rag(
            db,
            root.to_path_buf(),
            embedder.clone(),
            vectors.clone(),
            true,
        );
        for (name, _) in files {
            idx.index_file(name).await.unwrap();
        }
        (embedder, vectors)
    }

    #[test]
    fn rrf_ranks_items_present_in_both_lists_higher() {
        // id 1: ранги 1 и 3; id 3: ранги 3 и 1 → равный score, тай-брейк по id↑ (1 раньше 3).
        let fused = rrf_fuse(&[vec![1, 2, 3], vec![3, 4, 1]], 60.0);
        let order: Vec<i64> = fused.iter().map(|(id, _)| *id).collect();
        assert_eq!(order[0], 1, "встретился в обоих списках высоко");
        assert_eq!(order[1], 3, "тоже в обоих, тай-брейк по id");
        let pos = |x: i64| order.iter().position(|&y| y == x).unwrap();
        assert!(pos(1) < pos(2), "присутствующий в обоих > одиночного");
        assert!(pos(3) < pos(4));
    }

    #[test]
    fn fts_query_sanitizes_tokenizes_and_ors() {
        assert_eq!(
            fts_query("привет мир").as_deref(),
            Some("\"привет\" OR \"мир\"")
        );
        assert_eq!(fts_query("a-b").as_deref(), Some("\"a\" OR \"b\""));
        assert_eq!(fts_query("   "), None);
        // спецсимволы FTS не утекают в синтаксис (токены в кавычках)
        assert_eq!(
            fts_query("foo OR bar").as_deref(),
            Some("\"foo\" OR \"OR\" OR \"bar\"")
        );
    }

    /// FTS-ветвь (без эмбеддера) находит документ по слову из тела + резолвит путь/сниппет.
    #[tokio::test]
    async fn fts_only_finds_doc_by_body_term() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let db = open_db(&root).await;
        index_rag(
            &db,
            &root,
            &[
                (
                    "Cooking.md",
                    "# Cooking\n\nHow to bake sourdough bread at home.\n",
                ),
                (
                    "Space.md",
                    "# Space\n\nThe rocket reached orbit successfully.\n",
                ),
            ],
            16,
        )
        .await;

        let hits = hybrid_search(db.reader(), None, None, "sourdough".into(), opts(10))
            .await
            .unwrap();
        assert!(!hits.is_empty(), "FTS-ветвь работает без эмбеддера");
        assert_eq!(hits[0].path, "Cooking.md");
        assert!(hits[0].snippet.contains("sourdough"));
    }

    /// Гибрид (вектор+FTS): результаты отсортированы по score↓, метаданные/сниппет заполнены.
    #[tokio::test]
    async fn hybrid_sorts_by_score_and_resolves_metadata() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let db = open_db(&root).await;
        let (emb, vectors) = index_rag(
            &db,
            &root,
            &[
                (
                    "Cooking.md",
                    "# Cooking\n\nHow to bake sourdough bread at home.\n",
                ),
                (
                    "Space.md",
                    "# Space\n\nThe rocket reached orbit successfully today.\n",
                ),
            ],
            16,
        )
        .await;

        let hits = hybrid_search(
            db.reader(),
            Some(vectors.as_ref()),
            Some(emb.as_ref()),
            "rocket orbit".into(),
            opts(10),
        )
        .await
        .unwrap();
        assert!(!hits.is_empty());
        assert!(hits.iter().any(|h| h.path == "Space.md"));
        assert!(hits.iter().all(|h| !h.snippet.is_empty()));
        for w in hits.windows(2) {
            assert!(w[0].score >= w[1].score, "RRF-score по убыванию");
        }
    }

    /// Пустой запрос и запрос без совпадений → пустая выдача.
    #[tokio::test]
    async fn empty_or_unmatched_query_returns_empty() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let db = open_db(&root).await;
        index_rag(&db, &root, &[("A.md", "# A\n\nalpha beta gamma\n")], 16).await;

        assert!(
            hybrid_search(db.reader(), None, None, "   ".into(), opts(10))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            hybrid_search(db.reader(), None, None, "zzzneverappears".into(), opts(10))
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// Живой гибрид на nomic :8081: запрос БЕЗ лексических пересечений → решает вектор (семантика).
    #[tokio::test]
    #[ignore = "нужен embedding-сервер (NEXUS_EMBED_URL, default 192.168.0.31:8083)"]
    async fn live_hybrid_ranks_semantically() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(
            root.join("cat.md"),
            "# Кошка\n\nКошка спит на коврике у батареи.\n",
        )
        .unwrap();
        fs::write(
            root.join("car.md"),
            "# Авто\n\nДвигатель внутреннего сгорания и коробка передач.\n",
        )
        .unwrap();
        let db = open_db(&root).await;
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::ai::live_test_embedder());
        let vectors = Arc::new(
            VectorIndex::open(
                root.join(".nexus").join("vectors.usearch"),
                crate::ai::LIVE_EMBED_DIM,
            )
            .unwrap(),
        );
        let idx = Indexer::with_rag(&db, root.clone(), embedder.clone(), vectors.clone(), true);
        idx.index_file("cat.md").await.unwrap();
        idx.index_file("car.md").await.unwrap();

        // Ни одного общего слова с cat.md → FTS пуст, решает семантика вектора.
        let hits = hybrid_search(
            db.reader(),
            Some(vectors.as_ref()),
            Some(embedder.as_ref()),
            "пушистый питомец мурлычет".into(),
            opts(5),
        )
        .await
        .unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].path, "cat.md", "семантический топ — про кошку");
    }

    /// AC-Б6-2: префильтр по папке/тегу применяется ДО KNN — выдача ограничена подпапкой.
    #[tokio::test]
    async fn prefilter_by_folder_restricts_results() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join("Work")).unwrap();
        fs::create_dir_all(root.join("Personal")).unwrap();
        let db = open_db(&root).await;
        let (emb, vectors) = index_rag(
            &db,
            &root,
            &[
                ("Work/Plan.md", "# Plan\n\nDeploy the rocket to orbit.\n"),
                (
                    "Personal/Diary.md",
                    "# Diary\n\nThe rocket launch was loud.\n",
                ),
            ],
            16,
        )
        .await;

        let folder_opts = SearchOptions {
            limit: 10,
            filter: Some(SearchFilter {
                folder: Some("Work".into()),
                tag: None,
            }),
            center: None,
        };
        let hits = hybrid_search(
            db.reader(),
            Some(vectors.as_ref()),
            Some(emb.as_ref()),
            "rocket".into(),
            folder_opts,
        )
        .await
        .unwrap();
        assert!(!hits.is_empty());
        assert!(
            hits.iter().all(|h| h.path.starts_with("Work/")),
            "префильтр по папке исключил Personal/* (AC-Б6-2)"
        );
    }

    /// Граф-ранг (изолированно): без эмбеддера и без лексических совпадений выдачу формирует ТОЛЬКО
    /// граф — сосед центра попадает, несвязанный файл — нет. Подтверждает 3-й источник RRF (§6.2).
    #[tokio::test]
    async fn graph_rank_surfaces_neighbor_of_center() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let db = open_db(&root).await;
        index_rag(
            &db,
            &root,
            &[
                (
                    "Hub.md",
                    "# Hub\n\nГлавная заметка ссылается на [[Neighbor]].\n",
                ),
                ("Neighbor.md", "# Neighbor\n\nкварки глюоны адроны бозоны\n"),
                ("Far.md", "# Far\n\nрецепт борща со сметаной\n"),
            ],
            16,
        )
        .await;

        // Вектор off (None), запрос без лексических совпадений → FTS пуст. Центр = Hub → выдачу
        // даёт ТОЛЬКО граф-ранг: сосед Neighbor есть, несвязанный Far отсутствует.
        let with_center = SearchOptions {
            limit: 10,
            filter: None,
            center: Some("Hub.md".into()),
        };
        let hits = hybrid_search(db.reader(), None, None, "zzqphysics".into(), with_center)
            .await
            .unwrap();
        assert!(
            hits.iter().any(|h| h.path == "Neighbor.md"),
            "сосед центра попал в выдачу через граф-ранг"
        );
        assert!(
            hits.iter().all(|h| h.path != "Far.md"),
            "несвязанный файл не подтянут графом"
        );

        // Без центра (и без вектора/FTS-совпадений) — выдача пуста.
        let hits_no_center = hybrid_search(db.reader(), None, None, "zzqphysics".into(), opts(10))
            .await
            .unwrap();
        assert!(hits_no_center.is_empty(), "без центра граф-ранга нет");
    }

    /// Dedup overlap: соседние перекрывающиеся чанки одного файла схлопываются (≤1 на регион).
    #[tokio::test]
    async fn dedup_collapses_adjacent_chunks() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let db = open_db(&root).await;
        // Длинный текст с повторяющимся термином → чанкер нарежет несколько СОСЕДНИХ чанков.
        let body = format!("# Doc\n\n{}", "vector ".repeat(2000));
        let (emb, vectors) = index_rag(&db, &root, &[("Big.md", &body)], 16).await;

        // Чанков должно быть >1 (иначе тест бессмыслен).
        let n: i64 = db
            .reader()
            .query(|c| c.query_row("SELECT count(*) FROM chunks", [], |r| r.get(0)))
            .await
            .unwrap();
        assert!(n > 1, "ожидаем несколько чанков (их {n})");

        let hits = hybrid_search(
            db.reader(),
            Some(vectors.as_ref()),
            Some(emb.as_ref()),
            "vector".into(),
            opts(10),
        )
        .await
        .unwrap();
        // Все из одного файла Big.md, но соседние (Δindex≤1) схлопнуты → не подряд идущие индексы.
        assert!(!hits.is_empty());
        assert!(
            hits.iter().all(|h| h.path == "Big.md"),
            "все попадания из одного файла"
        );
        // Дедуп должен оставить меньше, чем всего чанков (схлопнул хотя бы пару соседних).
        assert!(
            (hits.len() as i64) < n,
            "overlap соседних чанков схлопнут ({} из {n})",
            hits.len()
        );
    }
}
