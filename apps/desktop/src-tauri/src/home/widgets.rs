//! HOME H2 — кэш LLM-виджетов + refresh-режимы (фундамент). Виджеты дашборда генерируются ФОНОМ
//! (планировщик ADR-007) и читаются мгновенно из таблицы `home_widgets` — LLM никогда не блокирует
//! загрузку HOME (концепт `PKM_Home_Concepts.md` §«Принципы»). Слой состоит из:
//!
//! - **данные кэша** ([`get`]/[`store_ready`]/[`mark_error`]) — `key → content (+ generated_at,
//!   source_hash, status)`; инвалидация по правкам vault через [`is_stale`] (`max_file_mtime` на момент
//!   генерации против текущего);
//! - **фреймворк** ([`WidgetGenerator`] → [`WidgetHandler`] → [`WidgetSink`]): обобщённый kind
//!   планировщика «сгенерировать → положить в кэш → уведомить фронт (`home:widget-updated`)»;
//! - **реестр** [`WidgetRegistry`] зарегистрированных виджетов — чтобы команда `refresh_widget` ставила
//!   джобу только для известного ключа (а не плодила dead-letter).
//!
//! Конкретные виджеты (Daily brief — H3, Stale radar — H4, Open questions/Context drift — H5)
//! реализуют [`WidgetGenerator`] и регистрируются в `open_vault`; сам фундамент виджетов не содержит.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::db::{DbResult, ReadPool, WriteActor};
use crate::scheduler::{max_file_mtime, now_secs, Job, JobHandler};

/// Префикс kind виджета в очереди планировщика: `home_widget:<key>`. Свой kind на виджет (а не общий
/// payload-ключ) — чтобы per-widget recurring/on-change/дедуп ADR-007 работали как для дайджеста.
const KIND_PREFIX: &str = "home_widget:";

/// Статус кэш-строки: контент валиден.
pub const STATUS_READY: &str = "ready";
/// Статус кэш-строки: последний refresh упал (прежний контент, если был, сохраняется для показа).
pub const STATUS_ERROR: &str = "error";

/// kind планировщика для виджета `key` (`home_widget:<key>`).
pub fn widget_kind(key: &str) -> String {
    format!("{KIND_PREFIX}{key}")
}

/// Кэшированный виджет (для фронта). `content` непрозрачен (текст/JSON — парсит конкретный виджет).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Widget {
    pub key: String,
    pub content: String,
    pub generated_at: i64,
    pub source_hash: i64,
    pub status: String,
    /// vault менялся с момента генерации (текущий `max_file_mtime` > `source_hash`) — кэш устарел.
    /// Считается на чтении: фронт может показать «обновляется…»/бейдж, не дожидаясь refresh.
    pub stale: bool,
}

/// Кэш устарел: vault менялся после генерации (`current_mtime` строго больше `source_hash`).
/// Чистая функция (без часов/БД) → детерминированный юнит-тест.
pub fn is_stale(source_hash: i64, current_mtime: i64) -> bool {
    current_mtime > source_hash
}

/// Кэшированный виджет по ключу (+ вычисленный `stale` против текущего `max_file_mtime`). `None`,
/// если виджет ещё не генерировался.
pub async fn get(reader: &ReadPool, key: &str) -> DbResult<Option<Widget>> {
    let key_owned = key.to_string();
    let row: Option<(String, i64, i64, String)> = reader
        .query(move |c| {
            c.query_row(
                "SELECT content,generated_at,source_hash,status FROM home_widgets WHERE key=?1",
                [key_owned],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
        })
        .await?;
    let Some((content, generated_at, source_hash, status)) = row else {
        return Ok(None);
    };
    let current = max_file_mtime(reader).await?;
    Ok(Some(Widget {
        key: key.to_string(),
        content,
        generated_at,
        source_hash,
        stale: is_stale(source_hash, current),
        status,
    }))
}

/// Кладёт успешно сгенерированный контент в кэш (upsert, `status='ready'`). `source_hash` — снимок
/// `max_file_mtime` на момент генерации (база для последующей инвалидации по правкам).
pub async fn store_ready(
    writer: &WriteActor,
    key: &str,
    content: &str,
    source_hash: i64,
    generated_at: i64,
) -> DbResult<()> {
    let (key, content) = (key.to_string(), content.to_string());
    writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT OR REPLACE INTO home_widgets(key,content,generated_at,source_hash,status) \
                 VALUES(?1,?2,?3,?4,?5)",
                params![key, content, generated_at, source_hash, STATUS_READY],
            )
            .map(|_| ())
        })
        .await
}

