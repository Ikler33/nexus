//! [`run_agent_session`] — ЕДИНАЯ композиция прогона агента (P0b-2): сборка начального контекста +
//! выбор реестра (пустой при ВЫКЛ актуаторе | гейтнутые актуаторы) + скиллы + `run_agent_loop`.
//! Транспорт-агностична:
//! куда уходят события — решает [`AgentEventForwarder`], переданный вызывающим.
//!
//! # Зачем модуль (DRY)
//! Раньше эта композиция жила в ТРЁХ местах: [`super::job::AgentRunHandler`] (headless scheduler),
//! desktop `commands::agent::drive_run` (стрим в Tauri-Channel) и — намечался — agentd-коннектор
//! (стрим в [`super::connect::Transport`]). Три копии расходились по контракту. Здесь — единственный
//! источник истины: каждый вызывающий лишь оборачивает run-lifecycle (run_store create/mark/finish)
//! и подаёт СВОЙ [`AgentEventForwarder`]:
//! - headless agentd → counter + `tracing`-лог Proposal/Diff (наблюдаемость, без стрима);
//! - desktop → маппинг в wire-DTO → `Channel<AgentStreamEvent>` (UI-1b);
//! - agentd-коннектор → wire-DTO → `agent/event`-нотификация через [`super::connect::Transport`].
//!
//! # Форма вызова (R-4)
//! Три аргумента вместо 14 позиционных: [`SessionSpec`] (плоские data-параметры) + [`SessionDeps`]
//! (хендлы: провайдер/store/гейт/каналы фич/форвардер) + [`SessionRole`] (top-level с owner-gated
//! каналами delegation/research ЛИБО субагент с сужением реестра — невалидная комбинация «субагент с
//! delegation/research» непредставима типом). Регистрация фич (web/skills/delegation/research) —
//! хелперы `features` с default-OFF условиями ВНУТРИ каждого.
//!
//! # Слияние двух потоков событий
//! Цикл шлёт AssistantToken/ToolCall/ToolResult/ContextUsage/Final/Error через `on_event`; гейт
//! актуатора шлёт Proposal/Diff через свой [`EventSink`]. Потоки НЕПЕРЕСЕКАЮЩИЕСЯ. Здесь оба сводятся
//! в ОДИН [`AgentEventForwarder`]: `on_event` зовёт `forward` напрямую, а гейт получает
//! [`ForwardingEventSink`]-обёртку над тем же форвардером. Так потребитель видит единый порядок.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::actuator::{
    ActionDispatcher, AuditSink, BatchDecision, DecisionSource, DispatchPolicy, EventSink,
    GatedToolCtx, NoteCreateTool, NoteEditTool, ProposalBatch, SetFrontmatterTool, SkillSaveCtx,
    SkillSaveTool,
};
use crate::ai::tools::ToolCapableProvider;
use crate::ai::{injection_marker, ChatMessage, ContextBudget, QwenTokenizer};
use crate::db::{ReadPool, WriteActor};
use crate::net::RunCtx;

use super::event::AgentEvent;
use super::job::{AGENT_PREAMBLE, RECALL_BUDGET_TOKENS};
use super::memory::AgentMemory;
use super::registry::ToolRegistry;
use super::runner::{run_agent_loop, LoopBounds, LoopOutcome};
use super::skill_tools::SkillContext;
use super::web_tools::WebToolsConfig;

mod features;
#[cfg(test)]
mod tests;

/// СИНХРОННЫЙ форвардер событий прогона наружу. Реализуется потребителем под свой транспорт; вызывается
/// из двух мест внутри [`run_agent_session`] (loop `on_event` + гейт-[`EventSink`]) — оба синхронны,
/// поэтому асинхронный транспорт (коннектор) форвардер мостит сам (mpsc → drain-таск).
pub trait AgentEventForwarder: Send + Sync {
    /// Принять одно событие хода (по ССЫЛКЕ — форвардер обычно клонирует лишь нужное / маппит в wire).
    fn forward(&self, ev: &AgentEvent);
}

