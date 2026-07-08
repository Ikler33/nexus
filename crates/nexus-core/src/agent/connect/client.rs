//! CONN-2 — клиентская сторона AGENT-CONNECT: [`ConnectClient`] драйвит внешний `ConnectHandler`
//! (agentd) поверх любого [`Transport`] (in-process / AF_UNIX). Зеркало серверного [`super::dispatch`]:
//! отправляет `initialize`/`agent/run`/`agent/approve`/`agent/control`/`agent/cancel`/`agent/undo`,
//! коррелирует ответы по `id` (out-of-order), а `agent/event`-нотификации стримит в events-канал.
//!
//! Контракт «один consumer» транспорта (см. [`super::Transport`]) держим ОДНИМ read-loop'ом: только он
//! зовёт `recv`. `send` идёт отдельным каналом транспорта — пересечений нет. **Fail-safe:** запрос с
//! таймаутом (мёртвый сервер не вешает UI); закрытие транспорта → все ждущие получают ошибку, events-канал
//! закрывается (потребитель видит конец стрима).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::rpc::RpcCorrelator;

use super::{
    InitializeResult, RpcError, RpcMessage, Transport, TransportError, EVENT_CHANNEL_CAP,
    SUPPORTED_VERSIONS,
};

/// Таймаут ОТВЕТА на запрос. NB: ack `agent/run` сервер шлёт СРАЗУ (до стрима событий), а cold-start
/// модели сидит в стриме `agent/event`, не в ack — поэтому скромный таймаут управляющих RPC безопасен.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Ошибка установки соединения (handshake).
#[derive(Debug)]
pub enum ConnectError {
    /// RPC `initialize` не прошёл (транспорт/таймаут/ошибка сервера).
    Rpc(RpcError),
    /// Ответ `initialize` не распарсился в [`InitializeResult`].
    BadHandshake,
    /// Сервер выбрал версию, которую клиент не поддерживает.
    VersionIncompatible(String),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::Rpc(e) => write!(f, "initialize: {}", e.message),
            ConnectError::BadHandshake => write!(f, "некорректный ответ initialize"),
            ConnectError::VersionIncompatible(v) => {
                write!(f, "несовместимая версия протокола сервера: {v}")
            }
        }
    }
}
impl std::error::Error for ConnectError {}

/// Значение, доставляемое ждущему запросу: разобранный `Response.result` (Ok/Err протокола).
type Reply = Result<Value, RpcError>;

/// Клиент протокола AGENT-CONNECT поверх [`Transport`]. Один read-loop демультиплексирует ответы
/// (по `id` через [`RpcCorrelator`]) и `agent/event`-нотификации (в events-канал). Дропается → read-loop
/// прерывается.
pub struct ConnectClient {
    transport: Arc<dyn Transport>,
    correlator: Arc<RpcCorrelator<Reply>>,
    read_task: tokio::task::JoinHandle<()>,
}

impl Drop for ConnectClient {
    fn drop(&mut self) {
        // Прерываем read-loop → его держатель `events_tx` дропается → потребитель events видит конец.
        self.read_task.abort();
    }
}

impl ConnectClient {
    /// Подключается: поднимает read-loop, делает `initialize`-handshake (version-negotiate). Возвращает
    /// клиент + приёмник `agent/event`-параметров (сериализованные [`super::AgentStreamEvent`]). Ошибка
    /// handshake → транспорт закрывается (read-loop прервётся при дропе клиента).
    pub async fn connect(
        transport: Arc<dyn Transport>,
    ) -> Result<(Self, mpsc::Receiver<Value>), ConnectError> {
        // Клиентское направление: id стартует с 1 (см. [`RpcCorrelator::new`]).
        let correlator: Arc<RpcCorrelator<Reply>> = Arc::new(RpcCorrelator::new(1));
        let (events_tx, events_rx) = mpsc::channel::<Value>(EVENT_CHANNEL_CAP);
        let read_task = tokio::spawn(read_loop(transport.clone(), correlator.clone(), events_tx));
        let client = Self {
            transport,
            correlator,
            read_task,
        };
        let res = client
            .request(
                "initialize",
                json!({ "supportedVersions": SUPPORTED_VERSIONS }),
            )
            .await
            .map_err(ConnectError::Rpc)?;
        let init: InitializeResult =
            serde_json::from_value(res).map_err(|_| ConnectError::BadHandshake)?;
        if !SUPPORTED_VERSIONS.contains(&init.version.as_str()) {
            return Err(ConnectError::VersionIncompatible(init.version));
        }
        Ok((client, events_rx))
    }

