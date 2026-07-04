//! ACP-2 — ACP **СЕРВЕР** поверх stdio: ИНВЕРСИЯ [`super::client`]. Внешний ACP-клиент (Zed/JetBrains
//! или наш [`super::AcpClient`]) спавнит `nexus acp` и драйвит ПРОГОН Castor по line-delimited
//! JSON-RPC 2.0. Спека — `docs/specs/acp-server.md`. Транспорт-агностичен: [`serve_acp`] берёт
//! `Arc<dyn Transport>` (юнит-тесты гоняют по `channel_pair`; CLI оборачивает реальные stdin/stdout в
//! [`StdinStdoutTransport`]).
//!
//! # Безопасность (SAFE BY DEFAULT, fail-closed)
//! - Актуатор ВЫКЛ по умолчанию (`actuator_enabled=false`) → [`run_agent_session`] НЕ ставит
//!   инструментов записи (реестр пуст, если не подключены read-only skills/web — B7), vault не
//!   пишется, [`AcpServerDecisionSource::decide`] не зовётся. Единственный эффект
//!   дефолтного `nexus acp` — строка `agent_runs`.
//! - Автономия по умолчанию `confirm`: каждый Confirm-тир (и Auto за blast-cap) идёт в клиент как
//!   `session/request_permission`. Любой сбой решения (send-fail/таймаут/EOF/cancel/Cancelled/неизвестная
//!   опция/parse-miss) → `reject_all` (запись НЕ применяется). `--auto` авто-применяет ЛИШЬ Auto-тир.
//!
//! # Архитектура (зеркало `handler.rs`)
//! Read-loop ([`serve_acp`]) классифицирует входящие и НЕ блокируется: `session/prompt` идёт в
//! СПАВНЕННУЮ drive-задачу, которая САМА отвечает на prompt-id ПОСЛЕ завершения стрима+permission — так
//! `session/cancel` и client-`Response` (ответы на наши permission-запросы) текут конкурентно (out-of-
//! order keystone). События цикла мостятся sync-форвардером → ограниченный mpsc → drain-таск →
//! [`map_event_to_acp`] → `session/update`. Весь outbound — через `serde_json::json!`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{BufReader, Stdin, Stdout};
use tokio::sync::{mpsc, Mutex};

use crate::actuator::{BatchDecision, DecisionSource, ItemDecision, ProposalBatch};
use crate::ai::tools::ToolCapableProvider;
use crate::ai::ChatMessage;
use crate::db::{ReadPool, WriteActor};
use crate::rpc::RpcCorrelator;

use super::super::super::event::AgentEvent;
use super::super::super::finish::{outcome_to_finish, CancelWording, PausePolicy};
use super::super::super::run_store;
use super::super::super::runner::{BudgetKind, LoopOutcome};
use super::super::super::session::{
    run_agent_session, AgentEventForwarder, SessionDeps, SessionRole, SessionSpec,
};
use super::super::{
    acp_tool_kind, framing, RpcError, RpcMessage, Transport, TransportError, EVENT_CHANNEL_CAP,
};
use super::ACP_PROTOCOL_VERSION;

/// Кап задачи одного `session/prompt` (анти-DoS): >256 KiB → invalid_params.
const MAX_PROMPT_BYTES: usize = 256 * 1024;
/// Таймаут ожидания решения клиента по `session/request_permission` → fail-closed reject_all.
const PERMISSION_TIMEOUT: Duration = Duration::from_secs(300);
/// Кэп истории мультитёрна (сообщений) — как `nexus agent` REPL / desktop-окно.
const HISTORY_MAX_MSGS: usize = 16;
/// База id наших исходящих permission-запросов: с большим оффсетом, чтобы НИКОГДА не пересечься с
/// собственными id клиента (тот стартует с 1; направления и так раздельны — belt-and-suspenders).
const PERM_ID_BASE: i64 = 1_000_000_000;
/// Кап текста в title/контенте session/update (не льём простыни на провод).
const CLIP_CHARS: usize = 200;

/// Композиционные зависимости ACP-сервера (общие на соединение). Группа (не позиционные аргументы —
/// `clippy::too_many_arguments`). Slice-1: НЕТ memory/skills/skills_learning/web/delegation/research
/// (все OFF — минимизируем blast-radius; задокументированные deferrals в `acp-server.md`).
pub struct AcpServerConfig {
    /// tool-capable LLM-провайдер прогонов (тот же `GuardedClient`/`EgressFeature::Chat`, что у chat).
    pub provider: Arc<dyn ToolCapableProvider>,
    /// Писатель БД vault (run_store + ledger актуатора).
    pub writer: WriteActor,
    /// Читатель БД vault.
    pub reader: ReadPool,
    /// КАНОНИЗИРОВАННЫЙ корень vault (предусловие гейта/apply; client-cwd НЕ репойнтит его — R7).
    pub canon_root: PathBuf,
    /// **GO-LIVE-флаг актуатора, SAFE BY DEFAULT** (`false` → без инструментов записи, vault не трогается).
    pub actuator_enabled: bool,
    /// Автономия прогона (`"confirm"`|`"auto"`), default `"confirm"`. Эффект только при `actuator_enabled`.
    pub autonomy: String,
    /// Порог «крупной перезаписи» → Confirm-тир.
    pub overwrite_threshold: usize,
    /// Кэп blast-radius прогона.
    pub blast_cap: u32,
    /// Окно контекста модели (токены) из конфига; `None` → дефолт `ContextBudget`.
    pub context_window: Option<usize>,
    /// Имя модели (для строки `agent_runs`).
    pub model: String,
}

/// Состояние ОДНОЙ сессии соединения (R1: одна сессия на коннект). Мультитёрн с аккумулируемой историей
/// (cap 16, только успешные ходы); один активный ход (`active`); кооперативный `cancel`.
struct AcpSession {
    id: String,
    history: Vec<ChatMessage>,
    /// client-cwd из `session/new` — ЛОГИРУЕТСЯ, НЕ репойнтит vault (R7).
    cwd: PathBuf,
    cancel: Arc<AtomicBool>,
    /// Идёт ли сейчас ход (R2: второй конкурентный prompt → invalid_params).
    active: bool,
}

/// Значение, доставляемое ждущему permission-запросу: разобранный client-`Response.result`
/// (Ok/Err), Cancelled при отмене или provaл при закрытии транспорта.
type PermReply = Result<Value, RpcError>;

/// Разделяемое состояние сервера. `session` — `Option` (одна сессия/коннект). `perm_pending` — канон
/// [`RpcCorrelator`] наших исходящих permission-id (read-loop резолвит при client-`Response`; decide()
/// ждёт; id-счётчик perm инкапсулирован в корреляторе, стартует с `PERM_ID_BASE`).
struct AcpServerState {
    cfg: Arc<AcpServerConfig>,
    transport: Arc<dyn Transport>,
    session: Mutex<Option<AcpSession>>,
    perm_pending: Arc<RpcCorrelator<PermReply>>,
    next_session: AtomicU64,
}

/// Поднимает read-loop ACP-сервера над `transport`, обслуживает ОДНУ сессию до EOF. На закрытие
/// транспорта (родитель закрыл stdin): проваливает все ждущие permission (decide() → reject_all),
/// взводит cancel сессии, возвращается — без зависов.
pub async fn serve_acp(transport: Arc<dyn Transport>, cfg: Arc<AcpServerConfig>) {
    let state = Arc::new(AcpServerState {
        cfg,
        transport: transport.clone(),
        session: Mutex::new(None),
        // perm-id стартует с большого оффсета PERM_ID_BASE (никогда не пересечься с id клиента).
        perm_pending: Arc::new(RpcCorrelator::new(PERM_ID_BASE)),
        next_session: AtomicU64::new(1),
    });

    while let Some(msg) = transport.recv().await {
        handle_message(state.clone(), msg).await;
    }

    // EOF: провалить все ждущие permission → decide() резолвится reject_all (fail-closed), взвести cancel.
    state
        .perm_pending
        .fail_all(Err(RpcError::internal("acp-server transport closed")))
        .await;
    if let Some(s) = state.session.lock().await.as_ref() {
        s.cancel.store(true, Ordering::Relaxed);
    }
    tracing::debug!(target: "agent::connect::acp::server", "acp-server: stdin EOF — выход");
}