/// Мост гейт-[`EventSink`] → [`AgentEventForwarder`]: Proposal/Diff гейта уходят в тот же форвардер,
/// что и события цикла (единый поток для потребителя).
struct ForwardingEventSink(Arc<dyn AgentEventForwarder>);

impl EventSink for ForwardingEventSink {
    fn emit(&self, event: AgentEvent) {
        self.0.forward(&event);
    }
}

/// **Fix BF-1 №1 — учёт «времени на паузе» у гейта.** Декоратор [`DecisionSource`]: замеряет, сколько
/// прогон БЛОКИРОВАЛСЯ на ожидании человеческого решения (`decide().await`), и аккумулирует наносекунды
/// в общий `paused_nanos`. [`run_agent_loop`] ВЫЧИТАЕТ это время из wall_clock-возраста прогона — раздумья
/// человека у changeset-гейта НЕ жгут бюджет (иначе «Подтвердить» после >5 мин раздумий = мгновенный
/// «бюджет исчерпан (WallClock)»). Прозрачен для РЕШЕНИЯ (просто прокси `inner.decide`), поэтому
/// fail-closed-семантика источника (пропуск айтема = Reject, закрытый канал = reject_all) не меняется.
/// Покрывает ВСЕ in-process пути гейта, идущие через ОДИН `decide()` каноничного `run_proposal_round`:
/// note-changeset ([`GatedToolCtx`]) и `skill.save` ([`SkillSaveCtx`]). Вычитается ТОЛЬКО длительность
/// блокировки на `decide` — kill-switch (пауза агента) и Cancelled НЕ затрагиваются.
struct PauseAccountingDecision {
    inner: Arc<dyn DecisionSource>,
    paused_nanos: Arc<AtomicU64>,
}

#[async_trait::async_trait]
impl DecisionSource for PauseAccountingDecision {
    async fn decide(&self, batch: &ProposalBatch) -> BatchDecision {
        let t0 = Instant::now();
        let decision = self.inner.decide(batch).await;
        // Насыщающее приведение u128→u64: реальная пауза человека (минуты) не приближается к переполнению
        // u64-наносекунд (~584 года), но перестраховываемся, чтобы `as u64` не завернулось молча.
        let elapsed = u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.paused_nanos.fetch_add(elapsed, Ordering::Relaxed);
        decision
    }
}

/// Плоские (data-only) параметры прогона. Хендлы (provider/memory/skills/decision_source/writer/reader/
/// флаги/форвардер) идут отдельной структурой [`SessionDeps`] (они не `Clone`/суть Arc/ссылки); роль
/// прогона (top-level/субагент) — [`SessionRole`].
pub struct SessionSpec {
    /// `id` строки `agent_runs` (корреляция эгресса/леджера/UI).
    pub run_id: i64,
    /// Задача пользователя (становится финальным `user`-сообщением начального контекста).
    pub task: String,
    /// W-4: история переписки ПРЕДЫДУЩИХ ходов сессии (user-задачи + assistant-ответы), вставляется
    /// между меню скиллов и текущей задачей. Десктоп-чат мультитёрный (`turns[]`), но прогон агента —
    /// one-shot per `run_id`; без истории follow-up-ход («теперь добавь раздел») не помнил, что прошлый
    /// ход правил заметку → модель отвечала прозой, write-tool не вызывался → не было changeset-гейта
    /// (ST-G3). Пусто (top-level/agentd/первый ход) → поведение без регрессии.
    pub history: Vec<ChatMessage>,
    /// Автономия прогона (`confirm`|`auto`|`None`→confirm в политике). Эффект только при `actuator_enabled`.
    pub autonomy: Option<String>,
    /// **GO-LIVE-флаг актуатора, SAFE BY DEFAULT.** `false` → БЕЗ инструментов записи (пустой реестр,
    /// B7; vault не трогается); `true` → гейтнутые инструменты-актуаторы за `dispatch_action`.
    pub actuator_enabled: bool,
    /// Порог «крупной перезаписи» → Confirm-тир. Эффект только при `actuator_enabled`.
    pub overwrite_threshold: usize,
    /// Кэп blast-radius прогона (анти-усталость). Эффект только при `actuator_enabled`.
    pub blast_cap: u32,
    /// Окно контекста модели (токены) из конфига; `None` → консервативный дефолт [`ContextBudget`].
    pub context_window: Option<usize>,
    /// КАНОНИЗИРОВАННЫЙ корень vault (предусловие гейта/apply). Нужен только при `actuator_enabled`.
    pub canon_root: PathBuf,
    /// **SELF-LEARNING SL-7d, OWNER-GATED, default false** (`ai.skills.learning_enabled`). `true` И
    /// `actuator_enabled` И сконфигурированный skills-каталог (`skills=Some`) → регистрируется `skill.save`
    /// (агент авторствует навыки через гейт Confirm-never-Auto) + откат прогона идёт `undo_run_full` со
    /// skills_root. `false` → инструмента нет, classify режет `SkillSave` HardBlocked (поведение без регрессии).
    pub skills_learning_enabled: bool,
}

