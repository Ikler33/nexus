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
//! # Слияние двух потоков событий
//! Цикл шлёт AssistantToken/ToolCall/ToolResult/ContextUsage/Final/Error через `on_event`; гейт
//! актуатора шлёт Proposal/Diff через свой [`EventSink`]. Потоки НЕПЕРЕСЕКАЮЩИЕСЯ. Здесь оба сводятся
//! в ОДИН [`AgentEventForwarder`]: `on_event` зовёт `forward` напрямую, а гейт получает
//! [`ForwardingEventSink`]-обёртку над тем же форвардером. Так потребитель видит единый порядок.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::actuator::{
    ActionDispatcher, AuditSink, DecisionSource, DispatchPolicy, EventSink, GatedToolCtx,
    NoteCreateTool, NoteEditTool, SetFrontmatterTool, SkillSaveCtx, SkillSaveTool,
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

/// Плоские (data-only) параметры прогона. Хендлы (provider/memory/skills/decision_source/writer/reader/
/// флаги/форвардер) идут отдельными аргументами [`run_agent_session`] (они не `Clone`/суть Arc/ссылки).
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

/// **SUB-3: оверрайды прогона СУБАГЕНТА** для [`run_agent_session`]. `None` (top-level) → текущее
/// поведение байт-в-байт. `Some` →
/// 1. реестр инструментов ребёнка СУЖАЕТСЯ до [`SubagentSpawn::allowed`] ПОСЛЕ полной сборки
///    (actuator+skills+web) — security keystone child ⊆ parent (имена из
///    [`crate::agent::delegate::build_child_registry`]); эскалация невозможна по построению;
/// 2. при [`SubagentSpawn::dispatcher`] `Some` note-инструменты делят РОДИТЕЛЬСКИЙ actuator-гейт (общий
///    blast-radius/ledger/policy/pause) вместо постройки своего — «запись через ОДИН gate»;
/// 3. `skill.save` НЕ регистрируется (дети не авторствуют навыки — blocklist SUB-1).
pub struct SubagentSpawn<'a> {
    /// Разрешённые ИМЕНА инструментов ребёнка (подмножество родителя минус блок-лист).
    pub allowed: &'a std::collections::BTreeSet<String>,
    /// Опц. ОБЩИЙ с родителем actuator-диспетчер (тот же gate). `None` → ребёнок строит эквивалентный
    /// gate из своих spec-полей (canon_root/autonomy/threshold/blast); blast-radius тогда per-child.
    pub dispatcher: Option<Arc<dyn ActionDispatcher>>,
}

/// **SUB-3b-2b: зависимости для РЕГИСТРАЦИИ `delegate.run`** в TOP-LEVEL прогоне. `Some` И `config.enabled`
/// → [`run_agent_session`] собирает [`SubagentContext`] (из своих хендлов + Arc-провайдера + общего gate)
/// и регистрирует [`crate::agent::delegate::DelegateTool`]. `None`/выключено → инструмента нет (поведение
/// без регрессии). Передаётся ТОЛЬКО для top-level: субагенты (`spawn_subagent`) зовут сессию БЕЗ него —
/// дети делегировать не могут (рекурсия-стоп). `provider` — `Arc` (дети клонируют в конкурентные задачи).
pub struct DelegationDeps {
    /// Провайдер модели как `Arc` (для порождения детей — `&dyn` цикла недостаточно, нужен 'static-клон).
    pub provider: Arc<dyn ToolCapableProvider>,
    /// Капы/флаг делегирования (`enabled`/`max_depth`/`max_fanout`/`max_total_spawns`).
    pub config: crate::ai::DelegationConfig,
}

