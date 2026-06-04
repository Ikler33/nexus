use rusqlite::Connection;

use super::error::{DbError, DbResult};

/// Одна миграция схемы. `version` совпадает с целевым `PRAGMA user_version`.
struct Migration {
    version: u32,
    name: &'static str,
    sql: &'static str,
}

/// Упорядоченный список миграций. Версия = индекс в эволюции схемы.
///
/// FTS5 (`fts_chunks`) и usearch нельзя `ALTER` — они появятся отдельными миграциями
/// в Ф1 (с пересозданием/переиндексацией из `chunks`); chat/link_suggestions — там же
/// по мере надобности. Здесь — ядро для Ф0 (см. `001_initial.sql`).
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial",
        sql: include_str!("migrations/001_initial.sql"),
    },
    Migration {
        version: 2,
        name: "chunks_fts",
        sql: include_str!("migrations/002_chunks_fts.sql"),
    },
    Migration {
        version: 3,
        name: "frontmatter_fields",
        sql: include_str!("migrations/003_frontmatter_fields.sql"),
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
        // user_version нельзя биндить плейсхолдером; значение — из нашего кода, не из ввода.
        tx.pragma_update(None, "user_version", m.version)
            .map_err(wrap)?;
        tx.commit().map_err(wrap)?;
        tracing::info!(version = m.version, name = m.name, "applied db migration");
    }
    Ok(())
}
