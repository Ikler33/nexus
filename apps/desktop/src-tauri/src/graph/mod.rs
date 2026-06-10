//! Граф ссылок — **ADR-004**: источник истины = SQLite. Беклинки и обходы — запросами
//! по индексу `idx_links_target`; petgraph в памяти НЕ держим (нет дублирования/рассинхрона).

use std::collections::BTreeSet;

use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::db::{DbResult, ReadPool};

/// Безопасный батч bind-параметров на один SQLite-запрос. Современный bundled-SQLite держит 32766,
/// но чанкуем по 900 — переносимо (старые сборки = 999) и с двойным запасом (часть запросов повторяет
/// набор в `IN` дважды). Граф на супер-хабе (узел с десятками тысяч связей) иначе ловил
/// `too many SQL variables` и валил команду (ревью A9). Чанкинг сохраняет ПОЛНЫЙ результат (без обрезки).
const SQL_VAR_CHUNK: usize = 900;

/// Гоняет запрос с одним `IN ({ph})` по `ids` чанками ≤ [`SQL_VAR_CHUNK`] и собирает строки.
/// `make_sql(ph)` строит SQL по строке плейсхолдеров; `map_row` маппит строку результата.
fn collect_in_chunks<T>(
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
            let edges: Vec<GraphEdge> = raw_edges
                .into_iter()
                .filter(|(_, t)| ids.contains(t))
                .map(|(source, target)| GraphEdge { source, target })
                .collect();

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

            // Топ-N файлов по степени (число инцидентных разрешённых связей), хабы первыми.
            let mut nstmt = c.prepare(
                "SELECT f.id, f.path, f.title \
                 FROM files f \
                 LEFT JOIN ( \
                     SELECT id, COUNT(*) AS deg FROM ( \
                         SELECT source_id AS id FROM links WHERE target_id IS NOT NULL \
                         UNION ALL \
                         SELECT target_id AS id FROM links WHERE target_id IS NOT NULL \
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
            let edges: Vec<GraphEdge> = raw_edges
                .into_iter()
                .filter(|(_, t)| ids.contains(t))
                .map(|(source, target)| GraphEdge { source, target })
                .collect();

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
}
