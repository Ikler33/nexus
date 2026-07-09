//! Управление установленными плагинами (включён/выключен) — персист в `settings` (как
//! `episodic.enabled`). Дефолт ВКЛЮЧЁН: строки нет → плагин активен (обратная совместимость с уже
//! установленными). Ключ — `plugins.<dir>.enabled` ("1"/"0"). Удаление плагина — на стороне команды
//! (move_to_trash + [`clear_settings`]).

use std::collections::HashSet;

use super::broker::PluginAuditRecord;
use crate::db::{DbResult, ReadPool, WriteActor};

/// Потолок числа строк durable-audit, возвращаемых за один запрос UI (защита от нагрузки при
/// раздутом журнале; append-only растёт, но панель показывает лишь недавнее). Команда принимает
/// свой `limit`, но не выше этого потолка.
pub const AUDIT_MAX_LIMIT: usize = 500;

/// Последние `limit` durable-записей брокер-audit (`plugin_audit`) — обратно-хронологически (свежие
/// первыми), для UI «Журнал доступа» (PLUG-1). `limit` зажимается в `1..=AUDIT_MAX_LIMIT`. Читает из
/// БД (не in-memory Vec брокера): durable-история переживает рестарт.
pub async fn recent_audit(reader: &ReadPool, limit: usize) -> DbResult<Vec<PluginAuditRecord>> {
    let limit = limit.clamp(1, AUDIT_MAX_LIMIT) as i64;
    reader
        .query(move |c| {
            let mut stmt = c.prepare(
                "SELECT id, plugin_id, method, target, allowed, denied_reason, created_at \
                 FROM plugin_audit ORDER BY id DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map([limit], |r| {
                Ok(PluginAuditRecord {
                    id: r.get(0)?,
                    plugin_id: r.get(1)?,
                    method: r.get(2)?,
                    target: r.get(3)?,
                    allowed: r.get::<_, i64>(4)? != 0,
                    denied_reason: r.get(5)?,
                    created_at: r.get(6)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
}

/// Ключ настройки «включён» для плагина в каталоге `dir`.
fn enabled_key(dir: &str) -> String {
    format!("plugins.{dir}.enabled")
}

/// Персист тоггла «включён» плагина. "1"/"0", upsert (как `episode::set_enabled`).
pub async fn set_enabled(writer: &WriteActor, dir: &str, on: bool) -> DbResult<()> {
    crate::db::settings::set(writer, &enabled_key(dir), if on { "1" } else { "0" }).await
}

/// Множество ВЫКЛЮЧЕННЫХ плагинов (каталоги, у которых `plugins.<dir>.enabled='0'`). Дефолт —
/// включён (ключа нет → не в множестве). Для обогащения `list_plugins`.
pub async fn disabled_dirs(reader: &ReadPool) -> DbResult<HashSet<String>> {
    reader
        .query(|c| {
            let mut stmt = c.prepare(
                "SELECT key FROM settings WHERE key LIKE 'plugins.%.enabled' AND value='0'",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            let mut set = HashSet::new();
            for k in rows {
                let k = k?;
                if let Some(dir) = k
                    .strip_prefix("plugins.")
                    .and_then(|s| s.strip_suffix(".enabled"))
                {
                    set.insert(dir.to_string());
                }
            }
            Ok(set)
        })
        .await
}

/// Включён ли плагин `dir` (дефолт да — ключа нет / значение не "0"). Гард для `plugin_open_session`.
pub async fn is_enabled(reader: &ReadPool, dir: &str) -> DbResult<bool> {
    let v = crate::db::settings::get(reader, &enabled_key(dir)).await?;
    Ok(v.as_deref() != Some("0"))
}

/// Удаляет настройки плагина (при remove): переустановка стартует «чистой» (включена по дефолту).
pub async fn clear_settings(writer: &WriteActor, dir: &str) -> DbResult<()> {
    crate::db::settings::delete(writer, &enabled_key(dir)).await
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

    #[tokio::test]
    async fn enabled_defaults_true_and_persists_toggle() {
        let (_d, db) = db().await;
        // Дефолт — включён (строки нет).
        assert!(is_enabled(db.reader(), "demo").await.unwrap());
        assert!(disabled_dirs(db.reader()).await.unwrap().is_empty());

        // Выключаем → is_enabled=false, в множестве disabled.
        set_enabled(db.writer(), "demo", false).await.unwrap();
        assert!(!is_enabled(db.reader(), "demo").await.unwrap());
        assert_eq!(
            disabled_dirs(db.reader()).await.unwrap(),
            HashSet::from(["demo".to_string()])
        );

        // Включаем обратно → снова true, не в множестве.
        set_enabled(db.writer(), "demo", true).await.unwrap();
        assert!(is_enabled(db.reader(), "demo").await.unwrap());
        assert!(disabled_dirs(db.reader()).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn clear_settings_resets_to_default() {
        let (_d, db) = db().await;
        set_enabled(db.writer(), "demo", false).await.unwrap();
        assert!(!is_enabled(db.reader(), "demo").await.unwrap());
        clear_settings(db.writer(), "demo").await.unwrap();
        // После очистки — дефолт (включён).
        assert!(is_enabled(db.reader(), "demo").await.unwrap());
    }
}
