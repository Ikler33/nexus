//! RES-5b: durable scheduler-джоба deep-research (`KIND_DEEP_RESEARCH`). СТАНДАЛОН-ресёрч (без agent-loop):
//! прямой прогон оркестратора RES-3 → запись отчёта через гейт RES-4 → Report-событие RES-5a, но как
//! ДОЛГОЖИВУЩАЯ джоба (строка `agent_runs` переживает рестарт, отменяема, уступает интерактиву). Отличие от
//! `research.run`-инструмента (синхронный, в agent-loop): эта джоба — fire-and-forget фоновый ресёрч.
//!
//! PARTIAL-SAVE наследуется: [`super::orchestrate::run_research`] возвращает ПАРТИАЛ при deadline/cancel, а
//! [`super::tool::ResearchTool`] всё равно пишет непустой отчёт через гейт. Default-OFF: handle финиширует
//! `error`, если research/web/actuator не сконфигурированы (инструмент-джоба структурно инертна).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use super::tool::{ResearchContext, ResearchTool};
use super::worker::GuardedResearchWeb;
use super::ResearchParams;
use crate::actuator::{
    ActionDispatcher, AuditSink, DecisionSource, DispatchPolicy, GatedToolCtx, TracingEventSink,
};
use crate::agent::event::AgentEvent;
use crate::agent::run_store;
use crate::agent::session::AgentEventForwarder;
use crate::agent::tool::Tool;
use crate::agent::web_tools::WebToolsConfig;
use crate::ai::AIClient;
use crate::db::{ReadPool, WriteActor};
use crate::net::RunCtx;
use crate::scheduler::{self, Job, JobHandler};

/// Kind durable-джобы deep-research (по нему `claim_next_handled` выгребает только её).
pub const KIND_DEEP_RESEARCH: &str = "deep_research";
/// Дефолт wall-clock фонового ресёрча (сек), если `ai.research.wall_clock_secs=0`.
const DEFAULT_WALL_CLOCK_SECS: u64 = 1800;
/// Клампы wall-clock (анти-overflow / минимальная осмысленность).
const WALL_CLOCK_MIN: u64 = 60;
const WALL_CLOCK_MAX: u64 = 86_400;
/// Задержка пере-кью под kill-switch паузой.
const PAUSE_REQUEUE_DELAY_SECS: i64 = 30;

/// Forwarder durable-джобы: логирует Report (наблюдаемость; стрима в headless нет). PlanProposed/Step —
/// в tracing на debug-уровне не льём (фон).
struct JobForwarder;
impl AgentEventForwarder for JobForwarder {
    fn forward(&self, ev: &AgentEvent) {
        if let AgentEvent::Report {
            run_id,
            path,
            sources_count,
            rounds,
            ..
        } = ev
        {
            tracing::info!(run_id, %path, sources_count, rounds, "deep_research: отчёт записан");
        }
    }
}

/// Хендлер `KIND_DEEP_RESEARCH`. Строит per-run гейт (как `run_agent_session` внутри) + [`ResearchContext`]
/// и зовёт [`ResearchTool`] напрямую (без agent-loop). Поля — снимок конфигурации agentd на старте.
pub struct DeepResearchHandler {
    writer: WriteActor,
    reader: ReadPool,
    ai: Arc<AIClient>,
    canon_root: PathBuf,
    actuator_enabled: bool,
    overwrite_threshold: usize,
    blast_cap: u32,
    decision_source: Arc<dyn DecisionSource>,
    agent_paused: Arc<AtomicBool>,
    web: Option<WebToolsConfig>,
    research: crate::ai::ResearchConfig,
    /// Капы fan-out/бюджета (research использует deadline + max_fanout; субагентов НЕ спавнит).
    delegation: crate::ai::DelegationConfig,
}

