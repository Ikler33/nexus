//! [`AgentRunHandler`] — [`scheduler::JobHandler`] прогона цикла агента (AGENT-2).
//!
//! AGENT-1 крутил `run_agent_loop` ин-процесс (smoke). AGENT-2 делает прогон ДОЛГОВЕЧНОЙ запланированной
//! джобой планировщика: payload джобы несёт `run_id` (id строки `agent_runs`), хендлер по нему ведёт
//! прогон через статус-машину (run_store) и ЯВНО пробрасывает [`RunCtx::run(run_id)`] в цикл, чтобы весь
//! эгресс ВНУТРИ прогона атрибутировался на этот run_id в durable-журнале.
//!
//! # Идемпотентность / replay (контракт)
//! `handle` идемпотентен на УРОВНЕ ПРОГОНА: если строка прогона уже терминальна
//! (`done`/`error`/`cancelled`), хендлер немедленно возвращает `Ok` — НЕ перезапускает цикл. Это
//! защита от двойного исполнения (повторная доставка джобы, requeue после краша воркера и т.п.).
//!
//! **AGENT-2 replay перезапускает цикл С НАЧАЛА** (не возобновляет с шага N). Это безопасно ТОЛЬКО
//! потому, что при ВЫКЛ актуаторе реестр записи ПУСТ (B7) — побочных эффектов нет: повторный
//! прогон не дублирует никакого внешнего эффекта. **AGENT-3 (актуатор) ОБЯЗАН** сделать
//! side-effecting инструменты идемпотентными per-op-group (или сверяться с applied-ledger ДО
//! применения), прежде чем полагаться на этот replay — иначе requeue после краша применит изменение
//! дважды. Леджер op-group здесь НЕ строится (scaffold-нота под AGENT-3).
//!
//! # Корреляция эгресса ([`RunCtx`], AGENT-3a)
//! run_id прогона ЯВНО ПРОБРАСЫВАЕТСЯ через [`run_agent_loop`] в провайдера инструментов как per-call
//! [`RunCtx::run(run_id)`] — а НЕ выставляется в процесс-глобальный слот audit. Поэтому: (а) сброс не
//! нужен (нет общего изменяемого состояния — ctx живёт в стеке вызова прогона и исчезает с ним; эгресс
//! ПОСЛЕ прогона по другому пути несёт свой ctx, обычно [`RunCtx::NONE`]); (б) КОНКУРЕНТНЫЕ прогоны
//! атрибутируют эгресс независимо — у каждого свой ctx в своём стеке, перетереть друг друга нечем.
//! Это снимает гонку процессного single-slot, бывшую блокирующим гейтом AGENT-2 перед AGENT-3 (доказано
//! тестом `concurrent_runs_tag_egress_independently`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use crate::actuator::{DecisionSource, EventSink, TracingEventSink};
use crate::ai::AIClient;
use crate::db::{ReadPool, WriteActor};
use crate::scheduler::{self, Job, JobHandler};

use super::event::AgentEvent;
use super::finish::{outcome_to_finish, CancelWording, PausePolicy, RunFinish};
use super::memory::AgentMemory;
use super::run_store::{self, STATUS_ERROR};
use super::runner::LoopBounds;
use super::session::{
    run_agent_session_bounded, AgentEventForwarder, SessionDeps, SessionRole, SessionSpec,
};
use super::skill_tools::SkillContext;
use super::web_tools::WebToolsConfig;

/// Kind джобы прогона агента (значение колонки `jobs.kind`).
pub const KIND_AGENT_RUN: &str = "agent_run";

/// Токен-бюджет ПОД ПАМЯТЬ в начальном контексте прогона (AGENT-MEM-1). Скромный кусок окна: память
/// агента — это ФОН (факты/прошлые разговоры/эпизоды), а не основной материал прогона; основное окно
/// оставляем под задачу + tool-результаты цикла. recall не превышает этот бюджет (дропает слои);
/// весь начальный контекст потом всё равно проходит общий `ContextBudget::fit` цикла.
pub const RECALL_BUDGET_TOKENS: usize = 1500;

