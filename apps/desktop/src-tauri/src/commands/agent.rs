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
//! actuator default-OFF (`ai.agent_actuator_enabled` нет/false → ПУСТОЙ реестр записи (B7), реальный
//! vault НЕ трогается); ВКЛ → гейтнутые инструменты-актуаторы за `actuator::dispatch_action` (тот же
//! гейт, тот же `GuardedClient`). РАЗНИЦА с agentd: agentd гоняет цикл ВНУТРИ `AgentRunHandler::handle`
//! (его `on_event` внутренний — `FIXME(UI-1)`); здесь мы гоним [`run_agent_loop`] НАПРЯМУЮ, чтобы
//! контролировать `on_event` → стрим в Channel в реальном времени. И UI-DecisionSource заменяет
//! headless `PolicyDefault` (auto-DENY): Confirm-тир реально аппрувится из UI (человек-в-петле).
//!
//! # Границы (СОХРАНЕНЫ)
//! - Actuator default OFF — дефолт НЕ меняется (флаг конфига, как в agentd).
//! - Эгресс/актуатор — через существующие гейты (`GuardedClient`/`dispatch_action`). chat: tool-провайдер
//!   строит `nexus_core::ai::tools::build_agent_tool_provider` — тот же `GuardedClient::for_chat` +
//!   `EgressFeature::Chat`, что и обычный chat.
//! - AGENT-0.2: web-инструменты агента (`web.search`/`web.fetch`) — default-OFF (`ai.web.enabled`);
//!   тот же `enable_web_tools`/`GuardedClient::for_web`/SSRF-гейт, что у agentd, НО на ИЗОЛИРОВАННОЙ
//!   `EgressPolicy` (делит лишь offline-kill-switch) — согласие агент-web НЕ протекает в Home-websearch/
//!   новости (общий глобальный policy). skills — read-only каталог из `ai.agent_skills_dir`.
//! - Переиспользуем ядро: `AgentRunHandler`-композицию (реестр/бюджет/токенайзер/память),
//!   `run_agent_loop`, `undo_run`, `DecisionSource`/`BatchDecision` — НЕ копируем логику.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use tauri::ipc::Channel;
use tauri::State;

use std::collections::HashMap;
use std::sync::Mutex;

use nexus_core::actuator::{
    self, AuditSink, BatchDecision, DecisionSource, ItemDecision, ProposalBatch,
};
use nexus_core::agent::run_store::PersistStep;
use nexus_core::agent::{
    run_agent_session_bounded, run_store, AgentEvent, AgentEventForwarder, AgentMemory,
    DelegationDeps, LoopBounds, LoopOutcome, SessionDeps, SessionRole, SessionSpec,
    VaultAgentMemory,
};
use nexus_core::ai::ChatMessage;

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

/// W-38: аккумулятор хода для персиста истории переписок. Копит склеенный текст ассистента и ленту
/// шагов (tool-вызовы + их результаты по `id`) ПО ХОДУ стрима; `persist_turn` пишет его на ТЕРМИНАЛЕ
/// (после `finish_run`). `index_by_id` сопоставляет `ToolCall.id` → позицию в `steps` (результат
/// приходит позже отдельным событием). Не персистим live-построчно (одна запись на финал — best-effort).
#[derive(Default)]
struct TurnAccum {
    text: String,
    steps: Vec<PersistStep>,
    /// `ToolCall.id` → индекс в `steps` (для проставления result/is_error из `ToolResult`).
    index_by_id: HashMap<String, usize>,
}

/// [`AgentEventForwarder`]-мост прогона → агент-стрим во фронт. ЕДИНЫЙ форвардер для desktop: его
/// получает и `run_agent_session` (события цикла), и гейт актуатора (Proposal/Diff — через внутренний
/// `ForwardingEventSink`). Маппит `AgentEvent` → wire-DTO и шлёт в [`Channel`] (best-effort: фронт мог
/// отвалиться). Гейт блокируется на `DecisionSource::decide`, ожидая `agent_approve` (человек-в-петле).
/// Headless agentd вместо этого считает шаги + tracing-логирует (см. `agent::job::HeadlessForwarder`).
///
/// W-38: помимо форварда в Channel, КОПИТ ход в `accum` (текст ассистента + шаги) для персиста истории
/// на терминале. Аккумуляция — best-effort и НЕ влияет на стрим (mutex поверх копилки; ошибок не несёт).
struct ChannelForwarder {
    channel: Channel<AgentStreamEvent>,
    accum: Arc<Mutex<TurnAccum>>,
}

