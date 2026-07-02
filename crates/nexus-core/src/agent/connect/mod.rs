//! AGENT-CONNECT (P0a) — транспорт-агностичный JSON-RPC 2.0 слой коннектора app ↔ `nexus-agentd`.
//!
//! Спека: `docs/specs/agent-connect.md`. **P0a = протокол-ФУНДАМЕНТ** (чистая граница, без LLM):
//! framing JSON-RPC 2.0 · подключаемый [`Transport`] (in-process [`ChannelTransport`]; WS/AF_UNIX —
//! позже) · [`dispatch`] метод→[`ConnectHandler`] · version-negotiate · sanitized-ошибки ·
//! [`acp_tool_kind`]. Маппинг `AgentEvent`→`agent/event` + привязка к `run_agent_loop` + LIVE tool-loop
//! на риг — СЛЕДУЮЩИЙ срез P0b (agentd-интеграция): там есть LLM, тут его нет.
//!
//! ACP-совместимость: методы/события зеркалят ACP-семантику (см. `acp_tool_kind`), framing —
//! JSON-RPC 2.0 (requests с `id` + notifications без `id`; ответы коррелируются по `id`, out-of-order).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::event::AgentEvent;

pub mod acp;
pub mod client;
pub mod handler;
pub mod stdio;
pub mod wire;
// R-1 (развязка слоёв): транспорт-нейтральное ядро JSON-RPC (конверт [`RpcMessage`]/[`RpcError`] +
// [`Transport`]/[`ChannelTransport`] + line-framing) живёт в [`crate::rpc`]; старые пути
// `agent::connect::*` сохранены реэкспортами (потребители — sandbox/agentd/cli/desktop — не правятся).
pub(crate) use crate::rpc::framing;
pub use crate::rpc::{
    channel_pair, ChannelTransport, RpcError, RpcMessage, Transport, TransportError,
};
pub use client::{ConnectClient, ConnectError};
pub use handler::{ConnectAgentHandler, ConnectDeps};
pub use stdio::StdioTransport;
pub use wire::{
    map_agent_event, AgentFileStatus, AgentPlanStep, AgentPlanStepState, AgentProposedFile,
    AgentProposedKind, AgentStreamEvent,
};

// AF_UNIX-хостинг коннектора (P0b-2c) — Unix-only (на Windows `tokio::net::Unix*` отсутствует).
#[cfg(unix)]
pub mod afunix;
#[cfg(unix)]
pub use afunix::{connect_unix, operator_uid, serve_unix, serve_unix_at, AfUnixTransport};
// Хардненинг bind-пути сокета (0600 + non-socket-refusal) + peer-uid через SO_PEERCRED —
// переиспользует host-side `SandboxRunner` (Unix-only, как и весь AF_UNIX-хостинг).
#[cfg(unix)]
pub(crate) use afunix::{harden_socket_perms, peer_uid, prepare_socket_path};

/// Версия протокола этой сборки. Клиент объявляет поддерживаемые в `initialize`; сервер выбирает.
pub const PROTOCOL_VERSION: &str = "1.0";
/// Поддерживаемые версии (по убыванию предпочтения).
pub const SUPPORTED_VERSIONS: &[&str] = &["1.0"];

/// Глубина канала событий прогона (forwarder→drain→транспорт). БОЛЬШОЙ, но ОГРАНИЧЕННЫЙ (не unbounded):
/// здоровый клиент дренирует быстрее, чем цикл эмитит, так что кап не достигается; но если клиент отвалился
/// и drain встал, кап не даёт памяти расти бесконечно, пока прогон ещё крутится (`forward` — `try_send`,
/// дроп при переполнении: события best-effort). ЕДИНЫЙ источник: и in-process коннектор (`handler.rs`), и
/// OUTWARD-форвардер песочницы (`sandbox::event`) используют ЭТУ константу — backpressure-поведение обоих
/// путей событий не дрейфует.
pub const EVENT_CHANNEL_CAP: usize = 1024;

