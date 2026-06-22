//! `run_sandbox_child_session` — драйвер in-container loop'а песочницы (SANDBOX-4b-2b, спека §2).
//!
//! Контейнер (`nexus-agentd --sandbox-child`) НЕ держит коннектора и НЕ строит host-side гейт — он лишь
//! крутит [`run_agent_loop`] поверх ТРЁХ прокси, замкнутых на host через AF_UNIX (`--network=none`):
//! - **провайдер** — [`ProxyToolProvider`] (egress.sock → host `GuardedProxy` → `GuardedClient`);
//! - **актуатор** — [`ProxyActuator`] как `Arc<dyn ActionDispatcher>` (act.sock → host `dispatch_action`);
//! - **форвардер** — [`ProxyEventForwarder`] + [`drain_events`] (event.sock → host релей в десктоп).
//!
//! Composition-root песочницы: реестр — те же файловые инструменты, что in-process (транспорт-агностичны
//! после SANDBOX-4b-2a), но диспетчер — `ProxyActuator`. Контекст пока МИНИМАЛЬНЫЙ (преамбула + задача);
//! recall памяти / меню скиллов / веб-инструменты в песочнице — последующие срезы (нужен `:ro`-DB-доступ
//! и адаптация web-инструментов под `ProxyGuardedClient`).
//!
//! # Lifecycle (НЕТ control-сокета в контейнер) — сознательное упрощение vs спека §6
//! Спека §6 предполагала 4-й (control) сокет: in-container резидент флипал бы `agent_paused`/`cancel`
//! Arc'и по `agent/control`. Этот срез его СОЗНАТЕЛЬНО опускает — пауза/отмена строго host-side: host
//! паузит/убивает podman (спека §5). Поэтому in-container `cancel`/`paused` — локальные `AtomicBool(false)`
//! (loop их честит, но НИКТО изнутри не взводит — это плейсхолдеры под будущий control.sock ЛИБО просто
//! host-kill-семантику). **Корректность kill-switch при этом НЕ страдает**: запись актуатора остаётся
//! fail-safe под паузой, т.к. host-side `dispatch_action` ПЕРЕЧИТЫВАЕТ `agent_paused` per-step (чек-пойнт
//! #3) — под взведённой host-паузой не применяется ни одной записи, даже если контейнер о ней не знает.
//! Цена упрощения: кооперативная остановка на ГРАНИЦЕ хода (чек-пойнт #2) внутри контейнера не работает —
//! летящий длинный stream прервётся грубо (podman kill), а не на границе. Для каркаса приемлемо.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::actuator::{ActionDispatcher, NoteCreateTool, NoteEditTool, SetFrontmatterTool};
use crate::agent::connect::Transport;
use crate::agent::event::AgentEvent;
use crate::agent::job::AGENT_PREAMBLE;
use crate::agent::registry::ToolRegistry;
use crate::agent::runner::{run_agent_loop, LoopBounds, LoopOutcome};
use crate::agent::AgentEventForwarder;
use crate::ai::{ChatMessage, ContextBudget, QwenTokenizer};
use crate::net::RunCtx;

use super::act::ProxyActuator;
use super::event::{drain_events, ProxyEventForwarder};
use super::provider::ProxyToolProvider;
use super::proxy::ProxyGuardedClient;

/// Плоские параметры песочного прогона (host передаёт их `--sandbox-child` через argv/env). Host-side
/// deps гейта (canon_root/ledger/policy/decision) контейнеру НЕ нужны — их держит host за act.sock.
pub struct SandboxChildSpec {
    /// `id` строки `agent_runs` (корреляция; host штампует его в egress/act аудит — НЕ из контейнера).
    pub run_id: i64,
    /// Задача пользователя (финальное `user`-сообщение начального контекста).
    pub task: String,
    /// База URL LLM (host резолвит реальный хост; контейнер только формирует endpoint-строку).
    pub base_url: String,
    /// Идентификатор модели (для тела запроса + `model_id`).
    pub model: String,
    /// Температура сэмплинга (None → дефолт провайдера).
    pub temperature: Option<f32>,
    /// Окно контекста модели (токены); None → консервативный дефолт [`ContextBudget`].
    pub context_window: Option<usize>,
}

