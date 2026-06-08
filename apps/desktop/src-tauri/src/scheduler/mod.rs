//! Планировщик фоновых задач (ADR-007). slice 1 — очередь `jobs` (данные); **slice 2 — движок
//! диспатча**: реестр обработчиков (kind → handler), прогон готовых джоб (`run_due`), воркер-луп
//! (tokio-interval, S1). Триггеры, первые kind и live-спавн в `open_vault` — следующие срезы;
//! backpressure чата (S5) — вместе с LLM-kind.
//!
//! Решения owner-codesign: состояния `pending → running → done | dead`; экспоненциальный backoff +
//! `max_attempts`, по исчерпании — видимый `dead` (S7, не тихий дроп); claim сериализован единственным
//! write-actor'ом (ADR-003 — без гонок); crash-recovery «зависших» `running → pending` на старте (S8);
//! offline-джобы остаются `pending` (S10). Логически значимое время (`run_at`/backoff) — явными
//! параметрами → детерминированные тесты; `created_at/updated_at` — внутренним `now_secs`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::db::{DbResult, ReadPool, WriteActor};

/// База экспоненциального backoff ретрая (сек) и потолок задержки.
const BACKOFF_BASE_SECS: i64 = 30;
const BACKOFF_MAX_SECS: i64 = 3600;
/// Интервал опроса очереди воркером (S1: tokio-interval пока vault открыт).
const TICK_SECS: u64 = 5;
/// Потолок джоб за один тик — анти-голодание (no silent caps: излишек растащится на следующие тики).
const MAX_PER_TICK: usize = 64;

/// Джоба очереди планировщика.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub payload: String,
    pub state: String,
    pub run_at: i64,
    pub attempts: i64,
    pub max_attempts: i64,
    pub last_error: Option<String>,
}

/// Текущее unix-время (сек) — для меток created_at/updated_at; планирование принимает время явно.
pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Ставит джобу в очередь (`pending`, не раньше `run_at`). Возвращает её id.
pub async fn enqueue(
    writer: &WriteActor,
    kind: &str,
    payload: &str,
    run_at: i64,
    max_attempts: i64,
) -> DbResult<i64> {
    let (kind, payload) = (kind.to_string(), payload.to_string());
    writer
        .transaction(move |tx| {
            let ts = now_secs();
            tx.execute(
                "INSERT INTO jobs(kind,payload,state,run_at,attempts,max_attempts,created_at,updated_at) \
                 VALUES(?1,?2,'pending',?3,0,?4,?5,?5)",
                params![kind, payload, run_at, max_attempts, ts],
            )?;
            Ok(tx.last_insert_rowid())
        })
        .await
}

/// Захватывает следующую готовую джобу (`pending` и `run_at<=now`), помечая `running`. `None` — нет
/// готовых. Атомарно: единственный write-actor сериализует claim (без гонок, ADR-003).
pub async fn claim_next(writer: &WriteActor, now: i64) -> DbResult<Option<Job>> {
    writer
        .transaction(move |tx| {
            let job = tx
                .query_row(
                    "SELECT id,kind,payload,state,run_at,attempts,max_attempts,last_error \
                     FROM jobs WHERE state='pending' AND run_at<=?1 ORDER BY run_at,id LIMIT 1",
                    [now],
                    |r| {
                        Ok(Job {
                            id: r.get(0)?,
                            kind: r.get(1)?,
                            payload: r.get(2)?,
                            state: r.get(3)?,
                            run_at: r.get(4)?,
                            attempts: r.get(5)?,
                            max_attempts: r.get(6)?,
                            last_error: r.get(7)?,
                        })
                    },
                )
                .optional()?;
            if let Some(j) = &job {
                tx.execute(
                    "UPDATE jobs SET state='running', updated_at=?2 WHERE id=?1",
                    params![j.id, now_secs()],
                )?;
            }
            Ok(job.map(|mut j| {
                j.state = "running".into();
                j
            }))
        })
        .await
}

