//! Граф ссылок — **ADR-004**: источник истины = SQLite. Беклинки и обходы — запросами
//! по индексу `idx_links_target`; petgraph в памяти НЕ держим (нет дублирования/рассинхрона).

use std::collections::{BTreeSet, HashSet};

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::db::{DbResult, ReadPool};

/// Безопасный батч bind-параметров на один SQLite-запрос. Современный bundled-SQLite держит 32766,
/// но чанкуем по 900 — переносимо (старые сборки = 999) и с двойным запасом (часть запросов повторяет
/// набор в `IN` дважды). Граф на супер-хабе (узел с десятками тысяч связей) иначе ловил
/// `too many SQL variables` и валил команду (ревью A9). Чанкинг сохраняет ПОЛНЫЙ результат (без обрезки).
const SQL_VAR_CHUNK: usize = 900;

/// Гоняет запрос с одним `IN ({ph})` по `ids` чанками ≤ [`SQL_VAR_CHUNK`] и собирает строки.
/// `make_sql(ph)` строит SQL по строке плейсхолдеров; `map_row` маппит строку результата.
/// `pub(crate)` — общий util для всех IN-по-id-набору запросов (граф + `search::graph_rank`,
/// находка аудита: graph_rank бил двойным неограниченным `IN` → краш на супер-хабе).
pub(crate) fn collect_in_chunks<T>(
    c: &rusqlite::Connection,
    ids: &[i64],
    make_sql: impl Fn(&str) -> String,
    map_row: impl Fn(&rusqlite::Row) -> rusqlite::Result<T>,
) -> rusqlite::Result<Vec<T>> {
    let mut out = Vec::new();
    for chunk in ids.chunks(SQL_VAR_CHUNK) {
        let ph = vec!["?"; chunk.len()].join(",");
        let mut stmt = c.prepare(&make_sql(&ph))?;
        let params: Vec<&dyn rusqlite::ToSql> =
            chunk.iter().map(|x| x as &dyn rusqlite::ToSql).collect();
        let rows = stmt
            .query_map(params.as_slice(), |r| map_row(r))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        out.extend(rows);
    }
    Ok(out)
}

/// Обратная ссылка: кто и в каком контексте ссылается на файл.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BacklinkEntry {
    pub source_path: String,
    pub source_title: Option<String>,
    pub context: Option<String>,
    pub line_number: Option<i64>,
}

