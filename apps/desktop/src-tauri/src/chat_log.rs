//! Сессии чата (решение владельца 2026-06-12): переписка живёт в vault-БД как часть «второго
//! мозга» — храним всё, ничего не удаляем. Заголовок — суммарайз первого вопроса мелкой моделью
//! (плейсхолдер до генерации — обрезанный вопрос). Экспорт сессии в md-заметку — ЯВНОЙ кнопкой
//! (владелец: «куча диалогов недостойны отдельных заметок», но память по ним нужна).

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::db::{DbResult, ReadPool, WriteActor};
use crate::scheduler::now_secs;

/// Плейсхолдер-заголовок: первые символы вопроса до генерации мелкой моделью.
const TITLE_PLACEHOLDER_CHARS: usize = 48;

/// Сессия для списка-истории (дропдаун в шапке AI-панели).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSession {
    pub id: i64,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Сообщение сессии (восстановление ленты при загрузке).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredMessage {
    pub role: String,
    pub content: String,
    /// JSON-снапшот источников фронта (vault `sources` / web `webSources`) — как было показано.
    pub sources_json: Option<String>,
    pub created_at: i64,
}

/// Пишет завершённый обмен (вопрос + ответ). `session_id=None` → создаёт сессию с
/// плейсхолдер-заголовком из вопроса. Возвращает (session_id, created — нужна ли генерация заголовка).
pub async fn log_exchange(
    writer: &WriteActor,
    session_id: Option<i64>,
    question: &str,
    answer: &str,
    sources_json: Option<String>,
) -> DbResult<(i64, bool)> {
    let q = question.to_string();
    let a = answer.to_string();
    writer
        .transaction(move |tx| {
            let now = now_secs();
            let (sid, created) = match session_id {
                Some(id) => {
                    // Сессия могла исчезнуть (другая копия vault) — тогда честно создаём новую.
                    let exists: Option<i64> = tx
                        .query_row("SELECT id FROM chat_sessions WHERE id=?1", [id], |r| {
                            r.get(0)
                        })
                        .optional()?;
                    match exists {
                        Some(id) => (id, false),
                        None => (insert_session(tx, &q, now)?, true),
                    }
                }
                None => (insert_session(tx, &q, now)?, true),
            };
            tx.execute(
                "INSERT INTO chat_messages(session_id, role, content, sources_json, created_at) \
                 VALUES(?1, 'user', ?2, NULL, ?3)",
                params![sid, q, now],
            )?;
            tx.execute(
                "INSERT INTO chat_messages(session_id, role, content, sources_json, created_at) \
                 VALUES(?1, 'assistant', ?2, ?3, ?4)",
                params![sid, a, sources_json, now],
            )?;
            tx.execute(
                "UPDATE chat_sessions SET updated_at=?2 WHERE id=?1",
                params![sid, now],
            )?;
            Ok((sid, created))
        })
        .await
}

fn insert_session(tx: &rusqlite::Transaction, question: &str, now: i64) -> rusqlite::Result<i64> {
    let placeholder: String = question.chars().take(TITLE_PLACEHOLDER_CHARS).collect();
    tx.execute(
        "INSERT INTO chat_sessions(title, created_at, updated_at) VALUES(?1, ?2, ?2)",
        params![placeholder, now],
    )?;
    Ok(tx.last_insert_rowid())
}

/// Обновляет заголовок (генерация мелкой моделью после первого обмена).
pub async fn set_title(writer: &WriteActor, id: i64, title: &str) -> DbResult<()> {
    let title = title.to_string();
    writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE chat_sessions SET title=?2 WHERE id=?1",
                params![id, title],
            )?;
            Ok(())
        })
        .await
}

