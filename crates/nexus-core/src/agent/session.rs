//! [`run_agent_session`] — ЕДИНАЯ композиция прогона агента (P0b-2): сборка начального контекста +
//! выбор реестра (стабы | гейтнутые актуаторы) + скиллы + `run_agent_loop`. Транспорт-агностична:
//! куда уходят события — решает [`AgentEventForwarder`], переданный вызывающим.
//!
//! # Зачем модуль (DRY)
//! Раньше эта композиция жила в ТРЁХ местах: [`super::job::AgentRunHandler`] (headless scheduler),
//! desktop `commands::agent::drive_run` (стрим в Tauri-Channel) и — намечался — agentd-коннектор
//! (стрим в [`super::connect::Transport`]). Три копии расходились по контракту. Здесь — единственный
//! источник истины: каждый вызывающий лишь оборачивает run-lifecycle (run_store create/mark/finish)
//! и подаёт СВОЙ [`AgentEventForwarder`]:
//! - headless agentd → counter + `tracing`-лог Proposal/Diff (наблюдаемость, без стрима);
//! - desktop → маппинг в wire-DTO → `Channel<AgentStreamEvent>` (UI-1b);
//! - agentd-коннектор → wire-DTO → `agent/event`-нотификация через [`super::connect::Transport`].
//!
//! # Слияние двух потоков событий
//! Цикл шлёт AssistantToken/ToolCall/ToolResult/ContextUsage/Final/Error через `on_event`; гейт
//! актуатора шлёт Proposal/Diff через свой [`EventSink`]. Потоки НЕПЕРЕСЕКАЮЩИЕСЯ. Здесь оба сводятся
//! в ОДИН [`AgentEventForwarder`]: `on_event` зовёт `forward` напрямую, а гейт получает
//! [`ForwardingEventSink`]-обёртку над тем же форвардером. Так потребитель видит единый порядок.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::actuator::{
    ActionDispatcher, AuditSink, DecisionSource, DispatchPolicy, EventSink, GatedToolCtx,
    NoteCreateTool, NoteEditTool, SetFrontmatterTool, SkillSaveCtx, SkillSaveTool,
};
use crate::ai::tools::ToolCapableProvider;
use crate::ai::{injection_marker, ChatMessage, ContextBudget, QwenTokenizer};
use crate::db::{ReadPool, WriteActor};
use crate::net::RunCtx;

use super::event::AgentEvent;
use super::job::{AGENT_PREAMBLE, RECALL_BUDGET_TOKENS};
use super::memory::AgentMemory;
use super::registry::ToolRegistry;
use super::runner::{run_agent_loop, LoopBounds, LoopOutcome};
use super::skill_tools::SkillContext;
use super::stubs::{EchoTool, NoopTool};
use super::web_tools::WebToolsConfig;

/// СИНХРОННЫЙ форвардер событий прогона наружу. Реализуется потребителем под свой транспорт; вызывается
/// из двух мест внутри [`run_agent_session`] (loop `on_event` + гейт-[`EventSink`]) — оба синхронны,
/// поэтому асинхронный транспорт (коннектор) форвардер мостит сам (mpsc → drain-таск).
pub trait AgentEventForwarder: Send + Sync {
    /// Принять одно событие хода (по ССЫЛКЕ — форвардер обычно клонирует лишь нужное / маппит в wire).
    fn forward(&self, ev: &AgentEvent);
}

/// Мост гейт-[`EventSink`] → [`AgentEventForwarder`]: Proposal/Diff гейта уходят в тот же форвардер,
/// что и события цикла (единый поток для потребителя).
struct ForwardingEventSink(Arc<dyn AgentEventForwarder>);

impl EventSink for ForwardingEventSink {
    fn emit(&self, event: AgentEvent) {
        self.0.forward(&event);
    }
}