/// ХЕНДЛЫ-зависимости прогона (`&dyn`-трейты / `Arc` / ссылки — не data) — второй аргумент
/// [`run_agent_session`]. До R-4 шли ДЕСЯТЬЮ позиционными аргументами, и у шести прод-вызывателей
/// выстраивались колонки `None, // …`-комментариев; именованные поля делают состав самодокументируемым.
/// Опциональные КАНАЛЫ ФИЧ (`memory`/`skills`/`web`) — `None` = фича выключена, поведение без
/// регрессии (см. доки полей).
pub struct SessionDeps<'a> {
    /// Провайдер модели цикла (`&dyn` — владение не нужно; `Arc`-клоны для детей несёт [`DelegationDeps`]).
    pub provider: &'a dyn ToolCapableProvider,
    /// Память recall'а (AGENT-MEM-1). `None` → recall пуст (поведение без памяти, без регрессии).
    pub memory: Option<&'a dyn AgentMemory>,
    /// Скиллы: tier-1 меню в контекст + tier-2/3 read-only инструменты (SKILL-2). `None` → ни меню,
    /// ни инструментов.
    pub skills: Option<&'a SkillContext>,
    /// Веб-инструменты web.search/web.fetch, read-only (EGR-AGENT). `None` → без них.
    pub web: Option<&'a WebToolsConfig>,
    /// Источник решений гейта актуатора (человек-в-петле, fail-closed). При ВЫКЛ актуаторе не
    /// используется (гейт не строится).
    pub decision_source: Arc<dyn DecisionSource>,
    pub writer: &'a WriteActor,
    pub reader: &'a ReadPool,
    /// KILL-SWITCH (AGENT-5): пробрасывается и в цикл (граница хода), и в политику гейта (чек-пойнт #3).
    pub paused: &'a Arc<AtomicBool>,
    /// Отмена прогона (цикл останавливается на границе хода).
    pub cancel: &'a Arc<AtomicBool>,
    /// ЕДИНЫЙ форвардер обоих потоков событий: loop `on_event` + гейт через [`ForwardingEventSink`].
    pub forwarder: Arc<dyn AgentEventForwarder>,
}

/// **SUB-3b-2b: зависимости для РЕГИСТРАЦИИ `delegate.run`** в TOP-LEVEL прогоне. `Some` И `config.enabled`
/// → [`run_agent_session`] собирает [`SubagentContext`](crate::agent::delegate::SubagentContext) (из своих
/// хендлов + Arc-провайдера + общего gate) и регистрирует [`crate::agent::delegate::DelegateTool`].
/// `None`/выключено → инструмента нет (поведение без регрессии). Канал существует ТОЛЬКО в
/// [`SessionRole::TopLevel`] — субагенты делегировать не могут ПО ПОСТРОЕНИЮ типа (рекурсия-стоп).
/// `provider` — `Arc` (дети клонируют в конкурентные задачи).
pub struct DelegationDeps {
    /// Провайдер модели как `Arc` (для порождения детей — `&dyn` цикла недостаточно, нужен 'static-клон).
    pub provider: Arc<dyn ToolCapableProvider>,
    /// Капы/флаг делегирования (`enabled`/`max_depth`/`max_fanout`/`max_total_spawns`).
    pub config: crate::ai::DelegationConfig,
}