/// Беклинки файла `path` — запрос по `idx_links_target` (доли мс из page-cache, ADR-004).
pub async fn get_backlinks(reader: &ReadPool, path: String) -> DbResult<Vec<BacklinkEntry>> {
    reader
        .query(move |c| {
            let mut stmt = c.prepare(
                "SELECT f.path, f.title, l.context, l.line_number \
                 FROM links l JOIN files f ON f.id = l.source_id \
                 WHERE l.target_id = (SELECT id FROM files WHERE path = ?1 AND is_deleted = 0) \
                   AND f.is_deleted = 0 \
                   AND l.source_id != l.target_id \
                 ORDER BY f.path, l.line_number",
            )?;
            let rows = stmt
                .query_map([path], |r| {
                    Ok(BacklinkEntry {
                        source_path: r.get(0)?,
                        source_title: r.get(1)?,
                        context: r.get(2)?,
                        line_number: r.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
}

/// Незалинкованное упоминание (UNLINK-1): заметка, чей ТЕКСТ содержит заголовок открытой, но без
/// явной `[[ссылки]]` — кандидат «связать», всплывание забытой связи (как «Unlinked mentions» Obsidian).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MentionEntry {
    pub source_path: String,
    pub source_title: Option<String>,
    pub snippet: String,
}

/// Минимум значимых букв/цифр в ключе упоминания — короткие/общие имена («ИИ», «Go») дают шум.
const MIN_KEY_CHARS: usize = 3;
/// Сколько упоминаний (файлов) максимум вернуть.
const MENTIONS_LIMIT: usize = 30;

/// Имя файла без папок и `.md` — основной ключ упоминания (как резолвятся `[[ссылки]]`/Obsidian).
fn basename_stem(path: &str) -> &str {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".md")
}

/// Строит ФРАЗОВЫЙ FTS5-MATCH из ключа: токены (по не-буквенно-цифровым границам, юникод сохраняется)
/// подряд в ОДНИХ кавычках → точное совпадение фразы (а не OR-токены, как `fts_query` поиска — иначе
/// «RAG» ИЛИ «Pipeline» ловило бы пол-vault). `None` — нет токенов / слишком короткий ключ (шум).
fn key_phrase(key: &str) -> Option<String> {
    let toks: Vec<&str> = key
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    if toks.is_empty() {
        return None;
    }
    let total: usize = toks.iter().map(|t| t.chars().count()).sum();
    if total < MIN_KEY_CHARS {
        return None;
    }
    let inner = toks
        .iter()
        .map(|t| t.replace('"', "\"\""))
        .collect::<Vec<_>>()
        .join(" ");
    Some(format!("\"{inner}\"")) // фраза: токены подряд
}

/// Короткий сниппет из тела чанка для показа в списке упоминаний: схлопывает пробелы, режет до 140
/// символов (по `chars`, UTF-8-safe), добавляет «…» при усечении.
fn make_snippet(content: &str) -> String {
    let flat = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = flat.chars().take(140).collect();
    if flat.chars().count() > 140 {
        format!("{cut}…")
    } else {
        cut
    }
}

/// Незалинкованные упоминания файла `path` по телу других заметок: текст содержит ИМЯ заметки
/// (basename, как резолвятся `[[ссылки]]`) ИЛИ её заголовок (H1/frontmatter `files.title`, как в прозе)
/// как ФРАЗУ, но без явной ссылки. Исключает сам файл, удалённые и уже-линкующих (`links`). Дедуп по
/// файлу — В SQL (оконная функция), чтобы LIMIT считал ФАЙЛЫ, а не чанки (иначе одна заметка, повторяющая
/// фразу в N чанках, вытеснила бы остальных). Сниппет — из наиболее релевантного чанка файла. Короткий
/// ключ / нет чанков → пусто.
pub async fn unlinked_mentions(reader: &ReadPool, path: String) -> DbResult<Vec<MentionEntry>> {
    reader
        .query(move |c| {
            // id + заголовок (может отсутствовать — тогда ключ только из имени файла).
            let target: Option<(i64, Option<String>)> = c
                .query_row(
                    "SELECT id, title FROM files WHERE path = ?1 AND is_deleted = 0",
                    [&path],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((id, title)) = target else {
                return Ok(Vec::new());
            };
            // Ключи: имя файла (всегда) + заголовок (если есть и отличается). Дедуп фраз.
            let mut phrases: Vec<String> = Vec::new();
            for key in [Some(basename_stem(&path)), title.as_deref()]
                .into_iter()
                .flatten()
            {
                if let Some(p) = key_phrase(key) {
                    if !phrases.contains(&p) {
                        phrases.push(p);
                    }
                }
            }
            if phrases.is_empty() {
                return Ok(Vec::new()); // имя и заголовок слишком короткие → шум, не ищем
            }
            let match_q = phrases.join(" OR ");

            // Дедуп по файлу В SQL: ROW_NUMBER по f.id (лучший по rank чанк = rn 1), затем LIMIT по
            // ФАЙЛАМ (иначе одна заметка, повторяющая фразу в N чанках, вытеснила бы остальных).
            // ORDER BY rank внешний — самые релевантные файлы первыми. Сниппет режем из тела чанка в
            // Rust: FTS5 `snippet()` внутри подзапроса с оконной функцией теряет контекст и пуст.
            let mut stmt = c.prepare(
                "SELECT path, title, content FROM ( \
                   SELECT f.path AS path, f.title AS title, ch.content AS content, \
                          fts_chunks.rank AS rk, \
                          ROW_NUMBER() OVER (PARTITION BY f.id ORDER BY fts_chunks.rank) AS rn \
                   FROM fts_chunks \
                   JOIN chunks ch ON ch.id = fts_chunks.rowid \
                   JOIN files f ON f.id = ch.file_id \
                   WHERE fts_chunks MATCH ?1 \
                     AND f.is_deleted = 0 \
                     AND f.id != ?2 \
                     AND f.id NOT IN (SELECT source_id FROM links WHERE target_id = ?2) \
                 ) \
                 WHERE rn = 1 \
                 ORDER BY rk \
                 LIMIT ?3",
            )?;
            let rows = stmt
                .query_map(params![match_q, id, MENTIONS_LIMIT as i64], |r| {
                    Ok(MentionEntry {
                        source_path: r.get(0)?,
                        source_title: r.get(1)?,
                        snippet: make_snippet(&r.get::<_, String>(2)?),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
}

/// Узел локального графа.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    pub id: i64,
    pub path: String,
    pub title: Option<String>,
    /// Теги заметки (file_tags) — цвет узла и фильтр-чипы (BACKLOG «Граф: теги», макет graph.jsx).
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Дочитывает теги для набора узлов одним JOIN'ом (IN-чанки ≤ лимита переменных, ревью A9).
fn attach_tags(c: &rusqlite::Connection, nodes: &mut [GraphNode]) -> rusqlite::Result<()> {
    let ids: Vec<i64> = nodes.iter().map(|n| n.id).collect();
    let pairs = collect_in_chunks(
        c,
        &ids,
        |ph| {
            format!(
                "SELECT ft.file_id, t.name FROM file_tags ft \
                 JOIN tags t ON t.id = ft.tag_id WHERE ft.file_id IN ({ph}) ORDER BY t.name"
            )
        },
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
    )?;
    let mut by_id: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
    for (id, tag) in pairs {
        by_id.entry(id).or_default().push(tag);
    }
    for n in nodes.iter_mut() {
        if let Some(tags) = by_id.remove(&n.id) {
            n.tags = tags;
        }
    }
    Ok(())
}

/// Ребро (по идентификаторам файлов).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    pub source: i64,
    pub target: i64,
}

/// Сводит сырые НАПРАВЛЕННЫЕ рёбра `links` (A→B и B→A — две строки) в КАНОНИЧЕСКИЕ
/// НЕНАПРАВЛЕННЫЕ пары: ровно одно `GraphEdge` на неупорядоченную пару `{s,t}` (P1-19).
/// Граф визуально ненаправленный (рендер `<line>` без стрелок/маркеров), поэтому реципрокная
/// ссылка не должна давать вдвое завышенную степень/размер узла, двойную линию и удвоенный счётчик
/// рёбер. Фильтрует self-loop (`s==t`, бессмысленно в knowledge-graph) и рёбра в узлы вне `ids`.
/// Канонизация `(min,max)` детерминирует ориентацию (для ненаправленного графа порядок не важен).
fn dedup_undirected_edges(raw: Vec<(i64, i64)>, ids: &BTreeSet<i64>) -> Vec<GraphEdge> {
    let mut seen: HashSet<(i64, i64)> = HashSet::new();
    let mut edges: Vec<GraphEdge> = Vec::new();
    for (s, t) in raw {
        if s == t || !ids.contains(&t) {
            continue; // self-loop или цель вне набора узлов
        }
        let key = (s.min(t), s.max(t));
        if seen.insert(key) {
            edges.push(GraphEdge {
                source: key.0,
                target: key.1,
            });
        }
    }
    edges
}

/// Локальный подграф вокруг файла.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GraphData {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// Локальный N-hop граф вокруг `center` (ADR-004: обход по SQLite, без petgraph).
/// BFS по неориентированным связям до глубины `hops`; рёбра — внутри полученного множества.
pub async fn get_local_graph(reader: &ReadPool, center: String, hops: u32) -> DbResult<GraphData> {
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
                return Ok(GraphData::default());
            };

            let mut ids: BTreeSet<i64> = BTreeSet::new();
            ids.insert(center_id);
            let mut frontier = vec![center_id];

            for _ in 0..hops {
                if frontier.is_empty() {
                    break;
                }
                // Чанкуем frontier по SQL_VAR_CHUNK/2 (набор повторяется в `source_id OR target_id`),
                // иначе супер-хаб даёт тысячи bind-переменных → `too many SQL variables` (ревью A9).
                let mut next = Vec::new();
                for batch in frontier.chunks(SQL_VAR_CHUNK / 2) {
                    let ph = vec!["?"; batch.len()].join(",");
                    let sql = format!(
                        "SELECT source_id, target_id FROM links \
                         WHERE target_id IS NOT NULL AND (source_id IN ({ph}) OR target_id IN ({ph}))"
                    );
                    let mut stmt = c.prepare(&sql)?;
                    let params: Vec<&dyn rusqlite::ToSql> = batch
                        .iter()
                        .chain(batch.iter())
                        .map(|x| x as &dyn rusqlite::ToSql)
                        .collect();
                    let neighbors = stmt
                        .query_map(params.as_slice(), |r| {
                            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    for (s, t) in neighbors {
                        for n in [s, t] {
                            if ids.insert(n) {
                                next.push(n);
                            }
                        }
                    }
                }
                frontier = next;
            }

            let id_vec: Vec<i64> = ids.iter().copied().collect();
            let mut nodes = collect_in_chunks(
                c,
                &id_vec,
                |ph| format!("SELECT id, path, title FROM files WHERE id IN ({ph})"),
                |r| {
                    Ok(GraphNode {
                        id: r.get(0)?,
                        path: r.get(1)?,
                        title: r.get(2)?,
                        tags: Vec::new(),
                    })
                },
            )?;
            attach_tags(c, &mut nodes)?;

            // Рёбра: одиночный `source_id IN (chunk)` + фильтр `target ∈ ids` в Rust — избегаем
            // двойного IN (source AND target), вдвое сокращая bind-переменные на запрос. Результат
            // тот же (рёбра внутри набора узлов); source_id разбивает чанки → дублей (s,t) нет.
            let raw_edges = collect_in_chunks(
                c,
                &id_vec,
                |ph| {
                    format!(
                        "SELECT DISTINCT source_id, target_id FROM links \
                         WHERE target_id IS NOT NULL AND source_id IN ({ph})"
                    )
                },
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )?;
            // P1-19: дедуп в канонические ненаправленные пары — реципрокная (A→B, B→A) → одно ребро.
            let edges = dedup_undirected_edges(raw_edges, &ids);

            Ok(GraphData { nodes, edges })
        })
        .await
}

/// Единый граф всего vault (AC-DOD-Ф3). В отличие от `GraphData`, несёт мета:
/// `total_files` (сколько всего файлов в vault) и `truncated` (показаны не все).
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FullGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub total_files: i64,
    pub truncated: bool,
}

/// Единый граф всего vault (AC-DOD-Ф3 «единый граф»). Узлы — топ-`limit` файлов по
/// **степени связности** (хабы наверх): на 50k это даёт осмысленный обзор, не перегружая
/// рендер (sigma + forceatlas2 в воркере). Рёбра — разрешённые связи внутри выбранных узлов.
pub async fn get_full_graph(reader: &ReadPool, limit: usize) -> DbResult<FullGraph> {
    let limit = limit.max(1) as i64;
    reader
        .query(move |c| {
            let total_files: i64 =
                c.query_row("SELECT COUNT(*) FROM files WHERE is_deleted = 0", [], |r| {
                    r.get(0)
                })?;

            // Топ-N файлов по НЕНАПРАВЛЕННОЙ степени (число инцидентных дедуп-рёбер), хабы первыми.
            // P1-19: канонические пары `p(a,b) = DISTINCT (MIN(s,t), MAX(s,t))` материализуем ОДИН
            // раз через CTE (реципрокная A→B,B→A → одна пара), затем каждый конец даёт +1 → ранг-степень
            // совпадает с числом инцидентных рёбер дедуп-графа (иначе реципрокные хабы выигрывали отбор).
            // CTE (а не нестед-подзапрос дважды) — один `SCAN links`, без двойного скана+sort (перф).
            let mut nstmt = c.prepare(
                "SELECT f.id, f.path, f.title \
                 FROM files f \
                 LEFT JOIN ( \
                     WITH p(a, b) AS ( \
                         SELECT DISTINCT MIN(source_id, target_id), MAX(source_id, target_id) \
                         FROM links WHERE target_id IS NOT NULL AND source_id != target_id \
                     ) \
                     SELECT id, COUNT(*) AS deg FROM ( \
                         SELECT a AS id FROM p UNION ALL SELECT b AS id FROM p \
                     ) GROUP BY id \
                 ) d ON d.id = f.id \
                 WHERE f.is_deleted = 0 \
                 ORDER BY COALESCE(d.deg, 0) DESC, f.id \
                 LIMIT ?1",
            )?;
            let mut nodes = nstmt
                .query_map([limit], |r| {
                    Ok(GraphNode {
                        id: r.get(0)?,
                        path: r.get(1)?,
                        title: r.get(2)?,
                        tags: Vec::new(),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            attach_tags(c, &mut nodes)?;

            let ids: BTreeSet<i64> = nodes.iter().map(|n| n.id).collect();
            let id_vec: Vec<i64> = ids.iter().copied().collect();
            // Рёбра внутри выбранных узлов — чанкуем по `source_id` + фильтр `target ∈ ids` в Rust
            // (как в get_local_graph): без двойного IN, безопасно при любом `limit` (ревью A9).
            let raw_edges = collect_in_chunks(
                c,
                &id_vec,
                |ph| {
                    format!(
                        "SELECT DISTINCT source_id, target_id FROM links \
                         WHERE target_id IS NOT NULL AND source_id IN ({ph})"
                    )
                },
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )?;
            // P1-19: дедуп в канонические ненаправленные пары — реципрокная (A→B, B→A) → одно ребро.
            let edges = dedup_undirected_edges(raw_edges, &ids);

            let truncated = total_files > nodes.len() as i64;
            Ok(FullGraph {
                nodes,
                edges,
                total_files,
                truncated,
            })
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::indexer::Indexer;
    use std::fs;
    use tempfile::TempDir;

    /// ADR-004 / AC-DOD-Ф0: беклинки приходят из SQLite, с контекстом.
    #[tokio::test]
    async fn backlinks_come_from_sqlite_with_context() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("A.md"), "# A\n\nсм. [[B]] тут\n").unwrap();
        fs::write(root.join("C.md"), "ссылка [[B]] и [[A]]\n").unwrap();
        fs::write(root.join("B.md"), "# B\n").unwrap();

        let db = Database::open(root.join(".nexus/nexus.db")).await.unwrap();
        let idx = Indexer::new(&db, root.clone());
        for f in ["A.md", "B.md", "C.md"] {
            idx.index_file(f).await.unwrap();
        }

        let mut bl = get_backlinks(db.reader(), "B.md".into()).await.unwrap();
        bl.sort_by(|a, b| a.source_path.cmp(&b.source_path));
        let sources: Vec<_> = bl.iter().map(|e| e.source_path.as_str()).collect();
        assert_eq!(sources, vec!["A.md", "C.md"]);
        assert!(bl[0].context.as_deref().unwrap_or("").contains("[[B]]"));
        assert!(bl[0].line_number.unwrap() >= 1);

        // У файла без входящих ссылок беклинков нет.
        let none = get_backlinks(db.reader(), "C.md".into()).await.unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn key_phrase_builds_adjacent_phrase_and_guards_short() {
        assert_eq!(
            key_phrase("RAG Pipeline").as_deref(),
            Some("\"RAG Pipeline\"")
        );
        assert_eq!(
            key_phrase("local-first").as_deref(),
            Some("\"local first\"")
        );
        assert_eq!(key_phrase("Go").as_deref(), None); // короткий → шум, не ищем
        assert_eq!(key_phrase("  ").as_deref(), None);
        // Пунктуация (вкл. кавычки) — разделитель токенов, в фразу попадают только токены.
        assert_eq!(key_phrase("a\"b cd").as_deref(), Some("\"a b cd\""));
        assert_eq!(basename_stem("Projects/RAG Pipeline.md"), "RAG Pipeline");
    }

    /// UNLINK-1: упоминания по ИМЕНИ файла И по H1-заголовку (разные ключи), исключая уже-линкующих
    /// и сам файл; несколько упоминателей переживают LIMIT (дедуп по файлу в SQL, а не по чанкам).
    /// RAG-индексатор (`with_rag`) ОБЯЗАТЕЛЕН — только он создаёт чанки/`fts_chunks` (грабля AIP-10).
    #[tokio::test]
    async fn unlinked_mentions_by_name_and_heading_excluding_linkers() {
        use crate::ai::{EmbeddingProvider, MockEmbedder};
        use crate::vector::VectorIndex;
        use std::sync::Arc;

        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        // Имя файла «Pipeline», но H1-заголовок «RAG Pipeline» — разные ключи (mustFix #2).
        fs::write(root.join("Pipeline.md"), "# RAG Pipeline\n\nописание\n").unwrap();
        fs::write(root.join("ByHeading.md"), "Читал про RAG Pipeline вчера.\n").unwrap();
        fs::write(root.join("ByName.md"), "Смотри Pipeline для деталей.\n").unwrap();
        fs::write(
            root.join("Linked.md"),
            "см [[Pipeline]] — RAG Pipeline там.\n",
        )
        .unwrap();
        fs::write(root.join("Other.md"), "# Other\n\nсовсем про другое\n").unwrap();

        let db = Database::open(root.join(".nexus/nexus.db")).await.unwrap();
        let vectors =
            Arc::new(VectorIndex::open(root.join(".nexus").join("vectors.usearch"), 16).unwrap());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder { dim: 16 });
        let idx = Indexer::with_rag(&db, root.clone(), embedder, vectors, true);
        for f in [
            "Pipeline.md",
            "ByHeading.md",
            "ByName.md",
            "Linked.md",
            "Other.md",
        ] {
            idx.index_file(f).await.unwrap();
        }

        let m = unlinked_mentions(db.reader(), "Pipeline.md".into())
            .await
            .unwrap();
        let paths: Vec<_> = m.iter().map(|e| e.source_path.as_str()).collect();
        assert!(
            paths.contains(&"ByHeading.md"),
            "упоминает H1 → есть: {paths:?}"
        );
        assert!(
            paths.contains(&"ByName.md"),
            "упоминает имя файла → есть: {paths:?}"
        );
        assert!(!paths.contains(&"Linked.md"), "уже ссылается → исключён");
        assert!(!paths.contains(&"Pipeline.md"), "сам файл → исключён");
        assert!(!paths.contains(&"Other.md"), "не упоминает → нет");
        assert!(m.iter().all(|e| !e.snippet.is_empty()), "сниппет у каждого");
    }

    #[tokio::test]
    async fn unlinked_mentions_short_title_is_empty() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("Go.md"), "# Go\n").unwrap();
        fs::write(root.join("Other.md"), "пишу про Go каждый день\n").unwrap();
        let db = Database::open(root.join(".nexus/nexus.db")).await.unwrap();
        let idx = Indexer::new(&db, root.clone());
        for f in ["Go.md", "Other.md"] {
            idx.index_file(f).await.unwrap();
        }
        // Короткий заголовок «Go» → title_phrase=None → пусто (без шума на пол-vault).
        assert!(unlinked_mentions(db.reader(), "Go.md".into())
            .await
            .unwrap()
            .is_empty());
    }

    /// AC-DOD-Ф0 (граф): локальный N-hop из SQLite расширяется с глубиной.
    #[tokio::test]
    async fn local_graph_expands_by_hops() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("A.md"), "[[B]] [[D]]\n").unwrap();
        fs::write(root.join("B.md"), "[[C]]\n").unwrap();
        fs::write(root.join("C.md"), "# C\n").unwrap();
        fs::write(root.join("D.md"), "# D\n").unwrap();

        let db = Database::open(root.join(".nexus/nexus.db")).await.unwrap();
        let idx = Indexer::new(&db, root.clone());
        for f in ["A.md", "B.md", "C.md", "D.md"] {
            idx.index_file(f).await.unwrap();
        }

        let g1 = get_local_graph(db.reader(), "A.md".into(), 1)
            .await
            .unwrap();
        let paths1: BTreeSet<_> = g1.nodes.iter().map(|n| n.path.as_str()).collect();
        assert_eq!(
            paths1,
            BTreeSet::from(["A.md", "B.md", "D.md"]),
            "1-hop: A и соседи B, D"
        );
        assert!(!paths1.contains("C.md")); // C — на 2-м хопе

        let g2 = get_local_graph(db.reader(), "A.md".into(), 2)
            .await
            .unwrap();
        let paths2: BTreeSet<_> = g2.nodes.iter().map(|n| n.path.as_str()).collect();
        assert!(paths2.contains("C.md"), "2-hop добавляет C");
        assert!(g2.edges.len() >= 3); // A-B, A-D, B-C

        // Несуществующий центр → пустой граф.
        let empty = get_local_graph(db.reader(), "Zzz.md".into(), 2)
            .await
            .unwrap();
        assert!(empty.nodes.is_empty() && empty.edges.is_empty());
    }

    /// AC-DOD-Ф3 (единый граф): полный граф отдаёт все файлы + мету;
    /// маленький лимит обрезает по степени связности (хабы наверх).
    #[tokio::test]
    async fn full_graph_returns_all_then_truncates_by_degree() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("A.md"), "[[B]] [[D]]\n").unwrap();
        fs::write(root.join("B.md"), "[[C]]\n").unwrap();
        fs::write(root.join("C.md"), "# C\n").unwrap();
        fs::write(root.join("D.md"), "# D\n").unwrap();

        let db = Database::open(root.join(".nexus/nexus.db")).await.unwrap();
        let idx = Indexer::new(&db, root.clone());
        for f in ["A.md", "B.md", "C.md", "D.md"] {
            idx.index_file(f).await.unwrap();
        }

        // Лимит с запасом → все 4 файла, не обрезано, мета честная.
        let full = get_full_graph(db.reader(), 100).await.unwrap();
        assert_eq!(full.total_files, 4);
        assert!(!full.truncated);
        let paths: BTreeSet<_> = full.nodes.iter().map(|n| n.path.as_str()).collect();
        assert_eq!(paths, BTreeSet::from(["A.md", "B.md", "C.md", "D.md"]));
        assert!(full.edges.len() >= 3); // A-B, A-D, B-C

        // Маленький лимит → обрезано до хабов (степень: A=2, B=2, C=1, D=1).
        let top = get_full_graph(db.reader(), 2).await.unwrap();
        assert_eq!(top.nodes.len(), 2);
        assert_eq!(top.total_files, 4);
        assert!(top.truncated);
        let tp: BTreeSet<_> = top.nodes.iter().map(|n| n.path.as_str()).collect();
        assert_eq!(tp, BTreeSet::from(["A.md", "B.md"]), "топ-2 по степени");
    }

    /// Срез «Граф: теги»: узлы обоих графов несут теги из `file_tags`
    /// (отсортированы по имени; без тегов — пустой вектор).
    #[tokio::test]
    async fn graph_nodes_carry_tags() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("A.md"), "#demo #docs см. [[B]]\n").unwrap();
        fs::write(root.join("B.md"), "# B\n").unwrap();

        let db = Database::open(root.join(".nexus/nexus.db")).await.unwrap();
        let idx = Indexer::new(&db, root.clone());
        for f in ["A.md", "B.md"] {
            idx.index_file(f).await.unwrap();
        }

        let local = get_local_graph(db.reader(), "A.md".into(), 1)
            .await
            .unwrap();
        let a = local.nodes.iter().find(|n| n.path == "A.md").unwrap();
        assert_eq!(a.tags, vec!["demo".to_string(), "docs".to_string()]);
        let b = local.nodes.iter().find(|n| n.path == "B.md").unwrap();
        assert!(b.tags.is_empty());

        let full = get_full_graph(db.reader(), 10).await.unwrap();
        let a = full.nodes.iter().find(|n| n.path == "A.md").unwrap();
        assert_eq!(a.tags, vec!["demo".to_string(), "docs".to_string()]);
    }

    /// Ревью A9: граф на супер-хабе (узел с тысячами связей) не падает на `too many SQL variables`
    /// и отдаёт ПОЛНЫЙ результат через чанкинг IN-запросов. N=1000 > SQL_VAR_CHUNK(900) → много чанков
    /// (узлы: 2 чанка; frontier hop-1: набор повторяется в OR). Фикстуру вставляем напрямую в БД
    /// (быстро), минуя индексатор.
    #[tokio::test]
    async fn super_hub_does_not_exceed_sql_var_limit() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join(".nexus/nexus.db"))
            .await
            .unwrap();

        const N: i64 = 1000;
        db.writer()
            .transaction(|tx| {
                // hub.md = id 1; n0..n(N-1) = id 2..N+1; hub ссылается на каждый.
                tx.execute(
                    "INSERT INTO files (id,path,hash,created_at,updated_at,indexed_at,size_bytes) \
                     VALUES (1,'hub.md','h',0,0,0,0)",
                    [],
                )?;
                for i in 0..N {
                    let fid = i + 2;
                    tx.execute(
                        "INSERT INTO files (id,path,hash,created_at,updated_at,indexed_at,size_bytes) \
                         VALUES (?1,?2,'h',0,0,0,0)",
                        rusqlite::params![fid, format!("n{i}.md")],
                    )?;
                    tx.execute(
                        "INSERT INTO links (source_id,target_id,target_raw,link_type) \
                         VALUES (1,?1,?2,'wikilink')",
                        rusqlite::params![fid, format!("n{i}")],
                    )?;
                }
                Ok(())
            })
            .await
            .unwrap();

        // 1-hop из хаба: ВСЕ N+1 узлов и N рёбер, без ошибки SQLite-лимита переменных.
        let g = get_local_graph(db.reader(), "hub.md".into(), 1)
            .await
            .expect("супер-хаб не должен валить запрос лимитом переменных");
        assert_eq!(g.nodes.len() as i64, N + 1, "все узлы (hub + N соседей)");
        assert_eq!(g.edges.len() as i64, N, "все рёбра hub→nK");

        // Полный граф с запасом по лимиту — тоже все узлы/рёбра.
        let full = get_full_graph(db.reader(), (N as usize) + 10)
            .await
            .unwrap();
        assert_eq!(full.nodes.len() as i64, N + 1);
        assert_eq!(full.edges.len() as i64, N);
    }

    /// Аудит: self-link (`[[A]]` в A.md) не показывается как беклинк самой A и не даёт self-loop ребра
    /// в графе (бессмысленно в knowledge-graph; раздувало беклинки/степень).
    #[tokio::test]
    async fn self_link_excluded_from_backlinks_and_edges() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join(".nexus/nexus.db"))
            .await
            .unwrap();
        db.writer()
            .transaction(|tx| {
                for (id, p) in [(1, "A.md"), (2, "B.md")] {
                    tx.execute(
                        "INSERT INTO files (id,path,hash,created_at,updated_at,indexed_at,size_bytes) \
                         VALUES (?1,?2,'h',0,0,0,0)",
                        rusqlite::params![id, p],
                    )?;
                }
                // A ссылается на СЕБЯ (self-loop) и на B.
                tx.execute(
                    "INSERT INTO links (source_id,target_id,target_raw,link_type) VALUES (1,1,'A','wikilink')",
                    [],
                )?;
                tx.execute(
                    "INSERT INTO links (source_id,target_id,target_raw,link_type) VALUES (1,2,'B','wikilink')",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        // Беклинки A: self-link исключён → пусто.
        assert!(
            get_backlinks(db.reader(), "A.md".into())
                .await
                .unwrap()
                .is_empty(),
            "self-backlink не показывается"
        );
        // Беклинки B: A ссылается → одна запись.
        assert_eq!(
            get_backlinks(db.reader(), "B.md".into())
                .await
                .unwrap()
                .len(),
            1
        );
        // Граф: ни одного self-loop ребра, ребро A→B на месте.
        let g = get_full_graph(db.reader(), 10).await.unwrap();
        assert!(
            g.edges.iter().all(|e| e.source != e.target),
            "self-loop рёбер нет"
        );
        assert!(
            g.edges.iter().any(|e| e.source == 1 && e.target == 2),
            "ребро A→B на месте"
        );
    }

    /// P1-19: реципрокная пара (links A→B И B→A — взаимные вики-ссылки) даёт РОВНО ОДНО
    /// ненаправленное ребро (не два), иначе степень/размер узла и счётчик рёбер раздувались бы
    /// вдвое, а в рендере была бы двойная линия. Одно-направленная пара — тоже одно ребро;
    /// self-loop отфильтрован. Проверяем оба места сбора рёбер (`get_local_graph` + `get_full_graph`).
    #[tokio::test]
    async fn reciprocal_links_collapse_to_single_undirected_edge() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join(".nexus/nexus.db"))
            .await
            .unwrap();
        db.writer()
            .transaction(|tx| {
                for (id, p) in [(1, "A.md"), (2, "B.md"), (3, "C.md")] {
                    tx.execute(
                        "INSERT INTO files (id,path,hash,created_at,updated_at,indexed_at,size_bytes) \
                         VALUES (?1,?2,'h',0,0,0,0)",
                        rusqlite::params![id, p],
                    )?;
                }
                // Реципрокная пара A↔B (две строки A→B и B→A) + одно-направленная A→C + self-loop B→B.
                for (s, t, raw) in [(1, 2, "B"), (2, 1, "A"), (1, 3, "C"), (2, 2, "B")] {
                    tx.execute(
                        "INSERT INTO links (source_id,target_id,target_raw,link_type) \
                         VALUES (?1,?2,?3,'wikilink')",
                        rusqlite::params![s, t, raw],
                    )?;
                }
                Ok(())
            })
            .await
            .unwrap();

        // Хелпер: число рёбер между неупорядоченной парой {x,y} (порядок концов не важен).
        fn count_pair(edges: &[GraphEdge], x: i64, y: i64) -> usize {
            edges
                .iter()
                .filter(|e| (e.source == x && e.target == y) || (e.source == y && e.target == x))
                .count()
        }

        // Полный граф: A↔B — одно ребро, A↔C — одно, self-loop B↔B отброшен → всего 2 ребра.
        let full = get_full_graph(db.reader(), 100).await.unwrap();
        assert_eq!(
            count_pair(&full.edges, 1, 2),
            1,
            "реципрокная пара A↔B → ровно одно ребро (full)"
        );
        assert_eq!(count_pair(&full.edges, 1, 3), 1, "A→C → одно ребро (full)");
        assert!(
            full.edges.iter().all(|e| e.source != e.target),
            "self-loop отфильтрован (full)"
        );
        assert_eq!(full.edges.len(), 2, "всего 2 ненаправленных ребра (full)");

        // Локальный граф из A (2 хопа покрывает B и C): та же дедупликация во ВТОРОМ месте сбора.
        let local = get_local_graph(db.reader(), "A.md".into(), 2)
            .await
            .unwrap();
        assert_eq!(
            count_pair(&local.edges, 1, 2),
            1,
            "реципрокная пара A↔B → ровно одно ребро (local)"
        );
        assert_eq!(
            count_pair(&local.edges, 1, 3),
            1,
            "A→C → одно ребро (local)"
        );
        assert!(
            local.edges.iter().all(|e| e.source != e.target),
            "self-loop отфильтрован (local)"
        );
        assert_eq!(local.edges.len(), 2, "всего 2 ненаправленных ребра (local)");
    }

    /// P1-19 (хвост MINOR): TOP-N отбор хабов в `get_full_graph` ранжирует по НЕНАПРАВЛЕННОЙ
    /// степени, согласованной с дедуп-рёбрами. Узел R с НЕСКОЛЬКИМИ реципрокными связями к ОДНОМУ
    /// соседу (R↔P, две строки) НЕ должен обогнать узел H с бОльшим числом УНИКАЛЬНЫХ соседей.
    /// Со старой сырой направленной степенью R считался бы как 2 (source в R→P + target в P→R),
    /// сравнявшись с H (2 уникальных соседа) и обойдя его по tie-break id — отбор был бы кривой.
    #[tokio::test]
    async fn full_graph_top_n_ranks_by_undirected_degree() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join(".nexus/nexus.db"))
            .await
            .unwrap();
        db.writer()
            .transaction(|tx| {
                // id 1=R (реципрокный), 2=P (партнёр R), 3=H (хаб), 4=X, 5=Y (соседи H).
                for (id, p) in [(1, "R.md"), (2, "P.md"), (3, "H.md"), (4, "X.md"), (5, "Y.md")] {
                    tx.execute(
                        "INSERT INTO files (id,path,hash,created_at,updated_at,indexed_at,size_bytes) \
                         VALUES (?1,?2,'h',0,0,0,0)",
                        rusqlite::params![id, p],
                    )?;
                }
                // R↔P — реципрокная пара (две строки) → R ненаправленная степень 1, НЕ 2.
                // H→X, H→Y — две УНИКАЛЬНЫЕ связи → H ненаправленная степень 2.
                for (s, t) in [(1, 2), (2, 1), (3, 4), (3, 5)] {
                    tx.execute(
                        "INSERT INTO links (source_id,target_id,target_raw,link_type) \
                         VALUES (?1,?2,'x','wikilink')",
                        rusqlite::params![s, t],
                    )?;
                }
                Ok(())
            })
            .await
            .unwrap();

        // Лимит 1 → берём ровно одного хаба. По ненаправленной степени это H (deg 2 > R deg 1).
        // Со старой направленной степенью R тоже имел бы 2 и обогнал бы H по `f.id` (1 < 3).
        let top1 = get_full_graph(db.reader(), 1).await.unwrap();
        assert_eq!(top1.nodes.len(), 1);
        assert_eq!(
            top1.nodes[0].path, "H.md",
            "TOP-1 — узел с бОльшей НЕНАПРАВЛЕННОЙ степенью (H=2), а не реципрокный R (=1)"
        );

        // И прямая проверка степени по дедуп-рёбрам в полном графе: R инцидентен 1 ребру, H — 2.
        let full = get_full_graph(db.reader(), 100).await.unwrap();
        let deg_of = |id: i64| {
            full.edges
                .iter()
                .filter(|e| e.source == id || e.target == id)
                .count()
        };
        assert_eq!(
            deg_of(1),
            1,
            "R: одно дедуп-ребро (реципрокная пара не двоит)"
        );
        assert_eq!(deg_of(3), 2, "H: два дедуп-ребра");
    }
}