/// Помечает кэш-строку виджета как `error` (последний refresh упал), НЕ затирая прежний контент/
/// `generated_at` — UI показывает последнюю удачную версию с бейджем ошибки. Если строки ещё нет
/// (виджет ни разу не генерировался) — no-op. Возвращает число затронутых строк (0 — строки не было).
pub async fn mark_error(writer: &WriteActor, key: &str) -> DbResult<usize> {
    let key = key.to_string();
    writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE home_widgets SET status=?2 WHERE key=?1",
                params![key, STATUS_ERROR],
            )
        })
        .await
}

/// Нужно ли обновить виджет на открытии (on-open run-if-overdue): кэша нет ИЛИ vault менялся с момента
/// генерации (stale). `current_mtime` передаётся явно (caller считает `max_file_mtime` один раз) →
/// детерминированно. Строки со `status='error'` не считаются overdue (не хаммерим ошибку на каждом
/// открытии) — повторную попытку даёт ручной `refresh_widget` или правка vault (меняет mtime → stale).
pub async fn is_overdue(reader: &ReadPool, key: &str, current_mtime: i64) -> DbResult<bool> {
    let key = key.to_string();
    let source_hash: Option<i64> = reader
        .query(move |c| {
            c.query_row(
                "SELECT source_hash FROM home_widgets WHERE key=?1",
                [key],
                |r| r.get(0),
            )
            .optional()
        })
        .await?;
    Ok(match source_hash {
        None => true,
        Some(h) => is_stale(h, current_mtime),
    })
}

// ── Фреймворк: генератор → обработчик → сток событий ────────────────────────────────────────────

/// Источник содержимого виджета: чистая генерация (LLM/динамика) без знания о кэше/событиях. Конкретные
/// виджеты (Daily brief, Stale radar, …) реализуют этот трейт; зависимости (chat/векторы) держат сами.
#[async_trait]
pub trait WidgetGenerator: Send + Sync {
    /// Сгенерировать содержимое (текст/JSON). `Err(msg)` → джоба ретраится/умирает (S7), кэш помечается
    /// `error`, но прежний удачный контент сохраняется.
    async fn generate(&self) -> Result<String, String>;
}

/// Сток уведомлений «виджет обновился» — тонкая обёртка над Tauri-эмиттером (`home:widget-updated`).
/// Вынесен в трейт, чтобы [`WidgetHandler`] оставался юнит-тестируемым без живого `AppHandle`.
pub trait WidgetSink: Send + Sync {
    /// Кэш виджета `key` изменился — фронту пора перечитать его (`tauriApi.home.widget(key)`).
    fn widget_updated(&self, key: &str);
}

/// Обобщённый kind виджета (планировщик ADR-007): снять `source_hash` → сгенерировать → положить в кэш
/// → уведомить фронт. Generic над генератором и стоком. `defer_under_interactive` (S5 backpressure) —
/// `true` для LLM-виджетов: пока пользователь занят чатом/inline, фоновый виджет уступает локальную модель.
pub struct WidgetHandler {
    key: String,
    generator: Arc<dyn WidgetGenerator>,
    sink: Arc<dyn WidgetSink>,
    reader: ReadPool,
    writer: WriteActor,
    defer: bool,
}

impl WidgetHandler {
    pub fn new(
        key: impl Into<String>,
        generator: Arc<dyn WidgetGenerator>,
        sink: Arc<dyn WidgetSink>,
        reader: ReadPool,
        writer: WriteActor,
        defer: bool,
    ) -> Self {
        Self {
            key: key.into(),
            generator,
            sink,
            reader,
            writer,
            defer,
        }
    }
}

#[async_trait]
impl JobHandler for WidgetHandler {
    fn defer_under_interactive(&self) -> bool {
        self.defer
    }

    async fn handle(&self, _job: &Job) -> Result<(), String> {
        // Снимок состояния vault ДО генерации: контент основан на нём, и любая последующая правка
        // корректно пометит виджет stale (даже если она пришла во время генерации).
        let source_hash = max_file_mtime(&self.reader)
            .await
            .map_err(|e| e.to_string())?;
        match self.generator.generate().await {
            Ok(content) => {
                store_ready(&self.writer, &self.key, &content, source_hash, now_secs())
                    .await
                    .map_err(|e| e.to_string())?;
                self.sink.widget_updated(&self.key);
                Ok(())
            }
            Err(e) => {
                // Помечаем кэш ошибкой (прежний контент остаётся); уведомляем фронт только если строка
                // реально была (иначе показывать нечего). Err пробрасываем → планировщик ретраит/умертвляет
                // джобу (видимо, S7).
                let affected = mark_error(&self.writer, &self.key).await.unwrap_or(0);
                if affected > 0 {
                    self.sink.widget_updated(&self.key);
                }
                Err(e)
            }
        }
    }
}

