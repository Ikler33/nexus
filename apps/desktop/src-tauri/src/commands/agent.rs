//! UI-1a — бэкенд агентского цикла в desktop: tauri-команды запуска/контроля прогона + стрим событий
//! во фронт через [`tauri::ipc::Channel`] + UI-driven [`DecisionSource`] для аппрува changeset'ов.
//!
//! Это БЭКЕНД-половина UI-1 (фронт `AgentView` — UI-1b). Команды:
//! - [`agent_run`] — создаёт прогон (`agent_runs`-строка), спавнит [`run_agent_loop`] в `tokio::spawn`,
//!   форвардит каждый [`AgentEvent`] → `channel.send(AgentStreamEvent)`, возвращает `run_id` СРАЗУ.
//! - [`agent_approve`] — кормит UI-DecisionSource [`BatchDecision`] (Confirm-тир аппрув/реджект).
//! - [`agent_pause`]/[`agent_resume`] — per-run kill-switch (AGENT-5).
//! - [`agent_undo`] — откат применённых действий прогона (AGENT-4, `actuator::undo_run`).
//! - [`agent_cancel`] — кооперативная отмена прогона.
//!
//! # Зеркало композиции agentd (НЕ дубль логики ядра)
//! Реестр инструментов строится КАК в `nexus-agentd::main` / [`nexus_core::agent::AgentRunHandler`]:
//! actuator default-OFF (`ai.agent_actuator_enabled` нет/false → СТАБЫ echo/noop, реальный vault НЕ
//! трогается) → стабы; ВКЛ → гейтнутые инструменты-актуаторы за `actuator::dispatch_action` (тот же
//! гейт, тот же `GuardedClient`). РАЗНИЦА с agentd: agentd гоняет цикл ВНУТРИ `AgentRunHandler::handle`
//! (его `on_event` внутренний — `FIXME(UI-1)`); здесь мы гоним [`run_agent_loop`] НАПРЯМУЮ, чтобы
//! контролировать `on_event` → стрим в Channel в реальном времени. И UI-DecisionSource заменяет
//! headless `PolicyDefault` (auto-DENY): Confirm-тир реально аппрувится из UI (человек-в-петле).
//!
//! # Границы (СОХРАНЕНЫ)
//! - Actuator default OFF — дефолт НЕ меняется (флаг конфига, как в agentd).
//! - Эгресс/актуатор — через существующие гейты (`GuardedClient`/`dispatch_action`). НЕТ новых
//!   egress-путей (tool-провайдер строит `nexus_core::ai::tools::build_agent_tool_provider` — тот же
//!   `GuardedClient::for_chat` + `EgressFeature::Chat`, что и chat).
//! - Переиспользуем ядро: `AgentRunHandler`-композицию (реестр/бюджет/токенайзер/память),
//!   `run_agent_loop`, `undo_run`, `DecisionSource`/`BatchDecision` — НЕ копируем логику.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use tauri::ipc::Channel;
use tauri::State;

use nexus_core::actuator::{
    self, AuditSink, BatchDecision, DecisionSource, ItemDecision, ProposalBatch,
};
use nexus_core::agent::{
    run_agent_session, run_store, AgentEvent, AgentEventForwarder, AgentMemory, LoopOutcome,
    SessionSpec, VaultAgentMemory,
};

use crate::error::{AppError, AppResult};
use crate::state::{AgentRunEntry, AppState};

/// Глубина канала решений UI-DecisionSource: предложений в прогоне может быть несколько (по одному на
/// changeset-айтем), фронт аппрувит их по очереди. Скромный буфер — каждый decide() ждёт своё решение.
const DECISION_CHANNEL_CAP: usize = 8;

// AGENT_PREAMBLE + RECALL_BUDGET_TOKENS импортируются из ядра (см. верхний use) — ЕДИНЫЙ источник
// истины (UI-1a-ревью: убрана локальная копия, чтобы desktop и agentd не разъехались по преамбулу/бюджету).

// ── Контракт стрима «бэкенд → фронт» (UI-1b потребитель) ──────────────────────────────────────────

// Wire-DTO событий агента + маппер вынесены в `nexus_core::agent::connect::wire` — ЕДИНЫЙ источник
// истины контракта «бэкенд→клиент»: тот же тип использует agentd-коннектор для `agent/event`-
// нотификаций (AGENT-CONNECT P0b), чтобы desktop (UI-1b) и сервис не разъехались по JSON-контракту.
// Ре-экспорт сохраняет прежние имена → остальной desktop-код и тесты не меняются.
pub use nexus_core::agent::connect::wire::{map_agent_event, AgentStreamEvent};