// ───────────────────────── Методы: params / result ─────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// Версии протокола, поддерживаемые клиентом.
    pub supported_versions: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InitializeResult {
    /// Выбранная сервером совместимая версия.
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunParams {
    pub session_id: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model_override: Option<String>,
}

/// Решение по одному предложенному действию (по `action_id` из `AgentEvent::Proposal`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ItemDecision {
    pub action_id: i64,
    pub approved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApproveParams {
    pub session_id: String,
    pub run_id: String,
    pub decisions: Vec<ItemDecision>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ControlParams {
    pub session_id: String,
    pub pause: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunRef {
    pub session_id: String,
    pub run_id: String,
}
/// Параметры undo/cancel (оба адресуют конкретный прогон).
pub type UndoParams = RunRef;
pub type CancelParams = RunRef;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UndoResult {
    /// Сколько действий восстановлено (идемпотентно: повторный undo → 0).
    pub restored: u32,
}

// ───────────────────────── Handler + dispatch ─────────────────────────

/// Поведение агент-сервиса за протоколом. Реализуется в `nexus-agentd` (там есть провайдер/память/
/// актуатор); P0a определяет границу + диспетчер. Стрим `AgentEvent` сервис шлёт сам через
/// [`event_notification`] на свой конец транспорта (не возврат из метода).
#[async_trait]
pub trait ConnectHandler: Send + Sync {
    async fn initialize(&self, p: InitializeParams) -> Result<InitializeResult, RpcError>;
    /// Запускает прогон; возвращает ack (напр. `{ "runId": ... }`). События — отдельным стримом.
    async fn agent_run(&self, p: AgentRunParams) -> Result<Value, RpcError>;
    async fn agent_undo(&self, p: UndoParams) -> Result<UndoResult, RpcError>;
    async fn agent_cancel(&self, p: CancelParams) -> Result<Value, RpcError>;
    /// Notification (без ответа): решение по предложениям → кормит DecisionSource.
    async fn agent_approve(&self, p: ApproveParams);
    /// Notification (без ответа): пауза/возобновление (kill-switch, durable в agent.json).
    async fn agent_control(&self, p: ControlParams);
}

fn parse_params<T: for<'de> Deserialize<'de>>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params).map_err(|_| RpcError::invalid_params())
}

/// Обрабатывает одно входящее сообщение: запрос → хендлер → Response в `out`; уведомление → хендлер
/// (без ответа); ответ от клиента в P0a игнорируется (сервер не шлёт запросов клиенту).
pub async fn dispatch(handler: &dyn ConnectHandler, msg: RpcMessage, out: &dyn Transport) {
    match msg {
        RpcMessage::Request { id, method, params } => {
            let result = route_request(handler, &method, params).await;
            let _ = out.send(RpcMessage::Response { id, result }).await;
        }
        RpcMessage::Notification { method, params } => {
            route_notification(handler, &method, params).await;
        }
        RpcMessage::Response { .. } => { /* P0a: клиент не отвечает серверу — игнор */
        }
    }
}

async fn route_request(
    handler: &dyn ConnectHandler,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    match method {
        "initialize" => {
            let p = parse_params::<InitializeParams>(params)?;
            let r = handler.initialize(p).await?;
            serde_json::to_value(r).map_err(|e| RpcError::internal(e.to_string()))
        }
        "agent/run" => {
            let p = parse_params::<AgentRunParams>(params)?;
            handler.agent_run(p).await
        }
        "agent/undo" => {
            let p = parse_params::<UndoParams>(params)?;
            let r = handler.agent_undo(p).await?;
            serde_json::to_value(r).map_err(|e| RpcError::internal(e.to_string()))
        }
        "agent/cancel" => {
            let p = parse_params::<CancelParams>(params)?;
            handler.agent_cancel(p).await
        }
        _ => Err(RpcError::method_not_found()),
    }
}

async fn route_notification(handler: &dyn ConnectHandler, method: &str, params: Value) {
    match method {
        "agent/approve" => {
            if let Ok(p) = parse_params::<ApproveParams>(params) {
                handler.agent_approve(p).await;
            }
        }
        "agent/control" => {
            if let Ok(p) = parse_params::<ControlParams>(params) {
                handler.agent_control(p).await;
            }
        }
        _ => { /* неизвестное уведомление — тихо игнор (JSON-RPC: на notification ответа нет) */
        }
    }
}

// ───────────────────────── Version + ACP-маппинг + события ─────────────────────────

/// Выбирает наибольшую совместимую версию: первую из [`SUPPORTED_VERSIONS`] (по предпочтению),
/// которую заявил клиент. `None` — нет общей версии.
pub fn negotiate_version(client_supported: &[String]) -> Option<&'static str> {
    SUPPORTED_VERSIONS
        .iter()
        .copied()
        .find(|v| client_supported.iter().any(|c| c == v))
}

