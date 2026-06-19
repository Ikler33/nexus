//! Теги vault (DP-2): список с количеством заметок — панель «Теги» сайдбара (макет
//! `sidebar.jsx`). Источник — индексные таблицы `tags`/`file_tags` (инлайн-теги тела,
//! Ф0-индексатор); frontmatter-теги — отдельный хвост BACKLOG (#35).

use serde::Serialize;

use crate::db::{DbResult, ReadPool};
use crate::vault::NoteRef;

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

/// Заметки с ТОЧНЫМ тегом (exact-match по имени, НЕ substring) — фильтр панели «Теги» сайдбара.
/// Свежие сверху. Точный фильтр чинит зашумлённый срез `search_notes` (клик по тегу «ai» ловил всё
/// с «ai» в пути/заголовке/других тегах — substring); тут — ровно заметки с этим тегом.
pub async fn notes_by_tag(reader: &ReadPool, tag: &str) -> DbResult<Vec<NoteRef>> {
    let tag = tag.to_string();
    reader
        .query(move |c| {
            let mut stmt = c.prepare(
                "SELECT f.path, f.title FROM files f \
                 JOIN file_tags ft ON ft.file_id = f.id \
                 JOIN tags t ON t.id = ft.tag_id \
                 WHERE t.name = ?1 AND f.is_deleted = 0 \
                 ORDER BY f.updated_at DESC, f.path LIMIT 200",
            )?;
            let rows = stmt
                .query_map([tag], |r| {
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

    /// notes_by_tag — ТОЧНЫЙ фильтр: ровно заметки с этим тегом, свежие сверху; substring-совпадения
    /// по пути/заголовку/другому тегу (#airflow) и удалённые НЕ попадают.
    #[tokio::test]
    async fn notes_by_tag_exact_recent_first_excludes_substring_and_deleted() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join("nexus.db")).await.unwrap();
        db.writer()
            .call(|c| {
                c.execute(
                    "INSERT INTO files (path,hash,title,created_at,updated_at,indexed_at,size_bytes,word_count) \
                     VALUES ('ai.md','h','AI',0,20,0,1,1),('recent.md','h','Recent',0,30,0,1,1),\
                            ('ai-notes.md','h','Airflow',0,10,0,1,1),('dead.md','h','d',0,99,0,1,1)",
                    [],
                )?;
                c.execute("UPDATE files SET is_deleted=1 WHERE path='dead.md'", [])?;
                c.execute("INSERT INTO tags (name) VALUES ('ai'),('airflow')", [])?;
                // ai.md/recent.md/dead.md → #ai; ai-notes.md → #airflow (substring 'ai' в имени тега и пути).
                c.execute(
                    "INSERT INTO file_tags (file_id,tag_id) \
                     SELECT f.id, t.id FROM files f, tags t \
                     WHERE (f.path IN ('ai.md','recent.md','dead.md') AND t.name='ai') \
                        OR (f.path='ai-notes.md' AND t.name='airflow')",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let out = notes_by_tag(db.reader(), "ai").await.unwrap();
        let paths: Vec<&str> = out.iter().map(|n| n.path.as_str()).collect();
        assert_eq!(
            paths,
            ["recent.md", "ai.md"],
            "ровно #ai, свежие сверху; #airflow/путь-substring и dead исключены"
        );
    }
}
