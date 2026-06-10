//! Теги vault (DP-2): список с количеством заметок — панель «Теги» сайдбара (макет
//! `sidebar.jsx`). Источник — индексные таблицы `tags`/`file_tags` (инлайн-теги тела,
//! Ф0-индексатор); frontmatter-теги — отдельный хвост BACKLOG (#35).

use serde::Serialize;

use crate::db::{DbResult, ReadPool};

/// Тег с числом не-удалённых заметок.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagCount {
    pub name: String,
    pub count: i64,
}

/// Все теги по убыванию частоты (счёт только по живым файлам); пустые (без файлов) не отдаются.
pub async fn list_tags(reader: &ReadPool) -> DbResult<Vec<TagCount>> {
    reader
        .query(|c| {
            let mut stmt = c.prepare(
                "SELECT t.name, count(ft.file_id) AS n FROM tags t \
                 JOIN file_tags ft ON ft.tag_id = t.id \
                 JOIN files f ON f.id = ft.file_id AND f.is_deleted = 0 \
                 GROUP BY t.id HAVING n > 0 ORDER BY n DESC, t.name",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(TagCount {
                        name: r.get(0)?,
                        count: r.get(1)?,
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
    use tempfile::TempDir;

    /// Теги считаются по живым файлам, сортировка по частоте, пустые теги скрыты.
    #[tokio::test]
    async fn counts_live_files_orders_by_frequency() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join("nexus.db")).await.unwrap();
        db.writer()
            .call(|c| {
                c.execute(
                    "INSERT INTO files (path,hash,title,created_at,updated_at,indexed_at,size_bytes,word_count) \
                     VALUES ('a.md','h','a',0,0,0,1,1),('b.md','h','b',0,0,0,1,1),('dead.md','h','d',0,0,0,1,1)",
                    [],
                )?;
                c.execute("UPDATE files SET is_deleted=1 WHERE path='dead.md'", [])?;
                c.execute("INSERT INTO tags (name) VALUES ('ai'),('rag'),('empty')", [])?;
                c.execute(
                    "INSERT INTO file_tags (file_id,tag_id) \
                     SELECT f.id, t.id FROM files f, tags t \
                     WHERE (f.path IN ('a.md','b.md') AND t.name='ai') \
                        OR (f.path='a.md' AND t.name='rag') \
                        OR (f.path='dead.md' AND t.name='rag')",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let tags = list_tags(db.reader()).await.unwrap();
        let pairs: Vec<(&str, i64)> = tags.iter().map(|t| (t.name.as_str(), t.count)).collect();
        assert_eq!(
            pairs,
            vec![("ai", 2), ("rag", 1)],
            "dead не считается, empty скрыт"
        );
    }
}