/// Гонит один песочный прогон: собирает прокси-провайдер (egress.sock) + прокси-актуатор (act.sock) +
/// прокси-форвардер (event.sock) и крутит [`run_agent_loop`]. Возвращает [`LoopOutcome`] (host решит, как
/// финализировать run_store — контейнер статус-машину прогона не трогает). Транспорты — generic ради
/// Tier-1-тестируемости (`ChannelTransport` в тестах; `AfUnixTransport` в `--sandbox-child`).
pub async fn run_sandbox_child_session<E, A, V>(
    spec: &SandboxChildSpec,
    egress: E,
    act: A,
    event: V,
) -> LoopOutcome
where
    E: Transport + 'static,
    A: Transport + 'static,
    V: Transport + 'static,
{
    // Провайдер: chat через egress.sock (host GuardedProxy → GuardedClient, chokepoint цел).
    let provider = ProxyToolProvider::new(
        ProxyGuardedClient::new(egress),
        &spec.base_url,
        &spec.model,
        spec.temperature,
    );

    // Актуатор: файловые инструменты через act.sock (host dispatch_action, authoritative). ШОВ
    // SANDBOX-4b-2a: тот же `NoteCreateTool` и пр., но диспетчер — `ProxyActuator`.
    let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(ProxyActuator::new(act));
    let mut registry = ToolRegistry::new();
    registry.insert(Arc::new(NoteCreateTool::new(dispatcher.clone())));
    registry.insert(Arc::new(NoteEditTool::new(dispatcher.clone())));
    registry.insert(Arc::new(SetFrontmatterTool::new(dispatcher)));

    // Форвардер: события хода → event.sock (host релей в десктоп). drain-таск маппит и шлёт.
    let (forwarder, rx) = ProxyEventForwarder::new();
    let drain = tokio::spawn(drain_events(rx, event));

    // Начальный контекст: преамбула + задача (recall/скиллы — последующий срез).
    let messages = vec![
        ChatMessage::system(AGENT_PREAMBLE),
        ChatMessage::user(&spec.task),
    ];

    // Зеркалит in-process дефолт (`run_agent_session`): max_steps=8 / wall_clock=5мин. Для песочницы
    // АВТОРИТЕТНЫЙ wall-clock — host (podman pause/kill через `SandboxRunner`, 4b-2b-2); это лишь
    // in-container backstop. Per-run bounds можно протянуть через `SandboxChildSpec`, когда host-раннер
    // приземлится (4b-2b-2).
    let bounds = LoopBounds::default();
    let budget = ContextBudget::from_context_window(spec.context_window);
    let tk = QwenTokenizer::embedded();
    // Lifecycle host-side (podman pause/kill) → in-container флаги локальные, всегда false.
    let cancel = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(false));

    // Скоупим on_event, чтобы его заём `forwarder` завершился ДО drop(forwarder) ниже.
    let outcome = {
        let mut on_event = |e: AgentEvent| forwarder.forward(&e);
        run_agent_loop(
            &provider,
            &registry,
            messages,
            bounds,
            &budget,
            &tk,
            &cancel,
            &paused,
            RunCtx::run(spec.run_id),
            &mut on_event,
        )
        .await
    };

    // Закрыть форвардер → его `tx` дропается → канал закрыт → drain дойдёт до конца и завершится
    // (все оставшиеся события дренированы в event.sock). Затем дожидаемся drain-таск. ШТАТНЫЙ путь:
    // хвост гарантированно слит (drain.await ждёт recv→None). Под панику `run_agent_loop` (источника
    // паник нет — единственный on_event инфаллибелен) хвост — best-effort (unwind дропнет forwarder,
    // drain дотечёт сам, но рантайм может свернуться раньше). JoinError логируем: drain_events структурно
    // инфаллибелен (только recv/send/event_notification), так что паника тут = регрессия — не глотаем молча.
    drop(forwarder);
    if let Err(e) = drain.await {
        if e.is_panic() {
            tracing::error!(target: "sandbox::event", "drain_events паниковал — событийный поток оборван");
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actuator::{Action, DispatchOutcome};
    use crate::agent::connect::{channel_pair, RpcMessage};
    use crate::agent::ToolError;
    use crate::net::{EgressFeature, NetError, RunCtx};
    use crate::sandbox::act::{ActuatorBackend, HostActServer};
    use crate::sandbox::proxy::{BackendResponse, EgressBackend, EgressBudget, GuardedProxy, Verb};
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Скриптованный мок-LLM-бэкенд egress: 1-й POST → tool_call `note.create`; 2-й → Final.
    struct ScriptedLlm {
        calls: AtomicUsize,
    }
    #[async_trait]
    impl EgressBackend for Arc<ScriptedLlm> {
        async fn fetch(
            &self,
            _verb: Verb,
            _url: &str,
            _feature: EgressFeature,
            _body: Option<&Value>,
            _ctx: RunCtx,
        ) -> Result<BackendResponse, NetError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            let body = if n == 0 {
                // Ход 1: модель зовёт note.create.
                r#"{"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"c1","type":"function","function":{"name":"note.create","arguments":"{\"path\":\"Notes/Sbx.md\",\"content\":\"тело песочницы\"}"}}]}}]}"#
            } else {
                // Ход 2: после tool-результата — финал.
                r#"{"choices":[{"message":{"role":"assistant","content":"готово"}}]}"#
            };
            Ok(BackendResponse {
                status: 200,
                content_type: Some("application/json".into()),
                body: body.as_bytes().to_vec(),
            })
        }
    }

    /// Мок host-актуатора: ловит применённое действие + возвращает Applied (как auto-тир на хосте).
    struct CaptureActuator {
        last: Mutex<Option<Action>>,
    }
    #[async_trait]
    impl ActuatorBackend for Arc<CaptureActuator> {
        async fn act(&self, action: &Action) -> Result<DispatchOutcome, ToolError> {
            *self.last.lock().unwrap() = Some(action.clone());
            Ok(DispatchOutcome::Applied(format!(
                "заметка {} создана",
                action.target.rel()
            )))
        }
    }

    /// СКВОЗНОЙ Tier-1: песочный loop через 3 прокси (mock LLM + capture-актуатор + сбор событий) →
    /// модель зовёт note.create → ProxyActuator → act.sock → host применяет → Final. Доказывает, что
    /// `run_sandbox_child_session` корректно сводит провайдер/актуатор/форвардер в рабочий tool-loop.
    #[tokio::test]
    async fn sandbox_child_drives_tool_loop_over_three_proxies() {
        // 3 пары транспортов: контейнер ↔ host.
        let (egress_c, egress_h) = channel_pair();
        let (act_c, act_h) = channel_pair();
        let (event_c, event_h) = channel_pair();

        // Host egress: GuardedProxy(mock LLM), allow Chat, щедрый бюджет.
        let llm = Arc::new(ScriptedLlm {
            calls: AtomicUsize::new(0),
        });
        let proxy = GuardedProxy::new(
            llm.clone(),
            1,
            vec![EgressFeature::Chat],
            EgressBudget::new(1 << 20, 8),
        );
        let egress_host = tokio::spawn(async move {
            while let Some(msg) = egress_h.recv().await {
                if let RpcMessage::Request { id, method, params } = msg {
                    let result = proxy.handle(&method, params).await;
                    if egress_h
                        .send(RpcMessage::Response { id, result })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });

        // Host act: HostActServer(capture-актуатор).
        let cap = Arc::new(CaptureActuator {
            last: Mutex::new(None),
        });
        let srv = HostActServer::new(cap.clone());
        let act_host = tokio::spawn(async move {
            while let Some(msg) = act_h.recv().await {
                if let RpcMessage::Request { id, method, params } = msg {
                    let result = srv.handle(&method, params).await;
                    if act_h
                        .send(RpcMessage::Response { id, result })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });

        // Host event: собираем релеенные agent/event.
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let ev2 = events.clone();
        let event_host = tokio::spawn(async move {
            while let Some(msg) = event_h.recv().await {
                if let RpcMessage::Notification { method, params } = msg {
                    if method == "agent/event" {
                        if let Some(t) = params.get("type").and_then(|t| t.as_str()) {
                            ev2.lock().unwrap().push(t.to_string());
                        }
                    }
                }
            }
        });

        let spec = SandboxChildSpec {
            run_id: 7,
            task: "создай заметку".into(),
            base_url: "http://llm.local:8080".into(),
            model: "qwen".into(),
            temperature: None,
            context_window: Some(8192),
        };
        let outcome = run_sandbox_child_session(&spec, egress_c, act_c, event_c).await;

        assert!(
            matches!(outcome, LoopOutcome::Final(ref s) if s == "готово"),
            "outcome={outcome:?}"
        );
        // Актуатор применил note.create с корректным путём (через act.sock host-side).
        let applied = cap
            .last
            .lock()
            .unwrap()
            .clone()
            .expect("действие применено");
        assert_eq!(applied.target.rel(), "Notes/Sbx.md");
        // Модель вызвана дважды (tool_call → final).
        assert_eq!(llm.calls.load(Ordering::SeqCst), 2);

        // Дожидаемся, пока host-таски досерверят. Клиентские транспорты дропнуты по завершении прогона
        // (provider/dispatcher/drain-event дропнуты), поэтому recv()→None и все три ГАРАНТИРОВАННО
        // завершаются — вечного await тут нет. Прямой await (без timeout-обёртки) убирает окно
        // недетерминизма: event_host гарантированно опустошил event.sock в `events` ДО ассертов ниже.
        egress_host.await.unwrap();
        act_host.await.unwrap();
        event_host.await.unwrap();

        // События доехали до event.sock: как минимум toolCall + final.
        let got = events.lock().unwrap().clone();
        assert!(got.contains(&"toolCall".to_string()), "события: {got:?}");
        assert!(got.contains(&"final".to_string()), "события: {got:?}");
    }

    /// НЕГАТИВНЫЙ: host закрыл egress.sock мид-прогон (LLM недоступен) → `run_sandbox_child_session`
    /// возвращает `LoopOutcome::Error` за КОНЕЧНОЕ время (не виснет), drain не залипает. Деградация при
    /// смерти host — graceful (ProxyToolProvider→AiError::Http→LoopOutcome::Error, цикл выходит).
    #[tokio::test]
    async fn sandbox_child_errors_when_egress_host_gone() {
        let (egress_c, egress_h) = channel_pair();
        let (act_c, _act_h) = channel_pair();
        let (event_c, event_h) = channel_pair();
        // egress host НЕ обслуживает — сразу закрываем его конец (первый же chat-запрос упрётся в закрытый
        // транспорт). act/event host-концы держим живыми приёмом-в-никуда, чтобы изолировать egress-сбой.
        drop(egress_h);
        let _event_sink = tokio::spawn(async move { while event_h.recv().await.is_some() {} });

        let spec = SandboxChildSpec {
            run_id: 1,
            task: "задача".into(),
            base_url: "http://llm.local:8080".into(),
            model: "qwen".into(),
            temperature: None,
            context_window: Some(4096),
        };
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            run_sandbox_child_session(&spec, egress_c, act_c, event_c),
        )
        .await
        .expect("прогон завершился за конечное время (не завис на мёртвом egress)");
        assert!(
            matches!(outcome, LoopOutcome::Error(_)),
            "мёртвый egress → LoopOutcome::Error, получено {outcome:?}"
        );
    }
}