impl DeepResearchHandler {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        writer: WriteActor,
        reader: ReadPool,
        ai: Arc<AIClient>,
        canon_root: PathBuf,
        actuator_enabled: bool,
        overwrite_threshold: usize,
        blast_cap: u32,
        decision_source: Arc<dyn DecisionSource>,
        agent_paused: Arc<AtomicBool>,
        web: Option<WebToolsConfig>,
        research: crate::ai::ResearchConfig,
        delegation: crate::ai::DelegationConfig,
    ) -> Self {
        Self {
            writer,
            reader,
            ai,
            canon_root,
            actuator_enabled,
            overwrite_threshold,
            blast_cap,
            decision_source,
            agent_paused,
            web,
            research,
            delegation,
        }
    }

    /// Прогон одной durable-джобы (идемпотентно: терминальный run → no-op). КРЭШ-семантика (ревью): при
    /// рестарте незавершённый run пере-claim'ится и ресёрч идёт С НУЛЯ (НЕ resume); если файл-отчёт от
    /// первой попытки уже лежит (тот же slug+date) — note.create fail-closed → run `error` (без дублей/
    /// data-loss). Т.е. crash-recovery здесь = «чисто упасть, оператор пере-триггерит», не «возобновить».
    async fn drive(&self, run_id: i64) -> Result<(), String> {
        let run = run_store::get_run(&self.reader, run_id)
            .await
            .map_err(|e| format!("deep_research {run_id}: чтение прогона: {e}"))?;
        let Some(run) = run else {
            tracing::warn!(
                run_id,
                "deep_research: строки прогона нет — пропуск (no-op)"
            );
            return Ok(());
        };
        if run_store::is_terminal(&run.status) {
            return Ok(()); // replay-safe
        }
        run_store::mark_running(&self.writer, run_id)
            .await
            .map_err(|e| format!("deep_research {run_id}: mark_running: {e}"))?;

        // Предусловия (fail-closed): research включён + web + actuator + провайдер. Иначе чисто финишируем
        // error (НЕ сбой джобы — джоба отработала, прогон помечен error).
        let precond = self.research.enabled
            && self.web.is_some()
            && self.actuator_enabled
            && self.ai.agent_tools.is_some();
        if !precond {
            let msg = "deep research недоступен (нужны ai.research.enabled + web + actuator + agent_tools)";
            run_store::finish_run(&self.writer, run_id, run_store::STATUS_ERROR, Some(msg))
                .await
                .map_err(|e| format!("deep_research {run_id}: finish(error): {e}"))?;
            tracing::warn!(run_id, "deep_research: предусловия не выполнены → error");
            return Ok(());
        }
        let provider = self
            .ai
            .agent_tools
            .clone()
            .expect("precond: agent_tools Some");
        let web = self.web.clone().expect("precond: web Some");

        // Per-run гейт — тот же, что строит run_agent_session (общий ledger/policy/blast/kill-switch).
        // autonomy из строки прогона: headless `auto` применяет Auto-тир (note.create новой заметки), под
        // `confirm` PolicyDefault auto-DENY → отчёт НЕ запишется (предложение без аппрувера).
        let policy = DispatchPolicy::with_paused(
            run.autonomy.as_deref(),
            self.overwrite_threshold,
            self.blast_cap,
            self.agent_paused.clone(),
        );
        let gate = GatedToolCtx::new(
            self.canon_root.clone(),
            AuditSink::new(self.writer.clone(), self.reader.clone()),
            run_id,
            policy,
            self.decision_source.clone(),
            Arc::new(TracingEventSink::new()),
        );
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(gate);

        let wall = {
            let s = if self.research.wall_clock_secs == 0 {
                DEFAULT_WALL_CLOCK_SECS
            } else {
                self.research.wall_clock_secs
            };
            Duration::from_secs(s.clamp(WALL_CLOCK_MIN, WALL_CLOCK_MAX))
        };
        let ctx = ResearchContext {
            web: Arc::new(GuardedResearchWeb::new(web, RunCtx::run(run_id), false)),
            provider,
            dispatcher,
            forwarder: Arc::new(JobForwarder),
            params: ResearchParams::from_config(&self.research, self.delegation.max_fanout),
            budget_config: self.delegation.clone(),
            wall_clock: wall,
            paused: self.agent_paused.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
            run_id,
        };
        let tool = ResearchTool::new(ctx);
        let args = serde_json::json!({ "question": run.task }).to_string();

        // ResearchTool сам прогоняет run_research (партиал на deadline/cancel) + пишет отчёт через гейт +
        // эмитит Report. Err инструмента → прогон error; Ok → done с summary.
        match tool.invoke(&args).await {
            Ok(summary) => {
                run_store::finish_run(&self.writer, run_id, run_store::STATUS_DONE, Some(&summary))
                    .await
                    .map_err(|e| format!("deep_research {run_id}: finish(done): {e}"))?
            }
            Err(e) => run_store::finish_run(
                &self.writer,
                run_id,
                run_store::STATUS_ERROR,
                Some(&format!("deep_research: {e:?}")),
            )
            .await
            .map_err(|e| format!("deep_research {run_id}: finish(error): {e}"))?,
        };
        Ok(())
    }
}

#[async_trait]
impl JobHandler for DeepResearchHandler {
    async fn handle(&self, job: &Job) -> Result<(), String> {
        let run_id: i64 =
            job.payload.trim().parse().map_err(|e| {
                format!("deep_research: payload не run_id ('{}'): {e}", job.payload)
            })?;

        // KILL-SWITCH чек-пойнт #1: пауза → прогон остаётся queued, пере-кью на un-pause (зеркало agent_run).
        if self.agent_paused.load(Ordering::Relaxed) {
            let still_pending = matches!(
                run_store::get_run(&self.reader, run_id).await,
                Ok(Some(run)) if !run_store::is_terminal(&run.status)
            );
            if still_pending {
                scheduler::enqueue(
                    &self.writer,
                    KIND_DEEP_RESEARCH,
                    &run_id.to_string(),
                    scheduler::now_secs() + PAUSE_REQUEUE_DELAY_SECS,
                    job.max_attempts,
                )
                .await
                .map_err(|e| format!("deep_research {run_id}: пере-кью под паузой: {e}"))?;
            }
            return Ok(());
        }

        self.drive(run_id).await
    }

