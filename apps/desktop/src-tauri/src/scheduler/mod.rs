//! Планировщик фоновых задач (ADR-007) — **слой данных очереди `jobs`** (slice 1). Воркер-луп,
//! триггеры (on-open/on-change/scheduled), backpressure и первые kind — следующие срезы.
//!
//! Решения owner-codesign: состояния `pending → running → done | dead`; экспоненциальный backoff +
//! `max_attempts`, по исчерпании — видимый `dead` (S7, НЕ тихий дроп); claim сериализован единственным
//! write-actor'ом (ADR-003 — без гонок); crash-recovery «зависших» `running → pending` на старте (S8);
//! offline-джобы остаются `pending` и ждут (S10). Логически значимое время (`run_at`/backoff) —
//! явными параметрами → детерминированные тесты; `created_at/updated_at` — внутренним `now_secs`.

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::db::{Database, DbResult};

/// База экспоненциального backoff ретрая (сек) и потолок задержки.
const BACKOFF_BASE_SECS: i64 = 30;
const BACKOFF_MAX_SECS: i64 = 3600;

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
    db: &Database,
    kind: &str,
    payload: &str,
    run_at: i64,
    max_attempts: i64,
) -> DbResult<i64> {
    let (kind, payload) = (kind.to_string(), payload.to_string());
    db.writer()
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
pub async fn claim_next(db: &Database, now: i64) -> DbResult<Option<Job>> {
    db.writer()
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
pub async fn complete(db: &Database, id: i64) -> DbResult<()> {
    db.writer()
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
pub async fn fail(db: &Database, id: i64, error: &str, now: i64) -> DbResult<()> {
    let error = error.to_string();
    db.writer()
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
/// Вызывается на старте планировщика. Возвращает число восстановленных.
pub async fn requeue_running(db: &Database) -> DbResult<usize> {
    db.writer()
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
pub async fn gc_done(db: &Database, before: i64) -> DbResult<usize> {
    db.writer()
        .transaction(move |tx| {
            tx.execute(
                "DELETE FROM jobs WHERE state='done' AND updated_at<?1",
                [before],
            )
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn open() -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join(".nexus/nexus.db"))
            .await
            .unwrap();
        (dir, db)
    }

    /// claim уважает `run_at` (future-джоба не берётся), помечает running (повторно не клеймится), complete→done.
    #[tokio::test]
    async fn claim_respects_run_at_and_completes() {
        let (_d, db) = open().await;
        let ready = enqueue(&db, "test", "{}", 100, 5).await.unwrap();
        let _future = enqueue(&db, "test", "{}", 1000, 5).await.unwrap();

        let j = claim_next(&db, 200).await.unwrap().expect("есть готовая");
        assert_eq!(j.id, ready);
        assert_eq!(j.state, "running");
        // running не переклеймится; future ещё не готова при now=200
        assert!(claim_next(&db, 200).await.unwrap().is_none());
        complete(&db, ready).await.unwrap();
    }

    /// fail: backoff (не готова сразу) → после задержки готова (attempts++) → по исчерпании max → dead.
    #[tokio::test]
    async fn fail_retries_with_backoff_then_dead() {
        let (_d, db) = open().await;
        let id = enqueue(&db, "test", "{}", 0, 2).await.unwrap(); // max_attempts=2
        claim_next(&db, 10).await.unwrap().unwrap();

        fail(&db, id, "boom", 10).await.unwrap();
        assert!(
            claim_next(&db, 10).await.unwrap().is_none(),
            "backoff: не готова сразу после неудачи"
        );
        let j = claim_next(&db, 10_000)
            .await
            .unwrap()
            .expect("после backoff снова готова");
        assert_eq!(j.attempts, 1);

        fail(&db, id, "boom2", 10_000).await.unwrap(); // attempts=2 >= max → dead
        assert!(
            claim_next(&db, 1_000_000).await.unwrap().is_none(),
            "dead не клеймится"
        );
    }

    /// requeue_running возвращает «зависшие» running в pending; gc_done чистит завершённые.
    #[tokio::test]
    async fn requeue_and_gc() {
        let (_d, db) = open().await;
        let a = enqueue(&db, "test", "", 0, 5).await.unwrap();
        claim_next(&db, 1).await.unwrap().unwrap(); // a → running
        assert_eq!(requeue_running(&db).await.unwrap(), 1);
        let j = claim_next(&db, 1).await.unwrap().expect("a снова pending");
        assert_eq!(j.id, a);
        complete(&db, a).await.unwrap();
        assert_eq!(gc_done(&db, i64::MAX).await.unwrap(), 1, "done удалён GC");
        assert!(claim_next(&db, 1).await.unwrap().is_none());
    }
}
