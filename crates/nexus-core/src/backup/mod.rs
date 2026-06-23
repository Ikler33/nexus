//! Backup/Restore (#59, порт духа odysseus): портабельный экспорт durable «второго мозга» в один
//! JSON и импорт обратно с дедупом (повторный импорт НЕ плодит дубли). Бэкап покрывает то, что иначе
//! не восстановить из самого vault:
//! - `memory_facts` — явные факты агента (MEM);
//! - `chat_sessions` + `chat_messages` — вся переписка (владелец: храним всё);
//! - `chat_episodes` — LLM-саммари сессий (EP, дорого регенерить);
//! - `agent_skill_usage` — телеметрия/lifecycle ТОЛЬКО агент-созданных скиллов (vendor/user — чужие).
//!
//! НЕ покрывается (намеренно): заметки `.md` (живут в vault → git-sync), vector-индексы usearch и
//! FTS (пересобираются reconcile/rebuild на открытии), audit-журналы (производные). См.
//! `~/Documents/Claude/nexus-agent-design/plans/backup-restore-plan.json`.
//!
//! Импорт АТОМАРЕН (одна транзакция writer): либо весь envelope лёг, либо rollback. Дедуп по
//! естественным ключам каждой таблицы; id сессий РЕМАПЯТСЯ (autoincrement → старые id не переносимы),
//! сообщения/эпизоды привязываются к новым/существующим сессиям через old→new карту.

use std::collections::HashMap;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::{DbResult, ReadPool, WriteActor};
use crate::scheduler::now_secs;

/// Магический маркер формата — импорт отвергает чужой JSON, не совпавший по `format`.
pub const BACKUP_FORMAT: &str = "nexus-backup-v1";

/// Потолок размера JSON-бэкапа (anti-DoS): `serde_json` материализует всё в RAM, а импорт держит
/// одну транзакцию на ЕДИНСТВЕННОМ потоке-писателе — гигантский/битый файл иначе застопорил бы все
/// записи приложения. 512 МиБ с запасом покрывает реальную историю «второго мозга».
pub const MAX_BACKUP_BYTES: usize = 512 * 1024 * 1024;

/// Потолок суммарного числа строк во всех таблицах бэкапа (anti-DoS второй эшелон, после байт-лимита).
pub const MAX_IMPORT_ROWS: usize = 5_000_000;

/// Один экспортированный факт памяти (MEM). Дедуп на импорте — по `text` (UNIQUE idx 017).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactBackup {
    pub text: String,
    pub pinned: bool,
    pub source: String,
    pub created_at: i64,
    pub used_at: i64,
}

/// Сессия чата. `id` — ОРИГИНАЛЬНЫЙ (для ремапа ссылок сообщений/эпизодов); на импорте не переносится.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionBackup {
    pub id: i64,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Сообщение чата. `session_id` — ОРИГИНАЛЬНЫЙ (ремапится на импорте). Дедуп — по
/// (session_id, role, content, created_at) точно (идемпотентность повторного импорта).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageBackup {
    pub session_id: i64,
    pub role: String,
    pub content: String,
    pub sources_json: Option<String>,
    pub created_at: i64,
}

/// Эпизод (EP, саммари сессии). `session_id` — ОРИГИНАЛЬНЫЙ (ремапится). `last_msg_id` — водяной знак
/// по СТАРЫМ id сообщений → на импорте пересчитывается под новые (консистентность с импортированной
/// перепиской; staleness иначе провоцировал бы лишний регенерейт rollup-джобой).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeBackup {
    pub session_id: i64,
    pub summary: String,
    pub topics: Option<String>,
    pub msg_count: i64,
    pub started_at: i64,
    pub ended_at: i64,
    pub model: Option<String>,
    pub embed_model: Option<String>,
    pub generated_at: i64,
    pub dismissed: bool,
}