/// Маппинг нашего dotted tool-kind → ACP tool-kind (read/write/search/other) для plan/визуализации.
pub fn acp_tool_kind(nexus_kind: &str) -> &'static str {
    match nexus_kind {
        "note.create" | "note.edit" | "set_frontmatter" | "fs.write" => "write",
        "read_note" | "fs.read" | "vault.read" => "read",
        "search" | "search_semantic" | "ai.searchSemantic" => "search",
        _ => "other",
    }
}

/// Оборачивает событие агента (`AgentEvent`) в `agent/event`-уведомление через wire-DTO
/// ([`map_agent_event`]). `None` — событие ядра без wire-представления (`non_exhaustive`-задел) →
/// не стримим. NB: маппим через DTO, а НЕ `to_value(AgentEvent)` — у ядра newtype-варианты
/// (`Final(String)` и т.п.) несовместимы с serde-internal-tag (см. регрессию в [`wire`]).
pub fn event_notification(ev: &AgentEvent) -> Option<RpcMessage> {
    let wire = map_agent_event(ev)?;
    // wire-DTO — struct-вариантный теговый enum, сериализуется штатно.
    let params = serde_json::to_value(wire).ok()?;
    Some(RpcMessage::notification("agent/event", params))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// `Arc<T>: Transport` делегирует send/recv → ДВА Arc-клона делят ОДНО соединение (6c-2f shared
    /// act.sock). Оба send'а одного клона приходят на другой конец (один underlying транспорт).
    #[tokio::test]
    async fn arc_transport_shares_one_connection() {
        let (a, b) = channel_pair();
        let a = Arc::new(a);
        let a2 = a.clone();
        a.send(RpcMessage::request(1, "ping", json!(null)))
            .await
            .unwrap();
        a2.send(RpcMessage::request(2, "pong", json!(null)))
            .await
            .unwrap();
        // Оба сообщения (с РАЗНЫХ Arc-клонов) пришли на b → клоны делят один underlying транспорт.
        assert!(matches!(b.recv().await, Some(RpcMessage::Request { .. })));
        assert!(matches!(b.recv().await, Some(RpcMessage::Request { .. })));
    }

    // ── framing ──
    #[test]
    fn rpc_envelope_roundtrip_classifies() {
        let req = RpcMessage::request(1, "initialize", json!({"supportedVersions":["1.0"]}));
        let parsed = RpcMessage::from_json(&req.to_json()).unwrap();
        assert_eq!(parsed, req);
        assert!(matches!(parsed, RpcMessage::Request { .. }));

        let note = RpcMessage::notification("agent/control", json!({"sessionId":"s","pause":true}));
        assert_eq!(RpcMessage::from_json(&note.to_json()).unwrap(), note);

        let ok = RpcMessage::Response {
            id: json!(1),
            result: Ok(json!({"version":"1.0"})),
        };
        assert_eq!(RpcMessage::from_json(&ok.to_json()).unwrap(), ok);
        let err = RpcMessage::Response {
            id: json!(2),
            result: Err(RpcError::method_not_found()),
        };
        assert_eq!(RpcMessage::from_json(&err.to_json()).unwrap(), err);
    }

    #[test]
    fn rpc_rejects_bad_jsonrpc_and_garbage() {
        assert_eq!(
            RpcMessage::from_json("not json").unwrap_err(),
            RpcError::parse()
        );
        // jsonrpc != "2.0"
        assert_eq!(
            RpcMessage::from_json(r#"{"jsonrpc":"1.0","method":"x","id":1}"#).unwrap_err(),
            RpcError::invalid_request()
        );
        // ни method, ни id
        assert_eq!(
            RpcMessage::from_json(r#"{"jsonrpc":"2.0"}"#).unwrap_err(),
            RpcError::invalid_request()
        );
    }

    #[test]
    fn version_negotiation() {
        assert_eq!(negotiate_version(&["1.0".into()]), Some("1.0"));
        assert_eq!(
            negotiate_version(&["2.0".into(), "1.0".into()]),
            Some("1.0")
        );
        assert_eq!(negotiate_version(&["0.9".into()]), None);
        assert_eq!(negotiate_version(&[]), None);
    }

    #[test]
    fn internal_error_does_not_leak_detail() {
        let e = RpcError::internal("vault open failed: /home/user/.secret/path");
        assert_eq!(e.code, -32603);
        assert_eq!(e.message, "internal error");
        assert!(!e.message.contains("/home"));
    }

    #[test]
    fn acp_tool_kind_maps() {
        for k in ["note.create", "note.edit", "set_frontmatter", "fs.write"] {
            assert_eq!(acp_tool_kind(k), "write", "{k}");
        }
        for k in ["read_note", "fs.read", "vault.read"] {
            assert_eq!(acp_tool_kind(k), "read", "{k}");
        }
        for k in ["search", "search_semantic", "ai.searchSemantic"] {
            assert_eq!(acp_tool_kind(k), "search", "{k}");
        }
        assert_eq!(acp_tool_kind("debug.echo"), "other");
        assert_eq!(acp_tool_kind(""), "other");
    }

    #[test]
    fn response_rejects_both_result_and_error() {
        // JSON-RPC: result И error вместе → невалидно (отвергаем сообщение).
        let both = r#"{"jsonrpc":"2.0","id":1,"result":{},"error":{"code":-1,"message":"x"}}"#;
        assert_eq!(
            RpcMessage::from_json(both).unwrap_err(),
            RpcError::invalid_request()
        );
        // id без method/result/error → тоже невалидный ответ.
        let neither = r#"{"jsonrpc":"2.0","id":1}"#;
        assert_eq!(
            RpcMessage::from_json(neither).unwrap_err(),
            RpcError::invalid_request()
        );
    }

    /// Регрессия-якорь для P0b: `AgentEvent` помечен `#[serde(tag="type")]`, но имеет newtype-варианты
    /// (`Final(String)`) — serde-internal-tag их сериализовать НЕ может. P0b ОБЯЗАН делать явный
    /// wire-DTO-маппинг, НЕ `to_value(event)`. Если этот тест начнёт ПАДАТЬ (serde починили newtype) —
    /// можно упростить P0b. Пока — фиксируем ловушку.
    #[test]
    fn agent_event_newtype_is_not_directly_serializable() {
        assert!(
            serde_json::to_value(AgentEvent::Final("done".into())).is_err(),
            "если стало Ok — serde-поведение изменилось, пересмотреть wire-DTO"
        );
    }

    #[test]
    fn event_notification_wraps_via_wire_dto() {
        // То, что ядро НЕ сериализует напрямую — через wire-DTO уходит штатно в agent/event.
        let n = event_notification(&AgentEvent::Final("done".into())).unwrap();
        match n {
            RpcMessage::Notification { method, params } => {
                assert_eq!(method, "agent/event");
                assert_eq!(params["type"], "final");
                assert_eq!(params["text"], "done");
            }
            _ => panic!("expected notification"),
        }
    }

    // ── transport ──
    #[tokio::test]
    async fn channel_transport_duplex_roundtrip() {
        let (a, b) = channel_pair();
        let req = RpcMessage::request(1, "initialize", json!({"supportedVersions":["1.0"]}));
        a.send(req.clone()).await.unwrap();
        assert_eq!(b.recv().await.unwrap(), req);
        let resp = RpcMessage::Response {
            id: json!(1),
            result: Ok(json!({"version":"1.0"})),
        };
        b.send(resp.clone()).await.unwrap();
        assert_eq!(a.recv().await.unwrap(), resp);
    }

    #[tokio::test]
    async fn channel_transport_closed_after_drop() {
        let (a, b) = channel_pair();
        drop(b);
        assert_eq!(
            a.send(RpcMessage::notification("x", Value::Null)).await,
            Err(TransportError::Closed)
        );
    }

    // ── dispatch (mock handler) ──
    struct MockHandler {
        approves: AtomicUsize,
        controls: AtomicUsize,
    }
    #[async_trait]
    impl ConnectHandler for MockHandler {
        async fn initialize(&self, p: InitializeParams) -> Result<InitializeResult, RpcError> {
            match negotiate_version(&p.supported_versions) {
                Some(v) => Ok(InitializeResult { version: v.into() }),
                None => Err(RpcError::version_incompatible()),
            }
        }
        async fn agent_run(&self, _p: AgentRunParams) -> Result<Value, RpcError> {
            Ok(json!({ "runId": "r1" }))
        }
        async fn agent_undo(&self, _p: UndoParams) -> Result<UndoResult, RpcError> {
            Ok(UndoResult { restored: 0 })
        }
        async fn agent_cancel(&self, _p: CancelParams) -> Result<Value, RpcError> {
            Ok(json!({ "ok": true }))
        }
        async fn agent_approve(&self, _p: ApproveParams) {
            self.approves.fetch_add(1, Ordering::Relaxed);
        }
        async fn agent_control(&self, _p: ControlParams) {
            self.controls.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[tokio::test]
    async fn dispatch_request_returns_response() {
        let h = MockHandler {
            approves: AtomicUsize::new(0),
            controls: AtomicUsize::new(0),
        };
        let (client, server) = channel_pair();
        dispatch(
            &h,
            RpcMessage::request(7, "initialize", json!({"supportedVersions":["1.0"]})),
            &server,
        )
        .await;
        match client.recv().await.unwrap() {
            RpcMessage::Response { id, result } => {
                assert_eq!(id, json!(7));
                assert_eq!(result.unwrap()["version"], "1.0");
            }
            _ => panic!("expected response"),
        }
    }

    #[tokio::test]
    async fn dispatch_unknown_method_errors() {
        let h = MockHandler {
            approves: AtomicUsize::new(0),
            controls: AtomicUsize::new(0),
        };
        let (client, server) = channel_pair();
        dispatch(&h, RpcMessage::request(1, "no/such", Value::Null), &server).await;
        match client.recv().await.unwrap() {
            RpcMessage::Response { result, .. } => {
                assert_eq!(result.unwrap_err(), RpcError::method_not_found());
            }
            _ => panic!("expected error response"),
        }
    }

    #[tokio::test]
    async fn dispatch_bad_params_errors() {
        let h = MockHandler {
            approves: AtomicUsize::new(0),
            controls: AtomicUsize::new(0),
        };
        let (client, server) = channel_pair();
        // initialize без supportedVersions → invalid params
        dispatch(
            &h,
            RpcMessage::request(1, "initialize", json!({"wrong":1})),
            &server,
        )
        .await;
        match client.recv().await.unwrap() {
            RpcMessage::Response { result, .. } => {
                assert_eq!(result.unwrap_err(), RpcError::invalid_params());
            }
            _ => panic!("expected error response"),
        }
    }

    #[tokio::test]
    async fn dispatch_notification_no_response() {
        let h = Arc::new(MockHandler {
            approves: AtomicUsize::new(0),
            controls: AtomicUsize::new(0),
        });
        let (client, server) = channel_pair();
        dispatch(
            h.as_ref(),
            RpcMessage::notification("agent/control", json!({"sessionId":"s","pause":true})),
            &server,
        )
        .await;
        assert_eq!(h.controls.load(Ordering::Relaxed), 1);
        // ответа быть НЕ должно (notification) — канал клиента пуст; проверяем неблокирующе.
        assert!(client.rx.lock().await.try_recv().is_err());
    }

    #[tokio::test]
    async fn dispatch_incompatible_version_errors() {
        let h = MockHandler {
            approves: AtomicUsize::new(0),
            controls: AtomicUsize::new(0),
        };
        let (client, server) = channel_pair();
        dispatch(
            &h,
            RpcMessage::request(1, "initialize", json!({"supportedVersions":["9.9"]})),
            &server,
        )
        .await;
        match client.recv().await.unwrap() {
            RpcMessage::Response { result, .. } => {
                assert_eq!(result.unwrap_err(), RpcError::version_incompatible());
            }
            _ => panic!("expected error response"),
        }
    }

    #[tokio::test]
    async fn dispatch_agent_run_and_undo() {
        let h = MockHandler {
            approves: AtomicUsize::new(0),
            controls: AtomicUsize::new(0),
        };
        let (client, server) = channel_pair();
        // agent/run → ack { runId }
        dispatch(
            &h,
            RpcMessage::request(1, "agent/run", json!({"sessionId":"s","prompt":"hi"})),
            &server,
        )
        .await;
        match client.recv().await.unwrap() {
            RpcMessage::Response { result, .. } => assert_eq!(result.unwrap()["runId"], "r1"),
            _ => panic!("expected response"),
        }
        // agent/undo → { restored }
        dispatch(
            &h,
            RpcMessage::request(2, "agent/undo", json!({"sessionId":"s","runId":"r1"})),
            &server,
        )
        .await;
        match client.recv().await.unwrap() {
            RpcMessage::Response { result, .. } => assert_eq!(result.unwrap()["restored"], 0),
            _ => panic!("expected response"),
        }
    }

    #[tokio::test]
    async fn dispatch_approve_notification_feeds_handler() {
        let h = MockHandler {
            approves: AtomicUsize::new(0),
            controls: AtomicUsize::new(0),
        };
        let (client, server) = channel_pair();
        dispatch(
            &h,
            RpcMessage::notification(
                "agent/approve",
                json!({"sessionId":"s","runId":"r1","decisions":[{"actionId":7,"approved":true}]}),
            ),
            &server,
        )
        .await;
        assert_eq!(h.approves.load(Ordering::Relaxed), 1);
        assert!(client.rx.lock().await.try_recv().is_err()); // notification → без ответа
    }
}
