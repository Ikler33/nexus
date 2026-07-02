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
    /// SUBAGENTS (SUB-0): id РОДИТЕЛЬСКОГО прогона (дерево делегирования). `None` = top-level прогон
    /// (все прежние `create_run`). Заполняется только [`create_child_run`].
    pub parent_run_id: Option<i64>,
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

/// W-38: создаёт прогон в статусе `queued`, привязанный к `session_id` (история переписок агента).
/// Идентично [`create_run`], но проставляет `agent_runs.session_id` — UI-путь десктопа группирует ходы
/// одной переписки общим `session_id` (фронт-стор). Top-level прогон (parent NULL). Возвращает `id`.
pub async fn create_run_in_session(
    writer: &WriteActor,
    session_id: &str,
    task: &str,
    model: Option<&str>,
    autonomy: Option<&str>,
) -> DbResult<i64> {
    let (session_id, task, model, autonomy) = (
        session_id.to_string(),
        task.to_string(),
        model.map(str::to_string),
        autonomy.map(str::to_string),
    );
    writer
        .transaction(move |tx| {
            let ts = now_secs();
            tx.execute(
                "INSERT INTO agent_runs(session_id,task,status,model,autonomy,step,created_at,updated_at) \
                 VALUES(?1,?2,?3,?4,?5,0,?6,?6)",
                params![session_id, task, STATUS_QUEUED, model, autonomy, ts],
            )?;
            Ok(tx.last_insert_rowid())
        })
        .await
}

/// Создаёт ПРОГОН-РЕБЁНОК (субагент, SUB-0) в статусе `queued` со ссылкой на `parent_run_id` (дерево
/// делегирования). Идентично [`create_run`], но проставляет `parent_run_id` (родитель ВИДЕН в строке для
/// реконструкции дерева, per-child корреляции egress/ledger, узлов плана). Возвращает `id` ребёнка
/// (= его run_id для egress-корреляции). Top-level прогоны по-прежнему создаёт [`create_run`] (parent
/// NULL) — обратная совместимость не нарушена.
pub async fn create_child_run(
    writer: &WriteActor,
    task: &str,
    model: Option<&str>,
    autonomy: Option<&str>,
    parent_run_id: i64,
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
                "INSERT INTO agent_runs(task,status,model,autonomy,parent_run_id,step,created_at,updated_at) \
                 VALUES(?1,?2,?3,?4,?5,0,?6,?6)",
                params![task, STATUS_QUEUED, model, autonomy, parent_run_id, ts],
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
                "SELECT id,session_id,task,status,model,autonomy,outcome,step,created_at,updated_at,parent_run_id \
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
/// (шаг сохраняется — наблюдаемость; replay перезапустит цикл с начала, что безопасно при ВЫКЛ
/// актуаторе — реестр записи пуст (B7), побочных эффектов нет — см. контракт в `agent/job.rs`). `now` явный →
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

/// **SUBAGENTS (SUB-3b-2a, фикс ревью #4): реконсиляция ОСИРОТЕВШИХ прогонов-ДЕТЕЙ.** Прогон-ребёнок
/// (`parent_run_id IS NOT NULL`) создаётся/гоняется ИНЛАЙН в `delegate.run` через `JoinSet`; при ДРОПЕ
/// фьючи инструмента (отмена/таймаут родителя) tokio аборнет дочерние задачи НА `.await` — строка
/// остаётся в `running` НАВСЕГДА (терминал `finish_run` не достигнут). В отличие от top-level прогонов
/// ([`requeue_stale_running`] → `queued` для re-claim джобой), ДЕТИ НЕ возобновляемы (нет джобы, нет
/// per-child replay) → застрявших переводим в ТЕРМИНАЛ `error` («прервано»). Запускается на СТАРТЕ
/// (как [`requeue_stale_running`]/`reconcile_stale_executing`) — **ЖЁСТКОЕ предусловие регистрации
/// `delegate.run` (SUB-3b-2b).** `older_than_secs` ДОЛЖЕН превышать макс. время жизни ребёнка (дети
/// ограничены тем же `LoopBounds.wall_clock`, что и top-level → тот же TTL безопасен). Возвращает число
/// реконсилированных. `now` явный → детерминированные тесты.
pub async fn reconcile_orphan_child_runs(
    writer: &WriteActor,
    older_than_secs: i64,
    now: i64,
) -> DbResult<usize> {
    writer
        .transaction(move |tx| {
            let cutoff = now - older_than_secs;
            tx.execute(
                "UPDATE agent_runs SET status=?1, outcome=?2, updated_at=?3 \
                 WHERE status=?4 AND parent_run_id IS NOT NULL AND updated_at < ?5",
                params![
                    STATUS_ERROR,
                    "прервано (осиротевший субагент)",
                    now,
                    STATUS_RUNNING,
                    cutoff
                ],
            )
        })
        .await
}

// ── W-38: персист ходов агента (история переписок) ─────────────────────────────────────────────────

/// Один шаг хода для персиста (`agent_turn_steps`). Зеркало `AgentStep` фронт-стора без runtime-полей:
/// `ord` — порядок в ходе (по нему реконструируем ленту), `kind`/`args`/`title`/`result`/`is_error`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistStep {
    pub ord: i64,
    pub kind: String,
    pub args: String,
    pub title: Option<String>,
    pub result: Option<String>,
    pub is_error: bool,
}