impl AgentEventForwarder for ChannelForwarder {
    fn forward(&self, ev: &AgentEvent) {
        // W-38: копим ход для персиста (до маппинга — внутренний AgentEvent несёт точные поля). Держим
        // std::sync::Mutex БЕЗ `.await` под гардом (forward синхронный) → clippy await_holding_lock чист.
        if let Ok(mut acc) = self.accum.lock() {
            match ev {
                AgentEvent::AssistantToken(t) => acc.text.push_str(t),
                AgentEvent::ToolCall { id, kind, args } => {
                    let idx = acc.steps.len();
                    acc.steps.push(PersistStep {
                        ord: idx as i64,
                        kind: kind.clone(),
                        args: args.clone(),
                        title: None,
                        result: None,
                        is_error: false,
                    });
                    acc.index_by_id.insert(id.clone(), idx);
                }
                AgentEvent::ToolResult {
                    id,
                    content,
                    is_error,
                } => {
                    if let Some(&idx) = acc.index_by_id.get(id) {
                        if let Some(step) = acc.steps.get_mut(idx) {
                            step.result = Some(content.clone());
                            step.is_error = *is_error;
                        }
                    }
                }
                _ => {}
            }
        }
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
/// бюджет, реестр (ПУСТОЙ при выключенном актуаторе [дефолт, B7], гейтнутые инструменты при ВКЛ), память
/// (recall + Add-only запись), UI-DecisionSource, per-run kill-switch.
/// W-4: элемент истории переписки из десктоп-чата (мультитёрн). `role` — `"assistant"` → assistant-
/// сообщение, иначе user. Фронт шлёт прошлые ходы, чтобы follow-up продолжал работу, см. SessionSpec.
#[derive(serde::Deserialize)]
pub struct HistoryMsg {
    pub role: String,
    pub text: String,
}

/// CONN-1: тонкий шим — делегирует активному [`AgentBackend`] (по умолчанию `EmbeddedBackend` =
/// сегодняшний in-process путь, байт-в-байт). Имя/параметры/возврат команды неизменны (фронт-контракт цел).
#[tauri::command]
pub async fn agent_run(
    state: State<'_, AppState>,
    task: String,
    autonomy: String,
    history: Vec<HistoryMsg>,
    // W-38: id переписки (генерится фронт-стором) — группирует ходы для истории. Опционален для
    // обратной совместимости с тестами/старым фронтом (отсутствует → пустая строка → не персистим).
    session_id: Option<String>,
    channel: Channel<AgentStreamEvent>,
) -> AppResult<i64> {
    state
        .agent_backend()
        .await
        .run(
            &state,
            task,
            autonomy,
            history,
            session_id.unwrap_or_default(),
            channel,
        )
        .await
}

/// EMBEDDED-реализация `agent_run` (CONN-1): прежнее тело команды без изменений (только `State`→`&AppState`).
/// Зовётся из [`crate::agent_backend::EmbeddedBackend::run`].
pub(crate) async fn run_impl(
    state: &AppState,
    task: String,
    autonomy: String,
    // W-4: история прошлых ходов сессии (из стора `turns[]`); фронт всегда шлёт (пустой массив для
    // первого хода).
    history: Vec<HistoryMsg>,
    // W-38: id переписки (история). Пусто → ход НЕ персистится (не-UI путь); непусто → group-ключ
    // `agent_runs.session_id` + `agent_turns.session_id` для левого сайдбара истории.
    session_id: String,
    channel: Channel<AgentStreamEvent>,
) -> AppResult<i64> {
    let autonomy = normalize_autonomy(&autonomy);
    // W-4: история в ChatMessage (пустые пропускаем); вставится перед текущей задачей в run_agent_session.
    let history: Vec<ChatMessage> = history
        .into_iter()
        .filter(|m| !m.text.trim().is_empty())
        .map(|m| {
            if m.role == "assistant" {
                ChatMessage::assistant(m.text)
            } else {
                ChatMessage::user(m.text)
            }
        })
        .collect();

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
    // Конфиг агента из local.json (тот же источник, что open_vault/agentd — канон R-3b): дефолт-OFF
    // актуатора живёт здесь. Нет/битый → AiConfig-дефолты (actuator OFF). ПОСЛЕ освобождения read-гарда.
    let cfg = crate::bootstrap::load_local_config(&root).await;

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

    // Параметры гейта актуатора из конфига — ДЕФОЛТ-OFF (флаг отсутствует/false → без инструментов
    // записи). НЕ меняем дефолт.
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
    // BF-1 (хвост #519): границы прогона (wall_clock/max_steps) из ai.agent_wall_clock_secs/
    // ai.agent_max_steps (клампятся в AiConfig). Нет конфига/ключей → LoopBounds::default (байт-прежнее).
    let loop_bounds = cfg
        .as_ref()
        .map(|c| LoopBounds::from_ai_config(&c.ai))
        .unwrap_or_default();

    // AGENT-0.2: веб-инструменты агента (web.search/web.fetch). ВКЛ только при `ai.web.enabled` И
    // непустом url. Default-OFF: нет секции / enabled=false / пустой url → None (агент без веб, без
    // регрессии). Строим ДО спавна (нужен `state.egress_*`, не `Send`), результат (Arc внутри) → в задачу.
    //
    // ВАЖНО (отличие от agentd): `enable_web_tools` МУТИРУЕТ переданный policy (scope "web" allowlist +
    // `web_allow_public` + feature `Web`). В agentd policy ничего больше не трогает (sync раз на старте).
    // В десктопе ТОТ ЖЕ глобальный `state.egress_policy` используют Home-websearch и новости (через
    // `*::sync_egress_policy`, тоже scope "web"/feature Web). Чтобы ВКЛ агент-web НЕ протекал в их
    // согласие (не клобберил хост, не оставлял Web/allow_public глобально ВКЛ) — строим веб-клиент агента
    // на ОТДЕЛЬНОЙ `EgressPolicy`, разделяющей лишь offline-kill-switch. Тот же SSRF/deny_private/resolver
    // (дефолтный, как у глобальной) + общий durable-audit. Изоляция согласия агента от Home-веба.
    let agent_web = cfg
        .as_ref()
        .and_then(|c| c.ai.web.as_ref())
        .filter(|w| w.enabled && !w.url.trim().is_empty())
        .and_then(|w| {
            let web_policy = Arc::new(crate::net::EgressPolicy::new(state.egress_offline.clone()));
            nexus_core::agent::enable_web_tools(
                &web_policy,
                &state.egress_audit,
                &w.url,
                std::time::Duration::from_secs(20),
                w.allow_public_fetch,
            )
        });

    // AGENT-0.2: навыки (SKILL.md) из `ai.agent_skills_dir` (относительный путь — от корня vault).
    // Канонизируем КАТАЛОГ (fail-closed: недоступен → None); пустой каталог → None (агент без навыков).
    // `usage_writer` = телеметрия использования (SL-2). Зеркало agentd `build_skill_context`.
    let agent_skills = cfg
        .as_ref()
        .and_then(|c| c.ai.agent_skills_dir.as_deref())
        .and_then(|dir| {
            let p = std::path::Path::new(dir);
            let abs = if p.is_absolute() {
                p.to_path_buf()
            } else {
                root.join(p)
            };
            let canon = abs.canonicalize().ok()?;
            let catalog = nexus_core::skills::discover_skills(&canon);
            if !catalog.errors().is_empty() {
                tracing::warn!(
                    count = catalog.errors().len(),
                    "skills: часть SKILL.md не распарсилась — пропущены (см. errors)"
                );
            }
            if catalog.is_empty() {
                return None;
            }
            Some(
                nexus_core::agent::SkillContext::new(std::sync::Arc::new(catalog), canon)
                    .with_usage_writer(writer.clone()),
            )
        });

    // SL-7: авторство навыков (skill.save) — ТОЛЬКО при `ai.skills.learning_enabled` (owner-gated,
    // default-OFF). Влияет на регистрацию skill.save внутри сессии.
    let skills_learning_enabled = cfg
        .as_ref()
        .map(|c| c.ai.skills.learning_enabled)
        .unwrap_or(false);

    // W-24: owner-gated делегирование (ai.delegation, default-OFF). DelegationDeps ТОЛЬКО для TOP-LEVEL
    // (desktop agent_run всегда top-level — subagent=None ниже); дети делегировать не могут (рекурсия-стоп).
    // Some только при enabled И наличии провайдера (без него цикл и так деградирует error-терминалом).
    // Клонируем Arc провайдера ДО его move в drive_run. Субагенты наследуют actuator-постуру родителя
    // (при OFF — read-only, без инструментов записи), флаги независимы.
    let delegation: Option<DelegationDeps> = cfg
        .as_ref()
        .map(|c| c.ai.delegation.clone())
        .filter(|d| d.enabled)
        .and_then(|config| {
            provider.clone().map(|p| DelegationDeps {
                provider: p,
                config,
            })
        });

    // W-25: owner-gated deep-research (ai.research, default-OFF). Передаём конфиг ТОЛЬКО при enabled;
    // research.run регистрируется в session.rs лишь при research+delegation+web+actuator+top-level
    // (любой co-requisite OFF → инструмента нет, без регрессии). Зеркало agentd.
    let research: Option<nexus_core::ai::ResearchConfig> = cfg
        .as_ref()
        .map(|c| c.ai.research.clone())
        .filter(|r| r.enabled);

    // Создаём строку прогона (queued) — источник run_id для UI/корреляции/ledger. W-38: при наличии
    // session_id привязываем прогон к переписке (история); пустой session_id (не-UI путь) → top-level
    // прогон без группировки (поведение прежнего create_run).
    let model_id = provider.as_ref().map(|p| p.model_id());
    let run_id = if session_id.is_empty() {
        run_store::create_run(&writer, &task, model_id, Some(autonomy)).await
    } else {
        run_store::create_run_in_session(&writer, &session_id, &task, model_id, Some(autonomy))
            .await
    }
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
            // decision-канал нужен только при ВКЛ актуаторе (без него предлагать нечему). Но регистрируем
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

    // W-38: аккумулятор хода (текст + шаги) для персиста истории на терминале. Создаём ДО спавна,
    // отдаём клон в drive_run (ChannelForwarder копит туда), читаем ПОСЛЕ финала. `task`/`session_id`
    // клонируем для персиста (task move'ится в drive_run).
    let accum: Arc<Mutex<TurnAccum>> = Arc::new(Mutex::new(TurnAccum::default()));
    let accum_for_loop = accum.clone();
    let task_for_persist = task.clone();

    tokio::spawn(async move {
        let outcome = drive_run(
            run_id,
            task,
            history,
            autonomy,
            provider,
            actuator_enabled,
            overwrite_threshold,
            blast_cap,
            context_window,
            loop_bounds,
            agent_web,
            agent_skills,
            skills_learning_enabled,
            delegation,
            research,
            decision_source,
            agent_memory,
            canon_root,
            accum_for_loop,
            &writer_for_loop,
            &reader_for_loop,
            paused,
            cancel,
            &channel,
        )
        .await;
        // Финал в БД (run_store) + дерегистрация из реестра. Финал best-effort (наблюдаемость).
        let (status, text) = finish_in_store(&writer_for_loop, run_id, outcome).await;
        // W-38: персист хода для истории переписок (best-effort — НЕ роняем прогон). Только UI-путь
        // (непустой session_id); status done|error|cancelled; report=текст для done, error=текст иначе.
        if !session_id.is_empty() {
            let (text_acc, steps) = match accum.lock() {
                Ok(g) => (g.text.clone(), g.steps.clone()),
                Err(_) => (String::new(), Vec::new()),
            };
            let is_done = status == run_store::STATUS_DONE;
            let report = is_done.then_some(text.as_str());
            let error = (!is_done).then_some(text.as_str());
            if let Err(e) = run_store::persist_turn(
                &writer_for_loop,
                run_id,
                &session_id,
                &task_for_persist,
                &text_acc,
                &steps,
                status,
                report,
                error,
                nexus_core::scheduler::now_secs(),
            )
            .await
            {
                tracing::warn!(error = %e, run_id, "W-38: персист хода истории не удался (игнор)");
            }
        }
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
    state
        .agent_backend()
        .await
        .approve(&state, run_id, decisions)
        .await
}

/// EMBEDDED-реализация `agent_approve` (CONN-1): прежнее тело (только `State`→`&AppState`).
pub(crate) async fn approve_impl(
    state: &AppState,
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
    state.agent_backend().await.pause(&state, run_id).await
}

/// EMBEDDED-реализация `agent_pause` (CONN-1).
pub(crate) async fn pause_impl(state: &AppState, run_id: i64) -> AppResult<()> {
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
    state.agent_backend().await.resume(&state, run_id).await
}

/// EMBEDDED-реализация `agent_resume` (CONN-1).
pub(crate) async fn resume_impl(state: &AppState, run_id: i64) -> AppResult<()> {
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
    state.agent_backend().await.cancel(&state, run_id).await
}

/// EMBEDDED-реализация `agent_cancel` (CONN-1).
pub(crate) async fn cancel_impl(state: &AppState, run_id: i64) -> AppResult<()> {
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
    state.agent_backend().await.undo(&state, run_id).await
}

/// EMBEDDED-реализация `agent_undo` (CONN-1): прежнее тело (только `State`→`&AppState`).
pub(crate) async fn undo_impl(state: &AppState, run_id: i64) -> AppResult<usize> {
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

// ── W-38: история переписок агента (левый сайдбар) ──────────────────────────────────────────────────

/// Сводка одной агент-сессии для списка истории (зеркало Rust `run_store::AgentSessionRow`). camelCase.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionDto {
    pub session_id: String,
    pub title: String,
    pub status: String,
    pub turn_count: i64,
    pub updated_at: i64,
}

/// Один персистированный шаг хода для UI (зеркало `run_store::PersistStep` без `ord`).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedStepDto {
    pub kind: String,
    pub args: String,
    pub title: Option<String>,
    pub result: Option<String>,
    pub is_error: bool,
}

/// Один персистированный ход переписки для UI (зеркало `run_store::PersistedTurnRow`).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedTurnDto {
    pub run_id: i64,
    pub task: String,
    pub assistant_text: String,
    pub report: Option<String>,
    pub error: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub steps: Vec<PersistedStepDto>,
}

/// Данные переоткрываемой переписки (ходы в хронологии ASC).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionDataDto {
    pub turns: Vec<PersistedTurnDto>,
}

// R-12 п.4: канон row→wire-DTO как `From`-импл (вместо инлайн-`.map`-пассивов в командах). Форма/serde
// DTO не меняются — контракт фронта тот же JSON; это лишь свод дублирующегося поле-в-поле копирования.
impl From<run_store::AgentSessionRow> for AgentSessionDto {
    fn from(r: run_store::AgentSessionRow) -> Self {
        Self {
            session_id: r.session_id,
            title: r.title,
            status: r.status,
            turn_count: r.turn_count,
            updated_at: r.updated_at,
        }
    }
}

impl From<run_store::PersistStep> for PersistedStepDto {
    fn from(s: run_store::PersistStep) -> Self {
        // `ord` (порядок в ходе) — служебное поле реконструкции ленты, в wire-DTO не выносится.
        Self {
            kind: s.kind,
            args: s.args,
            title: s.title,
            result: s.result,
            is_error: s.is_error,
        }
    }
}

impl From<run_store::PersistedTurnRow> for PersistedTurnDto {
    fn from(t: run_store::PersistedTurnRow) -> Self {
        Self {
            run_id: t.run_id,
            task: t.task,
            assistant_text: t.assistant_text,
            report: t.report,
            error: t.error,
            status: t.status,
            created_at: t.created_at,
            steps: t.steps.into_iter().map(Into::into).collect(),
        }
    }
}

/// W-38: список агент-сессий (история переписок) для левого сайдбара — свежие сверху.
#[tauri::command]
pub async fn agent_sessions_list(state: State<'_, AppState>) -> AppResult<Vec<AgentSessionDto>> {
    let reader = state.vault().await?.db.reader().clone();
    let rows = run_store::list_agent_sessions(&reader).await?;
    Ok(rows.into_iter().map(AgentSessionDto::from).collect())
}

/// W-38: загружает все ходы одной переписки (для переоткрытия в UI).
#[tauri::command]
pub async fn agent_session_load(
    state: State<'_, AppState>,
    session_id: String,
) -> AppResult<AgentSessionDataDto> {
    let reader = state.vault().await?.db.reader().clone();
    let rows = run_store::load_agent_session(&reader, &session_id).await?;
    let turns = rows.into_iter().map(PersistedTurnDto::from).collect();
    Ok(AgentSessionDataDto { turns })
}

// ── Драйв цикла (spawned task) ────────────────────────────────────────────────────────────────────

/// Гонит [`run_agent_loop`] для прогона `run_id`, форвардя события в `channel`. Зеркало
/// [`agent::AgentRunHandler::drive`] по сборке входов/реестра, но цикл гоняется НАПРЯМУЮ (нам нужен
/// контроль `on_event` для стрима). Возвращает [`LoopOutcome`] для финализации в run_store.
#[allow(clippy::too_many_arguments)]
async fn drive_run(
    run_id: i64,
    task: String,
    // W-4: история прошлых ходов сессии (мультитёрн) — в начальный контекст перед текущей задачей.
    history: Vec<ChatMessage>,
    autonomy: &'static str,
    provider: Option<Arc<dyn nexus_core::ai::tools::ToolCapableProvider>>,
    actuator_enabled: bool,
    overwrite_threshold: usize,
    blast_cap: u32,
    context_window: Option<usize>,
    // BF-1 (хвост #519): границы прогона (wall_clock/max_steps) из ai.agent_wall_clock_secs/ai.agent_max_steps.
    loop_bounds: LoopBounds,
    web: Option<nexus_core::agent::WebToolsConfig>,
    skills: Option<nexus_core::agent::SkillContext>,
    skills_learning_enabled: bool,
    // W-24: owner-gated делегирование (ai.delegation, default-OFF). Some → регистрируется delegate.run.
    delegation: Option<DelegationDeps>,
    // W-25: owner-gated deep-research (ai.research, default-OFF). Some → регистрируется research.run
    // (лишь при наличии delegation+web+actuator — см. session.rs).
    research: Option<nexus_core::ai::ResearchConfig>,
    decision_source: Arc<dyn DecisionSource>,
    memory: Arc<dyn AgentMemory>,
    canon_root: PathBuf,
    // W-38: копилка хода (текст + шаги) для персиста истории — ChannelForwarder пишет сюда по ходу.
    accum: Arc<Mutex<TurnAccum>>,
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
    // recall/скиллы/budget — внутри сессии (actuator default-OFF → пустой реестр записи, B7; vault не
    // трогается). Skills приходят параметром из `ai.agent_skills_dir` (None, если каталог не задан).
    // `RunCtx::run(run_id)` строит сама сессия.
    let forwarder: Arc<dyn AgentEventForwarder> = Arc::new(ChannelForwarder {
        channel: channel.clone(),
        accum,
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
        // W-4: история прошлых ходов сессии (десктоп-чат мультитёрный поверх one-shot прогонов).
        history,
        // SL-7: авторство навыков (skill.save) — только при ai.skills.learning_enabled (AGENT-0.2).
        skills_learning_enabled,
    };
    run_agent_session_bounded(
        &spec,
        &SessionDeps {
            provider: provider.as_ref(),
            memory: Some(memory.as_ref()),
            // AGENT-0.2: навыки из ai.agent_skills_dir (None если не задан/пуст).
            skills: skills.as_ref(),
            // AGENT-0.2: web.search/web.fetch при ai.web.enabled (None если выкл).
            web: web.as_ref(),
            decision_source,
            writer,
            reader,
            paused: &paused,
            cancel: &cancel,
            forwarder,
        },
        // Top-level desktop-прогон.
        SessionRole::TopLevel {
            // W-24: owner-gated делегирование (ai.delegation, default-OFF).
            delegation: delegation.as_ref(),
            // W-25: owner-gated deep-research (ai.research, default-OFF).
            research: research.as_ref(),
        },
        // BF-1: границы прогона из конфига (ai.agent_wall_clock_secs/ai.agent_max_steps).
        loop_bounds,
    )
    .await
}

/// Финализирует прогон в run_store по исходу цикла — маппинг статусов/текстов идёт КАНОНОМ R-2
/// (`nexus_core::agent::outcome_to_finish`, зеркало терминала `AgentRunHandler::drive`): Final→done,
/// Cancelled→cancelled («прогон отменён; …»), прочее исчерпание бюджета→error. Пауза мид-ран
/// (BudgetExhausted{Paused}) финализируется `PausePolicy::FinalizeError`: цикл драйвится единым
/// `tokio::spawn` (не реквью планировщика) — если пауза остановила цикл, помечаем прогон error с
/// пометкой паузы (UI может перезапустить). Это desktop-упрощение vs agentd-requeue (парковка
/// `PausePolicy::Requeue` — только у scheduler-пути).
/// Возвращает финальный `(status, text)` (W-38: используется и для персиста хода истории — report для
/// done, error-текст иначе).
async fn finish_in_store(
    writer: &nexus_core::db::WriteActor,
    run_id: i64,
    outcome: LoopOutcome,
) -> (&'static str, String) {
    use nexus_core::agent::{outcome_to_finish, CancelWording, PausePolicy};
    let (status, text) = outcome_to_finish(
        &outcome,
        PausePolicy::FinalizeError,
        CancelWording::RunCancelled,
    )
    .expect_finalize();
    let _ = run_store::finish_run(writer, run_id, status, Some(&text)).await;
    (status, text)
}

// ── Вспомогательное ───────────────────────────────────────────────────────────────────────────────
// Чтение/разбор `.nexus/local.json` — КАНОН `crate::bootstrap::load_local_config` (R-3b; бывшая
// локальная реплика с дрейфовавшим текстом warn-лога удалена, колл-сайты зовут канон напрямую).

// ── W-10: SL-панель (просмотр авто-навыков агента) ─────────────────────────────────────────────────

/// Навык для UI: данные с диска (SKILL.md) ЛЕВО-СОЕДИНЁННЫЕ с телеметрией БД (`agent_skill_usage`).
/// `state`/`pinned`/`createdBy` — `None`/0, если у навыка ещё нет строки телеметрии (создаётся лениво).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRowDto {
    name: String,
    description: String,
    /// `"vendor"` (hash-pinned bundle) | `"local"` (TrustedLocal — владельца/агента).
    tier: String,
    rel_path: String,
    is_vendor: bool,
    use_count: i64,
    last_used_at: Option<i64>,
    created_by: Option<String>,
    is_agent_created: bool,
    pinned: bool,
    /// `"active"|"stale"|"archived"` (advisory lifecycle), либо `None`.
    state: Option<String>,
    license: Option<String>,
}

/// Снимок для SL-панели: состояние самообучения + каталог навыков (или пусто/каталог не задан).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillListDto {
    learning_enabled: bool,
    skills_dir: Option<String>,
    skills: Vec<SkillRowDto>,
    /// Сколько SKILL.md не распознано (для честной пометки в UI, как у news).
    parse_errors: usize,
}

