//! `spawn_subagent` (SUB-3b-1) — примитив порождения ОДНОГО субагента: второй (вложенный) вызов
//! [`run_agent_session`] с изоляцией контекста (`memory=None`), СУЖЕННЫМ реестром (child ⊆ parent через
//! [`super::build_child_registry`] + [`SessionRole::Subagent`]), ОБЩИМ kill-switch (`paused` тот же Arc) и
//! fail-closed списанием [`DelegationBudget`]. НЕ инструмент и НЕ fan-out — это строительный блок, который
//! `DelegateTool` (SUB-3b-2) гоняет конкурентно через `JoinSet`.
//!
//! Контекст РОДИТЕЛЯ ([`SubagentContext`]) держит ВЛАДЕЕМЫЕ клоны/Arc (provider/writer/reader/forwarder/
//! dispatcher/budget/…), чтобы fan-out (3b-2) клонировал его в каждую конкурентную задачу. Изоляция:
//! ребёнок стартует с `memory=None` (ни recall фактов, ни история родителя не протекают); РОДИТЕЛЬ видит
//! ТОЛЬКО краткое саммари (никаких промежуточных tool-call/токенов ребёнка) — контракт hermes.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::actuator::{ActionDispatcher, DecisionSource};
use crate::agent::event::{AgentEvent, SubagentState};
use crate::agent::run_store::{self, STATUS_DONE, STATUS_ERROR};
use crate::agent::runner::{BudgetKind, LoopOutcome};
use crate::agent::session::{
    run_agent_session, AgentEventForwarder, SessionDeps, SessionRole, SessionSpec,
};
use crate::agent::skill_tools::SkillContext;
use crate::agent::web_tools::WebToolsConfig;
use crate::ai::tools::ToolCapableProvider;
use crate::db::{ReadPool, WriteActor};

use super::budget::DelegationBudget;
use super::child_task::build_child_task;
use super::registry::build_child_registry;

/// ВЛАДЕЕМЫЙ контекст РОДИТЕЛЯ для порождения субагентов (общий на все спавны прогона). Все поля
/// `Clone`/`Arc` — fan-out (3b-2) клонирует его в каждую конкурентную задачу. Строится `DelegateTool`
/// (SUB-3b-2) из хендлов прогона.
#[derive(Clone)]
pub struct SubagentContext {
    /// Провайдер модели (тот же, что у родителя) — `Arc`, чтобы конкурентные задачи владели клоном.
    pub provider: Arc<dyn ToolCapableProvider>,
    /// Каталог скиллов (если у родителя есть) — наследуется (сужение реестра отфильтрует лишнее).
    pub skills: Option<SkillContext>,
    /// Веб-инструменты (если включены) — для read-only research-воркеров (RES-*).
    pub web: Option<WebToolsConfig>,
    /// Источник решений гейта (тот же, что у родителя).
    pub decision_source: Arc<dyn DecisionSource>,
    pub writer: WriteActor,
    pub reader: ReadPool,
    /// ГЛОБАЛЬНЫЙ kill-switch — ТОТ ЖЕ Arc, что у родителя (пауза останавливает и детей).
    pub paused: Arc<AtomicBool>,
    /// Cancel родителя — ребёнок СИДИТСЯ его текущим значением (живую проводку родитель→ребёнок ведёт
    /// fan-out в 3b-2; здесь — стартовый снимок, плюс `paused` halt'ит в любом случае).
    pub parent_cancel: Arc<AtomicBool>,
    /// Форвардер событий (тот же) — `SubagentStatus`/события ребёнка уходят родительским транспортом.
    pub forwarder: Arc<dyn AgentEventForwarder>,
    /// run_id РОДИТЕЛЯ (для `parent_run_id` дерева).
    pub parent_run_id: i64,
    /// ИМЕНА инструментов родителя — база сужения [`build_child_registry`] (child ⊆ parent).
    pub parent_tool_names: BTreeSet<String>,
    /// Общий с родителем actuator-gate (общий blast-radius/ledger). `None` → ребёнок строит свой.
    pub dispatcher: Option<Arc<dyn ActionDispatcher>>,
    /// Наследуемые spec-поля родителя.
    pub actuator_enabled: bool,
    pub autonomy: Option<String>,
    pub overwrite_threshold: usize,
    pub blast_cap: u32,
    pub context_window: Option<usize>,
    pub canon_root: PathBuf,
    pub model: Option<String>,
    /// ОБЩИЙ анти-runaway бюджет дерева (списывается fail-closed перед спавном).
    pub budget: DelegationBudget,
}

