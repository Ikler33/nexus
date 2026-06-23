//! RES-3: оркестратор IterResearch (порт odysseus `IterResearch.research()`) — склейка RES-1 (промпты/
//! парсеры) + RES-2 (read-only воркер). Цикл: `decompose → [раунд: gen-queries → fan-out воркеров →
//! collect+quality-dedup → synthesize] → should_stop → final-report`. Производит отчёт В ПАМЯТИ + список
//! источников; **запись в vault — RES-4** (здесь возврат, не запись → срез ревьюится изолированно).
//!
//! Бюджет/безопасность: общий [`DelegationBudget`] (wall-deadline), `cancel`/`paused` короткозамыкают с
//! ВОЗВРАТОМ ПАРТИАЛА (порт odysseus timeout/error-ветвей), `max_rounds`-кап, `max_empty_rounds` (поиск
//! «лёг» → выходим). Запросы раунда — ПОСЛЕДОВАТЕЛЬНО (каждый сам fan-out'ит URL'ы; так конкурентных
//! extract'ов не больше `concurrency`, а не queries×concurrency — критично на одном GPU). LLM-вызовы plan/
//! query/synth/stop/final — без инструментов (`stream_chat_tools` с пустым tools → Final). Контент уже
//! фенсится воркером (RES-2) ДО extract.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::Mutex;

use super::query::{dedup_new_queries, normalize_query, parse_queries};
use super::worker::{research_query, ResearchWeb, WorkerCfg};
use super::{
    build_final_report_prompt, build_plan_prompt, build_query_prompt, build_stop_prompt,
    build_synthesize_prompt, dedup_findings_by_url, normalize_url, parse_plan, parse_stop, Finding,
};
use crate::agent::delegate::DelegationBudget;
use crate::agent::event::{AgentEvent, PlanStep, PlanStepState};
use crate::agent::session::AgentEventForwarder;
use crate::ai::tools::{ToolCapableProvider, ToolTurn};
use crate::ai::ResearchConfig;
use crate::ai::{fence_observation, injection_marker, ChatMessage};
use crate::net::RunCtx;

/// Почему остановился цикл (для наблюдаемости/тестов).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Модель решила, что отчёт исчерпывающий (`should_stop`).
    Completed,
    /// Достигнут потолок раундов.
    MaxRounds,
    /// Подряд `max_empty_rounds` раундов без новых находок (поиск «лёг»).
    EmptyRounds,
    /// Истёк wall-deadline бюджета.
    Deadline,
    /// Отмена прогона (возвращён партиал).
    Cancelled,
    /// Пауза прогона kill-switch'ем (возвращён партиал; отличаем от Cancelled — ревью #6, зеркало
    /// `BudgetKind::Paused` основного цикла).
    Paused,
}

/// Итог ресёрча В ПАМЯТИ (RES-4 запишет `report` в vault через actuator-гейт).
#[derive(Debug, Clone)]
pub struct ResearchOutcome {
    pub report: String,
    pub sources: Vec<Finding>,
    pub rounds: usize,
    pub stop_reason: StopReason,
}

/// Параметры цикла (RES-4 строит из [`ResearchConfig`] через [`ResearchParams::from_config`]).
#[derive(Debug, Clone, Copy)]
pub struct ResearchParams {
    /// Потолок раундов (жёсткий; `0` → клампится к 1).
    pub max_rounds: u8,
    /// Минимум раундов до того, как `should_stop` может остановить (не стопимся слишком рано).
    pub min_rounds: u8,
    /// Подряд пустых раундов → выход (поиск недоступен/иссяк).
    pub max_empty_rounds: u8,
    /// Запросов на раунд (= подвопросов плана; кап делегирования).
    pub max_fanout: usize,
    /// Сколько находок раунда подаётся в synthesize (окно — анти-токен-флуд).
    pub synthesis_window: usize,
    /// Параметры воркера (URL/контент/конкурентность).
    pub worker: WorkerCfg,
}

impl ResearchParams {
    /// Жёсткий потолок раундов (defense-in-depth поверх конфига) — порт odysseus «hard cap ~8».
    pub const HARD_MAX_ROUNDS: u8 = 8;
    const DEFAULT_MIN_ROUNDS: u8 = 1;
    const DEFAULT_MAX_EMPTY_ROUNDS: u8 = 2;
    const DEFAULT_SYNTHESIS_WINDOW: usize = 12;