/// Сводка одной агент-сессии для левого сайдбара истории (W-38). `title` — задача ПЕРВОГО хода (с чего
/// началась переписка); `status` — статус ПОСЛЕДНЕГО (терминал последнего хода); `turn_count`/`updated_at`
/// — агрегаты по `session_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSessionRow {
    pub session_id: String,
    pub title: String,
    pub status: String,
    pub turn_count: i64,
    pub updated_at: i64,
}

/// Один персистированный ход переписки (W-38) + его шаги (для реконструкции ленты при переоткрытии).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedTurnRow {
    pub run_id: i64,
    pub task: String,
    pub assistant_text: String,
    pub report: Option<String>,
    pub error: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub steps: Vec<PersistStep>,
}

/// W-38: персистит ОДИН ход переписки агента (терминал прогона) через `INSERT OR REPLACE` строки
/// `agent_turns` плюс полную перезапись её шагов (`DELETE` по `run_id` затем пакетный `INSERT`).
/// Идемпотентно по `run_id` (повторный персист того же прогона перезаписывает, не двоит). Best-effort у
/// вызывающего: ошибку он логирует, не роняя прогон. Пустой `session_id` вызывающий НЕ передаёт (UI-путь
/// всегда с сессией).
#[allow(clippy::too_many_arguments)]
pub async fn persist_turn(
    writer: &WriteActor,
    run_id: i64,
    session_id: &str,
    task: &str,
    assistant_text: &str,
    steps: &[PersistStep],
    status: &str,
    report: Option<&str>,
    error: Option<&str>,
    created_at: i64,
) -> DbResult<()> {
    let (session_id, task, assistant_text, status) = (
        session_id.to_string(),
        task.to_string(),
        assistant_text.to_string(),
        status.to_string(),
    );
    let (report, error) = (report.map(str::to_string), error.map(str::to_string));
    let steps = steps.to_vec();
    writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT OR REPLACE INTO agent_turns\
                 (run_id,session_id,task,assistant_text,report,error_text,status,created_at) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    run_id,
                    session_id,
                    task,
                    assistant_text,
                    report,
                    error,
                    status,
                    created_at
                ],
            )?;
            // Полная перезапись шагов хода (идемпотентность: REPLACE строки + чистый набор шагов).
            tx.execute(
                "DELETE FROM agent_turn_steps WHERE run_id=?1",
                params![run_id],
            )?;
            for s in &steps {
                tx.execute(
                    "INSERT INTO agent_turn_steps(run_id,ord,kind,args,title,result,is_error) \
                     VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        run_id,
                        s.ord,
                        s.kind,
                        s.args,
                        s.title,
                        s.result,
                        s.is_error as i64
                    ],
                )?;
            }
            Ok(())
        })
        .await
}