/// Итог одного субагента (то, что fan-out агрегирует в ответ родителю).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentResult {
    /// run_id порождённого ребёнка (`0`, если спавн отклонён бюджетом — строка не создавалась).
    pub child_run_id: i64,
    /// Терминальный статус ребёнка.
    pub state: SubagentState,
    /// КРАТКОЕ саммари (Final-текст / partial+причина / текст ошибки) — единственное, что видит родитель.
    pub summary: String,
}

/// Форвардер РЕБЁНКА (ревью SUB-3b-1 MAJOR — изоляция контекста). ГЛУШИТ АНОНИМНЫЕ события цикла ребёнка
/// (`AssistantToken`/`ToolCall`/`ToolResult`/`Final`/`Error`/`ContextUsage` — у них НЕТ `run_id`, и на
/// per-parent-run канале потребителя они приписались бы РОДИТЕЛЮ как ЕГО собственные ходы/финал — прямое
/// нарушение «родитель видит ТОЛЬКО саммари»). ПРОПУСКАЕТ id-несущие (gate `Proposal`/`Diff`/`Exec*`,
/// `Plan*`, `SubagentStatus` — они атрибутируемы по run_id). Итог ребёнка родитель получает как
/// `SubagentStatus{Done, summary}` (эмитит `spawn_subagent` НАПРЯМУЮ через родительский форвардер, минуя
/// эту обёртку) и как [`SubagentResult::summary`].
struct SubagentForwarder(Arc<dyn AgentEventForwarder>);

impl AgentEventForwarder for SubagentForwarder {
    fn forward(&self, ev: &AgentEvent) {
        match ev {
            // Анонимные ходы ребёнка → ГЛУШИМ (не приписываем родителю).
            AgentEvent::AssistantToken(_)
            | AgentEvent::ToolCall { .. }
            | AgentEvent::ToolResult { .. }
            | AgentEvent::Final(_)
            | AgentEvent::Error(_)
            | AgentEvent::ContextUsage { .. } => {}
            // id-несущие (gate-предложения/планы/статусы) — пропускаем (атрибутируемы по run_id).
            other => self.0.forward(other),
        }
    }
}

/// Свернуть [`LoopOutcome`] ребёнка в `(статус, краткое-саммари)`. Родитель НИКОГДА не видит
/// промежуточных ходов — только это.
fn collapse(outcome: LoopOutcome) -> (SubagentState, String) {
    match outcome {
        LoopOutcome::Final(s) => (SubagentState::Done, s),
        LoopOutcome::BudgetExhausted {
            kind: BudgetKind::Paused,
            partial,
        } => (
            SubagentState::Paused,
            if partial.trim().is_empty() {
                "(субагент остановлен паузой)".to_string()
            } else {
                partial
            },
        ),
        LoopOutcome::BudgetExhausted { kind, partial } => (
            SubagentState::Failed,
            format!("(субагент: исчерпан бюджет {kind:?}) {partial}")
                .trim()
                .to_string(),
        ),
        LoopOutcome::Error(e) => (SubagentState::Failed, format!("(субагент: ошибка) {e}")),
    }
}

