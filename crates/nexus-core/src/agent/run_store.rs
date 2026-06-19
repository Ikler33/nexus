//! Хранилище прогонов агента (AGENT-2) — async-CRUD над таблицей `agent_runs` (миграция 021).
//!
//! Источник истины состояния прогона цикла агента: статус-машина
//! `queued → running → done | error | cancelled`, шаг цикла (наблюдаемость/replay) и корреляция с
//! `egress_audit.run_id` (id строки прогона == тот самый i64 run_id). Все мутации идут через
//! единственный [`WriteActor`] (ADR-003 — сериализованы, без гонок); чтения — через [`ReadPool`].
//!
//! Append/update-only: строки НЕ удаляются — это журнал прогонов; меняются только статус/шаг/исход/
//! метка `updated_at`. Каждый переход обновляет `updated_at` (нужно для TTL crash-recovery).
//!
//! Терминальные статусы (`done`/`error`/`cancelled`) — поглощающие: [`finish_run`] из них больше не
//! двигает строку (см. контракт ниже). Это якорь идемпотентности replay: повторный handle уже
//! терминального прогона — no-op (см. `agent/job.rs`).

use rusqlite::{params, OptionalExtension};

use crate::db::{DbResult, ReadPool, WriteActor};
use crate::scheduler::now_secs;

/// Статусы прогона (значения колонки `agent_runs.status`). Строковые литералы — единый источник, чтобы
/// SQL и проверки не разъехались по опечаткам.
pub const STATUS_QUEUED: &str = "queued";
pub const STATUS_RUNNING: &str = "running";
pub const STATUS_DONE: &str = "done";
pub const STATUS_ERROR: &str = "error";
pub const STATUS_CANCELLED: &str = "cancelled";

/// Терминален ли статус (поглощающий — finish/replay из него не двигают строку).
pub fn is_terminal(status: &str) -> bool {
    matches!(status, STATUS_DONE | STATUS_ERROR | STATUS_CANCELLED)
}

/// Снимок строки прогона (`agent_runs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRun {
    pub id: i64,
    pub session_id: Option<String>,
    pub task: String,
    pub status: String,
    pub model: Option<String>,
    pub autonomy: Option<String>,
    pub outcome: Option<String>,
    pub step: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Создаёт прогон в статусе `queued` (шаг 0, без исхода). Возвращает его `id` (= run_id для
/// корреляции egress). `task` — пользовательский промпт; `model`/`autonomy` — метаданные прогона.
pub async fn create_run(
    writer: &WriteActor,
    task: &str,
    model: Option<&str>,
    autonomy: Option<&str>,
) -> DbResult<i64> {
    let (task, model, autonomy) = (
        task.to_string(),
        model.map(str::to_string),
        autonomy.map(str::to_string),
    );
    writer
        .transaction(move |tx| {
            let ts = now_secs();
            tx.execute(
                "INSERT INTO agent_runs(task,status,model,autonomy,step,created_at,updated_at) \
                 VALUES(?1,?2,?3,?4,0,?5,?5)",
                params![task, STATUS_QUEUED, model, autonomy, ts],
            )?;
            Ok(tx.last_insert_rowid())
        })
        .await
}

/// Помечает прогон `running` (бамп `updated_at` — обновляет TTL-метку для crash-recovery). Перевод
/// НЕ из `queued` тоже допускается (повторный claim после requeue), но НЕ трогает терминальные:
/// финальный прогон не «оживает». Возвращает `true`, если строка реально перешла в `running`.
pub async fn mark_running(writer: &WriteActor, id: i64) -> DbResult<bool> {
    writer
        .transaction(move |tx| {
            let n = tx.execute(
                "UPDATE agent_runs SET status=?2, updated_at=?3 \
                 WHERE id=?1 AND status NOT IN (?4,?5,?6)",
                params![
                    id,
                    STATUS_RUNNING,
                    now_secs(),
                    STATUS_DONE,
                    STATUS_ERROR,
                    STATUS_CANCELLED
                ],
            )?;
            Ok(n > 0)
        })
        .await
}

/// Фиксирует достигнутый шаг цикла (наблюдаемость/replay) + бамп `updated_at` (TTL-heartbeat: пока
/// прогон жив, requeue_stale_running его не подберёт). Монотонность не навязываем — пишем как есть.
/// Терминальные строки не трогаем (поздний bump после финала не должен «воскрешать» TTL-метку).
pub async fn bump_step(writer: &WriteActor, id: i64, step: i64) -> DbResult<()> {
    writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE agent_runs SET step=?2, updated_at=?3 \
                 WHERE id=?1 AND status NOT IN (?4,?5,?6)",
                params![
                    id,
                    step,
                    now_secs(),
                    STATUS_DONE,
                    STATUS_ERROR,
                    STATUS_CANCELLED
                ],
            )?;
            Ok(())
        })
        .await
}

