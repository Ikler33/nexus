//! Граф ссылок — **ADR-004**: источник истины = SQLite. Беклинки и обходы — запросами
//! по индексу `idx_links_target`; petgraph в памяти НЕ держим (нет дублирования/рассинхрона).

use std::collections::BTreeSet;

use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::db::{DbResult, ReadPool};

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
                let neighbors = stmt
                    .query_map(params.as_slice(), |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;

                let mut next = Vec::new();
                for (s, t) in neighbors {
                    for n in [s, t] {
                        if ids.insert(n) {
                            next.push(n);
                        }
                    }
                }
                frontier = next;
            }

            let id_ph = vec!["?"; ids.len()].join(",");
            let id_params: Vec<&dyn rusqlite::ToSql> =
                ids.iter().map(|x| x as &dyn rusqlite::ToSql).collect();

            let mut nstmt = c.prepare(&format!(
                "SELECT id, path, title FROM files WHERE id IN ({id_ph})"
            ))?;
            let nodes = nstmt
                .query_map(id_params.as_slice(), |r| {
                    Ok(GraphNode {
                        id: r.get(0)?,
                        path: r.get(1)?,
                        title: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let edge_params: Vec<&dyn rusqlite::ToSql> = ids
                .iter()
                .chain(ids.iter())
                .map(|x| x as &dyn rusqlite::ToSql)
                .collect();
            let mut estmt = c.prepare(&format!(
                "SELECT DISTINCT source_id, target_id FROM links \
                 WHERE target_id IS NOT NULL AND source_id IN ({id_ph}) AND target_id IN ({id_ph})"
            ))?;
            let edges = estmt
                .query_map(edge_params.as_slice(), |r| {
                    Ok(GraphEdge {
                        source: r.get(0)?,
                        target: r.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            Ok(GraphData { nodes, edges })
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
}
