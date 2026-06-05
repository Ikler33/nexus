//! «Дайджест изменений» (#35) — первый LLM-kind планировщика (ADR-007, slice 4). Периодически суммирует
//! заметки, изменённые за окно (по умолчанию сутки), через chat-провайдер; результат — таблица `digests`.
//! Регистрируется ТОЛЬКО при сконфигурированном chat (иначе kind отсутствует — джоба не зависнет в dead).
//! backpressure чата (S5: приоритет интерактивного чата над дайджест-джобой) — следующий под-срез.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::ai::{ChatMessage, ChatProvider};
use crate::db::{DbResult, ReadPool, WriteActor};
use crate::scheduler::{now_secs, Job, JobHandler};

/// kind «digest» (ключ реестра обработчиков планировщика).
pub const KIND_DIGEST: &str = "digest";
/// Окно «недавних» изменений (сек).
const WINDOW_SECS: i64 = 24 * 3600;
/// Не раздувать промпт: максимум заметок и длина сниппета на заметку.
const MAX_NOTES: usize = 40;
const SNIPPET_CHARS: usize = 200;

/// Сгенерированный дайджест (для UI / истории).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Digest {
    pub created_at: i64,
    pub since: i64,
    pub content: String,
    pub note_count: i64,
}

/// Заметки, изменённые после `since` (`updated_at`), + сниппет первого чанка. Лимит `MAX_NOTES`.
async fn recent_notes(
    reader: &ReadPool,
    since: i64,
) -> DbResult<Vec<(String, Option<String>, String)>> {
    reader
        .query(move |c| {
            let mut stmt = c.prepare(
                "SELECT f.path, f.title, \
                 COALESCE((SELECT ch.content FROM chunks ch WHERE ch.file_id=f.id ORDER BY ch.chunk_index LIMIT 1), '') \
                 FROM files f \
                 WHERE f.is_deleted=0 AND f.updated_at>=?1 \
                 ORDER BY f.updated_at DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(rusqlite::params![since, MAX_NOTES as i64], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
}

/// Промпт суммаризации из списка недавних заметок (имя + короткий сниппет).
fn build_prompt(notes: &[(String, Option<String>, String)]) -> Vec<ChatMessage> {
    let mut body = String::from("Заметки, изменённые недавно:\n\n");
    for (path, title, snippet) in notes {
        let name = title.clone().unwrap_or_else(|| path.clone());
        let snip: String = snippet
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(SNIPPET_CHARS)
            .collect();
        body.push_str(&format!("- {name}: {snip}\n"));
    }
    body.push_str(
        "\nСделай краткий дайджест (3–6 пунктов): над чем шла работа, что изменилось. По-русски, по делу, без воды.",
    );
    vec![
        ChatMessage::system(
            "Ты делаешь краткие дайджесты недавних изменений в личной базе заметок.",
        ),
        ChatMessage::user(body),
    ]
}

/// Сохраняет дайджест в БД.
async fn store(writer: &WriteActor, d: Digest) -> DbResult<()> {
    writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO digests(created_at,since,content,note_count) VALUES(?1,?2,?3,?4)",
                rusqlite::params![d.created_at, d.since, d.content, d.note_count],
            )?;
            Ok(())
        })
        .await
}

/// Последний дайджест (для UI). `None`, если ещё не генерировался.
pub async fn latest(reader: &ReadPool) -> DbResult<Option<Digest>> {
    reader
        .query(move |c| {
            c.query_row(
                "SELECT created_at,since,content,note_count FROM digests ORDER BY created_at DESC LIMIT 1",
                [],
                |r| {
                    Ok(Digest {
                        created_at: r.get(0)?,
                        since: r.get(1)?,
                        content: r.get(2)?,
                        note_count: r.get(3)?,
                    })
                },
            )
            .optional()
        })
        .await
}

/// Нужно ли генерировать (нет дайджеста за последнее окно) — для on-open run-if-overdue (S2).
pub async fn should_generate(reader: &ReadPool) -> DbResult<bool> {
    let cutoff = now_secs() - WINDOW_SECS;
    reader
        .query(move |c| {
            let recent: i64 = c.query_row(
                "SELECT count(*) FROM digests WHERE created_at>=?1",
                [cutoff],
                |r| r.get(0),
            )?;
            Ok(recent == 0)
        })
        .await
}

