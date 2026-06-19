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
    Migration {
        version: 8,
        name: "home_widgets",
        sql: include_str!("migrations/008_home_widgets.sql"),
        rebuild_fts: false, // новая таблица-кэш, производных не инвалидирует
    },
    Migration {
        version: 9,
        name: "stale_cache",
        sql: include_str!("migrations/009_stale_cache.sql"),
        rebuild_fts: false, // новая таблица-кэш, производных не инвалидирует
    },
    Migration {
        version: 10,
        name: "news",
        sql: include_str!("migrations/010_news.sql"),
        rebuild_fts: false, // новые таблицы ленты (NF-3), производных не инвалидирует
    },
    Migration {
        version: 11,
        name: "news_bodies",
        sql: include_str!("migrations/011_news_bodies.sql"),
        rebuild_fts: false, // колонки-кэш reader'а (NF-6), производных не инвалидирует
    },
    Migration {
        version: 12,
        name: "chat_sessions",
        sql: include_str!("migrations/012_chat_sessions.sql"),
        rebuild_fts: false, // новые таблицы переписки, производных не инвалидирует
    },
    Migration {
        version: 13,
        name: "links_dangling_index",
        sql: include_str!("migrations/013_links_dangling_index.sql"),
        rebuild_fts: false, // только индекс для перф-резолва ссылок, данные не трогает
    },
    Migration {
        version: 14,
        name: "news_comments_url",
        sql: include_str!("migrations/014_news_comments_url.sql"),
        rebuild_fts: false, // nullable-колонка ссылки на HN-обсуждение, производных не трогает
    },
    Migration {
        version: 15,
        name: "edit_events",
        sql: include_str!("migrations/015_edit_events.sql"),
        rebuild_fts: false, // журнал изменений для временной оси, производных индексов не трогает
    },
    Migration {
        version: 16,
        name: "relation_reasons",
        sql: include_str!("migrations/016_relation_reasons.sql"),
        rebuild_fts: false, // новая таблица-кэш LLM-объяснений связей, производных не инвалидирует
    },
    Migration {
        version: 17,
        name: "memory_facts",
        sql: include_str!("migrations/017_memory_facts.sql"),
        rebuild_fts: false, // память агента (MEM): отдельный слой фактов, заметочных производных не трогает
    },
    Migration {
        version: 18,
        name: "memory_fact_events",
        sql: include_str!("migrations/018_memory_fact_events.sql"),
        rebuild_fts: false, // история/supersede фактов памяти (MEM-7), заметочных производных не трогает
    },
    Migration {
        version: 19,
        name: "chat_episodes",
        sql: include_str!("migrations/019_chat_episodes.sql"),
        rebuild_fts: false, // эпизодическая память (EP): саммари сессий, заметочных производных не трогает
    },
    Migration {
        version: 20,
        name: "egress_audit",
        sql: include_str!("migrations/020_egress_audit.sql"),
        rebuild_fts: false, // durable egress-журнал (P0-b): append-only журнал подотчётности, производных не трогает
    },
    Migration {
        version: 21,
        name: "agent_runs",
        sql: include_str!("migrations/021_agent_runs.sql"),
        rebuild_fts: false, // durable запись прогонов агента (AGENT-2): статус-машина прогона, производных не трогает
    },
    Migration {
        version: 22,
        name: "agent_actions",
        sql: include_str!("migrations/022_agent_actions.sql"),
        rebuild_fts: false, // idempotency-ledger актуатора (AGENT-3b): журнал действий, производных не трогает
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
    // Потолок версии: БД, поднятая БОЛЕЕ новым приложением, не должна молча открываться старым —
    // операции по новым полям/таблицам падали бы / портили данные. Явный отказ вместо тихого приёма
    // (находка аудита: downgrade молча «работал»).
    if current > i64::from(latest_version()) {
        return Err(DbError::External(format!(
            "БД схемы v{current} новее приложения (v{}) — обновите приложение, downgrade не поддержан",
            latest_version()
        )));
    }
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

    /// Аудит: БД новее приложения (downgrade) → явный отказ, а не тихий приём (иначе операции по
    /// новым полям/таблицам портили бы данные / падали).
    #[test]
    fn apply_rejects_db_newer_than_app() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn).unwrap();
        // Имитируем БД, поднятую более новым приложением.
        conn.pragma_update(None, "user_version", i64::from(latest_version()) + 1)
            .unwrap();
        assert!(
            apply(&mut conn).is_err(),
            "downgrade (current > latest) должен падать с ошибкой"
        );
    }
}
