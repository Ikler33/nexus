//! Транспорт-нейтральные примитивы JSON-RPC 2.0 (R-1, развязка слоёв): конверт/классификация
//! сообщений ([`RpcMessage`]/[`RpcError`]) + подключаемый [`Transport`] (in-process
//! [`ChannelTransport`]/[`channel_pair`]) + line-delimited framing потоковых транспортов
//! ([`framing`], pub(crate)).
//!
//! Вынесено из `agent::connect`: эти типы НЕ знают про агента и методы протокола коннектора — их
//! тянут и коннектор (`crate::agent::connect`), и песочница (`crate::sandbox`: proxy/act/event/
//! exec-цепочки), и внешние крейты (agentd/cli/desktop). Старые пути
//! `agent::connect::{RpcMessage, RpcError, Transport, …}` сохранены реэкспортами — потребители не
//! правятся. Протокол-специфика коннектора (методы `agent/run|undo|…`, params/result-типы,
//! `dispatch`, version-negotiate, event-стрим) остаётся в `agent::connect`. Утроенная логика корреляции
//! запрос→ответ по `id` (Pending-map + счётчик + drain-on-close + timeout) сведена в канон
//! [`correlator::RpcCorrelator`] (R-9): её тянут `connect::client` / `acp::client` / `acp::server`.
//! Sandbox-эгресс (`ProxyGuardedClient`) НЕ коррелятор — синхронный single-in-flight send→recv без
//! Pending-map, вне R-9.
//!
//! NB: `target:` трейсинга оставлен `"agent::connect"` НАМЕРЕННО (R-1 строго behavior-preserving):
//! существующие RUST_LOG-фильтры/наблюдаемость не ломаем; переименование target'ов — отдельное решение.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, Mutex};

pub(crate) mod correlator;
pub(crate) mod framing;

pub(crate) use correlator::RpcCorrelator;

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
    // pub(crate): тесты `agent::connect` (dispatch_notification_no_response и др.) неблокирующе
    // заглядывают в приёмный канал (`try_recv`) — до R-1 поле было приватным в том же модуле.
    pub(crate) rx: Mutex<mpsc::Receiver<RpcMessage>>,
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

/// Делегирующий [`Transport`] для `Arc<T>`: позволяет ДВУМ in-sandbox-шимам делить ОДНО соединение через
/// `Arc`-клоны (SANDBOX-6c-2f: `ProxyActuator` host/act + `ProxyExecDispatcher` host/exec на одном act.sock).
/// Безопасно, т.к. tool-вызовы СЕРИАЛИЗОВАНЫ (`run_agent_loop` — один инструмент за раз) → send/recv одного
/// соединения не пересекаются (контракт «один consumer» соблюдён de-facto последовательностью).
#[async_trait]
impl<T: Transport + ?Sized> Transport for std::sync::Arc<T> {
    async fn send(&self, msg: RpcMessage) -> Result<(), TransportError> {
        (**self).send(msg).await
    }
    async fn recv(&self) -> Option<RpcMessage> {
        (**self).recv().await
    }
}