/// W-38: список агент-сессий для левого сайдбара (свежие сверху). Группирует `agent_turns` по
/// `session_id` (агрегаты `MAX(created_at)`/`COUNT`); `title` = задача ПЕРВОГО хода (минимальный
/// `created_at`, тай-брейк по `run_id`), `status` = статус ПОСЛЕДНЕГО хода (максимальный `created_at`,
/// тай-брейк по `run_id`). Title/status берутся коррелированными подзапросами — корректно, без вранья
/// (не «первый попавшийся» из GROUP BY).
pub async fn list_agent_sessions(reader: &ReadPool) -> DbResult<Vec<AgentSessionRow>> {
    reader
        .query(move |c| {
            let mut stmt = c.prepare(
                "SELECT t.session_id, \
                        (SELECT task FROM agent_turns f \
                          WHERE f.session_id=t.session_id \
                          ORDER BY f.created_at ASC, f.run_id ASC LIMIT 1) AS title, \
                        (SELECT status FROM agent_turns l \
                          WHERE l.session_id=t.session_id \
                          ORDER BY l.created_at DESC, l.run_id DESC LIMIT 1) AS status, \
                        COUNT(*) AS cnt, \
                        MAX(t.created_at) AS upd \
                 FROM agent_turns t \
                 GROUP BY t.session_id \
                 ORDER BY upd DESC",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(AgentSessionRow {
                        session_id: r.get(0)?,
                        title: r.get(1)?,
                        status: r.get(2)?,
                        turn_count: r.get(3)?,
                        updated_at: r.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
}

/// W-38: загружает все ходы одной агент-сессии (хронология ASC) с их шагами — для переоткрытия переписки
/// в UI. Шаги читаются вторым запросом и раскладываются по `run_id` (ASC по `ord`).
pub async fn load_agent_session(
    reader: &ReadPool,
    session_id: &str,
) -> DbResult<Vec<PersistedTurnRow>> {
    let session_id = session_id.to_string();
    reader
        .query(move |c| {
            let mut stmt = c.prepare(
                "SELECT run_id, task, assistant_text, report, error_text, status, created_at \
                 FROM agent_turns WHERE session_id=?1 ORDER BY created_at ASC, run_id ASC",
            )?;
            let mut turns = stmt
                .query_map(params![session_id], |r| {
                    Ok(PersistedTurnRow {
                        run_id: r.get(0)?,
                        task: r.get(1)?,
                        assistant_text: r.get(2)?,
                        report: r.get(3)?,
                        error: r.get(4)?,
                        status: r.get(5)?,
                        created_at: r.get(6)?,
                        steps: Vec::new(),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            // Шаги: один запрос на сессию (join по session_id), раскладка по run_id.
            let mut step_stmt = c.prepare(
                "SELECT s.run_id, s.ord, s.kind, s.args, s.title, s.result, s.is_error \
                 FROM agent_turn_steps s \
                 JOIN agent_turns t ON t.run_id = s.run_id \
                 WHERE t.session_id=?1 ORDER BY s.run_id ASC, s.ord ASC",
            )?;
            let steps = step_stmt
                .query_map(params![session_id], |r| {
                    let run_id: i64 = r.get(0)?;
                    let is_error: i64 = r.get(6)?;
                    Ok((
                        run_id,
                        PersistStep {
                            ord: r.get(1)?,
                            kind: r.get(2)?,
                            args: r.get(3)?,
                            title: r.get(4)?,
                            result: r.get(5)?,
                            is_error: is_error != 0,
                        },
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (run_id, step) in steps {
                if let Some(turn) = turns.iter_mut().find(|t| t.run_id == run_id) {
                    turn.steps.push(step);
                }
            }
            Ok(turns)
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
        parent_run_id: r.get(10)?,
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

    /// SUB-0 / миграция 024: `create_child_run` персистит `parent_run_id` и `get_run` его читает;
    /// `create_run` (top-level) оставляет `None` (обратная совместимость).
    #[tokio::test]
    async fn migration_024_parent_run_id_roundtrip() {
        let (_d, db) = open().await;
        let w = db.writer();
        // Top-level: parent_run_id = None.
        let parent = create_run(w, "родитель", Some("qwen"), Some("auto"))
            .await
            .unwrap();
        assert_eq!(
            get_run(db.reader(), parent)
                .await
                .unwrap()
                .unwrap()
                .parent_run_id,
            None,
            "top-level прогон — parent_run_id NULL (back-compat)"
        );
        // Ребёнок: parent_run_id = Some(parent).
        let child = create_child_run(w, "ребёнок", Some("qwen"), Some("auto"), parent)
            .await
            .unwrap();
        let cr = get_run(db.reader(), child).await.unwrap().unwrap();
        assert_eq!(
            cr.parent_run_id,
            Some(parent),
            "ребёнок ссылается на родителя"
        );
        assert_eq!(cr.task, "ребёнок");
        assert_eq!(cr.status, STATUS_QUEUED);
        assert_ne!(child, parent, "у ребёнка свой run_id");
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

    /// SUB-3b-2a фикс #4: reconcile_orphan_child_runs переводит ЗАСТРЯВШИЙ `running` ПРОГОН-РЕБЁНКА
    /// (parent_run_id set) в ТЕРМИНАЛ `error`; top-level `running` (parent NULL) и `done`-ребёнок — НЕ
    /// трогает.
    #[tokio::test]
    async fn reconcile_orphan_child_runs_terminates_only_stale_children() {
        let (_d, db) = open().await;
        let w = db.writer();
        // top-level running (parent NULL) — НЕ должен быть тронут.
        let top = create_run(w, "родитель", None, None).await.unwrap();
        mark_running(w, top).await.unwrap();
        // осиротевший running ребёнок — должен стать error.
        let orphan = create_child_run(w, "ребёнок-сирота", None, None, top)
            .await
            .unwrap();
        mark_running(w, orphan).await.unwrap();
        // завершённый ребёнок — НЕ должен быть тронут (не running).
        let done = create_child_run(w, "ребёнок-готов", None, None, top)
            .await
            .unwrap();
        mark_running(w, done).await.unwrap();
        finish_run(w, done, STATUS_DONE, Some("ок")).await.unwrap();

        // now далеко в будущем + older_than=0 → cutoff в будущем, все running «старые».
        let reconciled = reconcile_orphan_child_runs(w, 0, now_secs() + 10_000)
            .await
            .unwrap();
        assert_eq!(reconciled, 1, "реконсилирован ровно один сирота-ребёнок");

        let o = get_run(db.reader(), orphan).await.unwrap().unwrap();
        assert_eq!(o.status, STATUS_ERROR, "сирота → error");
        assert!(o.outcome.as_deref().unwrap_or("").contains("прервано"));
        assert_eq!(
            get_run(db.reader(), top).await.unwrap().unwrap().status,
            STATUS_RUNNING,
            "top-level (parent NULL) НЕ тронут"
        );
        assert_eq!(
            get_run(db.reader(), done).await.unwrap().unwrap().status,
            STATUS_DONE,
            "завершённый ребёнок НЕ тронут"
        );
    }

    // ── W-38: персист ходов агента (история переписок) ──────────────────────────────────────────

    /// `create_run_in_session` персистит `session_id` (читается через `get_run`); миграция 026
    /// применяется (round-trip персиста хода ниже это докажет — таблицы существуют).
    #[tokio::test]
    async fn create_run_in_session_persists_session_id() {
        let (_d, db) = open().await;
        let w = db.writer();
        let id = create_run_in_session(w, "sess-A", "задача", Some("qwen"), Some("auto"))
            .await
            .unwrap();
        let r = get_run(db.reader(), id).await.unwrap().unwrap();
        assert_eq!(r.session_id.as_deref(), Some("sess-A"));
        assert_eq!(r.task, "задача");
        assert_eq!(r.parent_run_id, None, "top-level прогон");
    }

    /// `persist_turn` round-trip: записанный ход + шаги читаются `load_agent_session` без потерь;
    /// повторный персист того же `run_id` (REPLACE) перезаписывает, не двоит шаги.
    #[tokio::test]
    async fn persist_turn_roundtrips_via_load() {
        let (_d, db) = open().await;
        let w = db.writer();
        let run_id = create_run_in_session(w, "sess-1", "сделай X", None, Some("confirm"))
            .await
            .unwrap();
        let steps = vec![
            PersistStep {
                ord: 0,
                kind: "fs.read".into(),
                args: r#"{"path":"a.md"}"#.into(),
                title: Some("Читает a.md".into()),
                result: Some("ok".into()),
                is_error: false,
            },
            PersistStep {
                ord: 1,
                kind: "note.create".into(),
                args: r#"{"path":"b.md"}"#.into(),
                title: None,
                result: Some("boom".into()),
                is_error: true,
            },
        ];
        persist_turn(
            w,
            run_id,
            "sess-1",
            "сделай X",
            "готов помочь",
            &steps,
            STATUS_DONE,
            Some("итог хода"),
            None,
            1000,
        )
        .await
        .unwrap();

        let turns = load_agent_session(db.reader(), "sess-1").await.unwrap();
        assert_eq!(turns.len(), 1);
        let t = &turns[0];
        assert_eq!(t.run_id, run_id);
        assert_eq!(t.task, "сделай X");
        assert_eq!(t.assistant_text, "готов помочь");
        assert_eq!(t.report.as_deref(), Some("итог хода"));
        assert_eq!(t.error, None);
        assert_eq!(t.status, STATUS_DONE);
        assert_eq!(t.created_at, 1000);
        assert_eq!(t.steps.len(), 2);
        assert_eq!(t.steps[0].kind, "fs.read");
        assert_eq!(t.steps[0].title.as_deref(), Some("Читает a.md"));
        assert!(!t.steps[0].is_error);
        assert_eq!(t.steps[1].kind, "note.create");
        assert!(t.steps[1].is_error, "is_error round-trips");

        // Повторный персист (REPLACE) с ОДНИМ шагом — перезаписывает, не двоит.
        persist_turn(
            w,
            run_id,
            "sess-1",
            "сделай X",
            "перезапись",
            &steps[..1],
            STATUS_DONE,
            Some("новый итог"),
            None,
            1001,
        )
        .await
        .unwrap();
        let turns = load_agent_session(db.reader(), "sess-1").await.unwrap();
        assert_eq!(turns.len(), 1, "ход не задвоился (REPLACE по run_id)");
        assert_eq!(turns[0].assistant_text, "перезапись");
        assert_eq!(turns[0].steps.len(), 1, "шаги перезаписаны (DELETE+INSERT)");
        assert_eq!(turns[0].report.as_deref(), Some("новый итог"));
    }

    /// `list_agent_sessions` группирует по `session_id`, сортирует по свежести (DESC), берёт title из
    /// ПЕРВОГО хода и status из ПОСЛЕДНЕГО; turn_count корректен.
    #[tokio::test]
    async fn list_agent_sessions_groups_and_sorts() {
        let (_d, db) = open().await;
        let w = db.writer();
        // Сессия A: два хода (первый — done, второй — error). created_at 100, 200.
        let a1 = create_run_in_session(w, "A", "первая задача A", None, None)
            .await
            .unwrap();
        persist_turn(
            w,
            a1,
            "A",
            "первая задача A",
            "txt",
            &[],
            STATUS_DONE,
            Some("ok"),
            None,
            100,
        )
        .await
        .unwrap();
        let a2 = create_run_in_session(w, "A", "вторая задача A", None, None)
            .await
            .unwrap();
        persist_turn(
            w,
            a2,
            "A",
            "вторая задача A",
            "txt",
            &[],
            STATUS_ERROR,
            None,
            Some("err"),
            200,
        )
        .await
        .unwrap();
        // Сессия B: один ход, created_at 300 (свежее A) → должна быть ПЕРВОЙ.
        let b1 = create_run_in_session(w, "B", "задача B", None, None)
            .await
            .unwrap();
        persist_turn(
            w,
            b1,
            "B",
            "задача B",
            "txt",
            &[],
            STATUS_DONE,
            Some("ok"),
            None,
            300,
        )
        .await
        .unwrap();

        let sessions = list_agent_sessions(db.reader()).await.unwrap();
        assert_eq!(sessions.len(), 2, "две сессии");
        // Свежесть DESC: B (upd=300) раньше A (upd=200).
        assert_eq!(sessions[0].session_id, "B");
        assert_eq!(sessions[0].turn_count, 1);
        assert_eq!(sessions[0].title, "задача B");
        assert_eq!(sessions[0].status, STATUS_DONE);
        assert_eq!(sessions[0].updated_at, 300);

        assert_eq!(sessions[1].session_id, "A");
        assert_eq!(sessions[1].turn_count, 2);
        assert_eq!(
            sessions[1].title, "первая задача A",
            "title = задача ПЕРВОГО хода"
        );
        assert_eq!(
            sessions[1].status, STATUS_ERROR,
            "status = статус ПОСЛЕДНЕГО хода"
        );
        assert_eq!(sessions[1].updated_at, 200);
    }

    /// `load_agent_session` несуществующей сессии → пусто; ходы возвращаются в хронологии ASC.
    #[tokio::test]
    async fn load_agent_session_orders_ascending() {
        let (_d, db) = open().await;
        let w = db.writer();
        assert!(load_agent_session(db.reader(), "нет")
            .await
            .unwrap()
            .is_empty());
        let r2 = create_run_in_session(w, "S", "t2", None, None)
            .await
            .unwrap();
        persist_turn(w, r2, "S", "t2", "", &[], STATUS_DONE, None, None, 200)
            .await
            .unwrap();
        let r1 = create_run_in_session(w, "S", "t1", None, None)
            .await
            .unwrap();
        persist_turn(w, r1, "S", "t1", "", &[], STATUS_DONE, None, None, 100)
            .await
            .unwrap();
        let turns = load_agent_session(db.reader(), "S").await.unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].task, "t1", "ASC по created_at: 100 раньше 200");
        assert_eq!(turns[1].task, "t2");
    }
}
