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

/// CONN-2/CONN-4: ЕДИНЫЙ выбор агент-бэкенда по `ai.connection.mode` (default embedded — нулевая
/// регрессия). Local → клиент коннектора к локальному agentd (AF_UNIX, lazy — соединение на первом
/// прогоне, отсутствие демона НЕ ломает выбор); Remote (CONN-3) пока → embedded с предупреждением.
/// Зовётся из `open_vault` (при открытии) И `set_agent_connection` (немедленный своп при смене в UI).
pub fn select_agent_backend(
    cfg: Option<&nexus_core::ai::LocalConfig>,
    #[cfg_attr(not(unix), allow(unused_variables))] root: &std::path::Path,
) -> std::sync::Arc<dyn AgentBackend> {
    use std::sync::Arc;
    let mode = cfg
        .map(|c| c.ai.connection.mode())
        .unwrap_or(nexus_core::ai::ConnectionMode::Embedded);
    match mode {
        nexus_core::ai::ConnectionMode::Local => {
            #[cfg(unix)]
            {
                let socket = cfg
                    .and_then(|c| c.ai.connection.socket.clone())
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| root.join(".nexus").join("agentd.sock"));
                tracing::info!(socket = %socket.display(), "CONN-2: агент-бэкенд = connected (local agentd)");
                Arc::new(ConnectedBackend::new(socket)) as Arc<dyn AgentBackend>
            }
            #[cfg(not(unix))]
            {
                tracing::warn!(
                    "ai.connection.mode=local требует Unix (AF_UNIX) → fallback embedded"
                );
                Arc::new(EmbeddedBackend) as Arc<dyn AgentBackend>
            }
        }
        nexus_core::ai::ConnectionMode::Remote => {
            tracing::warn!(
                "ai.connection.mode=remote ещё не реализован (CONN-3) → fallback embedded"
            );
            Arc::new(EmbeddedBackend)
        }
        nexus_core::ai::ConnectionMode::Acp => {
            let command = cfg.and_then(|c| c.ai.connection.acp_command.clone());
            let cwd = cfg
                .and_then(|c| c.ai.connection.acp_cwd.clone())
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| root.to_path_buf());
            tracing::info!(?command, cwd = %cwd.display(), "ACP-1: агент-бэкенд = acp (spawned subprocess)");
            Arc::new(acp_backend::AcpBackend::new(command, cwd)) as Arc<dyn AgentBackend>
        }
        nexus_core::ai::ConnectionMode::Embedded => Arc::new(EmbeddedBackend),
    }
}

// ── CONN-2: ConnectedBackend — клиент коннектора к локальному agentd (AF_UNIX) ─────────────────────
//
// При `ai.connection.mode="local"` десктоп НЕ гонит цикл in-process, а драйвит ВНЕШНИЙ `nexus-agentd`
// по протоколу AGENT-CONNECT через [`nexus_core::agent::connect::ConnectClient`]. События `agent/event`
// (тот же `AgentStreamEvent`-контракт, что у Channel) форвардятся в Tauri-канал БЕЗ ремапа.
//
// ЧЕСТНЫЕ ЛИМИТЫ CONN-2 (протокол v1.0):
//   R1: `AgentRunParams` не несёт ни history, ни autonomy — сервер one-shot (`history=[]`), autonomy
//       берёт из СВОЕГО конфига. → local-режим = ОДИН ход, без мультитёрн-истории и без per-run
//       autonomy (embedded-путь полноисторичен). Расширение протокола — CONN-3.
//   R2: agentd правит СВОЙ vault (его `canon_root`); для когерентности его `--vault` ДОЛЖЕН совпадать
//       с открытым в десктопе (два писателя в один SQLite — демон авторитетен по агент-записям).
//   R3: reconnect/resume нет — перезапуск agentd сиротит in-flight прогон (переотправь run).
//   R4: события на проводе не несут per-variant run_id → один активный прогон на соединение (коннектор
//       и так держит один активный run на сессию). Корректно для single-owner local.
#[cfg(unix)]
mod connected {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use serde_json::{json, Value};
    use tokio::sync::Mutex;

    use crate::error::AppError;
    use nexus_core::agent::connect::{connect_unix, ConnectClient, UndoResult};

    /// Текущий канал событий активного прогона (форвард-таск шлёт сюда).
    type SharedChannel = Arc<Mutex<Option<Channel<AgentStreamEvent>>>>;

    /// Живое соединение: клиент + карта `run_id→session_id` + канал текущего прогона + форвард-таск.
    struct ConnState {
        client: Arc<ConnectClient>,
        sessions: HashMap<i64, String>,
        current_channel: SharedChannel,
        // Таск самозавершается, когда клиент дропается (read-loop прерывается → events закрывается).
        _forward_task: tokio::task::JoinHandle<()>,
    }