// ── AgentEventForwarder → Channel (FIXME(UI-1) РЕШЁН): стрим событий прогона во фронт ──────────────

/// [`AgentEventForwarder`]-мост прогона → агент-стрим во фронт. ЕДИНЫЙ форвардер для desktop: его
/// получает и `run_agent_session` (события цикла), и гейт актуатора (Proposal/Diff — через внутренний
/// `ForwardingEventSink`). Маппит `AgentEvent` → wire-DTO и шлёт в [`Channel`] (best-effort: фронт мог
/// отвалиться). Гейт блокируется на `DecisionSource::decide`, ожидая `agent_approve` (человек-в-петле).
/// Headless agentd вместо этого считает шаги + tracing-логирует (см. `agent::job::HeadlessForwarder`).
struct ChannelForwarder {
    channel: Channel<AgentStreamEvent>,
}

impl AgentEventForwarder for ChannelForwarder {
    fn forward(&self, ev: &AgentEvent) {
        if let Some(mapped) = map_agent_event(ev) {
            let _ = self.channel.send(mapped);
        }
    }
}

// ── UI-driven DecisionSource (заменяет headless PolicyDefault auto-DENY) ───────────────────────────

/// Источник решений по предложениям, КОРМИМЫЙ `agent_approve` через mpsc-канал. Поведение
/// [`nexus_core::actuator::ChannelDecision`], но канал отдаётся UI (не пред-засеян тестом): `decide`
/// ждёт следующий [`BatchDecision`], присланный `agent_approve`. **Fail-closed**: канал закрыт/пуст
/// (фронт ушёл, не ответив) → `reject_all` (НИ один айтем не применяется без явного Approve). Auto-тир
/// под `autonomy=auto` применяется гейтом БЕЗ обращения сюда (как в гейте) — этот источник нужен
/// только для Confirm-тира.
struct UiDecisionSource {
    rx: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<BatchDecision>>,
}

impl UiDecisionSource {
    /// Источник + sender для решений (sender кладётся в реестр прогона; `agent_approve` шлёт в него).
    fn new() -> (Self, tokio::sync::mpsc::Sender<BatchDecision>) {
        let (tx, rx) = tokio::sync::mpsc::channel(DECISION_CHANNEL_CAP);
        (
            Self {
                rx: tokio::sync::Mutex::new(rx),
            },
            tx,
        )
    }
}

#[async_trait]
impl DecisionSource for UiDecisionSource {
    async fn decide(&self, _batch: &ProposalBatch) -> BatchDecision {
        // Берём следующее решение, присланное `agent_approve`. None (канал закрыт и пуст — фронт ушёл,
        // не ответив) ⇒ fail-closed reject_all: ничего не применяем без явного Approve.
        let mut rx = self.rx.lock().await;
        rx.recv().await.unwrap_or_else(BatchDecision::reject_all)
    }
}

// ── Команды ───────────────────────────────────────────────────────────────────────────────────────

/// Решение по одному предложенному действию (вход `agent_approve`): `action_id` строки `agent_actions`
/// + одобрить ли. camelCase для фронта (`{actionId, approve}`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDecision {
    /// `id` строки ledger (из `AgentStreamEvent::Proposal.files[].actionId`).
    pub action_id: i64,
    /// Одобрить (apply) или отклонить (диск не трогаем).
    pub approve: bool,
}

/// Уровень автономии прогона (вход `agent_run`): `"confirm"` (Confirm-тир ждёт аппрува) | `"auto"`
/// (Auto-тир применяется под blast-radius-кэпом без аппрува). Прочее/отсутствие → confirm (безопаснее).
fn normalize_autonomy(autonomy: &str) -> &'static str {
    match autonomy {
        "auto" => "auto",
        _ => "confirm",
    }
}