    /// Из конфига: клампит `max_rounds` к `1..=HARD_MAX_ROUNDS`; `max_fanout` берёт из делегирования (общий
    /// fan-out-кап); воркер — из `ai.research`.
    pub fn from_config(cfg: &ResearchConfig, max_fanout: usize) -> Self {
        Self {
            max_rounds: cfg.max_rounds.clamp(1, Self::HARD_MAX_ROUNDS),
            min_rounds: Self::DEFAULT_MIN_ROUNDS,
            max_empty_rounds: Self::DEFAULT_MAX_EMPTY_ROUNDS,
            max_fanout: max_fanout.max(1),
            synthesis_window: Self::DEFAULT_SYNTHESIS_WINDOW,
            worker: WorkerCfg {
                max_urls: cfg.max_urls_per_round.max(1),
                max_content_chars: cfg.max_content_chars,
                concurrency: cfg.extraction_concurrency.max(1),
            },
        }
    }
}

/// Завершён ли прогон извне (отмена/пауза/дедлайн)? Объединяет три условия выхода-с-партиалом.
fn should_halt(
    cancel: &Arc<AtomicBool>,
    paused: &Arc<AtomicBool>,
    budget: &DelegationBudget,
) -> Option<StopReason> {
    // Пауза (kill-switch) и отмена различаются (ревью #6): паузу проверяем первой.
    if paused.load(Ordering::Relaxed) {
        return Some(StopReason::Paused);
    }
    if cancel.load(Ordering::Relaxed) {
        return Some(StopReason::Cancelled);
    }
    if budget.deadline_exceeded() {
        return Some(StopReason::Deadline);
    }
    None
}

/// Один LLM-ход без инструментов → финальный текст (None при ошибке/неожиданных tool-calls). Plan/query/
/// synth/stop/final все идут через него.
async fn complete(
    provider: &dyn ToolCapableProvider,
    prompt: String,
    cancel: &Arc<AtomicBool>,
    ctx: RunCtx,
) -> Option<String> {
    let messages = [ChatMessage::user(prompt)];
    let mut sink = |_t: String| {};
    // Различаем причины None (ревью #11): провайдер-ошибка vs неожиданные tool_calls на бес-инструментном
    // ходе — обе → None (fail-soft, партиал), но логируем по-разному для диагностики «застрявшего» провайдера.
    match provider
        .stream_chat_tools(&messages, &[], &mut sink, cancel, ctx)
        .await
    {
        Ok(ToolTurn::Final(t)) => Some(t),
        Ok(ToolTurn::ToolCalls(_)) => {
            tracing::warn!(
                "research: провайдер вернул tool_calls на бес-инструментном ходе — игнор"
            );
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, "research: LLM-ход не удался — продолжаем с партиалом");
            None
        }
    }
}