/// W-10: список авто-навыков агента (read-only) — диск (`discover_skills`) ⟕ телеметрия (usage).
#[tauri::command]
pub async fn agent_list_skills(state: State<'_, AppState>) -> AppResult<SkillListDto> {
    let (root, reader) = {
        let ctx = state.vault().await?;
        (ctx.root.clone(), ctx.db.reader().clone())
    };
    let cfg = crate::bootstrap::load_local_config(&root).await;
    let learning_enabled = cfg
        .as_ref()
        .map(|c| c.ai.skills.learning_enabled)
        .unwrap_or(false);
    let skills_dir = cfg.as_ref().and_then(|c| c.ai.agent_skills_dir.clone());

    let empty = |dir: Option<String>| SkillListDto {
        learning_enabled,
        skills_dir: dir,
        skills: Vec::new(),
        parse_errors: 0,
    };
    let Some(dir) = skills_dir.clone() else {
        return Ok(empty(None));
    };
    let p = std::path::Path::new(&dir);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    };
    let Ok(canon) = abs.canonicalize() else {
        return Ok(empty(Some(dir))); // каталог не существует → пустой список (UI подскажет)
    };

    let catalog = nexus_core::skills::discover_skills(&canon);
    let parse_errors = catalog.errors().len();
    let overlay = nexus_core::skills::usage::ranked_overlay(&reader)
        .await
        .unwrap_or_default();
    let by_name: std::collections::HashMap<&str, &nexus_core::skills::usage::UsageRecord> =
        overlay.iter().map(|r| (r.skill_name.as_str(), r)).collect();

    let skills = catalog
        .skills()
        .iter()
        .map(|s| {
            let u = by_name.get(s.name.as_str()).copied();
            let is_vendor = matches!(s.tier, nexus_core::skills::TrustTier::Vendor);
            SkillRowDto {
                name: s.name.clone(),
                description: s.description.clone(),
                tier: if is_vendor { "vendor" } else { "local" }.to_string(),
                rel_path: s.rel_path.clone(),
                is_vendor,
                use_count: u.map(|r| r.use_count).unwrap_or(0),
                last_used_at: u.and_then(|r| r.last_used_at),
                created_by: u.and_then(|r| r.created_by.clone()),
                is_agent_created: u.map(|r| r.is_agent_created()).unwrap_or(false),
                pinned: u.map(|r| r.pinned).unwrap_or(false),
                state: u.and_then(|r| {
                    r.state.map(|st| {
                        use nexus_core::skills::usage::SkillState::*;
                        match st {
                            Active => "active",
                            Stale => "stale",
                            Archived => "archived",
                        }
                        .to_string()
                    })
                }),
                license: s.license.clone(),
            }
        })
        .collect();

    Ok(SkillListDto {
        learning_enabled,
        skills_dir: Some(dir),
        skills,
        parse_errors,
    })
}

