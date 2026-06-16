//! Канбан-доска (BOARD-2): кросс-файловая выборка «задач» = заметок с frontmatter-ключом `status`.
//! Чистый SQL-read из индекса (`frontmatter_fields` + `file_tags`), без LLM/сети — детерминирован, офлайн.
//!
//! ВАЖНО (§14.2 спеки): задача-доски ≠ чеклист-строка `commands/tasks.rs`. Та модель — подзадачи
//! `- [ ]` ВНУТРИ заметки (скан тел). Доска оперирует ТОЛЬКО заметками-задачами через индекс (никакого
//! `list_tasks`-скана). Колонкование (по значению `status`) и ручной порядок — на фронте / в board JSON
//! (BOARD-3); здесь — плоский детерминированный список карточек.

use serde::Serialize;

use crate::db::{DbResult, ReadPool};

/// Ключ статуса по умолчанию (колонка доски = его значение).
pub const DEFAULT_STATUS_KEY: &str = "status";

/// Карточка задачи: путь+заголовок + плоские frontmatter-скаляры (`status` обязателен, прочее опц.) +
/// теги (из `file_tags`, отсортированы). `status` — raw-значение как в файле (нормализация колонок — фронт).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCard {
    pub path: String,
    pub title: Option<String>,
    pub status: String,
    pub project: Option<String>,
    pub priority: Option<String>,
    pub due: Option<String>,
    pub tags: Vec<String>,
}

/// Все заметки-задачи (есть frontmatter-ключ `status_key`) с полями для доски. INNER JOIN по статусу =
/// «только задачи»; LEFT JOIN project/priority/due (`frontmatter_fields` UNIQUE(file_id,key) → ≤1 строка).
/// Теги — коррелированный `group_concat` (имена тегов валидны без запятых: alnum/`_`/`-`/`/`). Сорт по
/// пути (детерминизм; ручной порядок придёт из board JSON в BOARD-3). Пусто, если задач нет.
pub async fn list_board(reader: &ReadPool, status_key: String) -> DbResult<Vec<TaskCard>> {
    reader
        .query(move |c| {
            let mut stmt = c.prepare(
                "SELECT f.path, f.title, st.value, pr.value, pri.value, due.value, \
                    (SELECT group_concat(name, ',') FROM \
                        (SELECT t.name AS name FROM file_tags ft JOIN tags t ON t.id = ft.tag_id \
                         WHERE ft.file_id = f.id ORDER BY t.name)) AS tags \
                 FROM files f \
                 JOIN frontmatter_fields st ON st.file_id = f.id AND st.key = ?1 \
                 LEFT JOIN frontmatter_fields pr ON pr.file_id = f.id AND pr.key = 'project' \
                 LEFT JOIN frontmatter_fields pri ON pri.file_id = f.id AND pri.key = 'priority' \
                 LEFT JOIN frontmatter_fields due ON due.file_id = f.id AND due.key = 'due' \
                 WHERE f.is_deleted = 0 \
                 ORDER BY f.path",
            )?;
            let rows = stmt.query_map([&status_key], |r| {
                let tags_csv: Option<String> = r.get(6)?;
                let tags = tags_csv
                    .map(|s| s.split(',').map(str::to_string).collect())
                    .unwrap_or_default();
                Ok(TaskCard {
                    path: r.get(0)?,
                    title: r.get(1)?,
                    status: r.get(2)?,
                    project: r.get(3)?,
                    priority: r.get(4)?,
                    due: r.get(5)?,
                    tags,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<TaskCard>>>()
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{EmbeddingProvider, MockEmbedder};
    use crate::db::Database;
    use crate::indexer::Indexer;
    use crate::vector::VectorIndex;
    use std::collections::HashMap;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn index(root: &std::path::Path, files: &[(&str, &str)]) -> Database {
        let db = Database::open(root.join(".nexus/nexus.db")).await.unwrap();
        let vectors =
            Arc::new(VectorIndex::open(root.join(".nexus").join("vectors.usearch"), 16).unwrap());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder { dim: 16 });
        let idx = Indexer::with_rag(&db, root.to_path_buf(), embedder, vectors, true);
        for (name, body) in files {
            fs::write(root.join(name), body).unwrap();
            idx.index_file(name).await.unwrap();
        }
        db
    }

    /// Задача = заметка с `status`; project/priority/due/tags подтягиваются; не-задача исключена.
    #[tokio::test]
    async fn lists_only_notes_with_status_and_their_fields() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let db = index(
            &root,
            &[
                (
                    "t1.md",
                    "---\nstatus: doing\nproject: Nexus\npriority: high\ndue: 2026-06-20\ntags: [task, frontend]\n---\n# Task 1\n",
                ),
                ("t2.md", "---\nstatus: todo\n---\n# Task 2\nбез полей\n"),
                ("t3.md", "---\nstatus: готово\n---\n# Кириллический статус\n"),
                ("n1.md", "# Not a task\n\nобычная заметка без status\n"),
            ],
        )
        .await;

        let cards = list_board(db.reader(), DEFAULT_STATUS_KEY.to_string())
            .await
            .unwrap();
        let by: HashMap<&str, &TaskCard> = cards.iter().map(|c| (c.path.as_str(), c)).collect();
        assert_eq!(by.len(), 3, "3 задачи (есть status), n1 исключена");
        assert!(!by.contains_key("n1.md"), "не-задача исключена");

        let t1 = by["t1.md"];
        assert_eq!(t1.status, "doing");
        assert_eq!(t1.project.as_deref(), Some("Nexus"));
        assert_eq!(t1.priority.as_deref(), Some("high"));
        assert_eq!(t1.due.as_deref(), Some("2026-06-20"));
        assert_eq!(t1.tags, vec!["frontend".to_string(), "task".to_string()]); // отсортированы

        let t2 = by["t2.md"];
        assert_eq!(t2.status, "todo");
        assert_eq!(t2.project, None);
        assert!(t2.tags.is_empty());

        assert_eq!(
            by["t3.md"].status, "готово",
            "raw-значение статуса как в файле"
        );
    }

    /// Сорт по пути — детерминирован; пустой vault → пустой список.
    #[tokio::test]
    async fn deterministic_order_and_empty() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let db = index(
            &root,
            &[
                ("b.md", "---\nstatus: todo\n---\nB\n"),
                ("a.md", "---\nstatus: todo\n---\nA\n"),
            ],
        )
        .await;
        let cards = list_board(db.reader(), DEFAULT_STATUS_KEY.to_string())
            .await
            .unwrap();
        let paths: Vec<&str> = cards.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, vec!["a.md", "b.md"], "сорт по пути");

        // Кастомный status_key, которого нет → пусто.
        let none = list_board(db.reader(), "phase".to_string()).await.unwrap();
        assert!(none.is_empty(), "нет ключа phase → нет карточек");
    }
}
