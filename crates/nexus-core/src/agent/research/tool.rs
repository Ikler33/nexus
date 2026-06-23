//! RES-4: инструмент `research.run` — запускает оркестратор RES-3 и пишет отчёт в vault ЧЕРЕЗ
//! actuator-гейт (RES-4 [`super::write`]). Регистрируется в `session.rs` ТОЛЬКО при
//! `ai.research.enabled` И `ai.delegation.enabled` И включённом web (структурно инертен иначе). Воркеры
//! (RES-2) read-only по конструкции — пишет ТОЛЬКО этот инструмент-оркестратор, и только через гейт.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use super::orchestrate::{run_research, ResearchParams};
use super::prompts::civil_from_unix;
use super::worker::ResearchWeb;
use super::write::{report_path, write_report};
use crate::actuator::ActionDispatcher;
use crate::agent::event::AgentEvent;
use crate::agent::session::AgentEventForwarder;
use crate::agent::tool::{Tool, ToolError, ToolSpec};
use crate::ai::tools::ToolCapableProvider;
use crate::ai::DelegationConfig;
use crate::net::RunCtx;

/// Зависимости `research.run` (собираются композиционным корнем `session.rs`). Всё за `Arc`/значением —
/// инструмент держит их и зовёт на `invoke`. `web` — seam (боевой `GuardedResearchWeb`, мок в тестах).
pub struct ResearchContext {
    pub web: Arc<dyn ResearchWeb>,
    pub provider: Arc<dyn ToolCapableProvider>,
    /// Тот же gate, что у note-инструментов прогона (общий ledger/policy/blast-cap/kill-switch).
    pub dispatcher: Arc<dyn ActionDispatcher>,
    pub forwarder: Arc<dyn AgentEventForwarder>,
    pub params: ResearchParams,
    /// Для построения `DelegationBudget` (общий wall-deadline ресёрча).
    pub budget_config: DelegationConfig,
    /// Wall-deadline ресёрча; якорится в МОМЕНТ `invoke` (per-invoke), не от старта сессии (ревью #7) —
    /// `research.run` может стартовать спустя N ходов, его дедлайн отсчитывается от вызова.
    pub wall_clock: Duration,
    pub paused: Arc<AtomicBool>,
    pub cancel: Arc<AtomicBool>,
    pub run_id: i64,
}

/// Регистрировать ли `research.run` (default-OFF truth-table). ВСЕ условия обязательны: флаг ресёрча +
/// флаг делегирования (берём оттуда провайдера/капы) + top-level (субагенты не ресёрчат). Presence web/gate
/// гарантируется структурно `if let Some(..)` в `session.rs` (отсутствие любого → инструмента нет).
pub(crate) fn should_register(
    research_enabled: bool,
    delegation_enabled: bool,
    top_level: bool,
) -> bool {
    research_enabled && delegation_enabled && top_level
}

/// `research.run` — многораундовый веб-ресёрч с записью цитированного отчёта в `Research/<slug>-<date>.md`.
pub struct ResearchTool {
    ctx: ResearchContext,
}

impl ResearchTool {
    pub fn new(ctx: ResearchContext) -> Self {
        Self { ctx }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResearchArgs {
    question: String,
    /// Необязательный override числа раундов (клампится оркестратором к `1..=HARD_MAX_ROUNDS`).
    #[serde(default)]
    max_rounds: Option<u8>,
}

/// Текущее время в unix-секундах (UTC) — для заземления дат промптов + имени файла. Вне теста. При
/// (практически невозможном) сбое часов до эпохи — `0` + warn (ревью #5: иначе тихо «1970-01-01»).
fn now_unix_secs() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => {
            tracing::warn!(error = %e, "research: системные часы до эпохи — дата отчёта будет 1970");
            0
        }
    }
}