/// Обработчик kind «digest»: собрать недавние заметки → LLM-суммаризация → сохранить. Держит свои
/// зависимости (reader/chat/writer). Если изменений нет — успех без записи (нечего суммировать).
pub struct DigestHandler {
    reader: ReadPool,
    chat: Arc<dyn ChatProvider>,
    writer: WriteActor,
}

impl DigestHandler {
    pub fn new(reader: ReadPool, chat: Arc<dyn ChatProvider>, writer: WriteActor) -> Self {
        Self {
            reader,
            chat,
            writer,
        }
    }
}

#[async_trait]
impl JobHandler for DigestHandler {
    async fn handle(&self, _job: &Job) -> Result<(), String> {
        let since = now_secs() - WINDOW_SECS;
        let notes = recent_notes(&self.reader, since)
            .await
            .map_err(|e| e.to_string())?;
        if notes.is_empty() {
            return Ok(()); // нечего суммировать — дайджест не пишем
        }
        let messages = build_prompt(&notes);
        let mut sink = |_t: String| {}; // не-стрим: берём полный текст из результата
        let cancel = Arc::new(AtomicBool::new(false));
        let content = self
            .chat
            .stream_chat(&messages, &mut sink, &cancel)
            .await
            .map_err(|e| e.to_string())?;
        store(
            &self.writer,
            Digest {
                created_at: now_secs(),
                since,
                content,
                note_count: notes.len() as i64,
            },
        )
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{AiResult, EmbeddingProvider, MockEmbedder};
    use crate::db::Database;
    use crate::indexer::Indexer;
    use crate::vector::VectorIndex;
    use std::fs;
    use tempfile::TempDir;

    /// Фейковый chat: возвращает фиксированный текст (без сети) — для офлайн-теста.
    struct FakeChat;
    #[async_trait]
    impl ChatProvider for FakeChat {
        async fn stream_chat(
            &self,
            _m: &[ChatMessage],
            on_token: &mut (dyn FnMut(String) + Send),
            _c: &Arc<AtomicBool>,
        ) -> AiResult<String> {
            on_token("дайджест".into());
            Ok("дайджест: всё ок".into())
        }
        fn model_id(&self) -> &str {
            "fake"
        }
    }

    fn dummy_job() -> Job {
        Job {
            id: 1,
            kind: KIND_DIGEST.into(),
            payload: String::new(),
            state: "running".into(),
            run_at: 0,
            attempts: 0,
            max_attempts: 2,
            last_error: None,
        }
    }

    async fn db_with_note(body: &str) -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let db = Database::open(root.join(".nexus/nexus.db")).await.unwrap();
        let vectors =
            Arc::new(VectorIndex::open(root.join(".nexus").join("vectors.usearch"), 16).unwrap());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder { dim: 16 });
        let idx = Indexer::with_rag(&db, root.clone(), embedder, vectors, true);
        fs::write(root.join("a.md"), body).unwrap();
        idx.index_file("a.md").await.unwrap();
        (dir, db)
    }

    #[tokio::test]
    async fn summarizes_recent_notes_and_stores() {
        let (_d, db) = db_with_note("# A\n\nважные изменения в проекте\n").await;
        assert!(
            should_generate(db.reader()).await.unwrap(),
            "дайджеста ещё нет"
        );

        let h = DigestHandler::new(db.reader().clone(), Arc::new(FakeChat), db.writer().clone());
        h.handle(&dummy_job()).await.unwrap();

        let d = latest(db.reader())
            .await
            .unwrap()
            .expect("дайджест сохранён");
        assert_eq!(d.content, "дайджест: всё ок");
        assert!(d.note_count >= 1, "вошла хотя бы одна заметка");
        assert!(
            !should_generate(db.reader()).await.unwrap(),
            "после генерации — не overdue"
        );
    }

    #[tokio::test]
    async fn no_recent_notes_no_digest() {
        // Пустой vault (нет заметок) → handle успешен, но дайджест НЕ пишется (нечего суммировать).
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join(".nexus/nexus.db"))
            .await
            .unwrap();
        let h = DigestHandler::new(db.reader().clone(), Arc::new(FakeChat), db.writer().clone());
        h.handle(&dummy_job()).await.unwrap();
        assert!(
            latest(db.reader()).await.unwrap().is_none(),
            "нет недавних заметок → дайджест не создан"
        );
    }
}
