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
use tokio::sync::{mpsc, Mutex};

use super::event::AgentEvent;

pub mod handler;
pub mod wire;
pub use handler::{ConnectAgentHandler, ConnectDeps};
pub use wire::{map_agent_event, AgentFileStatus, AgentProposedFile, AgentStreamEvent};

// AF_UNIX-хостинг коннектора (P0b-2c) — Unix-only (на Windows `tokio::net::Unix*` отсутствует).
#[cfg(unix)]
pub mod afunix;
#[cfg(unix)]
pub use afunix::{connect_unix, serve_unix, serve_unix_at, AfUnixTransport};

/// Версия протокола этой сборки. Клиент объявляет поддерживаемые в `initialize`; сервер выбирает.
pub const PROTOCOL_VERSION: &str = "1.0";
/// Поддерживаемые версии (по убыванию предпочтения).
pub const SUPPORTED_VERSIONS: &[&str] = &["1.0"];

// ───────────────────────── JSON-RPC 2.0 framing ─────────────────────────

/// Ошибка JSON-RPC (code+message). **Sanitized**: `message` для клиента — общий, без путей/токенов;
/// детали уходят в server-лог через конструкторы (T3/THREAT_MODEL).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

impl RpcError {
    pub fn parse() -> Self {
        Self {
            code: -32700,
            message: "parse error".into(),
        }
    }
    pub fn invalid_request() -> Self {
        Self {
            code: -32600,
            message: "invalid request".into(),
        }
    }
    pub fn method_not_found() -> Self {
        Self {
            code: -32601,
            message: "method not found".into(),
        }
    }
    pub fn invalid_params() -> Self {
        Self {
            code: -32602,
            message: "invalid params".into(),
        }
    }
    /// Внутренняя ошибка: `detail` НЕ уходит клиенту (только в лог), сообщение — общее (анти-утечка).
    pub fn internal(detail: impl AsRef<str>) -> Self {
        tracing::warn!(target: "agent::connect", detail = detail.as_ref(), "internal rpc error");
        Self {
            code: -32603,
            message: "internal error".into(),
        }
    }
    /// Несовместимая версия протокола (custom-код вне зарезервированного JSON-RPC-диапазона).
    pub fn version_incompatible() -> Self {
        Self {
            code: -32001,
            message: "protocol version incompatible".into(),
        }
    }
}

/// Сырой конверт JSON-RPC (для serde на проводе). Опциональные поля омитятся (не шлём `null`).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RpcEnvelope {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

/// Типизированное сообщение протокола.
#[derive(Debug, Clone, PartialEq)]
pub enum RpcMessage {
    /// Запрос (ждёт ответа), коррелируется по `id`.
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// Уведомление (без ответа).
    Notification { method: String, params: Value },
    /// Ответ на запрос с тем же `id` (Ok-результат или ошибка).
    Response {
        id: Value,
        result: Result<Value, RpcError>,
    },
}

impl RpcMessage {
    pub fn request(id: impl Into<Value>, method: impl Into<String>, params: Value) -> Self {
        Self::Request {
            id: id.into(),
            method: method.into(),
            params,
        }
    }
    pub fn notification(method: impl Into<String>, params: Value) -> Self {
        Self::Notification {
            method: method.into(),
            params,
        }
    }