/// Классифицирует ОДНО входящее сообщение. Запросы → Response (fail-closed); `session/prompt` — в
/// спавненную drive-задачу (loop НЕ блокируется); уведомления — без ответа; client-`Response` → роутинг
/// в `perm_pending`.
async fn handle_message(state: Arc<AcpServerState>, msg: RpcMessage) {
    match msg {
        RpcMessage::Request { id, method, params } => match method.as_str() {
            "initialize" => {
                reply(&state.transport, id, handle_initialize(params)).await;
            }
            "session/new" => {
                let r = handle_new_session(&state, params).await;
                reply(&state.transport, id, r).await;
            }
            "session/prompt" => {
                // НЕ отвечаем здесь: drive-задача ответит на `id` сама ПОСЛЕ стрима+permission.
                handle_prompt(state.clone(), id, params).await;
            }
            // Некоторые клиенты шлют cancel как REQUEST — обрабатываем и подтверждаем пустым Ok.
            "session/cancel" => {
                cancel_session(&state, params).await;
                reply(&state.transport, id, Ok(json!({}))).await;
            }
            // fs/*, terminal/*, session/load, session/fork, session/set_mode, прочее → method_not_found.
            _ => {
                reply(&state.transport, id, Err(RpcError::method_not_found())).await;
            }
        },
        RpcMessage::Notification { method, params } => {
            // Только session/cancel значим; прочие уведомления — тихо игнор (JSON-RPC: ответа нет).
            if method == "session/cancel" {
                cancel_session(&state, params).await;
            }
        }
        // Ответы клиента на НАШИ session/request_permission — роутинг в ждущий oneshot.
        RpcMessage::Response { id, result } => route_response(&state, id, result).await,
    }
}

/// Отвечает на запрос по `id` (sanitized; ошибка отправки — лог, без паники).
async fn reply(transport: &Arc<dyn Transport>, id: Value, result: Result<Value, RpcError>) {
    let _ = transport.send(RpcMessage::Response { id, result }).await;
}

/// `initialize`: парсим (лениво — лишь форма object), ОБЪЯВЛЯЕМ свою версию (=1) независимо от запрошенной
/// клиентом (ACP-конвенция). fs/terminal-caps клиента игнорируем (мы не зовём fs/* и terminal/*).
fn handle_initialize(params: Value) -> Result<Value, RpcError> {
    if !params.is_object() {
        return Err(RpcError::invalid_params());
    }
    Ok(json!({ "protocolVersion": ACP_PROTOCOL_VERSION }))
}

/// `session/new`: одна сессия на коннект (R1 — вторая → invalid_params). `cwd` логируется, vault НЕ
/// репойнтится (R7); `mcp_servers` игнорируются (логируются, если непусты).
async fn handle_new_session(state: &Arc<AcpServerState>, params: Value) -> Result<Value, RpcError> {
    let p: super::schema::NewSessionParams =
        serde_json::from_value(params).map_err(|_| RpcError::invalid_params())?;
    let mut slot = state.session.lock().await;
    if slot.is_some() {
        return Err(RpcError::invalid_params()); // R1: одна сессия на соединение
    }
    if !p.mcp_servers.is_empty() {
        tracing::info!(
            target: "agent::connect::acp::server",
            n = p.mcp_servers.len(),
            "session/new: mcpServers проигнорированы (slice-1)"
        );
    }
    let id = format!("s{}", state.next_session.fetch_add(1, Ordering::Relaxed));
    tracing::info!(
        target: "agent::connect::acp::server",
        session = %id,
        client_cwd = %p.cwd.display(),
        vault = %state.cfg.canon_root.display(),
        "session/new (vault фиксирован --vault; client cwd проигнорирован)"
    );
    *slot = Some(AcpSession {
        id: id.clone(),
        history: Vec::new(),
        cwd: p.cwd,
        cancel: Arc::new(AtomicBool::new(false)),
        active: false,
    });
    Ok(json!({ "sessionId": id }))
}

/// `session/prompt`: валидирует/собирает задачу, CAS active false→true (R2), сбрасывает cancel, КЛОНИРУЕТ
/// историю+cancel из-под лока и СПАВНИТ drive-задачу. Drive сама ответит на `prompt_id`. Ошибки валидации
/// → немедленный Response(err) на `prompt_id` (active не взводился).
async fn handle_prompt(state: Arc<AcpServerState>, prompt_id: Value, params: Value) {
    let p: super::schema::PromptParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(_) => {
            reply(&state.transport, prompt_id, Err(RpcError::invalid_params())).await;
            return;
        }
    };

    // Текст хода из Text-блоков (Other/image/audio игнорируем).
    let task = concat_prompt_text(&p.prompt);
    if task.trim().is_empty() {
        reply(&state.transport, prompt_id, Err(RpcError::invalid_params())).await;
        return;
    }
    if task.len() > MAX_PROMPT_BYTES {
        reply(&state.transport, prompt_id, Err(RpcError::invalid_params())).await;
        return;
    }

    // Под локом: совпадение session_id, CAS active, сброс cancel, клон (history, cancel, session_id).
    let (history, cancel, session_id) = {
        let mut slot = state.session.lock().await;
        let Some(sess) = slot.as_mut() else {
            // нет сессии (prompt до session/new) → invalid_params.
            reply(&state.transport, prompt_id, Err(RpcError::invalid_params())).await;
            return;
        };
        if sess.id != p.session_id {
            reply(&state.transport, prompt_id, Err(RpcError::invalid_params())).await;
            return;
        }
        if sess.active {
            reply(&state.transport, prompt_id, Err(RpcError::invalid_params())).await; // R2
            return;
        }
        sess.active = true;
        sess.cancel.store(false, Ordering::Relaxed); // мультитёрн: прошлый cancel не травит новый ход
        let _ = &sess.cwd; // cwd ЛОГИРУЕТСЯ в session/new, не репойнтит vault (R7)
        (sess.history.clone(), sess.cancel.clone(), sess.id.clone())
    };

    tokio::spawn(drive_prompt(
        state, prompt_id, session_id, task, history, cancel,
    ));
}

