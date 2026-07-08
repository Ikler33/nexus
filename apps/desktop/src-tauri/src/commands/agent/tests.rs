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
        LoopBounds::default(), // BF-1: границы прогона (тест — дефолт)
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
        LoopBounds::default(), // BF-1: границы прогона (тест — дефолт)
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
        LoopBounds::default(), // BF-1: границы прогона (тест — дефолт)
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
        LoopBounds::default(), // BF-1: границы прогона (тест — дефолт)
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
        LoopBounds::default(), // BF-1: границы прогона (тест — дефолт)
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
        LoopBounds::default(), // BF-1: границы прогона (тест — дефолт)
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

/// R-2 ХАРАКТЕРИЗАЦИЯ (фикстура «до/после» дедупа): полная таблица вариант → (статус, текст)
/// `finish_in_store` ЭТОГО вызывателя, точным сравнением (байт-в-байт). Тексты попадают в
/// run_store/историю прогонов/UI — канонизация R-2 обязана сохранить их без изменений.
#[tokio::test]
async fn finish_in_store_characterization_full_table() {
    use nexus_core::agent::run_store::{STATUS_CANCELLED, STATUS_DONE, STATUS_ERROR};
    use nexus_core::agent::BudgetKind;
    let (_dir, db, _canon) = open_db().await;
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
        // Свежая строка прогона на каждый вариант — finish_run пишет в реальный run_store.
        let run_id = run_store::create_run(db.writer(), "задача", Some("fake"), None)
            .await
            .unwrap();
        let debug_outcome = format!("{outcome:?}");
        let (status, text) = finish_in_store(db.writer(), run_id, outcome).await;
        assert_eq!(
            (status, text.as_str()),
            (want_status, want_text),
            "вариант: {debug_outcome}"
        );
        // Терминал реально записан в run_store с теми же статусом/текстом.
        let run = run_store::get_run(db.reader(), run_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.status, want_status, "вариант: {debug_outcome}");
        assert_eq!(
            run.outcome.as_deref(),
            Some(want_text),
            "вариант: {debug_outcome}"
        );
    }
}
