//! event — OUTWARD-форвардер событий песочного прогона (SANDBOX-4b, спека §2/§5).
//!
//! Контейнер `--network=none` крутит `run_agent_loop` БЕЗ in-container коннектора; события хода
//! (AssistantToken/ToolCall/ToolResult/Final/...) должны дойти до host (десктоп-коннектор → лента UI).
//! In-sandbox [`ProxyEventForwarder`] (impl [`AgentEventForwarder`]) кладёт СЫРОЙ [`AgentEvent`] в
//! ОГРАНИЧЕННЫЙ канал (`try_send`, НЕ блокирует loop), а [`drain_events`] маппит его в
//! `agent/event`-нотификацию через [`event_notification`] (ТОТ ЖЕ wire-контракт, что у коннектора) и шлёт
//! поверх [`Transport`] (event.sock) → host [`EventForwardServer`] РЕ-ВАЛИДИРУЕТ форму и релеит в исходящий
//! транспорт коннектора (десктоп). Зеркалит `TransportForwarder` коннектора (handler.rs): сырой `AgentEvent`
//! в канале, маппинг — в drain (serde НЕ на горячем пути loop'а; вытесняемые при переполнении события даже
//! не сериализуются).
//!
//! # Почему маппинг через [`event_notification`], а НЕ `to_value(AgentEvent)`
//! `AgentEvent` — `#[serde(tag="type")]` с newtype-вариантами (`AssistantToken(String)`/`Final(String)`),
//! НЕсовместимыми с serde-internal-tag (сериализация роняет их → потеря событий). Поэтому маппинг идёт
//! через struct-вариантный wire-DTO `AgentStreamEvent` (как и у коннектора) — ЕДИНЫЙ источник маппинга.
//!
//! # Направление и доверие
//! Инвертировано относительно [`super::proxy`]/[`super::act`] (там sandbox→host REQUEST/RESPONSE). Тут
//! sandbox→host NOTIFICATION (ответа нет). События — ТОЛЬКО для отображения: authoritative-решения от них
//! не зависят (Proposal/Diff на host порождает host-side `dispatch_action`, Approve/Reject валидируется
//! host-side по реальному ledger). Контейнер НЕДОВЕРЕННЫЙ (мог писать в event.sock мимо
//! [`ProxyEventForwarder`]) → host [`EventForwardServer::serve`] РЕ-ВАЛИДИРУЕТ каждое сообщение: только
//! метод `agent/event` И только если `params` десериализуется в типизированный [`AgentStreamEvent`]
//! (форма на проводе к десктопу = ровно DTO, как у in-process `TransportForwarder` из доверенного enum);
//! чужой метод / кривая форма ДРОПАЮТСЯ (с `tracing::debug!` для наблюдаемости adversarial-поведения).

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::agent::connect::{
    event_notification, AgentStreamEvent, RpcMessage, Transport, EVENT_CHANNEL_CAP,
};
use crate::agent::event::AgentEvent;
use crate::agent::AgentEventForwarder;

/// JSON-RPC метод события хода (зеркалит метод, который эмитит [`event_notification`] коннектора).
pub const EVENT_METHOD: &str = "agent/event";

/// In-sandbox-форвардер: синхронный [`AgentEventForwarder::forward`] кладёт СЫРОЙ [`AgentEvent`] в
/// ОГРАНИЧЕННЫЙ канал через `try_send` (НИКОГДА не блокирует loop); маппинг в wire-нотификацию делает
/// [`drain_events`]. Зеркалит `TransportForwarder` коннектора (handler.rs), но цель — event.sock.
pub struct ProxyEventForwarder {
    tx: mpsc::Sender<AgentEvent>,
}

impl ProxyEventForwarder {
    /// Создаёт форвардер + приёмник для [`drain_events`]. Кап — общий [`EVENT_CHANNEL_CAP`] коннектора.
    pub fn new() -> (Self, mpsc::Receiver<AgentEvent>) {
        Self::with_capacity(EVENT_CHANNEL_CAP)
    }

