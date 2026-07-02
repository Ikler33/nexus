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
    run_agent_session, run_store, AgentEvent, AgentEventForwarder, AgentMemory, DelegationDeps,
    LoopOutcome, SessionSpec, VaultAgentMemory,
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

/// W-38: список агент-сессий (история переписок) для левого сайдбара — свежие сверху.
#[tauri::command]
pub async fn agent_sessions_list(state: State<'_, AppState>) -> AppResult<Vec<AgentSessionDto>> {
    let reader = state.vault().await?.db.reader().clone();
    let rows = run_store::list_agent_sessions(&reader).await?;
    Ok(rows
        .into_iter()
        .map(|r| AgentSessionDto {
            session_id: r.session_id,
            title: r.title,
            status: r.status,
            turn_count: r.turn_count,
            updated_at: r.updated_at,
        })
        .collect())
}

/// W-38: загружает все ходы одной переписки (для переоткрытия в UI).
#[tauri::command]
pub async fn agent_session_load(
    state: State<'_, AppState>,
    session_id: String,
) -> AppResult<AgentSessionDataDto> {
    let reader = state.vault().await?.db.reader().clone();
    let rows = run_store::load_agent_session(&reader, &session_id).await?;
    let turns = rows
        .into_iter()
        .map(|t| PersistedTurnDto {
            run_id: t.run_id,
            task: t.task,
            assistant_text: t.assistant_text,
            report: t.report,
            error: t.error,
            status: t.status,
            created_at: t.created_at,
            steps: t
                .steps
                .into_iter()
                .map(|s| PersistedStepDto {
                    kind: s.kind,
                    args: s.args,
                    title: s.title,
                    result: s.result,
                    is_error: s.is_error,
                })
                .collect(),
        })
        .collect();
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
    run_agent_session(
        &spec,
        provider.as_ref(),
        Some(memory.as_ref()),
        skills.as_ref(), // AGENT-0.2: навыки из ai.agent_skills_dir (None если не задан/пуст)
        web.as_ref(),    // AGENT-0.2: web.search/web.fetch при ai.web.enabled (None если выкл)
        decision_source,
        writer,
        reader,
        &paused,
        &cancel,
        forwarder,
        None,                // top-level desktop-прогон (не субагент)
        delegation.as_ref(), // W-24: owner-gated делегирование (ai.delegation, default-OFF)
        research.as_ref(),   // W-25: owner-gated deep-research (ai.research, default-OFF)
    )
    .await
}