/// Терминирует прогон: ставит финальный `status` (`done`/`error`/`cancelled`) + `outcome` + бамп
/// `updated_at`. **Поглощающий:** если строка УЖЕ терминальна, finish — no-op (первый терминал
/// побеждает; повторный handle/replay не перезаписывает исход). Возвращает `true`, если строка
/// реально терминирована этим вызовом. Не-терминальный `status` (попытка финишировать в `queued`/
/// `running`) отвергается аргументом-инвариантом — вызывающий передаёт только терминал.
pub async fn finish_run(
    writer: &WriteActor,
    id: i64,
    status: &str,
    outcome: Option<&str>,
) -> DbResult<bool> {
    debug_assert!(
        is_terminal(status),
        "finish_run требует терминальный статус, получено: {status}"
    );
    let (status, outcome) = (status.to_string(), outcome.map(str::to_string));
    writer
        .transaction(move |tx| {
            let n = tx.execute(
                "UPDATE agent_runs SET status=?2, outcome=?3, updated_at=?4 \
                 WHERE id=?1 AND status NOT IN (?5,?6,?7)",
                params![
                    id,
                    status,
                    outcome,
                    now_secs(),
                    STATUS_DONE,
                    STATUS_ERROR,
                    STATUS_CANCELLED
                ],
            )?;
            Ok(n > 0)
        })
        .await
}

/// KILL-SWITCH (AGENT-5): возвращает НЕ-терминальный прогон в `queued` (пауза мид-ран — прогон не
/// завершён, должен возобновиться на un-pause). Терминальные строки (`done`/`error`/`cancelled`) НЕ
/// трогаем (finished-прогон не «оживает»). `step` сохраняется (наблюдаемость; replay перезапустит цикл
/// с начала — replay-safe, см. контракт `agent/job.rs`). Возвращает `true`, если строка реально
/// возвращена в queued. Зеркало `requeue_stale_running`, но адресно по id и без TTL-условия.
pub async fn requeue_to_queued(writer: &WriteActor, id: i64) -> DbResult<bool> {
    writer
        .transaction(move |tx| {
            let n = tx.execute(
                "UPDATE agent_runs SET status=?2, updated_at=?3 \
                 WHERE id=?1 AND status NOT IN (?4,?5,?6)",
                params![
                    id,
                    STATUS_QUEUED,
                    now_secs(),
                    STATUS_DONE,
                    STATUS_ERROR,
                    STATUS_CANCELLED
                ],
            )?;
            Ok(n > 0)
        })
        .await
}

/// Читает строку прогона по id (`None` — нет такой).
pub async fn get_run(reader: &ReadPool, id: i64) -> DbResult<Option<AgentRun>> {
    reader
        .query(move |c| {
            c.query_row(
                "SELECT id,session_id,task,status,model,autonomy,outcome,step,created_at,updated_at \
                 FROM agent_runs WHERE id=?1",
                [id],
                row_to_run,
            )
            .optional()
        })
        .await
}