/// Запускает прогон агента: создаёт строку `agent_runs`, регистрирует прогон в реестре state, спавнит
/// [`run_agent_loop`] в `tokio::spawn` (форвардит каждый [`AgentEvent`] → `channel`), возвращает `run_id`
/// СРАЗУ (прогон асинхронный). НЕ блокирует.
///
/// Композиция зеркалит agentd / [`agent::AgentRunHandler`]: tool-провайдер из `ai.chat`, токенайзер/
/// бюджет, реестр (стабы при выключенном актуаторе [дефолт], гейтнутые инструменты при ВКЛ), память
/// (recall + Add-only запись), UI-DecisionSource, per-run kill-switch.
#[tauri::command]
pub async fn agent_run(
    state: State<'_, AppState>,
    task: String,
    autonomy: String,
    channel: Channel<AgentStreamEvent>,
) -> AppResult<i64> {
    let autonomy = normalize_autonomy(&autonomy);

    // Снимаем нужное из контекста vault и отпускаем read-гард ДО долгого спавна (как chat.rs). Берём
    // ТОЛЬКО нужные хендлы (AIClient не `Clone` — клонируем поля точечно).
    let (
        root,
        reader,
        writer,
        agent_tools,
        embedder,
        memory_vectors,
        chat_vectors,
        episode_vectors,
    ) = {
        let ctx = state.vault().await?;
        (
            ctx.root.clone(),
            ctx.db.reader().clone(),
            ctx.db.writer().clone(),
            ctx.ai.agent_tools.clone(),
            ctx.ai.embedder.clone(),
            ctx.memory_vectors.clone(),
            ctx.chat_vectors.clone(),
            ctx.episode_vectors.clone(),
        )
    };
    // Конфиг агента из local.json (тот же источник, что open_vault/agentd): дефолт-OFF актуатора живёт
    // здесь. Нет/битый → AiConfig-дефолты (actuator OFF). Читаем ПОСЛЕ освобождения read-гарда.
    let cfg = load_local_config(&root).await;

    // AGENT-1: tool-провайдер цикла. Десктоп держит `ai.agent_tools=None` (он строится только тут /
    // в agentd — I-5). Строим через ОБЩИЙ ядровой строитель (whitelisted дом типа): тот же
    // GuardedClient::for_chat + EgressFeature::Chat, что и chat — НЕТ нового egress-пути. Нет ai.chat
    // → None → деградируем: прогон финишируется error ("agent tools unavailable") как в agentd.
    // Если десктоп уже держит провайдер в AIClient (обычно None — строится только тут / в agentd),
    // уважаем его; иначе строим из конфига общим ядровым строителем.
    let provider = agent_tools.or_else(|| {
        cfg.as_ref().and_then(|c| {
            nexus_core::ai::tools::build_agent_tool_provider(
                c,
                &state.egress_policy,
                &state.egress_audit,
            )
        })
    });

    // Параметры гейта актуатора из конфига — ДЕФОЛТ-OFF (флаг отсутствует/false → стабы). НЕ меняем дефолт.
    let actuator_enabled = cfg
        .as_ref()
        .map(|c| c.ai.agent_actuator_enabled)
        .unwrap_or(false);
    let overwrite_threshold = cfg
        .as_ref()
        .and_then(|c| c.ai.agent_overwrite_threshold)
        .unwrap_or(actuator::OVERWRITE_THRESHOLD);
    let blast_cap = cfg
        .as_ref()
        .and_then(|c| c.ai.agent_blast_radius_cap)
        .unwrap_or(nexus_core::ai::AiConfig::DEFAULT_BLAST_RADIUS_CAP);
    let context_window = cfg
        .as_ref()
        .and_then(|c| c.ai.chat.as_ref())
        .and_then(|c| c.context_window);

    // Создаём строку прогона (queued) — источник run_id для UI/корреляции/ledger.
    let run_id = run_store::create_run(
        &writer,
        &task,
        provider.as_ref().map(|p| p.model_id()),
        Some(autonomy),
    )
    .await
    .map_err(|e| AppError::Msg(format!("agent_run: создание прогона: {e}")))?;

    // Per-run kill-switch (AGENT-5) + cancel-флаг + UI-DecisionSource (sender в реестр).
    let paused = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    let (decision_source, decision_tx): (Arc<dyn DecisionSource>, _) = {
        let (src, tx) = UiDecisionSource::new();
        (Arc::new(src), tx)
    };
    state.register_agent_run(
        run_id,
        AgentRunEntry {
            // decision-канал нужен только при ВКЛ актуаторе (стабы не предлагают). Но регистрируем
            // всегда для единообразия approve-команды; при OFF предложений не будет (gate не строится).
            decisions: Some(decision_tx),
            paused: paused.clone(),
            cancel: cancel.clone(),
        },
    );

    // Мост к памяти (AGENT-MEM-1): degrade-safe (None-эмбеддер/индексы → recall пуст). exclude_session
    // = None — прогон не привязан к чат-сессии.
    let agent_memory: Arc<dyn AgentMemory> = Arc::new(VaultAgentMemory::new(
        reader.clone(),
        writer.clone(),
        embedder.clone(),
        memory_vectors,
        chat_vectors,
        episode_vectors,
        None,
    ));

    // Канонизированный корень (предусловие гейта/apply). root из open_vault уже канонизирован.
    let canon_root = root.clone();

    // Спавним прогон. Возвращаем run_id СРАЗУ — цикл крутится в фоне, стримит в channel.
    // Tauri `State` не `Send` через границу `tokio::spawn`; забираем нужные Arc/handles ДО спавна
    // (как chat.rs клонирует провайдеры). Реестр прогонов — через клон `Arc` (дерегистрация в финале).
    let writer_for_loop = writer.clone();
    let reader_for_loop = reader.clone();
    let runs = state.agent_runs_handle();

    tokio::spawn(async move {
        let outcome = drive_run(
            run_id,
            task,
            autonomy,
            provider,
            actuator_enabled,
            overwrite_threshold,
            blast_cap,
            context_window,
            decision_source,
            agent_memory,
            canon_root,
            &writer_for_loop,
            &reader_for_loop,
            paused,
            cancel,
            &channel,
        )
        .await;
        // Финал в БД (run_store) + дерегистрация из реестра. Финал best-effort (наблюдаемость).
        finish_in_store(&writer_for_loop, run_id, outcome).await;
        if let Ok(mut g) = runs.lock() {
            g.remove(&run_id);
        }
    });

    Ok(run_id)
}