/// Успешное завершение → `done`.
pub async fn complete(writer: &WriteActor, id: i64) -> DbResult<()> {
    writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE jobs SET state='done', last_error=NULL, updated_at=?2 WHERE id=?1",
                params![id, now_secs()],
            )?;
            Ok(())
        })
        .await
}

/// Неудача: `attempts++`; если `attempts >= max_attempts` → `dead` (видимый, S7), иначе → `pending` с
/// экспоненциальным backoff (`run_at = now + base*2^attempts`, cap). `now` явный → детерминированно.
pub async fn fail(writer: &WriteActor, id: i64, error: &str, now: i64) -> DbResult<()> {
    let error = error.to_string();
    writer
        .transaction(move |tx| {
            let (attempts, max): (i64, i64) = tx.query_row(
                "SELECT attempts,max_attempts FROM jobs WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let attempts = attempts + 1;
            let ts = now_secs();
            if attempts >= max {
                tx.execute(
                    "UPDATE jobs SET state='dead', attempts=?2, last_error=?3, updated_at=?4 WHERE id=?1",
                    params![id, attempts, error, ts],
                )?;
            } else {
                let delay =
                    BACKOFF_BASE_SECS.saturating_mul(1i64 << attempts.min(20)).min(BACKOFF_MAX_SECS);
                tx.execute(
                    "UPDATE jobs SET state='pending', attempts=?2, last_error=?3, run_at=?4, updated_at=?5 \
                     WHERE id=?1",
                    params![id, attempts, error, now + delay, ts],
                )?;
            }
            Ok(())
        })
        .await
}

/// Crash-recovery: «зависшие» `running` (приложение упало во время выполнения) → `pending` (S8).
/// Вызывается на старте воркера. Возвращает число восстановленных.
pub async fn requeue_running(writer: &WriteActor) -> DbResult<usize> {
    writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE jobs SET state='pending', updated_at=?1 WHERE state='running'",
                [now_secs()],
            )
        })
        .await
}

/// GC: удаляет `done`-джобы старше `before` (`updated_at < before`) — чтобы `idx_jobs_claim` не
/// деградировал на тысячах завершённых (S7). Возвращает число удалённых.
pub async fn gc_done(writer: &WriteActor, before: i64) -> DbResult<usize> {
    writer
        .transaction(move |tx| {
            tx.execute(
                "DELETE FROM jobs WHERE state='done' AND updated_at<?1",
                [before],
            )
        })
        .await
}

/// Сводка очереди для StatusBar (S7/S8 — видимость состояния). `done` не считаем (их чистит gc).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobCounts {
    /// Ожидают выполнения (в т.ч. отложенные backpressure'ом).
    pub pending: i64,
    /// Выполняются сейчас.
    pub running: i64,
    /// Исчерпали ретраи — видимый «мёртвый» (S7), нужен взгляд пользователя.
    pub dead: i64,
}

/// Считает джобы по состояниям (для StatusBar N/M). Один GROUP BY — дёшево по `idx_jobs_claim`.
pub async fn counts(reader: &ReadPool) -> DbResult<JobCounts> {
    reader
        .query(|c| {
            let mut out = JobCounts::default();
            let mut stmt = c.prepare(
                "SELECT state, count(*) FROM jobs \
                 WHERE state IN ('pending','running','dead') GROUP BY state",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            for row in rows {
                let (state, n) = row?;
                match state.as_str() {
                    "pending" => out.pending = n,
                    "running" => out.running = n,
                    "dead" => out.dead = n,
                    _ => {}
                }
            }
            Ok(out)
        })
        .await
}

/// Backpressure (S5): отложить заклеймленную джобу обратно в `pending` с новым `run_at`, **без** штрафа
/// `attempts` (это не неудача, а уступка интерактивному LLM). Воркер так уступает дайджест, пока
/// пользователь занят чатом/inline.
pub async fn defer(writer: &WriteActor, id: i64, run_at: i64) -> DbResult<()> {
    writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE jobs SET state='pending', run_at=?2, updated_at=?3 WHERE id=?1",
                params![id, run_at, now_secs()],
            )
            .map(|_| ())
        })
        .await
}

