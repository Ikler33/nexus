use rusqlite::Connection;

use super::error::{DbError, DbResult};

/// Одна миграция схемы. `version` совпадает с целевым `PRAGMA user_version`.
struct Migration {
    version: u32,
    name: &'static str,
    sql: &'static str,
    /// Миграция инвалидирует производный FTS5-индекс (смена схемы/конфига `fts_chunks`) → раннер после
    /// её SQL **пересобирает `fts_chunks` из `chunks`** (external-content rebuild), пользователю НЕ нужно
    /// удалять `.nexus`. Чистый SQL (content-таблица `chunks` цела). usearch (смена размерности эмбеддинга)
    /// пересобирается индексатором (reconcile на открытии — нужен embedder), см. `docs/dev/db.md`.
    /// ADR-007/§5.1: примитив-резюмируемости для будущих схемо-миграций (#14 re-chunk, jobs и т.п.).
    rebuild_fts: bool,
}

/// Упорядоченный список миграций. Версия = индекс в эволюции схемы.
///
/// FTS5 (`fts_chunks`) и usearch нельзя `ALTER`; при инвалидации производных — `rebuild_fts`-хук (FTS из
/// `chunks`) + reconcile usearch на открытии. Здесь — ядро Ф0/Ф1 (см. `00x_*.sql`).
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial",
        sql: include_str!("migrations/001_initial.sql"),
        rebuild_fts: false,
    },
    Migration {
        version: 2,
        name: "chunks_fts",
        sql: include_str!("migrations/002_chunks_fts.sql"),
        rebuild_fts: false, // создаёт FTS — пересобирать нечего
    },
    Migration {
        version: 3,
        name: "frontmatter_fields",
        sql: include_str!("migrations/003_frontmatter_fields.sql"),
        rebuild_fts: false,
    },
    Migration {
        version: 4,
        name: "jobs",
        sql: include_str!("migrations/004_jobs.sql"),
        rebuild_fts: false, // новая таблица, производных не инвалидирует
    },
    Migration {
        version: 5,
        name: "digests",
        sql: include_str!("migrations/005_digests.sql"),
        rebuild_fts: false,
    },
    Migration {
        version: 6,
        name: "contradictions",
        sql: include_str!("migrations/006_contradictions.sql"),
        rebuild_fts: false,
    },
    Migration {
        version: 7,
        name: "contradiction_cache",
        sql: include_str!("migrations/007_contradiction_cache.sql"),
        rebuild_fts: false,
    },
];

/// Версия схемы, на которую рассчитан этот билд (максимальная из [`MIGRATIONS`]).
pub fn latest_version() -> u32 {
    MIGRATIONS.iter().map(|m| m.version).max().unwrap_or(0)
}

/// Текущая версия схемы в БД (`PRAGMA user_version`).
pub(crate) fn user_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
}

/// Применяет недостающие миграции по возрастанию версии.
///
/// Версия схемы хранится в `PRAGMA user_version` (а не в `settings('schema.version')`,
/// как в раннем черновике §5.1): user_version транзакционен, не требует chicken-egg с
/// таблицей `settings` и не гонится. Каждая миграция выполняется в своей транзакции и
/// тем же коммитом поднимает `user_version` → процесс идемпотентен и резюмируем после
/// краха (см. `docs/dev/db.md`).
pub(crate) fn apply(conn: &mut Connection) -> DbResult<()> {
    let current = user_version(conn)?;
    for m in MIGRATIONS {
        if i64::from(m.version) <= current {
            continue;
        }
        let wrap = |source| DbError::Migration {
            version: m.version,
            source,
        };
        let tx = conn.transaction().map_err(wrap)?;
        tx.execute_batch(m.sql).map_err(wrap)?;
        // Пост-хук пересборки производного FTS-индекса из chunks (резюмируемость: без ручного сноса .nexus).
        if m.rebuild_fts {
            rebuild_fts(&tx).map_err(wrap)?;
        }
        // user_version нельзя биндить плейсхолдером; значение — из нашего кода, не из ввода.
        tx.pragma_update(None, "user_version", m.version)
            .map_err(wrap)?;
        tx.commit().map_err(wrap)?;
        tracing::info!(version = m.version, name = m.name, "applied db migration");
    }
    Ok(())
}

/// Пересобирает external-content FTS5-индекс `fts_chunks` из content-таблицы `chunks` (встроенная
/// команда FTS5 `'rebuild'`). Применяется после миграции, инвалидирующей FTS (`rebuild_fts: true`), и
/// как точечный ремонт рассинхрона. `chunks` не трогается — переразбор файлов НЕ нужен.
pub(crate) fn rebuild_fts(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("INSERT INTO fts_chunks(fts_chunks) VALUES('rebuild');")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `rebuild_fts` восстанавливает external-content FTS5 из `chunks` после рассинхрона (delete-all) —
    /// без переразбора файлов. Это и есть примитив резюмируемости для будущих FTS-схемо-миграций (#13).
    #[test]
    fn rebuild_fts_restores_index_from_chunks() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn).unwrap(); // 001..003

        // Тест про FTS, не про FK — отключаем FK, чтобы не заводить files-строку; триггер наполнит FTS.
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        conn.execute(
            "INSERT INTO chunks(file_id,chunk_index,content,char_start,char_end,token_count) \
             VALUES(1,0,'alpha beta gamma',0,16,3)",
            [],
        )
        .unwrap();
        let hits = |c: &Connection| -> i64 {
            c.query_row(
                "SELECT count(*) FROM fts_chunks WHERE fts_chunks MATCH 'alpha'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(hits(&conn), 1, "триггер наполнил FTS");

        // Рассинхрон: чистим FTS-индекс (chunks целы).
        conn.execute_batch("INSERT INTO fts_chunks(fts_chunks) VALUES('delete-all');")
            .unwrap();
        assert_eq!(hits(&conn), 0, "после delete-all индекс пуст");

        rebuild_fts(&conn).unwrap();
        assert_eq!(hits(&conn), 1, "rebuild восстановил индекс из chunks");
    }

    /// Идемпотентность раннера: повторный `apply` не двигает `user_version` и доводит до `latest`.
    #[test]
    fn apply_is_idempotent_to_latest() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn).unwrap();
        let v = user_version(&conn).unwrap();
        assert_eq!(v, i64::from(latest_version()));
        apply(&mut conn).unwrap();
        assert_eq!(user_version(&conn).unwrap(), v, "повторный apply — no-op");
    }
}
