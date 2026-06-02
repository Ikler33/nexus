//! Поиск: по метаданным (Ф0: title/path/tags) и гибридный по ТЕЛУ (Ф1-6).
//!
//! **Гибрид (§6.2):** две независимые выдачи кандидатов — вектор (usearch, семантика) и FTS5/BM25
//! (`fts_chunks`, лексика) — сливаются через **Reciprocal Rank Fusion** (RRF), не по «сырым» score
//! (они в разных шкалах). Деградация изящная: нет эмбеддера → только FTS; обе пусты → пусто.

use std::collections::HashMap;

use serde::Serialize;

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

/// Гибридный поиск по телу заметок: вектор (если есть эмбеддер+индекс) + FTS5/BM25 → RRF → топ-`limit`
/// с резолвом метаданных файла и сниппетом. Запрос эмбеддится ВНЕ блокировки read-пула.
pub async fn hybrid_search(
    reader: &ReadPool,
    vectors: Option<&VectorIndex>,
    embedder: Option<&dyn EmbeddingProvider>,
    query: String,
    limit: usize,
) -> DbResult<Vec<SearchHit>> {
    let q = query.trim();
    if q.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    // Выдача 1 — векторная (семантика). Запрос эмбеддится → KNN в usearch.
    let mut vec_ranked: Vec<i64> = Vec::new();
    if let (Some(index), Some(embedder)) = (vectors, embedder) {
        let qvec = embedder
            .embed_query(q)
            .await
            .map_err(|e| DbError::External(e.to_string()))?;
        let hits = index
            .search(&qvec, CANDIDATES)
            .map_err(|e| DbError::External(e.to_string()))?;
        vec_ranked = hits.into_iter().map(|h| h.chunk_id as i64).collect();
    }

    // Выдача 2 — лексическая (FTS5/BM25). `rank` возрастает = релевантнее.
    let mut fts_ranked: Vec<i64> = Vec::new();
    if let Some(match_q) = fts_query(q) {
        fts_ranked = reader
            .query(move |c| {
                let mut stmt = c.prepare(
                    "SELECT rowid FROM fts_chunks WHERE fts_chunks MATCH ?1 \
                     ORDER BY rank LIMIT ?2",
                )?;
                let ids = stmt
                    .query_map(rusqlite::params![match_q, CANDIDATES as i64], |r| {
                        r.get::<_, i64>(0)
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(ids)
            })
            .await?;
    }

    // Слияние и отбор топ-`limit`.
    let fused = rrf_fuse(&[vec_ranked, fts_ranked], RRF_K);
    let top: Vec<(i64, f32)> = fused.into_iter().take(limit).collect();
    if top.is_empty() {
        return Ok(Vec::new());
    }
    let score_of: HashMap<i64, f32> = top.iter().copied().collect();
    let ids: Vec<i64> = top.iter().map(|(id, _)| *id).collect();

    // Резолв метаданных + содержимого чанков одним запросом (IN-список).
    let mut hits = reader
        .query(move |c| {
            let placeholders = vec!["?"; ids.len()].join(",");
            let sql = format!(
                "SELECT ch.id, f.path, f.title, ch.heading_path, ch.content \
                 FROM chunks ch JOIN files f ON f.id = ch.file_id \
                 WHERE f.is_deleted = 0 AND ch.id IN ({placeholders})"
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(ids.iter()), |r| {
                    let chunk_id: i64 = r.get(0)?;
                    let content: String = r.get(4)?;
                    Ok(SearchHit {
                        chunk_id,
                        path: r.get(1)?,
                        title: r.get(2)?,
                        heading_path: r.get(3)?,
                        snippet: snippet_of(&content),
                        score: 0.0, // проставим из RRF ниже
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await?;

    // Проставляем RRF-score и сортируем по нему (IN не сохраняет порядок).
    for h in &mut hits {
        h.score = score_of.get(&h.chunk_id).copied().unwrap_or(0.0);
    }
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.chunk_id.cmp(&b.chunk_id))
    });
    Ok(hits)
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

        let hits = hybrid_search(db.reader(), None, None, "sourdough".into(), 10)
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
            10,
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

        assert!(hybrid_search(db.reader(), None, None, "   ".into(), 10)
            .await
            .unwrap()
            .is_empty());
        assert!(
            hybrid_search(db.reader(), None, None, "zzzneverappears".into(), 10)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// Живой гибрид на nomic :8081: запрос БЕЗ лексических пересечений → решает вектор (семантика).
    #[tokio::test]
    #[ignore = "нужен embedding-сервер на 127.0.0.1:8081"]
    async fn live_hybrid_ranks_semantically() {
        use crate::ai::{default_prefixes, OpenAiEmbedder};
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
        idx.index_file("car.md").await.unwrap();

        // Ни одного общего слова с cat.md → FTS пуст, решает семантика вектора.
        let hits = hybrid_search(
            db.reader(),
            Some(vectors.as_ref()),
            Some(embedder.as_ref()),
            "пушистый питомец мурлычет".into(),
            5,
        )
        .await
        .unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].path, "cat.md", "семантический топ — про кошку");
    }
}
