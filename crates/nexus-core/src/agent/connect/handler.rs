//! [`ConnectAgentHandler`] — реализация [`ConnectHandler`] поверх [`run_agent_session`] (P0b-2b).
//!
//! Замыкает коннектор: протокол (P0a framing/dispatch) + wire-DTO (P0b-1) + единая композиция
//! (P0b-2a) → РАБОЧИЙ агент-сервис за [`Transport`]. Один хендлер обслуживает несколько сессий; каждый
//! `agent/run` спавнит [`run_agent_session`] и стримит его события клиенту как `agent/event`-нотификации
//! через [`event_notification`] (тот же wire-контракт, что у desktop UI-1b — без расхождения).
//!
//! # Транспорт + асинхронный мост
//! `dispatch` отдаёт ОТВЕТ на запрос через `out`, переданный в вызов; но события прогона текут АСИНХРОННО
//! (цикл живёт в `tokio::spawn`), поэтому хендлер держит СВОЙ `Arc<dyn Transport>` (тот же эндпоинт
//! сервиса) и шлёт в него нотификации. [`AgentEventForwarder`] синхронен (требование цикла); транспорт
//! асинхронен — мостим через ОГРАНИЧЕННЫЙ mpsc + drain-таск (sync `forward`=try_send → канал → `await
//! out.send`; кап не даёт памяти расти при мёртвом drain — анти-leak на отвале клиента).
//!
//! # Сессии и контроль
//! Реестр `session_id → SessionHandle` (run_id + decision-sender + cancel). `agent/approve` кормит
//! [`ChannelDecision`] (человек-в-петле, fail-closed reject_all при закрытии). `agent/control` — ГЛОБАЛЬНАЯ
//! пауза демона (`agent_paused`, тот же kill-switch, что SIGUSR1/agent.json; single-owner). `agent/cancel`
//! — кооперативно (per-run).
//! `agent/undo` — [`actuator::undo_run`] по run_id (идемпотентно). Автономия прогонов — из
//! [`ConnectDeps::autonomy`] (default `confirm` — человек-в-петле; headless-сервер поднимает до `auto`
//! конфигом `ai.agent_autonomy`, owner-gated 2026-06-22).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex};

use crate::actuator::{self, AuditSink, BatchDecision, ChannelDecision, ItemDecision};
use crate::ai::tools::ToolCapableProvider;
use crate::db::{ReadPool, WriteActor};

use super::super::event::AgentEvent;
use super::super::memory::AgentMemory;
use super::super::run_store::{self, STATUS_CANCELLED, STATUS_DONE, STATUS_ERROR};
use super::super::runner::{BudgetKind, LoopOutcome};
use super::super::session::{run_agent_session, AgentEventForwarder, SessionSpec};
use super::super::skill_tools::SkillContext;
use super::super::web_tools::WebToolsConfig;
use super::{
    event_notification, negotiate_version, AgentRunParams, ApproveParams, CancelParams,
    ConnectHandler, ControlParams, InitializeParams, InitializeResult, RpcError, Transport,
    UndoParams, UndoResult,
};

/// Глубина decision-канала сессии (как desktop `DECISION_CHANNEL_CAP`): предложений в прогоне может быть
/// несколько (по одному на changeset-айтем), клиент аппрувит их по очереди.
const DECISION_CAP: usize = 8;

// Глубина канала событий — ЕДИНЫЙ источник `super::EVENT_CHANNEL_CAP` (общий с `sandbox::event`, чтобы
// backpressure обоих путей событий не дрейфовал).
use super::EVENT_CHANNEL_CAP;

/// Композиционные зависимости хендлера (общие на все сессии). Строит композиционный корень (agentd /
/// desktop in-process). `provider` ОБЯЗАТЕЛЕН (без него коннектор не поднимают — в отличие от desktop,
/// где провайдер опционален и прогон деградирует в error).
pub struct ConnectDeps {
    /// tool-capable LLM-провайдер прогонов (тот же `GuardedClient`/`EgressFeature::Chat`, что и chat).
    pub provider: Arc<dyn ToolCapableProvider>,
    /// Память агента (AGENT-MEM-1); `None` → прогоны без recall (без регрессии).
    pub memory: Option<Arc<dyn AgentMemory>>,
    /// Писатель/читатель БД vault (run_store, ledger актуатора).
    pub writer: WriteActor,
    /// Читатель БД vault.
    pub reader: ReadPool,
    /// КАНОНИЗИРОВАННЫЙ корень vault (предусловие гейта/apply + база undo).
    pub canon_root: PathBuf,
    /// **GO-LIVE-флаг актуатора, SAFE BY DEFAULT** (`false` → стабы echo/noop, vault не трогается).
    pub actuator_enabled: bool,
    /// **Автономия прогонов коннектора** (`"confirm"` | `"auto"`), default `"confirm"`
    /// (безопасно для интерактивного десктопа: человек-в-петле — ВСЕ тиры предлагаются). При `"auto"`
    /// (owner-gated 2026-06-22) Auto-тир АВТО-применяется (blast-cap+undo+audit), а Confirm-тир (риск)
    /// по-прежнему ПРЕДЛАГАЕТСЯ по проводу (Proposal) и пишется лишь по явному `agent/approve`
    /// (decision_source — `ChannelDecision`, fail-closed reject_all при дисконнекте). Невалидное значение
    /// конфига нормализуется в `"confirm"` ядром (`DispatchPolicy`: `auto = autonomy == Some("auto")`).
    pub autonomy: String,
    /// Порог «крупной перезаписи» → Confirm-тир (эффект при `actuator_enabled`).
    pub overwrite_threshold: usize,
    /// Кэп blast-radius прогона (эффект при `actuator_enabled`).
    pub blast_cap: u32,
    /// Окно контекста модели (токены) из конфига; `None` → дефолт `ContextBudget`.
    pub context_window: Option<usize>,
    /// Контекст скиллов (SKILL-2); `None` → без меню/инструментов скиллов.
    pub skills: Option<SkillContext>,
    /// **SELF-LEARNING SL-7d, OWNER-GATED, default false** (`ai.skills.learning_enabled`). `true` +
    /// `actuator_enabled` + `skills=Some` → регистрируется `skill.save` (агент авторствует навыки через
    /// гейт). default-OFF: поведение без регрессии.
    pub skills_learning_enabled: bool,
    /// **EGR-AGENT-2: веб-инструменты** (`web.search`/`web.fetch`); `None` → без веба. Эгресс через
    /// `GuardedClient`/`EgressFeature::Web` (composition root собирает через `enable_web_tools`).
    pub web: Option<WebToolsConfig>,
    /// **KILL-SWITCH (AGENT-5): глобальная пауза агента.** ТОТ ЖЕ `Arc`, что у headless `AgentRunHandler`
    /// (agentd: персист `agent.json` + SIGUSR1). Прогоны коннектора честят его → SIGUSR1/agent.json/
    /// `agent/control` останавливают ход мид-ран на границе И блокируют запись актуатора (чек-пойнт #3).
    /// Single-owner-семантика: `agent/control(pause)` коннектора = ГЛОБАЛЬНАЯ пауза демона (один владелец,
    /// один агент); per-session гранулярность — поздний multi-client срез.
    pub agent_paused: Arc<AtomicBool>,
}