    /// Бэкенд, драйвящий внешний agentd по AF_UNIX. Lazy-connect на первом `run` (отсутствие демона НЕ
    /// ломает открытие vault — ошибка всплывает понятным текстом при запуске прогона).
    pub struct ConnectedBackend {
        socket: PathBuf,
        inner: Mutex<Option<ConnState>>,
        next_session: AtomicU64,
    }

    impl ConnectedBackend {
        /// Синхронный конструктор: НЕ открывает сокет (lazy).
        pub fn new(socket: PathBuf) -> Self {
            Self {
                socket,
                inner: Mutex::new(None),
                next_session: AtomicU64::new(1),
            }
        }

        /// Открывает соединение + handshake. Внятная диагностика (зеркало `nexus status`): нет файла →
        /// «демон не запущен»; не-сокет → мисконфиг. Никогда не паникует.
        async fn connect(&self) -> AppResult<ConnState> {
            use std::os::unix::fs::FileTypeExt;
            match std::fs::symlink_metadata(&self.socket) {
                Ok(m) if !m.file_type().is_socket() => {
                    return Err(AppError::Msg(format!(
                        "{}: путь существует, но это НЕ сокет (проверь ai.connection.socket)",
                        self.socket.display()
                    )))
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(AppError::Msg(format!(
                        "agentd не запущен? сокет {} не найден (`nexus deploy local --apply`)",
                        self.socket.display()
                    )))
                }
                _ => {}
            }
            let transport = connect_unix(&self.socket).await.map_err(|e| {
                AppError::Msg(format!(
                    "подключение к agentd ({}): {e}",
                    self.socket.display()
                ))
            })?;
            let (client, events_rx) = ConnectClient::connect(Arc::new(transport))
                .await
                .map_err(|e| AppError::Msg(format!("handshake agentd: {e}")))?;
            let current_channel: SharedChannel = Arc::new(Mutex::new(None));
            let forward_task = tokio::spawn(forward_events(events_rx, current_channel.clone()));
            Ok(ConnState {
                client: Arc::new(client),
                sessions: HashMap::new(),
                current_channel,
                _forward_task: forward_task,
            })
        }

        /// Достаёт `(client, session_id)` по `run_id`, ОСВОБОЖДАЯ lock (для request'ов, чтобы не держать
        /// `inner` через await). Нет соединения/прогона → понятная ошибка.
        async fn client_and_session(&self, run_id: i64) -> AppResult<(Arc<ConnectClient>, String)> {
            let guard = self.inner.lock().await;
            let st = guard
                .as_ref()
                .ok_or_else(|| AppError::Msg("нет активного соединения с agentd".into()))?;
            let sid = st
                .sessions
                .get(&run_id)
                .cloned()
                .ok_or_else(|| AppError::Msg(format!("прогон {run_id} не активен")))?;
            Ok((st.client.clone(), sid))
        }

        /// pause/resume → `agent/control` (на сервере пауза глобальная — приемлемо для single-owner local).
        async fn control(&self, run_id: i64, pause: bool) -> AppResult<()> {
            let guard = self.inner.lock().await;
            let st = guard
                .as_ref()
                .ok_or_else(|| AppError::Msg("нет активного соединения с agentd".into()))?;
            let sid = st
                .sessions
                .get(&run_id)
                .ok_or_else(|| AppError::Msg(format!("прогон {run_id} не активен")))?;
            st.client
                .notify("agent/control", json!({"sessionId": sid, "pause": pause}))
                .await
                .map_err(|_| AppError::Msg("agent/control: транспорт закрыт".into()))
        }
    }

    /// Форвард-таск: `agent/event`-params → `AgentStreamEvent` → текущий Tauri-канал (без ремапа — тот же
    /// DTO). Закрытие events (демон ушёл) → синтетическая ошибка в канал, чтобы UI не висел.
    async fn forward_events(mut rx: tokio::sync::mpsc::Receiver<Value>, current: SharedChannel) {
        while let Some(v) = rx.recv().await {
            if let Ok(ev) = serde_json::from_value::<AgentStreamEvent>(v) {
                // Терминал хода: после Final/Error освобождаем канал — иначе поздний дисконнект (ниже)
                // отправил бы ЛОЖНУЮ Error в УЖЕ завершённый прогон, и слот не освобождался бы для
                // следующего прогона (R4 single-owner). Ревью CONN-2 MINOR-1.
                let terminal = matches!(
                    ev,
                    AgentStreamEvent::Final { .. } | AgentStreamEvent::Error { .. }
                );
                let mut g = current.lock().await;
                if let Some(ch) = g.as_ref() {
                    let _ = ch.send(ev);
                }
                if terminal {
                    *g = None;
                }
            }
        }
        // Демон ушёл МИД-РАН (канал ещё занят незавершённым прогоном) → синтетическая Error, чтобы UI
        // не висел. После Final/Error канал уже None (выше) — ложной Error не будет.
        if let Some(ch) = current.lock().await.as_ref() {
            let _ = ch.send(AgentStreamEvent::Error {
                message: "соединение с agentd потеряно".into(),
            });
        }
    }