/// W-10: закрепить/открепить навык (защита от авто-архивации curator'ом). Ядро no-op'ит на
/// не-agent-навыках (vendor/user) — структурный гейт `created_by='agent'`.
#[tauri::command]
pub async fn agent_skill_set_pinned(
    state: State<'_, AppState>,
    name: String,
    pinned: bool,
) -> AppResult<bool> {
    let writer = state.vault().await?.db.writer().clone();
    Ok(nexus_core::skills::usage::set_pinned(&writer, &name, pinned).await?)
}

/// W-10: архивировать/разархивировать навык (ОБРАТИМО). Это НЕ «выключить»: агент всё ещё видит
/// навык в каталоге (фильтрации по state нет — см. BACKLOG). Ядро no-op'ит на не-agent-навыках.
#[tauri::command]
pub async fn agent_skill_set_archived(
    state: State<'_, AppState>,
    name: String,
    archived: bool,
) -> AppResult<bool> {
    let writer = state.vault().await?.db.writer().clone();
    let ok = if archived {
        nexus_core::skills::usage::archive(&writer, &name).await?
    } else {
        nexus_core::skills::usage::set_state(
            &writer,
            &name,
            nexus_core::skills::usage::SkillState::Active,
        )
        .await?
    };
    Ok(ok)
}

#[cfg(test)]
mod tests;