/// СПАВНЕННЫЙ ход: create_run → run_agent_session (AcpForwarder + AcpServerDecisionSource) → finish_run →
/// Response(prompt_id, {stopReason}). На успешном Final аккумулирует историю (cap 16); снимает active.
async fn drive_prompt(
    state: Arc<AcpServerState>,
    prompt_id: Value,
    session_id: String,
    task: String,
    history: Vec<ChatMessage>,
    cancel: Arc<AtomicBool>,
) {
    let cfg = &state.cfg;

    let run_id = match run_store::create_run(
        &cfg.writer,
        &task,
        Some(&cfg.model),
        Some(cfg.autonomy.as_str()),
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            clear_active(&state).await;
            reply(
                &state.transport,
                prompt_id,
                Err(RpcError::internal(format!("create_run: {e}"))),
            )
            .await;
            return;
        }
    };

    // Мост событий: sync forward → ОГРАНИЧЕННЫЙ канал → drain-таск → map_event_to_acp → session/update.
    let (ev_tx, mut ev_rx) = mpsc::channel::<AgentEvent>(EVENT_CHANNEL_CAP);
    let forwarder: Arc<dyn AgentEventForwarder> = Arc::new(AcpForwarder { tx: ev_tx });
    let drain_transport = state.transport.clone();
    let drain_session = session_id.clone();
    let drain = tokio::spawn(async move {
        while let Some(ev) = ev_rx.recv().await {
            for msg in map_event_to_acp(&drain_session, &ev) {
                if drain_transport.send(msg).await.is_err() {
                    return; // клиент ушёл — прекращаем стрим
                }
            }
        }
    });

    // Источник решений: НАШ AcpServerDecisionSource (sole-place записи) — шлёт request_permission клиенту.
    let decision: Arc<dyn DecisionSource> = Arc::new(AcpServerDecisionSource {
        transport: state.transport.clone(),
        session_id: session_id.clone(),
        run_id,
        perm_pending: state.perm_pending.clone(),
        cancel: cancel.clone(),
        timeout: PERMISSION_TIMEOUT,
    });

    let spec = SessionSpec {
        run_id,
        task: task.clone(),
        history,
        autonomy: Some(cfg.autonomy.clone()),
        actuator_enabled: cfg.actuator_enabled,
        overwrite_threshold: cfg.overwrite_threshold,
        blast_cap: cfg.blast_cap,
        context_window: cfg.context_window,
        canon_root: cfg.canon_root.clone(),
        skills_learning_enabled: false,
    };
    let paused = Arc::new(AtomicBool::new(false));
    let _ = run_store::mark_running(&cfg.writer, run_id).await;

    let outcome = run_agent_session(
        &spec,
        &SessionDeps {
            provider: cfg.provider.as_ref(),
            memory: None, // slice-1
            skills: None, // slice-1
            web: None,    // slice-1
            decision_source: decision,
            writer: &cfg.writer,
            reader: &cfg.reader,
            paused: &paused,
            cancel: &cancel,
            forwarder,
        },
        SessionRole::TopLevel {
            delegation: None, // slice-1
            research: None,   // slice-1
        },
    )
    .await;

    // Дренаж событий завершится сам по закрытию ev_tx (move в run_agent_session forwarder уже дропнут).
    let _ = drain.await;

    // Терминал по канону R-2 (зеркало handler: single-spawn, пауза → error — у ACP-сервера нет
    // scheduler-requeue-пути возобновления).
    let (status, text) = outcome_to_finish(
        &outcome,
        PausePolicy::FinalizeError,
        CancelWording::RunCancelled,
    )
    .expect_finalize();
    let _ = run_store::finish_run(&cfg.writer, run_id, status, Some(&text)).await;

    // Мультитёрн: историю копим ТОЛЬКО на успешном Final (провальные ходы не травят контекст).
    if let LoopOutcome::Final(answer) = &outcome {
        let mut slot = state.session.lock().await;
        if let Some(sess) = slot.as_mut().filter(|s| s.id == session_id) {
            sess.history.push(ChatMessage::user(&task));
            sess.history.push(ChatMessage::assistant(answer));
            if sess.history.len() > HISTORY_MAX_MSGS {
                let drop = sess.history.len() - HISTORY_MAX_MSGS;
                sess.history.drain(0..drop);
            }
        }
    }

    clear_active(&state).await;
    let stop = stopreason_from_outcome(&outcome);
    reply(
        &state.transport,
        prompt_id,
        Ok(json!({ "stopReason": stop })),
    )
    .await;
}

/// Снимает `active` текущей сессии (по завершении хода — следующий prompt разрешён).
async fn clear_active(state: &Arc<AcpServerState>) {
    if let Some(sess) = state.session.lock().await.as_mut() {
        sess.active = false;
    }
}

/// `session/cancel`: взводит кооперативный cancel сессии (если session_id совпал) + проваливает ждущие
/// permission → Cancelled. Неизвестная/несовпавшая сессия → тихий no-op.
async fn cancel_session(state: &Arc<AcpServerState>, params: Value) {
    let Ok(p) = serde_json::from_value::<super::schema::CancelParams>(params) else {
        return;
    };
    let matched = {
        let slot = state.session.lock().await;
        match slot.as_ref() {
            Some(s) if s.id == p.session_id => {
                s.cancel.store(true, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    };
    if matched {
        // Проваливаем ждущие permission → decide() резолвится reject_all (отмена не одобряет запись).
        // Cancelled — валидное значение доставки: outcome_to_batch_decision → reject_all (fail-closed).
        state
            .perm_pending
            .fail_all(Ok(json!({ "outcome": { "outcome": "cancelled" } })))
            .await;
    }
}

/// Роутинг client-`Response` (ответ на НАШ session/request_permission) в ждущий oneshot по i64-id.
/// Неизвестный id (поздний/дубль) — тихо отбрасываем (внутри [`RpcCorrelator::resolve`]).
async fn route_response(state: &Arc<AcpServerState>, id: Value, result: Result<Value, RpcError>) {
    let Some(i) = id.as_i64() else { return };
    state.perm_pending.resolve(i, result).await;
}

/// Собирает текст задачи из `Text`-блоков (join `\n`); Other/image/audio игнорирует.
fn concat_prompt_text(prompt: &[super::schema::ContentBlock]) -> String {
    prompt
        .iter()
        .filter_map(|b| match b {
            super::schema::ContentBlock::Text { text } => Some(text.as_str()),
            super::schema::ContentBlock::Other => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ───────────────────────── Маппинг событий цикла → ACP session/update ─────────────────────────

/// Sync-форвардер событий прогона → ОГРАНИЧЕННЫЙ mpsc (try_send, никогда не блокирует цикл; дроп при
/// переполнении — best-effort, как `handler::TransportForwarder`).
struct AcpForwarder {
    tx: mpsc::Sender<AgentEvent>,
}

impl AgentEventForwarder for AcpForwarder {
    fn forward(&self, ev: &AgentEvent) {
        let _ = self.tx.try_send(ev.clone());
    }
}

/// УТФ-8-безопасная обрезка до `n` символов с многоточием.
fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n).collect::<String>())
    } else {
        s.to_string()
    }
}

/// Наш dotted tool-kind → ACP `ToolKind`-строка (для session/update tool_call). Через `acp_tool_kind`
/// (write→"edit", read→"read", search→"search", else→"other").
fn acp_tool_kind_str(nexus_kind: &str) -> &'static str {
    match acp_tool_kind(nexus_kind) {
        "write" => "edit",
        "read" => "read",
        "search" => "search",
        _ => "other",
    }
}

/// ACP-статус шага плана из нашего `PlanStepState` (Failed→completed: ACP-план не несёт failed).
fn acp_plan_status(s: &crate::agent::event::PlanStepState) -> &'static str {
    use crate::agent::event::PlanStepState::*;
    match s {
        Pending => "pending",
        Running => "in_progress",
        Done | Failed => "completed",
    }
}

/// Чистый маппинг ОДНОГО события цикла → ноль-или-более `session/update`-уведомлений. ИСЧЕРПЫВАЮЩИЙ match
/// + `_ => vec![]` (AgentEvent — `#[non_exhaustive]`). Outbound — через `json!` (без Serialize).
fn map_event_to_acp(session_id: &str, ev: &AgentEvent) -> Vec<RpcMessage> {
    // ACP-спека: params = {sessionId, update:{sessionUpdate,…}} — `update` ВЛОЖЕН (не flatten).
    // Реальный клиент (Zed/JetBrains) и наш AcpClient ждут именно эту форму; плоская молча не парсится.
    let upd = |body: Value| {
        vec![RpcMessage::notification(
            "session/update",
            json!({ "sessionId": session_id, "update": body }),
        )]
    };
    match ev {
        AgentEvent::AssistantToken(s) => upd(json!({
            "sessionUpdate": "agent_message_chunk",
            "content": { "type": "text", "text": s }
        })),
        AgentEvent::ToolCall { id, kind, args } => upd(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": id,
            "title": format!("{kind} {}", clip(args, CLIP_CHARS)),
            "kind": acp_tool_kind_str(kind),
            "status": "in_progress"
        })),
        AgentEvent::ToolResult {
            id,
            content,
            is_error,
        } => upd(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": id,
            "status": if *is_error { "failed" } else { "completed" },
            "content": [{ "type": "content", "content": { "type": "text", "text": clip(content, CLIP_CHARS) } }]
        })),
        AgentEvent::PlanProposed { steps, .. } => {
            let entries: Vec<Value> = steps
                .iter()
                .map(|st| {
                    json!({
                        "content": st.label,
                        "priority": "medium",
                        "status": acp_plan_status(&st.status)
                    })
                })
                .collect();
            upd(json!({ "sessionUpdate": "plan", "entries": entries }))
        }
        // Сообщаем причину клиенту как chunk (стоп-резон выставится на Response); путей не льём.
        AgentEvent::Error(s) => upd(json!({
            "sessionUpdate": "agent_message_chunk",
            "content": { "type": "text", "text": format!("[error] {s}") }
        })),
        // Остальные варианты НЕ маппятся в slice-1 (документировано в acp-server.md):
        //   Proposal/Diff — permission-поверхность через request_permission (decide()), не дублируем;
        //   Final — текст уже стримился как chunk, stopReason едет на Response;
        //   ContextUsage/PlanStepStatus/ExecProposal/ExecResult/SubagentStatus/Report — нет ACP-эквивалента
        //   / актуатор-exec/делегирование/research выключены. `_` обязателен — AgentEvent `#[non_exhaustive]`.
        _ => vec![],
    }
}