    #[async_trait]
    impl AgentBackend for ConnectedBackend {
        async fn run(
            &self,
            _state: &AppState,
            task: String,
            _autonomy: String, // R1: autonomy НЕ идёт по проводу (сервер берёт из своего конфига)
            _history: Vec<HistoryMsg>, // R1: history НЕ идёт по проводу (сервер one-shot)
            channel: Channel<AgentStreamEvent>,
        ) -> AppResult<i64> {
            let mut guard = self.inner.lock().await;
            if guard.is_none() {
                *guard = Some(self.connect().await?);
            }
            let st = guard.as_mut().unwrap();
            // R4 (ревью CONN-2 MINOR-2): один активный прогон на соединение. Канал ещё занят (прошлый
            // прогон не дошёл до Final/Error — forward_events его не очистил) → отклоняем, иначе события
            // двух прогонов смешались бы в одном канале. Single-owner-модель: UI и так гонит по одному.
            if st.current_channel.lock().await.is_some() {
                return Err(AppError::Msg(
                    "прогон уже идёт (одно соединение — один активный прогон)".into(),
                ));
            }
            *st.current_channel.lock().await = Some(channel.clone());
            let session_id = format!(
                "desktop-{}",
                self.next_session.fetch_add(1, Ordering::Relaxed)
            );
            // Хелпер: при сбое старта прогона освобождаем канал (прогон не стартовал — слот не должен
            // остаться «занятым», иначе следующий run ложно отклонится гардом выше).
            let ack = match st
                .client
                .request(
                    "agent/run",
                    json!({"sessionId": session_id, "prompt": task}),
                )
                .await
            {
                Ok(a) => a,
                Err(e) => {
                    *st.current_channel.lock().await = None;
                    return Err(AppError::Msg(format!("agent/run: {}", e.message)));
                }
            };
            let run_id = match ack
                .get("runId")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<i64>().ok())
            {
                Some(id) => id,
                None => {
                    *st.current_channel.lock().await = None;
                    return Err(AppError::Msg("agent/run: некорректный runId в ack".into()));
                }
            };
            st.sessions.insert(run_id, session_id);
            Ok(run_id)
        }

        async fn approve(
            &self,
            _state: &AppState,
            run_id: i64,
            decisions: Vec<ApprovalDecision>,
        ) -> AppResult<()> {
            let guard = self.inner.lock().await;
            let st = guard
                .as_ref()
                .ok_or_else(|| AppError::Msg("нет активного соединения с agentd".into()))?;
            let sid = st
                .sessions
                .get(&run_id)
                .ok_or_else(|| AppError::Msg(format!("прогон {run_id} не активен")))?;
            let ds: Vec<Value> = decisions
                .iter()
                .map(|d| json!({"actionId": d.action_id, "approved": d.approve}))
                .collect();
            st.client
                .notify(
                    "agent/approve",
                    json!({"sessionId": sid, "runId": run_id.to_string(), "decisions": ds}),
                )
                .await
                .map_err(|_| AppError::Msg("agent/approve: транспорт закрыт".into()))
        }

        async fn pause(&self, _state: &AppState, run_id: i64) -> AppResult<()> {
            self.control(run_id, true).await
        }
        async fn resume(&self, _state: &AppState, run_id: i64) -> AppResult<()> {
            self.control(run_id, false).await
        }

        async fn cancel(&self, _state: &AppState, run_id: i64) -> AppResult<()> {
            let (client, sid) = self.client_and_session(run_id).await?;
            client
                .request(
                    "agent/cancel",
                    json!({"sessionId": sid, "runId": run_id.to_string()}),
                )
                .await
                .map(|_| ())
                .map_err(|e| AppError::Msg(format!("agent/cancel: {}", e.message)))
        }

