//! CONN-1 — абстракция агент-бэкенда (фундамент ACP/расцепления).
//!
//! Пять+1 tauri-команд агента (`agent_run`/`approve`/`pause`/`resume`/`cancel`/`undo`) делегируют через
//! трейт [`AgentBackend`], вместо прямого вызова in-process логики. Это шов, в который на CONN-2/ACP-1
//! встанут внешние бэкенды (клиент коннектора к agentd, ACP-клиент к Hermes/любому агенту).
//!
//! На этом срезе подключён ТОЛЬКО [`EmbeddedBackend`] — он БАЙТ-В-БАЙТ зовёт прежние тела команд
//! (`commands::agent::*_impl`), так что поведение и контракт фронта неизменны (нулевая регрессия).
//! Выбор бэкенда — `ai.connection.mode` (default `embedded`, см. `nexus_core::ai::ConnectionConfig`);
//! пока активен всегда Embedded (Connected/ACP — CONN-2+).

use async_trait::async_trait;
use tauri::ipc::Channel;

use crate::commands::agent::{AgentStreamEvent, ApprovalDecision, HistoryMsg};
use crate::error::AppResult;
use crate::state::AppState;

/// Источник прогона агента: in-process (Embedded) | внешний коннектор (CONN-2) | ACP (ACP-1). Команды —
/// тонкие шимы поверх него. Методы берут `&AppState` (Embedded читает live-хендлы как раньше).
#[async_trait]
pub trait AgentBackend: Send + Sync {
    /// Запустить прогон, стримя события в `channel`; вернуть `run_id` сразу (асинхронно).
    async fn run(
        &self,
        state: &AppState,
        task: String,
        autonomy: String,
        history: Vec<HistoryMsg>,
        channel: Channel<AgentStreamEvent>,
    ) -> AppResult<i64>;
    /// Решение по changeset'у (Confirm-тир аппрув/реджект).
    async fn approve(
        &self,
        state: &AppState,
        run_id: i64,
        decisions: Vec<ApprovalDecision>,
    ) -> AppResult<()>;
    /// Пауза прогона (kill-switch).
    async fn pause(&self, state: &AppState, run_id: i64) -> AppResult<()>;
    /// Снять паузу.
    async fn resume(&self, state: &AppState, run_id: i64) -> AppResult<()>;
    /// Кооперативная отмена.
    async fn cancel(&self, state: &AppState, run_id: i64) -> AppResult<()>;
    /// Откат применённых действий прогона. Возвращает число откаченных.
    async fn undo(&self, state: &AppState, run_id: i64) -> AppResult<usize>;
}

/// In-process бэкенд (сегодняшнее поведение). ZST — состояния не держит, читает live `&AppState` per call.
/// Делегирует прежним телам команд (`commands::agent::*_impl`) — без изменения логики.
pub struct EmbeddedBackend;

#[async_trait]
impl AgentBackend for EmbeddedBackend {
    async fn run(
        &self,
        state: &AppState,
        task: String,
        autonomy: String,
        history: Vec<HistoryMsg>,
        channel: Channel<AgentStreamEvent>,
    ) -> AppResult<i64> {
        crate::commands::agent::run_impl(state, task, autonomy, history, channel).await
    }
    async fn approve(
        &self,
        state: &AppState,
        run_id: i64,
        decisions: Vec<ApprovalDecision>,
    ) -> AppResult<()> {
        crate::commands::agent::approve_impl(state, run_id, decisions).await
    }
    async fn pause(&self, state: &AppState, run_id: i64) -> AppResult<()> {
        crate::commands::agent::pause_impl(state, run_id).await
    }
    async fn resume(&self, state: &AppState, run_id: i64) -> AppResult<()> {
        crate::commands::agent::resume_impl(state, run_id).await
    }
    async fn cancel(&self, state: &AppState, run_id: i64) -> AppResult<()> {
        crate::commands::agent::cancel_impl(state, run_id).await
    }
    async fn undo(&self, state: &AppState, run_id: i64) -> AppResult<usize> {
        crate::commands::agent::undo_impl(state, run_id).await
    }
}