/// Кормит UI-DecisionSource прогона решениями фронта (Confirm-тир аппрув/реджект). Собирает
/// [`BatchDecision`] из пар `(action_id, approve)` и шлёт в decision-канал прогона — гейт, ждущий на
/// `decide()`, применит одобренные, отклонит прочие (отсутствующий айтем = Reject, fail-closed).
#[tauri::command]
pub async fn agent_approve(
    state: State<'_, AppState>,
    run_id: i64,
    decisions: Vec<ApprovalDecision>,
) -> AppResult<()> {
    let Some(tx) = state.agent_decision_sender(run_id) else {
        return Err(AppError::Msg(format!(
            "agent_approve: прогон {run_id} не активен (нет в реестре)"
        )));
    };
    let batch = BatchDecision::from_pairs(decisions.into_iter().map(|d| {
        (
            d.action_id,
            if d.approve {
                ItemDecision::Approve
            } else {
                ItemDecision::Reject
            },
        )
    }));
    tx.send(batch)
        .await
        .map_err(|_| AppError::Msg(format!("agent_approve: канал прогона {run_id} закрыт")))?;
    Ok(())
}

/// Ставит прогон на паузу (AGENT-5 kill-switch): цикл останавливается на следующей границе хода,
/// гейт актуатора не пишет под паузой. Прогон НЕ снимается из реестра (resume его возобновит в рамках
/// текущего цикла — пауза проверяется fail-safe на каждом шаге).
#[tauri::command]
pub async fn agent_pause(state: State<'_, AppState>, run_id: i64) -> AppResult<()> {
    if state.set_agent_paused(run_id, true) {
        Ok(())
    } else {
        Err(AppError::Msg(format!(
            "agent_pause: прогон {run_id} не активен"
        )))
    }
}

/// Снимает паузу прогона (AGENT-5). Если цикл ещё крутится (пауза проверяется между ходами), он
/// продолжит со следующего хода.
#[tauri::command]
pub async fn agent_resume(state: State<'_, AppState>, run_id: i64) -> AppResult<()> {
    if state.set_agent_paused(run_id, false) {
        Ok(())
    } else {
        Err(AppError::Msg(format!(
            "agent_resume: прогон {run_id} не активен"
        )))
    }
}

/// Кооперативно отменяет прогон: взводит cancel-флаг, цикл завершится `cancelled` на следующей границе
/// хода (партиал не теряется — он в outcome).
#[tauri::command]
pub async fn agent_cancel(state: State<'_, AppState>, run_id: i64) -> AppResult<()> {
    if state.cancel_agent_run(run_id) {
        Ok(())
    } else {
        Err(AppError::Msg(format!(
            "agent_cancel: прогон {run_id} не активен"
        )))
    }
}