/// Маппинг исхода → ACP stopReason (валидный для всех путей — Response остаётся успешным JSON-RPC).
fn stopreason_from_outcome(outcome: &LoopOutcome) -> &'static str {
    match outcome {
        LoopOutcome::Final(_) => "end_turn",
        LoopOutcome::BudgetExhausted {
            kind: BudgetKind::Cancelled | BudgetKind::Paused,
            ..
        } => "cancelled",
        LoopOutcome::BudgetExhausted { .. } => "max_turn_requests",
        LoopOutcome::Error(_) => "refusal",
    }
}

// ───────────────────────── DecisionSource: session/request_permission ─────────────────────────

/// Источник решений ACP-сервера — ЕДИНСТВЕННОЕ место авторизации записи (инверсия ACP-1: ТАМ клиент
/// получает request_permission, ЗДЕСЬ мы его шлём и ждём ответ клиента). Fail-closed на КАЖДОМ рубеже:
/// send-fail / таймаут / oneshot-closed (EOF) / cancel / Cancelled / неизвестная опция / parse-miss → reject_all.
struct AcpServerDecisionSource {
    transport: Arc<dyn Transport>,
    session_id: String,
    run_id: i64,
    perm_pending: Arc<RpcCorrelator<PermReply>>,
    cancel: Arc<AtomicBool>,
    timeout: Duration,
}

#[async_trait]
impl DecisionSource for AcpServerDecisionSource {
    async fn decide(&self, batch: &ProposalBatch) -> BatchDecision {
        // Уже отменено к моменту запроса разрешения → не дёргаем клиента (fail-closed).
        if self.cancel.load(Ordering::Relaxed) {
            return BatchDecision::reject_all();
        }
        let (perm_id, rx) = self.perm_pending.begin().await;
        let params = proposal_to_permission_params(&self.session_id, self.run_id, perm_id, batch);

        if self
            .transport
            .send(RpcMessage::request(
                perm_id,
                "session/request_permission",
                params,
            ))
            .await
            .is_err()
        {
            self.perm_pending.cancel(perm_id).await;
            return BatchDecision::reject_all(); // транспорт закрыт
        }

        // Ждём ответ клиента с СОБСТВЕННЫМ таймаутом (5 мин — параметр self.timeout, инвариант R-9; cancel
        // дополнительно проваливается через cancel_session). Снятие записи на fallback — в корреляторе.
        let result = self
            .perm_pending
            .await_reply(
                perm_id,
                rx,
                Some(self.timeout),
                || Err(RpcError::internal("perm oneshot closed")),
                || Err(RpcError::internal("perm timeout")),
            )
            .await;
        outcome_to_batch_decision(batch, &result)
    }
}

