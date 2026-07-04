//! Регистр-хелперы ФИЧ agent-сессии (R-4): web / skills / delegation / research. Вынесены из тела
//! [`super::run_agent_session`] — каждый хелпер держит СВОИ условия регистрации (default-OFF
//! truth-table) внутри, а не размазывает их по телу композиции. **Behavior-preserving**: фактическая
//! структура условий — та же, что была инлайном; `register_delegation`/`register_research` зовутся
//! ТОЛЬКО из TopLevel-ветки [`super::SessionRole`] (у субагентов этих каналов нет по построению типа).

use std::sync::Arc;
use std::time::Duration;

use crate::actuator::ActionDispatcher;
use crate::net::RunCtx;

use super::super::registry::ToolRegistry;
use super::{DelegationDeps, SessionDeps, SessionSpec};

/// EGR-AGENT: web.search/web.fetch — READ-ONLY (vault не трогают), НЕЗАВИСИМО от actuator-флага.
/// Эгресс — через GuardedClient(EgressFeature::Web) внутри инструментов; per-run RunCtx для аудита.
/// `deps.web = None` → без web-инструментов (без регрессии).
pub(super) fn register_web(reg: &mut ToolRegistry, deps: &SessionDeps<'_>, spec: &SessionSpec) {
    if let Some(web) = deps.web {
        for tool in crate::agent::web_tools::web_tools(web, RunCtx::run(spec.run_id)) {
            reg.insert(tool);
        }
    }
}

/// SKILL-2 (tier 2 + 3): READ-ONLY инструменты скиллов (activate_skill + read_skill_resource),
/// НЕЗАВИСИМО от actuator-флага (скиллы только читают). Активация скилла НЕ добавляет иных
/// инструментов (capability-инертность — гейт у SKILL-3). `deps.skills = None` → без них.
pub(super) fn register_skills(reg: &mut ToolRegistry, deps: &SessionDeps<'_>) {
    if let Some(skills) = deps.skills {
        for tool in skills.tools() {
            reg.insert(tool);
        }
    }
}

/// SUB-3b-2b: регистрация `delegate.run` (fan-out субагентов) — зовётся ТОЛЬКО из TopLevel-ветки
/// [`super::SessionRole`] (у субагентов канала delegation НЕТ — рекурсия-стоп по построению) и
/// требует `ai.delegation.enabled`. `SubagentContext` собираем из ТЕКУЩИХ хендлов сессии:
/// parent_tool_names = снимок реестра ДО `delegate.run` (он сам в блок-листе ребёнка), gate — общий
/// (parent_dispatcher), один `DelegationBudget` на дерево (клонируется детям, общий счётчик спавнов).
pub(super) fn register_delegation(
    reg: &mut ToolRegistry,
    deps: &SessionDeps<'_>,
    spec: &SessionSpec,
    delegation: Option<&DelegationDeps>,
    parent_dispatcher: Option<&Arc<dyn ActionDispatcher>>,
    wall_clock: Duration,
) {
    if let Some(dd) = delegation {
        if dd.config.enabled {
            let sub_ctx = crate::agent::delegate::SubagentContext {
                provider: dd.provider.clone(),
                skills: deps.skills.cloned(),
                web: deps.web.cloned(),
                decision_source: deps.decision_source.clone(),
                writer: deps.writer.clone(),
                reader: deps.reader.clone(),
                paused: deps.paused.clone(),
                parent_cancel: deps.cancel.clone(),
                forwarder: deps.forwarder.clone(),
                parent_run_id: spec.run_id,
                parent_tool_names: reg.names(),
                dispatcher: parent_dispatcher.cloned(),
                actuator_enabled: spec.actuator_enabled,
                autonomy: spec.autonomy.clone(),
                overwrite_threshold: spec.overwrite_threshold,
                blast_cap: spec.blast_cap,
                context_window: spec.context_window,
                canon_root: spec.canon_root.clone(),
                model: Some(deps.provider.model_id().to_string()),
                budget: crate::agent::delegate::DelegationBudget::from_config(
                    &dd.config, wall_clock,
                ),
            };
            reg.insert(Arc::new(crate::agent::delegate::DelegateTool::new(
                sub_ctx,
                dd.config.max_fanout,
            )));
        }
    }
}

/// RES-4: регистрация `research.run` — ВСЕ 5 условий default-OFF внутри этого хелпера:
/// (1) `ai.research.enabled`, (2) `ai.delegation.enabled` (берём Arc-провайдера/капы оттуда),
/// (3) top-level — гарантирован ТИПОМ (хелпер зовётся только из TopLevel-ветки
/// [`super::SessionRole`]; субагенты не ресёрчат), (4) web включён, (5) actuator-gate построен
/// (нужен для записи отчёта). Воркеры (RES-2) read-only по конструкции; пишет лишь оркестратор через
/// ТОТ ЖЕ parent_dispatcher (общий ledger/blast-cap/undo/kill-switch). Любое условие false →
/// инструмента нет.
/// SECURITY-ИНВАРИАНТ (ревью #2): убрать любой из 4 `Some(..)`-байндингов = регрессия default-OFF
/// (web/gate presence — структурная часть truth-table; компилятор ловит дроп, т.к. все 4 ниже исп.).
pub(super) fn register_research(
    reg: &mut ToolRegistry,
    deps: &SessionDeps<'_>,
    spec: &SessionSpec,
    research: Option<&crate::ai::ResearchConfig>,
    delegation: Option<&DelegationDeps>,
    parent_dispatcher: Option<&Arc<dyn ActionDispatcher>>,
    wall_clock: Duration,
) {
    if let (Some(rcfg), Some(deleg), Some(web_cfg), Some(disp)) =
        (research, delegation, deps.web, parent_dispatcher)
    {
        if crate::agent::research::tool::should_register(
            rcfg.enabled,
            deleg.config.enabled,
            // top-level: по построению SessionRole (см. док хелпера) — у субагентов канала research нет.
            true,
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
                forwarder: deps.forwarder.clone(),
                params: crate::agent::research::ResearchParams::from_config(
                    rcfg,
                    deleg.config.max_fanout,
                ),
                budget_config: deleg.config.clone(),
                wall_clock,
                paused: deps.paused.clone(),
                cancel: deps.cancel.clone(),
                run_id: spec.run_id,
            };
            reg.insert(Arc::new(crate::agent::research::ResearchTool::new(rctx)));
        }
    }
}