        async fn undo(&self, _state: &AppState, run_id: i64) -> AppResult<usize> {
            let (client, sid) = self.client_and_session(run_id).await?;
            let res = client
                .request(
                    "agent/undo",
                    json!({"sessionId": sid, "runId": run_id.to_string()}),
                )
                .await
                .map_err(|e| AppError::Msg(format!("agent/undo: {}", e.message)))?;
            let u: UndoResult = serde_json::from_value(res)
                .map_err(|_| AppError::Msg("agent/undo: некорректный результат".into()))?;
            Ok(u.restored as usize)
        }
    }
}

#[cfg(unix)]
pub use connected::ConnectedBackend;

// ── ACP-1: AcpBackend — клиент ACP к ВНЕШНЕМУ агенту (Hermes и пр.), спавненному подпроцессом ───────
//
// При `ai.connection.mode="acp"` десктоп спавнит ACP-агента (`acp_command`, напр. `["hermes","acp"]`) и
// драйвит его по ACP (stdio JSON-RPC) через [`nexus_core::agent::connect::acp::AcpClient`]. `session/update`
// → `AgentStreamEvent` → Tauri-канал; входящий `session/request_permission` → синтетический `action_id` →
// `Proposal` в UI; `agent_approve` → ACP-ответ (Selected/Cancelled). Файлы агент пишет САМ (наши
// capabilities=false) — наш «актуатор» = ТОЛЬКО решение по permission. Кросс-платформенно (без cfg(unix)).
//
// ЧЕСТНЫЕ ЛИМИТЫ (первый срез, см. docs/specs/acp-client.md):
//   R1: `session/prompt` НЕ несёт history/autonomy (сессии stateful у агента) → один ход на прогон.
//   R2: агент правит СВОЙ `cwd` (наш vault, только если acp_cwd == корень vault); caps=false.
//   R3: undo → Ok(0) (нет леджера для записей агента); pause/resume → Err (в ACP нет паузы).
//   R4: один активный прогон на соединение; соединение = спавн агента ПЕР-ПРОГОН (переиспользование
//       сессии для мультитёрна — отложено).
//   R5: нет reconnect (краш агента → синтетическая Error в канал, переотправь).
//   R6: только ACP v1 stable (unstable session-fork выключен).
mod acp_backend {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::json;
    use tokio::sync::Mutex;

    use crate::error::AppError;
    use nexus_core::agent::connect::acp::{
        acp_kind_to_display, schema, AcpClient, InboundPermission, ACP_PROTOCOL_VERSION,
    };
    use nexus_core::agent::connect::{AgentFileStatus, AgentProposedFile, StdioTransport};

    /// Текущий канал событий активного прогона. `None` после терминала (R4-слот свободен).
    type SharedChannel = Arc<Mutex<Option<Channel<AgentStreamEvent>>>>;