/// Телеметрия/lifecycle агент-созданного скилла. Дедуп — по `skill_name` (PK): skip если есть (счётчики
/// НЕ мержим — мердж двусмыслен). Экспортируются ТОЛЬКО `created_by='agent'` (curation-провенанс).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillUsageBackup {
    pub skill_name: String,
    pub use_count: i64,
    pub view_count: i64,
    pub save_count: i64,
    pub patch_count: i64,
    pub last_used_at: Option<i64>,
    pub last_viewed_at: Option<i64>,
    pub last_saved_at: Option<i64>,
    pub last_patched_at: Option<i64>,
    pub created_at: i64,
    pub created_by: Option<String>,
    pub state: String,
    pub pinned: bool,
    pub archived_at: Option<i64>,
}

/// Конверт бэкапа: версионируемый, самодостаточный. `schema_version` штампуется на экспорте
/// (`PRAGMA user_version`); на импорте — мягкая сверка (warn при mismatch, append-only таблицы
/// прямо/обратно совместимы по этим колонкам), НЕ fail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupEnvelope {
    pub format: String,
    pub schema_version: i64,
    pub app_version: String,
    pub exported_at: i64,
    pub memory_facts: Vec<FactBackup>,
    pub chat_sessions: Vec<SessionBackup>,
    pub chat_messages: Vec<MessageBackup>,
    pub chat_episodes: Vec<EpisodeBackup>,
    pub agent_skill_usage: Vec<SkillUsageBackup>,
}

/// Отчёт импорта: что добавлено, что пропущено (дедуп). `sessions_reused` — сессия с тем же
/// (title, created_at) уже была → переиспользована (её id в ремап-карте), не создана заново.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub facts_added: u64,
    pub facts_skipped: u64,
    pub sessions_added: u64,
    pub sessions_reused: u64,
    pub messages_added: u64,
    pub messages_skipped: u64,
    pub episodes_added: u64,
    pub episodes_skipped: u64,
    pub skills_added: u64,
    pub skills_skipped: u64,
    /// Сообщения, чья сессия отсутствует в бэкапе (битый/ручной envelope) — отброшены. Отдельно от
    /// `messages_skipped` (легитимный дедуп), чтобы не маскировать потерю данных под «дедуп».
    pub messages_orphaned: u64,
    /// Эпизоды, чья сессия отсутствует в бэкапе — отброшены (как `messages_orphaned`).
    pub episodes_orphaned: u64,
    /// `schema_version` бэкапа СТАРШЕ текущей БД (`<`) — импорт выполнен, но это сигнал для UI
    /// (часть колонок старого формата могла быть уже, всё совместимо для append-only таблиц). Импорт
    /// БОЛЕЕ НОВОГО бэкапа (`>`) НЕ выполняется — см. [`ImportError::SchemaTooNew`].
    pub schema_version_mismatch: bool,
}