/// [`AgentEventForwarder`] → асинхронный [`Transport`]. Синхронный `forward` кладёт событие в ОГРАНИЧЕННЫЙ
/// канал через `try_send` (НИКОГДА не блокирует цикл); drain-таск маппит в `agent/event` и шлёт в транспорт.
struct TransportForwarder {
    tx: mpsc::Sender<AgentEvent>,
}

impl AgentEventForwarder for TransportForwarder {
    fn forward(&self, ev: &AgentEvent) {
        // try_send (НЕ блокирует цикл): канал полон (drain отстал/ушёл — клиент отвалился) ИЛИ закрыт →
        // best-effort дроп. Так память не растёт безгранично при мёртвом drain (анти-leak), а здоровый
        // клиент кап не достигает (drain быстрее эмиссии).
        let _ = self.tx.try_send(ev.clone());
    }
}

/// Активная сессия: адрес её прогона + ручки контроля.
struct SessionHandle {
    /// `id` строки `agent_runs` этого прогона (для валидации approve/cancel/undo по `run_id`).
    run_id: i64,
    /// Sender в [`ChannelDecision`] прогона (кормится `agent/approve`).
    decisions: mpsc::Sender<BatchDecision>,
    /// Кооперативная отмена (`agent/cancel`).
    cancel: Arc<AtomicBool>,
}

/// Агент-сервис за протоколом коннектора. Держит композиционные зависимости + исходящий транспорт (для
/// `agent/event`-нотификаций) + реестр активных сессий.
pub struct ConnectAgentHandler {
    deps: Arc<ConnectDeps>,
    out: Arc<dyn Transport>,
    sessions: Arc<Mutex<HashMap<String, SessionHandle>>>,
}