/// Системный преамбул цикла агента (AGENT-2): минимальная инструкция. Богаче (skills/политика
/// автономности) — поздние срезы; здесь — каркас, доказывающий проводку прогона. **Pub** —
/// единый источник истины для прямых вызывающих `run_agent_loop` (desktop UI-1a), а не AgentRunHandler.
pub const AGENT_PREAMBLE: &str =
    "Ты — автономный агент-ассистент Nexus. Реши задачу пользователя, при \
    необходимости вызывая доступные инструменты. Когда задача решена — дай финальный ответ.";

/// Форвардер событий прогона для HEADLESS agentd. Композиция прогона ([`run_agent_session`]) сводит
/// события цикла И Proposal/Diff гейта в один [`AgentEventForwarder`]; здесь — headless-поведение:
/// (1) считает `ToolResult`'ы → счётчик шагов (наблюдаемость/replay, персистится `bump_step` ПОСЛЕ
/// цикла); (2) `tracing`-логирует Proposal/Diff гейта (как прежний [`TracingEventSink`] — UI-стрима у
/// headless нет; под `PolicyDefault` предложения тут же auto-DENY-отклоняются, но лог остаётся для
/// аудита). Прочее (AssistantToken/ToolCall/ContextUsage/Final/Error) игнорируется (нет UI/стрима).
struct HeadlessForwarder {
    steps: Arc<std::sync::atomic::AtomicI64>,
    tracing: TracingEventSink,
}

impl AgentEventForwarder for HeadlessForwarder {
    fn forward(&self, ev: &AgentEvent) {
        match ev {
            AgentEvent::ToolResult { .. } => {
                self.steps.fetch_add(1, Ordering::Relaxed);
            }
            AgentEvent::Proposal { .. } | AgentEvent::Diff { .. } => self.tracing.emit(ev.clone()),
            _ => {}
        }
    }
}