/// РОЛЬ прогона — третий аргумент [`run_agent_session`]. До R-4 роль кодировалась ТРЕМЯ независимыми
/// `Option`-аргументами (`subagent`/`delegation`/`research`), и инвариант «субагент БЕЗ
/// delegation/research» держался только `None, //`-комментариями у вызывателей. Теперь невалидная
/// комбинация НЕПРЕДСТАВИМА: каналы delegation/research существуют только в [`SessionRole::TopLevel`],
/// а сужение реестра/общий gate — только в [`SessionRole::Subagent`] («дети не делегируют/не ресёрчат» —
/// по построению типа; блок-лист имён `CHILD_BLOCKED_TOOLS` остаётся второй линией там, где был —
/// в `build_child_registry`).
pub enum SessionRole<'a> {
    /// Top-level прогон (agentd/desktop/cli/коннектор/ACP): без сужения реестра, с опциональными
    /// owner-gated каналами delegation/research (оба default-OFF).
    TopLevel {
        /// SUB-3b-2b: `Some` И `config.enabled` → регистрируется `delegate.run` (см. [`DelegationDeps`]).
        delegation: Option<&'a DelegationDeps>,
        /// RES-4/5: конфиг `research.run` (default-OFF). Регистрируется ЛИШЬ при всех 5 условиях —
        /// см. `features::register_research`.
        research: Option<&'a crate::ai::ResearchConfig>,
    },
    /// **SUB-3: прогон-СУБАГЕНТ.** Относительно top-level:
    /// 1. реестр инструментов ребёнка СУЖАЕТСЯ до `allowed` ПОСЛЕ полной сборки (actuator+skills+web) —
    ///    security keystone child ⊆ parent (имена из [`crate::agent::delegate::build_child_registry`]);
    ///    эскалация невозможна по построению;
    /// 2. при `dispatcher = Some` note-инструменты делят РОДИТЕЛЬСКИЙ actuator-гейт (общий
    ///    blast-radius/ledger/policy/pause) вместо постройки своего — «запись через ОДИН gate»;
    /// 3. `skill.save` НЕ регистрируется (дети не авторствуют навыки — blocklist SUB-1).
    Subagent {
        /// Разрешённые ИМЕНА инструментов ребёнка (подмножество родителя минус блок-лист).
        allowed: &'a std::collections::BTreeSet<String>,
        /// Опц. ОБЩИЙ с родителем actuator-диспетчер (тот же gate). `None` → ребёнок строит эквивалентный
        /// gate из своих spec-полей (canon_root/autonomy/threshold/blast); blast-radius тогда per-child.
        dispatcher: Option<Arc<dyn ActionDispatcher>>,
    },
}

/// Гонит один прогон агента: собирает начальный контекст ([system преамбул] + [recall памяти] +
/// [меню скиллов] + [задача]), выбирает реестр (пустой при ВЫКЛ актуаторе | гейтнутые актуаторы с
/// [`ForwardingEventSink`]), регистрирует tier-2/3 инструменты скиллов и крутит [`run_agent_loop`].
/// Возвращает [`LoopOutcome`] — финализацию в `run_store` делает ВЫЗЫВАЮЩИЙ (этот слой не трогает
/// статус-машину прогона, чтобы оставаться переиспользуемым для scheduler/desktop/коннектора).
///
/// `deps.memory = None` → recall пуст (поведение без памяти, без регрессии). `deps.skills = None` →
/// ни меню, ни tier-2/3 инструментов. KILL-SWITCH (`deps.paused`) и `deps.cancel` пробрасываются в
/// цикл (и в политику гейта). Роль: [`SessionRole::Subagent`] → прогон-РЕБЁНОК (сужение реестра/общий
/// gate); [`SessionRole::TopLevel`] → опц. каналы delegation/research (без них — без регрессии).
pub async fn run_agent_session(
    spec: &SessionSpec,
    deps: &SessionDeps<'_>,
    role: SessionRole<'_>,
) -> LoopOutcome {
    run_agent_session_bounded(spec, deps, role, LoopBounds::default()).await
}