/// Находки → текст для synthesize/financial-report промптов (нумерованные блоки с URL).
fn findings_to_text(findings: &[Finding]) -> String {
    findings
        .iter()
        .enumerate()
        .map(|(i, f)| {
            format!(
                "[{}] {} ({})\n{}\n{}",
                i + 1,
                f.title,
                f.url,
                f.summary,
                f.evidence
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Прогнать deep-research цикл. `now_secs` — unix UTC для заземления дат в промптах (инъекция → тест-
/// детерминизм). Возвращает отчёт В ПАМЯТИ + источники. НЕ пишет в vault (RES-4).
#[allow(clippy::too_many_arguments)]
pub async fn run_research(
    web: &dyn ResearchWeb,
    provider: &dyn ToolCapableProvider,
    question: &str,
    run_id: i64,
    params: &ResearchParams,
    budget: &DelegationBudget,
    cancel: &Arc<AtomicBool>,
    paused: &Arc<AtomicBool>,
    forwarder: &dyn AgentEventForwarder,
    now_secs: i64,
    ctx: RunCtx,
) -> ResearchOutcome {
    let mut sources: Vec<Finding> = Vec::new();
    let mut report = String::new();
    // Один per-run anti-injection marker: находки (summary/evidence) — это вывод модели НАД недоверенным
    // веб-контентом, поэтому при ВЛОЖЕНИИ обратно в synthesize/final промпты их фенсим (ревью MAJOR:
    // вредоносная страница могла бы заставить воркер вернуть summary с инъекцией в синтез).
    let marker = injection_marker();

    if let Some(reason) = should_halt(cancel, paused, budget) {
        return ResearchOutcome {
            report,
            sources,
            rounds: 0,
            stop_reason: reason,
        };
    }

    // 1) DECOMPOSE: план → подвопросы. Fail-soft: пустой/мусорный ответ → [вопрос] (parse_plan).
    let plan_resp = complete(provider, build_plan_prompt(question, now_secs), cancel, ctx)
        .await
        .unwrap_or_default();
    let plan = parse_plan(&plan_resp, question, params.max_fanout);

    // PlanProposed: подвопросы как шаги (стабильные id q0..qN). Прогресс — PlanStepStatus по раундам.
    let steps: Vec<PlanStep> = plan
        .sub_questions
        .iter()
        .enumerate()
        .map(|(i, q)| PlanStep {
            id: format!("q{i}"),
            label: q.clone(),
            status: PlanStepState::Pending,
        })
        .collect();
    forwarder.forward(&AgentEvent::PlanProposed {
        run_id,
        steps: steps.clone(),
    });
    let plan_text = format!(
        "Sub-questions:\n{}\nKey topics: {}\nSuccess: {}",
        plan.sub_questions.join("\n- "),
        plan.key_topics.join(", "),
        plan.success_criteria
    );

    // 2) Итеративные раунды.
    let mut used_queries: HashSet<String> = HashSet::new();
    let mut empty_streak: u8 = 0;
    let mut rounds_done: usize = 0;
    let mut stop_reason = StopReason::MaxRounds;
    // Defense-in-depth: повторно клампим (from_config уже клампит, но прямой конструктор ResearchParams
    // мог бы выставить больше HARD_MAX_ROUNDS — ревью NIT #6).
    let max_rounds = params.max_rounds.clamp(1, ResearchParams::HARD_MAX_ROUNDS);

    for round in 1..=max_rounds {
        if let Some(reason) = should_halt(cancel, paused, budget) {
            stop_reason = reason;
            break;
        }
        // прогресс плана: помечаем шаг этого раунда Running (если в пределах списка)
        let step_idx = (round as usize).saturating_sub(1);
        if let Some(s) = steps.get(step_idx) {
            forwarder.forward(&AgentEvent::PlanStepStatus {
                id: s.id.clone(),
                status: PlanStepState::Running,
            });
        }

        // 2a) QUERY-GEN. Раунд 1 fallback: пустые запросы → берём сами подвопросы плана.
        let query_resp = complete(
            provider,
            build_query_prompt(
                question,
                &plan_text,
                &report,
                round as u32,
                params.max_fanout,
                now_secs,
            ),
            cancel,
            ctx,
        )
        .await
        .unwrap_or_default();
        let mut round_queries = dedup_new_queries(parse_queries(&query_resp), &used_queries);
        if round_queries.is_empty() && round == 1 {
            round_queries = dedup_new_queries(plan.sub_questions.clone(), &used_queries);
        }
        round_queries.truncate(params.max_fanout);

        if round_queries.is_empty() {
            // нет новых запросов → пустой раунд (поиск иссяк/дубли). Шаг этого раунда → Done (не оставляем
            // Running через continue — ревью: каждый Running должен иметь парный Done).
            if let Some(s) = steps.get(step_idx) {
                forwarder.forward(&AgentEvent::PlanStepStatus {
                    id: s.id.clone(),
                    status: PlanStepState::Done,
                });
            }
            empty_streak += 1;
            rounds_done = round as usize;
            if empty_streak >= params.max_empty_rounds {
                stop_reason = StopReason::EmptyRounds;
                break;
            }
            continue;
        }
        // В used кладём ТОЛЬКО фактически искомые (после truncate) — сверх-сгенерённые запросы остаются
        // допустимыми в следующих раундах по дизайну (ревью NIT #8).
        for q in &round_queries {
            used_queries.insert(normalize_query(q));
        }

        // 2b) FAN-OUT воркеров (ПОСЛЕДОВАТЕЛЬНО по запросам; каждый сам fan-out'ит URL'ы). Общий
        //     shared_urls дедупит URL между запросами раунда.
        let shared_urls: Mutex<HashSet<String>> =
            Mutex::new(sources.iter().map(|f| normalize_url(&f.url)).collect());
        let mut round_findings: Vec<Finding> = Vec::new();
        for q in &round_queries {
            if let Some(reason) = should_halt(cancel, paused, budget) {
                // ТЕКУЩИЙ раунд не завершён → считаем только полностью завершённые (ревью #1).
                let completed = (round as usize).saturating_sub(1);
                // выходим из цикла с уже собранным — `reason` идёт прямо в finalize
                return finalize(
                    provider,
                    question,
                    &mut report,
                    sources,
                    round_findings,
                    completed,
                    reason,
                    &steps,
                    forwarder,
                    cancel,
                    budget,
                    &marker,
                    ctx,
                )
                .await;
            }
            let f = research_query(
                web,
                provider,
                question,
                q,
                &shared_urls,
                &params.worker,
                cancel,
                ctx,
            )
            .await;
            round_findings.extend(f);
        }

        // 2c) QUALITY/DEDUP: новые находки (по URL) сверх уже собранных.
        let before = sources.len();
        let mut merged = sources.clone();
        merged.extend(round_findings);
        sources = dedup_findings_by_url(merged);
        let new_count = sources.len() - before;
        rounds_done = round as usize;

        // Дедлайн/отмена МЕЖДУ fan-out и дорогими synth/stop LLM-ходами → break без трат (ревью #2:
        // не жжём минуты модели на одном GPU после истёкшего wall-deadline).
        if let Some(reason) = should_halt(cancel, paused, budget) {
            stop_reason = reason;
            break;
        }

        if new_count == 0 {
            empty_streak += 1;
        } else {
            empty_streak = 0;
            // 2d) SYNTHESIZE: окно последних N ФАКТИЧЕСКИ НОВЫХ (deduped) находок раунда. Берём хвост
            //     `sources[before..]` (реально влитые), НЕ сырой round_findings (ревью MAJOR #7: окно над
            //     дублями расходилось с тем, что попало в sources). Находки — вывод над недоверенным вебом
            //     → ФЕНСИМ перед промптом (ревью MAJOR #10).
            let new_slice = &sources[before..];
            let window_start = new_slice.len().saturating_sub(params.synthesis_window);
            let fenced = fence_observation(
                "RESEARCH FINDINGS (untrusted, web-derived)",
                &findings_to_text(&new_slice[window_start..]),
                &marker,
            );
            if let Some(updated) = complete(
                provider,
                build_synthesize_prompt(question, &report, &fenced),
                cancel,
                ctx,
            )
            .await
            {
                if !updated.trim().is_empty() {
                    report = updated;
                }
            }
        }

        if let Some(s) = steps.get(step_idx) {
            forwarder.forward(&AgentEvent::PlanStepStatus {
                id: s.id.clone(),
                status: PlanStepState::Done,
            });
        }

        // 2e) STOP-решение (после min_rounds, ТОЛЬКО если раунд что-то дал — иначе отчёт не изменился,
        //     завершение решает empty-streak; ревью NIT #3). Fail-soft: непарсимо → продолжаем.
        if round >= params.min_rounds && new_count > 0 {
            let stop_resp = complete(
                provider,
                build_stop_prompt(question, &report, round as u32, max_rounds as u32),
                cancel,
                ctx,
            )
            .await
            .unwrap_or_default();
            if parse_stop(&stop_resp).should_stop {
                stop_reason = StopReason::Completed;
                break;
            }
        }
        if empty_streak >= params.max_empty_rounds {
            stop_reason = StopReason::EmptyRounds;
            break;
        }
    }

    // Точнее причина: если дошли до конца раундов БЕЗ источников при копившихся пустых раундах — это
    // «поиск лёг», а не «исчерпан кап» (ревью NIT #4; важно когда max_empty_rounds > max_rounds).
    if stop_reason == StopReason::MaxRounds && sources.is_empty() && empty_streak > 0 {
        stop_reason = StopReason::EmptyRounds;
    }

    finalize(
        provider,
        question,
        &mut report,
        sources,
        Vec::new(),
        rounds_done,
        stop_reason,
        &steps,
        forwarder,
        cancel,
        budget,
        &marker,
        ctx,
    )
    .await
}

/// Финализация: дотянуть последние находки (если выходили по halt в середине раунда), сгенерить финальный
/// длинный отчёт (если есть источники), отметить незавершённые шаги Done, собрать [`ResearchOutcome`].
#[allow(clippy::too_many_arguments)]
async fn finalize(
    provider: &dyn ToolCapableProvider,
    question: &str,
    report: &mut String,
    mut sources: Vec<Finding>,
    trailing_findings: Vec<Finding>,
    rounds: usize,
    stop_reason: StopReason,
    steps: &[PlanStep],
    forwarder: &dyn AgentEventForwarder,
    cancel: &Arc<AtomicBool>,
    budget: &DelegationBudget,
    marker: &str,
    ctx: RunCtx,
) -> ResearchOutcome {
    if !trailing_findings.is_empty() {
        let mut merged = sources.clone();
        merged.extend(trailing_findings);
        sources = dedup_findings_by_url(merged);
    }
    // Финальный длинный отчёт — есть источники, НЕ отменено И не истёк дедлайн (ревью #2: иначе самый
    // дорогой LLM-ход всё равно жёгся после deadline).
    if !sources.is_empty() && !cancel.load(Ordering::Relaxed) && !budget.deadline_exceeded() {
        // База — отчёт (вывод модели), либо, если пусто, фенсенные находки (web-derived → I-5, ревью #10).
        let fenced_evidence;
        let base = if report.trim().is_empty() {
            fenced_evidence = fence_observation(
                "RESEARCH FINDINGS (untrusted, web-derived)",
                &findings_to_text(&sources),
                marker,
            );
            &fenced_evidence
        } else {
            report.as_str()
        };
        if let Some(final_report) = complete(
            provider,
            build_final_report_prompt(question, base),
            cancel,
            ctx,
        )
        .await
        {
            if !final_report.trim().is_empty() {
                *report = final_report;
            }
        }
    }
    // Все шаги → Done (наблюдаемость; ленту не врём «застрял»).
    for s in steps {
        forwarder.forward(&AgentEvent::PlanStepStatus {
            id: s.id.clone(),
            status: PlanStepState::Done,
        });
    }
    ResearchOutcome {
        report: std::mem::take(report),
        sources,
        rounds,
        stop_reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::research::worker::WebHit;
    use crate::agent::tool::ToolSpec;
    use crate::ai::AiResult;
    use async_trait::async_trait;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    // ── Mock web: URL'ы ЗАВИСЯТ от запроса (разные запросы → разные URL → новые находки каждый раунд,
    //    как в реальности). `empty=true` → поиск «лёг». ──────────────────────────────────────────────
    struct MockWeb {
        per_query: usize,
        empty: bool,
    }
    #[async_trait]
    impl ResearchWeb for MockWeb {
        async fn search(&self, q: &str) -> Result<Vec<WebHit>, String> {
            if self.empty {
                return Ok(Vec::new());
            }
            Ok((0..self.per_query)
                .map(|i| WebHit {
                    title: format!("T-{q}-{i}"),
                    url: format!("http://ex.com/{q}/{i}"),
                    snippet: "s".into(),
                })
                .collect())
        }
        async fn fetch(&self, url: &str) -> Result<String, String> {
            Ok(format!(
                "substantive page content for {url} with many useful words about the topic"
            ))
        }
    }

    // ── Mock provider: диспетчеризует по содержимому промпта; считает stop-вызовы ─────────────────
    struct MockProvider {
        n_sub: usize,       // сколько подвопросов в плане
        stop_yes_at: usize, // на каком stop-вызове вернуть YES (usize::MAX = всегда NO)
        stop_calls: AtomicUsize,
        last_synth: Mutex<String>,
    }
    impl MockProvider {
        fn new(n_sub: usize, stop_yes_at: usize) -> Self {
            Self {
                n_sub,
                stop_yes_at,
                stop_calls: AtomicUsize::new(0),
                last_synth: Mutex::new(String::new()),
            }
        }
    }
    #[async_trait]
    impl ToolCapableProvider for MockProvider {
        async fn stream_chat_tools(
            &self,
            messages: &[ChatMessage],
            _tools: &[ToolSpec],
            _on_token: &mut (dyn FnMut(String) + Send),
            _cancel: &Arc<AtomicBool>,
            _ctx: RunCtx,
        ) -> AiResult<ToolTurn> {
            let p = messages.last().map(|m| m.content.as_str()).unwrap_or("");
            let resp = if p.contains("research strategist") {
                // план: n_sub подвопросов
                let qs: Vec<String> = (0..self.n_sub)
                    .map(|i| format!("\"sub-question {i}\""))
                    .collect();
                format!(
                    "{{\"sub_questions\": [{}], \"success_criteria\": \"cover all\"}}",
                    qs.join(", ")
                )
            } else if p.contains("planning web searches") {
                // round-specific запросы (парсим «**Round:** N») → не дедупятся между раундами
                let round = p
                    .split("**Round:**")
                    .nth(1)
                    .and_then(|s| s.split_whitespace().next())
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(1);
                format!("[\"q{round}a\", \"q{round}b\"]")
            } else if p.contains("updating an evolving research report") {
                *self.last_synth.lock().await = p.to_string();
                "Updated evolving report with the new evidence integrated.".to_string()
            } else if p.contains("comprehensive enough") {
                let n = self.stop_calls.fetch_add(1, Ordering::SeqCst) + 1;
                if n >= self.stop_yes_at {
                    "YES — comprehensive.".to_string()
                } else {
                    "NO — keep going.".to_string()
                }
            } else if p.contains("long, detailed, comprehensive") {
                "# Final Report\n\nA thorough synthesized answer.".to_string()
            } else if p.contains("extracting evidence") {
                // воркер extract → fenced-JSON finding
                "{\"summary\": \"A substantive finding about the topic discovered on the page.\", \"evidence\": \"quote\"}".to_string()
            } else {
                "unknown".to_string()
            };
            Ok(ToolTurn::Final(resp))
        }
        fn model_id(&self) -> &str {
            "mock"
        }
    }

    struct NoopFwd;
    impl AgentEventForwarder for NoopFwd {
        fn forward(&self, _ev: &AgentEvent) {}
    }

    fn params(max_rounds: u8) -> ResearchParams {
        ResearchParams {
            max_rounds,
            min_rounds: 1,
            max_empty_rounds: 2,
            max_fanout: 3,
            synthesis_window: 12,
            worker: WorkerCfg {
                max_urls: 3,
                max_content_chars: 500,
                concurrency: 2,
            },
        }
    }
    fn big_budget() -> DelegationBudget {
        // (max_depth, max_total_spawns, max_fanout_per_call, wall_clock) — RES-3 читает только deadline.
        DelegationBudget::new(2, 100, 4, Duration::from_secs(600))
    }

    async fn run(
        web: &MockWeb,
        prov: &MockProvider,
        p: &ResearchParams,
        budget: &DelegationBudget,
        cancel: Arc<AtomicBool>,
        paused: Arc<AtomicBool>,
    ) -> ResearchOutcome {
        run_research(
            web,
            prov,
            "What is X?",
            1,
            p,
            budget,
            &cancel,
            &paused,
            &NoopFwd,
            1_782_172_800,
            RunCtx::NONE,
        )
        .await
    }

    #[tokio::test]
    async fn loop_stops_on_should_stop_after_min_rounds() {
        let web = MockWeb {
            per_query: 3,
            empty: false,
        };
        let prov = MockProvider::new(3, 1); // YES на первом stop-вызове
        let p = params(5);
        let out = run(
            &web,
            &prov,
            &p,
            &big_budget(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        )
        .await;
        assert_eq!(out.stop_reason, StopReason::Completed);
        assert_eq!(
            out.rounds, 1,
            "остановились после 1-го раунда (min_rounds=1, YES)"
        );
        assert!(!out.sources.is_empty(), "есть источники");
        assert!(
            out.report.contains("Final Report"),
            "финальный отчёт сгенерён"
        );
    }

    #[tokio::test]
    async fn loop_stops_on_max_rounds_cap() {
        let web = MockWeb {
            per_query: 3,
            empty: false,
        };
        let prov = MockProvider::new(3, usize::MAX); // всегда NO
        let p = params(3);
        let out = run(
            &web,
            &prov,
            &p,
            &big_budget(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        )
        .await;
        assert_eq!(out.stop_reason, StopReason::MaxRounds);
        assert_eq!(out.rounds, 3);
    }

    #[tokio::test]
    async fn loop_breaks_on_empty_rounds_search_down() {
        let web = MockWeb {
            per_query: 3,
            empty: true,
        }; // поиск пуст → находок нет
        let prov = MockProvider::new(3, usize::MAX);
        let p = params(5);
        let out = run(
            &web,
            &prov,
            &p,
            &big_budget(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        )
        .await;
        assert_eq!(out.stop_reason, StopReason::EmptyRounds);
        assert!(
            out.rounds <= 2,
            "вышли по пустым раундам рано (max_empty=2): {}",
            out.rounds
        );
        assert!(out.sources.is_empty());
    }

    #[tokio::test]
    async fn loop_breaks_on_wall_deadline() {
        let web = MockWeb {
            per_query: 3,
            empty: false,
        };
        let prov = MockProvider::new(3, usize::MAX);
        let p = params(5);
        let past = DelegationBudget::new(2, 100, 4, Duration::ZERO); // дедлайн ≈ сейчас → истёк
        let out = run(
            &web,
            &prov,
            &p,
            &past,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        )
        .await;
        assert_eq!(out.stop_reason, StopReason::Deadline);
        assert_eq!(out.rounds, 0, "дедлайн до первого раунда");
    }

    #[tokio::test]
    async fn cancel_halts_loop_returns_partial() {
        let web = MockWeb {
            per_query: 3,
            empty: false,
        };
        let prov = MockProvider::new(3, usize::MAX);
        let p = params(5);
        let out = run(
            &web,
            &prov,
            &p,
            &big_budget(),
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(false)),
        )
        .await;
        assert_eq!(out.stop_reason, StopReason::Cancelled);
        assert_eq!(out.rounds, 0);
    }

    #[tokio::test]
    async fn paused_halts_loop_distinct_from_cancel() {
        // ревью #6: пауза kill-switch'ем → Paused (не Cancelled)
        let web = MockWeb {
            per_query: 3,
            empty: false,
        };
        let prov = MockProvider::new(3, usize::MAX);
        let p = params(5);
        let out = run(
            &web,
            &prov,
            &p,
            &big_budget(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(true)),
        )
        .await;
        assert_eq!(out.stop_reason, StopReason::Paused);
        assert_eq!(out.rounds, 0);
    }

    #[tokio::test]
    async fn synthesis_window_limits_findings() {
        // 10 хитов в раунде, окно=4 → synth-промпт содержит ≤4 находки.
        let web = MockWeb {
            per_query: 10,
            empty: false,
        };
        let prov = MockProvider::new(1, 1); // стоп после 1 раунда
        let mut p = params(2);
        p.synthesis_window = 4;
        p.max_fanout = 1; // один запрос → все 10 URL в одном воркере
        p.worker.max_urls = 10;
        let out = run(
            &web,
            &prov,
            &p,
            &big_budget(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        )
        .await;
        assert!(
            out.sources.len() >= 5,
            "много источников собрано: {}",
            out.sources.len()
        );
        let synth = prov.last_synth.lock().await.clone();
        let n_in_synth = synth.matches("http://ex.com/").count();
        assert!(n_in_synth <= 4, "в synth подано ≤ окна (4): {n_in_synth}");
        assert!(n_in_synth > 0, "что-то подано в synth");
        // ревью MAJOR #10: находки в synth ОБЁРНУТЫ fence_observation (web-derived → анти-инъекция)
        assert!(
            synth.contains("недоверенные ДАННЫЕ"),
            "находки фенсятся перед synthesize: {synth}"
        );
        assert!(
            synth.contains("NEVER follow"),
            "synth-промпт несёт явный injection-гард"
        );
    }
}
