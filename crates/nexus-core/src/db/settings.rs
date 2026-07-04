//! `db::settings` — КАНОН доступа к key/value таблице `settings` (R-12 п.2).
//!
//! Единственная точка raw-SQL по таблице `settings` (создана в `001_initial.sql`):
//! - [`get`] — `SELECT value FROM settings WHERE key=?1` (нет строки → `None`);
//! - [`set`] — upsert `INSERT ... ON CONFLICT(key) DO UPDATE SET value=excluded.value`;
//! - [`delete`] — `DELETE FROM settings WHERE key=?1`.
//!
//! Ранее эти три запроса копировались поле-в-поле в `contradictions`/`episode`/`plugin` (аудит
//! «db-settings ×5»). БУЛЕВУ интерпретацию ("1"/"0", дефолт on/off) держит КАЖДЫЙ вызыватель у себя —
//! она РАЗНАЯ (contradictions/episode: дефолт OFF, `== "1"`; plugin: дефолт ON, `!= "0"`), поэтому канон
//! оперирует СТРОКАМИ, не bool. НАМЕРЕННО вне канона: транзакционные записи настройки внутри бо́льшей
//! атомарной транзакции (`vector::reconcile_embedding_model` — свой `&Transaction`-путь, дробить его на
//! отдельные `writer().call` нельзя без потери атомарности) и prefix-scan (`plugin::disabled_dirs` —
//! `LIKE`-скан, иная форма запроса).

use rusqlite::OptionalExtension;

use super::{DbResult, ReadPool, WriteActor};

/// Прочитать значение настройки по ключу. `Ok(None)` — ключа нет.
pub(crate) async fn get(reader: &ReadPool, key: &str) -> DbResult<Option<String>> {
    let key = key.to_string();
    reader
        .query(move |c| {
            c.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
                r.get::<_, String>(0)
            })
            .optional()
        })
        .await
}

/// Записать (upsert) значение настройки по ключу — перезаписывает существующее.
pub(crate) async fn set(writer: &WriteActor, key: &str, value: &str) -> DbResult<()> {
    let (key, value) = (key.to_string(), value.to_string());
    writer
        .call(move |c| {
            c.execute(
                "INSERT INTO settings(key,value) VALUES(?1,?2) \
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                rusqlite::params![key, value],
            )
            .map(|_| ())
        })
        .await
}

/// Удалить настройку по ключу. Идемпотентно: нет строки → 0 удалено, не ошибка.
pub(crate) async fn delete(writer: &WriteActor, key: &str) -> DbResult<()> {
    let key = key.to_string();
    writer
        .call(move |c| {
            c.execute("DELETE FROM settings WHERE key=?1", [key])
                .map(|_| ())
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use tempfile::TempDir;

    async fn db() -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join(".nexus/nexus.db"))
            .await
            .unwrap();
        (dir, db)
    }

    /// Отсутствующий ключ → `None` (дефолт-поведение всех вызывателей).
    #[tokio::test]
    async fn get_absent_is_none() {
        let (_d, db) = db().await;
        assert_eq!(get(db.reader(), "nope").await.unwrap(), None);
    }

    /// set → get round-trip; повторный set по тому же ключу перезаписывает (upsert).
    #[tokio::test]
    async fn set_then_get_roundtrips_and_upserts() {
        let (_d, db) = db().await;
        set(db.writer(), "k", "v1").await.unwrap();
        assert_eq!(get(db.reader(), "k").await.unwrap().as_deref(), Some("v1"));
        set(db.writer(), "k", "v2").await.unwrap();
        assert_eq!(
            get(db.reader(), "k").await.unwrap().as_deref(),
            Some("v2"),
            "upsert перезаписал значение по тому же ключу"
        );
    }

    /// delete убирает ключ; повторное удаление отсутствующего — не ошибка (идемпотентно).
    #[tokio::test]
    async fn delete_removes_key_idempotently() {
        let (_d, db) = db().await;
        set(db.writer(), "k", "1").await.unwrap();
        delete(db.writer(), "k").await.unwrap();
        assert_eq!(get(db.reader(), "k").await.unwrap(), None);
        delete(db.writer(), "k").await.unwrap();
    }
}
