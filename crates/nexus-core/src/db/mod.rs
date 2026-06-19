//! БД-слой Nexus — **ADR-003**.
//!
//! - **Writer** ([`WriteActor`]): единственный поток-писатель, синхронные транзакции
//!   rusqlite. Сериализует мутации → нет `SQLITE_BUSY` между писателями (AC-Б7-1),
//!   индексация атомарна на файл (AC-Б7-2).
//! - **Reader** ([`ReadPool`]): пул read-коннектов; WAL допускает параллельное чтение
//!   во время записи; запросы — в `spawn_blocking`.
//! - **Миграции** ([`mod@migrations`]): версионированные SQL, версия в `PRAGMA user_version`
//!   (AC-PR-3).
//!
//! Контракты и инварианты подробно — `docs/dev/db.md`.

mod error;
mod migrations;
mod read_pool;
mod write_actor;

pub use error::{DbError, DbResult};
pub use read_pool::ReadPool;
pub use write_actor::WriteActor;

use std::path::Path;

use rusqlite::Connection;

/// Размер пула read-коннектов. Калибруется под нагрузку позже (Ф3).
const READ_POOL_SIZE: usize = 4;

/// Открытая БД vault: единый писатель + пул читателей. Источник истины метаданных,
/// ссылок/беклинков и тегов (ADR-004). Живёт всё время работы с vault.
pub struct Database {
    writer: WriteActor,
    reader: ReadPool,
}

impl Database {
    /// Открывает/создаёт БД по `path`, включает WAL + pragmas, применяет миграции —
    /// и только потом готова к приёму запросов.
    pub async fn open(path: impl AsRef<Path>) -> DbResult<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Writer: WAL включается писателем один раз; миграции — синхронно до spawn.
        let mut write_conn = Connection::open(&path)?;
        configure_write(&write_conn)?;
        migrations::apply(&mut write_conn)?;
        let writer = WriteActor::spawn(write_conn);

        // Reader-пул: WAL уже персистентен в файле БД.
        let mut conns = Vec::with_capacity(READ_POOL_SIZE);
        for _ in 0..READ_POOL_SIZE {
            let conn = Connection::open(&path)?;
            configure_read(&conn)?;
            conns.push(conn);
        }
        let reader = ReadPool::new(conns);

        tracing::info!(
            schema_version = migrations::latest_version(),
            read_pool = READ_POOL_SIZE,
            "opened vault database"
        );

        Ok(Self { writer, reader })
    }

    /// Единый писатель (ADR-003). Клонируется для передачи в indexer/команды.
    pub fn writer(&self) -> &WriteActor {
        &self.writer
    }

    /// Пул читателей (WAL).
    pub fn reader(&self) -> &ReadPool {
        &self.reader
    }

    /// Текущая версия схемы (`PRAGMA user_version`).
    pub async fn schema_version(&self) -> DbResult<u32> {
        let v = self.reader.query(migrations::user_version).await?;
        Ok(v as u32)
    }
}

/// Pragmas write-коннекта: WAL + внешние ключи + busy-timeout + `synchronous=NORMAL` + перф
/// (`mmap_size` 256MB, `cache_size` 64MB, `temp_store=MEMORY` — кросс-план #6; ускоряет индексацию).
fn configure_write(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;\n\
         PRAGMA foreign_keys=ON;\n\
         PRAGMA busy_timeout=5000;\n\
         PRAGMA synchronous=NORMAL;\n\
         PRAGMA mmap_size=268435456;\n\
         PRAGMA cache_size=-65536;\n\
         PRAGMA temp_store=MEMORY;",
    )
}

