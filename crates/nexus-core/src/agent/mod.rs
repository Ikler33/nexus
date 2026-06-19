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
//! - [`run_store`] — async-CRUD над `agent_runs` (миграция 021): статус-машина прогона (AGENT-2).
//! - [`job`] — [`AgentRunHandler`] ([`crate::scheduler::JobHandler`]) — прогон цикла как ДОЛГОВЕЧНАЯ
//!   запланированная джоба + корреляция эгресса на run_id + идемпотентный replay (AGENT-2).
//! - [`memory`] — [`AgentMemory`] трейт + [`VaultAgentMemory`] адаптер: мост к 3 слоям памяти Nexus
//!   (факты/переписка/эпизоды) — recall в начальный контекст + Add-only воронка записи (AGENT-MEM-1).
//! - [`control`] — KILL-SWITCH персист (AGENT-5): [`AgentControlState`] (`agent.json`, зеркало
//!   egress-kill-switch) — пауза агента переживает рестарт; agentd рестор + рантайм-Arc (UI — UI-1).
//!
//! tool-capable провайдер ([`crate::ai::tools::ToolCapableProvider`]/`OpenAiToolProvider`) — РАЗДЕЛЬНЫЙ
//! от chat-провайдера тип (I-5/ADR-005): tools не протекают в chat/web путь. Стережёт grep-линт
//! `scripts/check-tooluse.mjs`.

pub mod control;
pub mod event;
pub mod job;
pub mod memory;
pub mod registry;
pub mod run_store;
pub mod runner;
pub mod stubs;
pub mod tool;

pub use control::{load_control_state, save_control_state, AgentControlState};
pub use event::{AgentEvent, FileStatus, ProposedFile};
pub use job::{enqueue_agent_run, AgentRunHandler, KIND_AGENT_RUN};
pub use memory::{AgentMemory, VaultAgentMemory};
pub use registry::{ToolRegistry, ToolResult};
pub use run_store::{requeue_stale_running, AgentRun};
pub use runner::{run_agent_loop, BudgetKind, LoopBounds, LoopOutcome};
pub use stubs::{EchoTool, NoopTool};
pub use tool::{Tool, ToolCall, ToolError, ToolSpec};
