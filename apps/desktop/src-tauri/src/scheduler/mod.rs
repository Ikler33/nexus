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

use crate::db::{DbResult, WriteActor};

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
fn now_secs() -> i64 {
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

// ── slice 2: движок диспатча ──────────────────────────────────────────────────────────────────

/// Обработчик джобы конкретного kind. Реализация держит свои зависимости (db/embedder/chat).
#[async_trait]
pub trait JobHandler: Send + Sync {
    /// Выполнить джобу: `Ok` → `done`; `Err(msg)` → retry/dead (S7).
    async fn handle(&self, job: &Job) -> Result<(), String>;
}

/// Реестр обработчиков по `kind`.
pub type Registry = HashMap<String, Arc<dyn JobHandler>>;

/// Прогоняет готовые джобы (claim → dispatch → complete/fail), не более `MAX_PER_TICK` за вызов.
/// Неизвестный `kind` → `fail` (после ретраев — видимый `dead`). Возвращает число обработанных.
pub async fn run_due(writer: &WriteActor, registry: &Registry, now: i64) -> DbResult<usize> {
    let mut n = 0;
    while n < MAX_PER_TICK {
        let Some(job) = claim_next(writer, now).await? else {
            break;
        };
        let result = match registry.get(&job.kind) {
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
/// жив токен задачи (спавнится на vault-open — срез триггеров). Backpressure чата (S5) — с LLM-kind.
pub fn spawn_worker(writer: WriteActor, app: tauri::AppHandle, registry: Arc<Registry>) {
    tokio::spawn(async move {
        if let Err(e) = requeue_running(&writer).await {
            tracing::warn!(error = %e, "scheduler crash-recovery failed");
        }
        let mut interval = tokio::time::interval(Duration::from_secs(TICK_SECS));
        loop {
            interval.tick().await;
            match run_due(&writer, &registry, now_secs()).await {
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

    /// Обработчик-счётчик (опц. падающий) для проверки диспатча.
    struct Counting {
        calls: Arc<AtomicUsize>,
        fail: bool,
    }
    #[async_trait]
    impl JobHandler for Counting {
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
            }),
        );
        reg.insert(
            "bad".into(),
            Arc::new(Counting {
                calls: calls.clone(),
                fail: true,
            }),
        );
        enqueue(w, "ok", "", 0, 5).await.unwrap();
        enqueue(w, "bad", "", 0, 5).await.unwrap();
        enqueue(w, "ghost", "", 0, 1).await.unwrap(); // нет хендлера, max=1 → сразу dead

        let n = run_due(w, &reg, 100).await.unwrap();
        assert_eq!(n, 3, "три готовые обработаны");
        assert_eq!(calls.load(Ordering::SeqCst), 2, "вызваны только ok+bad");
        // ok→done, bad→backoff (не готова), ghost→dead → готовых нет
        assert!(
            run_due(w, &reg, 100).await.unwrap() == 0,
            "повторно готовых нет"
        );
    }
}