/// Откатывает применённые действия прогона (AGENT-4): идёт по `agent_actions` прогона NEWEST-FIRST через
/// `actuator::undo_run` и восстанавливает каждое. Идемпотентно (повтор — no-op). Возвращает число
/// откаченных действий. Требует открытого vault (канонизированный корень — предусловие apply-рубежа).
#[tauri::command]
pub async fn agent_undo(state: State<'_, AppState>, run_id: i64) -> AppResult<usize> {
    let (canon_root, writer, reader) = {
        let ctx = state.vault().await?;
        (
            ctx.root.clone(),
            ctx.db.writer().clone(),
            ctx.db.reader().clone(),
        )
    };
    // ledger-обёртка над тем же writer/reader, что и прогон — undo_run читает executed-строки прогона.
    let ledger = AuditSink::new(writer, reader);
    let outcome = actuator::undo_run(run_id, &canon_root, &ledger).await;
    Ok(outcome.restored())
}

// ── Драйв цикла (spawned task) ────────────────────────────────────────────────────────────────────

/// Гонит [`run_agent_loop`] для прогона `run_id`, форвардя события в `channel`. Зеркало
/// [`agent::AgentRunHandler::drive`] по сборке входов/реестра, но цикл гоняется НАПРЯМУЮ (нам нужен
/// контроль `on_event` для стрима). Возвращает [`LoopOutcome`] для финализации в run_store.
#[allow(clippy::too_many_arguments)]
async fn drive_run(
    run_id: i64,
    task: String,
    autonomy: &'static str,
    provider: Option<Arc<dyn nexus_core::ai::tools::ToolCapableProvider>>,
    actuator_enabled: bool,
    overwrite_threshold: usize,
    blast_cap: u32,
    context_window: Option<usize>,
    decision_source: Arc<dyn DecisionSource>,
    memory: Arc<dyn AgentMemory>,
    canon_root: PathBuf,
    writer: &nexus_core::db::WriteActor,
    reader: &nexus_core::db::ReadPool,
    paused: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    channel: &Channel<AgentStreamEvent>,
) -> LoopOutcome {
    // mark_running (heartbeat для crash-recovery TTL); ошибка — продолжаем (наблюдаемость, не корректность).
    let _ = run_store::mark_running(writer, run_id).await;

    // Нет провайдера → деградируем чисто (как agentd): error-терминал, lifecycle доказан.
    let Some(provider) = provider else {
        let msg = "agent tools unavailable";
        let _ = channel.send(AgentStreamEvent::Error {
            message: msg.to_string(),
        });
        return LoopOutcome::Error(msg.to_string());
    };

    // Прогон через ЕДИНУЮ ядровую композицию [`run_agent_session`] (DRY: тот же код у agentd/коннектора).
    // Форвардер `ChannelForwarder` стримит и события цикла, и Proposal/Diff гейта в один Channel (фронт
    // видит changeset ДО решения; гейт блокируется на UI-DecisionSource, ожидая agent_approve). Реестр/
    // recall/скиллы/budget — внутри сессии (actuator default-OFF → стабы, vault не трогается). Skills у
    // desktop пока нет (None). `RunCtx::run(run_id)` строит сама сессия.
    let forwarder: Arc<dyn AgentEventForwarder> = Arc::new(ChannelForwarder {
        channel: channel.clone(),
    });
    let spec = SessionSpec {
        run_id,
        task,
        autonomy: Some(autonomy.to_string()),
        actuator_enabled,
        overwrite_threshold,
        blast_cap,
        context_window,
        canon_root,
    };
    run_agent_session(
        &spec,
        provider.as_ref(),
        Some(memory.as_ref()),
        None, // skills (desktop без скиллов пока)
        None, // EGR-AGENT web-инструменты — активация в отдельном срезе
        decision_source,
        writer,
        reader,
        &paused,
        &cancel,
        forwarder,
    )
    .await
}

/// Финализирует прогон в run_store по исходу цикла (зеркало терминала `AgentRunHandler::drive`):
/// Final→done, Cancelled→cancelled, прочее исчерпание бюджета→error, Error→error. Пауза мид-ран
/// (BudgetExhausted{Paused}) здесь трактуется как НЕ-терминал в desktop-модели: цикл драйвится единым
/// `tokio::spawn` (не реквью планировщика) — если пауза остановила цикл, мы помечаем прогон error с
/// пометкой паузы (UI может перезапустить). Это desktop-упрощение vs agentd-requeue (план планировщика).
async fn finish_in_store(writer: &nexus_core::db::WriteActor, run_id: i64, outcome: LoopOutcome) {
    use nexus_core::agent::run_store::{STATUS_CANCELLED, STATUS_DONE, STATUS_ERROR};
    use nexus_core::agent::BudgetKind;
    let (status, text) = match outcome {
        LoopOutcome::Final(s) => (STATUS_DONE, s),
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
        LoopOutcome::Error(e) => (STATUS_ERROR, e),
    };
    let _ = run_store::finish_run(writer, run_id, status, Some(&text)).await;
}