// ── slice 2: движок диспатча ──────────────────────────────────────────────────────────────────

/// Обработчик джобы конкретного kind. Реализация держит свои зависимости (db/embedder/chat).
#[async_trait]
pub trait JobHandler: Send + Sync {
    /// Выполнить джобу: `Ok` → `done`; `Err(msg)` → retry/dead (S7).
    async fn handle(&self, job: &Job) -> Result<(), String>;

    /// Уступать ли эту джобу интерактивному LLM (S5 backpressure). `true` для тяжёлых фоновых
    /// LLM-kind (дайджест/карта/противоречия) — пока идёт чат/inline, такие джобы откладываются.
    /// `false` (по умолчанию) — лёгкие/не-LLM (gc) выполняются всегда.
    fn defer_under_interactive(&self) -> bool {
        false
    }
}

/// Реестр обработчиков по `kind`.
pub type Registry = HashMap<String, Arc<dyn JobHandler>>;

/// Прогоняет готовые джобы (claim → dispatch → complete/fail), не более `MAX_PER_TICK` за вызов.
/// Неизвестный `kind` → `fail` (после ретраев — видимый `dead`). Возвращает число обработанных.
/// `busy` (S5) — идёт ли интерактивный LLM: тогда `defer_under_interactive`-джобы откладываются
/// (`run_at = now + TICK`), не выполняются и не считаются обработанными (уступка чату/inline).
pub async fn run_due(
    writer: &WriteActor,
    registry: &Registry,
    now: i64,
    busy: bool,
) -> DbResult<usize> {
    let mut n = 0;
    while n < MAX_PER_TICK {
        let Some(job) = claim_next(writer, now).await? else {
            break;
        };
        let handler = registry.get(&job.kind);
        // Backpressure (S5): уступаем тяжёлые LLM-джобы интерактиву → откладываем за текущий тик.
        if busy && handler.is_some_and(|h| h.defer_under_interactive()) {
            defer(writer, job.id, now + TICK_SECS as i64).await?;
            continue; // run_at в будущем → этот же job в этом тике повторно не заклеймится
        }
        let result = match handler {
            Some(h) => h.handle(&job).await,
            None => Err(format!("неизвестный kind: {}", job.kind)),
        };
        match result {
            Ok(()) => complete(writer, job.id).await?,
            Err(e) => fail(writer, job.id, &e, now).await?,
        }
        n += 1;
    }
    Ok(n)
}

/// Воркер-луп (S1): tokio-interval опрашивает очередь и прогоняет готовые джобы. На старте — crash-
/// recovery (S8). После продуктивного тика шлёт `jobs:changed` (для StatusBar — срез UI). Живёт, пока
/// жив токен задачи. **Backpressure (S5):** каждый тик снимает «занят ли интерактивный LLM» из
/// `AppState` и уступает фоновые LLM-джобы, пока идёт чат/inline.
pub fn spawn_worker(writer: WriteActor, app: tauri::AppHandle, registry: Arc<Registry>) {
    use tauri::Manager;
    tokio::spawn(async move {
        if let Err(e) = requeue_running(&writer).await {
            tracing::warn!(error = %e, "scheduler crash-recovery failed");
        }
        let mut interval = tokio::time::interval(Duration::from_secs(TICK_SECS));
        loop {
            interval.tick().await;
            let busy = app.state::<crate::state::AppState>().is_interactive_busy();
            match run_due(&writer, &registry, now_secs(), busy).await {
                Ok(n) if n > 0 => emit_jobs_changed(&app),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "scheduler tick failed"),
            }
        }
    });
}

/// Tauri-событие «состояние очереди изменилось» (для StatusBar N/M — срез UI). Best-effort.
fn emit_jobs_changed(app: &tauri::AppHandle) {
    use tauri::Emitter;
    let _ = app.emit("jobs:changed", ());
}

// ── slice 3: первый встроенный kind + реестр по умолчанию ───────────────────────────────────────

