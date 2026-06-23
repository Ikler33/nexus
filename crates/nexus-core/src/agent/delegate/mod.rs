//! Делегирование/субагенты (SUBAGENTS, порт паттернов hermes `delegate_tool.py`) — НЕ новый рантайм, а
//! композиция поверх существующего цикла: субагент = ВТОРОЙ ин-процесс вызов `run_agent_session`
//! (composition root) с изолированным контекстом (`memory=None`), подмножеством инструментов
//! (child ⊆ parent) и ОБЩИМ анти-runaway бюджетом/kill-switch.
//!
//! Прогресс: SUB-0 — safety-примитивы [`DelegationBudget`] (глубина/спавны/дедлайн, fail-closed),
//! `agent_runs.parent_run_id` (миграция 024, см. [`crate::agent::run_store::create_child_run`]), конфиг
//! [`crate::ai::config::DelegationConfig`] (default-OFF). SUB-1 — [`build_child_registry`] (child ⊆ parent,
//! set-intersection) + [`build_child_task`] (фокус-обрамление). SUB-2 — события плана/субагента
//! (`crate::agent::event`). SUB-3a — шов субагента в `run_agent_session` ([`crate::agent::session::SubagentSpawn`]:
//! сужение реестра до allowed + опц. общий dispatcher + skip skill.save). ДАЛЬШЕ: SUB-3b (`spawn_subagent` +
//! `DelegateTool` fan-out поверх run_agent_session + JoinSet).

pub mod budget;
pub mod child_task;
pub mod registry;
pub mod spawn;
pub mod tool;

pub use budget::{BudgetError, DelegationBudget};
pub use child_task::build_child_task;
pub use registry::{build_child_registry, DELEGATE_RUN_TOOL, RESEARCH_RUN_TOOL, SKILL_SAVE_TOOL};
pub use spawn::{spawn_subagent, SubagentContext, SubagentResult};
pub use tool::DelegateTool;