// ── Вспомогательное ───────────────────────────────────────────────────────────────────────────────

/// Читает/парсит `.nexus/local.json` (зеркало `vault::load_local_config`/`agentd::load_local_config`).
/// `None` — нет/битый (агент стартует на AiConfig-дефолтах: actuator OFF).
async fn load_local_config(root: &std::path::Path) -> Option<nexus_core::ai::LocalConfig> {
    let raw = tokio::fs::read_to_string(root.join(".nexus").join("local.json"))
        .await
        .ok()?;
    nexus_core::ai::LocalConfig::parse(&raw)
        .map_err(|e| tracing::warn!(error = %e, "agent_run: local.json не распарсен — дефолты"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::agent::tool::{ToolCall, ToolSpec};
    use nexus_core::ai::tools::{ToolCapableProvider, ToolTurn};
    use nexus_core::ai::{AiResult, ChatMessage};
    use nexus_core::db::Database;
    use nexus_core::net::RunCtx;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // ── Тест-коллектор Channel: собирает отправленные события как parsed JSON ──────────────────────

    /// Строит `Channel<AgentStreamEvent>`, складывающий КАЖДОЕ отправленное событие как `serde_json::
    /// Value` в общий `Vec` (тот же путь, что Tauri: `send` сериализует через `IpcResponse`). Возврат —
    /// (channel, общий буфер). Так офлайн-тест проверяет ТОЧНЫЙ JSON-контракт, который увидит UI-1b.
    fn collector_channel() -> (
        Channel<AgentStreamEvent>,
        Arc<Mutex<Vec<serde_json::Value>>>,
    ) {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let sink = buf.clone();
        let channel = Channel::new(move |body: tauri::ipc::InvokeResponseBody| {
            if let tauri::ipc::InvokeResponseBody::Json(s) = body {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                    sink.lock().unwrap().push(v);
                }
            }
            Ok(())
        });
        (channel, buf)
    }

    /// Фейк tool-capable провайдер: отдаёт скриптованную последовательность ходов (как runner-тесты).
    struct FakeProvider {
        turns: Mutex<VecDeque<AiResult<ToolTurn>>>,
    }
    impl FakeProvider {
        fn new(turns: Vec<AiResult<ToolTurn>>) -> Arc<Self> {
            Arc::new(Self {
                turns: Mutex::new(turns.into_iter().collect()),
            })
        }
    }
    #[async_trait]
    impl ToolCapableProvider for FakeProvider {
        async fn stream_chat_tools(
            &self,
            _m: &[ChatMessage],
            _t: &[ToolSpec],
            _o: &mut (dyn FnMut(String) + Send),
            _c: &Arc<AtomicBool>,
            _ctx: RunCtx,
        ) -> AiResult<ToolTurn> {
            self.turns
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(ToolTurn::Final("ok".into())))
        }
        fn model_id(&self) -> &str {
            "fake"
        }
    }

    async fn open_db() -> (TempDir, Database, PathBuf) {
        let dir = TempDir::new().unwrap();
        // canon_root КАНОНИЗИРОВАН — предусловие гейта/apply (macOS /tmp → /private/tmp).
        let canon = dir.path().canonicalize().unwrap();
        let db = Database::open(canon.join(".nexus").join("nexus.db"))
            .await
            .unwrap();
        (dir, db, canon)
    }

    /// Пустая память (recall → пусто): тот же эффект, что VaultAgentMemory без эмбеддера/индексов.
    fn empty_memory(db: &Database) -> Arc<dyn AgentMemory> {
        Arc::new(VaultAgentMemory::new(
            db.reader().clone(),
            db.writer().clone(),
            None,
            None,
            None,
            None,
            None,
        ))
    }

    fn type_of(v: &serde_json::Value) -> &str {
        v.get("type").and_then(|t| t.as_str()).unwrap_or("?")
    }

    // ── 1. Маппинг From<&AgentEvent> → AgentStreamEvent ───────────────────────────────────────────
    // Юниты на КАЖДЫЙ вариант DTO + roundtrip живут у ЕДИНОГО источника контракта
    // (`nexus_core::agent::connect::wire`), чтобы desktop и agentd не разъехались. Здесь — только
    // desktop-специфика: drive_run/approve гонят РЕАЛЬНЫЙ EventSink→Channel поверх re-export'нутого
    // `map_agent_event`, что заодно доказывает, что путь маппинга из desktop работает end-to-end.

    // ── 2. Смоук: drive_run против фейк-провайдера (стабы) → Channel получает ToolCall/Result/Final ─

    /// КЛЮЧЕВОЕ ДОКАЗАТЕЛЬСТВО (offline, как agentd `agent_loop_smoke`): фейк-провайдер возвращает
    /// ToolCalls([echo]) на ходу 1, Final на ходу 2. `drive_run` (actuator ВЫКЛ → стабы) гонит цикл и
    /// форвардит события в наш collector-Channel. Проверяем: поток несёт toolCall → toolResult → final
    /// ПО ПОРЯДКУ + хотя бы один contextUsage; исход done. Сети/модели нет.
    #[tokio::test]
    async fn drive_run_streams_toolcall_result_final_in_order() {
        let (_dir, db, canon) = open_db().await;
        let provider = FakeProvider::new(vec![
            Ok(ToolTurn::ToolCalls(vec![ToolCall {
                id: "c1".into(),
                name: "debug.echo".into(),
                arguments: r#"{"text":"привет"}"#.into(),
            }])),
            Ok(ToolTurn::Final("готово".into())),
        ]);
        let (channel, buf) = collector_channel();
        let (decision, _tx) = UiDecisionSource::new();

        let outcome = drive_run(
            1,
            "smoke: позови echo".into(),
            "auto",
            Some(provider),
            false, // actuator ВЫКЛ → стабы (vault не трогается)
            64 * 1024,
            16,
            Some(32768),
            Arc::new(decision),
            empty_memory(&db),
            canon,
            db.writer(),
            db.reader(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            &channel,
        )
        .await;

        assert_eq!(outcome, LoopOutcome::Final("готово".into()));

        let events = buf.lock().unwrap().clone();
        let pos = |ty: &str| events.iter().position(|v| type_of(v) == ty);
        let p_call = pos("toolCall").expect("есть toolCall");
        let p_res = pos("toolResult").expect("есть toolResult");
        let p_final = pos("final").expect("есть final");
        assert!(p_call < p_res, "toolCall раньше toolResult");
        assert!(p_res < p_final, "toolResult раньше final");
        assert!(
            events.iter().any(|v| type_of(v) == "contextUsage"),
            "есть хотя бы один contextUsage"
        );
        // Корреляция call↔result по id + содержимое echo.
        let call = events.iter().find(|v| type_of(v) == "toolCall").unwrap();
        let res = events.iter().find(|v| type_of(v) == "toolResult").unwrap();
        assert_eq!(call["id"], "c1");
        assert_eq!(res["id"], "c1");
        assert_eq!(res["isError"], false);
    }

    /// Деградация: провайдер None → стрим error("agent tools unavailable"), исход Error (как agentd).
    #[tokio::test]
    async fn drive_run_without_provider_streams_error() {
        let (_dir, db, canon) = open_db().await;
        let (channel, buf) = collector_channel();
        let (decision, _tx) = UiDecisionSource::new();
        let run_id = run_store::create_run(db.writer(), "t", None, Some("auto"))
            .await
            .unwrap();
        let outcome = drive_run(
            run_id,
            "t".into(),
            "auto",
            None,
            false,
            64 * 1024,
            16,
            Some(32768),
            Arc::new(decision),
            empty_memory(&db),
            canon,
            db.writer(),
            db.reader(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            &channel,
        )
        .await;
        assert!(matches!(outcome, LoopOutcome::Error(_)));
        let events = buf.lock().unwrap().clone();
        assert!(events.iter().any(|v| type_of(v) == "error"));
    }

    // ── 3. DecisionSource: approve применяет Confirm-айтем; без approve — fail-closed (не применяется)

    /// Скрипт «note.create rel=Notes/Gate.md, затем Final» для actuator-теста (один note.create).
    fn note_create_then_final(rel: &str, content: &str) -> Arc<FakeProvider> {
        let args = format!(r#"{{"path":"{rel}","content":"{content}"}}"#);
        FakeProvider::new(vec![
            Ok(ToolTurn::ToolCalls(vec![ToolCall {
                id: "n1".into(),
                name: "note.create".into(),
                arguments: args,
            }])),
            Ok(ToolTurn::Final("готово".into())),
        ])
    }

    /// **APPROVE → APPLY.** Actuator ВКЛ + autonomy=confirm → note.create ПРЕДЛАГАЕТСЯ (Proposal в
    /// стрим), гейт ждёт решения. Кормим Approve через decision-sender (как `agent_approve`) → файл
    /// записан, ledger executed, исход done. Полностью офлайн (фейк-провайдер). Доказывает живой
    /// человек-в-петле путь Proposal → approve → apply.
    #[tokio::test]
    async fn approve_applies_confirm_item() {
        let (_dir, db, canon) = open_db().await;
        let provider = note_create_then_final("Notes/Gate.md", "создано аппрувом");
        let (channel, buf) = collector_channel();
        let (decision, tx): (Arc<dyn DecisionSource>, _) = {
            let (s, t) = UiDecisionSource::new();
            (Arc::new(s), t)
        };

        // Кормим Approve в фоне: ждём, что гейт спросит decide() и снимет решение из канала. Решение
        // адресуем action_id'у, который придёт в Proposal-событии — но т.к. это первая (и единственная)
        // строка предложения, её action_id известен заранее НЕ будет; поэтому approve ВСЕХ присланных
        // батчей: читаем action_id из Proposal-события буфера. Проще — слать Approve по факту Proposal.
        let buf_for_approver = buf.clone();
        let approver = tokio::spawn(async move {
            // Поллим буфер, пока не увидим Proposal с action_id, затем шлём Approve этому id.
            loop {
                let action_id = {
                    let g = buf_for_approver.lock().unwrap();
                    g.iter()
                        .find(|v| type_of(v) == "proposal")
                        .and_then(|v| v["files"][0]["actionId"].as_i64())
                };
                if let Some(id) = action_id {
                    let _ = tx
                        .send(BatchDecision::from_pairs([(id, ItemDecision::Approve)]))
                        .await;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        });

        let outcome = drive_run(
            1,
            "создай заметку".into(),
            "confirm", // confirm-прогон → даже Auto-тир note.create предлагается
            Some(provider),
            true, // actuator ВКЛ (go-live, тестовый temp-vault)
            64 * 1024,
            16,
            Some(32768),
            decision,
            empty_memory(&db),
            canon.clone(),
            db.writer(),
            db.reader(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            &channel,
        )
        .await;
        approver.await.unwrap();

        assert_eq!(outcome, LoopOutcome::Final("готово".into()));
        // Файл реально записан ЧЕРЕЗ ГЕЙТ (Approve применил Confirm-айтем).
        let written = std::fs::read_to_string(canon.join("Notes/Gate.md")).ok();
        assert_eq!(written.as_deref(), Some("создано аппрувом"));
        // Поверхность аппрува стримилась во фронт: Proposal присутствует.
        let events = buf.lock().unwrap().clone();
        assert!(
            events.iter().any(|v| type_of(v) == "proposal"),
            "Proposal стримлен во фронт"
        );
    }

    /// **БЕЗ APPROVE → FAIL-CLOSED (не применяется).** Тот же путь, но decision-sender ДРОПНУТ (фронт
    /// ушёл, не ответив) → UiDecisionSource.decide возвращает reject_all → note.create НЕ применяется,
    /// файл НЕ создан. Доказывает fail-closed: нет явного Approve ⇒ диск не тронут.
    #[tokio::test]
    async fn no_approve_is_fail_closed_not_applied() {
        let (_dir, db, canon) = open_db().await;
        let provider = note_create_then_final("Notes/NoApprove.md", "не должно записаться");
        let (channel, _buf) = collector_channel();
        let (decision, tx): (Arc<dyn DecisionSource>, _) = {
            let (s, t) = UiDecisionSource::new();
            (Arc::new(s), t)
        };
        // Дропаем sender — решатель «ушёл, не ответив»: decide() ⇒ reject_all (fail-closed).
        drop(tx);

        let outcome = drive_run(
            1,
            "создай заметку".into(),
            "confirm",
            Some(provider),
            true,
            64 * 1024,
            16,
            Some(32768),
            decision,
            empty_memory(&db),
            canon.clone(),
            db.writer(),
            db.reader(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            &channel,
        )
        .await;

        // Цикл доходит до Final (модель «закончила»), но note.create был ОТКЛОНЁН → файла нет.
        assert_eq!(outcome, LoopOutcome::Final("готово".into()));
        assert!(
            !canon.join("Notes/NoApprove.md").exists(),
            "без Approve файл НЕ записан (fail-closed)"
        );
    }
}