/// Хендлер прогона агента: держит зависимости для прогона цикла как долговечной джобы.
///
/// `defer_under_interactive() = true` — прогон агента уступает интерактивному LLM (S5 backpressure):
/// он НЕ должен забивать модель, пока пользователь активно чатится (см. модульный док backpressure).
pub struct AgentRunHandler {
    writer: WriteActor,
    reader: ReadPool,
    ai: Arc<AIClient>,
    /// Контекстное окно модели (токены) — из конфига; `None` → консервативный дефолт ContextBudget.
    context_window: Option<usize>,
    /// Память агента (AGENT-MEM-1): recall в начальный контекст + Add-only запись. `None` →
    /// прогон стартует с «голым» контекстом (поведение AGENT-2, без регрессии). Композиционный
    /// корень (agentd) собирает [`super::VaultAgentMemory`] из ридера/райтера/эмбеддера/индексов.
    memory: Option<Arc<dyn AgentMemory>>,
    /// КАНОНИЗИРОВАННЫЙ корень vault (предусловие гейта/apply). Нужен ТОЛЬКО когда актуатор включён.
    canon_root: PathBuf,
    /// **GO-LIVE-флаг актуатора (AGENT-3e), SAFE BY DEFAULT.** `false` → прогон БЕЗ инструментов
    /// записи (пустой реестр, B7; реальный vault не затрагивается); `true` → регистрируются гейтнутые
    /// инструменты-актуаторы.
    actuator_enabled: bool,
    /// Порог «крупной перезаписи» → Confirm-тир (из конфига). Эффект только при `actuator_enabled`.
    overwrite_threshold: usize,
    /// Кэп blast-radius прогона (анти-усталость). Эффект только при `actuator_enabled`.
    blast_cap: u32,
    /// Источник решений по предложениям. Headless agentd передаёт [`crate::actuator::PolicyDefault`]
    /// (auto-DENY). Эффект только при `actuator_enabled` (без актуатора предлагать нечему).
    decision_source: Arc<dyn DecisionSource>,
    /// **SKILL-2: контекст скиллов прогона.** `Some` ⇔ skills-каталог сконфигурирован: drive инжектит
    /// tier-1 МЕНЮ каталога (user-role, фенсен) в начальный контекст И регистрирует `activate_skill`
    /// (tier 2) + `read_skill_resource` (tier 3) в реестр. `None` → ни меню, ни инструментов скиллов
    /// (поведение AGENT-2/MEM-1, без регрессии). Скиллы READ-ONLY — работают и при ВЫКЛ актуаторе.
    skills: Option<SkillContext>,
    /// **EGR-AGENT-2: веб-инструменты прогона.** `Some` ⇔ `ai.web.enabled` — drive регистрирует read-only
    /// `web.search`/`web.fetch` (эгресс через `GuardedClient`/`EgressFeature::Web`). `None` → без веба.
    web: Option<WebToolsConfig>,
    /// **SELF-LEARNING SL-7d, OWNER-GATED, default false** (`ai.skills.learning_enabled`). `true` +
    /// `actuator_enabled` + `skills=Some` → drive регистрирует `skill.save` (агент авторствует навыки
    /// через гейт). default-OFF: classify режет `SkillSave` HardBlocked, инструмента нет.
    skills_learning_enabled: bool,
    /// **SUBAGENTS (SUB-3b-2b), OWNER-GATED, default disabled** (`ai.delegation`). `enabled` → drive
    /// собирает `DelegationDeps` и регистрирует `delegate.run` (fan-out субагентов) в top-level прогоне.
    /// Выключено (дефолт) → инструмента нет (без регрессии).
    delegation: crate::ai::DelegationConfig,
    /// **DEEP-RESEARCH (RES-5), OWNER-GATED, default disabled** (`ai.research`). `enabled` (+ delegation +
    /// web + actuator) → drive регистрирует `research.run` (многораундовый веб-ресёрч с записью отчёта через
    /// гейт). Выключено (дефолт) → инструмента нет (без регрессии).
    research: crate::ai::ResearchConfig,
    /// **KILL-SWITCH (AGENT-5): глобальная пауза агента.** Process-global `Arc<AtomicBool>` (взведён ⇒
    /// fail-safe останов). Проверяется на ТРЁХ слоях: (1) `drive` ДО старта (взведён ⇒ прогон остаётся
    /// queued, ре-кьюится); (2) пробрасывается в `run_agent_loop` (мид-ран останов → `Paused`);
    /// (3) пробрасывается в [`DispatchPolicy`] актуатора (НЕ пишет под паузой). Триггер — agentd
    /// (персист `agent.json` + рантайм-Arc); UI-кнопка — UI-1.
    agent_paused: Arc<AtomicBool>,
    /// **BF-1 (хвост #519): границы прогона** (`wall_clock`/`max_steps`) из `ai.agent_wall_clock_secs`/
    /// `ai.agent_max_steps`. Дефолт [`LoopBounds::default`] (300 с / 8 ходов) — конструктор ставит его,
    /// agentd переопределяет через [`AgentRunHandler::with_loop_bounds`]. Отсутствие конфиг-ключей →
    /// байт-прежнее поведение.
    loop_bounds: LoopBounds,
}