#[async_trait]
impl Tool for ResearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "research.run".into(),
            description:
                "Глубокий веб-ресёрч по вопросу: декомпозиция → многораундовый поиск/чтение → синтез \
                 цитированного отчёта, сохранённого в заметку Research/. Используй для вопросов, \
                 требующих свежих источников из интернета."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "question": { "type": "string", "description": "Исследовательский вопрос" },
                    "max_rounds": { "type": "integer", "description": "Необяз. лимит раундов (по умолчанию из конфига)" }
                },
                "required": ["question"],
                "additionalProperties": false
            }),
        }
    }

    async fn invoke(&self, args: &str) -> Result<String, ToolError> {
        let a: ResearchArgs =
            serde_json::from_str(args).map_err(|e| ToolError::BadArgs(e.to_string()))?;
        let question = a.question.trim();
        if question.is_empty() {
            return Err(ToolError::BadArgs("пустой вопрос".into()));
        }
        let now = now_unix_secs();
        let (y, m, d) = civil_from_unix(now);
        let date_ymd = format!("{y:04}-{m:02}-{d:02}");

        let mut params = self.ctx.params;
        if let Some(mr) = a.max_rounds {
            params.max_rounds = mr.clamp(1, ResearchParams::HARD_MAX_ROUNDS);
        }
        let budget = crate::agent::delegate::DelegationBudget::from_config(
            &self.ctx.budget_config,
            self.ctx.wall_clock,
        );

        let outcome = run_research(
            self.ctx.web.as_ref(),
            self.ctx.provider.as_ref(),
            question,
            self.ctx.run_id,
            &params,
            &budget,
            &self.ctx.cancel,
            &self.ctx.paused,
            self.ctx.forwarder.as_ref(),
            now,
            RunCtx::run(self.ctx.run_id),
        )
        .await;

        // Нечего сохранять (поиск не дал источников/отчёта) — НЕ пишем пустую заметку.
        if outcome.sources.is_empty() && outcome.report.trim().is_empty() {
            return Ok(format!(
                "Ресёрч не дал результатов (причина: {:?}, раундов: {}). Заметка не создана.",
                outcome.stop_reason, outcome.rounds
            ));
        }

        let path = report_path(question, &date_ymd);
        let gate_summary = write_report(
            self.ctx.dispatcher.as_ref(),
            question,
            &outcome.report,
            outcome.sources.len(),
            &date_ymd,
        )
        .await?;

        // RES-5: отчёт записан через гейт → событие Report (карточка дока правого дока). Эмитим ПОСЛЕ
        // успешной записи (если гейт вернул Err — `?` выше уже вышел, события нет).
        self.ctx.forwarder.forward(&AgentEvent::report(
            self.ctx.run_id,
            question,
            &path,
            outcome.sources.len(),
            outcome.rounds,
        ));

        Ok(format!(
            "Ресёрч завершён ({:?}): {} источник(ов), {} раунд(ов). {}",
            outcome.stop_reason,
            outcome.sources.len(),
            outcome.rounds,
            gate_summary
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actuator::Action;
    use crate::agent::event::AgentEvent;
    use crate::agent::research::worker::WebHit;
    use crate::agent::research::WorkerCfg;
    use crate::agent::tool::ToolSpec as _ToolSpec;
    use crate::agent::web_tools::WebToolsConfig;
    use crate::ai::tools::ToolTurn;
    use crate::ai::ChatMessage;
    use std::sync::Mutex;

    // Mock web: один хит, непустой контент.
    struct MockWeb;
    #[async_trait]
    impl ResearchWeb for MockWeb {
        async fn search(&self, q: &str) -> Result<Vec<WebHit>, String> {
            Ok(vec![WebHit {
                title: format!("Result for {q}"),
                url: "http://ex.com/1".into(),
                snippet: "s".into(),
            }])
        }
        async fn fetch(&self, url: &str) -> Result<String, String> {
            Ok(format!(
                "substantive page content at {url} with useful words about the topic"
            ))
        }
    }

    // Mock provider: план/запрос/синтез/стоп/финал/extract по содержимому промпта.
    struct MockProvider;
    #[async_trait]
    impl ToolCapableProvider for MockProvider {
        async fn stream_chat_tools(
            &self,
            messages: &[ChatMessage],
            _tools: &[_ToolSpec],
            _on_token: &mut (dyn FnMut(String) + Send),
            _cancel: &Arc<AtomicBool>,
            _ctx: RunCtx,
        ) -> crate::ai::AiResult<ToolTurn> {
            let p = messages.last().map(|m| m.content.as_str()).unwrap_or("");
            let r = if p.contains("research strategist") {
                "{\"sub_questions\": [\"sub a\"]}".to_string()
            } else if p.contains("planning web searches") {
                "[\"q1\"]".to_string()
            } else if p.contains("updating an evolving research report") {
                "Evolving report with integrated evidence about the topic.".to_string()
            } else if p.contains("comprehensive enough") {
                "YES — done.".to_string()
            } else if p.contains("long, detailed, comprehensive") {
                "# Final Report\n\nDetailed synthesized answer with citations.".to_string()
            } else if p.contains("extracting evidence") {
                "{\"summary\": \"A substantive finding about the topic from the page.\", \"evidence\": \"q\"}".to_string()
            } else {
                "unknown".to_string()
            };
            Ok(ToolTurn::Final(r))
        }
        fn model_id(&self) -> &str {
            "mock"
        }
    }

    // Recording dispatcher: захватывает Action, возвращает фиктивный summary (как Auto-применение).
    struct RecordingDispatcher(Mutex<Option<Action>>);
    #[async_trait]
    impl ActionDispatcher for RecordingDispatcher {
        async fn apply(&self, action: Action) -> Result<String, ToolError> {
            *self.0.lock().unwrap() = Some(action);
            Ok("создана заметка".to_string())
        }
    }

    struct NoopFwd;
    impl AgentEventForwarder for NoopFwd {
        fn forward(&self, _ev: &AgentEvent) {}
    }

    #[derive(Default)]
    struct RecordingFwd(Mutex<Vec<AgentEvent>>);
    impl AgentEventForwarder for RecordingFwd {
        fn forward(&self, ev: &AgentEvent) {
            self.0.lock().unwrap().push(ev.clone());
        }
    }

    fn ctx(disp: Arc<RecordingDispatcher>) -> ResearchContext {
        ResearchContext {
            web: Arc::new(MockWeb),
            provider: Arc::new(MockProvider),
            dispatcher: disp,
            forwarder: Arc::new(NoopFwd),
            params: ResearchParams {
                max_rounds: 1,
                min_rounds: 1,
                max_empty_rounds: 2,
                max_fanout: 1,
                synthesis_window: 12,
                worker: WorkerCfg {
                    max_urls: 1,
                    max_content_chars: 500,
                    concurrency: 1,
                },
            },
            budget_config: DelegationConfig::default(),
            wall_clock: Duration::from_secs(600),
            paused: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            run_id: 1,
        }
    }

    #[tokio::test]
    async fn report_written_via_dispatch_action_with_frontmatter() {
        let disp = Arc::new(RecordingDispatcher(Mutex::new(None)));
        let fwd = Arc::new(RecordingFwd::default());
        let mut c = ctx(disp.clone());
        c.forwarder = fwd.clone();
        let tool = ResearchTool::new(c);
        let out = tool
            .invoke("{\"question\": \"What is Rust async?\"}")
            .await
            .unwrap();
        assert!(out.contains("источник"), "summary: {out}");
        // прошло ЧЕРЕЗ dispatcher (не сырой fs) note_create с провенансом
        let action = disp.0.lock().unwrap().clone().expect("action recorded");
        let dbg = format!("{action:?}");
        assert!(dbg.contains("Research/"), "путь Research/: {dbg}");
        assert!(
            dbg.contains("source: nexus-deep-research"),
            "frontmatter provenance: {dbg}"
        );
        assert!(dbg.contains("sources_count: 1"), "sources_count: {dbg}");
        // ревью NIT: Report-событие эмитится на УСПЕШНОЙ записи (path совпадает с записанным)
        let evs = fwd.0.lock().unwrap();
        let report = evs
            .iter()
            .find(|e| matches!(e, AgentEvent::Report { .. }))
            .expect("Report event forwarded");
        if let AgentEvent::Report { path, .. } = report {
            assert!(path.starts_with("Research/"), "report path: {path}");
        }
    }

    #[tokio::test]
    async fn empty_research_does_not_write() {
        // web пуст → нет источников → не пишем заметку
        struct EmptyWeb;
        #[async_trait]
        impl ResearchWeb for EmptyWeb {
            async fn search(&self, _q: &str) -> Result<Vec<WebHit>, String> {
                Ok(Vec::new())
            }
            async fn fetch(&self, _u: &str) -> Result<String, String> {
                Ok(String::new())
            }
        }
        let disp = Arc::new(RecordingDispatcher(Mutex::new(None)));
        let mut c = ctx(disp.clone());
        c.web = Arc::new(EmptyWeb);
        let tool = ResearchTool::new(c);
        let out = tool.invoke("{\"question\": \"q\"}").await.unwrap();
        assert!(out.contains("не дал результатов"), "{out}");
        assert!(
            disp.0.lock().unwrap().is_none(),
            "пустой ресёрч НЕ пишет в vault"
        );
    }

    #[test]
    fn should_register_requires_all_conditions() {
        assert!(
            super::should_register(true, true, true),
            "все условия → регистрируем"
        );
        assert!(
            !super::should_register(false, true, true),
            "research выкл → нет"
        );
        assert!(
            !super::should_register(true, false, true),
            "delegation выкл → нет"
        );
        assert!(
            !super::should_register(true, true, false),
            "субагент (не top-level) → нет"
        );
        assert!(
            !super::should_register(false, false, false),
            "всё выкл → нет"
        );
    }

    #[tokio::test]
    async fn rejects_empty_question_and_unknown_field() {
        let disp = Arc::new(RecordingDispatcher(Mutex::new(None)));
        let tool = ResearchTool::new(ctx(disp));
        assert!(tool.invoke("{\"question\": \"  \"}").await.is_err());
        assert!(tool
            .invoke("{\"question\": \"q\", \"bogus\": 1}")
            .await
            .is_err());
    }

    /// RES-5b LIVE Tier-2: ВЕСЬ deep-research пайплайн (RES-1..4) вживую против реального Qwen (.28) +
    /// SearXNG (VPS) → отчёт записан в ВРЕМЕННЫЙ vault через РЕАЛЬНЫЙ actuator-гейт (auto). Доказывает
    /// plan→fan-out→fetch→fence→extract→synthesize→stop→final→note.create end-to-end. Env-gated, в CI
    /// пропускается (как live_agent_web). Запуск на .28:
    /// `NEXUS_LIVE_CHAT=1 cargo test -p nexus-core --lib agent::research::tool::tests::live_research_pipeline -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "live: нужны Qwen :8080 + SearXNG (NEXUS_LIVE_CHAT=1, NEXUS_LIVE_CHAT_URL/MODEL, NEXUS_LIVE_SEARX_URL)"]
    async fn live_research_pipeline() {
        use crate::actuator::{
            AuditSink, DispatchPolicy, GatedToolCtx, PolicyDefault, TracingEventSink,
        };
        use crate::ai::tools::OpenAiToolProvider;
        use crate::db::Database;
        use crate::net::{EgressAudit, EgressFeature, EgressPolicy, GuardedClient};
        use std::time::Duration;
        use tempfile::TempDir;

        if std::env::var("NEXUS_LIVE_CHAT").ok().as_deref() != Some("1") {
            eprintln!("SKIP: NEXUS_LIVE_CHAT!=1");
            return;
        }
        let chat_url =
            std::env::var("NEXUS_LIVE_CHAT_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into());
        let model =
            std::env::var("NEXUS_LIVE_CHAT_MODEL").unwrap_or_else(|_| "qwen3.6-27b-mtp".into());
        let searx = std::env::var("NEXUS_LIVE_SEARX_URL")
            .unwrap_or_else(|_| "http://89.127.211.153:8888".into());
        let searx_host = reqwest::Url::parse(&searx)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .expect("searx host");

        let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
        policy.set_feature_enabled(EgressFeature::Chat, true);
        policy.set_feature_enabled(EgressFeature::Web, true);
        policy.set_scoped_allowlist("web", [searx_host]);
        let audit = Arc::new(EgressAudit::default());
        let gc = GuardedClient::for_chat(policy, audit, Duration::from_secs(30)).unwrap();
        let provider: Arc<dyn ToolCapableProvider> = Arc::new(OpenAiToolProvider::new(
            &gc,
            EgressFeature::Chat,
            &chat_url,
            &model,
            Some(0.2),
        ));
        let web = WebToolsConfig {
            client: gc.clone(),
            searxng_url: Some(searx),
        };

        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join("t.db")).await.unwrap();
        let canon = dir.path().to_path_buf();
        // with_paused (как durable-джоба/прод-гейт), не new — упражняем kill-switch-проводку гейта.
        let gate = GatedToolCtx::new(
            canon.clone(),
            AuditSink::new(db.writer().clone(), db.reader().clone()),
            1,
            DispatchPolicy::with_paused(
                Some("auto"),
                64 * 1024,
                64,
                Arc::new(AtomicBool::new(false)),
            ),
            Arc::new(PolicyDefault),
            Arc::new(TracingEventSink::new()),
        );
        let ctx = ResearchContext {
            web: Arc::new(crate::agent::research::worker::GuardedResearchWeb::new(
                web,
                RunCtx::run(1),
                false,
            )),
            provider,
            dispatcher: Arc::new(gate),
            forwarder: Arc::new(NoopFwd),
            params: ResearchParams {
                max_rounds: 2,
                min_rounds: 1,
                max_empty_rounds: 2,
                max_fanout: 2,
                synthesis_window: 8,
                worker: WorkerCfg {
                    max_urls: 2,
                    max_content_chars: 8000,
                    concurrency: 2,
                },
            },
            budget_config: DelegationConfig {
                enabled: true,
                max_fanout: 2,
                ..Default::default()
            },
            wall_clock: Duration::from_secs(300),
            paused: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            run_id: 1,
        };
        let out = ResearchTool::new(ctx)
            .invoke("{\"question\": \"What is the Rust borrow checker and why does it matter?\"}")
            .await
            .unwrap();
        eprintln!("LIVE research outcome: {out}");
        // отчёт записан в Research/ временного vault'а
        let research_dir = canon.join("Research");
        let wrote = std::fs::read_dir(&research_dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .any(|e| e.path().extension().is_some_and(|x| x == "md"))
            })
            .unwrap_or(false);
        assert!(
            wrote,
            "отчёт .md записан в {research_dir:?}; summary: {out}"
        );
    }
}
