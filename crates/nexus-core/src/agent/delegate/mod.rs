//! Делегирование/субагенты (SUBAGENTS, порт паттернов hermes `delegate_tool.py`) — НЕ новый рантайм, а
//! композиция поверх существующего цикла: субагент = ВТОРОЙ ин-процесс вызов `run_agent_session`
//! (composition root) с изолированным контекстом (`memory=None`), подмножеством инструментов
//! (child ⊆ parent) и ОБЩИМ анти-runaway бюджетом/kill-switch.
//!
//! Срез SUB-0 закладывает ТОЛЬКО safety-примитивы (без инструмента/спавна): [`DelegationBudget`]
//! (глубина/спавны/дедлайн, fail-closed), `agent_runs.parent_run_id` (миграция 024, дерево прогонов —
//! см. [`crate::agent::run_store::create_child_run`]) и конфиг [`crate::ai::config::DelegationConfig`]
//! (default-OFF). Реестр-подмножество (SUB-1), события (SUB-2) и сам инструмент (SUB-3/4) — следующие срезы.

pub mod budget;

pub use budget::{BudgetError, DelegationBudget};
