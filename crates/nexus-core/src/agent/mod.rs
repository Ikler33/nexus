//! Слой агента (AGENT-1, Фаза 1): типизированная граница вызова инструментов + ограниченный,
//! событие-стримящий цикл + минимальный реестр инструментов. ZERO actuator по умолчанию — при ВЫКЛ
//! актуаторе реестр записи ПУСТ (B7); гейтнутые актуаторы/skills/web добавляются своими срезами.
//!
//! Состав:
//! - [`event`] — [`AgentEvent`]: поток событий цикла для будущего Agent UI (контракт «бэкенд → фронт»,
//!   `agent-ui-design/CONTRACT-NOTES.md`).
//! - [`tool`] — [`Tool`] трейт + [`ToolSpec`]/[`ToolCall`]/[`ToolError`] (fail-closed на границе, I-4).
//! - [`registry`] — [`ToolRegistry`]: имя→инструмент, `dispatch` (неизвестное → ошибочный результат).
//! - [`runner`] — [`run_agent_loop`]: цикл «спросить → исполнить → зафенсить → назад», ограниченный
//!   [`LoopBounds`] + [`crate::ai::ContextBudget`].
//! - [`stubs`] — стаб-инструменты ([`EchoTool`]/[`NoopTool`]) ТОЛЬКО для тестов/smoke (B7: в
//!   прод-реестр не регистрируются).
//! - [`run_store`] — async-CRUD над `agent_runs` (миграция 021): статус-машина прогона (AGENT-2).
//! - [`finish`] — КАНОН маппинга [`LoopOutcome`] → терминал run_store ([`outcome_to_finish`] +
//!   [`PausePolicy`]/[`CancelWording`]) — единственный источник статусов/текстов финализации (R-2).
//! - [`job`] — [`AgentRunHandler`] ([`crate::scheduler::JobHandler`]) — прогон цикла как ДОЛГОВЕЧНАЯ
//!   запланированная джоба + корреляция эгресса на run_id + идемпотентный replay (AGENT-2).
//! - [`memory`] — [`AgentMemory`] трейт + [`VaultAgentMemory`] адаптер: мост к 3 слоям памяти Nexus
//!   (факты/переписка/эпизоды) — recall в начальный контекст + Add-only воронка записи (AGENT-MEM-1).
//! - [`skill_tools`] — SKILL-2 3-tier раскрытие скиллов: [`SkillContext`] + tier-1 инъекция каталога
//!   + [`ActivateSkillTool`] (tier 2) + [`ReadSkillResourceTool`] (tier 3). Контент скилла —
//!   НЕДОВЕРЕННЫЕ ДАННЫЕ (фенсен, user/tool-роль, не system); capabilities ИНЕРТНЫ (гейт — SKILL-3).
//! - [`control`] — KILL-SWITCH персист (AGENT-5): [`AgentControlState`] (`agent.json`, зеркало
//!   egress-kill-switch) — пауза агента переживает рестарт; agentd рестор + рантайм-Arc (UI — UI-1).
//!
//! tool-capable провайдер ([`crate::ai::tools::ToolCapableProvider`]/`OpenAiToolProvider`) — РАЗДЕЛЬНЫЙ
//! от chat-провайдера тип (I-5/ADR-005): tools не протекают в chat/web путь. Стережёт grep-линт
//! `scripts/check-tooluse.mjs`.

pub mod connect;
pub mod control;
pub mod delegate;
pub mod event;
pub mod finish;
pub mod job;
pub mod memory;
pub mod registry;
pub mod research;
pub mod run_store;
pub mod runner;
pub mod session;
pub mod skill_tools;
pub mod stubs;
pub mod tool;
pub mod web_tools;

pub use connect::{
    acp_tool_kind, channel_pair, dispatch, event_notification, map_agent_event, negotiate_version,
    AgentFileStatus, AgentProposedFile, AgentStreamEvent, ChannelTransport, ConnectAgentHandler,
    ConnectDeps, ConnectHandler, RpcError, RpcMessage, Transport, PROTOCOL_VERSION,
};
pub use control::{load_control_state, save_control_state, AgentControlState};
pub use delegate::{BudgetError, DelegationBudget};
pub use event::{AgentEvent, FileStatus, ProposedFile};
pub use finish::{outcome_to_finish, CancelWording, PausePolicy, RunFinish};
pub use job::{
    enqueue_agent_run, AgentRunHandler, AGENT_PREAMBLE, KIND_AGENT_RUN, RECALL_BUDGET_TOKENS,
};
pub use memory::{AgentMemory, VaultAgentMemory};
pub use registry::{ToolRegistry, ToolResult};
pub use run_store::{reconcile_orphan_child_runs, requeue_stale_running, AgentRun};
pub use runner::{run_agent_loop, BudgetKind, LoopBounds, LoopOutcome};
pub use session::{
    run_agent_session, AgentEventForwarder, DelegationDeps, SessionDeps, SessionRole, SessionSpec,
};
pub use skill_tools::{
    ActivateSkillTool, ReadSkillResourceTool, SkillContext, ACTIVATE_SKILL_TOOL,
    READ_SKILL_RESOURCE_TOOL,
};
pub use stubs::{EchoTool, NoopTool};
pub use tool::{Tool, ToolCall, ToolError, ToolSpec};
pub use web_tools::{enable_web_tools, WebToolsConfig};