/// Как [`run_agent_session`], но с ЯВНЫМИ [`LoopBounds`]. **BF-1 (хвост из #519): конфигурируемый
/// `wall_clock`/`max_steps` прогона.** Config-вызыватели (desktop/agentd/cli/acp) резолвят границы из
/// `ai.agent_wall_clock_secs`/`ai.agent_max_steps` через [`LoopBounds::from_ai_config`] и зовут ИМЕННО
/// эту функцию; вызыватели без конфига (субагенты/smoke/eval) остаются на [`run_agent_session`] (дефолт).
/// Границы пробрасываются в [`run_agent_loop`] И (для top-level) в капы дочернего делегирования/research
/// (`bounds.wall_clock` → `register_delegation`/`register_research`) — точь-в-точь как и дефолт до BF-1.
/// Также тест-шов Fix BF-1 №1: session-тест с коротким `wall_clock` доказывает ПРОВОДКУ
/// pause-accounting-декоратора (медленное решение у гейта не валит прогон по WallClock) — мутант «в гейт
/// передан голый `deps.decision_source`» валит этот тест.
pub async fn run_agent_session_bounded(
    spec: &SessionSpec,
    deps: &SessionDeps<'_>,
    role: SessionRole<'_>,
    bounds: LoopBounds,
) -> LoopOutcome {
    // Начальный контекст: [system преамбул] + [recall памяти] + [меню скиллов] + [задача]. recall —
    // только чтение, никогда не ошибка (деградирует в пусто); None память → пусто (без регрессии).
    // Меню скиллов (tier-1) — фенсенный user-role блок (данные, не инструкции, I-5); per-request
    // injection_marker. Тела скиллов раскрывает лишь tier-2 `activate_skill`.
    let recalled = match deps.memory {
        Some(mem) => mem.recall(&spec.task, RECALL_BUDGET_TOKENS).await,
        None => Vec::new(),
    };
    let skill_menu: Option<ChatMessage> = deps
        .skills
        .and_then(|sk| sk.catalog_block(&injection_marker()))
        .map(ChatMessage::user);
    let mut messages = Vec::with_capacity(
        recalled.len() + spec.history.len() + 2 + usize::from(skill_menu.is_some()),
    );
    messages.push(ChatMessage::system(AGENT_PREAMBLE));
    messages.extend(recalled);
    messages.extend(skill_menu);
    // W-4: история прошлых ходов сессии ПЕРЕД текущей задачей — чтобы follow-up продолжал работу
    // прошлого хода (и снова предлагал правки через гейт), а не отвечал прозой с нуля.
    messages.extend(spec.history.iter().cloned());
    messages.push(ChatMessage::user(&spec.task));

    let budget = ContextBudget::from_context_window(spec.context_window);
    let tk = QwenTokenizer::embedded();

    // Fix BF-1 №1: per-run счётчик «времени на паузе» у гейта. Гейт актуатора получает decision_source,
    // ОБЁРНУТЫЙ в [`PauseAccountingDecision`] (копит наносекунды блокировки на `decide()` сюда), а цикл
    // получает `&paused_nanos` и вычитает это время из wall_clock. Делегирование/research кладут в
    // `SubagentContext` НЕобёрнутый `deps.decision_source` И родительский `parent_dispatcher`
    // (features.rs): ребёнок БЕЗ общего gate (`dispatcher=None`) обернёт источник СВОИМ счётчиком в
    // собственном `run_agent_session`; ребёнок с ОБЩИМ родительским gate (`dispatcher=Some`) свой гейт
    // НЕ строит и идёт через РОДИТЕЛЬСКИЙ декоратор — ожидания решений по его правкам кредитуются в
    // счётчик РОДИТЕЛЯ, а собственный `paused_nanos` ребёнка не пополняется (осознанный хвост, ошибка
    // fail-safe «родитель работает дольше»; см. `docs/BACKLOG.md` §«Хвосты среза BF-1»).
    let paused_nanos = Arc::new(AtomicU64::new(0));
    let gate_decision: Arc<dyn DecisionSource> = Arc::new(PauseAccountingDecision {
        inner: deps.decision_source.clone(),
        paused_nanos: paused_nanos.clone(),
    });

    // Реестр: дефолт-OFF → ПУСТОЙ реестр записи (B7: debug-стабы echo/noop вычищены из прод-пути —
    // модель не видит пустышек в списке инструментов; read-only skills/web добавляются НИЖЕ независимо
    // от флага); ВКЛ → гейтнутые актуаторы за
    // dispatch_action. Гейт получает ForwardingEventSink → Proposal/Diff уходят тем же форвардером,
    // что и события цикла. Per-run DispatchPolicy (общий blast-radius между инструментами) + проброс
    // `paused` в политику (KILL-SWITCH чек-пойнт #3: НЕ пишет под паузой даже мид-инструмент).
    // SUB-3b-2b: gate-диспетчер хойстим наружу блока — `delegate.run` (если включён) положит его в
    // `SubagentContext`, чтобы ДЕТИ писали через ТОТ ЖЕ родительский gate (общий blast-radius/ledger).
    let mut parent_dispatcher: Option<Arc<dyn ActionDispatcher>> = None;
    let mut registry = if spec.actuator_enabled {
        let mut reg = ToolRegistry::new();
        // ШОВ актуатора (SANDBOX-4b-2): инструменты держат `Arc<dyn ActionDispatcher>`. In-process путь —
        // `GatedToolCtx` (локальный `dispatch_action`). СУБАГЕНТ (SUB-3) с общим dispatcher → переиспуем
        // РОДИТЕЛЬСКИЙ gate (общий blast-radius/ledger/policy/pause; «запись через ОДИН gate»), и `skill.save`
        // детям НЕ кладём (blocklist SUB-1). Иначе строим свой gate как раньше.
        let dispatcher: Arc<dyn ActionDispatcher> = if let SessionRole::Subagent {
            dispatcher: Some(d),
            ..
        } = &role
        {
            d.clone()
        } else {
            let ledger = AuditSink::new(deps.writer.clone(), deps.reader.clone());
            // SL-7d: skills-флаги в политику → classify_skill_save видит learning/root. Note/exec не затронуты.
            let policy = DispatchPolicy::with_paused(
                spec.autonomy.as_deref(),
                spec.overwrite_threshold,
                spec.blast_cap,
                deps.paused.clone(),
            )
            .with_skills_flags(spec.skills_learning_enabled, deps.skills.is_some());
            let events: Arc<dyn EventSink> = Arc::new(ForwardingEventSink(deps.forwarder.clone()));
            // SL-7d: `skill.save` — ТОЛЬКО top-level ([`SessionRole::TopLevel`] — у субагентов
            // непредставим) + learning + skills. Дети навыки не авторствуют. Делит
            // policy/ledger/decision_source/events с note-инструментами.
            if matches!(role, SessionRole::TopLevel { .. }) && spec.skills_learning_enabled {
                if let Some(sk) = deps.skills {
                    let skill_ctx = SkillSaveCtx::new(
                        sk.skills_root().to_path_buf(),
                        ledger.clone(),
                        spec.run_id,
                        policy.clone(),
                        // Fix BF-1 №1: гейт навыка на том же pause-accounting-источнике, что note-гейт.
                        gate_decision.clone(),
                        events.clone(),
                        Some(deps.writer.clone()),
                    );
                    reg.insert(Arc::new(SkillSaveTool::new(Arc::new(skill_ctx))));
                }
            }
            // Fix BF-1 №1: note-гейт получает pause-accounting-обёртку (раздумья человека не жгут бюджет).
            // `deps.decision_source` (НЕобёрнутый) остаётся для `SubagentContext` делегирования ниже:
            // им пользуется только ребёнок, строящий СВОЙ гейт (`dispatcher=None`); ребёнок с ОБЩИМ
            // родительским gate идёт через ЭТОТ декоратор и кредитует счётчик РОДИТЕЛЯ (см. коммент у
            // `paused_nanos` выше + BACKLOG §«Хвосты среза BF-1»).
            Arc::new(GatedToolCtx::new(
                spec.canon_root.clone(),
                ledger,
                spec.run_id,
                policy,
                gate_decision.clone(),
                events,
            ))
        };
        parent_dispatcher = Some(dispatcher.clone());
        reg.insert(Arc::new(NoteCreateTool::new(dispatcher.clone())));
        reg.insert(Arc::new(NoteEditTool::new(dispatcher.clone())));
        reg.insert(Arc::new(SetFrontmatterTool::new(dispatcher)));
        reg
    } else {
        // Default-safe (B7): БЕЗ инструментов записи — пустой реестр, vault не трогается. Пустой набор
        // корректен по всему пути: провайдер опускает `tools`/`tool_choice` из тела запроса
        // (`request_body_adds_tools_only_when_present`), преамбула тулов не перечисляет, цикл без
        // вызовов ждёт Final; вызов несуществующего имени → UnknownTool is_error (модель
        // восстанавливается). decision_source/canon_root при OFF не используются.
        let _ = (&deps.decision_source, &spec.canon_root);
        ToolRegistry::new()
    };
    // Read-only каналы фич — НЕЗАВИСИМО от actuator-флага; условия регистрации — ВНУТРИ
    // register-хелперов `features` (R-4, структура условий не менялась).
    features::register_web(&mut registry, deps, spec);
    features::register_skills(&mut registry, deps);

    // Роль-специфика ([`SessionRole`]: представимы только валидные комбинации).
    match &role {
        // SUB-3 (security keystone проводки): реестр СУБАГЕНТА сужаем до выданного подмножества ПОСЛЕ
        // полной сборки (actuator+skills+web). `allowed` = build_child_registry(child ⊆ parent минус
        // блок-лист) — имя сверху физически удаляется, субагент не вызовет инструмент сверх выданного.
        SessionRole::Subagent { allowed, .. } => registry.retain(allowed),
        // Top-level — без сужения; owner-gated каналы delegate.run/research.run (default-OFF,
        // truth-table регистрации — внутри хелперов).
        SessionRole::TopLevel {
            delegation,
            research,
        } => {
            features::register_delegation(
                &mut registry,
                deps,
                spec,
                *delegation,
                parent_dispatcher.as_ref(),
                bounds.wall_clock,
            );
            features::register_research(
                &mut registry,
                deps,
                spec,
                *research,
                *delegation,
                parent_dispatcher.as_ref(),
                bounds.wall_clock,
            );
        }
    }

    // on_event: КАЖДОЕ событие цикла → форвардер (тот же, что у гейта). Запись шага/стрим/лог — забота
    // форвардера (path-specific). Синхронный (как требует `run_agent_loop`).
    let mut on_event = |e: AgentEvent| deps.forwarder.forward(&e);

    // KILL-SWITCH (AGENT-5, чек-пойнт #2): `paused` в цикл — пауза мид-ран остановит на границе хода.
    // Fix BF-1 №1: `&paused_nanos` — то же время, что копит pause-accounting-обёртка гейта; цикл вычитает
    // его из wall_clock (ожидание решения человека не жжёт бюджет прогона).
    run_agent_loop(
        deps.provider,
        &registry,
        messages,
        bounds,
        &budget,
        &tk,
        deps.cancel,
        deps.paused,
        &paused_nanos,
        RunCtx::run(spec.run_id),
        &mut on_event,
    )
    .await
}