    /// Шлёт запрос и ждёт ответ (коррелируется по `id`). Таймаут/закрытый транспорт → `Err`
    /// (не виснет). Снятие ожидания из карты при send-fail/таймауте/закрытии — внутри [`RpcCorrelator`].
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let (id, rx) = self.correlator.begin().await;
        if self
            .transport
            .send(RpcMessage::request(id, method, params))
            .await
            .is_err()
        {
            self.correlator.cancel(id).await;
            return Err(RpcError::internal("transport send failed"));
        }
        // Управляющие RPC клиента: фиксированный таймаут REQUEST_TIMEOUT (параметром — см. инвариант R-9).
        self.correlator
            .await_reply(
                id,
                rx,
                Some(REQUEST_TIMEOUT),
                || Err(RpcError::internal("response channel closed")),
                || Err(RpcError::internal("request timeout")),
            )
            .await
    }

    /// Шлёт уведомление (без ответа): `agent/approve`, `agent/control`.
    pub async fn notify(&self, method: &str, params: Value) -> Result<(), TransportError> {
        self.transport
            .send(RpcMessage::notification(method, params))
            .await
    }
}

/// Единственный read-loop: ответы → ждущим по `id`; `agent/event` → events-канал (bounded, best-effort
/// drop при переполнении — зеркало серверного forwarder'а). Закрытие транспорта → провал всех ждущих.
async fn read_loop(
    transport: Arc<dyn Transport>,
    correlator: Arc<RpcCorrelator<Reply>>,
    events_tx: mpsc::Sender<Value>,
) {
    while let Some(msg) = transport.recv().await {
        match msg {
            RpcMessage::Response { id, result } => {
                if let Some(i) = id.as_i64() {
                    correlator.resolve(i, result).await; // роутинг по id
                }
            }
            RpcMessage::Notification { method, params } if method == "agent/event" => {
                let _ = events_tx.try_send(params); // best-effort: дроп при переполнении
            }
            _ => { /* сервер не шлёт клиенту запросов (P0a) / прочие нотификации — игнор */
            }
        }
    }
    // Транспорт закрыт (сервер ушёл) → провалить все висящие запросы, чтобы `request` не вис.
    correlator
        .fail_all(Err(RpcError::internal("transport closed")))
        .await;
    // `events_tx` дропается здесь → приёмник events видит `None` (конец стрима).
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::connect::handler::{ConnectAgentHandler, ConnectDeps};
    use crate::agent::connect::{channel_pair, dispatch, ChannelTransport};
    use crate::agent::test_support::{open_db, FakeProvider};
    use crate::agent::tool::ToolCall;
    use crate::ai::tools::{ToolCapableProvider, ToolTurn};
    use crate::db::Database;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    fn deps_with(
        provider: Arc<dyn ToolCapableProvider>,
        canon_root: std::path::PathBuf,
        db: &Database,
        actuator_enabled: bool,
    ) -> Arc<ConnectDeps> {
        Arc::new(ConnectDeps {
            provider,
            memory: None,
            writer: db.writer().clone(),
            reader: db.reader().clone(),
            canon_root,
            actuator_enabled,
            autonomy: "confirm".to_string(),
            overwrite_threshold: 64 * 1024,
            blast_cap: 16,
            context_window: Some(32768),
            skills: None,
            web: None,
            skills_learning_enabled: false,
            delegation: crate::ai::DelegationConfig::default(),
            research: crate::ai::ResearchConfig::default(),
            agent_paused: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Поднимает serve-loop над server-эндпоинтом (как handler.rs::serve).
    fn serve(handler: Arc<ConnectAgentHandler>, server: Arc<ChannelTransport>) {
        tokio::spawn(async move {
            while let Some(msg) = server.recv().await {
                dispatch(handler.as_ref(), msg, server.as_ref()).await;
            }
        });
    }

    /// (1) initialize: handshake через ConnectClient негоциирует v1.0.
    #[tokio::test]
    async fn client_initialize_negotiates_v1() {
        let (client_t, server) = channel_pair();
        let server = Arc::new(server);
        let (_dir, db) = open_db().await;
        let provider: Arc<dyn ToolCapableProvider> =
            Arc::new(FakeProvider::new(vec![Ok(ToolTurn::Final("ok".into()))]));
        let handler = Arc::new(ConnectAgentHandler::new(
            deps_with(provider, _dir.path().to_path_buf(), &db, false),
            server.clone(),
        ));
        serve(handler, server.clone());

        let (_client, _events) = ConnectClient::connect(Arc::new(client_t))
            .await
            .expect("handshake ok");
    }

    /// (2) end-to-end: run через ConnectClient → ack runId + поток событий (toolCall→final),
    /// проверяем НА УРОВНЕ events-канала (params десериализуются в AgentStreamEvent).
    #[tokio::test]
    async fn client_drives_run_end_to_end() {
        use crate::agent::connect::AgentStreamEvent;
        let (client_t, server) = channel_pair();
        let server = Arc::new(server);
        let (_dir, db) = open_db().await;
        let provider: Arc<dyn ToolCapableProvider> = Arc::new(FakeProvider::new(vec![
            Ok(ToolTurn::ToolCalls(vec![ToolCall {
                id: "c1".into(),
                name: "echo".into(),
                arguments: r#"{"text":"hi"}"#.into(),
            }])),
            Ok(ToolTurn::Final("готово".into())),
        ]));
        let handler = Arc::new(ConnectAgentHandler::new(
            deps_with(provider, _dir.path().to_path_buf(), &db, false),
            server.clone(),
        ));
        serve(handler, server.clone());

        let (client, mut events) = ConnectClient::connect(Arc::new(client_t)).await.unwrap();
        let ack = client
            .request(
                "agent/run",
                json!({"sessionId": "s1", "prompt": "сделай эхо"}),
            )
            .await
            .expect("ack");
        let run_id: i64 = ack["runId"].as_str().unwrap().parse().unwrap();
        assert!(run_id > 0, "ack с валидным runId");

        let mut got_toolcall = false;
        let mut got_final = false;
        for _ in 0..60 {
            let v = tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("event timeout");
            let Some(v) = v else { break };
            let ev: AgentStreamEvent = serde_json::from_value(v).expect("event → AgentStreamEvent");
            match ev {
                AgentStreamEvent::ToolCall { .. } => got_toolcall = true,
                AgentStreamEvent::Final { .. } => {
                    got_final = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(got_toolcall, "toolCall застримлен");
        assert!(got_final, "final застримлен");
    }

    /// (3) approve по проводу через ConnectClient применяет Confirm-айтем (файл записан через гейт).
    #[tokio::test]
    async fn client_approve_over_wire_applies() {
        use crate::agent::connect::AgentStreamEvent;
        let (client_t, server) = channel_pair();
        let server = Arc::new(server);
        let (dir, db) = open_db().await;
        let canon = dir.path().canonicalize().unwrap();
        let provider: Arc<dyn ToolCapableProvider> = Arc::new(FakeProvider::new(vec![
            Ok(ToolTurn::ToolCalls(vec![ToolCall {
                id: "n1".into(),
                name: "note.create".into(),
                arguments: r#"{"path":"Notes/Wire.md","content":"созданоклиентом"}"#.into(),
            }])),
            Ok(ToolTurn::Final("готово".into())),
        ]));
        let handler = Arc::new(ConnectAgentHandler::new(
            deps_with(provider, canon.clone(), &db, true),
            server.clone(),
        ));
        serve(handler, server.clone());

        let (client, mut events) = ConnectClient::connect(Arc::new(client_t)).await.unwrap();
        let ack = client
            .request(
                "agent/run",
                json!({"sessionId": "sx", "prompt": "создай заметку"}),
            )
            .await
            .unwrap();
        let run_id = ack["runId"].as_str().unwrap().to_string();

        let mut approved = false;
        let mut got_final = false;
        for _ in 0..80 {
            let v = tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("event timeout");
            let Some(v) = v else { break };
            let ev: AgentStreamEvent = serde_json::from_value(v).unwrap();
            match ev {
                AgentStreamEvent::Proposal { files, .. } if !approved => {
                    let action_id = files[0].action_id;
                    client
                        .notify(
                            "agent/approve",
                            json!({"sessionId":"sx","runId":run_id,"decisions":[{"actionId":action_id,"approved":true}]}),
                        )
                        .await
                        .unwrap();
                    approved = true;
                }
                AgentStreamEvent::Final { .. } => {
                    got_final = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(approved, "proposal пришёл и отправлен approve");
        assert!(got_final, "прогон дошёл до final");
        assert_eq!(
            std::fs::read_to_string(canon.join("Notes/Wire.md"))
                .ok()
                .as_deref(),
            Some("созданоклиентом"),
            "approve по проводу применил note.create через гейт"
        );
    }

    /// (4) закрытый транспорт → request возвращает Err (не виснет), ждущие провалены.
    #[tokio::test]
    async fn client_request_fails_on_closed_transport() {
        let (client_t, server) = channel_pair();
        // initialize нужен серверный ответ — поднимем минимальный эхо-сервер только на initialize,
        // затем закроем его, и убедимся, что последующий request падает (а не виснет).
        let server = Arc::new(server);
        let (_dir, db) = open_db().await;
        let provider: Arc<dyn ToolCapableProvider> =
            Arc::new(FakeProvider::new(vec![Ok(ToolTurn::Final("ok".into()))]));
        let handler = Arc::new(ConnectAgentHandler::new(
            deps_with(provider, _dir.path().to_path_buf(), &db, false),
            server.clone(),
        ));
        // serve один цикл, затем дропаем server (закрытие транспорта).
        let server_for_loop = server.clone();
        let h = tokio::spawn(async move {
            // обслужим только initialize, потом выйдем (дроп server → транспорт закрыт)
            if let Some(msg) = server_for_loop.recv().await {
                dispatch(handler.as_ref(), msg, server_for_loop.as_ref()).await;
            }
        });
        let (client, _events) = ConnectClient::connect(Arc::new(client_t)).await.unwrap();
        h.await.unwrap();
        drop(server); // закрываем серверный конец
                      // Теперь любой request должен вернуть Err (read-loop увидел закрытие и провалил ожидания).
        let r = client
            .request("agent/cancel", json!({"sessionId":"s","runId":"1"}))
            .await;
        assert!(
            r.is_err(),
            "request на закрытом транспорте → Err, не виснет"
        );
    }
}
