//! ACP-1 — клиент ACP поверх [`Transport`]. В ОТЛИЧИЕ от half-duplex [`super::super::ConnectClient`]
//! (CONN-2: сервер не шлёт клиенту запросов), ACP **двунаправлен** — агент шлёт клиенту ЗАПРОСЫ
//! (`session/request_permission`, `fs/*`, `terminal/*`), на которые НАДО ответить. Поэтому свой read-loop
//! с рукавом `Request`. Корреляция исходящих запросов по `id` — как у `ConnectClient`.
//!
//! TODO(de-dup ACP/Connect rpc-core): pending-map + next_id + request-with-timeout продублированы из
//! `client.rs` НАМЕРЕННО — чтобы НЕ трогать горячий путь CONN-2 (zero-regression приоритетнее DRY). После
//! стабилизации ACP-1 вынести общий `rpc_core`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::{mpsc, oneshot, Mutex};

use super::super::{RpcError, RpcMessage, Transport, TransportError, EVENT_CHANNEL_CAP};
use super::schema::{RequestPermissionParams, SessionNotification};

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>>>;

/// Входящий от агента `session/request_permission` (нужен наш Response). `id` — для ответа.
pub struct InboundPermission {
    pub id: Value,
    pub params: RequestPermissionParams,
}

/// Клиент ACP: исходящие запросы (`initialize`/`session/new`/`session/prompt`/`session/cancel`) +
/// демультиплекс входящих (session/update → канал; request_permission → канал; fs/terminal → auto-deny).
pub struct AcpClient {
    transport: Arc<dyn Transport>,
    next_id: AtomicI64,
    pending: Pending,
    read_task: tokio::task::JoinHandle<()>,
}

impl Drop for AcpClient {
    fn drop(&mut self) {
        self.read_task.abort();
    }
}

impl AcpClient {
    /// Поднимает bidirectional read-loop. Возвращает клиент + поток `session/update` + поток входящих
    /// permission-запросов. `fs/*`/`terminal/*` агента read-loop сам отвечает `method_not_found`
    /// (мы объявили capabilities=false — агент звать их НЕ должен; если зовёт, fail-closed без зависа).
    pub fn new(
        transport: Arc<dyn Transport>,
    ) -> (
        Self,
        mpsc::Receiver<SessionNotification>,
        mpsc::Receiver<InboundPermission>,
    ) {
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (updates_tx, updates_rx) = mpsc::channel::<SessionNotification>(EVENT_CHANNEL_CAP);
        let (perms_tx, perms_rx) = mpsc::channel::<InboundPermission>(16);
        let read_task = tokio::spawn(acp_read_loop(
            transport.clone(),
            pending.clone(),
            updates_tx,
            perms_tx,
        ));
        (
            Self {
                transport,
                next_id: AtomicI64::new(1),
                pending,
                read_task,
            },
            updates_rx,
            perms_rx,
        )
    }

    /// Исходящий запрос с ОПЦИОНАЛЬНЫМ таймаутом. `None` → ждём бесконечно (для `session/prompt`: целый
    /// ход + cold-start модели 1-3 мин). `Some(d)` → управляющие RPC (`initialize`/`session/new`/`cancel`).
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
    ) -> Result<Value, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        if self
            .transport
            .send(RpcMessage::request(id, method, params))
            .await
            .is_err()
        {
            self.pending.lock().await.remove(&id);
            return Err(RpcError::internal("acp transport send failed"));
        }
        let wait = async {
            match rx.await {
                Ok(result) => result,
                Err(_) => Err(RpcError::internal("acp response channel closed")),
            }
        };
        match timeout {
            Some(d) => match tokio::time::timeout(d, wait).await {
                Ok(r) => {
                    if r.is_err() {
                        self.pending.lock().await.remove(&id);
                    }
                    r
                }
                Err(_) => {
                    self.pending.lock().await.remove(&id);
                    Err(RpcError::internal("acp request timeout"))
                }
            },
            None => {
                let r = wait.await;
                if r.is_err() {
                    self.pending.lock().await.remove(&id);
                }
                r
            }
        }
    }

    /// Исходящее уведомление (без ответа): `session/cancel`.
    pub async fn notify(&self, method: &str, params: Value) -> Result<(), TransportError> {
        self.transport
            .send(RpcMessage::notification(method, params))
            .await
    }

    /// Отвечает на ВХОДЯЩИЙ запрос агента (по его `id`) — используется мостом аппрува для permission-ответа.
    pub async fn respond(
        &self,
        id: Value,
        result: Result<Value, RpcError>,
    ) -> Result<(), TransportError> {
        self.transport
            .send(RpcMessage::Response { id, result })
            .await
    }
}

