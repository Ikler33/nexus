use super::*;
use crate::actuator::PolicyDefault;
use crate::agent::test_support::open_db;
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

/// Фейк tool-провайдер: возвращает заранее заданную последовательность ходов (как agent_loop_smoke)
/// и записывает ИМЕНА тулов каждого хода (B7: доказательство состава реестра, который видит модель).
struct FakeProvider {
    turns: Mutex<std::collections::VecDeque<AiResult<ToolTurn>>>,
    seen_tools: Mutex<Vec<Vec<String>>>,
}
impl FakeProvider {
    fn new(turns: Vec<AiResult<ToolTurn>>) -> Self {
        Self {
            turns: Mutex::new(turns.into_iter().collect()),
            seen_tools: Mutex::new(Vec::new()),
        }
    }
    /// Имена тулов, показанные модели на ПЕРВОМ ходу (состав реестра фиксирован на весь прогон).
    fn first_turn_tools(&self) -> Vec<String> {
        self.seen_tools
            .lock()
            .unwrap()
            .first()
            .cloned()
            .unwrap_or_default()
    }
}
#[async_trait]
impl ToolCapableProvider for FakeProvider {
    async fn stream_chat_tools(
        &self,
        _messages: &[Msg],
        tools: &[ToolSpec],
        _on_token: &mut (dyn FnMut(String) + Send),
        _cancel: &Arc<AtomicBool>,
        _ctx: RunCtx,
    ) -> AiResult<ToolTurn> {
        self.seen_tools
            .lock()
            .unwrap()
            .push(tools.iter().map(|t| t.name.clone()).collect());
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

/// Skills-контекст с одним временным скиллом «alpha» — реальные read-only инструменты
/// (`activate_skill`/`read_skill_resource`) для тестов состава/сужения реестра при ВЫКЛ актуаторе
/// (B7: debug-стабов в прод-реестре больше нет, живые тулы дают скиллы).
fn skills_alpha() -> (TempDir, crate::agent::skill_tools::SkillContext) {
    use crate::skills::discover_skills;
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let d = root.join("alpha");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("SKILL.md"),
        "---\nname: alpha\ndescription: тестовый скилл\n---\nТЕЛО СКИЛЛА",
    )
    .unwrap();
    let ctx = crate::agent::skill_tools::SkillContext::new(Arc::new(discover_skills(&root)), root);
    (tmp, ctx)
}

/// **B7, actuator OFF → реестр ПУСТ** (debug.echo/debug.noop в прод-пути больше не регистрируются:
/// модель не видит пустышек в списке инструментов). Фейк зовёт `debug.echo` на ходу 1 — это теперь
/// UnknownTool (is_error), цикл НЕ падает; Final на ходу 2. Форвардер видит ПО ПОРЯДКУ
/// ToolCall → ToolResult → Final (единый слитый поток), vault не трогается.
#[tokio::test]
async fn actuator_off_empty_registry_forwards_toolcall_result_final_in_order() {
    let (_dir, db) = open_db().await;
    let provider = FakeProvider::new(vec![
        Ok(ToolTurn::ToolCalls(vec![ToolCall {
            id: "c1".into(),
            name: "debug.echo".into(),
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
        history: Vec::new(),
        skills_learning_enabled: false,
    };
    let paused = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    let outcome = run_agent_session(
        &spec,
        &SessionDeps {
            provider: &provider,
            memory: None,
            skills: None,
            web: None,
            decision_source: policy_default(),
            writer: db.writer(),
            reader: db.reader(),
            paused: &paused,
            cancel: &cancel,
            forwarder: fwd.clone(),
        },
        SessionRole::TopLevel {
            delegation: None,
            research: None, // research (RES-4): default-OFF; прод-проводка в RES-5
        },
    )
    .await;

    assert!(matches!(outcome, LoopOutcome::Final(s) if s == "готово"));
    // B7-доказательство: модель на первом ходу получила ПУСТОЙ список тулов — никаких debug.*.
    assert!(
        provider.first_turn_tools().is_empty(),
        "actuator OFF → пустой реестр (без debug-стабов): {:?}",
        provider.first_turn_tools()
    );
    let evs = fwd.events.lock().unwrap();
    let pos = |pred: &dyn Fn(&AgentEvent) -> bool| evs.iter().position(pred);
    let call = pos(&|e| matches!(e, AgentEvent::ToolCall { .. })).expect("toolcall");
    let res = pos(&|e| matches!(e, AgentEvent::ToolResult { .. })).expect("toolresult");
    let fin = pos(&|e| matches!(e, AgentEvent::Final(_))).expect("final");
    assert!(call < res && res < fin, "порядок ToolCall<ToolResult<Final");
    // Вызов несуществующего тула честно помечен ошибкой (UnknownTool), но прогон дошёл до Final.
    assert!(
        matches!(evs[res], AgentEvent::ToolResult { is_error: true, .. }),
        "debug.echo не зарегистрирован → UnknownTool is_error"
    );
}

/// SUB-3a (security keystone проводки): `subagent=Some(allowed)` СУЖАЕТ реестр ребёнка. Полный
/// реестр (skills, B7: стабов больше нет) содержит `read_skill_resource`, но его НЕТ в
/// `allowed={activate_skill}` → реестр ребёнка его не содержит → `UnknownTool` is_error.
/// Эскалация инструментом сверх выданного невозможна по построению.
#[tokio::test]
async fn subagent_filtered_tool_is_unknown() {
    use crate::agent::skill_tools::{ACTIVATE_SKILL_TOOL, READ_SKILL_RESOURCE_TOOL};
    let (_sk_tmp, skills) = skills_alpha();
    let (_dir, db) = open_db().await;
    let provider = FakeProvider::new(vec![
        Ok(ToolTurn::ToolCalls(vec![ToolCall {
            id: "c1".into(),
            name: READ_SKILL_RESOURCE_TOOL.into(),
            arguments: r#"{"skill":"alpha","resource_path":"x.md"}"#.into(),
        }])),
        Ok(ToolTurn::Final("ок".into())),
    ]);
    let fwd = Arc::new(CollectingForwarder::default());
    let spec = SessionSpec {
        run_id: 10,
        task: "t".into(),
        autonomy: None,
        actuator_enabled: false,
        overwrite_threshold: 100,
        blast_cap: 10,
        context_window: Some(4096),
        canon_root: _dir.path().to_path_buf(),
        history: Vec::new(),
        skills_learning_enabled: false,
    };
    let paused = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    let allowed: std::collections::BTreeSet<String> =
        [ACTIVATE_SKILL_TOOL.to_string()].into_iter().collect();
    run_agent_session(
        &spec,
        &SessionDeps {
            provider: &provider,
            memory: None,
            skills: Some(&skills),
            web: None,
            decision_source: policy_default(),
            writer: db.writer(),
            reader: db.reader(),
            paused: &paused,
            cancel: &cancel,
            forwarder: fwd.clone(),
        },
        SessionRole::Subagent {
            allowed: &allowed,
            dispatcher: None,
        },
    )
    .await;
    // Сужение видно и МОДЕЛИ: в спеках ребёнка только allowed-имя (read_skill_resource удалён).
    assert_eq!(
        provider.first_turn_tools(),
        vec![ACTIVATE_SKILL_TOOL.to_string()],
        "реестр ребёнка сужен до allowed"
    );
    let evs = fwd.events.lock().unwrap();
    let is_error = evs.iter().find_map(|e| match e {
        AgentEvent::ToolResult { is_error, .. } => Some(*is_error),
        _ => None,
    });
    assert_eq!(
        is_error,
        Some(true),
        "read_skill_resource отфильтрован из реестра ребёнка (allowed={{activate_skill}}) → UnknownTool is_error"
    );
}

/// SUB-3a контроль: инструмент, ВКЛЮЧённый в `allowed`, у ребёнка вызывается успешно (сужение не
/// режет лишнего). Живой тул — `activate_skill` (B7: read-only скиллы вместо вычищенных стабов).
#[tokio::test]
async fn subagent_allowed_tool_works() {
    use crate::agent::skill_tools::{ACTIVATE_SKILL_TOOL, READ_SKILL_RESOURCE_TOOL};
    let (_sk_tmp, skills) = skills_alpha();
    let (_dir, db) = open_db().await;
    let provider = FakeProvider::new(vec![
        Ok(ToolTurn::ToolCalls(vec![ToolCall {
            id: "c1".into(),
            name: ACTIVATE_SKILL_TOOL.into(),
            arguments: r#"{"skill":"alpha"}"#.into(),
        }])),
        Ok(ToolTurn::Final("ок".into())),
    ]);
    let fwd = Arc::new(CollectingForwarder::default());
    let spec = SessionSpec {
        run_id: 11,
        task: "t".into(),
        autonomy: None,
        actuator_enabled: false,
        overwrite_threshold: 100,
        blast_cap: 10,
        context_window: Some(4096),
        canon_root: _dir.path().to_path_buf(),
        history: Vec::new(),
        skills_learning_enabled: false,
    };
    let paused = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    let allowed: std::collections::BTreeSet<String> = [
        ACTIVATE_SKILL_TOOL.to_string(),
        READ_SKILL_RESOURCE_TOOL.to_string(),
    ]
    .into_iter()
    .collect();
    run_agent_session(
        &spec,
        &SessionDeps {
            provider: &provider,
            memory: None,
            skills: Some(&skills),
            web: None,
            decision_source: policy_default(),
            writer: db.writer(),
            reader: db.reader(),
            paused: &paused,
            cancel: &cancel,
            forwarder: fwd.clone(),
        },
        SessionRole::Subagent {
            allowed: &allowed,
            dispatcher: None,
        },
    )
    .await;
    let evs = fwd.events.lock().unwrap();
    let is_error = evs.iter().find_map(|e| match e {
        AgentEvent::ToolResult { is_error, .. } => Some(*is_error),
        _ => None,
    });
    assert_eq!(
        is_error,
        Some(false),
        "activate_skill в allowed → вызывается успешно"
    );
}

/// SUB-3b-2b: `delegation=None` → `delegate.run` НЕ зарегистрирован → вызов модели → UnknownTool
/// is_error (без регрессии: дефолт-поведение).
#[tokio::test]
async fn delegation_disabled_means_no_delegate_tool() {
    let (_dir, db) = open_db().await;
    let provider = FakeProvider::new(vec![
        Ok(ToolTurn::ToolCalls(vec![ToolCall {
            id: "c1".into(),
            name: "delegate.run".into(),
            arguments: r#"{"tasks":[{"goal":"x"}]}"#.into(),
        }])),
        Ok(ToolTurn::Final("ок".into())),
    ]);
    let fwd = Arc::new(CollectingForwarder::default());
    let spec = SessionSpec {
        run_id: 20,
        task: "t".into(),
        autonomy: None,
        actuator_enabled: false,
        overwrite_threshold: 100,
        blast_cap: 10,
        context_window: Some(4096),
        canon_root: _dir.path().to_path_buf(),
        history: Vec::new(),
        skills_learning_enabled: false,
    };
    let paused = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    run_agent_session(
        &spec,
        &SessionDeps {
            provider: &provider,
            memory: None,
            skills: None,
            web: None,
            decision_source: policy_default(),
            writer: db.writer(),
            reader: db.reader(),
            paused: &paused,
            cancel: &cancel,
            forwarder: fwd.clone(),
        },
        SessionRole::TopLevel {
            delegation: None, // delegation выкл
            research: None,   // research (RES-4): default-OFF; прод-проводка в RES-5
        },
    )
    .await;
    let evs = fwd.events.lock().unwrap();
    let is_error = evs.iter().find_map(|e| match e {
        AgentEvent::ToolResult { is_error, .. } => Some(*is_error),
        _ => None,
    });
    assert_eq!(
        is_error,
        Some(true),
        "delegate.run НЕ зарегистрирован при delegation=None → UnknownTool"
    );
}

/// SUB-3b-2b: `delegation=Some(enabled)` → `delegate.run` ЗАРЕГИСТРИРОВАН → вызов модели порождает
/// ребёнка (дерево parent_run_id) и возвращает агрегат (НЕ UnknownTool). Изоляция: анонимные ходы
/// ребёнка не текут в поток родителя.
#[tokio::test]
async fn delegation_enabled_registers_delegate_tool() {
    let (_dir, db) = open_db().await;
    let provider = Arc::new(FakeProvider::new(vec![
        Ok(ToolTurn::ToolCalls(vec![ToolCall {
            id: "c1".into(),
            name: "delegate.run".into(),
            arguments: r#"{"tasks":[{"goal":"под-цель"}]}"#.into(),
        }])),
        Ok(ToolTurn::Final("child done".into())),  // ребёнок
        Ok(ToolTurn::Final("parent done".into())), // родитель
    ]));
    let fwd = Arc::new(CollectingForwarder::default());
    let spec = SessionSpec {
        run_id: 21,
        task: "t".into(),
        autonomy: None,
        actuator_enabled: false,
        overwrite_threshold: 100,
        blast_cap: 10,
        context_window: Some(4096),
        canon_root: _dir.path().to_path_buf(),
        history: Vec::new(),
        skills_learning_enabled: false,
    };
    let deps = DelegationDeps {
        provider: provider.clone(),
        config: crate::ai::DelegationConfig {
            enabled: true,
            ..Default::default()
        },
    };
    let paused = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    run_agent_session(
        &spec,
        &SessionDeps {
            provider: provider.as_ref(),
            memory: None,
            skills: None,
            web: None,
            decision_source: policy_default(),
            writer: db.writer(),
            reader: db.reader(),
            paused: &paused,
            cancel: &cancel,
            forwarder: fwd.clone(),
        },
        SessionRole::TopLevel {
            delegation: Some(&deps),
            research: None, // research (RES-4): default-OFF; прод-проводка в RES-5
        },
    )
    .await;
    // Извлекаем ToolResult в блоке → guard дропается ДО await ниже (clippy await_holding_lock).
    let tr = {
        let evs = fwd.events.lock().unwrap();
        evs.iter().find_map(|e| match e {
            AgentEvent::ToolResult {
                is_error, content, ..
            } => Some((*is_error, content.clone())),
            _ => None,
        })
    };
    let (is_error, content) = tr.expect("есть ToolResult delegate.run");
    assert!(
        !is_error,
        "delegate.run зарегистрирован и отработал: {content}"
    );
    assert!(
        content.contains("child done"),
        "агрегат несёт саммари ребёнка: {content}"
    );
    // Дерево: ровно один ребёнок с parent_run_id=21.
    let kids: i64 = db
        .reader()
        .query(|c| {
            c.query_row(
                "SELECT count(*) FROM agent_runs WHERE parent_run_id=21",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(kids, 1, "порождён один ребёнок");
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
        history: Vec::new(),
        skills_learning_enabled: false,
    };
    let paused = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    let outcome = run_agent_session(
        &spec,
        &SessionDeps {
            provider: &provider,
            memory: None,
            skills: None,
            web: None,
            decision_source: policy_default(),
            writer: db.writer(),
            reader: db.reader(),
            paused: &paused,
            cancel: &cancel,
            forwarder: fwd.clone(),
        },
        SessionRole::TopLevel {
            delegation: None,
            research: None, // research (RES-4): default-OFF; прод-проводка в RES-5
        },
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
/// провайдеру), (б) tier-2/3 инструменты (`activate_skill`/`read_skill_resource`) зарегистрированы —
/// НЕЗАВИСИМО от actuator-флага (скиллы только читают); при ВЫКЛ актуаторе они РОВНО весь реестр
/// (B7: debug-стабов нет).
#[tokio::test]
async fn skills_inject_menu_and_register_tier2_3_tools() {
    use crate::agent::skill_tools::{SkillContext, ACTIVATE_SKILL_TOOL, READ_SKILL_RESOURCE_TOOL};
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
        history: Vec::new(),
        skills_learning_enabled: false,
    };
    let paused = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    let outcome = run_agent_session(
        &spec,
        &SessionDeps {
            provider: &provider,
            memory: None,
            skills: Some(&skills),
            web: None,
            decision_source: policy_default(),
            writer: db.writer(),
            reader: db.reader(),
            paused: &paused,
            cancel: &cancel,
            forwarder: fwd.clone(),
        },
        SessionRole::TopLevel {
            delegation: None,
            research: None, // research (RES-4): default-OFF; прод-проводка в RES-5
        },
    )
    .await;
    assert!(matches!(outcome, LoopOutcome::Final(_)));

    // (а) меню скилла (имя «alpha») попало в начальный контекст, отданный провайдеру.
    let msgs = provider.seen_msgs.lock().unwrap();
    assert!(
        msgs.iter().any(|m| m.contains("alpha")),
        "tier-1 меню скилла должно быть в контексте: {msgs:?}"
    );
    // (б) tier-2/3 инструменты скиллов зарегистрированы, и при ВЫКЛ актуаторе реестр состоит РОВНО
    // из них (B7: никаких debug.*-стабов в спеках, которые видит модель).
    let tools = provider.seen_tools.lock().unwrap();
    let mut names: Vec<&str> = tools.iter().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![ACTIVATE_SKILL_TOOL, READ_SKILL_RESOURCE_TOOL],
        "actuator OFF + skills → реестр ровно из двух read-only skill-тулов"
    );
}

/// W-4: `spec.history` (прошлые ходы мультитёрн-сессии) попадает в начальный контекст ПЕРЕД
/// текущей задачей. Без этого follow-up-ход не помнил контекст и не предлагал правки (ST-G3).
#[tokio::test]
async fn history_threaded_into_context_before_task() {
    let (_dir, db) = open_db().await;
    let provider = RecordingProvider {
        seen_msgs: Mutex::new(Vec::new()),
        seen_tools: Mutex::new(Vec::new()),
    };
    let fwd = Arc::new(CollectingForwarder::default());
    let spec = SessionSpec {
        run_id: 7,
        task: "теперь добавь раздел про кэш".into(),
        autonomy: None,
        actuator_enabled: false,
        overwrite_threshold: 100,
        blast_cap: 10,
        context_window: Some(8192),
        canon_root: _dir.path().to_path_buf(),
        history: vec![
            Msg::user("создай заметку про оплату"),
            Msg::assistant("Создал черновик заметки «Оплата»."),
        ],
        skills_learning_enabled: false,
    };
    let paused = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    let outcome = run_agent_session(
        &spec,
        &SessionDeps {
            provider: &provider,
            memory: None,
            skills: None,
            web: None,
            decision_source: policy_default(),
            writer: db.writer(),
            reader: db.reader(),
            paused: &paused,
            cancel: &cancel,
            forwarder: fwd.clone(),
        },
        SessionRole::TopLevel {
            delegation: None,
            research: None,
        },
    )
    .await;
    assert!(matches!(outcome, LoopOutcome::Final(_)));

    let msgs = provider.seen_msgs.lock().unwrap();
    // История ОБОИХ ролей и текущая задача — все в контексте.
    assert!(
        msgs.iter().any(|m| m.contains("создай заметку про оплату")),
        "history user-ход в контексте: {msgs:?}"
    );
    assert!(
        msgs.iter().any(|m| m.contains("черновик заметки")),
        "history assistant-ход в контексте: {msgs:?}"
    );
    // Порядок: последний элемент = ТЕКУЩАЯ задача (история строго ПЕРЕД ней).
    let last = msgs.last().cloned().unwrap_or_default();
    assert!(
        last.contains("добавь раздел про кэш"),
        "текущая задача — последняя: {last}"
    );
    let idx_hist = msgs
        .iter()
        .position(|m| m.contains("создай заметку про оплату"))
        .unwrap();
    assert!(
        idx_hist < msgs.len() - 1,
        "история строго перед текущей задачей"
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
    let url =
        std::env::var("NEXUS_LIVE_CHAT_URL").unwrap_or_else(|_| "http://192.168.0.31:8080".into());
    let model = std::env::var("NEXUS_LIVE_CHAT_MODEL").unwrap_or_else(|_| "qwen36-mtp.gguf".into());

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
        history: Vec::new(),
        skills_learning_enabled: false,
    };
    let fwd = Arc::new(CollectingForwarder::default());
    let paused = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    let outcome = run_agent_session(
        &spec,
        &SessionDeps {
            provider: provider.as_ref(),
            memory: None,
            skills: None,
            web: None,
            decision_source: policy_default(),
            writer: db.writer(),
            reader: db.reader(),
            paused: &paused,
            cancel: &cancel,
            forwarder: fwd.clone(),
        },
        SessionRole::TopLevel {
            delegation: None,
            research: None, // research (RES-4): default-OFF; прод-проводка в RES-5
        },
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

/// **Fix BF-1 №1 — ПРОВОДКА pause-accounting-декоратора (session-уровень).** Доказывает, что
/// `run_agent_session` реально ставит [`PauseAccountingDecision`] на путь гейта и отдаёт ТОТ ЖЕ счётчик
/// циклу: МЕДЛЕННОЕ человеческое решение (сон в `decide()` ДОЛЬШЕ wall_clock) не валит прогон по
/// WallClock — после аппрува цикл доходит до Final, а одобренная заметка реально записана.
/// **Мутант-гард:** передать в `GatedToolCtx` голый `deps.decision_source` вместо `gate_decision`
/// (фикс молча развинчен) → сон не кредитуется в `paused_nanos` → WallClock → тест падает.
#[tokio::test]
async fn session_slow_gate_decision_does_not_burn_wall_clock() {
    use crate::actuator::{BatchDecision, ItemDecision, ProposalBatch};
    use crate::agent::run_store;
    use std::time::Duration;

    /// ApproveAll с задержкой: эмулирует человека, думающего над changeset'ом дольше wall_clock.
    struct SlowApproveAll(Duration);
    #[async_trait]
    impl DecisionSource for SlowApproveAll {
        async fn decide(&self, batch: &ProposalBatch) -> BatchDecision {
            tokio::time::sleep(self.0).await;
            BatchDecision::from_pairs(
                batch
                    .items
                    .iter()
                    .map(|i| (i.action_id, ItemDecision::Approve)),
            )
        }
    }

    let dir = TempDir::new().unwrap();
    let canon = dir.path().canonicalize().unwrap();
    let db = Database::open(canon.join("nexus.db")).await.unwrap();
    let run_id = run_store::create_run(db.writer(), "медленное решение", None, Some("confirm"))
        .await
        .unwrap();

    // Ход 1: note.create (confirm-прогон → propose → decide спит 500мс → approve → apply). Ход 2: Final.
    let provider = FakeProvider::new(vec![
        Ok(ToolTurn::ToolCalls(vec![ToolCall {
            id: "c1".into(),
            name: "note.create".into(),
            arguments: r#"{"path":"Notes/BF1.md","content":"привет"}"#.into(),
        }])),
        Ok(ToolTurn::Final("успел".into())),
    ]);
    let spec = SessionSpec {
        run_id,
        task: "создай заметку".into(),
        history: Vec::new(),
        autonomy: Some("confirm".into()),
        actuator_enabled: true,
        overwrite_threshold: 64 * 1024,
        blast_cap: 16,
        context_window: Some(4096),
        canon_root: canon.clone(),
        skills_learning_enabled: false,
    };
    let fwd = Arc::new(CollectingForwarder::default());
    let paused = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    // wall_clock (250мс) ЗАМЕТНО МЕНЬШЕ сна решения (1с): без вычитания паузы прогон обязан упасть
    // по WallClock на границе перед ходом 2; с проводкой декоратора — дойти до Final. Слак 250мс на
    // НЕкредитуемую работу (sqlite/classify/apply) — запас против медленного CI.
    let bounds = LoopBounds {
        max_steps: 5,
        wall_clock: Duration::from_millis(250),
    };
    let outcome = run_agent_session_bounded(
        &spec,
        &SessionDeps {
            provider: &provider,
            memory: None,
            skills: None,
            web: None,
            decision_source: Arc::new(SlowApproveAll(Duration::from_millis(1000))),
            writer: db.writer(),
            reader: db.reader(),
            paused: &paused,
            cancel: &cancel,
            forwarder: fwd.clone(),
        },
        SessionRole::TopLevel {
            delegation: None,
            research: None,
        },
        bounds,
    )
    .await;
    assert_eq!(
        outcome,
        LoopOutcome::Final("успел".into()),
        "медленное решение у гейта НЕ должно жечь wall_clock (проводка декоратора): {outcome:?}"
    );
    // Одобренная правка реально применена (полный путь propose→decide→approve→apply прошёл).
    assert!(
        canon.join("Notes/BF1.md").exists(),
        "одобренная заметка должна быть записана"
    );
}