/// Плоские (data-only) параметры прогона. Хендлы (provider/memory/skills/decision_source/writer/reader/
/// флаги/форвардер) идут отдельными аргументами [`run_agent_session`] (они не `Clone`/суть Arc/ссылки).
pub struct SessionSpec {
    /// `id` строки `agent_runs` (корреляция эгресса/леджера/UI).
    pub run_id: i64,
    /// Задача пользователя (становится финальным `user`-сообщением начального контекста).
    pub task: String,
    /// Автономия прогона (`confirm`|`auto`|`None`→confirm в политике). Эффект только при `actuator_enabled`.
    pub autonomy: Option<String>,
    /// **GO-LIVE-флаг актуатора, SAFE BY DEFAULT.** `false` → только стабы (vault не трогается); `true`
    /// → гейтнутые инструменты-актуаторы за `dispatch_action`.
    pub actuator_enabled: bool,
    /// Порог «крупной перезаписи» → Confirm-тир. Эффект только при `actuator_enabled`.
    pub overwrite_threshold: usize,
    /// Кэп blast-radius прогона (анти-усталость). Эффект только при `actuator_enabled`.
    pub blast_cap: u32,
    /// Окно контекста модели (токены) из конфига; `None` → консервативный дефолт [`ContextBudget`].
    pub context_window: Option<usize>,
    /// КАНОНИЗИРОВАННЫЙ корень vault (предусловие гейта/apply). Нужен только при `actuator_enabled`.
    pub canon_root: PathBuf,
    /// **SELF-LEARNING SL-7d, OWNER-GATED, default false** (`ai.skills.learning_enabled`). `true` И
    /// `actuator_enabled` И сконфигурированный skills-каталог (`skills=Some`) → регистрируется `skill.save`
    /// (агент авторствует навыки через гейт Confirm-never-Auto) + откат прогона идёт `undo_run_full` со
    /// skills_root. `false` → инструмента нет, classify режет `SkillSave` HardBlocked (поведение без регрессии).
    pub skills_learning_enabled: bool,
}