/// Встроенный kind «gc»: периодическая чистка завершённых джоб (S7). Первый live-потребитель воркера.
pub const KIND_GC: &str = "gc";
/// Сколько хранить `done`-джобы до сборки мусора.
const GC_RETENTION_SECS: i64 = 7 * 24 * 3600;

/// Обработчик «gc»: удаляет `done`-джобы старше retention. Держит свой клон write-actor.
struct GcHandler {
    writer: WriteActor,
    retention_secs: i64,
}

#[async_trait]
impl JobHandler for GcHandler {
    async fn handle(&self, _job: &Job) -> Result<(), String> {
        gc_done(&self.writer, now_secs() - self.retention_secs)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// Реестр встроенных обработчиков (сейчас — только `gc`). Расширяется реальными kind (Карта/Противоречия)
/// в следующих срезах. `writer` — клон для обработчиков, которым нужна запись.
pub fn default_registry(writer: WriteActor) -> Registry {
    let mut reg = Registry::new();
    reg.insert(
        KIND_GC.to_string(),
        Arc::new(GcHandler {
            writer,
            retention_secs: GC_RETENTION_SECS,
        }),
    );
    reg
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    async fn open() -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join(".nexus/nexus.db"))
            .await
            .unwrap();
        (dir, db)
    }

