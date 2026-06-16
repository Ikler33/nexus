//! Канбан-доска (BOARD-2): кросс-файловая выборка «задач» = заметок с frontmatter-ключом `status`.
//! Чистый SQL-read из индекса (`frontmatter_fields` + `file_tags`), без LLM/сети — детерминирован, офлайн.
//!
//! ВАЖНО (§14.2 спеки): задача-доски ≠ чеклист-строка `commands/tasks.rs`. Та модель — подзадачи
//! `- [ ]` ВНУТРИ заметки (скан тел). Доска оперирует ТОЛЬКО заметками-задачами через индекс (никакого
//! `list_tasks`-скана). Колонкование (по значению `status`) и ручной порядок — на фронте / в board JSON
//! (BOARD-3); здесь — плоский детерминированный список карточек.

use serde::Serialize;

use crate::db::{DbResult, ReadPool};

/// Персист доски (BOARD-3): конфиг колонок/порядка/scope в `.nexus/boards/<id>.json`.
pub mod config;

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

/// Застрявшая задача (AI-2a, спека §10 A2): заметка-задача, не правленная дольше порога. `last_edit` —
/// max наблюдённого `edit_events.ts` (фолбэк `files.updated_at`, если событий ещё нет — заметка
/// проиндексирована до P2/мигр.015); `days_stale` = floor((now − last_edit)/86400). Терминальные
/// (`done`-like) статусы здесь НЕ отсеиваются — бэкенд не знает колонок доски; фронт фильтрует по конфигу.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaleTask {
    pub path: String,
    pub title: Option<String>,
    pub status: String,
    pub last_edit: i64,
    pub days_stale: i64,
}

/// Задачи (есть `status_key`), не правленные ≥ `threshold_days` дней относительно `now` (unix-сек).
/// Детерминированный SQL-read из индекса (без LLM/сети). `last_edit` = `COALESCE(MAX(edit_events.ts),
/// files.updated_at)`. Сорт «застряло дольше» (самые старые сверху), затем путь — детерминизм. Пусто, если
/// нет задач старше порога.
pub async fn stale_tasks(
    reader: &ReadPool,
    status_key: String,
    threshold_days: i64,
    now: i64,
) -> DbResult<Vec<StaleTask>> {
    // saturating — устойчивость к экстремальному порогу при прямом вызове lib-функции (команда клампит ≥1).
    let cutoff = now.saturating_sub(threshold_days.max(0).saturating_mul(86_400));
    reader
        .query(move |c| {
            let mut stmt = c.prepare(
                "SELECT f.path, f.title, st.value, COALESCE(MAX(e.ts), f.updated_at) AS last_edit \
                 FROM files f \
                 JOIN frontmatter_fields st ON st.file_id = f.id AND st.key = ?1 \
                 LEFT JOIN edit_events e ON e.file_id = f.id \
                 WHERE f.is_deleted = 0 \
                 GROUP BY f.id \
                 HAVING last_edit <= ?2 \
                 ORDER BY last_edit ASC, f.path ASC",
            )?;
            let rows = stmt.query_map(rusqlite::params![&status_key, cutoff], |r| {
                let last_edit: i64 = r.get(3)?;
                Ok(StaleTask {
                    path: r.get(0)?,
                    title: r.get(1)?,
                    status: r.get(2)?,
                    last_edit,
                    days_stale: (now - last_edit).max(0) / 86_400,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<StaleTask>>>()
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

    const DAY: i64 = 86_400;

    /// Засевает files(updated_at) + frontmatter_fields(status) + опц. edit_events(ts) напрямую — чтобы
    /// детерминированно контролировать «возраст» задачи (индексатор пишет ts≈сейчас, для stale-теста нужны
    /// заданные времена). `(path, status_opt, updated_at, edit_ts_opt)`.
    async fn db_with_tasks(rows: &[(&str, Option<&str>, i64, Option<i64>)]) -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join("nexus.db")).await.unwrap();
        let rows: Vec<(String, Option<String>, i64, Option<i64>)> = rows
            .iter()
            .map(|(p, s, u, e)| (p.to_string(), s.map(str::to_string), *u, *e))
            .collect();
        db.writer()
            .call(move |c| {
                for (path, status, updated_at, edit_ts) in &rows {
                    c.execute(
                        "INSERT INTO files (path,hash,title,created_at,updated_at,indexed_at,size_bytes,word_count) \
                         VALUES (?1,'h',?1,0,?2,0,1,42)",
                        rusqlite::params![path, updated_at],
                    )?;
                    let fid = c.last_insert_rowid();
                    if let Some(s) = status {
                        c.execute(
                            "INSERT INTO frontmatter_fields (file_id,key,value) VALUES (?1,'status',?2)",
                            rusqlite::params![fid, s],
                        )?;
                    }
                    if let Some(ts) = edit_ts {
                        c.execute(
                            "INSERT INTO edit_events (file_id,ts,kind) VALUES (?1,?2,'modify')",
                            rusqlite::params![fid, ts],
                        )?;
                    }
                }
                Ok(())
            })
            .await
            .unwrap();
        (dir, db)
    }

    /// AI-2a: задача застряла, если последнее НАБЛЮДЁННОЕ изменение (`MAX(edit_events.ts)`, фолбэк mtime)
    /// старше порога. edit_events перебивает mtime (touch без смены контента не «освежает»); не-задача и
    /// свежая задача исключены; done-like НЕ фильтруется бэкендом; сорт — самые старые сверху.
    #[tokio::test]
    async fn stale_tasks_threshold_and_edit_events_fallback() {
        let now = 1_780_000_000;
        let (_d, db) = db_with_tasks(&[
            ("stale.md", Some("todo"), now - 40 * DAY, None), // фолбэк mtime: 40д → застряла
            ("done.md", Some("done"), now - 20 * DAY, None), // 20д → застряла (done не фильтруем тут)
            (
                "fresh-mtime.md",
                Some("doing"),
                now - 50 * DAY,
                Some(now - 2 * DAY),
            ), // edit 2д назад → НЕ застряла
            ("touched.md", Some("todo"), now - DAY, Some(now - 30 * DAY)), // mtime свежий, но edit 30д → застряла
            ("notask.md", None, now - 99 * DAY, None),                     // не задача → исключена
        ])
        .await;

        let stale = stale_tasks(db.reader(), DEFAULT_STATUS_KEY.to_string(), 14, now)
            .await
            .unwrap();
        let paths: Vec<&str> = stale.iter().map(|s| s.path.as_str()).collect();
        // last_edit ASC (самые старые сверху): stale(now−40d) < touched(now−30d) < done(now−20d).
        assert_eq!(paths, vec!["stale.md", "touched.md", "done.md"]);
        let by: HashMap<&str, &StaleTask> = stale.iter().map(|s| (s.path.as_str(), s)).collect();
        assert_eq!(by["stale.md"].days_stale, 40);
        assert_eq!(
            by["touched.md"].days_stale, 30,
            "edit_events перебивает свежий mtime"
        );
        assert!(
            !by.contains_key("fresh-mtime.md"),
            "свежий edit_event → не застряла"
        );
        assert!(!by.contains_key("notask.md"), "не задача исключена");
    }
}
