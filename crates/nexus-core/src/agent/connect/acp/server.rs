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
mod tests;