impl ConnectAgentHandler {
    /// Собирает хендлер из зависимостей + исходящего эндпоинта транспорта (тот же, на котором serve-loop
    /// читает входящие — `dispatch` отвечает в него же).
    pub fn new(deps: Arc<ConnectDeps>, out: Arc<dyn Transport>) -> Self {
        Self {
            deps,
            out,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

/// Маппинг исхода цикла → терминальный статус run_store (как desktop `finish_in_store`: single-spawn,
/// пауза → error, т.к. у коннектора нет scheduler-requeue headless-пути).
fn outcome_to_finish(outcome: &LoopOutcome) -> (&'static str, String) {
    match outcome {
        LoopOutcome::Final(s) => (STATUS_DONE, s.clone()),
        LoopOutcome::BudgetExhausted {
            kind: BudgetKind::Cancelled,
            partial,
        } => (
            STATUS_CANCELLED,
            format!("прогон отменён; частичный ответ: {partial}"),
        ),
        LoopOutcome::BudgetExhausted {
            kind: BudgetKind::Paused,
            partial,
        } => (
            STATUS_ERROR,
            format!("прогон приостановлен (kill-switch); частичный ответ: {partial}"),
        ),
        LoopOutcome::BudgetExhausted { kind, partial } => (
            STATUS_ERROR,
            format!("бюджет исчерпан ({kind:?}); частичный ответ: {partial}"),
        ),
        LoopOutcome::Error(e) => (STATUS_ERROR, e.clone()),
    }
}

#[async_trait]
impl ConnectHandler for ConnectAgentHandler {
    async fn initialize(&self, p: InitializeParams) -> Result<InitializeResult, RpcError> {
        match negotiate_version(&p.supported_versions) {
            Some(v) => Ok(InitializeResult {
                version: v.to_string(),
            }),
            None => Err(RpcError::version_incompatible()),
        }
    }

    async fn agent_run(&self, p: AgentRunParams) -> Result<Value, RpcError> {
        // model_override: протокол его несёт, но коннектор P0b использует СВОЙ сконфигурированный
        // провайдер (single-model embedded-кейс). Per-model выбор — забота композиционного корня (agentd
        // строит провайдеры по моделям) — будущий срез. Логируем расхождение, не молчим.
        if let Some(m) = p.model_override.as_deref() {
            if m != self.deps.provider.model_id() {
                tracing::debug!(
                    requested = m,
                    active = self.deps.provider.model_id(),
                    "agent/run: model_override не применён (коннектор использует сконфигурированный провайдер)"
                );
            }
        }

        // Контроль + decision-источник (человек-в-петле, fail-closed) + регистрация сессии. Пауза —
        // ГЛОБАЛЬНАЯ (`deps.agent_paused`, общая с kill-switch демона), НЕ per-session (single-owner).
        let cancel = Arc::new(AtomicBool::new(false));
        let (decision_source, decision_tx) = ChannelDecision::new(DECISION_CAP);
        let decision_source: Arc<dyn actuator::DecisionSource> = Arc::new(decision_source);

        // ОДИН активный прогон на session_id: реестр держим под локом ЧЕРЕЗ create_run (анти-TOCTOU —
        // иначе два конкурентных agent/run на одну сессию создали бы две строки и перетёрли бы хендл,
        // оставив один прогон неадресуемым для approve/cancel). Параллельные прогоны — РАЗНЫЕ session_id.
        // Повторное использование session_id ПОСЛЕДОВАТЕЛЬНО (после finish сессия снята) — штатно.
        let run_id = {
            let mut sessions = self.sessions.lock().await;
            if sessions.contains_key(&p.session_id) {
                return Err(RpcError::invalid_params()); // сессия уже ведёт активный прогон
            }
            let run_id = run_store::create_run(
                &self.deps.writer,
                &p.prompt,
                Some(self.deps.provider.model_id()),
                Some(self.deps.autonomy.as_str()),
            )
            .await
            .map_err(|e| RpcError::internal(format!("create_run: {e}")))?;
            sessions.insert(
                p.session_id.clone(),
                SessionHandle {
                    run_id,
                    decisions: decision_tx,
                    cancel: cancel.clone(),
                },
            );
            run_id
        };

        // Мост событий: sync forward → ОГРАНИЧЕННЫЙ канал → drain-таск → agent/event в транспорт.
        let (ev_tx, mut ev_rx) = mpsc::channel::<AgentEvent>(EVENT_CHANNEL_CAP);
        let forwarder: Arc<dyn AgentEventForwarder> = Arc::new(TransportForwarder { tx: ev_tx });
        let drain_out = self.out.clone();
        tokio::spawn(async move {
            while let Some(ev) = ev_rx.recv().await {
                if let Some(msg) = event_notification(&ev) {
                    if drain_out.send(msg).await.is_err() {
                        break; // клиент ушёл — прекращаем стрим (цикл сам завершится по своим границам)
                    }
                }
            }
        });

        // Прогон в фоне: ack (`runId`) уходит клиенту СРАЗУ, цикл стримит события асинхронно.
        let deps = self.deps.clone();
        let sessions = self.sessions.clone();
        let session_id = p.session_id.clone();
        let prompt = p.prompt;
        tokio::spawn(async move {
            let spec = SessionSpec {
                run_id,
                task: prompt,
                autonomy: Some(deps.autonomy.clone()),
                actuator_enabled: deps.actuator_enabled,
                overwrite_threshold: deps.overwrite_threshold,
                blast_cap: deps.blast_cap,
                context_window: deps.context_window,
                canon_root: deps.canon_root.clone(),
                skills_learning_enabled: deps.skills_learning_enabled,
            };
            let _ = run_store::mark_running(&deps.writer, run_id).await;
            let outcome = run_agent_session(
                &spec,
                deps.provider.as_ref(),
                deps.memory.as_deref(),
                deps.skills.as_ref(),
                deps.web.as_ref(), // EGR-AGENT-2: веб-инструменты (Some ⇔ ai.web.enabled)
                decision_source,
                &deps.writer,
                &deps.reader,
                &deps.agent_paused, // ГЛОБАЛЬНЫЙ kill-switch (SIGUSR1/agent.json/agent.control)
                &cancel,
                forwarder,
            )
            .await;
            let (status, text) = outcome_to_finish(&outcome);
            let _ = run_store::finish_run(&deps.writer, run_id, status, Some(&text)).await;
            // Дерегистрируем сессию ТОЛЬКО если хендл всё ещё ЭТОТ прогон (guard по run_id) — defense in
            // depth на случай, если сессию переиспользовали (новый прогон не должен быть снят нашим
            // финишем). После снятия approve/cancel вернут «не активна» (идемпотентно).
            let mut s = sessions.lock().await;
            if s.get(&session_id).map(|h| h.run_id) == Some(run_id) {
                s.remove(&session_id);
            }
        });

        Ok(json!({ "runId": run_id.to_string() }))
    }

    async fn agent_undo(&self, p: UndoParams) -> Result<UndoResult, RpcError> {
        let run_id: i64 = p.run_id.parse().map_err(|_| RpcError::invalid_params())?;
        // ledger над тем же writer/reader, что и прогон — undo_run читает executed-строки прогона.
        let ledger = AuditSink::new(self.deps.writer.clone(), self.deps.reader.clone());
        // SL-7d: skills_root для отката строк навыков (skill.save → Snapshot/Trash под skills_root, НЕ
        // vault). None → строки навыка не откатятся (fail-closed), но note/exec идут под canon_root.
        let skills_root = self.deps.skills.as_ref().map(|s| s.skills_root());
        let outcome =
            actuator::undo_run_full(run_id, &self.deps.canon_root, skills_root, &ledger, None)
                .await;
        Ok(UndoResult {
            restored: outcome.restored() as u32,
        })
    }

    async fn agent_cancel(&self, p: CancelParams) -> Result<Value, RpcError> {
        // Взводим cancel-флаг сессии, если адрес совпал. Неактивна/чужой run_id → idempotent no-op.
        let cancel = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(&p.session_id)
                .filter(|h| h.run_id.to_string() == p.run_id)
                .map(|h| h.cancel.clone())
        };
        match cancel {
            Some(flag) => {
                flag.store(true, Ordering::Relaxed);
                Ok(json!({ "cancelled": true }))
            }
            None => Ok(json!({ "cancelled": false })),
        }
    }

    async fn agent_approve(&self, p: ApproveParams) {
        // Клонируем sender и ОТПУСКАЕМ лок ДО await (не держим Mutex через сетевой/канальный await).
        let tx = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(&p.session_id)
                .filter(|h| h.run_id.to_string() == p.run_id)
                .map(|h| h.decisions.clone())
        };
        let Some(tx) = tx else {
            tracing::debug!(
                session = %p.session_id,
                run = %p.run_id,
                "agent/approve: сессия не активна — игнор (idempotent)"
            );
            return;
        };
        let batch = BatchDecision::from_pairs(p.decisions.into_iter().map(|d| {
            (
                d.action_id,
                if d.approved {
                    ItemDecision::Approve
                } else {
                    ItemDecision::Reject
                },
            )
        }));
        // best-effort: канал закрыт (прогон завершился) → решение неактуально.
        let _ = tx.send(batch).await;
    }

    async fn agent_control(&self, p: ControlParams) {
        // ГЛОБАЛЬНАЯ пауза демона (single-owner): ставит ТОТ ЖЕ `agent_paused`, что и SIGUSR1/agent.json
        // headless-AgentRunHandler. Цикл останавливается на границе хода; актуатор не пишет под паузой
        // (чек-пойнт #3). session_id адресный, но эффект глобальный (один владелец, один агент) — per-session
        // гранулярность отложена в multi-client срез. Логируем адресата.
        tracing::info!(session = %p.session_id, pause = p.pause, "agent/control: глобальная пауза агента");
        self.deps.agent_paused.store(p.pause, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tool::{ToolCall, ToolSpec};
    use crate::ai::tools::ToolTurn;
    use crate::ai::{AiResult, ChatMessage};
    use crate::db::Database;
    use crate::net::RunCtx;
    use std::collections::VecDeque;
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;
    use tempfile::TempDir;

    use super::super::{channel_pair, dispatch, RpcMessage};

    /// Фейк tool-провайдер: скриптованная последовательность ходов (offline, как agent_loop_smoke).
    struct FakeProvider {
        turns: StdMutex<VecDeque<AiResult<ToolTurn>>>,
    }
    impl FakeProvider {
        fn new(turns: Vec<AiResult<ToolTurn>>) -> Self {
            Self {
                turns: StdMutex::new(turns.into_iter().collect()),
            }
        }
    }
    #[async_trait]
    impl ToolCapableProvider for FakeProvider {
        async fn stream_chat_tools(
            &self,
            _messages: &[ChatMessage],
            _tools: &[ToolSpec],
            _on_token: &mut (dyn FnMut(String) + Send),
            _cancel: &Arc<AtomicBool>,
            _ctx: RunCtx,
        ) -> AiResult<ToolTurn> {
            self.turns
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(ToolTurn::Final("(no more turns)".into())))
        }
        fn model_id(&self) -> &str {
            "fake"
        }
    }

    async fn open_db() -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join("test.db")).await.unwrap();
        (dir, db)
    }