/// Финализирует прогон в run_store по исходу цикла (зеркало терминала `AgentRunHandler::drive`):
/// Final→done, Cancelled→cancelled, прочее исчерпание бюджета→error, Error→error. Пауза мид-ран
/// (BudgetExhausted{Paused}) здесь трактуется как НЕ-терминал в desktop-модели: цикл драйвится единым
/// `tokio::spawn` (не реквью планировщика) — если пауза остановила цикл, мы помечаем прогон error с
/// пометкой паузы (UI может перезапустить). Это desktop-упрощение vs agentd-requeue (план планировщика).
/// Возвращает финальный `(status, text)` (W-38: используется и для персиста хода истории — report для
/// done, error-текст иначе).
async fn finish_in_store(
    writer: &nexus_core::db::WriteActor,
    run_id: i64,
    outcome: LoopOutcome,
) -> (&'static str, String) {
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
    (status, text)
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
    let cfg = load_local_config(&root).await;
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

    /// Skills-контекст с одним временным скиллом «alpha»: живой read-only инструмент `activate_skill`
    /// для стрим-тестов успешного tool-шага (B7: debug-стабов в прод-реестре больше нет — при ВЫКЛ
    /// актуаторе реестр наполняют только скиллы/web).
    fn skills_alpha() -> (TempDir, nexus_core::agent::SkillContext) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let d = root.join("alpha");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("SKILL.md"),
            "---\nname: alpha\ndescription: тестовый скилл\n---\nТЕЛО СКИЛЛА",
        )
        .unwrap();
        let ctx = nexus_core::agent::SkillContext::new(
            Arc::new(nexus_core::skills::discover_skills(&root)),
            root,
        );
        (tmp, ctx)
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
    /// ToolCalls([activate_skill]) на ходу 1, Final на ходу 2. `drive_run` (actuator ВЫКЛ → без
    /// инструментов записи; живой read-only тул даёт skills, B7) гонит цикл и форвардит события в наш
    /// collector-Channel. Проверяем: поток несёт toolCall → toolResult → final ПО ПОРЯДКУ + хотя бы
    /// один contextUsage; исход done, tool-шаг УСПЕШЕН (isError=false). Сети/модели нет.
    #[tokio::test]
    async fn drive_run_streams_toolcall_result_final_in_order() {
        let (_dir, db, canon) = open_db().await;
        let (_sk_tmp, skills) = skills_alpha();
        let provider = FakeProvider::new(vec![
            Ok(ToolTurn::ToolCalls(vec![ToolCall {
                id: "c1".into(),
                name: nexus_core::agent::ACTIVATE_SKILL_TOOL.into(),
                arguments: r#"{"skill":"alpha"}"#.into(),
            }])),
            Ok(ToolTurn::Final("готово".into())),
        ]);
        let (channel, buf) = collector_channel();
        let (decision, _tx) = UiDecisionSource::new();

        let outcome = drive_run(
            1,
            "smoke: активируй скилл".into(),
            vec![],
            "auto",
            Some(provider),
            false, // actuator ВЫКЛ → без инструментов записи (vault не трогается)
            64 * 1024,
            16,
            Some(32768),
            None,         // web (AGENT-0.2): тест без веб-инструментов
            Some(skills), // skills: живой read-only тул (B7: стабов нет)
            false,        // skills_learning_enabled
            None,         // delegation (W-24)
            None,         // research (W-25)
            Arc::new(decision),
            empty_memory(&db),
            canon,
            Arc::new(Mutex::new(TurnAccum::default())), // W-38: accum (тест истории не проверяет)
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
        // Корреляция call↔result по id + успешный результат живого тула (activate_skill).
        let call = events.iter().find(|v| type_of(v) == "toolCall").unwrap();
        let res = events.iter().find(|v| type_of(v) == "toolResult").unwrap();
        assert_eq!(call["id"], "c1");
        assert_eq!(res["id"], "c1");
        assert_eq!(res["isError"], false);
    }

    /// **W-38: персист хода истории.** `drive_run` КОПИТ в общий `accum` (через ChannelForwarder) текст
    /// ассистента + шаги по ходу стрима; на терминале `persist_turn` пишет ход, а `load_agent_session`
    /// его читает (зеркало пути `run_impl`-spawn). Доказывает, что аккумуляция и персист сцеплены: после
    /// прогона переписка переоткрывается с тем же шагом (kind+result) и финальным отчётом.
    #[tokio::test]
    async fn drive_run_accumulates_and_persists_turn_for_history() {
        let (_dir, db, canon) = open_db().await;
        let session_id = "sess-hist-1";
        let (_sk_tmp, skills) = skills_alpha();
        let provider = FakeProvider::new(vec![
            Ok(ToolTurn::ToolCalls(vec![ToolCall {
                id: "c1".into(),
                name: nexus_core::agent::ACTIVATE_SKILL_TOOL.into(),
                arguments: r#"{"skill":"alpha"}"#.into(),
            }])),
            Ok(ToolTurn::Final("итог хода".into())),
        ]);
        let (channel, _buf) = collector_channel();
        let (decision, _tx) = UiDecisionSource::new();
        // Прогон в сессии (как run_impl создаёт строку при непустом session_id).
        let run_id = run_store::create_run_in_session(
            db.writer(),
            session_id,
            "разбери входящие",
            None,
            Some("auto"),
        )
        .await
        .unwrap();
        let accum: Arc<Mutex<TurnAccum>> = Arc::new(Mutex::new(TurnAccum::default()));

        let outcome = drive_run(
            run_id,
            "разбери входящие".into(),
            vec![],
            "auto",
            Some(provider),
            false,
            64 * 1024,
            16,
            Some(32768),
            None,
            Some(skills), // живой read-only тул для успешного шага (B7: стабов нет)
            false,
            None,
            None,
            Arc::new(decision),
            empty_memory(&db),
            canon,
            accum.clone(),
            db.writer(),
            db.reader(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            &channel,
        )
        .await;

        // Финализируем как run_impl: finish_in_store → persist_turn из accum.
        let (status, text) = finish_in_store(db.writer(), run_id, outcome).await;
        assert_eq!(status, run_store::STATUS_DONE);
        let (text_acc, steps) = {
            let g = accum.lock().unwrap();
            (g.text.clone(), g.steps.clone())
        };
        // Аккумулятор поймал шаг activate_skill (его result) — даже без текста ассистента.
        assert_eq!(steps.len(), 1, "accum поймал один tool-шаг");
        assert_eq!(steps[0].kind, nexus_core::agent::ACTIVATE_SKILL_TOOL);
        assert!(steps[0].result.is_some(), "результат шага зафиксирован");
        assert!(!steps[0].is_error);

        run_store::persist_turn(
            db.writer(),
            run_id,
            session_id,
            "разбери входящие",
            &text_acc,
            &steps,
            status,
            Some(text.as_str()),
            None,
            1234,
        )
        .await
        .unwrap();

        // Переоткрытие переписки: ход и его шаг на месте.
        let turns = run_store::load_agent_session(db.reader(), session_id)
            .await
            .unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].run_id, run_id);
        assert_eq!(turns[0].task, "разбери входящие");
        assert_eq!(turns[0].status, run_store::STATUS_DONE);
        assert_eq!(turns[0].report.as_deref(), Some("итог хода"));
        assert_eq!(turns[0].steps.len(), 1);
        assert_eq!(
            turns[0].steps[0].kind,
            nexus_core::agent::ACTIVATE_SKILL_TOOL
        );

        // И в списке сессий появилась наша переписка.
        let sessions = run_store::list_agent_sessions(db.reader()).await.unwrap();
        assert!(sessions
            .iter()
            .any(|s| s.session_id == session_id && s.turn_count == 1));
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
            vec![],
            "auto",
            None,
            false,
            64 * 1024,
            16,
            Some(32768),
            None,  // web (AGENT-0.2): тест без веб-инструментов
            None,  // skills (AGENT-0.2): тест без навыков
            false, // skills_learning_enabled
            None,  // delegation (W-24)
            None,  // research (W-25)
            Arc::new(decision),
            empty_memory(&db),
            canon,
            Arc::new(Mutex::new(TurnAccum::default())), // W-38: accum (тест истории не проверяет)
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
            vec![],
            "confirm", // confirm-прогон → даже Auto-тир note.create предлагается
            Some(provider),
            true, // actuator ВКЛ (go-live, тестовый temp-vault)
            64 * 1024,
            16,
            Some(32768),
            None,  // web (AGENT-0.2): тест без веб-инструментов
            None,  // skills (AGENT-0.2): тест без навыков
            false, // skills_learning_enabled
            None,  // delegation (W-24)
            None,  // research (W-25)
            decision,
            empty_memory(&db),
            canon.clone(),
            Arc::new(Mutex::new(TurnAccum::default())), // W-38: accum (тест истории не проверяет)
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

    /// **W-12: ДЕТЕРМИНИРОВАННЫЙ E2E критпути агента в CI — задача→tool→proposal→approve→ЗАПИСЬ→UNDO.**
    /// Раньше полный путь с откатом был только в ignored live-тесте (нужен рижский LLM). Здесь тот же
    /// desktop-путь (`drive_run` + реальный `UiDecisionSource` + гейт actuator'а + temp-vault БД), но на
    /// `FakeProvider` — поэтому гоняется в CI на каждом PR/push. Сцепляет уже-проверенные по-отдельности
    /// записи-через-гейт и `undo_run` (зеркало `agent_undo`) в ОДНУ непрерывную цепочку.
    #[tokio::test]
    async fn approve_then_undo_reverts_write_e2e() {
        let (_dir, db, canon) = open_db().await;
        let provider = note_create_then_final("Notes/E2E.md", "созданоаппрувом");
        let (channel, buf) = collector_channel();
        let (decision, tx): (Arc<dyn DecisionSource>, _) = {
            let (s, t) = UiDecisionSource::new();
            (Arc::new(s), t)
        };

        // Approve по факту прихода Proposal (как в approve_applies_confirm_item).
        let buf_for_approver = buf.clone();
        let approver = tokio::spawn(async move {
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

        let run_id = 1;
        let outcome = drive_run(
            run_id,
            "создай заметку".into(),
            vec![],
            "confirm",
            Some(provider),
            true, // actuator ВКЛ (go-live в temp-vault)
            64 * 1024,
            16,
            Some(32768),
            None,
            None,
            false,
            None, // delegation (W-24)
            None, // research (W-25)
            decision,
            empty_memory(&db),
            canon.clone(),
            Arc::new(Mutex::new(TurnAccum::default())), // W-38: accum (тест истории не проверяет)
            db.writer(),
            db.reader(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            &channel,
        )
        .await;
        approver.await.unwrap();

        // Этап 1: approve применил Confirm-айтем → файл записан через гейт.
        assert_eq!(outcome, LoopOutcome::Final("готово".into()));
        assert_eq!(
            std::fs::read_to_string(canon.join("Notes/E2E.md"))
                .ok()
                .as_deref(),
            Some("созданоаппрувом"),
            "файл записан после approve"
        );

        // Этап 2: UNDO прогона (зеркало agent_undo: AuditSink над тем же writer/reader) → файл откатан.
        let ledger = nexus_core::actuator::AuditSink::new(db.writer().clone(), db.reader().clone());
        let undone = nexus_core::actuator::undo_run(run_id, &canon, &ledger).await;
        assert!(undone.restored() >= 1, "undo восстановил ≥1 действие");
        assert!(
            !canon.join("Notes/E2E.md").exists(),
            "после undo созданный файл удалён (откат записи)"
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
            vec![],
            "confirm",
            Some(provider),
            true,
            64 * 1024,
            16,
            Some(32768),
            None,  // web (AGENT-0.2): тест без веб-инструментов
            None,  // skills (AGENT-0.2): тест без навыков
            false, // skills_learning_enabled
            None,  // delegation (W-24)
            None,  // research (W-25)
            decision,
            empty_memory(&db),
            canon.clone(),
            Arc::new(Mutex::new(TurnAccum::default())), // W-38: accum (тест истории не проверяет)
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
