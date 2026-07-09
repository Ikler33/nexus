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
    /// W-38: `session_id` группирует ходы одной переписки (история) — Embedded персистит его; внешние
    /// бэкенды (Connected/ACP) пока его игнорируют (история — десктоп-embedded-фича).
    async fn run(
        &self,
        state: &AppState,
        task: String,
        autonomy: String,
        history: Vec<HistoryMsg>,
        session_id: String,
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
        session_id: String,
        channel: Channel<AgentStreamEvent>,
    ) -> AppResult<i64> {
        crate::commands::agent::run_impl(state, task, autonomy, history, session_id, channel).await
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
            // ACP-REMOTE-SSH: итоговый argv через резолвер транспорта (ssh-сборка при
            // acp_transport="ssh", иначе локальный acp_command). AcpBackend по-прежнему получает
            // Option<Vec<String>> — спавн/реюз/UI-контракт неизменны.
            let command = cfg.and_then(|c| c.ai.connection.acp_spawn_argv());
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
    use nexus_core::agent::connect::{
        classify_socket, connect_unix, ConnectClient, SocketDiag, UndoResult,
    };

    /// CONN-4: байт-прежнее сообщение диагностики сокета для lazy-connect бэкенда по вердикту канона
    /// [`classify_socket`]. `None` — путь пригоден (connect продолжается). Тексты специфичны для
    /// backend (`ai.connection.socket`, `nexus deploy local --apply`) — маппинг здесь, не в ядре.
    fn connect_socket_diag_err(diag: SocketDiag, socket: &std::path::Path) -> Option<String> {
        match diag {
            SocketDiag::NotSocket => Some(format!(
                "{}: путь существует, но это НЕ сокет (проверь ai.connection.socket)",
                socket.display()
            )),
            SocketDiag::Missing => Some(format!(
                "agentd не запущен? сокет {} не найден (`nexus deploy local --apply`)",
                socket.display()
            )),
            SocketDiag::Usable => None,
        }
    }

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
            // Внятная диагностика (зеркало `nexus status`) — ЕДИНАЯ классификация в ядре
            // (`classify_socket`), байт-прежний backend-текст маппит `connect_socket_diag_err`.
            if let Some(e) = connect_socket_diag_err(classify_socket(&self.socket), &self.socket) {
                return Err(AppError::Msg(e));
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
            _session_id: String, // W-38: история — embedded-фича; коннектор группировку не персистит
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

    // CONN-4/R-12b: характеризация БАЙТ-ПРЕЖНИХ backend-текстов диагностики сокета после дедупа
    // (канон `classify_socket` в ядре; тексты — тут). Пинят точные строки. В КОНЦЕ модуля
    // (clippy::items_after_test_module: за тест-модулем не должно быть прод-элементов).
    #[cfg(test)]
    mod diag_tests {
        use super::{connect_socket_diag_err, SocketDiag};
        use std::path::Path;

        #[test]
        fn connect_socket_diag_messages_byte_exact() {
            let p = Path::new("/v/.nexus/agentd.sock");
            assert_eq!(
                connect_socket_diag_err(SocketDiag::NotSocket, p).unwrap(),
                "/v/.nexus/agentd.sock: путь существует, но это НЕ сокет (проверь ai.connection.socket)"
            );
            assert_eq!(
                connect_socket_diag_err(SocketDiag::Missing, p).unwrap(),
                "agentd не запущен? сокет /v/.nexus/agentd.sock не найден (`nexus deploy local --apply`)"
            );
            assert!(connect_socket_diag_err(SocketDiag::Usable, p).is_none());
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
//   R4: соединение + сессия ПЕРЕИСПОЛЬЗУЮТСЯ между ходами (perf: спавн+initialize+session/new ОДИН раз;
//       первый ход греет cold-start ~9.5с, каждый следующий = только новый `session/prompt` по той же
//       сессии → почти мгновенно). ОДИН активный ход на соединение (R2): пока ход не дошёл до Final/Error,
//       новый run() отклоняется. Мёртвое соединение (`!client.is_alive()` — агент отвалился) переспавнится
//       на следующем run() (старый AcpState дропнется → агент убит kill_on_drop).
//   R5: нет reconnect МИД-хода (краш агента → синтетическая Error в канал; следующий run() переспавнит).
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
    use nexus_core::agent::connect::{
        AgentFileStatus, AgentPlanStep, AgentPlanStepState, AgentProposedFile, AgentProposedKind,
        StdioTransport, Transport,
    };

    /// Текущий канал событий активного прогона. `None` после терминала (R4-слот свободен).
    type SharedChannel = Arc<Mutex<Option<Channel<AgentStreamEvent>>>>;

    /// Фабрика транспорта к ACP-агенту: `(program, args, cwd) → Transport`. В проде — спавн подпроцесса
    /// ([`StdioTransport`]); тест-шов подменяет на in-process [`ChannelTransport`] со счётчиком спавнов,
    /// чтобы доказать переиспользование (один спавн на N ходов). Возвращает boxed future (async-замыкание).
    type SpawnFut = std::pin::Pin<
        Box<dyn std::future::Future<Output = std::io::Result<Arc<dyn Transport>>> + Send>,
    >;
    type TransportFactory = Arc<dyn Fn(String, Vec<String>, PathBuf) -> SpawnFut + Send + Sync>;

    /// Таймаут управляющих RPC (`initialize`/`session/new`).
    const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);
    /// Верхняя граница на ОДИН ход `session/prompt`. Cold-start+инференс легитимно длятся 1-3 мин →
    /// порог щедрый (10 мин). Нужен из-за reuse: КРАШ агента провалит запрос через EOF-дренаж
    /// (client.rs: «acp transport closed»), но ЗАВИСШИЙ-но-живой агент (без EOF) при `None` висел бы
    /// вечно → ход не терминируется → `current_channel` занят → ВСЕ следующие ходы «уже идёт» (залип
    /// бэкенда до рестарта). Таймаут гарантирует терминал → освобождение слота → респавн на след. ходе.
    const PROMPT_TIMEOUT: Duration = Duration::from_secs(600);

    /// Висящий `request_permission`: `rpc_id` запроса агента + опции (для маппинга approve→outcome).
    struct PendingPerm {
        rpc_id: serde_json::Value,
        options: Vec<(String, schema::PermissionOptionKind)>,
    }
    type PendingPerms = Arc<Mutex<HashMap<i64, PendingPerm>>>;

    /// Приёмники потоков соединения (`session/update` + входящие permission). ПЕРЕЖИВАЮТ ходы: живут в
    /// [`AcpState`], а активный ход на время прогона лочит их (R2 — один активный ход → нет контеншена).
    type SharedUpdates = Arc<Mutex<tokio::sync::mpsc::Receiver<schema::SessionNotification>>>;
    type SharedPerms = Arc<Mutex<tokio::sync::mpsc::Receiver<InboundPermission>>>;

    /// Живое ACP-соединение. ПЕРЕИСПОЛЬЗУЕТСЯ между ходами (perf: спавн+initialize+session/new ОДИН раз;
    /// каждый следующий ход — только `session/prompt` по ЭТОМУ же соединению и сессии). `updates`/`perms`
    /// держатся здесь (а не отдаются drive-таску по значению), чтобы пережить ход. Дроп AcpState → дроп
    /// client → агент убит (kill_on_drop) — так заменяется мёртвое соединение при переспавне.
    struct AcpState {
        client: Arc<AcpClient>,
        session_id: String,
        current_channel: SharedChannel,
        pending_perms: PendingPerms,
        updates: SharedUpdates,
        perms: SharedPerms,
    }

    /// Бэкенд, драйвящий внешний ACP-агент. Lazy-spawn на первом `run`.
    pub struct AcpBackend {
        command: Option<Vec<String>>,
        cwd: PathBuf,
        inner: Mutex<Option<AcpState>>,
        next_run: AtomicI64,
        next_action: Arc<AtomicI64>,
        // Фабрика транспорта (прод: спавн подпроцесса; тест: ChannelTransport + счётчик спавнов).
        spawn: TransportFactory,
    }

    /// Прод-фабрика: спавнит реальный подпроцесс ([`StdioTransport`], kill_on_drop).
    fn default_spawn() -> TransportFactory {
        Arc::new(
            |program: String, args: Vec<String>, cwd: PathBuf| -> SpawnFut {
                Box::pin(async move {
                    let t = StdioTransport::spawn(&program, &args, &cwd).await?;
                    Ok(Arc::new(t) as Arc<dyn Transport>)
                })
            },
        )
    }

    impl AcpBackend {
        pub fn new(command: Option<Vec<String>>, cwd: PathBuf) -> Self {
            Self {
                command,
                cwd,
                inner: Mutex::new(None),
                next_run: AtomicI64::new(1),
                next_action: Arc::new(AtomicI64::new(1)),
                spawn: default_spawn(),
            }
        }

        /// Тест-конструктор: подменяет фабрику транспорта (in-process [`ChannelTransport`] + счётчик
        /// спавнов) — чтобы доказать переиспользование соединения (один спавн на N ходов).
        #[cfg(test)]
        fn with_transport_factory(
            command: Option<Vec<String>>,
            cwd: PathBuf,
            spawn: TransportFactory,
        ) -> Self {
            Self {
                command,
                cwd,
                inner: Mutex::new(None),
                next_run: AtomicI64::new(1),
                next_action: Arc::new(AtomicI64::new(1)),
                spawn,
            }
        }
    }

    /// Отправить событие в текущий канал прогона (no-op, если канал уже освобождён).
    async fn send_ev(current: &SharedChannel, ev: AgentStreamEvent) {
        if let Some(ch) = current.lock().await.as_ref() {
            let _ = ch.send(ev);
        }
    }

    /// ACP-1b: извлекает ВСЕ файлы permission-запроса (по одному на `Diff`-content-блок): путь + грубый
    /// счёт строк + статус + род (ACP-EXEC). Раньше (ACP-1) показывался только ПЕРВЫЙ diff →
    /// мульти-файловый permission под-репортил scope юзеру. Нет ни одного diff (exec/fetch-permission) →
    /// деградируем к одной строке-КОМАНДЕ (`AgentProposedKind::Exec`): фронт рисует её как `$ cmd`
    /// exec-стилем (без ±строк/диффа), а не как фейковый файл. Diff-блоки → `File`. (Счёт строк грубый —
    /// full-replace; точный line-diff — refinement.)
    fn extract_files(
        tc: &schema::ToolCallUpdate,
    ) -> Vec<(String, u32, u32, AgentFileStatus, AgentProposedKind)> {
        let diffs: Vec<_> = tc
            .content
            .as_ref()
            .map(|c| {
                c.iter()
                    .filter_map(|x| match x {
                        schema::ToolCallContent::Diff(d) => Some(d),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        if diffs.is_empty() {
            // нет diff (exec/fetch-permission) → деградируем к строке-КОМАНДЕ (kind=Exec). Чистим
            // очевидный префикс заголовка («terminal: ») для опрятного `$ cmd`; статус Edit для exec
            // не используется фронтом (exec-строка рисуется без ±/диффа). action_id всё равно есть.
            let raw = tc.title.clone().unwrap_or_else(|| "действие агента".into());
            let cmd = raw.strip_prefix("terminal: ").unwrap_or(&raw).to_string();
            return vec![(cmd, 0, 0, AgentFileStatus::Edit, AgentProposedKind::Exec)];
        }
        diffs
            .into_iter()
            .map(|d| {
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
                (
                    d.path.to_string_lossy().into_owned(),
                    add,
                    del,
                    status,
                    AgentProposedKind::File,
                )
            })
            .collect()
    }

    /// Входящий permission → синтетический `action_id` + регистрация в `pending_perms` + `Proposal` в UI.
    async fn handle_permission(
        current: &SharedChannel,
        pending_perms: &PendingPerms,
        next_action: &AtomicI64,
        run_id: i64,
        inbound: InboundPermission,
    ) {
        // ACP-1b: одна ACP-permission = ОДНО атомарное решение (один Response). Поэтому ВСЕ файлы делят
        // ОДИН синтетический action_id (одобрить любой = одобрить весь permission); моделировать per-file
        // action_id было бы ложью (`agent_approve` шлёт один outcome на весь запрос). `approve` снимает
        // pending_perms по этому единственному id (дубль-решения от стора дедуплицируются — см. store).
        let action_id = next_action.fetch_add(1, Ordering::Relaxed);
        let files: Vec<AgentProposedFile> = extract_files(&inbound.params.tool_call)
            .into_iter()
            .map(|(path, add, del, status, kind)| AgentProposedFile {
                path,
                add,
                del,
                status,
                kind,
                action_id,
            })
            .collect();
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
        send_ev(current, AgentStreamEvent::Proposal { run_id, files }).await;
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
    /// `run_id` нужен для `Plan`-события (ACP-1b).
    fn map_update(run_id: i64, update: schema::SessionUpdate) -> Vec<AgentStreamEvent> {
        use schema::{
            AcpPlanStatus, ContentBlock, SessionUpdate as U, ToolCallContent, ToolCallStatus,
        };
        match update {
            U::AgentMessageChunk { .. } | U::AgentThoughtChunk { .. } => Vec::new(), // обрабатываются в цикле (accum)
            U::ToolCall(tc) => vec![AgentStreamEvent::ToolCall {
                id: tc.tool_call_id,
                kind: acp_kind_to_display(tc.kind).to_string(),
                args: tc.raw_input.map(|v| v.to_string()).unwrap_or_default(),
                // Hermes/ACP даёт человеко-подпись (напр. «Fetching docs.rs»); пустую → None (фронт
                // достроит из kind+args).
                title: Some(tc.title).filter(|s| !s.is_empty()),
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
            // ACP-1b: план (полный список каждым апдейтом) → PlanProposed. id синтезируем по индексу
            // (позиционно стабилен в ходе). ACP не шлёт инкрементальный статус → только PlanProposed.
            U::Plan { entries } => vec![AgentStreamEvent::PlanProposed {
                run_id,
                steps: entries
                    .into_iter()
                    .enumerate()
                    .map(|(i, e)| AgentPlanStep {
                        id: format!("p{i}"),
                        label: e.content,
                        status: match e.status {
                            AcpPlanStatus::InProgress => AgentPlanStepState::Running,
                            AcpPlanStatus::Completed => AgentPlanStepState::Done,
                            AcpPlanStatus::Pending | AcpPlanStatus::Other => {
                                AgentPlanStepState::Pending
                            }
                        },
                    })
                    .collect(),
            }],
            U::Other => Vec::new(),
        }
    }

    /// Достаёт текст ОТВЕТА из чанка (`agent_message_chunk`) для стрима/accum в Final.
    /// `agent_thought_chunk` (reasoning) НАМЕРЕННО НЕ извлекаем: внешние агенты (Hermes) льют длинные
    /// рассуждения, и при склейке они мешались с ответом. Решение владельца — скрыть размышления
    /// (виден индикатор хода, пока агент думает); в ответ идёт только сам ответ.
    fn chunk_text(update: &schema::SessionUpdate) -> Option<&str> {
        use schema::{ContentBlock, SessionUpdate as U};
        match update {
            U::AgentMessageChunk {
                content: ContentBlock::Text { text },
            } => Some(text.as_str()),
            _ => None,
        }
    }

    /// Drive-таск ОДНОГО хода: гонит `session/prompt` (с `PROMPT_TIMEOUT`) + параллельно пампит updates/perms в
    /// канал, до терминала хода. На терминале — финальное событие + разрешение висящих permission в
    /// Cancelled + освобождение R2-слота (канал → None). Соединение/приёмники НЕ дропаются — живут в
    /// AcpState для следующего хода.
    ///
    /// `updates_arc`/`perms_arc` лочатся на ВЕСЬ ход: активный ход ВЛАДЕЕТ приёмниками (соединение
    /// переживает ход, приёмники переживают ход). Держать гард `tokio::sync::Mutex` через `.await` —
    /// НАМЕРЕННО и безопасно: clippy `await_holding_lock` срабатывает только на `std::sync::Mutex`; R2
    /// гарантирует ОДИН активный ход на соединение → контеншена за эти мьютексы нет.
    #[allow(clippy::too_many_arguments)]
    async fn drive_run(
        client: Arc<AcpClient>,
        updates_arc: SharedUpdates,
        perms_arc: SharedPerms,
        current: SharedChannel,
        pending_perms: PendingPerms,
        next_action: Arc<AtomicI64>,
        run_id: i64,
        session_id: String,
        task: String,
    ) {
        // Лочим приёмники на время хода (R2: один активный ход → без контеншена; см. док-коммент выше).
        let mut updates = updates_arc.lock().await;
        let mut perms = perms_arc.lock().await;
        let prompt = client.request(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": [{"type":"text","text": task}]}),
            Some(PROMPT_TIMEOUT), // верхняя граница хода: зависший-но-живой агент не залипит слот навсегда
        );
        tokio::pin!(prompt);
        let mut answer = String::new();

        // Терминал НЕ строим прямо в select!: сначала фиксируем «как закончился ход», потом дренируем
        // запоздавшие токены (фолдим их в `answer`), и ТОЛЬКО затем собираем Final с ПОЛНЫМ текстом.
        // Иначе гонка select! (Response пришёл раньше, чем буфер updates обработан → токен дренируется
        // ПОСЛЕ `mem::take(answer)`) терялась бы из Final — особенно на быстрых ходах переиспользования.
        enum End {
            Final,
            Error(String),
        }
        let end: End = loop {
            tokio::select! {
                res = &mut prompt => {
                    break match res {
                        Ok(v) => {
                            let stop = v.get("stopReason").and_then(|s| s.as_str()).unwrap_or("end_turn");
                            match stop {
                                "refusal" => End::Error("ACP-агент отклонил запрос (refusal)".into()),
                                "cancelled" => End::Error("прогон отменён".into()),
                                _ => End::Final,
                            }
                        }
                        Err(e) => End::Error(format!("ACP session/prompt: {}", e.message)),
                    };
                }
                n = updates.recv() => match n {
                    Some(notif) => {
                        if let Some(t) = chunk_text(&notif.update) {
                            answer.push_str(t);
                            send_ev(&current, AgentStreamEvent::AssistantToken { text: t.to_string() }).await;
                        } else {
                            for ev in map_update(run_id, notif.update) { send_ev(&current, ev).await; }
                        }
                    }
                    None => break End::Error("ACP-агент отключился".into()),
                },
                p = perms.recv() => {
                    if let Some(inbound) = p {
                        handle_permission(&current, &pending_perms, &next_action, run_id, inbound).await;
                    }
                }
            }
        };

        // Best-effort дренаж буферизованных апдейтов перед терминалом (фолдим токены в `answer`).
        while let Ok(notif) = updates.try_recv() {
            if let Some(t) = chunk_text(&notif.update) {
                answer.push_str(t);
                send_ev(
                    &current,
                    AgentStreamEvent::AssistantToken {
                        text: t.to_string(),
                    },
                )
                .await;
            } else {
                for ev in map_update(run_id, notif.update) {
                    send_ev(&current, ev).await;
                }
            }
        }
        let terminal = match end {
            End::Final => AgentStreamEvent::Final { text: answer },
            End::Error(message) => AgentStreamEvent::Error { message },
        };
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
        // Освобождаем ТОЛЬКО R2-слот хода (канал → None). Соединение/приёмники остаются в AcpState для
        // следующего хода (переиспользование). Если ход кончился из-за ухода агента ("ACP-агент
        // отключился"), соединение мёртво — это поймает `is_alive()` на следующем run() → переспавн.
        *current.lock().await = None;
    }

    #[async_trait]
    impl AgentBackend for AcpBackend {
        async fn run(
            &self,
            _state: &AppState,
            task: String,
            _autonomy: String, // R1: autonomy не идёт по проводу (агент берёт из своего конфига)
            _history: Vec<HistoryMsg>, // R1: history не идёт по проводу (сессии stateful у агента)
            _session_id: String, // W-38: история — embedded-фича; ACP-агент свою группировку не персистит
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

            // R2: ОДИН активный ход на соединение. Прошлый ход ещё не дошёл до Final/Error (канал занят) →
            // отклоняем (события двух ходов смешались бы в одном канале; UI и так гонит по одному).
            if let Some(st) = guard.as_ref() {
                if st.current_channel.lock().await.is_some() {
                    return Err(AppError::Msg(
                        "ACP-прогон уже идёт (один активный прогон на соединение)".into(),
                    ));
                }
            }

            // ПЕРЕИСПОЛЬЗОВАНИЕ: соединение есть и живо → НЕ спавним подпроцесс, НЕ переинициализируем.
            // Новый ход = только новый `session/prompt` по той же сессии (первый ход прогрел cold-start,
            // остальные мгновенны). Приёмники updates/perms живут в AcpState и лочатся drive-таском на ход.
            if let Some(st) = guard.as_ref() {
                if st.client.is_alive() {
                    let run_id = self.next_run.fetch_add(1, Ordering::Relaxed);
                    *st.current_channel.lock().await = Some(channel);
                    tokio::spawn(drive_run(
                        st.client.clone(),
                        st.updates.clone(),
                        st.perms.clone(),
                        st.current_channel.clone(),
                        st.pending_perms.clone(),
                        self.next_action.clone(),
                        run_id,
                        st.session_id.clone(),
                        task,
                    ));
                    return Ok(run_id);
                }
            }

            // СВЕЖИЙ СПАВН: соединения нет ИЛИ оно мёртвое (`!is_alive()` — агент отвалился). Спавним
            // подпроцесс + initialize + session/new (cold-start ~9.5с — как warm-up нативного агента).
            // Замена мёртвого: старый AcpState дропнется при `*guard = Some(...)` → старый client дропнется
            // → старый подпроцесс убьётся kill_on_drop.
            let (program, args) = command
                .split_first()
                .expect("command непустой (проверено выше)");
            let transport = (self.spawn)(program.clone(), args.to_vec(), self.cwd.clone())
                .await
                .map_err(|e| AppError::Msg(format!("спавн ACP-агента `{program}`: {e}")))?;
            let (client, updates_rx, perms_rx) = AcpClient::new(transport);
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
            let updates: SharedUpdates = Arc::new(Mutex::new(updates_rx));
            let perms: SharedPerms = Arc::new(Mutex::new(perms_rx));
            tokio::spawn(drive_run(
                client.clone(),
                updates.clone(),
                perms.clone(),
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
                updates,
                perms,
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
        fn extract_files_new_file_from_diff() {
            let files = extract_files(&tc_update_with_diff(None, "a\nb\nc"));
            assert_eq!(files.len(), 1);
            let (path, add, del, status, kind) = &files[0];
            assert_eq!(path, "Notes/A.md");
            assert_eq!((*add, *del), (3, 0));
            assert_eq!(*status, AgentFileStatus::New);
            // ACP-EXEC: diff-блок → File (рисуется как путь + ±строки + дифф).
            assert_eq!(*kind, AgentProposedKind::File);
        }

        #[test]
        fn extract_files_edit_from_diff() {
            let files = extract_files(&tc_update_with_diff(Some("a\nb"), "a\nb\nc\nd"));
            let (_, add, del, status, kind) = &files[0];
            assert_eq!((*add, *del), (4, 2));
            assert_eq!(*status, AgentFileStatus::Edit);
            assert_eq!(*kind, AgentProposedKind::File);
        }

        #[test]
        fn extract_files_degraded_without_diff_is_exec() {
            // ACP-EXEC: нет diff (exec-permission) → строка-КОМАНДА kind=Exec, 0/0 строк.
            let tc = ToolCallUpdate {
                tool_call_id: "t1".into(),
                status: None,
                content: None,
                title: Some("run `ls`".into()),
                kind: None,
            };
            let files = extract_files(&tc);
            assert_eq!(files.len(), 1);
            let (path, add, del, status, kind) = &files[0];
            assert_eq!(path, "run `ls`");
            assert_eq!((*add, *del), (0, 0));
            assert_eq!(*status, AgentFileStatus::Edit); // не используется фронтом для exec
            assert_eq!(*kind, AgentProposedKind::Exec);
        }

        #[test]
        fn extract_files_exec_strips_terminal_prefix() {
            // ACP-EXEC: очевидный префикс «terminal: » срезается для опрятного `$ cmd`.
            let tc = ToolCallUpdate {
                tool_call_id: "t1".into(),
                status: None,
                content: None,
                title: Some("terminal: cargo build --release".into()),
                kind: None,
            };
            let files = extract_files(&tc);
            let (path, _, _, _, kind) = &files[0];
            assert_eq!(path, "cargo build --release");
            assert_eq!(*kind, AgentProposedKind::Exec);
        }

        #[test]
        fn extract_files_exec_fallback_title_when_missing() {
            // ACP-EXEC: нет заголовка → фолбэк-строка, всё ещё Exec.
            let tc = ToolCallUpdate {
                tool_call_id: "t1".into(),
                status: None,
                content: None,
                title: None,
                kind: None,
            };
            let files = extract_files(&tc);
            let (path, _, _, _, kind) = &files[0];
            assert_eq!(path, "действие агента");
            assert_eq!(*kind, AgentProposedKind::Exec);
        }

        #[test]
        fn extract_files_returns_all_diffs() {
            // ACP-1b: мульти-файловый permission → ВСЕ Diff-блоки (не только первый).
            let tc = ToolCallUpdate {
                tool_call_id: "t1".into(),
                status: None,
                content: Some(vec![
                    ToolCallContent::Diff(Diff {
                        path: "Notes/A.md".into(),
                        old_text: None,
                        new_text: "alpha".into(),
                    }),
                    ToolCallContent::Diff(Diff {
                        path: "Notes/B.md".into(),
                        old_text: Some("x".into()),
                        new_text: "beta".into(),
                    }),
                ]),
                title: Some("multi".into()),
                kind: None,
            };
            let files = extract_files(&tc);
            assert_eq!(files.len(), 2);
            assert_eq!(files[0].0, "Notes/A.md");
            assert_eq!(files[0].3, AgentFileStatus::New);
            assert_eq!(files[1].0, "Notes/B.md");
            assert_eq!(files[1].3, AgentFileStatus::Edit);
        }

        #[test]
        fn map_update_plan_maps_to_plan_proposed() {
            // ACP-1b: plan → PlanProposed (id по индексу, статусы).
            let upd = SessionUpdate::Plan {
                entries: vec![
                    schema::PlanEntry {
                        content: "step one".into(),
                        priority: schema::AcpPlanPriority::High,
                        status: schema::AcpPlanStatus::InProgress,
                    },
                    schema::PlanEntry {
                        content: "step two".into(),
                        priority: schema::AcpPlanPriority::Medium,
                        status: schema::AcpPlanStatus::Pending,
                    },
                ],
            };
            let evs = map_update(7, upd);
            match evs.first() {
                Some(AgentStreamEvent::PlanProposed { run_id, steps }) => {
                    assert_eq!(*run_id, 7);
                    assert_eq!(steps.len(), 2);
                    assert_eq!(steps[0].id, "p0");
                    assert_eq!(steps[0].label, "step one");
                    assert_eq!(steps[0].status, AgentPlanStepState::Running);
                    assert_eq!(steps[1].id, "p1");
                    assert_eq!(steps[1].status, AgentPlanStepState::Pending);
                }
                other => panic!("ожидался PlanProposed, получено {other:?}"),
            }
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
            let evs = map_update(1, SessionUpdate::ToolCall(tc));
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
            let evs = map_update(1, SessionUpdate::ToolCallUpdate(done));
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
            assert!(map_update(1, SessionUpdate::ToolCallUpdate(pending)).is_empty());
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
                map_update(1, SessionUpdate::ToolCallUpdate(failed)).first(),
                Some(AgentStreamEvent::ToolResult { is_error: true, .. })
            ));
        }

        #[test]
        fn chunk_text_extracts_message_skips_thought() {
            let msg = SessionUpdate::AgentMessageChunk {
                content: ContentBlock::Text { text: "hi".into() },
            };
            assert_eq!(chunk_text(&msg), Some("hi"));
            // reasoning НЕ извлекаем (скрыт): thought-чанк → None, не попадает в ответ.
            let thought = SessionUpdate::AgentThoughtChunk {
                content: ContentBlock::Text {
                    text: "thinking".into(),
                },
            };
            assert_eq!(chunk_text(&thought), None);
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

        // ── Переиспользование соединения: ДВА хода по ОДНОМУ соединению, спавн РОВНО ОДИН раз ──────────

        use nexus_core::agent::connect::{channel_pair, ChannelTransport, RpcMessage};
        use std::sync::atomic::AtomicUsize;
        use tauri::ipc::Channel;

        type EventBuf = Arc<std::sync::Mutex<Vec<serde_json::Value>>>;

        /// Channel, складывающий каждое отправленное событие (parsed JSON) в `buf` (тот же путь, что Tauri).
        fn channel_into(buf: EventBuf) -> Channel<AgentStreamEvent> {
            Channel::new(move |body: tauri::ipc::InvokeResponseBody| {
                if let tauri::ipc::InvokeResponseBody::Json(s) = body {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                        buf.lock().unwrap().push(v);
                    }
                }
                Ok(())
            })
        }

        /// Ждёт (с дедлайном), пока в буфере появится событие нужного `type`-тега; возвращает его текст
        /// (поле `text`, если есть). Паникует по таймауту — чтобы тест не висел вечно.
        async fn wait_for_event(buf: &EventBuf, ty: &str) -> serde_json::Value {
            for _ in 0..200 {
                if let Some(ev) = buf
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|v| v.get("type").and_then(|t| t.as_str()) == Some(ty))
                {
                    return ev.clone();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!(
                "событие type={ty} не пришло за дедлайн; буфер: {:?}",
                buf.lock().unwrap()
            );
        }

        /// In-process мок-ACP-агент на серверном конце ChannelTransport: initialize → session/new →
        /// затем БЕСКОНЕЧНО обслуживает `session/prompt` (по одному токену + end_turn на ход), пока
        /// клиент не закроет транспорт. Так ОДНО соединение несёт несколько ходов (как реальный агент).
        async fn mock_multi_turn_agent(srv: ChannelTransport) {
            // initialize
            let RpcMessage::Request { id, method, .. } = srv.recv().await.unwrap() else {
                panic!("ждали initialize-Request")
            };
            assert_eq!(method, "initialize");
            srv.send(RpcMessage::Response {
                id,
                result: Ok(json!({"protocolVersion": ACP_PROTOCOL_VERSION})),
            })
            .await
            .unwrap();
            // session/new (ОДИН раз — доказывает, что второй ход НЕ переинициализирует сессию)
            let RpcMessage::Request { id, method, .. } = srv.recv().await.unwrap() else {
                panic!("ждали session/new-Request")
            };
            assert_eq!(method, "session/new");
            srv.send(RpcMessage::Response {
                id,
                result: Ok(json!({"sessionId": "s1"})),
            })
            .await
            .unwrap();
            // ходы: каждый session/prompt → токен + end_turn; до закрытия транспорта (клиент дропнул state)
            while let Some(msg) = srv.recv().await {
                let RpcMessage::Request { id, method, params } = msg else {
                    continue; // ответы клиента (на наши request_permission тут не шлём) — игнор
                };
                assert_eq!(
                    method, "session/prompt",
                    "после рукопожатия — только prompt'ы"
                );
                // эхо текста запроса как один токен ответа (доказывает стрим)
                let task = params
                    .pointer("/prompt/0/text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                srv.send(RpcMessage::notification(
                    "session/update",
                    json!({"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk",
                           "content":{"type":"text","text": format!("echo:{task}")}}}),
                ))
                .await
                .unwrap();
                srv.send(RpcMessage::Response {
                    id,
                    result: Ok(json!({"stopReason": "end_turn"})),
                })
                .await
                .unwrap();
            }
        }

        #[tokio::test]
        async fn acp_reuses_connection_across_two_turns_spawns_once() {
            // Серверный конец отдаём моку; клиентский — отдаём фабрике транспорта (ОДИН раз).
            let (client_t, server_t) = channel_pair();
            tokio::spawn(mock_multi_turn_agent(server_t));

            let spawns = Arc::new(AtomicUsize::new(0));
            // Клиентский транспорт прячем в Option — фабрика берёт его на ПЕРВОМ (и единственном) спавне;
            // повторный спавн (если бы переиспользование сломалось) запаниковал бы на take().unwrap().
            let slot = Arc::new(std::sync::Mutex::new(Some(
                Arc::new(client_t) as Arc<dyn Transport>
            )));
            let spawns_f = spawns.clone();
            let factory: TransportFactory = Arc::new(move |_program, _args, _cwd| -> SpawnFut {
                let spawns_f = spawns_f.clone();
                let slot = slot.clone();
                Box::pin(async move {
                    spawns_f.fetch_add(1, Ordering::Relaxed);
                    let t = slot
                        .lock()
                        .unwrap()
                        .take()
                        .expect("спавн вызван второй раз — переиспользование сломано");
                    Ok(t)
                })
            });

            let backend = AcpBackend::with_transport_factory(
                Some(vec!["mock".into()]),
                std::env::temp_dir(),
                factory,
            );
            let state = crate::state::AppState::new();

            // ── Ход 1: спавн + initialize + session/new + prompt → Final ─────────────────────────────
            let buf1: EventBuf = Arc::new(std::sync::Mutex::new(Vec::new()));
            let run1 = backend
                .run(
                    &state,
                    "первый".into(),
                    "ask".into(),
                    vec![],
                    String::new(), // W-38: session_id (ACP игнорирует историю-группировку)
                    channel_into(buf1.clone()),
                )
                .await
                .expect("run #1");
            let final1 = wait_for_event(&buf1, "final").await;
            assert_eq!(
                final1.get("text").and_then(|t| t.as_str()),
                Some("echo:первый"),
                "ход 1 должен стримить ответ и завершиться Final"
            );

            // ── Ход 2: ТОТ ЖЕ процесс/сессия — только новый prompt → Final (без переспавна) ──────────
            // Final хода 1 отправляется ДО очистки R2-слота (канал → None) и до релиза локов приёмников
            // drive-таском; крошечное окно между «увидели Final» и «слот свободен» закрываем ретраем по
            // тому же буферу (реальный UI тоже не шлёт второй ход раньше, чем дорисует ответ первого).
            let buf2: EventBuf = Arc::new(std::sync::Mutex::new(Vec::new()));
            let run2 = loop {
                match backend
                    .run(
                        &state,
                        "второй".into(),
                        "ask".into(),
                        vec![],
                        String::new(), // W-38: session_id (ACP игнорирует историю-группировку)
                        channel_into(buf2.clone()),
                    )
                    .await
                {
                    Ok(id) => break id,
                    Err(e) => {
                        // единственная ожидаемая транзиентная ошибка — слот хода 1 ещё не освобождён
                        assert!(
                            e.to_string().contains("уже идёт"),
                            "неожиданная ошибка run #2: {e}"
                        );
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }
            };
            let final2 = wait_for_event(&buf2, "final").await;
            assert_eq!(
                final2.get("text").and_then(|t| t.as_str()),
                Some("echo:второй"),
                "ход 2 должен стримить ответ и завершиться Final по тому же соединению"
            );

            assert_ne!(run1, run2, "у каждого хода свой run_id");
            assert_eq!(
                spawns.load(Ordering::Relaxed),
                1,
                "ПЕРЕИСПОЛЬЗОВАНИЕ: транспорт/агент спавнится РОВНО ОДИН раз на ДВА хода \
                 (никакого респавна + initialize/session/new на втором ходе)"
            );
        }
    }
}

pub use acp_backend::AcpBackend;