    /// Обработчик-счётчик (опц. падающий / уступающий интерактиву) для проверки диспатча.
    struct Counting {
        calls: Arc<AtomicUsize>,
        fail: bool,
        defer: bool,
    }
    #[async_trait]
    impl JobHandler for Counting {
        fn defer_under_interactive(&self) -> bool {
            self.defer
        }
        async fn handle(&self, _job: &Job) -> Result<(), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err("boom".into())
            } else {
                Ok(())
            }
        }
    }

    /// claim уважает `run_at`; помечает running (повторно не клеймится); complete→done.
    #[tokio::test]
    async fn claim_respects_run_at_and_completes() {
        let (_d, db) = open().await;
        let w = db.writer();
        let ready = enqueue(w, "test", "{}", 100, 5).await.unwrap();
        let _future = enqueue(w, "test", "{}", 1000, 5).await.unwrap();

        let j = claim_next(w, 200).await.unwrap().expect("есть готовая");
        assert_eq!(j.id, ready);
        assert_eq!(j.state, "running");
        assert!(claim_next(w, 200).await.unwrap().is_none());
        complete(w, ready).await.unwrap();
    }

    /// fail: backoff → после задержки готова (attempts++) → по исчерпании max → dead.
    #[tokio::test]
    async fn fail_retries_with_backoff_then_dead() {
        let (_d, db) = open().await;
        let w = db.writer();
        let id = enqueue(w, "test", "{}", 0, 2).await.unwrap();
        claim_next(w, 10).await.unwrap().unwrap();

        fail(w, id, "boom", 10).await.unwrap();
        assert!(
            claim_next(w, 10).await.unwrap().is_none(),
            "backoff: не готова сразу"
        );
        let j = claim_next(w, 10_000)
            .await
            .unwrap()
            .expect("после backoff готова");
        assert_eq!(j.attempts, 1);

        fail(w, id, "boom2", 10_000).await.unwrap(); // attempts=2 >= max → dead
        assert!(
            claim_next(w, 1_000_000).await.unwrap().is_none(),
            "dead не клеймится"
        );
    }

    /// requeue_running возвращает running→pending; gc_done чистит завершённые.
    #[tokio::test]
    async fn requeue_and_gc() {
        let (_d, db) = open().await;
        let w = db.writer();
        let a = enqueue(w, "test", "", 0, 5).await.unwrap();
        claim_next(w, 1).await.unwrap().unwrap();
        assert_eq!(requeue_running(w).await.unwrap(), 1);
        let j = claim_next(w, 1).await.unwrap().expect("a снова pending");
        assert_eq!(j.id, a);
        complete(w, a).await.unwrap();
        assert_eq!(gc_done(w, i64::MAX).await.unwrap(), 1, "done удалён GC");
    }

    /// run_due диспатчит готовые: успешный kind→done, падающий→backoff; неизвестный kind → fail.
    #[tokio::test]
    async fn run_due_dispatches_by_kind() {
        let (_d, db) = open().await;
        let w = db.writer();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut reg: Registry = HashMap::new();
        reg.insert(
            "ok".into(),
            Arc::new(Counting {
                calls: calls.clone(),
                fail: false,
                defer: false,
            }),
        );
        reg.insert(
            "bad".into(),
            Arc::new(Counting {
                calls: calls.clone(),
                fail: true,
                defer: false,
            }),
        );
        enqueue(w, "ok", "", 0, 5).await.unwrap();
        enqueue(w, "bad", "", 0, 5).await.unwrap();
        enqueue(w, "ghost", "", 0, 1).await.unwrap(); // нет хендлера, max=1 → сразу dead

        let n = run_due(w, &reg, 100, false).await.unwrap();
        assert_eq!(n, 3, "три готовые обработаны");
        assert_eq!(calls.load(Ordering::SeqCst), 2, "вызваны только ok+bad");
        // ok→done, bad→backoff (не готова), ghost→dead → готовых нет
        assert!(
            run_due(w, &reg, 100, false).await.unwrap() == 0,
            "повторно готовых нет"
        );
    }

    /// S5 backpressure: `defer_under_interactive`-джоба при `busy` откладывается (не выполняется,
    /// не считается), при `!busy` — выполняется. Лёгкие джобы (defer=false) под busy идут как обычно.
    #[tokio::test]
    async fn run_due_defers_llm_job_under_interactive() {
        let (_d, db) = open().await;
        let w = db.writer();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut reg: Registry = HashMap::new();
        reg.insert(
            "digest".into(),
            Arc::new(Counting {
                calls: calls.clone(),
                fail: false,
                defer: true,
            }),
        );
        enqueue(w, "digest", "", 0, 5).await.unwrap();

        // busy → отложена: handle не вызван, n=0, и в этом тике повторно не клеймится (run_at в будущем).
        assert_eq!(run_due(w, &reg, 100, true).await.unwrap(), 0);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "под интерактивом не выполняется"
        );
        assert!(
            claim_next(w, 100).await.unwrap().is_none(),
            "отложена за текущий тик"
        );

        // не busy (позже, когда run_at наступил) → выполняется.
        assert_eq!(run_due(w, &reg, 1000, false).await.unwrap(), 1);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "без интерактива выполнилась"
        );
    }

    /// `counts` отражает состояния очереди для StatusBar (pending/running/dead).
    #[tokio::test]
    async fn counts_reports_states() {
        let (_d, db) = open().await;
        let w = db.writer();
        enqueue(w, "a", "", 0, 5).await.unwrap(); // готовая → заклеймим в running
        enqueue(w, "b", "", 1000, 5).await.unwrap(); // будущая → остаётся pending
        let _running = claim_next(w, 100).await.unwrap().expect("a готова");

        let c = counts(db.reader()).await.unwrap();
        assert_eq!(c.running, 1, "a выполняется");
        assert_eq!(c.pending, 1, "b ждёт");
        assert_eq!(c.dead, 0);
    }

    /// Встроенный kind `gc` зарегистрирован в `default_registry`, прогоняется воркером и завершается
    /// успешно (done, без retry/dead) — проверяет диспатч встроенного обработчика.
    #[tokio::test]
    async fn gc_kind_registered_and_runs() {
        let (_d, db) = open().await;
        let w = db.writer();
        let reg = default_registry(w.clone());
        assert!(reg.contains_key(KIND_GC), "gc зарегистрирован");

        enqueue(w, KIND_GC, "", 0, 3).await.unwrap();
        assert_eq!(
            run_due(w, &reg, now_secs(), false).await.unwrap(),
            1,
            "gc-джоба обработана"
        );
        assert!(
            claim_next(w, now_secs() + 1).await.unwrap().is_none(),
            "gc завершилась (done), не ушла в retry/dead"
        );
    }
}
