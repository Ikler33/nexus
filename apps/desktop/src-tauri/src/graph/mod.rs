//! Граф ссылок — **ADR-004**: источник истины = SQLite. Беклинки и обходы — запросами
//! по индексу `idx_links_target`; petgraph в памяти НЕ держим (нет дублирования/рассинхрона).

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
}