/// Список сессий, свежие сверху.
pub async fn list_sessions(reader: &ReadPool) -> DbResult<Vec<ChatSession>> {
    reader
        .query(|c| {
            let mut stmt = c.prepare(
                "SELECT id, title, created_at, updated_at FROM chat_sessions \
                 ORDER BY updated_at DESC, id DESC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(ChatSession {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    created_at: r.get(2)?,
                    updated_at: r.get(3)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
}

/// Сообщения сессии в хронологическом порядке.
pub async fn session_messages(reader: &ReadPool, id: i64) -> DbResult<Vec<StoredMessage>> {
    reader
        .query(move |c| {
            let mut stmt = c.prepare(
                "SELECT role, content, sources_json, created_at FROM chat_messages \
                 WHERE session_id=?1 ORDER BY id",
            )?;
            let rows = stmt.query_map([id], |r| {
                Ok(StoredMessage {
                    role: r.get(0)?,
                    content: r.get(1)?,
                    sources_json: r.get(2)?,
                    created_at: r.get(3)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
}

/// Markdown-экспорт сессии («Сохранить в заметки»): фронтматтер с датой + Q/A-секции.
pub async fn session_markdown(reader: &ReadPool, id: i64) -> DbResult<Option<(String, String)>> {
    let session: Option<ChatSession> = reader
        .query(move |c| {
            c.query_row(
                "SELECT id, title, created_at, updated_at FROM chat_sessions WHERE id=?1",
                [id],
                |r| {
                    Ok(ChatSession {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        created_at: r.get(2)?,
                        updated_at: r.get(3)?,
                    })
                },
            )
            .optional()
        })
        .await?;
    let Some(session) = session else {
        return Ok(None);
    };
    let messages = session_messages(reader, id).await?;
    let mut md = format!("# {}\n\n", session.title);
    for m in &messages {
        match m.role.as_str() {
            "user" => md.push_str(&format!("## 🧑 Вопрос\n\n{}\n\n", m.content)),
            _ => md.push_str(&format!("## 🤖 Ответ\n\n{}\n\n", m.content)),
        }
    }
    Ok(Some((session.title, md)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use tempfile::TempDir;

    async fn open() -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join(".nexus/nexus.db"))
            .await
            .unwrap();
        (dir, db)
    }

    /// Первый обмен создаёт сессию с плейсхолдером из вопроса; второй — дописывает в неё же;
    /// список свежие-сверху; сообщения в хронологии с источниками.
    #[tokio::test]
    async fn log_list_and_load_roundtrip() {
        let (_d, db) = open().await;
        let (sid, created) = log_exchange(
            db.writer(),
            None,
            "Как настроить SearXNG?",
            "Вот так…",
            Some(r#"[{"path":"a.md"}]"#.into()),
        )
        .await
        .unwrap();
        assert!(created, "первый обмен создал сессию");

        let (sid2, created2) =
            log_exchange(db.writer(), Some(sid), "А подробнее?", "Подробнее…", None)
                .await
                .unwrap();
        assert_eq!(sid2, sid, "второй обмен — в ту же сессию");
        assert!(!created2);

        let sessions = list_sessions(db.reader()).await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].title.starts_with("Как настроить"));

        let msgs = session_messages(db.reader(), sid).await.unwrap();
        assert_eq!(msgs.len(), 4, "2 обмена = 4 сообщения");
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].role, "assistant");
        assert!(msgs[1].sources_json.as_deref().unwrap().contains("a.md"));
    }

    /// set_title обновляет заголовок (генерация мелкой моделью); markdown-экспорт собирает Q/A.
    #[tokio::test]
    async fn title_and_markdown_export() {
        let (_d, db) = open().await;
        let (sid, _) = log_exchange(db.writer(), None, "вопрос про граф", "ответ про граф", None)
            .await
            .unwrap();
        set_title(db.writer(), sid, "Граф связей").await.unwrap();

        let (title, md) = session_markdown(db.reader(), sid).await.unwrap().unwrap();
        assert_eq!(title, "Граф связей");
        assert!(md.starts_with("# Граф связей"));
        assert!(md.contains("## 🧑 Вопрос\n\nвопрос про граф"));
        assert!(md.contains("## 🤖 Ответ\n\nответ про граф"));

        assert!(
            session_markdown(db.reader(), 999).await.unwrap().is_none(),
            "несуществующая сессия — None"
        );
    }

    /// Протухший session_id (сессии нет в БД) → честно создаётся новая, обмен не теряется.
    #[tokio::test]
    async fn stale_session_id_creates_new() {
        let (_d, db) = open().await;
        let (sid, created) = log_exchange(db.writer(), Some(777), "вопрос", "ответ", None)
            .await
            .unwrap();
        assert!(created);
        assert_ne!(sid, 777);
        assert_eq!(session_messages(db.reader(), sid).await.unwrap().len(), 2);
    }
}