impl AgentRunHandler {
    /// Собирает хендлер из ядровых зависимостей. `context_window` — окно модели агента из конфига
    /// (`ai.chat.context_window`), `None` → дефолт [`ContextBudget::from_context_window`].
    /// `memory` — мост к памяти (`None` → прогон без recall, как AGENT-2: нет регрессии).
    ///
    /// AGENT-3a: хендлер БОЛЬШЕ НЕ держит `Arc<EgressAudit>` — корреляция эгресса идёт через per-call
    /// [`RunCtx`], а не через касание процесс-глобального слота audit. Audit-сток (`set_writer`) и
    /// общий [`EgressAudit`] живут в провайдере инструментов (через его [`GuardedClient`]) и
    /// композиционном корне.
    ///
    /// AGENT-3e (go-live актуатора): `canon_root`/`actuator_enabled`/`overwrite_threshold`/`blast_cap`/
    /// `decision_source` — параметры гейтнутого реестра. При `actuator_enabled=false` (дефолт конфига)
    /// они НЕ используются: прогон работает с пустым реестром записи (B7), vault не затрагивается.
    /// SKILL-2: `skills` — контекст скиллов (`Some` при сконфигурированном skills-каталоге → меню в
    /// контекст + tier-2/3 инструменты в реестр; `None` → без скиллов, без регрессии AGENT-2/MEM-1).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        writer: WriteActor,
        reader: ReadPool,
        ai: Arc<AIClient>,
        context_window: Option<usize>,
        memory: Option<Arc<dyn AgentMemory>>,
        canon_root: PathBuf,
        actuator_enabled: bool,
        overwrite_threshold: usize,
        blast_cap: u32,
        decision_source: Arc<dyn DecisionSource>,
        agent_paused: Arc<AtomicBool>,
        skills: Option<SkillContext>,
        web: Option<WebToolsConfig>,
        skills_learning_enabled: bool,
        delegation: crate::ai::DelegationConfig,
        research: crate::ai::ResearchConfig,
    ) -> Self {
        Self {
            writer,
            reader,
            ai,
            context_window,
            memory,
            canon_root,
            actuator_enabled,
            overwrite_threshold,
            blast_cap,
            decision_source,
            agent_paused,
            skills,
            web,
            skills_learning_enabled,
            delegation,
            research,
            // BF-1: дефолтные границы; agentd переопределяет через with_loop_bounds (конфиг-ключи).
            loop_bounds: LoopBounds::default(),
        }
    }

    /// **BF-1 (хвост #519): переопределить границы прогона** (`wall_clock`/`max_steps`) из конфига. agentd
    /// зовёт это с [`LoopBounds::from_ai_config`] (`ai.agent_wall_clock_secs`/`ai.agent_max_steps`).
    /// НЕ вызвано → [`LoopBounds::default`] из конструктора (нулевая регрессия). Builder-стиль (а не
    /// ещё-один-аргумент в уже-16-местный `new`), чтобы тестовые вызыватели не трогать.
    #[must_use]
    pub fn with_loop_bounds(mut self, bounds: LoopBounds) -> Self {
        self.loop_bounds = bounds;
        self
    }

    /// Клон process-global kill-switch (AGENT-5) для рантайм-триггера/наблюдения проводкой (agentd
    /// рестор персиста + будущий control-plane/UI-1). Взвести ⇒ `pause()`, снять ⇒ `resume()`.
    pub fn pause_handle(&self) -> Arc<AtomicBool> {
        self.agent_paused.clone()
    }

    /// Взвести kill-switch (пауза агента).
    pub fn pause(&self) {
        self.agent_paused.store(true, Ordering::Relaxed);
    }

    /// Снять kill-switch (возобновление). Прогоны, оставшиеся queued под паузой, возобновятся воркером.
    pub fn resume(&self) {
        self.agent_paused.store(false, Ordering::Relaxed);
    }

    /// Ведёт прогон цикла: статус-машина run_store + корреляция эгресса + run_agent_loop. Возвращает
    /// `Ok(())` всегда, когда ЖИЗНЕННЫЙ ЦИКЛ прогона корректно доведён до терминала (включая исход
    /// `error` цикла — это НЕ сбой джобы, а штатный терминал прогона; джоба → `done`). `Err` —
    /// только инфраструктурный сбой (БД и т.п.), чтобы планировщик ретраил саму джобу.
    async fn drive(&self, run_id: i64) -> Result<(), String> {
        // 1. Идемпотентность: уже терминальный прогон — НЕ перезапускаем (replay-safety).
        let run = run_store::get_run(&self.reader, run_id)
            .await
            .map_err(|e| format!("agent_run {run_id}: чтение прогона: {e}"))?;
        let Some(run) = run else {
            // Нет строки прогона — payload указывает в пустоту. Не ретраим (ретрай не поможет):
            // возвращаем Ok, джоба уходит в done (видимого «вечного dead» не плодим).
            tracing::warn!(run_id, "agent_run: строки прогона нет — пропуск (no-op)");
            return Ok(());
        };
        if run_store::is_terminal(&run.status) {
            tracing::info!(
                run_id,
                status = %run.status,
                "agent_run: прогон уже терминален — идемпотентный no-op (replay-safe)"
            );
            return Ok(());
        }

        // 2. running. Корреляция эгресса — через per-call RunCtx (НЕ процесс-глобальный слот): его
        //    строит [`run_agent_session`] (`RunCtx::run(run_id)`) и пробрасывает в цикл/провайдера явно.
        //    Сброса не нужно — ctx живёт в стеке вызова и исчезает с ним (другой путь несёт свой ctx).
        run_store::mark_running(&self.writer, run_id)
            .await
            .map_err(|e| format!("agent_run {run_id}: mark_running: {e}"))?;

        // 3. Провайдер инструментов: нет — финишируем прогон с error (НЕ сбой джобы — деградируем
        //    чисто, доказываем lifecycle + RunCtx-проводку даже без живой модели).
        let Some(provider) = self.ai.agent_tools.clone() else {
            run_store::finish_run(
                &self.writer,
                run_id,
                STATUS_ERROR,
                Some("agent tools unavailable"),
            )
            .await
            .map_err(|e| format!("agent_run {run_id}: finish(error): {e}"))?;
            tracing::warn!(run_id, "agent_run: agent_tools=None → finish error");
            return Ok(());
        };

        // 4-5. Прогон через ЕДИНУЮ композицию [`run_agent_session`] (DRY: тот же код у desktop/коннектора).
        //    Она собирает начальный контекст ([system преамбул] + [recall памяти AGENT-MEM-1] +
        //    [меню скиллов SKILL-2 tier-1] + [задача]), выбирает реестр (ПУСТОЙ при ВЫКЛ актуаторе, B7 →
        //    vault не трогается; ВКЛ → гейтнутые актуаторы per-run DispatchPolicy с decision_source=PolicyDefault
        //    + проброс agent_paused в политику), регистрирует tier-2/3 инструменты скиллов и крутит цикл.
        //
        //    Headless-форвардер: считает `ToolResult`'ы в счётчик шагов (наблюдаемость/replay) +
        //    tracing-логирует Proposal/Diff гейта (как прежний TracingEventSink). Запись шага — НЕ из
        //    синхронного форвардера (он не может await), а ПОСЛЕ цикла одним awaited `bump_step`. Счётчик
        //    стартует с 0 (НЕ с `run.step`): replay перезапускает цикл С НАЧАЛА — `step` означает
        //    «результатов инструментов В ЭТОЙ попытке», а не high-water между requeue.
        //    KILL-SWITCH (AGENT-5, чек-пойнт #2): `agent_paused` в цикл → пауза мид-ран → BudgetExhausted{Paused}.
        let steps = Arc::new(std::sync::atomic::AtomicI64::new(0));
        let forwarder: Arc<dyn AgentEventForwarder> = Arc::new(HeadlessForwarder {
            steps: steps.clone(),
            tracing: TracingEventSink::new(),
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let spec = SessionSpec {
            run_id,
            task: run.task.clone(),
            autonomy: run.autonomy.clone(),
            actuator_enabled: self.actuator_enabled,
            overwrite_threshold: self.overwrite_threshold,
            blast_cap: self.blast_cap,
            context_window: self.context_window,
            canon_root: self.canon_root.clone(),
            history: Vec::new(), // scheduler-джоба: задача-one-shot (мультитёрн — только десктоп-чат)
            skills_learning_enabled: self.skills_learning_enabled,
        };
        // SUB-3b-2b: delegate.run в top-level прогоне ТОЛЬКО при `ai.delegation.enabled`. Провайдер как Arc
        // (`self.ai.agent_tools` = тот же `provider`), чтобы дети владели клоном.
        let delegation_deps =
            self.delegation
                .enabled
                .then(|| crate::agent::session::DelegationDeps {
                    provider: provider.clone(),
                    config: self.delegation.clone(),
                });
        let outcome = run_agent_session_bounded(
            &spec,
            &SessionDeps {
                provider: provider.as_ref(),
                memory: self.memory.as_deref(),
                skills: self.skills.as_ref(),
                web: self.web.as_ref(), // EGR-AGENT-2: веб-инструменты (Some ⇔ ai.web.enabled)
                decision_source: self.decision_source.clone(),
                writer: &self.writer,
                reader: &self.reader,
                paused: &self.agent_paused,
                cancel: &cancel,
                forwarder,
            },
            SessionRole::TopLevel {
                delegation: delegation_deps.as_ref(),
                // RES-5: research.run (default-OFF; регистрируется лишь при всех условиях).
                research: Some(&self.research),
            },
            // BF-1: границы из конфига (`ai.agent_wall_clock_secs`/`ai.agent_max_steps`); дефолт если не задан.
            self.loop_bounds,
        )
        .await;

        // 6. Персистим достигнутый шаг ДО финала (bump_step терминал-гард не отвергнет — строка ещё
        //    running). Ошибка бампа не валит прогон (наблюдаемость, не корректность).
        let reached = steps.load(std::sync::atomic::Ordering::Relaxed);
        if reached > 0 {
            if let Err(err) = run_store::bump_step(&self.writer, run_id, reached).await {
                tracing::warn!(run_id, error = %err, "agent_run: bump_step не удался (наблюдаемость)");
            }
        }

        // 7. Терминал/парковка прогона — КАНОН R-2 (`agent::finish::outcome_to_finish`).
        //    PausePolicy::Requeue — ЕДИНСТВЕННЫЙ вызыватель с парковкой паузы (scheduler-путь).
        match outcome_to_finish(&outcome, PausePolicy::Requeue, CancelWording::RunCancelled) {
            // 7a. KILL-SWITCH (AGENT-5, чек-пойнт #2): пауза мид-ран — НЕ терминал. Прогон ВОЗВРАЩАЕТСЯ
            //     в `queued` + пере-кьюется (как чек-пойнт #1), чтобы возобновиться на un-pause.
            //     replay-safe: повторный заход перезапускает цикл С НАЧАЛА (актуатор идемпотентен
            //     per-op-group), а под паузой записей всё равно не было (чек-пойнт #3 + цикл-чек
            //     остановил ДО хода). НЕ пишем finish (прогон не завершён) — наоборот,
            //     requeue_to_queued возвращает строку в queued.
            RunFinish::Park => {
                run_store::requeue_to_queued(&self.writer, run_id)
                    .await
                    .map_err(|e| format!("agent_run {run_id}: пауза мид-ран → queued: {e}"))?;
                scheduler::enqueue(
                    &self.writer,
                    KIND_AGENT_RUN,
                    &run_id.to_string(),
                    scheduler::now_secs() + PAUSE_REQUEUE_DELAY_SECS,
                    3,
                )
                .await
                .map_err(|e| format!("agent_run {run_id}: пере-кью паузы мид-ран: {e}"))?;
                tracing::info!(
                    run_id,
                    "agent_run: kill-switch ВЗВЕДЁН мид-ран — прогон → queued, пере-кью на un-pause"
                );
            }
            // 7b. Терминал прогона по исходу цикла. Отмена (cancel) → `cancelled` (отдельный
            //     терминал, не error): таксономия статусов не врёт. Прочее исчерпание бюджета (steps/
            //     wall_clock/tokens) → error (прогон не довёл задачу).
            RunFinish::Finalize { status, text } => {
                run_store::finish_run(&self.writer, run_id, status, Some(&text))
                    .await
                    .map_err(|e| format!("agent_run {run_id}: finish({status}): {e}"))?;
                tracing::info!(run_id, status, "agent_run: прогон завершён");
            }
        }
        Ok(())
    }
}

/// Через сколько секунд ПЕРЕ-ПОСТАВИТЬ джобу прогона, отложенного kill-switch'ем (AGENT-5 чек-пойнт
/// #1): пока пауза взведена, прогон остаётся `queued` и периодически пере-кьюится с этой задержкой,
/// чтобы возобновиться вскоре после un-pause. Скромный период (как тик планировщика) — не «битый цикл».
const PAUSE_REQUEUE_DELAY_SECS: i64 = 5;

#[async_trait]
impl JobHandler for AgentRunHandler {
    async fn handle(&self, job: &Job) -> Result<(), String> {
        let run_id: i64 = job
            .payload
            .trim()
            .parse()
            .map_err(|e| format!("agent_run: payload не run_id ('{}'): {e}", job.payload))?;

        // KILL-SWITCH (AGENT-5, чек-пойнт #1): ДО любого старта прогона. Взведён ⇒ НЕ запускаем цикл,
        // прогон ОСТАЁТСЯ `queued` (drive ниже даже не зовётся → нет mark_running, нет хода модели, нет
        // диспатча инструмента → НИ ОДНОЙ записи). Чтобы прогон возобновился на un-pause — пере-кьюим
        // СВЕЖУЮ джобу с задержкой (текущая уйдёт в done). Прогон replay-safe (drive run-level
        // идемпотентен), поэтому повторный заход безопасен. Не трогаем строку прогона — она и так queued.
        if self.agent_paused.load(Ordering::Relaxed) {
            // Пере-кьюим только пока строка прогона ещё НЕ терминальна (иначе плодили бы вечные джобы
            // для давно завершённого прогона). Терминальный/отсутствующий ⇒ просто done (no-op).
            let still_pending = matches!(
                run_store::get_run(&self.reader, run_id).await,
                Ok(Some(run)) if !run_store::is_terminal(&run.status)
            );
            if still_pending {
                scheduler::enqueue(
                    &self.writer,
                    KIND_AGENT_RUN,
                    &run_id.to_string(),
                    scheduler::now_secs() + PAUSE_REQUEUE_DELAY_SECS,
                    job.max_attempts,
                )
                .await
                .map_err(|e| format!("agent_run {run_id}: пере-кью под паузой: {e}"))?;
                tracing::info!(
                    run_id,
                    "agent_run: kill-switch ВЗВЕДЁН — прогон остаётся queued, пере-кью на un-pause"
                );
            }
            return Ok(());
        }

        self.drive(run_id).await
    }

    fn defer_under_interactive(&self) -> bool {
        // S5: прогон агента — тяжёлый LLM-фон, уступает интерактивному чату (не стартует, пока busy).
        true
    }
}

/// Ставит прогон агента в очередь: создаёт строку `agent_runs` (queued) → энкьюит джобу
/// `KIND_AGENT_RUN` с payload = run_id → возвращает run_id (для UI/корреляции). `max_attempts` —
/// небольшое (прогон replay-safe при ВЫКЛ актуаторе — реестр записи пуст; см. контракт replay).
pub async fn enqueue_agent_run(
    writer: &WriteActor,
    task: &str,
    model: Option<&str>,
    autonomy: Option<&str>,
) -> crate::db::DbResult<i64> {
    let run_id = run_store::create_run(writer, task, model, autonomy).await?;
    scheduler::enqueue(
        writer,
        KIND_AGENT_RUN,
        &run_id.to_string(),
        scheduler::now_secs(),
        3,
    )
    .await?;
    Ok(run_id)
}

#[cfg(test)]
mod tests;