    /// Как [`ProxyEventForwarder::new`], но с явной ёмкостью (для тестов).
    pub fn with_capacity(cap: usize) -> (Self, mpsc::Receiver<AgentEvent>) {
        let (tx, rx) = mpsc::channel(cap);
        (Self { tx }, rx)
    }
}

impl AgentEventForwarder for ProxyEventForwarder {
    fn forward(&self, ev: &AgentEvent) {
        // try_send (НЕ блокирует loop): канал полон (drain отстал — host/socket залип) ИЛИ закрыт (drain
        // ушёл) → best-effort дроп ХВОСТА (анти-leak: память не растёт при мёртвом drain). Маппинг в wire
        // ОТЛОЖЕН в drain_events (как коннектор) → serde не на горячем пути и не тратится на дроп.
        let _ = self.tx.try_send(ev.clone());
    }
}

/// Дренаж канала [`ProxyEventForwarder`] → [`Transport`] (event.sock): маппит каждый [`AgentEvent`] в
/// `agent/event`-нотификацию ([`event_notification`]) и шлёт. Событие без wire-представления
/// (`non_exhaustive`-задел) → пропускается (как коннектор). Транспорт-сбой (host ушёл) ИЛИ закрытие канала
/// (loop завершился) → выход.
pub async fn drain_events<T: Transport>(mut rx: mpsc::Receiver<AgentEvent>, transport: T) {
    while let Some(ev) = rx.recv().await {
        let Some(msg) = event_notification(&ev) else {
            continue; // событие ядра без wire-DTO — не стримим.
        };
        if transport.send(msg).await.is_err() {
            tracing::debug!(target: "sandbox::event", "event.sock drain: транспорт закрыт — выход");
            break; // host/socket закрыт — дренировать некуда.
        }
    }
}

/// Host-side приёмник event.sock: РЕ-ВАЛИДИРУЕТ форму и релеит `agent/event` в исходящий транспорт
/// коннектора (`out` — тот же эндпоинт, в который коннектор шлёт `agent/event` in-process прогона).
pub struct EventForwardServer {
    out: Arc<dyn Transport>,
}

impl EventForwardServer {
    /// Обернуть исходящий транспорт коннектора (десктоп).
    pub fn new(out: Arc<dyn Transport>) -> Self {
        Self { out }
    }