/// Двунаправленный read-loop: (a) Response → ждущим по `id`; (b) `session/update` → updates-канал;
/// (c) входящий `session/request_permission` → perms-канал; (d) прочие входящие запросы (fs/terminal) →
/// `method_not_found` (мы не объявляли caps). Закрытие транспорта → провал всех ждущих, закрытие каналов.
async fn acp_read_loop(
    transport: Arc<dyn Transport>,
    pending: Pending,
    updates_tx: mpsc::Sender<SessionNotification>,
    perms_tx: mpsc::Sender<InboundPermission>,
) {
    while let Some(msg) = transport.recv().await {
        match msg {
            RpcMessage::Response { id, result } => {
                if let Some(i) = id.as_i64() {
                    if let Some(tx) = pending.lock().await.remove(&i) {
                        let _ = tx.send(result);
                    }
                }
            }
            RpcMessage::Notification { method, params } if method == "session/update" => {
                if let Ok(n) = serde_json::from_value::<SessionNotification>(params) {
                    let _ = updates_tx.try_send(n); // best-effort (bounded)
                }
            }
            RpcMessage::Notification { .. } => { /* прочие нотификации агента — игнор в первом срезе */
            }
            RpcMessage::Request { id, method, params } => match method.as_str() {
                "session/request_permission" => {
                    match serde_json::from_value::<RequestPermissionParams>(params) {
                        Ok(p) => {
                            // Агент БЛОКИРУЕТСЯ на этом запросе до нашего Response → канал не переполнится.
                            // Если перегружен (закрыт consumer) — fail-closed: отвечаем Cancelled сами.
                            if perms_tx
                                .send(InboundPermission {
                                    id: id.clone(),
                                    params: p,
                                })
                                .await
                                .is_err()
                            {
                                let cancelled =
                                    serde_json::json!({"outcome": {"outcome": "cancelled"}});
                                let _ = transport
                                    .send(RpcMessage::Response {
                                        id,
                                        result: Ok(cancelled),
                                    })
                                    .await;
                            }
                        }
                        Err(_) => {
                            let _ = transport
                                .send(RpcMessage::Response {
                                    id,
                                    result: Err(RpcError::invalid_params()),
                                })
                                .await;
                        }
                    }
                }
                // fs/read_text_file, fs/write_text_file, terminal/* — caps=false → method_not_found
                // (НИКОГДА не вешаем агента молча; он не должен звать без объявленной capability).
                _ => {
                    let _ = transport
                        .send(RpcMessage::Response {
                            id,
                            result: Err(RpcError::method_not_found()),
                        })
                        .await;
                }
            },
        }
    }
    // Транспорт закрыт → провалить все ждущие исходящие запросы. updates_tx/perms_tx дропаются здесь
    // (consumer видит конец стрима → мост аппрува разрешит висящие permission в Cancelled).
    let mut p = pending.lock().await;
    for (_, tx) in p.drain() {
        let _ = tx.send(Err(RpcError::internal("acp transport closed")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::connect::{channel_pair, ChannelTransport};
    use serde_json::json;

    /// Скриптованный мок-ACP-агент на другом конце ChannelTransport: проходит весь путь
    /// initialize → session/new → prompt(стрим + request_permission round-trip) → end_turn.
    async fn mock_agent(srv: ChannelTransport) {
        // initialize
        let req = srv.recv().await.unwrap();
        if let RpcMessage::Request { id, method, .. } = req {
            assert_eq!(method, "initialize");
            srv.send(RpcMessage::Response {
                id,
                result: Ok(json!({"protocolVersion": 1})),
            })
            .await
            .unwrap();
        } else {
            panic!("ждали initialize");
        }
        // session/new
        let req = srv.recv().await.unwrap();
        let RpcMessage::Request { id, method, .. } = req else {
            panic!("ждали session/new")
        };
        assert_eq!(method, "session/new");
        srv.send(RpcMessage::Response {
            id,
            result: Ok(json!({"sessionId": "s1"})),
        })
        .await
        .unwrap();
        // session/prompt
        let req = srv.recv().await.unwrap();
        let RpcMessage::Request {
            id: prompt_id,
            method,
            ..
        } = req
        else {
            panic!("ждали session/prompt")
        };
        assert_eq!(method, "session/prompt");
        // стрим: токен
        srv.send(RpcMessage::notification(
            "session/update",
            json!({"sessionId":"s1","sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"}}),
        ))
        .await
        .unwrap();
        // запрос разрешения (с diff) — БЛОКИРУЕМСЯ до ответа клиента
        srv.send(RpcMessage::request(
            777,
            "session/request_permission",
            json!({"sessionId":"s1","toolCall":{"toolCallId":"t1","content":[{"type":"diff","path":"Notes/A.md","newText":"x"}]},
                   "options":[{"optionId":"a","name":"Allow","kind":"allow_once"},{"optionId":"d","name":"Deny","kind":"reject_once"}]}),
        ))
        .await
        .unwrap();
        let resp = srv.recv().await.unwrap();
        match resp {
            RpcMessage::Response { id, result } => {
                assert_eq!(id, json!(777));
                assert_eq!(
                    result.unwrap(),
                    json!({"outcome":{"outcome":"selected","optionId":"a"}})
                );
            }
            other => panic!("ждали Response на permission, got {other:?}"),
        }
        // финал хода
        srv.send(RpcMessage::Response {
            id: prompt_id,
            result: Ok(json!({"stopReason":"end_turn"})),
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn acp_client_full_bidirectional_round_trip() {
        let (client_t, server_t) = channel_pair();
        let agent = tokio::spawn(mock_agent(server_t));

        let (client, mut updates, mut perms) = AcpClient::new(Arc::new(client_t));
        // initialize
        let r = client
            .request("initialize", json!({"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}}), Some(Duration::from_secs(5)))
            .await
            .unwrap();
        assert_eq!(r["protocolVersion"], 1);
        // session/new
        let r = client
            .request(
                "session/new",
                json!({"cwd":"/v","mcpServers":[]}),
                Some(Duration::from_secs(5)),
            )
            .await
            .unwrap();
        assert_eq!(r["sessionId"], "s1");

        // prompt (no timeout) — gонится конкурентно; пока он крутится, обрабатываем update + permission
        let prompt = client.request(
            "session/prompt",
            json!({"sessionId":"s1","prompt":[{"type":"text","text":"do it"}]}),
            None,
        );

        let drive = async {
            // первый апдейт — токен
            let n = updates.recv().await.unwrap();
            assert!(matches!(
                n.update,
                super::super::schema::SessionUpdate::AgentMessageChunk { .. }
            ));
            // входящий permission → отвечаем Selected(allow option)
            let p = perms.recv().await.unwrap();
            assert_eq!(p.params.options[0].option_id, "a");
            client
                .respond(
                    p.id,
                    Ok(json!({"outcome":{"outcome":"selected","optionId":"a"}})),
                )
                .await
                .unwrap();
        };

        let (prompt_res, _) = tokio::join!(prompt, drive);
        assert_eq!(prompt_res.unwrap()["stopReason"], "end_turn");
        agent.await.unwrap();
    }

    #[tokio::test]
    async fn acp_client_request_errs_on_closed_transport() {
        let (client_t, server_t) = channel_pair();
        drop(server_t); // агент ушёл
        let (client, _u, _p) = AcpClient::new(Arc::new(client_t));
        let r = client
            .request("initialize", json!({}), Some(Duration::from_millis(200)))
            .await;
        assert!(r.is_err(), "закрытый транспорт → Err, не зависание");
    }
}