/// Строит params `session/request_permission` (через json!). ProposalItem несёт ТОЛЬКО path+add/del →
/// деградированный diff (newText пуст, счётчики в title) — задокументированный R4. toolCallId
/// детерминирован (`run{run_id}-perm{perm_id}`); корреляция назад — по i64-id (perm_pending), НЕ по
/// client-supplied toolCallId.
fn proposal_to_permission_params(
    session_id: &str,
    run_id: i64,
    perm_id: i64,
    batch: &ProposalBatch,
) -> Value {
    let (add, del): (u32, u32) = batch
        .items
        .iter()
        .fold((0, 0), |(a, d), i| (a + i.add, d + i.del));
    let content: Vec<Value> = batch
        .items
        .iter()
        .map(|i| json!({ "type": "diff", "path": i.target_rel, "newText": "" }))
        .collect();
    json!({
        "sessionId": session_id,
        "toolCall": {
            "toolCallId": format!("run{run_id}-perm{perm_id}"),
            "title": format!("{} change(s): +{add}/-{del}", batch.items.len()),
            "kind": "edit",
            "content": content
        },
        "options": [
            { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
            { "optionId": "reject", "name": "Reject", "kind": "reject_once" }
        ]
    })
}

/// Чистый, FAIL-CLOSED маппинг client-исхода → BatchDecision. ТОЛЬКО `selected`+`optionId=="allow"`
/// одобряет ВЕСЬ батч (пер-батч: один Allow = все айтемы); reject/неизвестная опция/cancelled/parse-
/// miss/Err → reject_all.
fn outcome_to_batch_decision(
    batch: &ProposalBatch,
    result: &Result<Value, RpcError>,
) -> BatchDecision {
    let Ok(v) = result else {
        return BatchDecision::reject_all();
    };
    let outcome = v.pointer("/outcome/outcome").and_then(Value::as_str);
    let option = v.pointer("/outcome/optionId").and_then(Value::as_str);
    if outcome == Some("selected") && option == Some("allow") {
        BatchDecision::from_pairs(
            batch
                .items
                .iter()
                .map(|i| (i.action_id, ItemDecision::Approve)),
        )
    } else {
        BatchDecision::reject_all()
    }
}

// ───────────────────────── StdinStdoutTransport ─────────────────────────

/// [`Transport`] поверх РЕАЛЬНЫХ stdin/stdout процесса (родитель-ACP-клиент спавнит нас и говорит по
/// нашим пайпам). Framing общий (`framing::{send_frame,recv_frame}`). stdout — ИСКЛЮЧИТЕЛЬНО канал
/// протокола: любой println! его испортит (всё логирование → stderr/tracing).
pub struct StdinStdoutTransport {
    read: Mutex<BufReader<Stdin>>,
    write: Mutex<Stdout>,
}

impl Default for StdinStdoutTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl StdinStdoutTransport {
    pub fn new() -> Self {
        Self {
            read: Mutex::new(BufReader::new(tokio::io::stdin())),
            write: Mutex::new(tokio::io::stdout()),
        }
    }
}

#[async_trait]
impl Transport for StdinStdoutTransport {
    // NB: лок `write` НАМЕРЕННО удерживается через все await'ы внутри send_frame
    // (write_all + flush). Это не нарушение «clone-out-before-await» (тот паттерн —
    // про СОСТОЯНИЕ, которое не нужно держать): здесь эксклюзивный writer обязан быть
    // захвачен на ВСЮ запись одного JSON-RPC фрейма, иначе байты двух конкурентных
    // отправителей (drive-task event-drain + AcpServerDecisionSource::decide) могут
    // перемешаться и испортить line-delimited поток. tokio::Mutex cancellation-safe и
    // не poison'ится, так что удержание через await корректно. НЕ «оптимизировать»
    // отпусканием лока между write и flush.
    async fn send(&self, msg: RpcMessage) -> Result<(), TransportError> {
        let mut w = self.write.lock().await;
        framing::send_frame(&mut *w, msg).await
    }
    // Один потребитель recv по контракту Transport (только read-loop зовёт recv);
    // лок read держится через await фрейма — единственный читатель, гонок нет.
    async fn recv(&self) -> Option<RpcMessage> {
        let mut r = self.read.lock().await;
        framing::recv_frame(&mut *r, "acp-server").await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actuator::ProposalItem;
    use crate::agent::event::{PlanStep, PlanStepState};
    use crate::agent::tool::{ToolCall, ToolSpec};
    use crate::ai::tools::ToolTurn;
    use crate::ai::AiResult;
    use crate::db::Database;
    use crate::net::RunCtx;
    use std::collections::VecDeque;
    use std::sync::Mutex as StdMutex;
    use tempfile::TempDir;

    use super::super::super::{channel_pair, ChannelTransport};

    // ── провайдеры (offline) ──

    /// Скриптованный fake: FIFO заданных ходов.
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
            on_token: &mut (dyn FnMut(String) + Send),
            _cancel: &Arc<AtomicBool>,
            _ctx: RunCtx,
        ) -> AiResult<ToolTurn> {
            let next = self
                .turns
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(ToolTurn::Final("(no more turns)".into())));
            // На Final эмитим хотя бы один токен (доказ. потока agent_message_chunk).
            if let Ok(ToolTurn::Final(s)) = &next {
                on_token(s.clone());
            }
            next
        }
        fn model_id(&self) -> &str {
            "fake"
        }
    }

    /// Провайдер, висящий на первом ходу — держит ход активным детерминированно (R2-тест).
    struct SleepyProvider;
    #[async_trait]
    impl ToolCapableProvider for SleepyProvider {
        async fn stream_chat_tools(
            &self,
            _m: &[ChatMessage],
            _t: &[ToolSpec],
            _o: &mut (dyn FnMut(String) + Send),
            _c: &Arc<AtomicBool>,
            _ctx: RunCtx,
        ) -> AiResult<ToolTurn> {
            tokio::time::sleep(Duration::from_millis(250)).await;
            Ok(ToolTurn::Final("done".into()))
        }
        fn model_id(&self) -> &str {
            "sleepy"
        }
    }

    /// Провайдер, который КРУТИТСЯ (каждый ход возвращает ToolCalls со sleep) — чтобы цикл проверял
    /// `cancel` на границе шага и останавливался Cancelled (Final никогда не достигается сам).
    struct LoopingSleepyProvider;
    #[async_trait]
    impl ToolCapableProvider for LoopingSleepyProvider {
        async fn stream_chat_tools(
            &self,
            _m: &[ChatMessage],
            _t: &[ToolSpec],
            _o: &mut (dyn FnMut(String) + Send),
            _c: &Arc<AtomicBool>,
            _ctx: RunCtx,
        ) -> AiResult<ToolTurn> {
            tokio::time::sleep(Duration::from_millis(80)).await;
            Ok(ToolTurn::ToolCalls(vec![ToolCall {
                id: "loop".into(),
                name: "noop".into(),
                arguments: "{}".into(),
            }]))
        }
        fn model_id(&self) -> &str {
            "looping"
        }
    }

    /// Провайдер, ЗАПИСЫВАЮЩИЙ полученные messages КАЖДОГО хода (мультитёрн-история).
    struct RecordingProvider {
        seen: Arc<StdMutex<Vec<Vec<ChatMessage>>>>,
    }
    #[async_trait]
    impl ToolCapableProvider for RecordingProvider {
        async fn stream_chat_tools(
            &self,
            messages: &[ChatMessage],
            _t: &[ToolSpec],
            _o: &mut (dyn FnMut(String) + Send),
            _c: &Arc<AtomicBool>,
            _ctx: RunCtx,
        ) -> AiResult<ToolTurn> {
            self.seen.lock().unwrap().push(messages.to_vec());
            Ok(ToolTurn::Final("ok".into()))
        }
        fn model_id(&self) -> &str {
            "rec"
        }
    }

    // ── харнесс ──

    async fn open_db() -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join("test.db")).await.unwrap();
        (dir, db)
    }

    fn cfg_with(
        provider: Arc<dyn ToolCapableProvider>,
        canon_root: PathBuf,
        db: &Database,
        actuator_enabled: bool,
        autonomy: &str,
    ) -> Arc<AcpServerConfig> {
        Arc::new(AcpServerConfig {
            provider,
            writer: db.writer().clone(),
            reader: db.reader().clone(),
            canon_root,
            actuator_enabled,
            autonomy: autonomy.to_string(),
            overwrite_threshold: 64 * 1024,
            blast_cap: 16,
            context_window: Some(8192),
            model: "fake".into(),
        })
    }

    /// Поднимает serve_acp над server-эндпоинтом, возвращает client-эндпоинт.
    fn serve(
        cfg: Arc<AcpServerConfig>,
        client: ChannelTransport,
        server: ChannelTransport,
    ) -> Arc<ChannelTransport> {
        let server = Arc::new(server);
        tokio::spawn(serve_acp(server, cfg));
        Arc::new(client)
    }

    /// Достаёт params из Notification (для пуро-функциональных тестов маппинга).
    fn notif_params(m: &RpcMessage) -> Value {
        match m {
            RpcMessage::Notification { params, .. } => params.clone(),
            _ => panic!("ожидалась Notification, получено {m:?}"),
        }
    }

    async fn recv_to(t: &dyn Transport) -> RpcMessage {
        tokio::time::timeout(Duration::from_secs(5), t.recv())
            .await
            .expect("recv timeout")
            .expect("transport closed")
    }

    /// Шлёт request, ждёт Response с тем же id (пропуская промежуточные notification/прочие Response).
    async fn request(
        client: &dyn Transport,
        id: i64,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        client
            .send(RpcMessage::request(id, method, params))
            .await
            .unwrap();
        loop {
            if let RpcMessage::Response { id: rid, result } = recv_to(client).await {
                if rid == json!(id) {
                    return result;
                }
            }
        }
    }

    async fn init_and_session(client: &dyn Transport) -> String {
        let r = request(
            client,
            1,
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {}}),
        )
        .await
        .unwrap();
        assert_eq!(r["protocolVersion"], 1);
        let r = request(
            client,
            2,
            "session/new",
            json!({"cwd": "/ignored", "mcpServers": []}),
        )
        .await
        .unwrap();
        r["sessionId"].as_str().unwrap().to_string()
    }

    // ── 1. initialize ──
    #[tokio::test]
    async fn initialize_returns_protocol_version_1() {
        let (c, s) = channel_pair();
        let (_d, db) = open_db().await;
        let cfg = cfg_with(
            Arc::new(FakeProvider::new(vec![])),
            _d.path().to_path_buf(),
            &db,
            false,
            "confirm",
        );
        let client = serve(cfg, c, s);
        let r = request(
            client.as_ref(),
            1,
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {}}),
        )
        .await
        .unwrap();
        assert_eq!(r["protocolVersion"], 1);
    }

    #[tokio::test]
    async fn initialize_non_object_params_invalid() {
        let (c, s) = channel_pair();
        let (_d, db) = open_db().await;
        let cfg = cfg_with(
            Arc::new(FakeProvider::new(vec![])),
            _d.path().to_path_buf(),
            &db,
            false,
            "confirm",
        );
        let client = serve(cfg, c, s);
        let r = request(client.as_ref(), 1, "initialize", json!("not-an-object")).await;
        assert_eq!(r.unwrap_err().code, -32602);
    }

    // ── 2. session/new ──
    #[tokio::test]
    async fn session_new_returns_session_id() {
        let (c, s) = channel_pair();
        let (_d, db) = open_db().await;
        let cfg = cfg_with(
            Arc::new(FakeProvider::new(vec![])),
            _d.path().to_path_buf(),
            &db,
            false,
            "confirm",
        );
        let client = serve(cfg, c, s);
        let _ = request(
            client.as_ref(),
            1,
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {}}),
        )
        .await;
        let r = request(
            client.as_ref(),
            2,
            "session/new",
            json!({"cwd": "/x", "mcpServers": []}),
        )
        .await
        .unwrap();
        assert!(r["sessionId"].as_str().unwrap().starts_with('s'));
    }

    // ── 3. prompt стримит и финалит end_turn ──
    #[tokio::test]
    async fn prompt_streams_and_finals_end_turn() {
        let (c, s) = channel_pair();
        let (_d, db) = open_db().await;
        let provider: Arc<dyn ToolCapableProvider> = Arc::new(FakeProvider::new(vec![
            Ok(ToolTurn::ToolCalls(vec![ToolCall {
                id: "c1".into(),
                name: "echo".into(),
                arguments: r#"{"text":"hi"}"#.into(),
            }])),
            Ok(ToolTurn::Final("готово".into())),
        ]));
        let cfg = cfg_with(provider, _d.path().to_path_buf(), &db, false, "confirm");
        let client = serve(cfg, c, s);
        let sid = init_and_session(client.as_ref()).await;

        client
            .send(RpcMessage::request(
                3,
                "session/prompt",
                json!({"sessionId": sid, "prompt": [{"type":"text","text":"do"}]}),
            ))
            .await
            .unwrap();

        let mut saw_chunk = false;
        let mut saw_tool_call = false;
        let mut saw_tool_update = false;
        let mut stop = String::new();
        for _ in 0..50 {
            match recv_to(client.as_ref()).await {
                RpcMessage::Notification { method, params } if method == "session/update" => {
                    match params["update"]["sessionUpdate"].as_str().unwrap_or("") {
                        "agent_message_chunk" => saw_chunk = true,
                        "tool_call" => saw_tool_call = true,
                        "tool_call_update" => saw_tool_update = true,
                        _ => {}
                    }
                }
                RpcMessage::Response { id, result } if id == json!(3) => {
                    stop = result.unwrap()["stopReason"].as_str().unwrap().to_string();
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_tool_call, "tool_call застримлен");
        assert!(saw_tool_update, "tool_call_update застримлен");
        assert!(saw_chunk, "agent_message_chunk застримлен");
        assert_eq!(stop, "end_turn", "Response пришёл ПОСЛЕ стрима");
    }

    // ── keystone-permission: helper, гоняющий note.create через гейт с заданным client-исходом ──
    async fn run_permission_case(
        autonomy: &str,
        // None → не отвечаем (для transport-close); Some(outcome) → шлём этот /result.
        client_outcome: Option<Value>,
        drop_after_perm: bool,
    ) -> (bool, String) {
        let (c, s) = channel_pair();
        let (dir, db) = open_db().await;
        let canon = dir.path().canonicalize().unwrap();
        let provider: Arc<dyn ToolCapableProvider> = Arc::new(FakeProvider::new(vec![
            Ok(ToolTurn::ToolCalls(vec![ToolCall {
                id: "n1".into(),
                name: "note.create".into(),
                arguments: r#"{"path":"Notes/W.md","content":"данные"}"#.into(),
            }])),
            Ok(ToolTurn::Final("готово".into())),
        ]));
        let cfg = cfg_with(provider, canon.clone(), &db, true, autonomy);
        let client = serve(cfg, c, s);
        let sid = init_and_session(client.as_ref()).await;
        client
            .send(RpcMessage::request(
                3,
                "session/prompt",
                json!({"sessionId": sid, "prompt": [{"type":"text","text":"создай"}]}),
            ))
            .await
            .unwrap();

        let mut stop = String::new();
        let mut perm_seen = false;
        for _ in 0..80 {
            match recv_to(client.as_ref()).await {
                RpcMessage::Request { id, method, .. }
                    if method == "session/request_permission" =>
                {
                    perm_seen = true;
                    if drop_after_perm {
                        drop(client); // транспорт закрыт мид-permission → fail-closed
                                      // ждём, пока серверный прогон завершится (он зафиналит)
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        break;
                    }
                    if let Some(out) = &client_outcome {
                        client
                            .send(RpcMessage::Response {
                                id,
                                result: Ok(out.clone()),
                            })
                            .await
                            .unwrap();
                    }
                }
                RpcMessage::Response { id, result } if id == json!(3) => {
                    stop = result.unwrap()["stopReason"].as_str().unwrap().to_string();
                    break;
                }
                _ => {}
            }
        }
        assert!(
            perm_seen || autonomy == "auto",
            "request_permission ожидался (confirm)"
        );
        let written = std::fs::read_to_string(canon.join("Notes/W.md")).is_ok();
        (written, stop)
    }

    // ── 4. allow применяет запись ──
    #[tokio::test]
    async fn permission_allow_applies_write() {
        let (written, stop) = run_permission_case(
            "confirm",
            Some(json!({"outcome": {"outcome": "selected", "optionId": "allow"}})),
            false,
        )
        .await;
        assert!(written, "allow → файл записан через гейт");
        assert_eq!(stop, "end_turn");
    }

    // ── 5. reject не пишет ──
    #[tokio::test]
    async fn permission_reject_does_not_write() {
        let (written, stop) = run_permission_case(
            "confirm",
            Some(json!({"outcome": {"outcome": "selected", "optionId": "reject"}})),
            false,
        )
        .await;
        assert!(!written, "reject → файл НЕ записан");
        assert_eq!(stop, "end_turn", "ход всё равно финалит");
    }

    // ── 6. cancelled не пишет ──
    #[tokio::test]
    async fn permission_cancelled_does_not_write() {
        let (written, _stop) = run_permission_case(
            "confirm",
            Some(json!({"outcome": {"outcome": "cancelled"}})),
            false,
        )
        .await;
        assert!(!written, "cancelled → reject_all → файл НЕ записан");
    }

    // ── 7. неизвестная опция не пишет ──
    #[tokio::test]
    async fn permission_unknown_option_rejects() {
        let (written, _stop) = run_permission_case(
            "confirm",
            Some(json!({"outcome": {"outcome": "selected", "optionId": "bogus"}})),
            false,
        )
        .await;
        assert!(
            !written,
            "неизвестная optionId → reject_all → файл НЕ записан"
        );
    }

    // ── 8. закрытие транспорта мид-permission → reject_all, без зависа ──
    #[tokio::test]
    async fn permission_transport_close_rejects() {
        let (written, _stop) = run_permission_case("confirm", None, true).await;
        assert!(
            !written,
            "EOF мид-permission → reject_all (fail-closed), файл НЕ записан"
        );
    }

    // ── 9. unknown method → -32601 ──
    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let (c, s) = channel_pair();
        let (_d, db) = open_db().await;
        let cfg = cfg_with(
            Arc::new(FakeProvider::new(vec![])),
            _d.path().to_path_buf(),
            &db,
            false,
            "confirm",
        );
        let client = serve(cfg, c, s);
        let r = request(client.as_ref(), 1, "fs/read_text_file", json!({})).await;
        assert_eq!(r.unwrap_err().code, -32601);
    }

    // ── 10. битые params prompt → -32602 ──
    #[tokio::test]
    async fn malformed_params_invalid_params() {
        let (c, s) = channel_pair();
        let (_d, db) = open_db().await;
        let cfg = cfg_with(
            Arc::new(FakeProvider::new(vec![])),
            _d.path().to_path_buf(),
            &db,
            false,
            "confirm",
        );
        let client = serve(cfg, c, s);
        let _ = init_and_session(client.as_ref()).await;
        let r = request(client.as_ref(), 3, "session/prompt", json!({"wrong": 1})).await;
        assert_eq!(r.unwrap_err().code, -32602);
    }

    // ── 11. вторая session/new → -32602 (R1) ──
    #[tokio::test]
    async fn second_session_rejected() {
        let (c, s) = channel_pair();
        let (_d, db) = open_db().await;
        let cfg = cfg_with(
            Arc::new(FakeProvider::new(vec![])),
            _d.path().to_path_buf(),
            &db,
            false,
            "confirm",
        );
        let client = serve(cfg, c, s);
        let _ = init_and_session(client.as_ref()).await;
        let r = request(
            client.as_ref(),
            9,
            "session/new",
            json!({"cwd": "/y", "mcpServers": []}),
        )
        .await;
        assert_eq!(r.unwrap_err().code, -32602, "R1: вторая сессия отклонена");
    }

    // ── 12. второй prompt при активном → -32602 (R2), затем третий проходит ──
    #[tokio::test]
    async fn second_prompt_while_active_rejected() {
        let (c, s) = channel_pair();
        let (_d, db) = open_db().await;
        let cfg = cfg_with(
            Arc::new(SleepyProvider),
            _d.path().to_path_buf(),
            &db,
            false,
            "confirm",
        );
        let client = serve(cfg, c, s);
        let sid = init_and_session(client.as_ref()).await;

        // первый prompt — НЕ ждём ответа (sleepy висит).
        client
            .send(RpcMessage::request(
                3,
                "session/prompt",
                json!({"sessionId": sid, "prompt": [{"type":"text","text":"first"}]}),
            ))
            .await
            .unwrap();
        // дать первому взвести active.
        tokio::time::sleep(Duration::from_millis(50)).await;
        // второй prompt при активном → invalid_params (id=4).
        let r = request(
            client.as_ref(),
            4,
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type":"text","text":"second"}]}),
        )
        .await;
        assert_eq!(
            r.unwrap_err().code,
            -32602,
            "R2: второй активный prompt отклонён"
        );

        // дождаться завершения первого (Response id=3) — потом третий проходит.
        let mut first_done = false;
        for _ in 0..80 {
            if let RpcMessage::Response { id, .. } = recv_to(client.as_ref()).await {
                if id == json!(3) {
                    first_done = true;
                    break;
                }
            }
        }
        assert!(first_done, "первый ход завершился");
        let r3 = request(
            client.as_ref(),
            5,
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type":"text","text":"third"}]}),
        )
        .await;
        assert_eq!(
            r3.unwrap()["stopReason"],
            "end_turn",
            "после завершения — третий ход принят"
        );
    }

    // ── 13. мультитёрн: ход 2 видит историю хода 1 ──
    #[tokio::test]
    async fn multi_turn_history_accumulates() {
        let (c, s) = channel_pair();
        let (_d, db) = open_db().await;
        let seen = Arc::new(StdMutex::new(Vec::<Vec<ChatMessage>>::new()));
        let provider: Arc<dyn ToolCapableProvider> =
            Arc::new(RecordingProvider { seen: seen.clone() });
        let cfg = cfg_with(provider, _d.path().to_path_buf(), &db, false, "confirm");
        let client = serve(cfg, c, s);
        let sid = init_and_session(client.as_ref()).await;

        let r1 = request(
            client.as_ref(),
            3,
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type":"text","text":"ALPHA"}]}),
        )
        .await;
        assert_eq!(r1.unwrap()["stopReason"], "end_turn");
        let r2 = request(
            client.as_ref(),
            4,
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type":"text","text":"BETA"}]}),
        )
        .await;
        assert_eq!(r2.unwrap()["stopReason"], "end_turn");

        let captured = seen.lock().unwrap();
        assert_eq!(captured.len(), 2, "два хода");
        // ход 2 ДОЛЖЕН видеть user(ALPHA)+assistant(ok) из хода 1.
        let turn2_has_alpha = captured[1].iter().any(|m| m.content.contains("ALPHA"));
        let turn2_has_assistant = captured[1].iter().any(|m| m.content == "ok");
        assert!(
            turn2_has_alpha,
            "ход 2 видит user-задачу хода 1 (W-4 история)"
        );
        assert!(turn2_has_assistant, "ход 2 видит assistant-ответ хода 1");
        // ход 1 НЕ должен видеть BETA (порядок).
        assert!(!captured[0].iter().any(|m| m.content.contains("BETA")));
    }

    // ── 14. session/cancel взводит флаг и останавливает ход ──
    #[tokio::test]
    async fn session_cancel_sets_flag_and_stops_turn() {
        let (c, s) = channel_pair();
        let (_d, db) = open_db().await;
        let cfg = cfg_with(
            Arc::new(LoopingSleepyProvider),
            _d.path().to_path_buf(),
            &db,
            false,
            "confirm",
        );
        let client = serve(cfg, c, s);
        let sid = init_and_session(client.as_ref()).await;

        client
            .send(RpcMessage::request(
                3,
                "session/prompt",
                json!({"sessionId": sid, "prompt": [{"type":"text","text":"go"}]}),
            ))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        // cancel как notification.
        client
            .send(RpcMessage::notification(
                "session/cancel",
                json!({"sessionId": sid}),
            ))
            .await
            .unwrap();

        let mut stop = String::new();
        for _ in 0..80 {
            if let RpcMessage::Response { id, result } = recv_to(client.as_ref()).await {
                if id == json!(3) {
                    stop = result.unwrap()["stopReason"].as_str().unwrap().to_string();
                    break;
                }
            }
        }
        assert_eq!(stop, "cancelled", "cancel → ход завершился cancelled");
    }

    // ── 15. auto: Auto-тир применяется БЕЗ permission ──
    #[tokio::test]
    async fn auto_autonomy_applies_auto_tier_without_permission() {
        let (c, s) = channel_pair();
        let (dir, db) = open_db().await;
        let canon = dir.path().canonicalize().unwrap();
        let provider: Arc<dyn ToolCapableProvider> = Arc::new(FakeProvider::new(vec![
            Ok(ToolTurn::ToolCalls(vec![ToolCall {
                id: "n1".into(),
                name: "note.create".into(),
                arguments: r#"{"path":"Notes/A.md","content":"auto"}"#.into(),
            }])),
            Ok(ToolTurn::Final("готово".into())),
        ]));
        let cfg = cfg_with(provider, canon.clone(), &db, true, "auto");
        let client = serve(cfg, c, s);
        let sid = init_and_session(client.as_ref()).await;
        client
            .send(RpcMessage::request(
                3,
                "session/prompt",
                json!({"sessionId": sid, "prompt": [{"type":"text","text":"создай"}]}),
            ))
            .await
            .unwrap();

        let mut perm = false;
        let mut stop = String::new();
        for _ in 0..80 {
            match recv_to(client.as_ref()).await {
                RpcMessage::Request { method, .. } if method == "session/request_permission" => {
                    perm = true
                }
                RpcMessage::Response { id, result } if id == json!(3) => {
                    stop = result.unwrap()["stopReason"].as_str().unwrap().to_string();
                    break;
                }
                _ => {}
            }
        }
        assert!(!perm, "Auto-тир под auto НЕ шлёт request_permission");
        assert_eq!(stop, "end_turn");
        assert!(
            std::fs::read_to_string(canon.join("Notes/A.md")).is_ok(),
            "Auto-тир авто-применён БЕЗ permission"
        );
    }

    // ── 16. EOF без активного хода → serve_acp возвращается ──
    #[tokio::test]
    async fn eof_drains_cleanly() {
        let (c, s) = channel_pair();
        let (_d, db) = open_db().await;
        let cfg = cfg_with(
            Arc::new(FakeProvider::new(vec![])),
            _d.path().to_path_buf(),
            &db,
            false,
            "confirm",
        );
        let server = Arc::new(s);
        let h = tokio::spawn(serve_acp(server, cfg));
        drop(c); // закрываем клиента → EOF
        let r = tokio::time::timeout(Duration::from_secs(5), h).await;
        assert!(r.is_ok(), "serve_acp вернулся по EOF (без зависа)");
    }

    // ── 17. слишком большой prompt → -32602 ──
    #[tokio::test]
    async fn oversized_prompt_rejected() {
        let (c, s) = channel_pair();
        let (_d, db) = open_db().await;
        let cfg = cfg_with(
            Arc::new(FakeProvider::new(vec![])),
            _d.path().to_path_buf(),
            &db,
            false,
            "confirm",
        );
        let client = serve(cfg, c, s);
        let sid = init_and_session(client.as_ref()).await;
        let big = "x".repeat(MAX_PROMPT_BYTES + 1);
        let r = request(
            client.as_ref(),
            3,
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type":"text","text": big}]}),
        )
        .await;
        assert_eq!(
            r.unwrap_err().code,
            -32602,
            "prompt > 256KiB → invalid_params"
        );
    }

    // ── 18. чистые функции ──
    #[test]
    fn map_event_assistant_token() {
        let v = map_event_to_acp("s1", &AgentEvent::AssistantToken("hi".into()));
        assert_eq!(v.len(), 1);
        match &v[0] {
            RpcMessage::Notification { method, params } => {
                assert_eq!(method, "session/update");
                assert_eq!(params["sessionId"], "s1");
                assert_eq!(params["update"]["sessionUpdate"], "agent_message_chunk");
                assert_eq!(params["update"]["content"]["text"], "hi");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn map_event_tool_call_and_result() {
        let call = map_event_to_acp(
            "s1",
            &AgentEvent::ToolCall {
                id: "t1".into(),
                kind: "note.create".into(),
                args: "{}".into(),
            },
        );
        match &call[0] {
            RpcMessage::Notification { params, .. } => {
                assert_eq!(params["update"]["sessionUpdate"], "tool_call");
                assert_eq!(params["update"]["toolCallId"], "t1");
                assert_eq!(params["update"]["kind"], "edit"); // note.create → write → edit
                assert_eq!(params["update"]["status"], "in_progress");
            }
            _ => panic!(),
        }
        let ok = map_event_to_acp(
            "s1",
            &AgentEvent::ToolResult {
                id: "t1".into(),
                content: "done".into(),
                is_error: false,
            },
        );
        assert_eq!(notif_params(&ok[0])["update"]["status"], "completed");
        let err = map_event_to_acp(
            "s1",
            &AgentEvent::ToolResult {
                id: "t1".into(),
                content: "boom".into(),
                is_error: true,
            },
        );
        assert_eq!(notif_params(&err[0])["update"]["status"], "failed");
    }

    #[test]
    fn map_event_plan_proposed() {
        let v = map_event_to_acp(
            "s1",
            &AgentEvent::PlanProposed {
                run_id: 1,
                steps: vec![
                    PlanStep {
                        id: "a".into(),
                        label: "research".into(),
                        status: PlanStepState::Running,
                    },
                    PlanStep {
                        id: "b".into(),
                        label: "write".into(),
                        status: PlanStepState::Failed,
                    },
                ],
            },
        );
        let p = notif_params(&v[0]);
        assert_eq!(p["update"]["sessionUpdate"], "plan");
        assert_eq!(p["update"]["entries"][0]["status"], "in_progress");
        assert_eq!(p["update"]["entries"][1]["status"], "completed"); // Failed → completed
    }

    #[test]
    fn map_event_empties() {
        for ev in [
            AgentEvent::Final("x".into()),
            AgentEvent::Proposal {
                run_id: 1,
                files: vec![],
            },
            AgentEvent::Diff {
                path: "a".into(),
                add: 1,
                del: 0,
                status: crate::agent::event::FileStatus::New,
            },
            AgentEvent::ContextUsage { used: 1, window: 2 },
            AgentEvent::PlanStepStatus {
                id: "x".into(),
                status: PlanStepState::Done,
            },
            AgentEvent::SubagentStatus {
                parent_run_id: 1,
                child_run_id: 2,
                goal: "g".into(),
                status: crate::agent::event::SubagentState::Done,
                summary: None,
            },
        ] {
            assert!(map_event_to_acp("s1", &ev).is_empty(), "{ev:?} → пусто");
        }
        // Error → один chunk с [error] (НЕ пусто).
        assert_eq!(
            map_event_to_acp("s1", &AgentEvent::Error("boom".into())).len(),
            1
        );
    }

    fn sample_batch() -> ProposalBatch {
        use crate::actuator::classify::{ConfirmReason, RiskTier};
        ProposalBatch {
            run_id: 7,
            items: vec![
                ProposalItem {
                    action_id: 10,
                    target_rel: "A.md".into(),
                    tier: RiskTier::Confirm(ConfirmReason::LargeOverwrite),
                    add: 3,
                    del: 1,
                },
                ProposalItem {
                    action_id: 20,
                    target_rel: "B.md".into(),
                    tier: RiskTier::Auto,
                    add: 2,
                    del: 0,
                },
            ],
        }
    }

    #[test]
    fn proposal_to_permission_params_shape() {
        let p = proposal_to_permission_params("s1", 7, PERM_ID_BASE, &sample_batch());
        assert_eq!(p["sessionId"], "s1");
        assert_eq!(
            p["toolCall"]["toolCallId"],
            format!("run7-perm{PERM_ID_BASE}")
        );
        assert_eq!(p["toolCall"]["kind"], "edit");
        let content = p["toolCall"]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2, "2-айтемный батч → 2 diff-записи");
        assert_eq!(content[0]["path"], "A.md");
        assert_eq!(content[0]["newText"], ""); // деградированный diff (R4)
        let opts = p["options"].as_array().unwrap();
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0]["optionId"], "allow");
        assert_eq!(opts[1]["optionId"], "reject");
        // title несёт суммарные +/-.
        assert!(p["toolCall"]["title"].as_str().unwrap().contains("+5/-1"));
    }

    #[test]
    fn outcome_to_batch_decision_cases() {
        let b = sample_batch();
        // selected+allow → одобряет ВСЕ айтемы.
        let allow = outcome_to_batch_decision(
            &b,
            &Ok(json!({"outcome": {"outcome": "selected", "optionId": "allow"}})),
        );
        assert!(allow.is_approved(10) && allow.is_approved(20));
        // reject / unknown / cancelled / Err → reject_all.
        for r in [
            Ok(json!({"outcome": {"outcome": "selected", "optionId": "reject"}})),
            Ok(json!({"outcome": {"outcome": "selected", "optionId": "weird"}})),
            Ok(json!({"outcome": {"outcome": "cancelled"}})),
            Err(RpcError::internal("x")),
        ] {
            let d = outcome_to_batch_decision(&b, &r);
            assert!(
                !d.is_approved(10) && !d.is_approved(20),
                "fail-closed reject_all для {r:?}"
            );
        }
    }

    #[test]
    fn stopreason_mapping() {
        assert_eq!(
            stopreason_from_outcome(&LoopOutcome::Final("x".into())),
            "end_turn"
        );
        assert_eq!(
            stopreason_from_outcome(&LoopOutcome::BudgetExhausted {
                kind: BudgetKind::Cancelled,
                partial: String::new()
            }),
            "cancelled"
        );
        assert_eq!(
            stopreason_from_outcome(&LoopOutcome::BudgetExhausted {
                kind: BudgetKind::Paused,
                partial: String::new()
            }),
            "cancelled"
        );
        assert_eq!(
            stopreason_from_outcome(&LoopOutcome::BudgetExhausted {
                kind: BudgetKind::Tokens,
                partial: String::new()
            }),
            "max_turn_requests"
        );
        assert_eq!(
            stopreason_from_outcome(&LoopOutcome::BudgetExhausted {
                kind: BudgetKind::Steps,
                partial: String::new()
            }),
            "max_turn_requests"
        );
        assert_eq!(
            stopreason_from_outcome(&LoopOutcome::BudgetExhausted {
                kind: BudgetKind::WallClock,
                partial: String::new()
            }),
            "max_turn_requests"
        );
        assert_eq!(
            stopreason_from_outcome(&LoopOutcome::Error("e".into())),
            "refusal"
        );
    }

    /// R-2 ХАРАКТЕРИЗАЦИЯ (фикстура «до/после» дедупа): полная таблица вариант → (статус, текст)
    /// ЭТОГО вызывателя (канон с параметрами ACP-сервера: FinalizeError + «прогон отменён»), точным
    /// сравнением (байт-в-байт). Тексты попадают в run_store/историю прогонов/UI — канонизация R-2
    /// обязана сохранить их без изменений; ассерты идентичны фикстуре «до» на локальной копии.
    #[test]
    fn outcome_to_finish_characterization_full_table() {
        use crate::agent::run_store::{STATUS_CANCELLED, STATUS_DONE, STATUS_ERROR};
        let be = |kind: BudgetKind| LoopOutcome::BudgetExhausted {
            kind,
            partial: "часть".into(),
        };
        let table: [(LoopOutcome, &str, &str); 7] = [
            (LoopOutcome::Final("итог".into()), STATUS_DONE, "итог"),
            (
                be(BudgetKind::Cancelled),
                STATUS_CANCELLED,
                "прогон отменён; частичный ответ: часть",
            ),
            (
                be(BudgetKind::Paused),
                STATUS_ERROR,
                "прогон приостановлен (kill-switch); частичный ответ: часть",
            ),
            (
                be(BudgetKind::Steps),
                STATUS_ERROR,
                "бюджет исчерпан (Steps); частичный ответ: часть",
            ),
            (
                be(BudgetKind::WallClock),
                STATUS_ERROR,
                "бюджет исчерпан (WallClock); частичный ответ: часть",
            ),
            (
                be(BudgetKind::Tokens),
                STATUS_ERROR,
                "бюджет исчерпан (Tokens); частичный ответ: часть",
            ),
            (LoopOutcome::Error("упал".into()), STATUS_ERROR, "упал"),
        ];
        for (outcome, want_status, want_text) in table {
            let (status, text) = outcome_to_finish(
                &outcome,
                PausePolicy::FinalizeError,
                CancelWording::RunCancelled,
            )
            .expect_finalize();
            assert_eq!(
                (status, text.as_str()),
                (want_status, want_text),
                "вариант: {outcome:?}"
            );
        }
    }

    #[test]
    fn concat_prompt_text_joins_text_blocks() {
        use super::super::schema::ContentBlock;
        let t = concat_prompt_text(&[
            ContentBlock::Text { text: "a".into() },
            ContentBlock::Other,
            ContentBlock::Text { text: "b".into() },
        ]);
        assert_eq!(t, "a\nb");
    }
}