/// Гонит один прогон агента: собирает начальный контекст ([system преамбул] + [recall памяти] +
/// [меню скиллов] + [задача]), выбирает реестр (пустой при ВЫКЛ актуаторе | гейтнутые актуаторы с
/// [`ForwardingEventSink`]), регистрирует tier-2/3 инструменты скиллов и крутит [`run_agent_loop`].
/// Возвращает [`LoopOutcome`] — финализацию в `run_store` делает ВЫЗЫВАЮЩИЙ (этот слой не трогает
/// статус-машину прогона, чтобы оставаться переиспользуемым для scheduler/desktop/коннектора).
///
/// `memory = None` → recall пуст (поведение без памяти, без регрессии). `skills = None` → ни меню, ни
/// tier-2/3 инструментов. KILL-SWITCH (`paused`) и `cancel` пробрасываются в цикл (и в политику гейта).
/// `subagent = Some` → прогон-РЕБЁНОК (см. [`SubagentSpawn`]); `None` → top-level (без регрессии).
#[allow(clippy::too_many_arguments)]
pub async fn run_agent_session(
    spec: &SessionSpec,
    provider: &dyn ToolCapableProvider,
    memory: Option<&dyn AgentMemory>,
    skills: Option<&SkillContext>,
    web: Option<&WebToolsConfig>,
    decision_source: Arc<dyn DecisionSource>,
    writer: &WriteActor,
    reader: &ReadPool,
    paused: &Arc<AtomicBool>,
    cancel: &Arc<AtomicBool>,
    forwarder: Arc<dyn AgentEventForwarder>,
    subagent: Option<&SubagentSpawn<'_>>,
    delegation: Option<&DelegationDeps>,
    research: Option<&crate::ai::ResearchConfig>,
) -> LoopOutcome {
    // Начальный контекст: [system преамбул] + [recall памяти] + [меню скиллов] + [задача]. recall —
    // только чтение, никогда не ошибка (деградирует в пусто); None память → пусто (без регрессии).
    // Меню скиллов (tier-1) — фенсенный user-role блок (данные, не инструкции, I-5); per-request
    // injection_marker. Тела скиллов раскрывает лишь tier-2 `activate_skill`.
    let recalled = match memory {
        Some(mem) => mem.recall(&spec.task, RECALL_BUDGET_TOKENS).await,
        None => Vec::new(),
    };
    let skill_menu: Option<ChatMessage> = skills
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

    let bounds = LoopBounds::default();
    let budget = ContextBudget::from_context_window(spec.context_window);
    let tk = QwenTokenizer::embedded();

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
        let dispatcher: Arc<dyn ActionDispatcher> =
            if let Some(d) = subagent.and_then(|s| s.dispatcher.clone()) {
                d
            } else {
                let ledger = AuditSink::new(writer.clone(), reader.clone());
                // SL-7d: skills-флаги в политику → classify_skill_save видит learning/root. Note/exec не затронуты.
                let policy = DispatchPolicy::with_paused(
                    spec.autonomy.as_deref(),
                    spec.overwrite_threshold,
                    spec.blast_cap,
                    paused.clone(),
                )
                .with_skills_flags(spec.skills_learning_enabled, skills.is_some());
                let events: Arc<dyn EventSink> = Arc::new(ForwardingEventSink(forwarder.clone()));
                // SL-7d: `skill.save` — ТОЛЬКО top-level (`subagent.is_none()`) + learning + skills.
                // Дети навыки не авторствуют. Делит policy/ledger/decision_source/events с note-инструментами.
                if subagent.is_none() && spec.skills_learning_enabled {
                    if let Some(sk) = skills {
                        let skill_ctx = SkillSaveCtx::new(
                            sk.skills_root().to_path_buf(),
                            ledger.clone(),
                            spec.run_id,
                            policy.clone(),
                            decision_source.clone(),
                            events.clone(),
                            Some(writer.clone()),
                        );
                        reg.insert(Arc::new(SkillSaveTool::new(Arc::new(skill_ctx))));
                    }
                }
                // decision_source КЛОНИРУЕМ (не move) — он ещё нужен для `SubagentContext` делегирования ниже.
                Arc::new(GatedToolCtx::new(
                    spec.canon_root.clone(),
                    ledger,
                    spec.run_id,
                    policy,
                    decision_source.clone(),
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
        let _ = (&decision_source, &spec.canon_root);
        ToolRegistry::new()
    };
    // SKILL-2 (tier 2 + 3): READ-ONLY инструменты скиллов (activate_skill + read_skill_resource),
    // НЕЗАВИСИМО от actuator-флага (скиллы только читают). Активация скилла НЕ добавляет иных
    // инструментов (capability-инертность — гейт у SKILL-3).
    // EGR-AGENT: web.search/web.fetch — READ-ONLY (vault не трогают), НЕЗАВИСИМО от actuator-флага.
    // Эгресс — через GuardedClient(EgressFeature::Web) внутри инструментов; per-run RunCtx для аудита.
    if let Some(web) = web {
        for tool in crate::agent::web_tools::web_tools(web, RunCtx::run(spec.run_id)) {
            registry.insert(tool);
        }
    }
    if let Some(skills) = skills {
        for tool in skills.tools() {
            registry.insert(tool);
        }
    }

    // SUB-3 (security keystone проводки): реестр СУБАГЕНТА сужаем до выданного подмножества ПОСЛЕ полной
    // сборки (actuator+skills+web). `allowed` = build_child_registry(child ⊆ parent минус блок-лист) —
    // имя сверху физически удаляется, субагент не вызовет инструмент сверх выданного. Top-level (None) —
    // без сужения.
    if let Some(sa) = subagent {
        registry.retain(sa.allowed);
    }

    // SUB-3b-2b: регистрация `delegate.run` (fan-out субагентов) — ТОЛЬКО top-level (`delegation=Some`,
    // дети его не получают) + `ai.delegation.enabled`. `SubagentContext` собираем из ТЕКУЩИХ хендлов сессии:
    // parent_tool_names = снимок реестра ДО `delegate.run` (он сам в блок-листе ребёнка), gate — общий
    // (parent_dispatcher), один `DelegationBudget` на дерево (клонируется детям, общий счётчик спавнов).
    if let Some(deps) = delegation {
        if deps.config.enabled {
            let sub_ctx = crate::agent::delegate::SubagentContext {
                provider: deps.provider.clone(),
                skills: skills.cloned(),
                web: web.cloned(),
                decision_source: decision_source.clone(),
                writer: writer.clone(),
                reader: reader.clone(),
                paused: paused.clone(),
                parent_cancel: cancel.clone(),
                forwarder: forwarder.clone(),
                parent_run_id: spec.run_id,
                parent_tool_names: registry.names(),
                dispatcher: parent_dispatcher.clone(),
                actuator_enabled: spec.actuator_enabled,
                autonomy: spec.autonomy.clone(),
                overwrite_threshold: spec.overwrite_threshold,
                blast_cap: spec.blast_cap,
                context_window: spec.context_window,
                canon_root: spec.canon_root.clone(),
                model: Some(provider.model_id().to_string()),
                budget: crate::agent::delegate::DelegationBudget::from_config(
                    &deps.config,
                    bounds.wall_clock,
                ),
            };
            registry.insert(Arc::new(crate::agent::delegate::DelegateTool::new(
                sub_ctx,
                deps.config.max_fanout,
            )));
        }
    }

    // RES-4: регистрация `research.run` — ТОЛЬКО top-level (`subagent.is_none()`) + `ai.research.enabled`
    // + `ai.delegation.enabled` (берём Arc-провайдера/капы оттуда) + web включён + actuator (нужен gate для
    // записи отчёта). Воркеры (RES-2) read-only по конструкции; пишет лишь оркестратор через ТОТ ЖЕ
    // parent_dispatcher (общий ledger/blast-cap/undo/kill-switch). Любое условие false → инструмента нет.
    // SECURITY-ИНВАРИАНТ (ревью #2): убрать любой из 4 `Some(..)`-байндингов = регрессия default-OFF
    // (web/gate presence — структурная часть truth-table; компилятор ловит дроп, т.к. все 4 ниже исп.).
    if let (Some(rcfg), Some(deleg), Some(web_cfg), Some(disp)) =
        (research, delegation, web, parent_dispatcher.as_ref())
    {
        if crate::agent::research::tool::should_register(
            rcfg.enabled,
            deleg.config.enabled,
            subagent.is_none(),
        ) {
            let web_seam: Arc<dyn crate::agent::research::ResearchWeb> =
                Arc::new(crate::agent::research::worker::GuardedResearchWeb::new(
                    web_cfg.clone(),
                    RunCtx::run(spec.run_id),
                    false,
                ));
            let rctx = crate::agent::research::ResearchContext {
                web: web_seam,
                provider: deleg.provider.clone(),
                dispatcher: disp.clone(),
                forwarder: forwarder.clone(),
                params: crate::agent::research::ResearchParams::from_config(
                    rcfg,
                    deleg.config.max_fanout,
                ),
                budget_config: deleg.config.clone(),
                wall_clock: bounds.wall_clock,
                paused: paused.clone(),
                cancel: cancel.clone(),
                run_id: spec.run_id,
            };
            registry.insert(Arc::new(crate::agent::research::ResearchTool::new(rctx)));
        }
    }

    // on_event: КАЖДОЕ событие цикла → форвардер (тот же, что у гейта). Запись шага/стрим/лог — забота
    // форвардера (path-specific). Синхронный (как требует `run_agent_loop`).
    let mut on_event = |e: AgentEvent| forwarder.forward(&e);

    // KILL-SWITCH (AGENT-5, чек-пойнт #2): `paused` в цикл — пауза мид-ран остановит на границе хода.
    run_agent_loop(
        provider,
        &registry,
        messages,
        bounds,
        &budget,
        &tk,
        cancel,
        paused,
        RunCtx::run(spec.run_id),
        &mut on_event,
    )
    .await
}

#[cfg(test)]
mod tests;