    /// Serve-loop: читает event.sock до закрытия; релеит в `out` ТОЛЬКО `agent/event`-нотификации, чьи
    /// `params` валидно десериализуются в [`AgentStreamEvent`] (контейнер недоверенный — приводим форму на
    /// проводе к десктопу к классу in-process `TransportForwarder`). Чужой метод / не-нотификация / кривая
    /// форма ДРОПАЮТСЯ (`tracing::debug!`). Сбой `out` (десктоп отвалился) → выход (релеить некуда).
    pub async fn serve<T: Transport>(&self, event_sock: T) {
        while let Some(msg) = event_sock.recv().await {
            // Request/Response на event.sock не ожидаются — контейнер только нотифицирует.
            let RpcMessage::Notification { method, params } = msg else {
                tracing::debug!(target: "sandbox::event", "event.sock: не-нотификация дропнута");
                continue;
            };
            if method != EVENT_METHOD {
                tracing::debug!(target: "sandbox::event", %method, "event.sock: чужой метод дропнут");
                continue;
            }
            // РЕ-ВАЛИДАЦИЯ формы: десериализуем в типизированный DTO (как гарантирует in-process путь через
            // event_notification из доверенного enum). Кривое → дроп, поток жив.
            let stream_ev: AgentStreamEvent = match serde_json::from_value(params) {
                Ok(e) => e,
                Err(_) => {
                    tracing::debug!(target: "sandbox::event", "event.sock: невалидный AgentStreamEvent дропнут");
                    continue;
                }
            };
            // Релеим РЕ-сериализованный валидный DTO → форма на проводе = ровно AgentStreamEvent.
            let params = match serde_json::to_value(&stream_ev) {
                Ok(v) => v,
                Err(_) => continue, // инфаллибельно для DTO; fail-soft.
            };
            if self
                .out
                .send(RpcMessage::notification(EVENT_METHOD, params))
                .await
                .is_err()
            {
                break; // десктоп ушёл — релеить некуда.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::connect::channel_pair;

    /// Снять method+params из RpcMessage-нотификации (для проверок).
    fn as_notification(msg: RpcMessage) -> (String, serde_json::Value) {
        match msg {
            RpcMessage::Notification { method, params } => (method, params),
            other => panic!("ожидалась нотификация, получено {other:?}"),
        }
    }

    /// `forward` кладёт в канал СЫРОЙ `AgentEvent` (маппинг отложен в drain — зеркало коннектора).
    #[test]
    fn forward_enqueues_raw_event() {
        let (fwd, mut rx) = ProxyEventForwarder::with_capacity(4);
        fwd.forward(&AgentEvent::AssistantToken("привет".into()));
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentEvent::AssistantToken("привет".into())
        );
    }

    #[test]
    fn forward_drops_when_channel_full_no_panic() {
        let (fwd, _rx) = ProxyEventForwarder::with_capacity(1);
        // Заполняем (1) + переполняем (дроп) — try_send НЕ паникует и НЕ блокирует.
        fwd.forward(&AgentEvent::AssistantToken("a".into()));
        fwd.forward(&AgentEvent::AssistantToken("b".into())); // дроп (best-effort)
    }

    /// Backpressure-инвариант: при переполнении дропается ХВОСТ, ГОЛОВА сохраняется (FIFO try_send). cap=1,
    /// forward(A) → forward(B) [дроп] → drain → на проводе РОВНО [A].
    #[tokio::test]
    async fn overflow_drops_tail_keeps_head() {
        let (sandbox_t, host_t) = channel_pair();
        let (fwd, rx) = ProxyEventForwarder::with_capacity(1);
        fwd.forward(&AgentEvent::AssistantToken("A".into()));
        fwd.forward(&AgentEvent::AssistantToken("B".into())); // дроп (канал полон)
        drop(fwd);
        drain_events(rx, sandbox_t).await;

        let mut got = Vec::new();
        while let Ok(Some(msg)) =
            tokio::time::timeout(std::time::Duration::from_secs(2), host_t.recv()).await
        {
            got.push(as_notification(msg));
        }
        assert_eq!(got.len(), 1, "хвост (B) дропнут, голова (A) сохранена");
        assert_eq!(got[0].1["type"], "assistantToken");
        assert_eq!(got[0].1["text"], "A");
    }

    /// СКВОЗНОЙ путь: forward (sandbox) → drain → event.sock → serve (host) → десктоп видит все события,
    /// ВКЛЮЧАЯ newtype-варианты (`AssistantToken`/`Final`), которые наивный `to_value(AgentEvent)` терял.
    #[tokio::test]
    async fn end_to_end_sandbox_to_desktop_relay() {
        let (sandbox_t, host_event_sock) = channel_pair(); // event.sock: sandbox↔host
        let (desktop_out, desktop_in) = channel_pair(); // host.out↔десктоп

        let out: Arc<dyn Transport> = Arc::new(desktop_out);
        let srv = EventForwardServer::new(out);
        let host = tokio::spawn(async move {
            srv.serve(host_event_sock).await;
        });

        // Sandbox: 3 события (вкл. newtype AssistantToken + Final), дренируем, затем дропаем форвардер.
        let (fwd, rx) = ProxyEventForwarder::with_capacity(8);
        fwd.forward(&AgentEvent::AssistantToken("раз".into()));
        fwd.forward(&AgentEvent::ToolCall {
            id: "c1".into(),
            kind: "note.create".into(),
            args: "{}".into(),
        });
        fwd.forward(&AgentEvent::Final("готово".into()));
        drop(fwd); // закрыть канал → drain дойдёт до конца и завершится.
        drain_events(rx, sandbox_t).await; // дренирует всё в host_event_sock; затем sandbox_t дропается.

        // Десктоп должен принять РОВНО 3 `agent/event` (порядок сохранён).
        let mut got = Vec::new();
        while let Ok(Some(msg)) =
            tokio::time::timeout(std::time::Duration::from_secs(2), desktop_in.recv()).await
        {
            got.push(as_notification(msg));
        }
        host.await.unwrap();

        assert_eq!(got.len(), 3, "все 3 события дошли до десктопа");
        assert_eq!(got[0].1["type"], "assistantToken");
        assert_eq!(got[0].1["text"], "раз");
        assert_eq!(got[1].1["type"], "toolCall");
        assert_eq!(got[2].1["type"], "final");
        assert_eq!(got[2].1["text"], "готово");
    }

    /// Host релеит ТОЛЬКО `agent/event`: чужой метод на event.sock игнорируется (контейнер не диктует).
    #[tokio::test]
    async fn server_ignores_foreign_method() {
        let (sandbox_t, host_event_sock) = channel_pair();
        let (desktop_out, desktop_in) = channel_pair();
        let out: Arc<dyn Transport> = Arc::new(desktop_out);
        let srv = EventForwardServer::new(out);
        let host = tokio::spawn(async move {
            srv.serve(host_event_sock).await;
        });

        // Чужая нотификация + затем валидное событие: первая дропается, второе релеится.
        sandbox_t
            .send(RpcMessage::notification(
                "host/act",
                serde_json::json!({"evil": true}),
            ))
            .await
            .unwrap();
        sandbox_t
            .send(event_notification(&AgentEvent::Final("ок".into())).unwrap())
            .await
            .unwrap();
        drop(sandbox_t);

        let mut got = Vec::new();
        while let Ok(Some(msg)) =
            tokio::time::timeout(std::time::Duration::from_secs(2), desktop_in.recv()).await
        {
            got.push(as_notification(msg));
        }
        host.await.unwrap();

        assert_eq!(got.len(), 1, "чужой метод НЕ релеится; только agent/event");
        assert_eq!(got[0].1["type"], "final");
    }

    /// S1: host ДРОПАЕТ `agent/event` с params, не парсящимися в `AgentStreamEvent` (контейнер слал
    /// произвольный JSON под видом события мимо ProxyEventForwarder) — на десктоп уходит только валидный DTO.
    #[tokio::test]
    async fn server_drops_malformed_agent_event() {
        let (sandbox_t, host_event_sock) = channel_pair();
        let (desktop_out, desktop_in) = channel_pair();
        let out: Arc<dyn Transport> = Arc::new(desktop_out);
        let srv = EventForwardServer::new(out);
        let host = tokio::spawn(async move {
            srv.serve(host_event_sock).await;
        });

        // Метод верный, но форма не AgentStreamEvent (нет/неизвестный type) → дроп. Затем валидное → релей.
        sandbox_t
            .send(RpcMessage::notification(
                EVENT_METHOD,
                serde_json::json!({"type": "bogusVariant", "x": 1}),
            ))
            .await
            .unwrap();
        sandbox_t
            .send(RpcMessage::notification(
                EVENT_METHOD,
                serde_json::json!({"not": "an event"}),
            ))
            .await
            .unwrap();
        sandbox_t
            .send(event_notification(&AgentEvent::Final("валидное".into())).unwrap())
            .await
            .unwrap();
        drop(sandbox_t);

        let mut got = Vec::new();
        while let Ok(Some(msg)) =
            tokio::time::timeout(std::time::Duration::from_secs(2), desktop_in.recv()).await
        {
            got.push(as_notification(msg));
        }
        host.await.unwrap();

        assert_eq!(
            got.len(),
            1,
            "оба кривых дропнуты; релеится только валидный DTO"
        );
        assert_eq!(got[0].1["type"], "final");
        assert_eq!(got[0].1["text"], "валидное");
    }
}