    /// Таймаут управляющих RPC (`initialize`/`session/new`). `session/prompt` — БЕЗ таймаута (cold-start 1-3м).
    const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);

    /// Висящий `request_permission`: `rpc_id` запроса агента + опции (для маппинга approve→outcome).
    struct PendingPerm {
        rpc_id: serde_json::Value,
        options: Vec<(String, schema::PermissionOptionKind)>,
    }
    type PendingPerms = Arc<Mutex<HashMap<i64, PendingPerm>>>;

    /// Живое ACP-соединение прогона.
    struct AcpState {
        client: Arc<AcpClient>,
        session_id: String,
        current_channel: SharedChannel,
        pending_perms: PendingPerms,
        // drive-таск самозавершается на терминале хода; дроп AcpState → дроп client → агент убит (kill_on_drop).
        _drive_task: tokio::task::JoinHandle<()>,
    }

    /// Бэкенд, драйвящий внешний ACP-агент. Lazy-spawn на первом `run`.
    pub struct AcpBackend {
        command: Option<Vec<String>>,
        cwd: PathBuf,
        inner: Mutex<Option<AcpState>>,
        next_run: AtomicI64,
        next_action: Arc<AtomicI64>,
    }

    impl AcpBackend {
        pub fn new(command: Option<Vec<String>>, cwd: PathBuf) -> Self {
            Self {
                command,
                cwd,
                inner: Mutex::new(None),
                next_run: AtomicI64::new(1),
                next_action: Arc::new(AtomicI64::new(1)),
            }
        }
    }

    /// Отправить событие в текущий канал прогона (no-op, если канал уже освобождён).
    async fn send_ev(current: &SharedChannel, ev: AgentStreamEvent) {
        if let Some(ch) = current.lock().await.as_ref() {
            let _ = ch.send(ev);
        }
    }

    /// Извлекает поверхность аппрува из tool_call'а permission-запроса: путь + грубый счёт строк + статус.
    /// (ACP-1: счёт строк грубый — full-replace; точный line-diff — refinement.)
    fn extract_proposal(tc: &schema::ToolCallUpdate) -> (String, u32, u32, AgentFileStatus) {
        let diff = tc.content.as_ref().and_then(|c| {
            c.iter().find_map(|x| match x {
                schema::ToolCallContent::Diff(d) => Some(d),
                _ => None,
            })
        });
        match diff {
            Some(d) => {
                let add = d.new_text.lines().count() as u32;
                let del = d
                    .old_text
                    .as_ref()
                    .map(|o| o.lines().count() as u32)
                    .unwrap_or(0);
                let status = if d.old_text.is_none() {
                    AgentFileStatus::New
                } else {
                    AgentFileStatus::Edit
                };
                (d.path.to_string_lossy().into_owned(), add, del, status)
            }
            // нет diff (exec/fetch-permission) → деградируем: показываем заголовок (action_id всё равно есть).
            None => (
                tc.title.clone().unwrap_or_else(|| "действие агента".into()),
                0,
                0,
                AgentFileStatus::Edit,
            ),
        }
    }

    /// Входящий permission → синтетический `action_id` + регистрация в `pending_perms` + `Proposal` в UI.
    async fn handle_permission(
        current: &SharedChannel,
        pending_perms: &PendingPerms,
        next_action: &AtomicI64,
        run_id: i64,
        inbound: InboundPermission,
    ) {
        let action_id = next_action.fetch_add(1, Ordering::Relaxed);
        let (path, add, del, status) = extract_proposal(&inbound.params.tool_call);
        pending_perms.lock().await.insert(
            action_id,
            PendingPerm {
                rpc_id: inbound.id,
                options: inbound
                    .params
                    .options
                    .iter()
                    .map(|o| (o.option_id.clone(), o.kind))
                    .collect(),
            },
        );
        send_ev(
            current,
            AgentStreamEvent::Proposal {
                run_id,
                files: vec![AgentProposedFile {
                    path,
                    add,
                    del,
                    status,
                    action_id,
                }],
            },
        )
        .await;
    }

    /// Выбирает ACP-outcome по решению юзера: approve → AllowOnce|AllowAlways; reject → RejectOnce|RejectAlways.
    /// Нет подходящей опции (нестандартный набор) → **fail-closed Cancelled** (НИКОГДА не авто-allow). Чистая
    /// функция — юнит-тестируема.
    fn pick_outcome(
        options: &[(String, schema::PermissionOptionKind)],
        approve: bool,
    ) -> serde_json::Value {
        use schema::PermissionOptionKind as K;
        let want: &[K] = if approve {
            &[K::AllowOnce, K::AllowAlways]
        } else {
            &[K::RejectOnce, K::RejectAlways]
        };
        let picked = want.iter().find_map(|w| {
            options
                .iter()
                .find(|(_, k)| k == w)
                .map(|(id, _)| id.clone())
        });
        match picked {
            Some(option_id) => json!({"outcome": {"outcome": "selected", "optionId": option_id}}),
            None => json!({"outcome": {"outcome": "cancelled"}}),
        }
    }

    /// Маппит одно `session/update` в события для UI (кроме accum-текста — он копится в drive-цикле).
    fn map_update(update: schema::SessionUpdate) -> Vec<AgentStreamEvent> {
        use schema::{ContentBlock, SessionUpdate as U, ToolCallContent, ToolCallStatus};
        match update {
            U::AgentMessageChunk { .. } | U::AgentThoughtChunk { .. } => Vec::new(), // обрабатываются в цикле (accum)
            U::ToolCall(tc) => vec![AgentStreamEvent::ToolCall {
                id: tc.tool_call_id,
                kind: acp_kind_to_display(tc.kind).to_string(),
                args: tc.raw_input.map(|v| v.to_string()).unwrap_or_default(),
            }],
            U::ToolCallUpdate(u) => match u.status {
                Some(ToolCallStatus::Completed) | Some(ToolCallStatus::Failed) => {
                    let is_error = matches!(u.status, Some(ToolCallStatus::Failed));
                    // Текст результата = конкатенация текстовых content-блоков (без сырого diff).
                    let content = u
                        .content
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|c| match c {
                            ToolCallContent::Content {
                                content: ContentBlock::Text { text },
                            } => Some(text),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    vec![AgentStreamEvent::ToolResult {
                        id: u.tool_call_id,
                        content,
                        is_error,
                    }]
                }
                _ => Vec::new(), // pending/in_progress апдейты — не финализируем tool
            },
            U::Other => Vec::new(),
        }
    }

    /// Достаёт текст из чанка ассистента/мышления (для accum в Final).
    fn chunk_text(update: &schema::SessionUpdate) -> Option<&str> {
        use schema::{ContentBlock, SessionUpdate as U};
        match update {
            U::AgentMessageChunk {
                content: ContentBlock::Text { text },
            }
            | U::AgentThoughtChunk {
                content: ContentBlock::Text { text },
            } => Some(text.as_str()),
            _ => None,
        }
    }

    /// Drive-таск прогона: гонит `session/prompt` (без таймаута) + параллельно пампит updates/perms в канал,
    /// до терминала хода. На терминале — финальное событие + разрешение висящих permission в Cancelled.
    #[allow(clippy::too_many_arguments)]
    async fn drive_run(
        client: Arc<AcpClient>,
        mut updates: tokio::sync::mpsc::Receiver<schema::SessionNotification>,
        mut perms: tokio::sync::mpsc::Receiver<InboundPermission>,
        current: SharedChannel,
        pending_perms: PendingPerms,
        next_action: Arc<AtomicI64>,
        run_id: i64,
        session_id: String,
        task: String,
    ) {
        let prompt = client.request(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": [{"type":"text","text": task}]}),
            None, // R1/cold-start: без таймаута на весь ход
        );
        tokio::pin!(prompt);
        let mut answer = String::new();

        let terminal: AgentStreamEvent = loop {
            tokio::select! {
                res = &mut prompt => {
                    break match res {
                        Ok(v) => {
                            let stop = v.get("stopReason").and_then(|s| s.as_str()).unwrap_or("end_turn");
                            match stop {
                                "refusal" => AgentStreamEvent::Error { message: "ACP-агент отклонил запрос (refusal)".into() },
                                "cancelled" => AgentStreamEvent::Error { message: "прогон отменён".into() },
                                _ => AgentStreamEvent::Final { text: std::mem::take(&mut answer) },
                            }
                        }
                        Err(e) => AgentStreamEvent::Error { message: format!("ACP session/prompt: {}", e.message) },
                    };
                }
                n = updates.recv() => match n {
                    Some(notif) => {
                        if let Some(t) = chunk_text(&notif.update) {
                            answer.push_str(t);
                            send_ev(&current, AgentStreamEvent::AssistantToken { text: t.to_string() }).await;
                        } else {
                            for ev in map_update(notif.update) { send_ev(&current, ev).await; }
                        }
                    }
                    None => break AgentStreamEvent::Error { message: "ACP-агент отключился".into() },
                },
                p = perms.recv() => {
                    if let Some(inbound) = p {
                        handle_permission(&current, &pending_perms, &next_action, run_id, inbound).await;
                    }
                }
            }
        };

        // Best-effort дренаж буферизованных апдейтов перед терминалом.
        while let Ok(notif) = updates.try_recv() {
            if let Some(t) = chunk_text(&notif.update) {
                send_ev(
                    &current,
                    AgentStreamEvent::AssistantToken {
                        text: t.to_string(),
                    },
                )
                .await;
            } else {
                for ev in map_update(notif.update) {
                    send_ev(&current, ev).await;
                }
            }
        }
        // Висящие permission на конце хода → Cancelled (fail-closed: ход окончен, агент ждать не должен).
        {
            let mut pp = pending_perms.lock().await;
            for (_, perm) in pp.drain() {
                let _ = client
                    .respond(
                        perm.rpc_id,
                        Ok(json!({"outcome": {"outcome": "cancelled"}})),
                    )
                    .await;
            }
        }
        send_ev(&current, terminal).await;
        *current.lock().await = None; // освобождаем R4-слот
    }

    #[async_trait]
    impl AgentBackend for AcpBackend {
        async fn run(
            &self,
            _state: &AppState,
            task: String,
            _autonomy: String, // R1: autonomy не идёт по проводу (агент берёт из своего конфига)
            _history: Vec<HistoryMsg>, // R1: history не идёт по проводу (сессии stateful у агента)
            channel: Channel<AgentStreamEvent>,
        ) -> AppResult<i64> {
            let command = self
                .command
                .as_ref()
                .filter(|c| !c.is_empty())
                .ok_or_else(|| {
                    AppError::Msg(
                        "ai.connection.acp_command не задан (напр. [\"hermes\",\"acp\"])".into(),
                    )
                })?;

            let mut guard = self.inner.lock().await;
            // R4: один активный прогон. Прошлый завершён (канал освобождён) → заменяем (старый client дропнется
            // → агент-подпроцесс убьётся kill_on_drop).
            if let Some(st) = guard.as_ref() {
                if st.current_channel.lock().await.is_some() {
                    return Err(AppError::Msg(
                        "ACP-прогон уже идёт (один активный прогон на соединение)".into(),
                    ));
                }
            }

            let (program, args) = command
                .split_first()
                .expect("command непустой (проверено выше)");
            let transport = StdioTransport::spawn(program, args, &self.cwd)
                .await
                .map_err(|e| AppError::Msg(format!("спавн ACP-агента `{program}`: {e}")))?;
            let (client, updates_rx, perms_rx) = AcpClient::new(Arc::new(transport));
            let client = Arc::new(client);

            client
                .request(
                    "initialize",
                    json!({"protocolVersion": ACP_PROTOCOL_VERSION, "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false}}),
                    Some(CONTROL_TIMEOUT),
                )
                .await
                .map_err(|e| AppError::Msg(format!("ACP initialize: {}", e.message)))?;
            let new_res = client
                .request(
                    "session/new",
                    json!({"cwd": self.cwd, "mcpServers": []}),
                    Some(CONTROL_TIMEOUT),
                )
                .await
                .map_err(|e| AppError::Msg(format!("ACP session/new: {}", e.message)))?;
            let session_id = new_res
                .get("sessionId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AppError::Msg("ACP session/new: нет sessionId в ответе".into()))?
                .to_string();

            let run_id = self.next_run.fetch_add(1, Ordering::Relaxed);
            let current_channel: SharedChannel = Arc::new(Mutex::new(Some(channel)));
            let pending_perms: PendingPerms = Arc::new(Mutex::new(HashMap::new()));
            let drive = tokio::spawn(drive_run(
                client.clone(),
                updates_rx,
                perms_rx,
                current_channel.clone(),
                pending_perms.clone(),
                self.next_action.clone(),
                run_id,
                session_id.clone(),
                task,
            ));
            *guard = Some(AcpState {
                client,
                session_id,
                current_channel,
                pending_perms,
                _drive_task: drive,
            });
            Ok(run_id)
        }

        async fn approve(
            &self,
            _state: &AppState,
            _run_id: i64,
            decisions: Vec<ApprovalDecision>,
        ) -> AppResult<()> {
            let guard = self.inner.lock().await;
            let st = guard
                .as_ref()
                .ok_or_else(|| AppError::Msg("нет активного ACP-прогона".into()))?;
            for d in decisions {
                let perm = st.pending_perms.lock().await.remove(&d.action_id);
                let Some(perm) = perm else { continue }; // уже разрешён/неизвестен — игнор
                let body = pick_outcome(&perm.options, d.approve);
                let _ = st.client.respond(perm.rpc_id, Ok(body)).await;
            }
            Ok(())
        }

        async fn pause(&self, _state: &AppState, _run_id: i64) -> AppResult<()> {
            Err(AppError::Msg(
                "пауза не поддерживается ACP-агентом (R3)".into(),
            ))
        }
        async fn resume(&self, _state: &AppState, _run_id: i64) -> AppResult<()> {
            Err(AppError::Msg(
                "возобновление не поддерживается ACP-агентом (R3)".into(),
            ))
        }

        async fn cancel(&self, _state: &AppState, _run_id: i64) -> AppResult<()> {
            let guard = self.inner.lock().await;
            let st = guard
                .as_ref()
                .ok_or_else(|| AppError::Msg("нет активного ACP-прогона".into()))?;
            let _ = st
                .client
                .notify("session/cancel", json!({"sessionId": st.session_id}))
                .await;
            // Висящие permission → Cancelled (агент не должен ждать после отмены).
            let mut pp = st.pending_perms.lock().await;
            for (_, perm) in pp.drain() {
                let _ = st
                    .client
                    .respond(
                        perm.rpc_id,
                        Ok(json!({"outcome": {"outcome": "cancelled"}})),
                    )
                    .await;
            }
            Ok(())
        }

        async fn undo(&self, _state: &AppState, _run_id: i64) -> AppResult<usize> {
            // R3: нет леджера для записей агента (он пишет в своей песочнице) → откатывать нечего.
            Ok(0)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use schema::{
            ContentBlock, Diff, PermissionOptionKind as K, SessionUpdate, ToolCall,
            ToolCallContent, ToolCallStatus, ToolCallUpdate, ToolKind,
        };

        fn tc_update_with_diff(old: Option<&str>, new: &str) -> ToolCallUpdate {
            ToolCallUpdate {
                tool_call_id: "t1".into(),
                status: None,
                content: Some(vec![ToolCallContent::Diff(Diff {
                    path: "Notes/A.md".into(),
                    old_text: old.map(str::to_string),
                    new_text: new.into(),
                })]),
                title: Some("edit Notes/A.md".into()),
                kind: None,
            }
        }

        #[test]
        fn extract_proposal_new_file_from_diff() {
            let (path, add, del, status) = extract_proposal(&tc_update_with_diff(None, "a\nb\nc"));
            assert_eq!(path, "Notes/A.md");
            assert_eq!((add, del), (3, 0));
            assert_eq!(status, AgentFileStatus::New);
        }

        #[test]
        fn extract_proposal_edit_from_diff() {
            let (_, add, del, status) =
                extract_proposal(&tc_update_with_diff(Some("a\nb"), "a\nb\nc\nd"));
            assert_eq!((add, del), (4, 2));
            assert_eq!(status, AgentFileStatus::Edit);
        }

        #[test]
        fn extract_proposal_degraded_without_diff() {
            let tc = ToolCallUpdate {
                tool_call_id: "t1".into(),
                status: None,
                content: None,
                title: Some("run `ls`".into()),
                kind: None,
            };
            let (path, add, del, status) = extract_proposal(&tc);
            assert_eq!(path, "run `ls`");
            assert_eq!((add, del), (0, 0));
            assert_eq!(status, AgentFileStatus::Edit);
        }

        #[test]
        fn map_update_tool_call_and_result() {
            let tc = ToolCall {
                tool_call_id: "t1".into(),
                title: "search".into(),
                kind: ToolKind::Search,
                status: ToolCallStatus::Pending,
                content: vec![],
                raw_input: Some(serde_json::json!({"q": "rust"})),
            };
            let evs = map_update(SessionUpdate::ToolCall(tc));
            assert!(matches!(
                evs.first(),
                Some(AgentStreamEvent::ToolCall { kind, .. }) if kind == "search"
            ));

            let done = ToolCallUpdate {
                tool_call_id: "t1".into(),
                status: Some(ToolCallStatus::Completed),
                content: Some(vec![ToolCallContent::Content {
                    content: ContentBlock::Text { text: "ok".into() },
                }]),
                title: None,
                kind: None,
            };
            let evs = map_update(SessionUpdate::ToolCallUpdate(done));
            assert!(matches!(
                evs.first(),
                Some(AgentStreamEvent::ToolResult { content, is_error: false, .. }) if content == "ok"
            ));

            // pending tool_call_update → НЕ финализирует tool (пусто)
            let pending = ToolCallUpdate {
                tool_call_id: "t1".into(),
                status: Some(ToolCallStatus::InProgress),
                content: None,
                title: None,
                kind: None,
            };
            assert!(map_update(SessionUpdate::ToolCallUpdate(pending)).is_empty());
        }

        #[test]
        fn map_update_failed_tool_is_error() {
            let failed = ToolCallUpdate {
                tool_call_id: "t1".into(),
                status: Some(ToolCallStatus::Failed),
                content: None,
                title: None,
                kind: None,
            };
            assert!(matches!(
                map_update(SessionUpdate::ToolCallUpdate(failed)).first(),
                Some(AgentStreamEvent::ToolResult { is_error: true, .. })
            ));
        }

        #[test]
        fn chunk_text_extracts_message_and_thought() {
            let msg = SessionUpdate::AgentMessageChunk {
                content: ContentBlock::Text { text: "hi".into() },
            };
            assert_eq!(chunk_text(&msg), Some("hi"));
            let thought = SessionUpdate::AgentThoughtChunk {
                content: ContentBlock::Text {
                    text: "thinking".into(),
                },
            };
            assert_eq!(chunk_text(&thought), Some("thinking"));
        }

        #[test]
        fn pick_outcome_is_fail_closed() {
            let opts = vec![
                ("a".to_string(), K::AllowOnce),
                ("d".to_string(), K::RejectOnce),
            ];
            assert_eq!(
                pick_outcome(&opts, true),
                json!({"outcome": {"outcome": "selected", "optionId": "a"}})
            );
            assert_eq!(
                pick_outcome(&opts, false),
                json!({"outcome": {"outcome": "selected", "optionId": "d"}})
            );
            // approve, но НЕТ allow-опции → fail-closed Cancelled (НЕ авто-allow)
            let only_reject = vec![("d".to_string(), K::RejectOnce)];
            assert_eq!(
                pick_outcome(&only_reject, true),
                json!({"outcome": {"outcome": "cancelled"}})
            );
            // allow_always когда нет allow_once
            let aa = vec![("x".to_string(), K::AllowAlways)];
            assert_eq!(
                pick_outcome(&aa, true),
                json!({"outcome": {"outcome": "selected", "optionId": "x"}})
            );
        }
    }
}

pub use acp_backend::AcpBackend;
