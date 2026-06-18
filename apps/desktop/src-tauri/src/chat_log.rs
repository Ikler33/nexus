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
) -> DbResult<LoggedExchange> {
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
            let user_msg_id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO chat_messages(session_id, role, content, sources_json, created_at) \
                 VALUES(?1, 'assistant', ?2, ?3, ?4)",
                params![sid, a, sources_json, now],
            )?;
            let assistant_msg_id = tx.last_insert_rowid();
            tx.execute(
                "UPDATE chat_sessions SET updated_at=?2 WHERE id=?1",
                params![sid, now],
            )?;
            Ok(LoggedExchange {
                session_id: sid,
                created,
                user_msg_id,
                assistant_msg_id,
            })
        })
        .await
}

/// Удаляет последний обмен сессии (последние user+assistant) — для регенерации ответа (P6-RGN):
/// перед повторным прогоном того же вопроса убираем прошлую пару, чтобы история не двоилась.
/// Возвращает id удалённых сообщений (вызывающий чистит их из `chat_vectors`). Если хвост сессии
/// НЕ пара (user, assistant) — ничего не трогаем (пустой Vec).
pub async fn delete_last_exchange(writer: &WriteActor, session_id: i64) -> DbResult<Vec<i64>> {
    writer
        .transaction(move |tx| {
            // Последние 2 сообщения сессии (id DESC): ожидаем [assistant, user].
            let mut stmt = tx.prepare(
                "SELECT id, role FROM chat_messages WHERE session_id=?1 ORDER BY id DESC LIMIT 2",
            )?;
            let rows: Vec<(i64, String)> = stmt
                .query_map([session_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(stmt);
            if rows.len() == 2 && rows[0].1 == "assistant" && rows[1].1 == "user" {
                tx.execute(
                    "DELETE FROM chat_messages WHERE id IN (?1, ?2)",
                    params![rows[0].0, rows[1].0],
                )?;
                Ok(vec![rows[0].0, rows[1].0])
            } else {
                Ok(Vec::new())
            }
        })
        .await
}

/// Итог записи обмена: id сессии (+ создана ли новая) и id двух сообщений — последние нужны
/// RAG-индексу переписки (N4): вызывающий эмбеддит их и кладёт в `chat_vectors`.
#[derive(Debug, Clone, Copy)]
pub struct LoggedExchange {
    pub session_id: i64,
    pub created: bool,
    pub user_msg_id: i64,
    pub assistant_msg_id: i64,
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

/// Сообщение чата (id+текст) для индексации в `chat_vectors` (RAG переписки, N4).
#[derive(Debug, Clone)]
pub struct IndexableMessage {
    pub id: i64,
    pub content: String,
}

/// Сообщения, ещё НЕ попавшие в `chat_vectors` (по их id) — для бэкфилла на старте vault (сессии,
/// записанные до N4, не имеют векторов). `indexed` — множество уже проиндексированных id.
pub async fn messages_missing_vectors(
    reader: &ReadPool,
    indexed: std::collections::HashSet<i64>,
) -> DbResult<Vec<IndexableMessage>> {
    reader
        .query(move |c| {
            let mut stmt =
                c.prepare("SELECT id, content FROM chat_messages WHERE length(content) > 0")?;
            let rows = stmt.query_map([], |r| {
                Ok(IndexableMessage {
                    id: r.get(0)?,
                    content: r.get(1)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                let m = row?;
                if !indexed.contains(&m.id) {
                    out.push(m);
                }
            }
            Ok(out)
        })
        .await
}

/// Найденный фрагмент переписки (RAG памяти, N4): сообщение + его сессия. `snippet` — обрезанный
/// текст сообщения; UI помечает источник «из прошлых разговоров» и по клику грузит сессию.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryHit {
    pub session_id: i64,
    pub session_title: String,
    pub role: String,
    pub snippet: String,
    pub score: f32,
}

/// Резолвит id сообщений (в порядке релевантности) в `MemoryHit` с заголовком сессии, ИСКЛЮЧАЯ
/// текущую сессию (свои же реплики не подмешиваем), сессии из `exclude_sessions` (EP-2: дедуп между
/// каналами — если разговор уже всплыл ЭПИЗОДОМ, его сырые реплики не дублируем) и дедуплицируя по
/// сессии (один лучший фрагмент на разговор). `snippet_chars` — длина выжимки.
pub async fn resolve_memory_hits(
    reader: &ReadPool,
    ranked: Vec<(i64, f32)>,
    exclude_session: Option<i64>,
    exclude_sessions: std::collections::HashSet<i64>,
    snippet_chars: usize,
) -> DbResult<Vec<MemoryHit>> {
    reader
        .query(move |c| {
            let mut out: Vec<MemoryHit> = Vec::new();
            let mut seen_sessions = std::collections::HashSet::new();
            for (id, score) in ranked {
                let row = c
                    .query_row(
                        "SELECT m.session_id, s.title, m.role, m.content                          FROM chat_messages m JOIN chat_sessions s ON s.id = m.session_id                          WHERE m.id = ?1",
                        [id],
                        |r| {
                            Ok((
                                r.get::<_, i64>(0)?,
                                r.get::<_, String>(1)?,
                                r.get::<_, String>(2)?,
                                r.get::<_, String>(3)?,
                            ))
                        },
                    )
                    .optional()?;
                let Some((sid, title, role, content)) = row else {
                    continue;
                };
                if exclude_session == Some(sid)
                    || exclude_sessions.contains(&sid)
                    || !seen_sessions.insert(sid)
                {
                    continue;
                }
                let snippet: String = content
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(snippet_chars)
                    .collect();
                out.push(MemoryHit {
                    session_id: sid,
                    session_title: title,
                    role,
                    snippet,
                    score, // MEM-6: реальная similarity из ANN (раньше хардкод 0.0 → фронт показывал 0%)
                });
            }
            Ok(out)
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

/// Поиск по памяти переписки (N4): эмбеддит запрос, ищет в `chat_vectors` (ключи = id сообщений),
/// резолвит топ-`k` в `MemoryHit` (исключая текущую сессию + `exclude_sessions`, дедуп по сессии).
/// Параллельный канал — заметочный RAG не трогаем. `None`-эмбеддер/индекс → пусто.
// Все 8 параметров осмысленны (источник/индекс/эмбеддер/запрос/k/исключения/длина) — bundling в struct
// читаемости не добавит; EP-2 добавил `exclude_sessions` (дедуп с эпизодами).
#[allow(clippy::too_many_arguments)]
pub async fn search_memory(
    reader: &ReadPool,
    vectors: &crate::vector::VectorIndex,
    embedder: &dyn crate::ai::EmbeddingProvider,
    query: &str,
    k: usize,
    exclude_session: Option<i64>,
    exclude_sessions: std::collections::HashSet<i64>,
    snippet_chars: usize,
) -> DbResult<Vec<MemoryHit>> {
    if query.trim().is_empty() || k == 0 || vectors.is_empty() {
        return Ok(Vec::new());
    }
    let qvec = embedder
        .embed_query(query)
        .await
        .map_err(|e| crate::db::DbError::External(e.to_string()))?;
    // Берём с запасом — дедуп по сессии и исключение текущей/эпизодных могут отсеять часть.
    let hits = vectors
        .search(&qvec, (k * 4).max(8))
        .map_err(|e| crate::db::DbError::External(e.to_string()))?;
    let ranked: Vec<(i64, f32)> = hits
        .into_iter()
        .map(|h| (h.chunk_id as i64, h.score))
        .collect();
    let mut out = resolve_memory_hits(
        reader,
        ranked,
        exclude_session,
        exclude_sessions,
        snippet_chars,
    )
    .await?;
    out.truncate(k);
    Ok(out)
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
        let ex = log_exchange(
            db.writer(),
            None,
            "Как настроить SearXNG?",
            "Вот так…",
            Some(r#"[{"path":"a.md"}]"#.into()),
        )
        .await
        .unwrap();
        let sid = ex.session_id;
        assert!(ex.created, "первый обмен создал сессию");
        assert!(
            ex.assistant_msg_id > ex.user_msg_id,
            "id сообщений возвращены"
        );

        let ex2 = log_exchange(db.writer(), Some(sid), "А подробнее?", "Подробнее…", None)
            .await
            .unwrap();
        assert_eq!(ex2.session_id, sid, "второй обмен — в ту же сессию");
        assert!(!ex2.created);

        let sessions = list_sessions(db.reader()).await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].title.starts_with("Как настроить"));

        let msgs = session_messages(db.reader(), sid).await.unwrap();
        assert_eq!(msgs.len(), 4, "2 обмена = 4 сообщения");
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].role, "assistant");
        assert!(msgs[1].sources_json.as_deref().unwrap().contains("a.md"));
    }

    /// P6-RGN: удаление последнего обмена убирает ровно последнюю пару (user+assistant) и
    /// возвращает их id; на сессии без пары — no-op (пустой Vec).
    #[tokio::test]
    async fn delete_last_exchange_removes_trailing_pair() {
        let (_d, db) = open().await;
        let ex1 = log_exchange(db.writer(), None, "Q1", "A1", None)
            .await
            .unwrap();
        let sid = ex1.session_id;
        let ex2 = log_exchange(db.writer(), Some(sid), "Q2", "A2", None)
            .await
            .unwrap();
        assert_eq!(session_messages(db.reader(), sid).await.unwrap().len(), 4);

        let removed = delete_last_exchange(db.writer(), sid).await.unwrap();
        assert_eq!(removed.len(), 2, "удалены 2 сообщения последнего обмена");
        assert!(removed.contains(&ex2.user_msg_id) && removed.contains(&ex2.assistant_msg_id));

        let msgs = session_messages(db.reader(), sid).await.unwrap();
        assert_eq!(msgs.len(), 2, "остался первый обмен");
        assert_eq!(msgs[0].content, "Q1");
        assert_eq!(msgs[1].content, "A1");

        // Несуществующая сессия — no-op без паники.
        assert!(delete_last_exchange(db.writer(), 99999)
            .await
            .unwrap()
            .is_empty());
    }

    /// set_title обновляет заголовок (генерация мелкой моделью); markdown-экспорт собирает Q/A.
    #[tokio::test]
    async fn title_and_markdown_export() {
        let (_d, db) = open().await;
        let sid = log_exchange(db.writer(), None, "вопрос про граф", "ответ про граф", None)
            .await
            .unwrap()
            .session_id;
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
        let ex = log_exchange(db.writer(), Some(777), "вопрос", "ответ", None)
            .await
            .unwrap();
        assert!(ex.created);
        assert_ne!(ex.session_id, 777);
        let sid = ex.session_id;
        assert_eq!(session_messages(db.reader(), sid).await.unwrap().len(), 2);
    }

    /// N4: индексируем сообщения двух сессий в chat_vectors (ключ = id сообщения), search_memory
    /// находит релевантную сессию по запросу, ИСКЛЮЧАЕТ текущую и дедуплицирует по сессии.
    #[tokio::test]
    async fn search_memory_finds_and_excludes_session() {
        use crate::ai::{EmbeddingProvider, MockEmbedder};
        use crate::vector::VectorIndex;

        let (_d, db) = open().await;
        let dir = TempDir::new().unwrap();
        let vectors = VectorIndex::open(dir.path().join("cv.usearch"), 16).unwrap();
        let emb = MockEmbedder { dim: 16 };

        // Сессия A — про SearXNG; сессия B — про граф.
        let a = log_exchange(
            db.writer(),
            None,
            "как настроить SearXNG",
            "ставь docker",
            None,
        )
        .await
        .unwrap();
        let b = log_exchange(
            db.writer(),
            None,
            "что такое граф связей",
            "беклинки заметок",
            None,
        )
        .await
        .unwrap();
        // Индексируем все 4 сообщения по их id реальными текстами.
        for (id, text) in [
            (a.user_msg_id, "как настроить SearXNG"),
            (a.assistant_msg_id, "ставь docker"),
            (b.user_msg_id, "что такое граф связей"),
            (b.assistant_msg_id, "беклинки заметок"),
        ] {
            let v = emb.embed_documents(&[text]).await.unwrap();
            vectors.upsert(id as u64, &v[0]).unwrap();
        }

        // Запрос про SearXNG → находит сессию A, не B.
        let hits = search_memory(
            db.reader(),
            &vectors,
            &emb,
            "настройка SearXNG",
            3,
            None,
            std::collections::HashSet::new(),
            80,
        )
        .await
        .unwrap();
        assert!(!hits.is_empty(), "память нашла релевантную сессию");
        assert_eq!(
            hits[0].session_id, a.session_id,
            "ближайшая — сессия про SearXNG"
        );
        // MEM-6: score — реальная similarity из ANN, а не хардкод 0.0 (фронт показывал 0%).
        assert!(hits[0].score > 0.0, "score проброшен из векторного поиска");

        // Исключение текущей сессии: если мы В сессии A, её реплики не подмешиваются.
        let excl = search_memory(
            db.reader(),
            &vectors,
            &emb,
            "настройка SearXNG",
            3,
            Some(a.session_id),
            std::collections::HashSet::new(),
            80,
        )
        .await
        .unwrap();
        assert!(
            excl.iter().all(|h| h.session_id != a.session_id),
            "текущая сессия исключена из памяти"
        );

        // EP-2: дедуп между каналами — сессия B исключена через exclude_sessions (она «уже эпизод»).
        let mut excl_set = std::collections::HashSet::new();
        excl_set.insert(b.session_id);
        let dd = search_memory(
            db.reader(),
            &vectors,
            &emb,
            "граф связей",
            3,
            None,
            excl_set,
            80,
        )
        .await
        .unwrap();
        assert!(
            dd.iter().all(|h| h.session_id != b.session_id),
            "сессия из exclude_sessions (всплывшая эпизодом) не дублируется сырыми репликами"
        );
    }
}