/// Tauri-реализация [`WidgetSink`]: шлёт `home:widget-updated` с ключом виджета (как `vault:changed`).
/// Best-effort (ошибку эмита глотаем — событие не критично для корректности кэша).
pub struct TauriWidgetSink(pub tauri::AppHandle);

impl WidgetSink for TauriWidgetSink {
    fn widget_updated(&self, key: &str) {
        use tauri::Emitter;
        let _ = self.0.emit("home:widget-updated", key);
    }
}

/// Реестр зарегистрированных HOME-виджетов (множество ключей). Команда `refresh_widget` проверяет по
/// нему, что ключ известен, прежде чем ставить джобу — иначе понятная ошибка вместо тихого dead-letter.
/// Наполняется в `open_vault` по мере регистрации виджетов (H3+); сейчас пуст (фундамент).
#[derive(Debug, Default)]
pub struct WidgetRegistry {
    keys: HashSet<String>,
}

impl WidgetRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Зарегистрировать ключ виджета (идемпотентно).
    pub fn register(&mut self, key: &str) {
        self.keys.insert(key.to_string());
    }

    /// Зарегистрирован ли виджет с таким ключом.
    pub fn contains(&self, key: &str) -> bool {
        self.keys.contains(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tempfile::TempDir;

    async fn open_db() -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join(".nexus/nexus.db"))
            .await
            .unwrap();
        (dir, db)
    }

    /// Выставляет `max_file_mtime` vault равным `mtime` (одна не-удалённая заметка с таким `updated_at`).
    async fn set_vault_mtime(db: &Database, mtime: i64) {
        db.writer()
            .call(move |c| {
                c.execute("DELETE FROM files", [])?;
                c.execute(
                    "INSERT INTO files (path,hash,title,created_at,updated_at,indexed_at,size_bytes,word_count) \
                     VALUES ('n.md','h','N',0,?1,0,1,1)",
                    [mtime],
                )?;
                Ok(())
            })
            .await
            .unwrap();
    }

    /// Генератор с фиксированным ответом (Ok или Err) и счётчиком вызовов.
    struct FakeGen {
        result: Result<String, String>,
        calls: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl WidgetGenerator for FakeGen {
        async fn generate(&self) -> Result<String, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.clone()
        }
    }

    /// Запоминающий сток — фиксирует ключи, по которым «виджет обновился».
    #[derive(Default)]
    struct RecordingSink {
        seen: Mutex<Vec<String>>,
    }
    impl WidgetSink for RecordingSink {
        fn widget_updated(&self, key: &str) {
            self.seen.lock().unwrap().push(key.to_string());
        }
    }

    fn dummy_job(kind: &str) -> Job {
        Job {
            id: 1,
            kind: kind.to_string(),
            payload: String::new(),
            state: "running".into(),
            run_at: 0,
            attempts: 0,
            max_attempts: 2,
            last_error: None,
        }
    }

    fn handler(
        db: &Database,
        gen_result: Result<String, String>,
        calls: Arc<AtomicUsize>,
        sink: Arc<RecordingSink>,
    ) -> WidgetHandler {
        WidgetHandler::new(
            "daily_brief",
            Arc::new(FakeGen {
                result: gen_result,
                calls,
            }),
            sink,
            db.reader().clone(),
            db.writer().clone(),
            true,
        )
    }

    #[test]
    fn is_stale_only_when_vault_advanced() {
        assert!(!is_stale(100, 100), "не менялся → свежий");
        assert!(!is_stale(100, 50), "mtime назад (теор.) → свежий");
        assert!(is_stale(100, 101), "vault менялся → устарел");
    }

    /// Генерация → кэш `ready` со снимком mtime → событие; чтение отдаёт контент, не stale.
    #[tokio::test]
    async fn generates_caches_and_notifies() {
        let (_d, db) = open_db().await;
        set_vault_mtime(&db, 500).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let sink = Arc::new(RecordingSink::default());
        let h = handler(
            &db,
            Ok("краткая сводка".into()),
            calls.clone(),
            sink.clone(),
        );

        h.handle(&dummy_job(&widget_kind("daily_brief")))
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1, "генератор вызван");
        let w = get(db.reader(), "daily_brief")
            .await
            .unwrap()
            .expect("виджет в кэше");
        assert_eq!(w.content, "краткая сводка");
        assert_eq!(w.status, STATUS_READY);
        assert_eq!(w.source_hash, 500, "снимок mtime vault");
        assert!(!w.stale, "vault не менялся → не stale");
        assert_eq!(sink.seen.lock().unwrap().as_slice(), ["daily_brief"]);
    }

    /// Правка vault после генерации → `get` отдаёт `stale=true`, `is_overdue` → true (on-open refresh).
    #[tokio::test]
    async fn stale_flips_when_vault_changes() {
        let (_d, db) = open_db().await;
        set_vault_mtime(&db, 500).await;
        let sink = Arc::new(RecordingSink::default());
        handler(&db, Ok("v1".into()), Arc::new(AtomicUsize::new(0)), sink)
            .handle(&dummy_job(&widget_kind("daily_brief")))
            .await
            .unwrap();

        // Свежий кэш — не overdue.
        assert!(!is_overdue(db.reader(), "daily_brief", 500).await.unwrap());

        // Правка vault поднимает mtime → виджет устарел.
        set_vault_mtime(&db, 900).await;
        let w = get(db.reader(), "daily_brief").await.unwrap().unwrap();
        assert!(w.stale, "vault менялся → stale");
        assert!(
            is_overdue(db.reader(), "daily_brief", 900).await.unwrap(),
            "stale → overdue (on-open перегенерит)"
        );
    }

    /// Нет кэша → `get` пусто, `is_overdue` → true (run-if-overdue на первом открытии).
    #[tokio::test]
    async fn absent_widget_is_overdue() {
        let (_d, db) = open_db().await;
        set_vault_mtime(&db, 100).await;
        assert!(get(db.reader(), "stale_radar").await.unwrap().is_none());
        assert!(is_overdue(db.reader(), "stale_radar", 100).await.unwrap());
    }

    /// Ошибка генерации поверх удачного контента: статус → `error`, прежний контент сохранён, событие
    /// послано; Err проброшен планировщику (ретрай/dead).
    #[tokio::test]
    async fn error_marks_status_keeps_content_and_notifies() {
        let (_d, db) = open_db().await;
        set_vault_mtime(&db, 500).await;

        // Сначала удачная генерация.
        let sink = Arc::new(RecordingSink::default());
        handler(
            &db,
            Ok("good".into()),
            Arc::new(AtomicUsize::new(0)),
            sink.clone(),
        )
        .handle(&dummy_job(&widget_kind("daily_brief")))
        .await
        .unwrap();

        // Затем падающая.
        let err = handler(
            &db,
            Err("llm down".into()),
            Arc::new(AtomicUsize::new(0)),
            sink.clone(),
        )
        .handle(&dummy_job(&widget_kind("daily_brief")))
        .await;
        assert_eq!(
            err,
            Err("llm down".into()),
            "ошибка проброшена планировщику"
        );

        let w = get(db.reader(), "daily_brief").await.unwrap().unwrap();
        assert_eq!(w.status, STATUS_ERROR, "статус помечен ошибкой");
        assert_eq!(w.content, "good", "прежний удачный контент сохранён");
        assert_eq!(
            sink.seen.lock().unwrap().as_slice(),
            ["daily_brief", "daily_brief"],
            "событие и на успехе, и на ошибке (строка была)"
        );
    }

    /// Ошибка генерации без прежнего кэша: строки нет → не помечаем, событие НЕ шлём (показывать нечего),
    /// но Err всё равно проброшен (планировщик увидит провал).
    #[tokio::test]
    async fn error_without_prior_cache_is_silent_but_fails() {
        let (_d, db) = open_db().await;
        set_vault_mtime(&db, 500).await;
        let sink = Arc::new(RecordingSink::default());
        let err = handler(
            &db,
            Err("boom".into()),
            Arc::new(AtomicUsize::new(0)),
            sink.clone(),
        )
        .handle(&dummy_job(&widget_kind("daily_brief")))
        .await;
        assert_eq!(err, Err("boom".into()));
        assert!(get(db.reader(), "daily_brief").await.unwrap().is_none());
        assert!(
            sink.seen.lock().unwrap().is_empty(),
            "нет строки → нет события"
        );
    }

    #[test]
    fn widget_kind_is_namespaced() {
        assert_eq!(widget_kind("daily_brief"), "home_widget:daily_brief");
    }

    #[test]
    fn registry_tracks_known_keys() {
        let mut r = WidgetRegistry::new();
        assert!(!r.contains("daily_brief"));
        r.register("daily_brief");
        r.register("daily_brief"); // идемпотентно
        assert!(r.contains("daily_brief"));
        assert!(!r.contains("unknown"));
    }
}