    /// Сериализация в строку JSON (для WS/stdio-транспорта; in-process передаёт структуру напрямую).
    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.to_envelope()).unwrap_or_else(|_| "{}".into())
    }

    /// Разбор из JSON-строки → классификация в типизированное сообщение.
    pub fn from_json(s: &str) -> Result<Self, RpcError> {
        let env: RpcEnvelope = serde_json::from_str(s).map_err(|_| RpcError::parse())?;
        Self::from_envelope(env)
    }

    fn to_envelope(&self) -> RpcEnvelope {
        let base = |id: Option<Value>| RpcEnvelope {
            jsonrpc: "2.0".into(),
            id,
            method: None,
            params: None,
            result: None,
            error: None,
        };
        match self.clone() {
            RpcMessage::Request { id, method, params } => RpcEnvelope {
                method: Some(method),
                params: Some(params),
                ..base(Some(id))
            },
            RpcMessage::Notification { method, params } => RpcEnvelope {
                method: Some(method),
                params: Some(params),
                ..base(None)
            },
            RpcMessage::Response { id, result } => match result {
                Ok(v) => RpcEnvelope {
                    result: Some(v),
                    ..base(Some(id))
                },
                Err(e) => RpcEnvelope {
                    error: Some(e),
                    ..base(Some(id))
                },
            },
        }
    }

    fn from_envelope(env: RpcEnvelope) -> Result<Self, RpcError> {
        if env.jsonrpc != "2.0" {
            return Err(RpcError::invalid_request());
        }
        match (env.method, env.id) {
            (Some(method), Some(id)) => Ok(RpcMessage::Request {
                id,
                method,
                params: env.params.unwrap_or(Value::Null),
            }),
            (Some(method), None) => Ok(RpcMessage::Notification {
                method,
                params: env.params.unwrap_or(Value::Null),
            }),
            (None, Some(id)) => {
                // JSON-RPC 2.0: ответ обязан нести РОВНО ОДНО из result/error. Дубль и отсутствие
                // обоих — невалидный ответ (отвергаем сообщение, ревью).
                let result = match (env.result, env.error) {
                    (Some(_), Some(_)) | (None, None) => return Err(RpcError::invalid_request()),
                    (_, Some(e)) => Err(e),
                    (Some(v), None) => Ok(v),
                };
                Ok(RpcMessage::Response { id, result })
            }
            (None, None) => Err(RpcError::invalid_request()),
        }
    }
}

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

// ───────────────────────── Transport ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    /// Канал закрыт (пир ушёл).
    Closed,
}

/// Подключаемый транспорт протокола: in-process / AF_UNIX / WS / stdio. P0a — только in-process
/// [`ChannelTransport`]; сетевые — следующие срезы (за тем же трейтом).
///
/// **Контракт: ОДИН consumer.** `recv` рассчитан на единственную приёмную задачу на эндпоинт (типовой
/// serve-loop / клиентский read-loop). Конкурентные `recv` на одном эндпоинте НЕ дедлокнут (tokio
/// async-Mutex; `send` идёт отдельным каналом — цикла блокировок нет), но СЕРИАЛИЗУЮТСЯ и поделят
/// поток (mpsc — один потребитель), что почти наверняка не то, что нужно. Держи один recv-таск.
#[async_trait]
pub trait Transport: Send + Sync {
    async fn send(&self, msg: RpcMessage) -> Result<(), TransportError>;
    /// `None` — транспорт закрыт. См. контракт «один consumer» на трейте.
    async fn recv(&self) -> Option<RpcMessage>;
}

/// In-process дуплекс на `tokio::mpsc` (embedded-агент: zero network). Создаётся парой через
/// [`channel_pair`]; одна половина — у клиента, другая — у сервиса.
pub struct ChannelTransport {
    tx: mpsc::Sender<RpcMessage>,
    rx: Mutex<mpsc::Receiver<RpcMessage>>,
}

/// Дуплекс-пара: `(a, b)` — `a.send` приходит в `b.recv` и наоборот.
pub fn channel_pair() -> (ChannelTransport, ChannelTransport) {
    let (tx1, rx1) = mpsc::channel(64);
    let (tx2, rx2) = mpsc::channel(64);
    (
        ChannelTransport {
            tx: tx1,
            rx: Mutex::new(rx2),
        },
        ChannelTransport {
            tx: tx2,
            rx: Mutex::new(rx1),
        },
    )
}

#[async_trait]
impl Transport for ChannelTransport {
    async fn send(&self, msg: RpcMessage) -> Result<(), TransportError> {
        self.tx.send(msg).await.map_err(|_| TransportError::Closed)
    }
    async fn recv(&self) -> Option<RpcMessage> {
        self.rx.lock().await.recv().await
    }
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