    fn defer_under_interactive(&self) -> bool {
        // Фоновый ресёрч — тяжёлый LLM+web-фон, уступает интерактивному чату (S5 backpressure).
        true
    }
}

/// Ставит durable deep-research в очередь: строка `agent_runs` (task=вопрос, autonomy) + джоба
/// `KIND_DEEP_RESEARCH` payload=run_id. `autonomy=Some("auto")` нужен, чтобы headless реально записал отчёт
/// (под confirm PolicyDefault отклонит). Возвращает run_id.
pub async fn enqueue_deep_research(
    writer: &WriteActor,
    question: &str,
    autonomy: Option<&str>,
) -> crate::db::DbResult<i64> {
    let run_id = run_store::create_run(writer, question, None, autonomy).await?;
    scheduler::enqueue(
        writer,
        KIND_DEEP_RESEARCH,
        &run_id.to_string(),
        scheduler::now_secs(),
        2,
    )
    .await?;
    Ok(run_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn handler(
        db: &Database,
        research_enabled: bool,
        web: bool,
        actuator: bool,
    ) -> DeepResearchHandler {
        let ai = Arc::new(AIClient {
            chat: None,
            chat_fast: None,
            chat_util: None,
            embedder: None,
            agent_tools: None, // нет провайдера → предусловие провалится (для precond-тестов достаточно)
            policy: Arc::new(crate::net::EgressPolicy::new(Arc::new(AtomicBool::new(
                false,
            )))),
        });
        let research = crate::ai::ResearchConfig {
            enabled: research_enabled,
            ..Default::default()
        };
        let web_cfg = web.then(|| WebToolsConfig {
            client: crate::net::GuardedClient::for_web(
                Arc::new(crate::net::EgressPolicy::new(Arc::new(AtomicBool::new(
                    false,
                )))),
                Arc::new(crate::net::EgressAudit::default()),
                Duration::from_secs(5),
            )
            .unwrap(),
            searxng_url: Some("http://searx.example:8888".into()),
        });
        DeepResearchHandler::new(
            db.writer().clone(),
            db.reader().clone(),
            ai,
            std::env::temp_dir(),
            actuator,
            crate::actuator::OVERWRITE_THRESHOLD,
            crate::ai::AiConfig::DEFAULT_BLAST_RADIUS_CAP,
            Arc::new(crate::actuator::PolicyDefault),
            Arc::new(AtomicBool::new(false)),
            web_cfg,
            research,
            crate::ai::DelegationConfig::default(),
        )
    }

    #[tokio::test]
    async fn precond_fail_finishes_error_not_job_failure() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Database::open(dir.path().join("t.db")).await.unwrap();
        // research выключен → предусловие провалено → прогон error, джоба Ok (не сбой)
        let h = handler(&db, false, true, true);
        let run_id = enqueue_deep_research(db.writer(), "What is X?", Some("auto"))
            .await
            .unwrap();
        let job = scheduler::claim_next_handled(
            db.writer(),
            scheduler::now_secs() + 5,
            &[KIND_DEEP_RESEARCH.to_string()],
        )
        .await
        .unwrap()
        .expect("job claimed");
        assert!(h.handle(&job).await.is_ok(), "джоба отработала чисто");
        let run = run_store::get_run(db.reader(), run_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            run.status,
            run_store::STATUS_ERROR,
            "прогон → error (предусловие)"
        );
    }

    #[tokio::test]
    async fn enqueue_creates_run_and_job() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Database::open(dir.path().join("t.db")).await.unwrap();
        let run_id = enqueue_deep_research(db.writer(), "Best Rust crates?", Some("auto"))
            .await
            .unwrap();
        let run = run_store::get_run(db.reader(), run_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.task, "Best Rust crates?");
        assert_eq!(run.status, run_store::STATUS_QUEUED);
        let due = scheduler::claim_next_handled(
            db.writer(),
            scheduler::now_secs() + 5,
            &[KIND_DEEP_RESEARCH.to_string()],
        )
        .await
        .unwrap();
        assert!(due.is_some(), "джоба KIND_DEEP_RESEARCH в очереди");
        assert_eq!(due.unwrap().payload, run_id.to_string());
    }

    #[tokio::test]
    async fn defer_under_interactive_true() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Database::open(dir.path().join("t.db")).await.unwrap();
        assert!(handler(&db, true, true, true).defer_under_interactive());
    }
}