/// Порождает ОДНОГО субагента под задачу `goal` (+ опц. `context`, опц. сужающий `requested` toolset).
///
/// Порядок (fail-closed): (1) **списать спавн из общего бюджета** — исчерпан (depth/spawns/deadline) →
/// `Failed` БЕЗ создания строки/прогона; (2) `create_child_run(parent_run_id)` (дерево) + `Spawned`-событие;
/// (3) реестр ребёнка = `build_child_registry(parent ∩ requested минус блок-лист)`; (4) cancel-снимок
/// родителя; (5) `run_agent_session` с `memory=None` + [`SessionRole::Subagent`] (сужение/общий gate); (6) свернуть
/// исход → саммари, финализировать строку ребёнка, эмитнуть терминальный `SubagentStatus`. Возвращает
/// [`SubagentResult`] (родитель видит ТОЛЬКО саммари).
pub async fn spawn_subagent(
    ctx: &SubagentContext,
    goal: &str,
    context: Option<&str>,
    requested: Option<&[String]>,
) -> SubagentResult {
    // (1) Fail-closed списание спавна из ОБЩЕГО бюджета дерева — ДО любого побочного эффекта.
    if let Err(e) = ctx.budget.check_then_acquire_spawn() {
        return SubagentResult {
            child_run_id: 0,
            state: SubagentState::Failed,
            summary: format!("делегирование отклонено: {e}"),
        };
    }

    // (2) Строка прогона-ребёнка (дерево parent_run_id) + событие Spawned.
    let child_run_id = match run_store::create_child_run(
        &ctx.writer,
        goal,
        ctx.model.as_deref(),
        ctx.autonomy.as_deref(),
        ctx.parent_run_id,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            return SubagentResult {
                child_run_id: 0,
                state: SubagentState::Failed,
                summary: format!("не удалось создать прогон субагента: {e}"),
            };
        }
    };
    ctx.forwarder.forward(&AgentEvent::subagent_status(
        ctx.parent_run_id,
        child_run_id,
        goal,
        SubagentState::Spawned,
        None,
    ));
    let _ = run_store::mark_running(&ctx.writer, child_run_id).await;

    // (3) Сужаем реестр ребёнка (child ⊆ parent минус блок-лист). build_child_registry → Vec; в набор
    // для SessionRole::Subagent.allowed (retain принимает BTreeSet).
    let allowed: BTreeSet<String> = build_child_registry(&ctx.parent_tool_names, requested)
        .into_iter()
        .collect();
    // (4) Снимок cancel родителя на старте (паузой `paused` halt'ит в любом случае; живую проводку
    //     cancel родитель→ребёнок ведёт fan-out в 3b-2).
    let child_cancel = Arc::new(AtomicBool::new(ctx.parent_cancel.load(Ordering::Relaxed)));

    let child_spec = SessionSpec {
        run_id: child_run_id,
        task: build_child_task(goal, context),
        autonomy: ctx.autonomy.clone(),
        actuator_enabled: ctx.actuator_enabled,
        overwrite_threshold: ctx.overwrite_threshold,
        blast_cap: ctx.blast_cap,
        context_window: ctx.context_window,
        canon_root: ctx.canon_root.clone(),
        history: Vec::new(), // дети — свежий one-shot подзадачи (без истории родителя)
        skills_learning_enabled: false, // дети навыки не авторствуют
    };
    // (5) Вложенный прогон с ИЗОЛЯЦИЕЙ (memory=None) + ОБЩИМ paused. Форвардер РЕБЁНКА обёрнут
    //     SubagentForwarder'ом: анонимные ходы ребёнка НЕ протекают в родительский поток (ревью MAJOR).
    //     Роль Subagent: сужение реестра/общий gate; каналы delegation/research у ребёнка непредставимы
    //     типом (рекурсия-стоп: delegate.run/research.run не регистрируются ребёнку по построению).
    let child_forwarder: Arc<dyn AgentEventForwarder> =
        Arc::new(SubagentForwarder(ctx.forwarder.clone()));
    let outcome = run_agent_session(
        &child_spec,
        &SessionDeps {
            provider: ctx.provider.as_ref(),
            memory: None, // изоляция: ни recall фактов, ни история родителя не протекают
            skills: ctx.skills.as_ref(),
            web: ctx.web.as_ref(),
            decision_source: ctx.decision_source.clone(),
            writer: &ctx.writer,
            reader: &ctx.reader,
            paused: &ctx.paused,
            cancel: &child_cancel,
            forwarder: child_forwarder,
        },
        SessionRole::Subagent {
            allowed: &allowed,
            dispatcher: ctx.dispatcher.clone(),
        },
    )
    .await;

    // (6) Свернуть → саммари, финализировать строку ребёнка (run-lifecycle — забота вызывающего; здесь
    //     вызывающий = spawn_subagent). Прогон-ребёнок ЭФЕМЕРЕН (не возобновляемый per-child) → Paused/Failed
    //     закрываем как error-терминал; Done → done.
    let (state, summary) = collapse(outcome);
    let status = if state == SubagentState::Done {
        STATUS_DONE
    } else {
        STATUS_ERROR
    };
    let _ = run_store::finish_run(&ctx.writer, child_run_id, status, Some(&summary)).await;
    ctx.forwarder.forward(&AgentEvent::subagent_status(
        ctx.parent_run_id,
        child_run_id,
        goal,
        state,
        Some(&summary),
    ));

    SubagentResult {
        child_run_id,
        state,
        summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actuator::PolicyDefault;
    use crate::agent::tool::{ToolCall, ToolSpec};
    use crate::ai::tools::{ToolCapableProvider, ToolTurn};
    use crate::ai::{AiResult, ChatMessage};
    use crate::db::Database;
    use crate::net::RunCtx;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use std::time::Duration;
    use tempfile::TempDir;

    /// Форвардер-сборщик событий.
    #[derive(Default)]
    struct Collect {
        events: Mutex<Vec<AgentEvent>>,
    }
    impl AgentEventForwarder for Collect {
        fn forward(&self, ev: &AgentEvent) {
            self.events.lock().unwrap().push(ev.clone());
        }
    }

    /// Фейк-провайдер скриптованных ходов.
    struct Fake {
        turns: Mutex<std::collections::VecDeque<AiResult<ToolTurn>>>,
    }
    impl Fake {
        fn new(turns: Vec<AiResult<ToolTurn>>) -> Arc<Self> {
            Arc::new(Self {
                turns: Mutex::new(turns.into_iter().collect()),
            })
        }
    }
    #[async_trait]
    impl ToolCapableProvider for Fake {
        async fn stream_chat_tools(
            &self,
            _m: &[ChatMessage],
            _t: &[ToolSpec],
            _on: &mut (dyn FnMut(String) + Send),
            _c: &Arc<AtomicBool>,
            _ctx: RunCtx,
        ) -> AiResult<ToolTurn> {
            self.turns
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(ToolTurn::Final("(no more)".into())))
        }
        fn model_id(&self) -> &str {
            "fake"
        }
    }

    async fn db() -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join("t.db")).await.unwrap();
        (dir, db)
    }

    fn ctx(
        db: &Database,
        dir: &TempDir,
        provider: Arc<dyn ToolCapableProvider>,
        forwarder: Arc<dyn AgentEventForwarder>,
        parent_tool_names: BTreeSet<String>,
        budget: DelegationBudget,
    ) -> SubagentContext {
        SubagentContext {
            provider,
            skills: None,
            web: None,
            decision_source: Arc::new(PolicyDefault),
            writer: db.writer().clone(),
            reader: db.reader().clone(),
            paused: Arc::new(AtomicBool::new(false)),
            parent_cancel: Arc::new(AtomicBool::new(false)),
            forwarder,
            parent_run_id: 1,
            parent_tool_names,
            dispatcher: None,
            actuator_enabled: false,
            autonomy: None,
            overwrite_threshold: 100,
            blast_cap: 10,
            context_window: Some(4096),
            canon_root: dir.path().to_path_buf(),
            model: Some("fake".into()),
            budget,
        }
    }

    fn big_budget() -> DelegationBudget {
        DelegationBudget::new(1, 8, 3, Duration::from_secs(3600))
    }

    /// Happy-path: ребёнок Final → Done + саммари; строка ребёнка персистится с parent_run_id; родитель
    /// получает ТОЛЬКО события статуса (Spawned + Done), а не промежуточные ходы ребёнка как свои.
    #[tokio::test]
    async fn spawn_child_returns_summary_and_persists_lineage() {
        let (dir, db) = db().await;
        let provider = Fake::new(vec![Ok(ToolTurn::Final("сделано".into()))]);
        let fwd = Arc::new(Collect::default());
        let names: BTreeSet<String> = ["debug.echo".into()].into_iter().collect();
        let c = ctx(&db, &dir, provider, fwd.clone(), names, big_budget());
        let res = spawn_subagent(&c, "найди X", None, None).await;
        assert_eq!(res.state, SubagentState::Done);
        assert_eq!(res.summary, "сделано");
        assert!(res.child_run_id > 0);

        let row = run_store::get_run(db.reader(), res.child_run_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.parent_run_id, Some(1), "lineage parent_run_id");
        assert_eq!(row.status, STATUS_DONE);
        assert_eq!(row.task, "найди X", "DB task = сырая цель");

        // ИЗОЛЯЦИЯ (ревью MAJOR): родитель видит ТОЛЬКО SubagentStatus[Spawned, Done]; анонимные ходы
        // ребёнка (Final/AssistantToken/ToolCall/ToolResult/ContextUsage) НЕ протекают в его поток.
        let evs = fwd.events.lock().unwrap();
        let statuses: Vec<_> = evs
            .iter()
            .filter_map(|e| match e {
                AgentEvent::SubagentStatus { status, .. } => Some(*status),
                _ => None,
            })
            .collect();
        assert_eq!(statuses, vec![SubagentState::Spawned, SubagentState::Done]);
        let leaked = evs.iter().any(|e| {
            matches!(
                e,
                AgentEvent::Final(_)
                    | AgentEvent::AssistantToken(_)
                    | AgentEvent::ToolCall { .. }
                    | AgentEvent::ToolResult { .. }
                    | AgentEvent::ContextUsage { .. }
            )
        });
        assert!(
            !leaked,
            "анонимные ходы ребёнка НЕ должны протечь в родительский поток"
        );
    }

    /// Fail-closed бюджет: исчерпанные спавны → Failed БЕЗ создания строки прогона (агент_runs не растёт).
    #[tokio::test]
    async fn spawn_budget_denied_creates_no_run() {
        let (dir, db) = db().await;
        let provider = Fake::new(vec![]);
        let fwd = Arc::new(Collect::default());
        let names: BTreeSet<String> = BTreeSet::new();
        // Бюджет с 0 спавнов (нормализуется к 1 в new → используем acquire заранее, чтобы исчерпать).
        let budget = DelegationBudget::new(1, 1, 3, Duration::from_secs(3600));
        budget.check_then_acquire_spawn().unwrap(); // исчерпали единственный спавн
        let c = ctx(&db, &dir, provider, fwd.clone(), names, budget);
        let res = spawn_subagent(&c, "цель", None, None).await;
        assert_eq!(res.state, SubagentState::Failed);
        assert_eq!(res.child_run_id, 0, "строка прогона НЕ создана");
        assert!(res.summary.contains("отклонено"));
        let n: i64 = db
            .reader()
            .query(|conn| conn.query_row("SELECT count(*) FROM agent_runs", [], |r| r.get(0)))
            .await
            .unwrap();
        assert_eq!(n, 0, "ни одной строки agent_runs не создано");
    }

    /// Kill-switch: `paused` ВКЛ до спавна → дочерний цикл сразу Paused (на границе), статус Paused,
    /// vault не трогается. `paused` — ТОТ ЖЕ Arc, что у родителя.
    #[tokio::test]
    async fn spawn_paused_child_halts() {
        let (dir, db) = db().await;
        // Провайдер не понадобится — пауза остановит до хода. Но дадим Final на всякий.
        let provider = Fake::new(vec![Ok(ToolTurn::Final("не должно".into()))]);
        let fwd = Arc::new(Collect::default());
        let names: BTreeSet<String> = ["debug.echo".into()].into_iter().collect();
        let mut c = ctx(&db, &dir, provider, fwd.clone(), names, big_budget());
        c.paused = Arc::new(AtomicBool::new(true)); // пауза ДО спавна
        let res = spawn_subagent(&c, "цель", None, None).await;
        assert_eq!(
            res.state,
            SubagentState::Paused,
            "пауза → ребёнок Paused (kill-switch)"
        );
        assert!(res.child_run_id > 0, "строка создана, но цикл не прошёл");
    }

    /// Рекурсия-стоп end-to-end: родитель «владеет» delegate.run, но ребёнок его НЕ получает (blocklist).
    /// Ребёнок зовёт delegate.run → UnknownTool (его нет в реестре ребёнка) → НИКАКОЙ внук не создаётся.
    /// Наблюдаем по дереву прогонов: после спавна ОДНОГО ребёнка в `agent_runs` ровно 1 строка (нет внука).
    /// (Внутренний is_error ребёнка НЕ протекает в родителя — изоляция; поэтому проверяем по БД.)
    #[tokio::test]
    async fn spawn_child_cannot_call_delegate_recursively() {
        let (dir, db) = db().await;
        let provider = Fake::new(vec![
            Ok(ToolTurn::ToolCalls(vec![ToolCall {
                id: "c1".into(),
                name: "delegate.run".into(),
                arguments: "{}".into(),
            }])),
            Ok(ToolTurn::Final("ок".into())),
        ]);
        let fwd = Arc::new(Collect::default());
        // Родитель «владеет» delegate.run + debug.echo.
        let names: BTreeSet<String> = ["debug.echo".into(), "delegate.run".into()]
            .into_iter()
            .collect();
        let c = ctx(&db, &dir, provider, fwd.clone(), names, big_budget());
        let res = spawn_subagent(&c, "попробуй делегировать", None, None).await;
        assert_eq!(
            res.state,
            SubagentState::Done,
            "ребёнок завершился (delegate.run внутри — no-op)"
        );
        // Дерево: РОВНО одна строка (ребёнок), внука нет — рекурсия структурно заблокирована.
        let n: i64 = db
            .reader()
            .query(|conn| conn.query_row("SELECT count(*) FROM agent_runs", [], |r| r.get(0)))
            .await
            .unwrap();
        assert_eq!(
            n, 1,
            "создан только ребёнок; внук НЕ порождён (delegate.run у ребёнка отсутствует)"
        );
    }
}