/// Crash-recovery (как `scheduler::requeue_running`, но на УРОВНЕ ПРОГОНА): прогоны, застрявшие в
/// `running` и НЕ обновлявшиеся дольше `older_than_secs` (по `updated_at`), возвращаются в `queued`
/// (шаг сохраняется — наблюдаемость; replay перезапустит цикл с начала, что безопасно для AGENT-1
/// стаб-инструментов без побочных эффектов — см. контракт в `agent/job.rs`). `now` явный →
/// детерминированные тесты. Возвращает число восстановленных. СВЕЖИЕ `running` (в пределах TTL) НЕ
/// трогаем — иначе оборвали бы живой прогон.
pub async fn requeue_stale_running(
    writer: &WriteActor,
    older_than_secs: i64,
    now: i64,
) -> DbResult<usize> {
    writer
        .transaction(move |tx| {
            let cutoff = now - older_than_secs;
            // `updated_at` ставим из ЯВНОГО `now` (а не `now_secs()`): единый источник времени с
            // `cutoff` → детерминированно в тестах и без рассинхрона метки/порога (прод передаёт
            // `now_secs()`, поведение то же).
            tx.execute(
                "UPDATE agent_runs SET status=?1, updated_at=?2 \
                 WHERE status=?3 AND updated_at < ?4",
                params![STATUS_QUEUED, now, STATUS_RUNNING, cutoff],
            )
        })
        .await
}