    fn deps_with(
        provider: Arc<dyn ToolCapableProvider>,
        canon_root: PathBuf,
        db: &Database,
        actuator_enabled: bool,
    ) -> Arc<ConnectDeps> {
        deps_with_autonomy(provider, canon_root, db, actuator_enabled, "confirm")
    }

    fn deps_with_autonomy(
        provider: Arc<dyn ToolCapableProvider>,
        canon_root: PathBuf,
        db: &Database,
        actuator_enabled: bool,
        autonomy: &str,
    ) -> Arc<ConnectDeps> {
        Arc::new(ConnectDeps {
            provider,
            memory: None,
            writer: db.writer().clone(),
            reader: db.reader().clone(),
            canon_root,
            actuator_enabled,
            autonomy: autonomy.to_string(),
            overwrite_threshold: 64 * 1024,
            blast_cap: 16,
            context_window: Some(32768),
            skills: None,
            web: None,
            skills_learning_enabled: false,
            agent_paused: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Поднимает serve-loop над server-эндпоинтом + возвращает client-эндпоинт для отправки запросов.
    fn serve(handler: Arc<ConnectAgentHandler>, server: Arc<super::super::ChannelTransport>) {
        tokio::spawn(async move {
            while let Some(msg) = server.recv().await {
                dispatch(handler.as_ref(), msg, server.as_ref()).await;
            }
        });
    }

    async fn recv_timeout(t: &dyn super::super::Transport) -> RpcMessage {
        tokio::time::timeout(Duration::from_secs(5), t.recv())
            .await
            .expect("recv timeout")
            .expect("transport closed")
    }

    /// E2E offline: initialize → agent/run (echo-стаб) → клиент видит ack{runId} + поток agent/event
    /// (toolCall → toolResult → final). Доказывает, что протокол ДРАЙВИТ реальный цикл и стримит wire-DTO.
    #[tokio::test]
    async fn connect_drives_run_end_to_end_offline() {
        let (client, server) = channel_pair();
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
        let deps = deps_with(provider, _dir.path().to_path_buf(), &db, false);
        let handler = Arc::new(ConnectAgentHandler::new(deps, server.clone()));
        serve(handler, server.clone());

        // initialize
        client
            .send(RpcMessage::request(
                1,
                "initialize",
                json!({"supportedVersions": ["1.0"]}),
            ))
            .await
            .unwrap();
        match recv_timeout(&client).await {
            RpcMessage::Response { id, result } => {
                assert_eq!(id, json!(1));
                assert_eq!(result.unwrap()["version"], "1.0");
            }
            other => panic!("ожидали Response на initialize, получили {other:?}"),
        }

        // agent/run
        client
            .send(RpcMessage::request(
                2,
                "agent/run",
                json!({"sessionId": "s1", "prompt": "сделай эхо"}),
            ))
            .await
            .unwrap();

        let mut run_id = String::new();
        let mut got_toolcall = false;
        let mut got_final = false;
        for _ in 0..60 {
            match recv_timeout(&client).await {
                RpcMessage::Response { id, result } if id == json!(2) => {
                    run_id = result.unwrap()["runId"].as_str().unwrap().to_string();
                }
                RpcMessage::Notification { method, params } if method == "agent/event" => {
                    match params["type"].as_str().unwrap_or("") {
                        "toolCall" => got_toolcall = true,
                        "final" => {
                            got_final = true;
                            break;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        assert!(!run_id.is_empty(), "ack с runId пришёл");
        assert!(got_toolcall, "toolCall застримлен");
        assert!(got_final, "final застримлен");
    }

    /// AGENT-AUTO: `ConnectDeps.autonomy="auto"` → создаваемый прогон несёт `autonomy="auto"` в run-строке
    /// (headless-сервер авто-применяет Auto-тир актуатора). Default ("confirm") покрыт прочими тестами.
    #[tokio::test]
    async fn deps_autonomy_auto_propagates_to_run_row() {
        let (client, server) = channel_pair();
        let server = Arc::new(server);
        let (_dir, db) = open_db().await;
        let provider: Arc<dyn ToolCapableProvider> =
            Arc::new(FakeProvider::new(vec![Ok(ToolTurn::Final("ok".into()))]));
        let deps = deps_with_autonomy(provider, _dir.path().to_path_buf(), &db, false, "auto");
        let handler = Arc::new(ConnectAgentHandler::new(deps, server.clone()));
        serve(handler, server.clone());

        client
            .send(RpcMessage::request(
                1,
                "initialize",
                json!({"supportedVersions": ["1.0"]}),
            ))
            .await
            .unwrap();
        let _ = recv_timeout(&client).await; // init response

        client
            .send(RpcMessage::request(
                2,
                "agent/run",
                json!({"sessionId": "s1", "prompt": "x"}),
            ))
            .await
            .unwrap();
        // ack (Response id=2) приходит ПЕРВЫМ (синхронный возврат), события стримятся после.
        let mut run_id: i64 = -1;
        for _ in 0..30 {
            if let RpcMessage::Response { id, result } = recv_timeout(&client).await {
                if id == json!(2) {
                    run_id = result.unwrap()["runId"].as_str().unwrap().parse().unwrap();
                    break;
                }
            }
        }
        assert!(run_id >= 0, "ack с runId пришёл");
        let run = crate::agent::run_store::get_run(db.reader(), run_id)
            .await
            .unwrap()
            .expect("run-строка существует");
        assert_eq!(
            run.autonomy.as_deref(),
            Some("auto"),
            "autonomy=auto из ConnectDeps проброшен в run-строку"
        );
    }

    /// initialize с несовместимой версией → Response с ошибкой version_incompatible (-32001).
    #[tokio::test]
    async fn initialize_rejects_incompatible_version() {
        let (client, server) = channel_pair();
        let server = Arc::new(server);
        let (_dir, db) = open_db().await;
        let provider: Arc<dyn ToolCapableProvider> =
            Arc::new(FakeProvider::new(vec![Ok(ToolTurn::Final("x".into()))]));
        let deps = deps_with(provider, _dir.path().to_path_buf(), &db, false);
        let handler = Arc::new(ConnectAgentHandler::new(deps, server.clone()));
        serve(handler, server.clone());

        client
            .send(RpcMessage::request(
                7,
                "initialize",
                json!({"supportedVersions": ["9.9"]}),
            ))
            .await
            .unwrap();
        match recv_timeout(&client).await {
            RpcMessage::Response { id, result } => {
                assert_eq!(id, json!(7));
                let err = result.expect_err("ожидали ошибку версии");
                assert_eq!(err.code, -32001);
            }
            other => panic!("ожидали Response, получили {other:?}"),
        }
    }

    /// КЛЮЧЕВОЕ: человек-в-петле ЧЕРЕЗ ПРОВОД. Actuator ВКЛ + note.create → клиент получает `proposal`,
    /// шлёт `agent/approve` (по actionId) → файл реально записан через гейт, прогон done. Доказывает, что
    /// approve работает end-to-end по протоколу (offline-провайдер, реальный temp-vault).
    #[tokio::test]
    async fn approve_over_wire_applies_confirm_item() {
        let (client, server) = channel_pair();
        let server = Arc::new(server);
        let (dir, db) = open_db().await;
        let canon = dir.path().canonicalize().unwrap();
        let provider: Arc<dyn ToolCapableProvider> = Arc::new(FakeProvider::new(vec![
            Ok(ToolTurn::ToolCalls(vec![ToolCall {
                id: "n1".into(),
                name: "note.create".into(),
                arguments: r#"{"path":"Notes/Wire.md","content":"создано по проводу"}"#.into(),
            }])),
            Ok(ToolTurn::Final("готово".into())),
        ]));
        let deps = deps_with(provider, canon.clone(), &db, true); // actuator ВКЛ (temp-vault)
        let handler = Arc::new(ConnectAgentHandler::new(deps, server.clone()));
        serve(handler, server.clone());

        client
            .send(RpcMessage::request(
                1,
                "agent/run",
                json!({"sessionId": "sx", "prompt": "создай заметку"}),
            ))
            .await
            .unwrap();

        let mut run_id = String::new();
        let mut approved = false;
        let mut got_final = false;
        for _ in 0..80 {
            match recv_timeout(&client).await {
                RpcMessage::Response { id, result } if id == json!(1) => {
                    run_id = result.unwrap()["runId"].as_str().unwrap().to_string();
                }
                RpcMessage::Notification { method, params } if method == "agent/event" => {
                    match params["type"].as_str().unwrap_or("") {
                        "proposal" if !approved => {
                            let action_id =
                                params["files"][0]["actionId"].as_i64().expect("actionId");
                            client
                                .send(RpcMessage::notification(
                                    "agent/approve",
                                    json!({
                                        "sessionId": "sx",
                                        "runId": run_id,
                                        "decisions": [{"actionId": action_id, "approved": true}],
                                    }),
                                ))
                                .await
                                .unwrap();
                            approved = true;
                        }
                        "final" => {
                            got_final = true;
                            break;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        assert!(approved, "proposal пришёл и мы отправили approve");
        assert!(got_final, "прогон дошёл до final");
        let written = std::fs::read_to_string(canon.join("Notes/Wire.md")).ok();
        assert_eq!(
            written.as_deref(),
            Some("создано по проводу"),
            "approve по проводу применил note.create через гейт"
        );
    }

    /// AGENT-AUTO keystone: `autonomy="auto"` + actuator ВКЛ → Auto-тир (`note.create`) АВТО-применяется
    /// БЕЗ единого `agent/approve` (в отличие от confirm-пути выше, где тот же `note.create` ждал approve).
    /// Пинит поведение НА ГРАНИЦЕ КОННЕКТОРА. (Что Confirm-тир под auto всё равно НЕ авто-применяется —
    /// покрыто матрицей classify×autonomy в `actuator/orchestrate.rs` + `approve_over_wire_*` выше.)
    #[tokio::test]
    async fn auto_autonomy_applies_auto_tier_without_approve() {
        let (client, server) = channel_pair();
        let server = Arc::new(server);
        let (dir, db) = open_db().await;
        let canon = dir.path().canonicalize().unwrap();
        let provider: Arc<dyn ToolCapableProvider> = Arc::new(FakeProvider::new(vec![
            Ok(ToolTurn::ToolCalls(vec![ToolCall {
                id: "n1".into(),
                name: "note.create".into(),
                arguments: r#"{"path":"Notes/Auto.md","content":"авто-применено"}"#.into(),
            }])),
            Ok(ToolTurn::Final("готово".into())),
        ]));
        // actuator ВКЛ + autonomy=auto (то, что headless-сервер на .28 поставит).
        let deps = deps_with_autonomy(provider, canon.clone(), &db, true, "auto");
        let handler = Arc::new(ConnectAgentHandler::new(deps, server.clone()));
        serve(handler, server.clone());

        client
            .send(RpcMessage::request(
                1,
                "agent/run",
                json!({"sessionId": "sauto", "prompt": "создай заметку"}),
            ))
            .await
            .unwrap();

        let mut got_final = false;
        let mut got_proposal = false;
        for _ in 0..80 {
            match recv_timeout(&client).await {
                RpcMessage::Notification { method, params } if method == "agent/event" => {
                    match params["type"].as_str().unwrap_or("") {
                        "proposal" => got_proposal = true,
                        "final" => {
                            got_final = true;
                            break;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        assert!(got_final, "прогон дошёл до final");
        assert!(
            !got_proposal,
            "Auto-тир под autonomy=auto НЕ предлагается (авто-применён) — ни одного proposal"
        );
        // КЛЮЧЕВОЕ: файл записан БЕЗ единого agent/approve.
        let written = std::fs::read_to_string(canon.join("Notes/Auto.md")).ok();
        assert_eq!(
            written.as_deref(),
            Some("авто-применено"),
            "Auto-тир note.create авто-применён под autonomy=auto БЕЗ approve"
        );
    }

    /// cancel/approve по неизвестной сессии — идемпотентный no-op (не паникует, cancelled:false).
    #[tokio::test]
    async fn cancel_unknown_session_is_idempotent() {
        let (client, server) = channel_pair();
        let server = Arc::new(server);
        let (_dir, db) = open_db().await;
        let provider: Arc<dyn ToolCapableProvider> =
            Arc::new(FakeProvider::new(vec![Ok(ToolTurn::Final("x".into()))]));
        let deps = deps_with(provider, _dir.path().to_path_buf(), &db, false);
        let handler = Arc::new(ConnectAgentHandler::new(deps, server.clone()));
        serve(handler, server.clone());

        client
            .send(RpcMessage::request(
                5,
                "agent/cancel",
                json!({"sessionId": "ghost", "runId": "999"}),
            ))
            .await
            .unwrap();
        match recv_timeout(&client).await {
            RpcMessage::Response { id, result } => {
                assert_eq!(id, json!(5));
                assert_eq!(result.unwrap()["cancelled"], false);
            }
            other => panic!("ожидали Response, получили {other:?}"),
        }
    }

    /// `agent/control(pause)` ставит ГЛОБАЛЬНЫЙ `agent_paused` (тот же kill-switch, что SIGUSR1/agent.json
    /// у headless-AgentRunHandler) — прогоны коннектора честят паузу демона.
    #[tokio::test]
    async fn agent_control_sets_global_pause() {
        let (_c, server) = channel_pair();
        let server = Arc::new(server);
        let (_dir, db) = open_db().await;
        let provider: Arc<dyn ToolCapableProvider> =
            Arc::new(FakeProvider::new(vec![Ok(ToolTurn::Final("x".into()))]));
        let deps = deps_with(provider, _dir.path().to_path_buf(), &db, false);
        let paused = deps.agent_paused.clone();
        let handler = ConnectAgentHandler::new(deps, server);

        assert!(!paused.load(Ordering::Relaxed));
        handler
            .agent_control(ControlParams {
                session_id: "s".into(),
                pause: true,
            })
            .await;
        assert!(
            paused.load(Ordering::Relaxed),
            "control(pause=true) ставит глобальный agent_paused"
        );
        handler
            .agent_control(ControlParams {
                session_id: "s".into(),
                pause: false,
            })
            .await;
        assert!(
            !paused.load(Ordering::Relaxed),
            "control(pause=false) снимает паузу"
        );
    }

    /// Провайдер, чей первый ход ВИСИТ (sleep) — держит прогон активным детерминированно.
    struct SleepyProvider;
    #[async_trait]
    impl ToolCapableProvider for SleepyProvider {
        async fn stream_chat_tools(
            &self,
            _messages: &[ChatMessage],
            _tools: &[ToolSpec],
            _on_token: &mut (dyn FnMut(String) + Send),
            _cancel: &Arc<AtomicBool>,
            _ctx: RunCtx,
        ) -> AiResult<ToolTurn> {
            tokio::time::sleep(Duration::from_millis(250)).await;
            Ok(ToolTurn::Final("done".into()))
        }
        fn model_id(&self) -> &str {
            "sleepy"
        }
    }

    /// Один активный прогон на session_id: пока первый идёт (провайдер висит), второй `agent/run` на ту
    /// же сессию ОТКЛОНЯЕТСЯ (invalid_params) — реестр не перетирается, прогон остаётся адресуемым.
    #[tokio::test]
    async fn second_run_same_session_rejected_while_active() {
        let (_client, server) = channel_pair();
        let server = Arc::new(server);
        let (_dir, db) = open_db().await;
        let provider: Arc<dyn ToolCapableProvider> = Arc::new(SleepyProvider);
        let deps = deps_with(provider, _dir.path().to_path_buf(), &db, false);
        let handler = ConnectAgentHandler::new(deps, server.clone());

        let p1 = AgentRunParams {
            session_id: "dup".into(),
            prompt: "первый".into(),
            model_override: None,
        };
        let p2 = AgentRunParams {
            session_id: "dup".into(),
            prompt: "второй".into(),
            model_override: None,
        };
        let r1 = handler.agent_run(p1).await;
        assert!(r1.is_ok(), "первый прогон стартовал: {r1:?}");
        let r2 = handler.agent_run(p2).await;
        assert!(
            matches!(r2, Err(ref e) if e.code == -32602),
            "второй прогон на активную сессию отклонён invalid_params, получили {r2:?}"
        );
        // Ждём, пока первый завершится и снимет сессию (поллим до ~3 c — устойчиво к нагрузке CI),
        // затем тот же session_id снова свободен для нового прогона.
        let mut r3 = Err(RpcError::invalid_params());
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            r3 = handler
                .agent_run(AgentRunParams {
                    session_id: "dup".into(),
                    prompt: "третий".into(),
                    model_override: None,
                })
                .await;
            if r3.is_ok() {
                break;
            }
        }
        assert!(r3.is_ok(), "после finish session_id снова свободен: {r3:?}");
    }

    /// E2E поверх РЕАЛЬНОГО AF_UNIX-сокета (P0b-2c): `serve_unix_at` биндит сокет, клиент
    /// `connect_unix` шлёт initialize → agent/run, видит ack{runId} + поток agent/event (toolCall→final)
    /// по проводу. Доказывает, что agentd-хостинг коннектора по сокету работает end-to-end.
    #[cfg(unix)]
    #[tokio::test]
    async fn serve_unix_drives_run_over_socket() {
        use crate::agent::connect::{connect_unix, operator_uid, serve_unix_at};

        let (_dir, db) = open_db().await;
        let provider: Arc<dyn ToolCapableProvider> = Arc::new(FakeProvider::new(vec![
            Ok(ToolTurn::ToolCalls(vec![ToolCall {
                id: "c1".into(),
                name: "echo".into(),
                arguments: r#"{"text":"hi"}"#.into(),
            }])),
            Ok(ToolTurn::Final("готово".into())),
        ]));
        let deps = deps_with(provider, _dir.path().to_path_buf(), &db, false);

        // Короткий путь сокета (лимит ~104 симв на macOS) в temp_dir, уникальный по PID.
        let sock = std::env::temp_dir().join(format!("nexus-connect-{}.sock", std::process::id()));
        let sock_for_server = sock.clone();
        // T8-гейт: ожидаемый peer = наш uid (operator_uid). Клиент `connect_unix` ниже — тот же процесс ⇒
        // тот же uid ⇒ пропускается (Linux fail-closed; не-Linux perms-only). Доказывает, что гейт не ломает
        // легитимного оператора end-to-end.
        let expected_uid = operator_uid();
        tokio::spawn(async move {
            let _ = serve_unix_at(&sock_for_server, deps, expected_uid).await;
        });

        // Сервер биндит сокет асинхронно — ждём появления файла (поллинг до ~2 c).
        let client = {
            let mut c = None;
            for _ in 0..40 {
                if let Ok(t) = connect_unix(&sock).await {
                    c = Some(t);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            c.expect("подключение к сокету")
        };

        client
            .send(RpcMessage::request(
                1,
                "initialize",
                json!({"supportedVersions": ["1.0"]}),
            ))
            .await
            .unwrap();
        match recv_timeout(&client).await {
            RpcMessage::Response { id, result } => {
                assert_eq!(id, json!(1));
                assert_eq!(result.unwrap()["version"], "1.0");
            }
            other => panic!("ожидали Response, получили {other:?}"),
        }

        client
            .send(RpcMessage::request(
                2,
                "agent/run",
                json!({"sessionId": "s1", "prompt": "эхо по сокету"}),
            ))
            .await
            .unwrap();

        let mut run_id = String::new();
        let mut got_toolcall = false;
        let mut got_final = false;
        for _ in 0..60 {
            match recv_timeout(&client).await {
                RpcMessage::Response { id, result } if id == json!(2) => {
                    run_id = result.unwrap()["runId"].as_str().unwrap().to_string();
                }
                RpcMessage::Notification { method, params } if method == "agent/event" => {
                    match params["type"].as_str().unwrap_or("") {
                        "toolCall" => got_toolcall = true,
                        "final" => {
                            got_final = true;
                            break;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        let _ = std::fs::remove_file(&sock);
        assert!(!run_id.is_empty(), "ack с runId по сокету");
        assert!(got_toolcall, "toolCall застримлен по сокету");
        assert!(got_final, "final застримлен по сокету");
    }

    // ── LIVE: реальный риг (192.168.0.31:8080). Запуск: `NEXUS_LIVE_CHAT=1 cargo test -p nexus-core \
    //    --lib agent::connect::handler::tests::live -- --ignored --nocapture`. Гейт env-флагом + ignore-атрибутом ──

    /// LIVE tool-loop на риге: реальный OpenAI-tool-провайдер (Qwen3.6-27B на llama.cpp) драйвит цикл
    /// через коннектор; стабы echo/noop (actuator ВЫКЛ — vault не трогается). Ждём, что модель ВЫЗОВЕТ
    /// инструмент (toolCall) и завершит ход (final). Доказывает реальный tool-calling end-to-end.
    #[tokio::test]
    #[ignore = "нужен живой chat-риг (NEXUS_LIVE_CHAT=1, NEXUS_LIVE_CHAT_URL, default 192.168.0.31:8080)"]
    async fn live_connect_tool_loop_on_rig() {
        use crate::ai::tools::OpenAiToolProvider;
        use crate::net::{EgressAudit, EgressFeature, EgressPolicy, GuardedClient};

        if std::env::var("NEXUS_LIVE_CHAT").ok().as_deref() != Some("1") {
            eprintln!("SKIP: NEXUS_LIVE_CHAT!=1");
            return;
        }
        let url = std::env::var("NEXUS_LIVE_CHAT_URL")
            .unwrap_or_else(|_| "http://192.168.0.31:8080".into());
        let model = std::env::var("NEXUS_LIVE_CHAT_MODEL").unwrap_or_else(|_| "qwen".into());

        let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
        let audit = Arc::new(EgressAudit::default());
        let gc = GuardedClient::for_chat(policy, audit, Duration::from_secs(20)).unwrap();
        let provider: Arc<dyn ToolCapableProvider> = Arc::new(OpenAiToolProvider::new(
            &gc,
            EgressFeature::Chat,
            &url,
            &model,
            Some(0.2),
        ));

        let (client, server) = channel_pair();
        let server = Arc::new(server);
        let (_dir, db) = open_db().await;
        let deps = deps_with(provider, _dir.path().to_path_buf(), &db, false); // actuator OFF (echo/noop)
        let handler = Arc::new(ConnectAgentHandler::new(deps, server.clone()));
        serve(handler, server.clone());

        client
            .send(RpcMessage::request(
                1,
                "agent/run",
                json!({
                    "sessionId": "live",
                    "prompt": "Вызови инструмент `echo` с аргументом text=\"привет с рига\", \
                               затем дай короткий финальный ответ.",
                }),
            ))
            .await
            .unwrap();

        // Живой первый токен может занять до ~3 мин (cold-start). Длинный таймаут на ход.
        let mut got_toolcall = false;
        let mut got_final = false;
        for _ in 0..200 {
            let m = tokio::time::timeout(Duration::from_secs(200), client.recv())
                .await
                .expect("live recv timeout")
                .expect("transport closed");
            if let RpcMessage::Notification { method, params } = m {
                if method == "agent/event" {
                    match params["type"].as_str().unwrap_or("") {
                        "toolCall" => {
                            got_toolcall = true;
                            eprintln!("LIVE toolCall: {}", params);
                        }
                        "assistantToken" => { /* стрим контента */ }
                        "final" => {
                            got_final = true;
                            eprintln!("LIVE final: {}", params);
                            break;
                        }
                        "error" => panic!("LIVE error: {params}"),
                        _ => {}
                    }
                }
            }
        }
        assert!(
            got_toolcall,
            "модель вызвала инструмент (real tool-calling)"
        );
        assert!(got_final, "прогон дошёл до final на живой модели");
    }
}
