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