/// Маппинг строки результата в [`AgentRun`] (порядок колонок фиксирован SELECT'ами выше).
fn row_to_run(r: &rusqlite::Row<'_>) -> rusqlite::Result<AgentRun> {
    Ok(AgentRun {
        id: r.get(0)?,
        session_id: r.get(1)?,
        task: r.get(2)?,
        status: r.get(3)?,
        model: r.get(4)?,
        autonomy: r.get(5)?,
        outcome: r.get(6)?,
        step: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
    })
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

    /// Полный happy-path статус-машины: create(queued)→mark_running→bump_step→finish(done);
    /// get_run на каждом шаге отражает переход; исход/шаг сохранены.
    #[tokio::test]
    async fn create_run_running_step_finish_lifecycle() {
        let (_d, db) = open().await;
        let w = db.writer();
        let id = create_run(w, "сделай X", Some("qwen"), Some("auto"))
            .await
            .unwrap();

        let r = get_run(db.reader(), id).await.unwrap().expect("создан");
        assert_eq!(r.status, STATUS_QUEUED);
        assert_eq!(r.task, "сделай X");
        assert_eq!(r.model.as_deref(), Some("qwen"));
        assert_eq!(r.autonomy.as_deref(), Some("auto"));
        assert_eq!(r.step, 0);
        assert!(r.outcome.is_none());

        assert!(mark_running(w, id).await.unwrap(), "queued→running");
        assert_eq!(
            get_run(db.reader(), id).await.unwrap().unwrap().status,
            STATUS_RUNNING
        );

        bump_step(w, id, 3).await.unwrap();
        assert_eq!(get_run(db.reader(), id).await.unwrap().unwrap().step, 3);

        assert!(
            finish_run(w, id, STATUS_DONE, Some("готово"))
                .await
                .unwrap(),
            "running→done"
        );
        let done = get_run(db.reader(), id).await.unwrap().unwrap();
        assert_eq!(done.status, STATUS_DONE);
        assert_eq!(done.outcome.as_deref(), Some("готово"));
        assert_eq!(done.step, 3, "шаг сохранён после финала");
    }

    /// Терминал — поглощающий: finish уже-терминальной строки — no-op (исход первого финала
    /// побеждает). mark_running/bump_step терминал тоже не трогают (прогон не «оживает»). Это
    /// фундамент идемпотентности replay (см. agent/job.rs).
    #[tokio::test]
    async fn terminal_status_is_absorbing() {
        let (_d, db) = open().await;
        let w = db.writer();
        let id = create_run(w, "t", None, None).await.unwrap();
        mark_running(w, id).await.unwrap();
        assert!(finish_run(w, id, STATUS_DONE, Some("первый"))
            .await
            .unwrap());

        // Повторный finish с ДРУГИМ исходом — no-op (false), исход не перезаписан.
        assert!(
            !finish_run(w, id, STATUS_ERROR, Some("второй"))
                .await
                .unwrap(),
            "терминал поглощающий: повторный finish — no-op"
        );
        // mark_running/bump_step тоже не трогают терминал.
        assert!(
            !mark_running(w, id).await.unwrap(),
            "running из терминала не воскрешает"
        );
        bump_step(w, id, 99).await.unwrap();

        let r = get_run(db.reader(), id).await.unwrap().unwrap();
        assert_eq!(r.status, STATUS_DONE, "статус — done первого финала");
        assert_eq!(r.outcome.as_deref(), Some("первый"), "исход первого финала");
        assert_eq!(r.step, 0, "bump_step не тронул терминал");
    }

    /// requeue_stale_running: устаревший 'running' (updated_at старше TTL) → 'queued'; СВЕЖИЙ
    /// 'running' (в пределах TTL) НЕ тронут; шаг сохранён.
    #[tokio::test]
    async fn requeue_stale_running_flips_only_stale() {
        let (_d, db) = open().await;
        let w = db.writer();
        // Два running-прогона; одному вручную состарим updated_at.
        let stale = create_run(w, "stale", None, None).await.unwrap();
        let fresh = create_run(w, "fresh", None, None).await.unwrap();
        mark_running(w, stale).await.unwrap();
        bump_step(w, stale, 5).await.unwrap();
        mark_running(w, fresh).await.unwrap();

        // Состарим updated_at у stale напрямую (имитируем краш во время прогона давным-давно).
        w.call(move |c| {
            c.execute("UPDATE agent_runs SET updated_at=100 WHERE id=?1", [stale])
                .map(|_| ())
        })
        .await
        .unwrap();

        // now=10_000, TTL=600 → cutoff=9400; stale.updated_at=100<9400 → requeue; fresh свежий → нет.
        let n = requeue_stale_running(w, 600, 10_000).await.unwrap();
        assert_eq!(n, 1, "ровно один устаревший восстановлен");

        let s = get_run(db.reader(), stale).await.unwrap().unwrap();
        assert_eq!(s.status, STATUS_QUEUED, "stale → queued");
        assert_eq!(s.step, 5, "шаг сохранён при requeue (наблюдаемость)");
        let f = get_run(db.reader(), fresh).await.unwrap().unwrap();
        assert_eq!(f.status, STATUS_RUNNING, "fresh не тронут");
    }

    /// requeue_stale_running НЕ трогает терминальные/queued строки даже со старым updated_at
    /// (восстановление — только для застрявших 'running').
    #[tokio::test]
    async fn requeue_stale_running_ignores_non_running() {
        let (_d, db) = open().await;
        let w = db.writer();
        let done = create_run(w, "done", None, None).await.unwrap();
        mark_running(w, done).await.unwrap();
        finish_run(w, done, STATUS_DONE, None).await.unwrap();
        let queued = create_run(w, "queued", None, None).await.unwrap();
        // Состарим обе.
        w.call(move |c| {
            c.execute("UPDATE agent_runs SET updated_at=1", [])
                .map(|_| ())
        })
        .await
        .unwrap();

        let n = requeue_stale_running(w, 0, 10_000).await.unwrap();
        assert_eq!(n, 0, "терминал и queued не восстанавливаются");
        assert_eq!(
            get_run(db.reader(), done).await.unwrap().unwrap().status,
            STATUS_DONE
        );
        assert_eq!(
            get_run(db.reader(), queued).await.unwrap().unwrap().status,
            STATUS_QUEUED
        );
    }

    /// get_run на несуществующем id → None.
    #[tokio::test]
    async fn get_run_missing_is_none() {
        let (_d, db) = open().await;
        assert!(get_run(db.reader(), 9999).await.unwrap().is_none());
    }
}