/// Собирает полный бэкап durable-состояния из БД (read-only). `app_version` — `CARGO_PKG_VERSION`
/// вызывающего (desktop/agentd). `exported_at` = текущее unix-время.
pub async fn export_backup(reader: &ReadPool, app_version: &str) -> DbResult<BackupEnvelope> {
    let app_version = app_version.to_string();
    let exported_at = now_secs();
    reader
        .query(move |c| {
            let schema_version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;

            let memory_facts = c
                .prepare(
                    "SELECT text, pinned, source, created_at, used_at FROM memory_facts ORDER BY id",
                )?
                .query_map([], |r| {
                    Ok(FactBackup {
                        text: r.get(0)?,
                        pinned: r.get(1)?,
                        source: r.get(2)?,
                        created_at: r.get(3)?,
                        used_at: r.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let chat_sessions = c
                .prepare(
                    "SELECT id, title, created_at, updated_at FROM chat_sessions ORDER BY id",
                )?
                .query_map([], |r| {
                    Ok(SessionBackup {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        created_at: r.get(2)?,
                        updated_at: r.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let chat_messages = c
                .prepare(
                    "SELECT session_id, role, content, sources_json, created_at \
                     FROM chat_messages ORDER BY id",
                )?
                .query_map([], |r| {
                    Ok(MessageBackup {
                        session_id: r.get(0)?,
                        role: r.get(1)?,
                        content: r.get(2)?,
                        sources_json: r.get(3)?,
                        created_at: r.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let chat_episodes = c
                .prepare(
                    "SELECT session_id, summary, topics, msg_count, started_at, ended_at, \
                            model, embed_model, generated_at, dismissed \
                     FROM chat_episodes ORDER BY id",
                )?
                .query_map([], |r| {
                    Ok(EpisodeBackup {
                        session_id: r.get(0)?,
                        summary: r.get(1)?,
                        topics: r.get(2)?,
                        msg_count: r.get(3)?,
                        started_at: r.get(4)?,
                        ended_at: r.get(5)?,
                        model: r.get(6)?,
                        embed_model: r.get(7)?,
                        generated_at: r.get(8)?,
                        dismissed: r.get(9)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            // Только агент-созданные скиллы (curation-провенанс): vendor/user-скиллы — чужие, не наши.
            let agent_skill_usage = c
                .prepare(
                    "SELECT skill_name, use_count, view_count, save_count, patch_count, \
                            last_used_at, last_viewed_at, last_saved_at, last_patched_at, \
                            created_at, created_by, state, pinned, archived_at \
                     FROM agent_skill_usage WHERE created_by = 'agent' ORDER BY skill_name",
                )?
                .query_map([], |r| {
                    Ok(SkillUsageBackup {
                        skill_name: r.get(0)?,
                        use_count: r.get(1)?,
                        view_count: r.get(2)?,
                        save_count: r.get(3)?,
                        patch_count: r.get(4)?,
                        last_used_at: r.get(5)?,
                        last_viewed_at: r.get(6)?,
                        last_saved_at: r.get(7)?,
                        last_patched_at: r.get(8)?,
                        created_at: r.get(9)?,
                        created_by: r.get(10)?,
                        state: r.get(11)?,
                        pinned: r.get(12)?,
                        archived_at: r.get(13)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            Ok(BackupEnvelope {
                format: BACKUP_FORMAT.to_string(),
                schema_version,
                app_version,
                exported_at,
                memory_facts,
                chat_sessions,
                chat_messages,
                chat_episodes,
                agent_skill_usage,
            })
        })
        .await
}

/// Причина отказа импорта (проверяется ДО любой записи — БД не трогается).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    /// Чужой/битый JSON (не совпал магический `format`).
    BadFormat,
    /// Бэкап снят на БОЛЕЕ НОВОЙ схеме, чем у этого приложения. Импорт запрещён: новый формат мог нести
    /// колонки, которых тут нет (тихая потеря на round-trip) или NOT NULL без дефолта (hard-fail). Это
    /// единственное направление несовместимости; старый бэкап в новое приложение (`<`) — ОК (warn-флаг).
    SchemaTooNew { backup: i64, current: i64 },
    /// Слишком много строк (anti-DoS, > [`MAX_IMPORT_ROWS`]).
    TooLarge { rows: usize, max: usize },
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::BadFormat => {
                write!(f, "неизвестный формат бэкапа (ожидался {BACKUP_FORMAT})")
            }
            ImportError::SchemaTooNew { backup, current } => write!(
                f,
                "бэкап новее этой версии приложения (схема бэкапа {backup} > текущей {current}); \
                 обновите приложение перед импортом"
            ),
            ImportError::TooLarge { rows, max } => {
                write!(f, "бэкап слишком большой ({rows} строк > предела {max})")
            }
        }
    }
}
impl std::error::Error for ImportError {}

/// Суммарное число строк во всех таблицах конверта (для anti-DoS порога).
fn total_rows(e: &BackupEnvelope) -> usize {
    e.memory_facts.len()
        + e.chat_sessions.len()
        + e.chat_messages.len()
        + e.chat_episodes.len()
        + e.agent_skill_usage.len()
}

/// Импортирует бэкап в БД с дедупом, АТОМАРНО (одна транзакция). Возвращает отчёт. Все предохранители
/// (формат / размер / версия-схемы-новее) проверяются ДО записи — при отказе БД не трогается.
pub async fn import_backup(
    writer: &WriteActor,
    envelope: BackupEnvelope,
) -> DbResult<Result<ImportReport, ImportError>> {
    if envelope.format != BACKUP_FORMAT {
        return Ok(Err(ImportError::BadFormat));
    }
    let rows = total_rows(&envelope);
    if rows > MAX_IMPORT_ROWS {
        return Ok(Err(ImportError::TooLarge {
            rows,
            max: MAX_IMPORT_ROWS,
        }));
    }
    writer
        .transaction(move |tx| {
            let cur_version: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
            // Бэкап НОВЕЕ приложения → отказ ДО записи (пустая транзакция, БД не изменена). Новый
            // формат мог нести неизвестные колонки (тихая потеря) или NOT NULL без дефолта (hard-fail).
            if envelope.schema_version > cur_version {
                return Ok(Err(ImportError::SchemaTooNew {
                    backup: envelope.schema_version,
                    current: cur_version,
                }));
            }
            // Только `<` (старый бэкап в новое приложение): append-only таблицы совместимы → warn-флаг.
            let mut rep = ImportReport {
                schema_version_mismatch: cur_version != envelope.schema_version,
                ..Default::default()
            };

            // Факты: дедуп по UNIQUE(text). INSERT OR IGNORE → changes()==1 добавлен, ==0 пропущен.
            for f in &envelope.memory_facts {
                let n = tx.execute(
                    "INSERT OR IGNORE INTO memory_facts(text, pinned, source, created_at, used_at) \
                     VALUES(?1, ?2, ?3, ?4, ?5)",
                    params![f.text, f.pinned, f.source, f.created_at, f.used_at],
                )?;
                if n == 1 {
                    rep.facts_added += 1;
                } else {
                    rep.facts_skipped += 1;
                }
            }

            // Сессии: дедуп по (title, created_at) ТОЛЬКО против строк, существовавших ДО этого импорта.
            // `inserted` хранит id, вставленные этим прогоном: если SELECT находит такую (две сессии в
            // ОДНОМ бэкапе с одинаковым (title, created_at) разного id), это НЕ дубль, а коллизия ключа
            // → вставляем новую строку, иначе их сообщения/эпизоды слились бы (а UNIQUE(session_id)
            // эпизода молча терял бы второй). Идемпотентность сохранена: при повторном импорте строки
            // прошлого прогона НЕ в `inserted` → переиспользуются. NB: межбэкаповая коллизия (две РАЗНЫЕ
            // беседы с совпавшими title+секунда из разных БД) — редкий принятый кейс: сольются, но без
            // дублей контента (дедуп сообщений) и без падения.
            // Карта old_id → effective_id для ремапа сообщений/эпизодов.
            let mut sid_map: HashMap<i64, i64> = HashMap::new();
            let mut inserted: std::collections::HashSet<i64> = std::collections::HashSet::new();
            for s in &envelope.chat_sessions {
                let existing: Option<i64> = tx
                    .query_row(
                        "SELECT id FROM chat_sessions WHERE title = ?1 AND created_at = ?2",
                        params![s.title, s.created_at],
                        |r| r.get(0),
                    )
                    .optional()?;
                let eff = match existing {
                    Some(id) if !inserted.contains(&id) => {
                        rep.sessions_reused += 1;
                        id
                    }
                    _ => {
                        tx.execute(
                            "INSERT INTO chat_sessions(title, created_at, updated_at) \
                             VALUES(?1, ?2, ?3)",
                            params![s.title, s.created_at, s.updated_at],
                        )?;
                        rep.sessions_added += 1;
                        let id = tx.last_insert_rowid();
                        inserted.insert(id);
                        id
                    }
                };
                sid_map.insert(s.id, eff);
            }

            // Сообщения: ремап session_id; дедуп по (session_id, role, content, created_at) точно.
            for m in &envelope.chat_messages {
                let Some(&sid) = sid_map.get(&m.session_id) else {
                    // Осиротевшее сообщение (сессии нет в бэкапе) — пропускаем, не роняем импорт.
                    rep.messages_orphaned += 1;
                    continue;
                };
                let exists: Option<i64> = tx
                    .query_row(
                        "SELECT 1 FROM chat_messages \
                         WHERE session_id = ?1 AND role = ?2 AND content = ?3 AND created_at = ?4",
                        params![sid, m.role, m.content, m.created_at],
                        |r| r.get(0),
                    )
                    .optional()?;
                if exists.is_some() {
                    rep.messages_skipped += 1;
                    continue;
                }
                tx.execute(
                    "INSERT INTO chat_messages(session_id, role, content, sources_json, created_at) \
                     VALUES(?1, ?2, ?3, ?4, ?5)",
                    params![sid, m.role, m.content, m.sources_json, m.created_at],
                )?;
                rep.messages_added += 1;
            }

            // Эпизоды: ремап session_id; UNIQUE(session_id) → INSERT OR IGNORE. last_msg_id
            // пересчитывается под новые id сообщений (консистентный водяной знак); 0 если сообщений нет.
            for e in &envelope.chat_episodes {
                let Some(&sid) = sid_map.get(&e.session_id) else {
                    rep.episodes_orphaned += 1;
                    continue;
                };
                let last_msg_id: i64 = tx.query_row(
                    "SELECT COALESCE(MAX(id), 0) FROM chat_messages WHERE session_id = ?1",
                    params![sid],
                    |r| r.get(0),
                )?;
                let n = tx.execute(
                    "INSERT OR IGNORE INTO chat_episodes(session_id, summary, topics, msg_count, \
                        last_msg_id, started_at, ended_at, model, embed_model, generated_at, dismissed) \
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        sid,
                        e.summary,
                        e.topics,
                        e.msg_count,
                        last_msg_id,
                        e.started_at,
                        e.ended_at,
                        e.model,
                        e.embed_model,
                        e.generated_at,
                        e.dismissed,
                    ],
                )?;
                if n == 1 {
                    rep.episodes_added += 1;
                } else {
                    rep.episodes_skipped += 1;
                }
            }

            // Скиллы: дедуп по PK(skill_name). Только created_by='agent' (defense-in-depth поверх
            // фильтра экспорта). INSERT OR IGNORE — счётчики не мержим (skip если есть).
            for k in &envelope.agent_skill_usage {
                if k.created_by.as_deref() != Some("agent") {
                    rep.skills_skipped += 1;
                    continue;
                }
                let n = tx.execute(
                    "INSERT OR IGNORE INTO agent_skill_usage(skill_name, use_count, view_count, \
                        save_count, patch_count, last_used_at, last_viewed_at, last_saved_at, \
                        last_patched_at, created_at, created_by, state, pinned, archived_at) \
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                    params![
                        k.skill_name,
                        k.use_count,
                        k.view_count,
                        k.save_count,
                        k.patch_count,
                        k.last_used_at,
                        k.last_viewed_at,
                        k.last_saved_at,
                        k.last_patched_at,
                        k.created_at,
                        k.created_by,
                        k.state,
                        k.pinned,
                        k.archived_at,
                    ],
                )?;
                if n == 1 {
                    rep.skills_added += 1;
                } else {
                    rep.skills_skipped += 1;
                }
            }

            Ok(Ok(rep))
        })
        .await
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

    /// Засевает БД минимальным durable-состоянием всех пяти таблиц.
    async fn seed(db: &Database) {
        db.writer()
            .call(|c| {
                c.execute(
                    "INSERT INTO memory_facts(text, pinned, source, created_at, used_at) \
                     VALUES('владелец предпочитает Rust', 1, 'explicit', 100, 0)",
                    [],
                )?;
                c.execute(
                    "INSERT INTO chat_sessions(title, created_at, updated_at) \
                     VALUES('Про SearXNG', 200, 250)",
                    [],
                )?;
                let sid = c.last_insert_rowid();
                c.execute(
                    "INSERT INTO chat_messages(session_id, role, content, sources_json, created_at) \
                     VALUES(?1, 'user', 'как поднять docker?', NULL, 210)",
                    [sid],
                )?;
                c.execute(
                    "INSERT INTO chat_messages(session_id, role, content, sources_json, created_at) \
                     VALUES(?1, 'assistant', 'podman run …', '[{\"path\":\"a.md\"}]', 220)",
                    [sid],
                )?;
                c.execute(
                    "INSERT INTO chat_episodes(session_id, summary, msg_count, last_msg_id, \
                        started_at, ended_at, generated_at) \
                     VALUES(?1, 'Диалог про docker/SearXNG.', 2, 999, 210, 220, 230)",
                    [sid],
                )?;
                // Один agent-скилл (бэкапим) + один vendor (НЕ бэкапим).
                c.execute(
                    "INSERT INTO agent_skill_usage(skill_name, use_count, created_at, created_by, state) \
                     VALUES('debug-flaky-tests', 5, 50, 'agent', 'active')",
                    [],
                )?;
                c.execute(
                    "INSERT INTO agent_skill_usage(skill_name, use_count, created_at, created_by, state) \
                     VALUES('kepano-writing', 3, 50, 'vendor', 'active')",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
    }

    /// Экспорт собирает все пять таблиц; vendor-скилл исключён; конверт штампован форматом/версией.
    #[tokio::test]
    async fn export_gathers_all_durable_state() {
        let (_d, db) = open().await;
        seed(&db).await;
        let env = export_backup(db.reader(), "9.9.9").await.unwrap();
        assert_eq!(env.format, BACKUP_FORMAT);
        assert_eq!(env.app_version, "9.9.9");
        assert!(env.schema_version >= 25, "штамп версии схемы");
        assert_eq!(env.memory_facts.len(), 1);
        assert!(env.memory_facts[0].pinned, "pinned как bool");
        assert_eq!(env.chat_sessions.len(), 1);
        assert_eq!(env.chat_messages.len(), 2);
        assert_eq!(env.chat_episodes.len(), 1);
        assert_eq!(
            env.agent_skill_usage.len(),
            1,
            "только created_by='agent' (vendor исключён)"
        );
        assert_eq!(env.agent_skill_usage[0].skill_name, "debug-flaky-tests");
    }

    /// Round-trip: экспорт → импорт в ПУСТУЮ БД восстанавливает всё; сообщения/эпизод привязаны к
    /// НОВОЙ сессии (ремап id); last_msg_id эпизода пересчитан под импортированные сообщения.
    #[tokio::test]
    async fn roundtrip_into_empty_db_restores_all() {
        let (_s, src) = open().await;
        seed(&src).await;
        let env = export_backup(src.reader(), "1.0.0").await.unwrap();
        // Перегон через JSON (доказывает сериализуемость конверта).
        let json = serde_json::to_vec(&env).unwrap();
        let env2: BackupEnvelope = serde_json::from_slice(&json).unwrap();

        let (_d, dst) = open().await;
        let rep = import_backup(dst.writer(), env2).await.unwrap().unwrap();
        assert_eq!(rep.facts_added, 1);
        assert_eq!(rep.sessions_added, 1);
        assert_eq!(rep.sessions_reused, 0);
        assert_eq!(rep.messages_added, 2);
        assert_eq!(rep.episodes_added, 1);
        assert_eq!(rep.skills_added, 1);

        // Эпизод привязан к новой сессии и его last_msg_id = max импортированного сообщения (не 999).
        let (new_sid, ep_sid, ep_last, max_msg): (i64, i64, i64, i64) = dst
            .reader()
            .query(|c| {
                let s: i64 =
                    c.query_row("SELECT id FROM chat_sessions LIMIT 1", [], |r| r.get(0))?;
                let (esid, elast): (i64, i64) = c.query_row(
                    "SELECT session_id, last_msg_id FROM chat_episodes LIMIT 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                let mx: i64 = c.query_row(
                    "SELECT MAX(id) FROM chat_messages WHERE session_id = ?1",
                    [s],
                    |r| r.get(0),
                )?;
                Ok((s, esid, elast, mx))
            })
            .await
            .unwrap();
        assert_eq!(ep_sid, new_sid, "эпизод привязан к НОВОЙ сессии (ремап id)");
        assert_eq!(
            ep_last, max_msg,
            "last_msg_id пересчитан под новые сообщения"
        );
        assert_ne!(ep_last, 999, "старый водяной знак не перенесён");
    }

    /// Идемпотентность: повторный импорт того же конверта в ту же БД ничего не добавляет (всё дедупнуто).
    #[tokio::test]
    async fn reimport_is_idempotent() {
        let (_s, src) = open().await;
        seed(&src).await;
        let env = export_backup(src.reader(), "1.0.0").await.unwrap();

        let (_d, dst) = open().await;
        let r1 = import_backup(dst.writer(), env.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            r1.facts_added + r1.sessions_added + r1.messages_added,
            1 + 1 + 2
        );

        let r2 = import_backup(dst.writer(), env).await.unwrap().unwrap();
        assert_eq!(r2.facts_added, 0, "факт дедупнут по text");
        assert_eq!(r2.facts_skipped, 1);
        assert_eq!(
            r2.sessions_added, 0,
            "сессия дедупнута по (title, created_at)"
        );
        assert_eq!(r2.sessions_reused, 1);
        assert_eq!(r2.messages_added, 0, "сообщения дедупнуты");
        assert_eq!(r2.messages_skipped, 2);
        assert_eq!(
            r2.episodes_added, 0,
            "эпизод дедупнут по UNIQUE(session_id)"
        );
        assert_eq!(r2.episodes_skipped, 1);
        assert_eq!(r2.skills_added, 0);
        assert_eq!(r2.skills_skipped, 1);
    }

    /// Чужой/битый формат отвергается ДО записи (ничего не импортируется).
    #[tokio::test]
    async fn bad_format_rejected_without_writes() {
        let (_d, db) = open().await;
        let bad = BackupEnvelope {
            format: "something-else".to_string(),
            schema_version: 25,
            app_version: "x".into(),
            exported_at: 0,
            memory_facts: vec![FactBackup {
                text: "не должен записаться".into(),
                pinned: false,
                source: "explicit".into(),
                created_at: 1,
                used_at: 0,
            }],
            chat_sessions: vec![],
            chat_messages: vec![],
            chat_episodes: vec![],
            agent_skill_usage: vec![],
        };
        assert!(import_backup(db.writer(), bad).await.unwrap().is_err());
        let n: i64 = db
            .reader()
            .query(|c| c.query_row("SELECT COUNT(*) FROM memory_facts", [], |r| r.get(0)))
            .await
            .unwrap();
        assert_eq!(n, 0, "битый формат не записал ничего");
    }

    /// Две сессии в ОДНОМ бэкапе с совпавшим (title, created_at), но разными id → импорт НЕ сливает их
    /// (коллизия ключа внутри прогона), оба эпизода сохранены (иначе UNIQUE(session_id) потерял бы один).
    #[tokio::test]
    async fn within_import_session_collision_does_not_merge() {
        let env = BackupEnvelope {
            format: BACKUP_FORMAT.to_string(),
            schema_version: 25,
            app_version: "x".into(),
            exported_at: 0,
            memory_facts: vec![],
            chat_sessions: vec![
                SessionBackup {
                    id: 1,
                    title: "Дубль".into(),
                    created_at: 100,
                    updated_at: 110,
                },
                SessionBackup {
                    id: 2,
                    title: "Дубль".into(),
                    created_at: 100,
                    updated_at: 120,
                },
            ],
            chat_messages: vec![
                MessageBackup {
                    session_id: 1,
                    role: "user".into(),
                    content: "из сессии 1".into(),
                    sources_json: None,
                    created_at: 101,
                },
                MessageBackup {
                    session_id: 2,
                    role: "user".into(),
                    content: "из сессии 2".into(),
                    sources_json: None,
                    created_at: 102,
                },
            ],
            chat_episodes: vec![
                EpisodeBackup {
                    session_id: 1,
                    summary: "эп1".into(),
                    topics: None,
                    msg_count: 1,
                    started_at: 101,
                    ended_at: 101,
                    model: None,
                    embed_model: None,
                    generated_at: 105,
                    dismissed: false,
                },
                EpisodeBackup {
                    session_id: 2,
                    summary: "эп2".into(),
                    topics: None,
                    msg_count: 1,
                    started_at: 102,
                    ended_at: 102,
                    model: None,
                    embed_model: None,
                    generated_at: 106,
                    dismissed: false,
                },
            ],
            agent_skill_usage: vec![],
        };
        let (_d, db) = open().await;
        let rep = import_backup(db.writer(), env).await.unwrap().unwrap();
        assert_eq!(rep.sessions_added, 2, "коллизия ключа НЕ слила сессии");
        assert_eq!(rep.sessions_reused, 0);
        assert_eq!(rep.messages_added, 2);
        assert_eq!(rep.episodes_added, 2, "оба эпизода сохранены");
        let (sessions, episodes): (i64, i64) = db
            .reader()
            .query(|c| {
                let s: i64 = c.query_row("SELECT COUNT(*) FROM chat_sessions", [], |r| r.get(0))?;
                let e: i64 = c.query_row("SELECT COUNT(*) FROM chat_episodes", [], |r| r.get(0))?;
                Ok((s, e))
            })
            .await
            .unwrap();
        assert_eq!(sessions, 2);
        assert_eq!(episodes, 2);
    }

    /// Бэкап НОВЕЕ текущей схемы → импорт отклонён ДО записи (БД не тронута): новый формат мог нести
    /// неизвестные колонки/NOT NULL → тихая потеря/hard-fail. `<` (старый бэкап) разрешён (warn-флаг).
    #[tokio::test]
    async fn schema_too_new_rejected_without_writes() {
        let (_d, db) = open().await;
        let cur: i64 = db
            .reader()
            .query(|c| c.query_row("PRAGMA user_version", [], |r| r.get(0)))
            .await
            .unwrap();
        let env = BackupEnvelope {
            format: BACKUP_FORMAT.to_string(),
            schema_version: cur + 1,
            app_version: "future".into(),
            exported_at: 0,
            memory_facts: vec![FactBackup {
                text: "из будущего".into(),
                pinned: false,
                source: "explicit".into(),
                created_at: 1,
                used_at: 0,
            }],
            chat_sessions: vec![],
            chat_messages: vec![],
            chat_episodes: vec![],
            agent_skill_usage: vec![],
        };
        let err = import_backup(db.writer(), env).await.unwrap().unwrap_err();
        assert!(matches!(err, ImportError::SchemaTooNew { .. }));
        let n: i64 = db
            .reader()
            .query(|c| c.query_row("SELECT COUNT(*) FROM memory_facts", [], |r| r.get(0)))
            .await
            .unwrap();
        assert_eq!(n, 0, "новее-бэкап ничего не записал");
    }

    /// `total_rows` суммирует все пять таблиц (предохранитель anti-DoS [`MAX_IMPORT_ROWS`]).
    #[test]
    fn total_rows_sums_all_tables() {
        let env = BackupEnvelope {
            format: BACKUP_FORMAT.to_string(),
            schema_version: 25,
            app_version: "x".into(),
            exported_at: 0,
            memory_facts: vec![FactBackup {
                text: "a".into(),
                pinned: false,
                source: "explicit".into(),
                created_at: 1,
                used_at: 0,
            }],
            chat_sessions: vec![SessionBackup {
                id: 1,
                title: "t".into(),
                created_at: 1,
                updated_at: 1,
            }],
            chat_messages: vec![],
            chat_episodes: vec![],
            agent_skill_usage: vec![],
        };
        assert_eq!(total_rows(&env), 2);
    }
}