/// Гонит один прогон агента: собирает начальный контекст ([system преамбул] + [recall памяти] +
/// [меню скиллов] + [задача]), выбирает реестр (стабы | гейтнутые актуаторы с
/// [`ForwardingEventSink`]), регистрирует tier-2/3 инструменты скиллов и крутит [`run_agent_loop`].
/// Возвращает [`LoopOutcome`] — финализацию в `run_store` делает ВЫЗЫВАЮЩИЙ (этот слой не трогает
/// статус-машину прогона, чтобы оставаться переиспользуемым для scheduler/desktop/коннектора).
///
/// `memory = None` → recall пуст (поведение без памяти, без регрессии). `skills = None` → ни меню, ни
/// tier-2/3 инструментов. KILL-SWITCH (`paused`) и `cancel` пробрасываются в цикл (и в политику гейта).
#[allow(clippy::too_many_arguments)]
pub async fn run_agent_session(
    spec: &SessionSpec,
    provider: &dyn ToolCapableProvider,
    memory: Option<&dyn AgentMemory>,
    skills: Option<&SkillContext>,
    web: Option<&WebToolsConfig>,
    decision_source: Arc<dyn DecisionSource>,
    writer: &WriteActor,
    reader: &ReadPool,
    paused: &Arc<AtomicBool>,
    cancel: &Arc<AtomicBool>,
    forwarder: Arc<dyn AgentEventForwarder>,
) -> LoopOutcome {
    // Начальный контекст: [system преамбул] + [recall памяти] + [меню скиллов] + [задача]. recall —
    // только чтение, никогда не ошибка (деградирует в пусто); None память → пусто (без регрессии).
    // Меню скиллов (tier-1) — фенсенный user-role блок (данные, не инструкции, I-5); per-request
    // injection_marker. Тела скиллов раскрывает лишь tier-2 `activate_skill`.
    let recalled = match memory {
        Some(mem) => mem.recall(&spec.task, RECALL_BUDGET_TOKENS).await,
        None => Vec::new(),
    };
    let skill_menu: Option<ChatMessage> = skills
        .and_then(|sk| sk.catalog_block(&injection_marker()))
        .map(ChatMessage::user);
    let mut messages = Vec::with_capacity(recalled.len() + 2 + usize::from(skill_menu.is_some()));
    messages.push(ChatMessage::system(AGENT_PREAMBLE));
    messages.extend(recalled);
    messages.extend(skill_menu);
    messages.push(ChatMessage::user(&spec.task));

    let bounds = LoopBounds::default();
    let budget = ContextBudget::from_context_window(spec.context_window);
    let tk = QwenTokenizer::embedded();

    // Реестр: дефолт-OFF → стабы (echo/noop, vault не трогается); ВКЛ → гейтнутые актуаторы за
    // dispatch_action. Гейт получает ForwardingEventSink → Proposal/Diff уходят тем же форвардером,
    // что и события цикла. Per-run DispatchPolicy (общий blast-radius между инструментами) + проброс
    // `paused` в политику (KILL-SWITCH чек-пойнт #3: НЕ пишет под паузой даже мид-инструмент).
    let mut registry = if spec.actuator_enabled {
        let ledger = AuditSink::new(writer.clone(), reader.clone());
        // SL-7d: skills-флаги в политику → classify_skill_save видит learning/root (иначе SkillSave всегда
        // HardBlocked). skills_root_configured = есть ли SkillContext (skills=Some). Note/exec не затронуты.
        let policy = DispatchPolicy::with_paused(
            spec.autonomy.as_deref(),
            spec.overwrite_threshold,
            spec.blast_cap,
            paused.clone(),
        )
        .with_skills_flags(spec.skills_learning_enabled, skills.is_some());
        let events: Arc<dyn EventSink> = Arc::new(ForwardingEventSink(forwarder.clone()));
        let mut reg = ToolRegistry::new();
        // SL-7d: `skill.save` (авторство навыков) — ТОЛЬКО при `skills_learning_enabled` + сконфигурированном
        // skills-каталоге (`skills=Some`). Делит policy(token-bucket/паузу)/ledger/decision_source/events с
        // note-инструментами (общий blast-radius/kill-switch); пишет под skills_root; провенанс через writer.
        // Флаг выкл (default) → инструмента НЕТ, а classify_skill_save и так режет SkillSave HardBlocked.
        if spec.skills_learning_enabled {
            if let Some(sk) = skills {
                let skill_ctx = SkillSaveCtx::new(
                    sk.skills_root().to_path_buf(),
                    ledger.clone(),
                    spec.run_id,
                    policy.clone(),
                    decision_source.clone(),
                    events.clone(),
                    Some(writer.clone()),
                );
                reg.insert(Arc::new(SkillSaveTool::new(Arc::new(skill_ctx))));
            }
        }
        let gate = GatedToolCtx::new(
            spec.canon_root.clone(),
            ledger,
            spec.run_id,
            policy,
            decision_source,
            events,
        );
        // ШОВ актуатора (SANDBOX-4b-2): инструменты держат `Arc<dyn ActionDispatcher>`. In-process путь —
        // `GatedToolCtx` (локальный `dispatch_action`). Песочница подставит `ProxyActuator` (host/act RPC).
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(gate);
        reg.insert(Arc::new(NoteCreateTool::new(dispatcher.clone())));
        reg.insert(Arc::new(NoteEditTool::new(dispatcher.clone())));
        reg.insert(Arc::new(SetFrontmatterTool::new(dispatcher)));
        reg
    } else {
        // Default-safe: стабы (НЕ касаются vault). decision_source/canon_root тогда не используются.
        let _ = (&decision_source, &spec.canon_root);
        let mut reg = ToolRegistry::new();
        reg.insert(Arc::new(EchoTool));
        reg.insert(Arc::new(NoopTool));
        reg
    };
    // SKILL-2 (tier 2 + 3): READ-ONLY инструменты скиллов (activate_skill + read_skill_resource),
    // НЕЗАВИСИМО от actuator-флага (скиллы только читают). Активация скилла НЕ добавляет иных
    // инструментов (capability-инертность — гейт у SKILL-3).
    // EGR-AGENT: web.search/web.fetch — READ-ONLY (vault не трогают), НЕЗАВИСИМО от actuator-флага.
    // Эгресс — через GuardedClient(EgressFeature::Web) внутри инструментов; per-run RunCtx для аудита.
    if let Some(web) = web {
        for tool in crate::agent::web_tools::web_tools(web, RunCtx::run(spec.run_id)) {
            registry.insert(tool);
        }
    }
    if let Some(skills) = skills {
        for tool in skills.tools() {
            registry.insert(tool);
        }
    }

    // on_event: КАЖДОЕ событие цикла → форвардер (тот же, что у гейта). Запись шага/стрим/лог — забота
    // форвардера (path-specific). Синхронный (как требует `run_agent_loop`).
    let mut on_event = |e: AgentEvent| forwarder.forward(&e);

    // KILL-SWITCH (AGENT-5, чек-пойнт #2): `paused` в цикл — пауза мид-ран остановит на границе хода.
    run_agent_loop(
        provider,
        &registry,
        messages,
        bounds,
        &budget,
        &tk,
        cancel,
        paused,
        RunCtx::run(spec.run_id),
        &mut on_event,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actuator::PolicyDefault;
    use crate::agent::tool::{ToolCall, ToolSpec};
    use crate::ai::tools::ToolTurn;
    use crate::ai::{AiResult, ChatMessage as Msg};
    use crate::db::Database;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use tempfile::TempDir;

    fn policy_default() -> Arc<dyn DecisionSource> {
        Arc::new(PolicyDefault)
    }

    /// Форвардер-сборщик: копит все события в порядке эмиссии (доказ. единого слитого потока).
    #[derive(Default)]
    struct CollectingForwarder {
        events: Mutex<Vec<AgentEvent>>,
    }
    impl AgentEventForwarder for CollectingForwarder {
        fn forward(&self, ev: &AgentEvent) {
            self.events.lock().unwrap().push(ev.clone());
        }
    }

    /// Фейк tool-провайдер: возвращает заранее заданную последовательность ходов (как agent_loop_smoke).
    struct FakeProvider {
        turns: Mutex<std::collections::VecDeque<AiResult<ToolTurn>>>,
    }
    impl FakeProvider {
        fn new(turns: Vec<AiResult<ToolTurn>>) -> Self {
            Self {
                turns: Mutex::new(turns.into_iter().collect()),
            }
        }
    }
    #[async_trait]
    impl ToolCapableProvider for FakeProvider {
        async fn stream_chat_tools(
            &self,
            _messages: &[Msg],
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
        let db = Database::open(&dir.path().join("test.db")).await.unwrap();
        (dir, db)
    }

    /// Стаб-путь (actuator OFF): фейк зовёт echo на ходу 1, Final на ходу 2. Форвардер должен увидеть
    /// ПО ПОРЯДКУ ToolCall → ToolResult → Final, реальный vault не трогается. Доказывает, что
    /// run_agent_session сводит события цикла в единый форвардер.
    #[tokio::test]
    async fn stub_path_forwards_toolcall_result_final_in_order() {
        let (_dir, db) = open_db().await;
        let provider = FakeProvider::new(vec![
            Ok(ToolTurn::ToolCalls(vec![ToolCall {
                id: "c1".into(),
                name: "echo".into(),
                arguments: r#"{"text":"hi"}"#.into(),
            }])),
            Ok(ToolTurn::Final("готово".into())),
        ]);
        let fwd = Arc::new(CollectingForwarder::default());
        let spec = SessionSpec {
            run_id: 1,
            task: "сделай эхо".into(),
            autonomy: None,
            actuator_enabled: false,
            overwrite_threshold: 100,
            blast_cap: 10,
            context_window: Some(4096),
            canon_root: _dir.path().to_path_buf(),
            skills_learning_enabled: false,
        };
        let paused = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let outcome = run_agent_session(
            &spec,
            &provider,
            None,
            None,
            None,
            policy_default(),
            db.writer(),
            db.reader(),
            &paused,
            &cancel,
            fwd.clone(),
        )
        .await;

        assert!(matches!(outcome, LoopOutcome::Final(s) if s == "готово"));
        let evs = fwd.events.lock().unwrap();
        let pos = |pred: &dyn Fn(&AgentEvent) -> bool| evs.iter().position(pred);
        let call = pos(&|e| matches!(e, AgentEvent::ToolCall { .. })).expect("toolcall");
        let res = pos(&|e| matches!(e, AgentEvent::ToolResult { .. })).expect("toolresult");
        let fin = pos(&|e| matches!(e, AgentEvent::Final(_))).expect("final");
        assert!(call < res && res < fin, "порядок ToolCall<ToolResult<Final");
    }

    /// Пустой провайдер-стрим, который сразу Final — форвардер видит хотя бы ContextUsage + Final, vault
    /// не тронут. Гард: даже тривиальный прогон проводится через единый форвардер.
    #[tokio::test]
    async fn immediate_final_still_forwards_context_usage() {
        let (_dir, db) = open_db().await;
        let provider = FakeProvider::new(vec![Ok(ToolTurn::Final("сразу".into()))]);
        let fwd = Arc::new(CollectingForwarder::default());
        let spec = SessionSpec {
            run_id: 2,
            task: "ничего".into(),
            autonomy: None,
            actuator_enabled: false,
            overwrite_threshold: 100,
            blast_cap: 10,
            context_window: Some(4096),
            canon_root: _dir.path().to_path_buf(),
            skills_learning_enabled: false,
        };
        let paused = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let outcome = run_agent_session(
            &spec,
            &provider,
            None,
            None,
            None,
            policy_default(),
            db.writer(),
            db.reader(),
            &paused,
            &cancel,
            fwd.clone(),
        )
        .await;
        assert!(matches!(outcome, LoopOutcome::Final(_)));
        let evs = fwd.events.lock().unwrap();
        assert!(evs
            .iter()
            .any(|e| matches!(e, AgentEvent::ContextUsage { .. })));
        assert!(evs.iter().any(|e| matches!(e, AgentEvent::Final(_))));
    }

    /// Провайдер, фиксирующий контекст и tool-спеки ПЕРВОГО хода (для проверки skills-инъекции).
    struct RecordingProvider {
        seen_msgs: Mutex<Vec<String>>,
        seen_tools: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl ToolCapableProvider for RecordingProvider {
        async fn stream_chat_tools(
            &self,
            messages: &[Msg],
            tools: &[ToolSpec],
            _on_token: &mut (dyn FnMut(String) + Send),
            _cancel: &Arc<AtomicBool>,
            _ctx: RunCtx,
        ) -> AiResult<ToolTurn> {
            // Debug-рендер сообщений (не зависим от приватности полей ChatMessage) — ищем имя скилла в меню.
            *self.seen_msgs.lock().unwrap() = messages.iter().map(|m| format!("{m:?}")).collect();
            *self.seen_tools.lock().unwrap() = tools.iter().map(|t| t.name.clone()).collect();
            Ok(ToolTurn::Final("ок".into()))
        }
        fn model_id(&self) -> &str {
            "rec"
        }
    }

    /// `skills = Some(..)` → (а) tier-1 МЕНЮ скилла попадает в начальный контекст (имя скилла видно
    /// провайдеру), (б) tier-2/3 инструменты (`activate_skill`/`read_skill_resource`) зарегистрированы
    /// в реестре рядом со стабами — НЕЗАВИСИМО от actuator-флага (скиллы только читают). Покрывает
    /// единственную ветку композиции, которую desktop не задействует (skills там всегда None).
    #[tokio::test]
    async fn skills_inject_menu_and_register_tier2_3_tools() {
        use crate::agent::skill_tools::{
            SkillContext, ACTIVATE_SKILL_TOOL, READ_SKILL_RESOURCE_TOOL,
        };
        use crate::skills::discover_skills;

        let skills_tmp = TempDir::new().unwrap();
        let skills_root = skills_tmp.path().canonicalize().unwrap();
        let d = skills_root.join("alpha");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("SKILL.md"),
            "---\nname: alpha\ndescription: первый скилл\n---\nТЕЛО СКИЛЛА",
        )
        .unwrap();
        let skills = SkillContext::new(Arc::new(discover_skills(&skills_root)), skills_root);

        let (_dir, db) = open_db().await;
        let provider = RecordingProvider {
            seen_msgs: Mutex::new(Vec::new()),
            seen_tools: Mutex::new(Vec::new()),
        };
        let fwd = Arc::new(CollectingForwarder::default());
        let spec = SessionSpec {
            run_id: 3,
            task: "используй скиллы".into(),
            autonomy: None,
            actuator_enabled: false, // скиллы работают и при ВЫКЛ актуаторе (read-only).
            overwrite_threshold: 100,
            blast_cap: 10,
            context_window: Some(8192),
            canon_root: _dir.path().to_path_buf(),
            skills_learning_enabled: false,
        };
        let paused = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let outcome = run_agent_session(
            &spec,
            &provider,
            None,
            Some(&skills),
            None,
            policy_default(),
            db.writer(),
            db.reader(),
            &paused,
            &cancel,
            fwd.clone(),
        )
        .await;
        assert!(matches!(outcome, LoopOutcome::Final(_)));

        // (а) меню скилла (имя «alpha») попало в начальный контекст, отданный провайдеру.
        let msgs = provider.seen_msgs.lock().unwrap();
        assert!(
            msgs.iter().any(|m| m.contains("alpha")),
            "tier-1 меню скилла должно быть в контексте: {msgs:?}"
        );
        // (б) tier-2/3 инструменты скиллов зарегистрированы (рядом со стабами echo/noop).
        let tools = provider.seen_tools.lock().unwrap();
        assert!(
            tools.iter().any(|t| t == ACTIVATE_SKILL_TOOL),
            "activate_skill должен быть зарегистрирован: {tools:?}"
        );
        assert!(
            tools.iter().any(|t| t == READ_SKILL_RESOURCE_TOOL),
            "read_skill_resource должен быть зарегистрирован: {tools:?}"
        );
    }

    /// LIVE: реальная модель на риге создаёт заметку ЧЕРЕЗ ГЕЙТ актуатора (autonomy=auto → Auto-тир
    /// применяется без аппрува), файл РЕАЛЬНО записан в temp-vault, затем `undo_run` его удаляет
    /// (восстановление). Доказывает ПОЛНЫЙ стек вживую: модель → tool-call note.create → `dispatch_action`
    /// гейт → apply на диск → undo. Запуск:
    /// `NEXUS_LIVE_CHAT=1 cargo test -p nexus-core --lib agent::session::tests::live_actuator -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "live actuator (нужна tool-capable модель: NEXUS_LIVE_CHAT=1, NEXUS_LIVE_CHAT_URL default 192.168.0.31:8080)"]
    async fn live_actuator_create_and_undo_on_rig() {
        use crate::actuator::AuditSink;
        use crate::agent::run_store;
        use crate::ai::tools::OpenAiToolProvider;
        use crate::net::{EgressAudit, EgressFeature, EgressPolicy, GuardedClient};
        use std::time::Duration;

        if std::env::var("NEXUS_LIVE_CHAT").ok().as_deref() != Some("1") {
            eprintln!("SKIP: NEXUS_LIVE_CHAT!=1");
            return;
        }
        let url = std::env::var("NEXUS_LIVE_CHAT_URL")
            .unwrap_or_else(|_| "http://192.168.0.31:8080".into());
        let model =
            std::env::var("NEXUS_LIVE_CHAT_MODEL").unwrap_or_else(|_| "qwen36-mtp.gguf".into());

        let dir = TempDir::new().unwrap();
        let canon = dir.path().canonicalize().unwrap();
        let db = Database::open(canon.join("nexus.db")).await.unwrap();

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

        let rel = "Notes/AgentLiveTest.md";
        let run_id = run_store::create_run(
            db.writer(),
            "live actuator",
            Some(provider.model_id()),
            Some("auto"),
        )
        .await
        .unwrap();
        let spec = SessionSpec {
            run_id,
            task: format!(
                "Создай заметку по пути {rel} с содержимым 'привет от агента' — используй инструмент \
                 создания заметки note.create (аргументы path и content). Затем дай короткий финальный ответ."
            ),
            autonomy: Some("auto".into()),
            actuator_enabled: true,
            overwrite_threshold: 64 * 1024,
            blast_cap: 16,
            context_window: Some(32768),
            canon_root: canon.clone(),
            skills_learning_enabled: false,
        };
        let fwd = Arc::new(CollectingForwarder::default());
        let paused = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let outcome = run_agent_session(
            &spec,
            provider.as_ref(),
            None,
            None,
            None,
            policy_default(),
            db.writer(),
            db.reader(),
            &paused,
            &cancel,
            fwd.clone(),
        )
        .await;
        eprintln!("LIVE outcome: {outcome:?}");
        for e in fwd.events.lock().unwrap().iter() {
            eprintln!("  ev: {e:?}");
        }

        let path = canon.join(rel);
        assert!(
            path.exists(),
            "модель должна была создать заметку через гейт (autonomy=auto): {}",
            path.display()
        );
        eprintln!(
            "LIVE created note: {:?}",
            std::fs::read_to_string(&path).unwrap()
        );

        // Undo восстанавливает (файл был создан → undo удаляет).
        let ledger = AuditSink::new(db.writer().clone(), db.reader().clone());
        let undo = crate::actuator::undo_run(run_id, &canon, &ledger).await;
        eprintln!("LIVE undo restored={}", undo.restored());
        assert!(undo.restored() >= 1, "undo должен откатить >=1 действие");
        assert!(!path.exists(), "undo должен удалить созданную заметку");
    }
}