/// Pragmas read-коннекта: внешние ключи + busy-timeout + `query_only` (defense-in-depth) + перф
/// (`mmap_size` 256MB, `cache_size` 16MB/конн, `temp_store=MEMORY` — кросс-план #6; ускоряет граф/поиск).
fn configure_read(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;\n\
         PRAGMA busy_timeout=5000;\n\
         PRAGMA query_only=ON;\n\
         PRAGMA mmap_size=268435456;\n\
         PRAGMA cache_size=-16384;\n\
         PRAGMA temp_store=MEMORY;",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Открывает БД во временном vault. Возвращаем `(Database, TempDir)` именно в таком
    /// порядке: при выходе из области сначала закрывается БД, затем удаляется каталог.
    async fn temp_db() -> (Database, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join(".nexus/nexus.db"))
            .await
            .expect("open db");
        (db, dir)
    }

    fn count_tables(conn: &Connection) -> rusqlite::Result<i64> {
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' \
             AND name IN ('files','links','tags','file_tags','aliases','settings','frontmatter_fields')",
            [],
            |r| r.get(0),
        )
    }

    /// AC-PR-3: миграции применяются (версия = latest, таблицы созданы) и идемпотентны
    /// при повторном открытии той же БД.
    #[tokio::test]
    async fn migrations_apply_and_are_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".nexus/nexus.db");

        let db = Database::open(&path).await.unwrap();
        assert_eq!(
            db.schema_version().await.unwrap(),
            migrations::latest_version()
        );
        assert_eq!(db.reader().query(count_tables).await.unwrap(), 7);

        // Повторное открытие той же БД: версия не меняется, ошибок нет.
        drop(db);
        let db2 = Database::open(&path).await.unwrap();
        assert_eq!(
            db2.schema_version().await.unwrap(),
            migrations::latest_version()
        );
        assert_eq!(db2.reader().query(count_tables).await.unwrap(), 7);
    }

    /// AC-Б7-2: транзакция атомарна — ошибка в середине откатывает всё, частичного
    /// состояния не остаётся.
    #[tokio::test]
    async fn transaction_is_atomic_on_error() {
        let (db, _dir) = temp_db().await;

        let res: DbResult<()> = db
            .writer()
            .transaction(|tx| {
                tx.execute(
                    "INSERT INTO files (path,hash,created_at,updated_at,indexed_at,size_bytes) \
                     VALUES ('Note.md','h1',0,0,0,1)",
                    [],
                )?;
                // Нарушаем UNIQUE(path) тем же путём → ошибка внутри транзакции.
                tx.execute(
                    "INSERT INTO files (path,hash,created_at,updated_at,indexed_at,size_bytes) \
                     VALUES ('Note.md','h2',0,0,0,1)",
                    [],
                )?;
                Ok(())
            })
            .await;
        assert!(res.is_err(), "транзакция должна была упасть на UNIQUE");

        let count: i64 = db
            .reader()
            .query(|c| {
                c.query_row("SELECT count(*) FROM files WHERE path='Note.md'", [], |r| {
                    r.get(0)
                })
            })
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "частичное состояние не должно сохраниться (полный rollback)"
        );
    }

    /// AC-Б7-1: множество конкурентных записей через единый write-actor проходят без
    /// `SQLITE_BUSY` и все коммитятся.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_writes_no_busy() {
        let (db, _dir) = temp_db().await;
        const N: usize = 64;

        let mut handles = Vec::with_capacity(N);
        for i in 0..N {
            let writer = db.writer().clone();
            handles.push(tokio::spawn(async move {
                writer
                    .transaction(move |tx| {
                        tx.execute(
                            "INSERT INTO files (path,hash,created_at,updated_at,indexed_at,size_bytes) \
                             VALUES (?1,?2,0,0,0,1)",
                            rusqlite::params![format!("note-{i}.md"), format!("h{i}")],
                        )?;
                        Ok(())
                    })
                    .await
            }));
        }
        for h in handles {
            h.await
                .unwrap()
                .expect("конкурентная запись не должна давать SQLITE_BUSY");
        }

        let count: i64 = db
            .reader()
            .query(|c| c.query_row("SELECT count(*) FROM files", [], |r| r.get(0)))
            .await
            .unwrap();
        assert_eq!(count as usize, N);
    }

    /// Чтения из пула идут конкурентно с записью (WAL) и не упираются в writer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_reads_during_writes() {
        let (db, _dir) = temp_db().await;

        // Фоновая запись.
        let writer = db.writer().clone();
        let write_task = tokio::spawn(async move {
            for i in 0..50 {
                writer
                    .transaction(move |tx| {
                        tx.execute(
                            "INSERT INTO files (path,hash,created_at,updated_at,indexed_at,size_bytes) \
                             VALUES (?1,'h',0,0,0,1)",
                            rusqlite::params![format!("w-{i}.md")],
                        )?;
                        Ok(())
                    })
                    .await
                    .unwrap();
            }
        });

        // Параллельные чтения из нескольких коннектов пула.
        let mut readers = Vec::new();
        for _ in 0..8 {
            let reader = db.reader().clone();
            readers.push(tokio::spawn(async move {
                reader
                    .query(|c| {
                        c.query_row("SELECT count(*) FROM files", [], |r| r.get::<_, i64>(0))
                    })
                    .await
                    .unwrap()
            }));
        }
        for r in readers {
            r.await.unwrap(); // не паникуем — чтение во время записи корректно
        }
        write_task.await.unwrap();

        let total: i64 = db
            .reader()
            .query(|c| c.query_row("SELECT count(*) FROM files", [], |r| r.get(0)))
            .await
            .unwrap();
        assert_eq!(total, 50);
    }

    /// Число чанков, найденных FTS по слову `vector`.
    async fn fts_vector_hits(db: &Database) -> i64 {
        db.reader()
            .query(|c| {
                c.query_row(
                    "SELECT count(*) FROM fts_chunks WHERE fts_chunks MATCH 'vector'",
                    [],
                    |r| r.get(0),
                )
            })
            .await
            .unwrap()
    }

    /// AC-Б8-1/8-2 (схема v2): FTS5 поверх chunks синхронизируется триггерами — текст
    /// находится сразу после вставки и исчезает после удаления чанка (нет «призраков»).
    #[tokio::test]
    async fn fts_chunks_synced_via_triggers() {
        let (db, _dir) = temp_db().await;

        db.writer()
            .call(|c| {
                c.execute(
                    "INSERT INTO files (path,hash,created_at,updated_at,indexed_at,size_bytes) \
                     VALUES ('A.md','h',0,0,0,1)",
                    [],
                )?;
                let fid: i64 =
                    c.query_row("SELECT id FROM files WHERE path='A.md'", [], |r| r.get(0))?;
                c.execute(
                    "INSERT INTO chunks (file_id,chunk_index,content,char_start,char_end,token_count) \
                     VALUES (?1,0,'hello vector search world',0,25,5)",
                    [fid],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(
            fts_vector_hits(&db).await,
            1,
            "FTS находит текст чанка сразу (AC-Б8-1)"
        );

        db.writer()
            .call(|c| c.execute("DELETE FROM chunks", []).map(|_| ()))
            .await
            .unwrap();
        assert_eq!(
            fts_vector_hits(&db).await,
            0,
            "после удаления чанка FTS чист (AC-Б8-2)"
        );
    }
}
