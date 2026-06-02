//! Поиск (Ф0): по title / path / tags на стороне SQLite.
//!
//! Полнотекстовый поиск по ТЕЛУ (FTS5 поверх `chunks`, §5) — Фаза 1: chunks появляются там.
//! Здесь — явное допущение Ф0 «поиск по метаданным» (§11.7 промпта), чтобы не блокироваться.

use crate::db::{DbResult, ReadPool};
use crate::vault::NoteRef;

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
}
