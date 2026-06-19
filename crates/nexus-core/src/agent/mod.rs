//! Слой агента (AGENT-1, Фаза 1): типизированная граница вызова инструментов + ограниченный,
//! событие-стримящий цикл + минимальный реестр инструментов. ZERO actuator — инструменты суть
//! безопасные стабы (echo + read-only no-op); актуатор/skills/аппрув/sandbox — более поздние срезы.
//!
//! Состав:
//! - [`event`] — [`AgentEvent`]: поток событий цикла для будущего Agent UI (контракт «бэкенд → фронт»,
//!   `agent-ui-design/CONTRACT-NOTES.md`).
//! - [`tool`] — [`Tool`] трейт + [`ToolSpec`]/[`ToolCall`]/[`ToolError`] (fail-closed на границе, I-4).
//! - [`registry`] — [`ToolRegistry`]: имя→инструмент, `dispatch` (неизвестное → ошибочный результат).
//! - [`runner`] — [`run_agent_loop`]: цикл «спросить → исполнить → зафенсить → назад», ограниченный
//!   [`LoopBounds`] + [`crate::ai::ContextBudget`].
//! - [`stubs`] — безопасные стаб-инструменты ([`EchoTool`]/[`NoopTool`]).
//!
//! tool-capable провайдер ([`crate::ai::tools::ToolCapableProvider`]/`OpenAiToolProvider`) — РАЗДЕЛЬНЫЙ
//! от chat-провайдера тип (I-5/ADR-005): tools не протекают в chat/web путь. Стережёт grep-линт
//! `scripts/check-tooluse.mjs`.

pub mod event;
pub mod registry;
pub mod runner;
pub mod stubs;
pub mod tool;

pub use event::AgentEvent;
pub use registry::{ToolRegistry, ToolResult};
pub use runner::{run_agent_loop, BudgetKind, LoopBounds, LoopOutcome};
pub use stubs::{EchoTool, NoopTool};
pub use tool::{Tool, ToolCall, ToolError, ToolSpec};
