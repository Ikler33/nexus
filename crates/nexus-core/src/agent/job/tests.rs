use super::*;
use crate::agent::tool::{ToolCall, ToolSpec};
use crate::ai::tools::{ToolCapableProvider, ToolTurn};
use crate::ai::{AiResult, ChatMessage};
use crate::db::Database;
use crate::net::{EgressAudit, EgressFeature, EgressPolicy, GuardedClient, RunCtx};
use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use tempfile::TempDir;

use crate::agent::run_store::{STATUS_CANCELLED, STATUS_DONE};
use crate::agent::runner::{BudgetKind, LoopOutcome};

/// R-2 ХАРАКТЕРИЗАЦИЯ (фикстура «до/после» дедупа): параметризация канона ЭТОЙ джобы
/// (PausePolicy::Requeue + «прогон отменён») — терминальные армы байт-в-байт как у инлайн-копии
/// до дедупа; Paused → Park, НЕ терминал (requeue-путь; живой прогон доказывает
/// `paused_actuator_run_writes_nothing_and_requeues`).
#[test]
fn outcome_to_finish_characterization_full_table() {
    let be = |kind: BudgetKind| LoopOutcome::BudgetExhausted {
        kind,
        partial: "часть".into(),
    };
    let table: [(LoopOutcome, &str, &str); 6] = [
        (LoopOutcome::Final("итог".into()), STATUS_DONE, "итог"),
        (
            be(BudgetKind::Cancelled),
            STATUS_CANCELLED,
            "прогон отменён; частичный ответ: часть",
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
        let got = outcome_to_finish(&outcome, PausePolicy::Requeue, CancelWording::RunCancelled);
        assert_eq!(
            got,
            RunFinish::Finalize {
                status: want_status,
                text: want_text.into()
            },
            "вариант: {outcome:?}"
        );
    }
    // Пауза мид-ран — парковка (7a), finish_run НЕ пишется.
    assert_eq!(
        outcome_to_finish(
            &be(BudgetKind::Paused),
            PausePolicy::Requeue,
            CancelWording::RunCancelled
        ),
        RunFinish::Park
    );
}

async fn open() -> (TempDir, Database) {
    let dir = TempDir::new().unwrap();
    let db = Database::open(dir.path().join(".nexus/nexus.db"))
        .await
        .unwrap();
    (dir, db)
}

/// Фейковый tool-capable провайдер (как в runner-тестах): отдаёт скриптованную последовательность
/// ходов. Опционально на КАЖДОМ ходу делает реальный guarded-эгресс (для теста корреляции run_id).
struct FakeToolProvider {
    turns: Mutex<VecDeque<AiResult<ToolTurn>>>,
    /// Если задан — на каждом ходу шлём guarded GET сюда (эгресс под текущим run_id).
    egress: Option<(GuardedClient, String)>,
}

impl FakeToolProvider {
    fn scripted(turns: Vec<AiResult<ToolTurn>>) -> Self {
        Self {
            turns: Mutex::new(turns.into_iter().collect()),
            egress: None,
        }
    }
    fn with_egress(turns: Vec<AiResult<ToolTurn>>, client: GuardedClient, url: String) -> Self {
        Self {
            turns: Mutex::new(turns.into_iter().collect()),
            egress: Some((client, url)),
        }
    }
}

#[async_trait]
impl ToolCapableProvider for FakeToolProvider {
    async fn stream_chat_tools(
        &self,
        _messages: &[ChatMessage],
        _tools: &[ToolSpec],
        _on_token: &mut (dyn FnMut(String) + Send),
        _cancel: &Arc<AtomicBool>,
        ctx: RunCtx,
    ) -> AiResult<ToolTurn> {
        if let Some((client, url)) = &self.egress {
            // Реальный guarded-эгресс на loopback-мок: durable-строка понесёт run_id из ПРОБРОШЕННОГО
            // per-call ctx (не из глобального слота) — так конкурентные прогоны не путают атрибуцию.
            let _ = client.get(url, EgressFeature::Chat, ctx).await;
        }
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

/// AIClient с заданным agent_tools-провайдером (остальное None). policy не используется тестом.
fn ai_with_tools(provider: Option<Arc<dyn ToolCapableProvider>>) -> Arc<AIClient> {
    Arc::new(AIClient {
        chat: None,
        chat_fast: None,
        chat_util: None,
        embedder: None,
        agent_tools: provider,
        policy: Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false)))),
    })
}

/// Вызов `debug.echo` из фейк-модели. B7: в прод-реестре (actuator OFF) стаба больше НЕТ —
/// диспатч даёт UnknownTool-ошибку; для lifecycle-тестов это ок (ToolResult всё равно эмитится).
fn echo_call(id: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "debug.echo".into(),
        arguments: r#"{"text":"привет"}"#.into(),
    }
}

/// Мок-сервер одного запроса на loopback (для теста эгресс-корреляции).
fn serve_once() -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf);
            let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        }
    });
    (addr, handle)
}

/// Process-global kill-switch для тестов: по умолчанию НЕ взведён (без регрессии поведения).
fn unpaused() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// Хендлер для AGENT-2-тестов: актуатор ВЫКЛ (без инструментов записи) — не регрессируем
/// поведение AGENT-2/MEM-1.
fn handler(db: &Database, ai: Arc<AIClient>) -> AgentRunHandler {
    handler_paused(db, ai, unpaused())
}

/// Хендлер как [`handler`], но с ЗАДАННЫМ kill-switch (AGENT-5 тесты паузы). Актуатор ВЫКЛ.
fn handler_paused(
    db: &Database,
    ai: Arc<AIClient>,
    agent_paused: Arc<AtomicBool>,
) -> AgentRunHandler {
    AgentRunHandler::new(
        db.writer().clone(),
        db.reader().clone(),
        ai,
        Some(32768),
        None,
        std::env::temp_dir(), // canon_root не используется при actuator_enabled=false
        false,                // actuator ВЫКЛ
        64 * 1024,
        16,
        Arc::new(crate::actuator::PolicyDefault),
        agent_paused,
        None,  // SKILL-2: без skills (AGENT-2-поведение, без регрессии)
        None,  // EGR-AGENT-2: без веба
        false, // SL-7d: тест не про авторство навыков
        crate::ai::DelegationConfig::default(), // SUB-3b-2b: тест не про делегирование
        crate::ai::ResearchConfig::default(), // RES-5: тест не про ресёрч
    )
}

/// Хендлер с подключённой памятью (AGENT-MEM-1): доказывает, что recall попадает в начальный
/// контекст между system-преамбулом и задачей. Актуатор ВЫКЛ.
fn handler_with_memory(
    db: &Database,
    ai: Arc<AIClient>,
    memory: Arc<dyn AgentMemory>,
) -> AgentRunHandler {
    AgentRunHandler::new(
        db.writer().clone(),
        db.reader().clone(),
        ai,
        Some(32768),
        Some(memory),
        std::env::temp_dir(),
        false,
        64 * 1024,
        16,
        Arc::new(crate::actuator::PolicyDefault),
        unpaused(),
        None,                                   // SKILL-2: без skills
        None,                                   // EGR-AGENT-2: без веба
        false,                                  // SL-7d: тест не про авторство навыков
        crate::ai::DelegationConfig::default(), // SUB-3b-2b: тест не про делегирование
        crate::ai::ResearchConfig::default(),   // RES-5: тест не про ресёрч
    )
}

/// Хендлер с ВКЛЮЧЁННЫМ актуатором (AGENT-3e) для go-live-тестов: гейтнутый реестр на `canon_root`,
/// заданный `decision_source`. Порог/кэп — параметры теста. kill-switch НЕ взведён.
fn handler_with_actuator(
    db: &Database,
    ai: Arc<AIClient>,
    canon_root: std::path::PathBuf,
    overwrite_threshold: usize,
    blast_cap: u32,
    decision_source: Arc<dyn crate::actuator::DecisionSource>,
) -> AgentRunHandler {
    handler_with_actuator_paused(
        db,
        ai,
        canon_root,
        overwrite_threshold,
        blast_cap,
        decision_source,
        unpaused(),
    )
}

/// Как [`handler_with_actuator`], но с ЗАДАННЫМ kill-switch (AGENT-5: пауза + актуатор).
#[allow(clippy::too_many_arguments)]
fn handler_with_actuator_paused(
    db: &Database,
    ai: Arc<AIClient>,
    canon_root: std::path::PathBuf,
    overwrite_threshold: usize,
    blast_cap: u32,
    decision_source: Arc<dyn crate::actuator::DecisionSource>,
    agent_paused: Arc<AtomicBool>,
) -> AgentRunHandler {
    AgentRunHandler::new(
        db.writer().clone(),
        db.reader().clone(),
        ai,
        Some(32768),
        None,
        canon_root,
        true, // actuator ВКЛ
        overwrite_threshold,
        blast_cap,
        decision_source,
        agent_paused,
        None,  // SKILL-2: без skills (go-live-тесты актуатора не про скиллы)
        None,  // EGR-AGENT-2: без веба
        false, // SL-7d: тест не про авторство навыков
        crate::ai::DelegationConfig::default(), // SUB-3b-2b: тест не про делегирование
        crate::ai::ResearchConfig::default(), // RES-5: тест не про ресёрч
    )
}

fn job_for(run_id: i64) -> Job {
    Job {
        id: 1,
        kind: KIND_AGENT_RUN.into(),
        payload: run_id.to_string(),
        state: "running".into(),
        run_at: 0,
        attempts: 0,
        max_attempts: 3,
        last_error: None,
    }
}

/// Lifecycle: handle с FakeToolProvider (ToolCalls→Final) ведёт прогон → done, исход установлен,
/// step бампнут (через ToolResult; B7: при ВЫКЛ актуаторе `debug.echo` не зарегистрирован →
/// UnknownTool-результат — он ТОЖЕ ToolResult, счётчик шагов честно бампается).
#[tokio::test]
async fn handle_drives_loop_to_done() {
    let (_d, db) = open().await;
    let provider = Arc::new(FakeToolProvider::scripted(vec![
        Ok(ToolTurn::ToolCalls(vec![echo_call("c1")])),
        Ok(ToolTurn::Final("итог".into())),
    ]));
    let ai = ai_with_tools(Some(provider));
    let h = handler(&db, ai);

    let run_id = run_store::create_run(db.writer(), "задача", Some("fake"), Some("auto"))
        .await
        .unwrap();
    h.handle(&job_for(run_id)).await.expect("джоба ok");

    // Шаг персистится синхронно ДО возврата handle (awaited bump_step перед finish) — без поллинга.
    let r = run_store::get_run(db.reader(), run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.status, STATUS_DONE);
    assert_eq!(r.outcome.as_deref(), Some("итог"));
    assert!(r.step >= 1, "step бампнут по ToolResult: {}", r.step);
}

/// Идемпотентность: handle уже-'done' прогона — НЕ перезапускает цикл (провайдер не зовётся;
/// исход не меняется).
#[tokio::test]
async fn handle_on_terminal_run_is_noop() {
    let (_d, db) = open().await;
    // Провайдер, который ПАНИКует если позван — доказывает, что цикл не запускался.
    struct PanicProvider;
    #[async_trait]
    impl ToolCapableProvider for PanicProvider {
        async fn stream_chat_tools(
            &self,
            _m: &[ChatMessage],
            _t: &[ToolSpec],
            _o: &mut (dyn FnMut(String) + Send),
            _c: &Arc<AtomicBool>,
            _ctx: RunCtx,
        ) -> AiResult<ToolTurn> {
            panic!("провайдер не должен вызываться для терминального прогона");
        }
        fn model_id(&self) -> &str {
            "panic"
        }
    }
    let ai = ai_with_tools(Some(Arc::new(PanicProvider)));
    let h = handler(&db, ai);

    let run_id = run_store::create_run(db.writer(), "t", None, None)
        .await
        .unwrap();
    // Сразу финишируем как done.
    run_store::mark_running(db.writer(), run_id).await.unwrap();
    run_store::finish_run(db.writer(), run_id, STATUS_DONE, Some("исходный"))
        .await
        .unwrap();

    h.handle(&job_for(run_id)).await.expect("идемпотентный ok");
    let r = run_store::get_run(db.reader(), run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.status, STATUS_DONE);
    assert_eq!(
        r.outcome.as_deref(),
        Some("исходный"),
        "исход не перезаписан"
    );
}

/// Деградация: agent_tools=None → прогон финишируется error чисто (джоба ok — lifecycle доказан).
/// Эгресс ВНЕ прогона (по своему пути с [`RunCtx::NONE`]) несёт run_id=NULL — корреляция не
/// «протекает» из завершённого прогона (RunCtx per-call, нет глобального слота, который мог бы залипнуть).
#[tokio::test]
async fn handle_without_tools_finishes_error_egress_outside_run_is_uncorrelated() {
    let (_d, db) = open().await;
    let audit = Arc::new(EgressAudit::default());
    let ai = ai_with_tools(None);
    let h = handler(&db, ai);

    let run_id = run_store::create_run(db.writer(), "t", None, None)
        .await
        .unwrap();
    h.handle(&job_for(run_id))
        .await
        .expect("джоба ok даже без tools");
    let r = run_store::get_run(db.reader(), run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.status, STATUS_ERROR);
    assert_eq!(r.outcome.as_deref(), Some("agent tools unavailable"));

    // Эгресс ВНЕ прогона (явный RunCtx::NONE) → durable-запись несёт run_id=NULL.
    audit.set_writer(db.writer().clone());
    let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
    let (addr, server) = serve_once();
    let client = GuardedClient::new(policy, audit.clone(), |b| b).unwrap();
    client
        .get(
            &format!("http://{addr}/x"),
            EgressFeature::Probe,
            RunCtx::NONE,
        )
        .await
        .expect("loopback ok");
    server.join().unwrap();
    let run_ids = durable_run_ids(&db).await;
    assert_eq!(
        run_ids.last(),
        Some(&None),
        "эгресс вне прогона (RunCtx::NONE): run_id=NULL: {run_ids:?}"
    );
}

/// run_id-корреляция (AGENT-3a): во время прогона guarded-эгресс несёт run_id == id прогона
/// (ПРОБРОШЕННЫЙ per-call RunCtx); эгресс ВНЕ прогона (явный RunCtx::NONE) несёт run_id=NULL.
#[tokio::test]
async fn egress_during_run_is_correlated_uncorrelated_outside() {
    let (_d, db) = open().await;
    let audit = Arc::new(EgressAudit::default());
    audit.set_writer(db.writer().clone());

    // Guarded-клиент на loopback-мок (local-first проходит). Провайдер делает эгресс на КАЖДОМ ходу.
    let (addr, server) = serve_once();
    let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
    let client = GuardedClient::new(policy, audit.clone(), |b| b).unwrap();
    let url = format!("http://{addr}/v1/chat");
    // Один ход: Final (один эгресс внутри прогона).
    let provider = Arc::new(FakeToolProvider::with_egress(
        vec![Ok(ToolTurn::Final("done".into()))],
        client.clone(),
        url.clone(),
    ));
    let ai = ai_with_tools(Some(provider));
    let h = handler(&db, ai);

    let run_id = run_store::create_run(db.writer(), "t", None, None)
        .await
        .unwrap();
    h.handle(&job_for(run_id)).await.expect("джоба ok");
    server.join().unwrap();

    // Эгресс ВНЕ прогона (тот же клиент, но явный RunCtx::NONE): несёт run_id=NULL — корреляция
    // не «протекает» из завершённого прогона (нет глобального слота; ctx — per-call).
    let (addr2, server2) = serve_once();
    client
        .get(
            &format!("http://{addr2}/after"),
            EgressFeature::Probe,
            RunCtx::NONE,
        )
        .await
        .expect("loopback ok");
    server2.join().unwrap();

    let run_ids = durable_run_ids(&db).await;
    assert!(
        run_ids.contains(&Some(run_id)),
        "эгресс внутри прогона коррелирован на run_id={run_id}: {run_ids:?}"
    );
    assert_eq!(
        run_ids.last(),
        Some(&None),
        "эгресс вне прогона (RunCtx::NONE): run_id=NULL: {run_ids:?}"
    );
}

/// **THE GATE (AGENT-3a, регрессия-гард конкурентности)**: ДВА overlapping прогона A и B, каждый
/// делает guarded-эгресс к СВОЕЙ хост-идентичности (`run-a.test` / `run-b.test`), драйвятся
/// ИНТЕРЛИВНО через `tokio::join!` двух `handle`-вызовов. Каждый провайдер на КАЖДОМ ходу шлёт
/// несколько GET'ов с `yield_now` между ними → исполнения двух прогонов чередуются на рантайме.
///
/// ИНВАРИАНТ: КАЖДАЯ durable-строка `egress_audit` с host=`run-a.test` несёт run_id == run_a, а
/// каждая с host=`run-b.test` — run_id == run_b. НОЛЬ кросс-тегирования.
///
/// Почему ВАЛИЛОСЬ на старом процесс-глобальном слоте: `set_run(A)` и `set_run(B)` писали в ОДИН
/// `Mutex<Option<i64>>`; при чередовании B перетирал слот, и часть эгресса прогона A читала слот=B
/// (и наоборот) → строки `run-a.test` с run_id=B. С per-call `RunCtx` слота нет: каждый прогон
/// несёт свой ctx в СВОЁМ стеке вызова до самого `record()`, перетереть нечем.
#[tokio::test]
async fn concurrent_runs_tag_egress_independently() {
    // Резолвер: ЛЮБОЙ хост → loopback-адрес мок-сервера (домены проходят как allowlisted +
    // резолвятся в loopback, который для Chat допустим local-first; host в audit = доменное имя).
    struct ToLoopback(std::net::IpAddr);
    #[async_trait]
    impl crate::net::Resolver for ToLoopback {
        async fn resolve(&self, _host: &str) -> std::io::Result<Vec<std::net::IpAddr>> {
            Ok(vec![self.0])
        }
    }

    // Мок-сервер, принимающий МНОГО соединений (оба прогона бьют по одному адресу; различаем по
    // доменному host в audit, не по сокету). Дренажный поток в фоне.
    fn serve_many() -> std::net::SocketAddr {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                match conn {
                    Ok(mut sock) => {
                        let mut buf = [0u8; 1024];
                        let _ = sock.read(&mut buf);
                        let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
                    }
                    Err(_) => break,
                }
            }
        });
        addr
    }

    /// Провайдер, делающий за ход несколько guarded-GET'ов к своему URL с `yield_now` между ними
    /// (форсит чередование двух прогонов на рантайме), затем Final.
    struct MultiEgressProvider {
        client: GuardedClient,
        url: String,
        per_turn: usize,
    }
    #[async_trait]
    impl ToolCapableProvider for MultiEgressProvider {
        async fn stream_chat_tools(
            &self,
            _m: &[ChatMessage],
            _t: &[ToolSpec],
            _o: &mut (dyn FnMut(String) + Send),
            _c: &Arc<AtomicBool>,
            ctx: RunCtx,
        ) -> AiResult<ToolTurn> {
            for _ in 0..self.per_turn {
                // Эгресс под ПРОБРОШЕННЫМ ctx (run_id этого прогона). yield → даём шанс другому прогону.
                let _ = self.client.get(&self.url, EgressFeature::Chat, ctx).await;
                tokio::task::yield_now().await;
            }
            Ok(ToolTurn::Final("done".into()))
        }
        fn model_id(&self) -> &str {
            "multi-egress"
        }
    }

    let (_d, db) = open().await;
    let audit = Arc::new(EgressAudit::default());
    audit.set_writer(db.writer().clone());
    let mock_ip = serve_many().ip();

    // Общая политика: ОБА доменных хоста в allowlist (host-гейт пропустит), резолв → loopback.
    let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
    policy.set_allowlist(["run-a.test".to_string(), "run-b.test".to_string()]);
    let make_client = || {
        GuardedClient::new(policy.clone(), audit.clone(), |b| b)
            .unwrap()
            .with_resolver(Arc::new(ToLoopback(mock_ip)))
    };

    const PER_TURN: usize = 6;
    let provider_a = Arc::new(MultiEgressProvider {
        client: make_client(),
        url: "http://run-a.test/v1/chat".to_string(),
        per_turn: PER_TURN,
    });
    let provider_b = Arc::new(MultiEgressProvider {
        client: make_client(),
        url: "http://run-b.test/v1/chat".to_string(),
        per_turn: PER_TURN,
    });

    let ai_a = ai_with_tools(Some(provider_a));
    let ai_b = ai_with_tools(Some(provider_b));
    let h_a = handler(&db, ai_a);
    let h_b = handler(&db, ai_b);

    let run_a = run_store::create_run(db.writer(), "задача A", None, None)
        .await
        .unwrap();
    let run_b = run_store::create_run(db.writer(), "задача B", None, None)
        .await
        .unwrap();
    assert_ne!(run_a, run_b);

    // ИНТЕРЛИВНЫЙ драйв: оба прогона исполняются конкурентно.
    let job_a = job_for(run_a);
    let job_b = job_for(run_b);
    let (ra, rb) = tokio::join!(h_a.handle(&job_a), h_b.handle(&job_b));
    ra.expect("джоба A ok");
    rb.expect("джоба B ok");

    // Снимок durable: (host, run_id). Проверяем НОЛЬ кросс-тегирования.
    let rows = durable_host_run_ids(&db).await;
    let a_rows: Vec<_> = rows.iter().filter(|(h, _)| h == "run-a.test").collect();
    let b_rows: Vec<_> = rows.iter().filter(|(h, _)| h == "run-b.test").collect();
    assert!(!a_rows.is_empty(), "прогон A сделал эгресс: {rows:?}");
    assert!(!b_rows.is_empty(), "прогон B сделал эгресс: {rows:?}");

    for (host, rid) in &a_rows {
        assert_eq!(
            *rid,
            Some(run_a),
            "host={host} (прогон A) обязан нести run_id={run_a}, а не {rid:?} — КРОСС-ТЕГИРОВАНИЕ"
        );
    }
    for (host, rid) in &b_rows {
        assert_eq!(
            *rid,
            Some(run_b),
            "host={host} (прогон B) обязан нести run_id={run_b}, а не {rid:?} — КРОСС-ТЕГИРОВАНИЕ"
        );
    }
    // И симметрично: НИ одна строка run_a не привязана к чужому хосту, и наоборот.
    for (host, rid) in &rows {
        if *rid == Some(run_a) {
            assert_eq!(host, "run-a.test", "run_id=A на чужом хосте {host}");
        }
        if *rid == Some(run_b) {
            assert_eq!(host, "run-b.test", "run_id=B на чужом хосте {host}");
        }
    }
}

/// AGENT-3a (per-call корреляция без скоупа/слота): эгресс с ПРОБРОШЕННЫМ RunCtx::run несёт этот
/// run_id; следующий эгресс с RunCtx::NONE по тому же клиенту — снова None. Нет общего состояния,
/// которое могло бы «залипнуть» (заменяет удалённый `run_scope_resets_set_run_on_panic`: с явным
/// per-call ctx нет слота, который паника могла бы оставить выставленным).
#[tokio::test]
async fn per_call_runctx_does_not_leak_between_egress() {
    let audit = Arc::new(EgressAudit::default());
    let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
    let client = GuardedClient::new(policy, audit.clone(), |b| b).unwrap();

    // Эгресс с ctx run=7 (denied — без сокета) несёт run_id=Some(7).
    let _ = client
        .get(
            "http://blocked.example.com/x",
            EgressFeature::Probe,
            RunCtx::run(7),
        )
        .await;
    assert_eq!(
        audit.entries().last().and_then(|e| e.run_id),
        Some(7),
        "эгресс с RunCtx::run(7) несёт run_id=7"
    );

    // Следующий эгресс с RunCtx::NONE — снова None: ctx не «протекает» (нет глобального слота).
    let _ = client
        .get(
            "http://blocked.example.com/z",
            EgressFeature::Probe,
            RunCtx::NONE,
        )
        .await;
    assert_eq!(
        audit.entries().last().and_then(|e| e.run_id),
        None,
        "следующий эгресс с RunCtx::NONE → run_id=None (ctx per-call, не залипает)"
    );
}

/// AGENT-MEM-1: с подключённой MockAgentMemory recall попадает в НАЧАЛЬНЫЙ контекст прогона —
/// провайдер на первом ходу видит сообщения `[system преамбул, recall-факт (user), задача
/// (user)]` именно в этом порядке. Доказывает проводку recall между system и task.
#[tokio::test]
async fn handler_injects_recall_between_system_and_task() {
    use crate::agent::memory::MockAgentMemory;

    let (_d, db) = open().await;

    // Провайдер, ЗАХВАТЫВАЮЩИЙ messages первого хода, затем Final.
    struct CapturingProvider {
        seen: Mutex<Option<Vec<ChatMessage>>>,
    }
    #[async_trait]
    impl ToolCapableProvider for CapturingProvider {
        async fn stream_chat_tools(
            &self,
            messages: &[ChatMessage],
            _t: &[ToolSpec],
            _o: &mut (dyn FnMut(String) + Send),
            _c: &Arc<AtomicBool>,
            _ctx: RunCtx,
        ) -> AiResult<ToolTurn> {
            let mut slot = self.seen.lock().unwrap();
            if slot.is_none() {
                *slot = Some(messages.to_vec());
            }
            Ok(ToolTurn::Final("итог".into()))
        }
        fn model_id(&self) -> &str {
            "capturing"
        }
    }
    let provider = Arc::new(CapturingProvider {
        seen: Mutex::new(None),
    });
    let ai = ai_with_tools(Some(provider.clone()));

    // Канонический recall: один факт (роль user — ДАННЫЕ, I-5).
    let canned = vec![ChatMessage::user(
        "⟦m⟧\nфакт #1\nпользователь любит Rust\n⟦m⟧",
    )];
    let mem: Arc<dyn AgentMemory> = Arc::new(MockAgentMemory::with_canned(canned));
    let h = handler_with_memory(&db, ai, mem);

    let run_id = run_store::create_run(db.writer(), "почини сборку", None, Some("auto"))
        .await
        .unwrap();
    h.handle(&job_for(run_id)).await.expect("джоба ok");

    let seen = provider
        .seen
        .lock()
        .unwrap()
        .clone()
        .expect("ход состоялся");
    assert_eq!(seen.len(), 3, "system + recall + task: {seen:?}");
    assert_eq!(seen[0].role, "system", "первым — системный преамбул");
    assert_eq!(seen[1].role, "user", "вторым — recall (ДАННЫЕ роли user)");
    assert!(
        seen[1].content.contains("пользователь любит Rust"),
        "recall-факт в начальном контексте: {}",
        seen[1].content
    );
    assert_eq!(seen[2].role, "user", "последним — задача пользователя");
    assert_eq!(seen[2].content, "почини сборку", "задача последней");
}

/// AGENT-MEM-1: БЕЗ памяти (memory=None) начальный контекст = `[system, task]` — поведение
/// AGENT-2 без регрессии (recall не вставляется).
#[tokio::test]
async fn handler_without_memory_keeps_agent2_context() {
    let (_d, db) = open().await;
    struct CapturingProvider {
        seen: Mutex<Option<Vec<ChatMessage>>>,
    }
    #[async_trait]
    impl ToolCapableProvider for CapturingProvider {
        async fn stream_chat_tools(
            &self,
            messages: &[ChatMessage],
            _t: &[ToolSpec],
            _o: &mut (dyn FnMut(String) + Send),
            _c: &Arc<AtomicBool>,
            _ctx: RunCtx,
        ) -> AiResult<ToolTurn> {
            let mut slot = self.seen.lock().unwrap();
            if slot.is_none() {
                *slot = Some(messages.to_vec());
            }
            Ok(ToolTurn::Final("ok".into()))
        }
        fn model_id(&self) -> &str {
            "capturing"
        }
    }
    let provider = Arc::new(CapturingProvider {
        seen: Mutex::new(None),
    });
    let ai = ai_with_tools(Some(provider.clone()));
    let h = handler(&db, ai); // memory=None

    let run_id = run_store::create_run(db.writer(), "задача", None, None)
        .await
        .unwrap();
    h.handle(&job_for(run_id)).await.expect("джоба ok");

    let seen = provider
        .seen
        .lock()
        .unwrap()
        .clone()
        .expect("ход состоялся");
    assert_eq!(seen.len(), 2, "без памяти — только system + task: {seen:?}");
    assert_eq!(seen[0].role, "system");
    assert_eq!(seen[1].role, "user");
    assert_eq!(seen[1].content, "задача");
}

// ── SKILL-2: проводка SkillContext (меню в контекст + tier-2/3 инструменты в реестр) ────────────

/// Провайдер, ЗАХВАТЫВАЮЩИЙ messages И tool-specs первого хода, затем Final.
struct CaptureMsgsAndTools {
    seen_msgs: Mutex<Option<Vec<ChatMessage>>>,
    seen_tools: Mutex<Option<Vec<ToolSpec>>>,
}
#[async_trait]
impl ToolCapableProvider for CaptureMsgsAndTools {
    async fn stream_chat_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
        _o: &mut (dyn FnMut(String) + Send),
        _c: &Arc<AtomicBool>,
        _ctx: RunCtx,
    ) -> AiResult<ToolTurn> {
        let mut m = self.seen_msgs.lock().unwrap();
        if m.is_none() {
            *m = Some(messages.to_vec());
            *self.seen_tools.lock().unwrap() = Some(tools.to_vec());
        }
        Ok(ToolTurn::Final("ok".into()))
    }
    fn model_id(&self) -> &str {
        "capture-msgs-tools"
    }
}

/// Хендлер с подключённым SkillContext (actuator ВЫКЛ — скиллы read-only, работают и без него).
fn handler_with_skills(db: &Database, ai: Arc<AIClient>, skills: SkillContext) -> AgentRunHandler {
    AgentRunHandler::new(
        db.writer().clone(),
        db.reader().clone(),
        ai,
        Some(32768),
        None,
        std::env::temp_dir(),
        false,
        64 * 1024,
        16,
        Arc::new(crate::actuator::PolicyDefault),
        unpaused(),
        Some(skills),
        None,                                   // EGR-AGENT-2: без веба
        false,                                  // SL-7d: тест не про авторство навыков
        crate::ai::DelegationConfig::default(), // SUB-3b-2b: тест не про делегирование
        crate::ai::ResearchConfig::default(),   // RES-5: тест не про ресёрч
    )
}

/// Строит SkillContext из временного skills-каталога с одним скиллом.
fn skills_ctx_one(name: &str, desc: &str, body: &str) -> (TempDir, SkillContext) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let d = root.join(name);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {desc}\n---\n{body}"),
    )
    .unwrap();
    let cat = Arc::new(crate::skills::discover_skills(&root));
    let ctx = SkillContext::new(cat, root);
    (tmp, ctx)
}

/// **skills сконфигурированы → tier-1 МЕНЮ инжектится (user-role, фенсен, без тела) + tier-2/3
/// инструменты зарегистрированы.** Провайдер видит меню в начальном контексте и оба инструмента
/// в tool-specs. Меню — роль user (I-5), и НЕ содержит тела скилла.
#[tokio::test]
async fn skills_configured_injects_menu_and_registers_tools() {
    let (_d, db) = open().await;
    let (_sktmp, skills) = skills_ctx_one("pdf-tools", "Работа с PDF", "ТЕЛО-СЕКРЕТ-tier2");
    let provider = Arc::new(CaptureMsgsAndTools {
        seen_msgs: Mutex::new(None),
        seen_tools: Mutex::new(None),
    });
    let ai = ai_with_tools(Some(provider.clone()));
    let h = handler_with_skills(&db, ai, skills);

    let run_id = run_store::create_run(db.writer(), "сделай PDF", None, Some("auto"))
        .await
        .unwrap();
    h.handle(&job_for(run_id)).await.expect("джоба ok");

    // Меню в контексте: [system, skill-menu(user), task(user)].
    let msgs = provider.seen_msgs.lock().unwrap().clone().unwrap();
    assert_eq!(msgs.len(), 3, "system + меню + задача: {msgs:?}");
    assert_eq!(msgs[0].role, "system");
    assert_eq!(
        msgs[1].role, "user",
        "меню скиллов — ДАННЫЕ роли user (I-5), не system"
    );
    assert!(
        msgs[1].content.contains("pdf-tools"),
        "меню несёт имя скилла"
    );
    assert!(
        msgs[1].content.contains("Работа с PDF"),
        "меню несёт описание"
    );
    assert!(
        !msgs[1].content.contains("ТЕЛО-СЕКРЕТ-tier2"),
        "tier-1 меню НЕ содержит тела: {}",
        msgs[1].content
    );
    assert_eq!(msgs[2].content, "сделай PDF", "задача последней");
    // НИ ОДНО сообщение роли system не несёт контента скилла (I-5).
    for m in &msgs {
        if m.role == "system" {
            assert!(
                !m.content.contains("pdf-tools"),
                "скилл НЕ в system: {}",
                m.content
            );
        }
    }

    // Инструменты: activate_skill + read_skill_resource зарегистрированы (B7: стабов нет,
    // при ВЫКЛ актуаторе это весь реестр).
    let tools = provider.seen_tools.lock().unwrap().clone().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(
        names.contains(&"activate_skill"),
        "tier-2 инструмент: {names:?}"
    );
    assert!(
        names.contains(&"read_skill_resource"),
        "tier-3 инструмент: {names:?}"
    );
    // enum activate_skill отражает текущий каталог.
    let act = tools.iter().find(|t| t.name == "activate_skill").unwrap();
    let enum_names: Vec<&str> = act.parameters["properties"]["skill"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(enum_names, vec!["pdf-tools"], "enum = живой каталог");
}

/// **skills НЕ сконфигурированы → НЕТ меню, НЕТ инструментов скиллов (no-regression).** Контекст =
/// [system, task]; tool-specs не содержат activate_skill/read_skill_resource (реестр пуст, B7).
#[tokio::test]
async fn skills_absent_no_menu_no_tools_no_regression() {
    let (_d, db) = open().await;
    let provider = Arc::new(CaptureMsgsAndTools {
        seen_msgs: Mutex::new(None),
        seen_tools: Mutex::new(None),
    });
    let ai = ai_with_tools(Some(provider.clone()));
    let h = handler(&db, ai); // skills=None

    let run_id = run_store::create_run(db.writer(), "задача", None, None)
        .await
        .unwrap();
    h.handle(&job_for(run_id)).await.expect("джоба ok");

    let msgs = provider.seen_msgs.lock().unwrap().clone().unwrap();
    assert_eq!(msgs.len(), 2, "без skills — только system + task: {msgs:?}");
    let tools = provider.seen_tools.lock().unwrap().clone().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(
        !names.contains(&"activate_skill") && !names.contains(&"read_skill_resource"),
        "без skills инструменты скиллов НЕ зарегистрированы: {names:?}"
    );
}

/// enqueue_agent_run: создаёт queued-прогон И джобу KIND_AGENT_RUN с payload=run_id.
#[tokio::test]
async fn enqueue_agent_run_creates_run_and_job() {
    let (_d, db) = open().await;
    let run_id = enqueue_agent_run(db.writer(), "задача", Some("m"), Some("confirm"))
        .await
        .unwrap();
    let r = run_store::get_run(db.reader(), run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.status, run_store::STATUS_QUEUED);
    // Джоба клеймится и несёт payload=run_id.
    let j = scheduler::claim_next(db.writer(), scheduler::now_secs() + 10)
        .await
        .unwrap()
        .expect("джоба поставлена");
    assert_eq!(j.kind, KIND_AGENT_RUN);
    assert_eq!(j.payload, run_id.to_string());
}

/// Backpressure (S5) через РЕАЛЬНЫЙ движок планировщика: при `busy=true` agent_run-джоба НЕ
/// исполняется (прогон остаётся `queued`, цикл провайдера не зовётся); при `busy=false` —
/// исполняется до `done`. Доказывает, что глобальный busy-гейт `run_due` уважает
/// `AgentRunHandler::defer_under_interactive()==true`.
#[tokio::test]
async fn run_due_defers_agent_run_under_interactive() {
    use std::collections::HashMap;
    let (_d, db) = open().await;
    // Провайдер ПАНИКует при вызове — доказывает, что под busy цикл не стартовал.
    struct PanicProvider;
    #[async_trait]
    impl ToolCapableProvider for PanicProvider {
        async fn stream_chat_tools(
            &self,
            _m: &[ChatMessage],
            _t: &[ToolSpec],
            _o: &mut (dyn FnMut(String) + Send),
            _c: &Arc<AtomicBool>,
            _ctx: RunCtx,
        ) -> AiResult<ToolTurn> {
            panic!("под backpressure цикл не должен стартовать");
        }
        fn model_id(&self) -> &str {
            "panic"
        }
    }
    let ai = ai_with_tools(Some(Arc::new(PanicProvider)));
    let h: Arc<dyn JobHandler> = Arc::new(handler(&db, ai));
    let mut reg = scheduler::Registry::new();
    reg.insert(KIND_AGENT_RUN.to_string(), h);

    let run_id = enqueue_agent_run(db.writer(), "t", None, None)
        .await
        .unwrap();
    // `now` ЗА run_at джобы (enqueue ставит run_at=now_secs()) → джоба ГОТОВА: дефер должен быть
    // именно от busy-гейта, а не оттого, что run_at в будущем (иначе тест зелёный по ложной причине).
    let now = scheduler::now_secs() + 100;

    // busy=true → отложено: 0 обработано, прогон остаётся queued (цикл не стартовал → нет паники).
    let n = scheduler::run_due(db.writer(), &reg, now, true, &HashMap::new())
        .await
        .unwrap();
    assert_eq!(n, 0, "под интерактивом agent_run не исполняется");
    assert_eq!(
        run_store::get_run(db.reader(), run_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        run_store::STATUS_QUEUED,
        "прогон остаётся queued (отложен)"
    );

    // Контроль: busy=false → джоба исполняется (PanicProvider паникует → run_due изолирует панику
    // в fail; джоба учтена, n==1). Доказывает, что дефер выше был именно от busy-гейта.
    // `defer` отодвинул run_at на now+TICK_SECS, поэтому берём `now` ещё дальше (готова снова).
    let later = now + 1000;
    let n2 = scheduler::run_due(db.writer(), &reg, later, false, &HashMap::new())
        .await
        .unwrap();
    assert_eq!(
        n2, 1,
        "под !busy та же джоба готова и заклеймлена (дефер был от busy)"
    );
}

/// Backpressure контр-проба: при `busy=false` тот же agent_run исполняется до `done` (с
/// работающим FakeToolProvider). Отделена от panic-теста (там провайдер падает намеренно).
#[tokio::test]
async fn run_due_runs_agent_run_when_not_busy() {
    use std::collections::HashMap;
    let (_d, db) = open().await;
    let provider = Arc::new(FakeToolProvider::scripted(vec![Ok(ToolTurn::Final(
        "готово".into(),
    ))]));
    let ai = ai_with_tools(Some(provider));
    let h: Arc<dyn JobHandler> = Arc::new(handler(&db, ai));
    let mut reg = scheduler::Registry::new();
    reg.insert(KIND_AGENT_RUN.to_string(), h);

    let run_id = enqueue_agent_run(db.writer(), "t", None, None)
        .await
        .unwrap();
    let now = scheduler::now_secs() + 100; // ЗА run_at джобы → готова.
    let n = scheduler::run_due(db.writer(), &reg, now, false, &HashMap::new())
        .await
        .unwrap();
    assert_eq!(n, 1, "без интерактива agent_run исполняется");
    let r = run_store::get_run(db.reader(), run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.status, STATUS_DONE);
    assert_eq!(r.outcome.as_deref(), Some("готово"));
}

/// Crash-recovery интеграционно: вставляем 'running' прогон со СТАРЫМ updated_at + его джобу →
/// requeue_stale_running флипает прогон в 'queued' → run_due исполняет джобу → прогон 'done'.
/// Доказывает связку run-level recovery (requeue_stale_running) + job-level dispatch.
#[tokio::test]
async fn crash_recovery_requeues_stale_run_then_worker_completes_it() {
    use std::collections::HashMap;
    let (_d, db) = open().await;
    let provider = Arc::new(FakeToolProvider::scripted(vec![Ok(ToolTurn::Final(
        "восстановлено".into(),
    ))]));
    let ai = ai_with_tools(Some(provider));
    let h: Arc<dyn JobHandler> = Arc::new(handler(&db, ai));
    let mut reg = scheduler::Registry::new();
    reg.insert(KIND_AGENT_RUN.to_string(), h);

    // Прогон + джоба, как при энкью; затем имитируем краш ВО ВРЕМЯ прогона: ставим прогон в
    // running со старым updated_at (как будто воркер упал, не успев финишировать).
    let run_id = enqueue_agent_run(db.writer(), "t", None, None)
        .await
        .unwrap();
    run_store::mark_running(db.writer(), run_id).await.unwrap();
    db.writer()
        .call(move |c| {
            c.execute("UPDATE agent_runs SET updated_at=100 WHERE id=?1", [run_id])
                .map(|_| ())
        })
        .await
        .unwrap();

    // Recovery: now=10_000, TTL=600 → cutoff=9400 → stale running (100) восстановлен в queued.
    let recovered = run_store::requeue_stale_running(db.writer(), 600, 10_000)
        .await
        .unwrap();
    assert_eq!(recovered, 1, "застрявший прогон восстановлен");
    assert_eq!(
        run_store::get_run(db.reader(), run_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        run_store::STATUS_QUEUED
    );

    // Воркер прогоняет джобу → прогон доходит до done. `now` ЗА run_at джобы (enqueue ставит
    // run_at=now_secs()) — иначе claim не подберёт готовую джобу.
    let now = scheduler::now_secs() + 100;
    let n = scheduler::run_due(db.writer(), &reg, now, false, &HashMap::new())
        .await
        .unwrap();
    assert_eq!(n, 1, "джоба восстановленного прогона исполнена");
    let r = run_store::get_run(db.reader(), run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.status, STATUS_DONE);
    assert_eq!(r.outcome.as_deref(), Some("восстановлено"));
}

// ── AGENT-3e: go-live актуатора через гейт (safe-by-default) ────────────────────────────────

/// Vault + БД с КАНОНИЗИРОВАННЫМ корнем (предусловие гейта/apply). Возвращаем dir, чтобы жил.
async fn open_vault() -> (TempDir, std::path::PathBuf, Database) {
    let dir = TempDir::new().unwrap();
    let canon_root = dir.path().canonicalize().unwrap();
    let db = Database::open(canon_root.join(".nexus/nexus.db"))
        .await
        .unwrap();
    (dir, canon_root, db)
}

/// Один вызов инструмента-актуатора, затем Final. `name`/`args` — имя гейтнутого инструмента и его JSON.
fn actuator_call_then_final(name: &str, args: &str) -> Vec<AiResult<ToolTurn>> {
    vec![
        Ok(ToolTurn::ToolCalls(vec![ToolCall {
            id: "a1".into(),
            name: name.into(),
            arguments: args.into(),
        }])),
        Ok(ToolTurn::Final("готово".into())),
    ]
}

/// Число executed-строк ledger (`agent_actions`) этого прогона — доказательство apply-через-гейт.
async fn executed_count(db: &Database, run_id: i64) -> i64 {
    db.reader()
        .query(move |c| {
            c.query_row(
                "SELECT count(*) FROM agent_actions WHERE run_id=?1 AND state='executed'",
                [run_id],
                |r| r.get(0),
            )
        })
        .await
        .unwrap()
}

/// **Флаг ВЫКЛ (дефолт) → реестр записи ПУСТ (B7); vault НЕ затронут.** Прогон с провайдером,
/// зовущим `note.create`, НЕ создаёт файл: инструмент не зарегистрирован → UnknownTool-ошибка,
/// диск не тронут. Прогон всё равно доходит до терминала (модель видит ошибку, финализирует).
#[tokio::test]
async fn flag_off_no_write_tools_vault_untouched() {
    let (_d, root, db) = open_vault().await;
    let provider = Arc::new(FakeToolProvider::scripted(actuator_call_then_final(
        "note.create",
        r#"{"path":"Notes/N.md","content":"hi"}"#,
    )));
    let ai = ai_with_tools(Some(provider));
    // handler() строит хендлер с actuator_enabled=false (дефолт).
    let h = handler(&db, ai);

    let run_id = run_store::create_run(db.writer(), "создай заметку", Some("fake"), Some("auto"))
        .await
        .unwrap();
    h.handle(&job_for(run_id)).await.expect("джоба ok");

    assert!(
        !root.join("Notes/N.md").exists(),
        "флаг ВЫКЛ → актуатор не зарегистрирован → файл НЕ создан"
    );
    // Ни одной executed-строки актуатора (без инструментов записи ledger пуст).
    assert_eq!(executed_count(&db, run_id).await, 0, "ledger пуст");
    let r = run_store::get_run(db.reader(), run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.status, STATUS_DONE, "прогон дошёл до терминала");
}

/// **auto-прогон + флаг ВКЛ + Auto-тир → ПРИМЕНЯЕТСЯ ЧЕРЕЗ ГЕЙТ.** Файл записан, ledger executed.
/// PolicyDefault передан, но НЕ спрошен (Auto-тир в auto-прогоне применяется напрямую под кэпом).
#[tokio::test]
async fn auto_run_flag_on_auto_tier_applies_via_gate() {
    let (_d, root, db) = open_vault().await;
    let provider = Arc::new(FakeToolProvider::scripted(actuator_call_then_final(
        "note.create",
        r#"{"path":"Notes/N.md","content":"тело"}"#,
    )));
    let ai = ai_with_tools(Some(provider));
    let src: Arc<dyn crate::actuator::DecisionSource> = Arc::new(crate::actuator::PolicyDefault);
    let h = handler_with_actuator(&db, ai, root.clone(), 64 * 1024, 16, src);

    let run_id = run_store::create_run(db.writer(), "создай", Some("fake"), Some("auto"))
        .await
        .unwrap();
    h.handle(&job_for(run_id)).await.expect("джоба ok");

    assert_eq!(
        std::fs::read_to_string(root.join("Notes/N.md")).unwrap(),
        "тело",
        "Auto-тир в auto-прогоне применён через гейт"
    );
    assert_eq!(
        executed_count(&db, run_id).await,
        1,
        "ровно одна executed apply-строка"
    );
}

/// **confirm-прогон + флаг ВКЛ + Auto-тир под PolicyDefault → ПРЕДЛОЖЕНО → auto-DENY → НЕ записано.**
/// Доказывает hard-gate #1 на уровне ПРОВОДКИ: даже Auto-тир в confirm-прогоне идёт через гейт и
/// без явного Approve (PolicyDefault) файл не пишется.
#[tokio::test]
async fn confirm_run_flag_on_auto_tier_proposed_not_written() {
    let (_d, root, db) = open_vault().await;
    let provider = Arc::new(FakeToolProvider::scripted(actuator_call_then_final(
        "note.create",
        r#"{"path":"Notes/N.md","content":"hi"}"#,
    )));
    let ai = ai_with_tools(Some(provider));
    let src: Arc<dyn crate::actuator::DecisionSource> = Arc::new(crate::actuator::PolicyDefault);
    let h = handler_with_actuator(&db, ai, root.clone(), 64 * 1024, 16, src);

    let run_id = run_store::create_run(db.writer(), "создай", Some("fake"), Some("confirm"))
        .await
        .unwrap();
    h.handle(&job_for(run_id)).await.expect("джоба ok");

    assert!(
        !root.join("Notes/N.md").exists(),
        "confirm-прогон под PolicyDefault: предложено и auto-DENY-отклонено → файл НЕ записан"
    );
    assert_eq!(executed_count(&db, run_id).await, 0, "ничего не applied");
}

/// **auto-прогон + флаг ВКЛ + Confirm-тир (крупная правка > порога) под PolicyDefault → ПРЕДЛОЖЕНО →
/// auto-DENY → НЕ записано.** auto НЕ перекрывает Confirm-тир (keystone): крупная перезапись всегда
/// предлагается. Порог мал (16 байт) — правка легко перешагивает.
#[tokio::test]
async fn auto_run_flag_on_confirm_tier_proposed_not_written() {
    let (_d, root, db) = open_vault().await;
    // Существующий файл, чтобы note.edit был валиден.
    std::fs::write(root.join("E.md"), "orig").unwrap();
    let big = "x".repeat(64); // > порога 16
    let args = format!(r#"{{"path":"E.md","content":"{big}"}}"#);
    let provider = Arc::new(FakeToolProvider::scripted(actuator_call_then_final(
        "note.edit",
        &args,
    )));
    let ai = ai_with_tools(Some(provider));
    let src: Arc<dyn crate::actuator::DecisionSource> = Arc::new(crate::actuator::PolicyDefault);
    let h = handler_with_actuator(&db, ai, root.clone(), 16, 16, src);

    let run_id = run_store::create_run(db.writer(), "правка", Some("fake"), Some("auto"))
        .await
        .unwrap();
    h.handle(&job_for(run_id)).await.expect("джоба ok");

    assert_eq!(
        std::fs::read_to_string(root.join("E.md")).unwrap(),
        "orig",
        "Confirm-тир в auto-прогоне предложен, не применён (auto не override Confirm)"
    );
    assert_eq!(executed_count(&db, run_id).await, 0, "ничего не applied");
}

/// **Replay-safety: применённое действие при повторном прогоне НЕ дублируется (AlreadyDone).**
/// Прогон 1 применяет note.create (файл + ledger executed). Затем «откатываем» строку прогона в
/// running (имитируем requeue после краша) и гоняем ТОТ ЖЕ провайдер снова — actuator-ledger по
/// idempotency_key детектит уже-исполненное → apply возвращает AlreadyDone, файл НЕ переписан, и
/// НЕ появляется второй executed (idempotency_key тот же → record_before отбит UNIQUE).
#[tokio::test]
async fn replay_already_done_no_double_apply() {
    let (_d, root, db) = open_vault().await;
    // Тот же набор ходов оба раза (replay перезапускает цикл С НАЧАЛА).
    let make_provider = || {
        Arc::new(FakeToolProvider::scripted(actuator_call_then_final(
            "note.create",
            r#"{"path":"Notes/R.md","content":"once"}"#,
        )))
    };
    let src: Arc<dyn crate::actuator::DecisionSource> = Arc::new(crate::actuator::PolicyDefault);

    let run_id = run_store::create_run(db.writer(), "создай", Some("fake"), Some("auto"))
        .await
        .unwrap();

    // Прогон 1 — применяет.
    let h1 = handler_with_actuator(
        &db,
        ai_with_tools(Some(make_provider())),
        root.clone(),
        64 * 1024,
        16,
        src.clone(),
    );
    h1.handle(&job_for(run_id)).await.expect("прогон 1 ok");
    assert_eq!(
        std::fs::read_to_string(root.join("Notes/R.md")).unwrap(),
        "once"
    );
    assert_eq!(executed_count(&db, run_id).await, 1, "applied один раз");
    let mtime1 = std::fs::metadata(root.join("Notes/R.md"))
        .unwrap()
        .modified()
        .unwrap();

    // Имитируем requeue после краша: строку прогона возвращаем в running (drive() иначе сделал бы
    // run-level no-op на терминальном прогоне; нам нужен ПОВТОРНЫЙ заход в цикл, чтобы проверить
    // именно ACTUATOR-ledger AlreadyDone, а не run-level гард).
    db.writer()
        .call(move |c| {
            c.execute(
                "UPDATE agent_runs SET status='running', outcome=NULL WHERE id=?1",
                [run_id],
            )
            .map(|_| ())
        })
        .await
        .unwrap();

    // Прогон 2 (replay) — то же действие. Actuator-ledger: AlreadyDone → файл НЕ переписан, второй
    // executed НЕ появляется (idempotency_key тот же).
    let h2 = handler_with_actuator(
        &db,
        ai_with_tools(Some(make_provider())),
        root.clone(),
        64 * 1024,
        16,
        src.clone(),
    );
    h2.handle(&job_for(run_id))
        .await
        .expect("прогон 2 (replay) ok");

    assert_eq!(
        std::fs::read_to_string(root.join("Notes/R.md")).unwrap(),
        "once",
        "replay НЕ переписал файл (AlreadyDone)"
    );
    assert_eq!(
        executed_count(&db, run_id).await,
        1,
        "ровно ОДНА executed-строка — нет двойного apply"
    );
    let mtime2 = std::fs::metadata(root.join("Notes/R.md"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(mtime1, mtime2, "файл физически не переписан при replay");
}

// ── AGENT-5: KILL-SWITCH (чек-пойнты #1/#2 на уровне хендлера + возобновление) ─────────────────

/// Провайдер, ПАНИКующий при вызове — доказывает «цикл не стартовал» (зеркало backpressure-теста).
struct PanicIfCalled;
#[async_trait]
impl ToolCapableProvider for PanicIfCalled {
    async fn stream_chat_tools(
        &self,
        _m: &[ChatMessage],
        _t: &[ToolSpec],
        _o: &mut (dyn FnMut(String) + Send),
        _c: &Arc<AtomicBool>,
        _ctx: RunCtx,
    ) -> AiResult<ToolTurn> {
        panic!("под паузой цикл агента не должен стартовать");
    }
    fn model_id(&self) -> &str {
        "panic-if-called"
    }
}

/// **KILL-SWITCH чек-пойнт #1 (пауза ДО старта)**: kill-switch взведён → handle НЕ запускает цикл
/// (PanicIfCalled не паникует — значит не вызван), прогон ОСТАЁТСЯ `queued`, и пере-кьюена свежая
/// джоба (для возобновления на un-pause). Провайдер не тронут → ни одной записи.
#[tokio::test]
async fn paused_before_start_keeps_run_queued() {
    let (_d, db) = open().await;
    let paused = Arc::new(AtomicBool::new(true)); // ПАУЗА взведена
    let ai = ai_with_tools(Some(Arc::new(PanicIfCalled)));
    let h = handler_paused(&db, ai, paused.clone());

    let run_id = enqueue_agent_run(db.writer(), "задача", Some("fake"), Some("auto"))
        .await
        .unwrap();
    // handle не должен паниковать (провайдер не вызван) и не должен трогать строку прогона.
    h.handle(&job_for(run_id))
        .await
        .expect("джоба ok под паузой");

    let r = run_store::get_run(db.reader(), run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        r.status,
        run_store::STATUS_QUEUED,
        "под паузой прогон ОСТАЁТСЯ queued (цикл не стартовал)"
    );
    // Пере-кьюена свежая джоба прогона (для возобновления на un-pause): есть pending KIND_AGENT_RUN.
    let pending = pending_agent_run_jobs(&db, run_id).await;
    assert!(
        pending >= 1,
        "под паузой пере-кьюена джоба для возобновления: pending={pending}"
    );
}

/// **KILL-SWITCH возобновление (un-pause → прогон идёт)**: пауза ДО старта оставила прогон queued;
/// снимаем паузу → тот же хендлер доводит прогон до `done` (с рабочим провайдером). Доказывает,
/// что прогон НЕ потерян под паузой — он возобновляется.
#[tokio::test]
async fn unpause_resumes_queued_run() {
    let (_d, db) = open().await;
    let paused = Arc::new(AtomicBool::new(true));
    // Провайдер, который под паузой НЕ зовётся, а после un-pause доводит до Final.
    let provider = Arc::new(FakeToolProvider::scripted(vec![Ok(ToolTurn::Final(
        "возобновлён".into(),
    ))]));
    let ai = ai_with_tools(Some(provider));
    let h = handler_paused(&db, ai, paused.clone());

    let run_id = enqueue_agent_run(db.writer(), "задача", Some("fake"), Some("auto"))
        .await
        .unwrap();
    // Под паузой: остаётся queued.
    h.handle(&job_for(run_id)).await.expect("под паузой ok");
    assert_eq!(
        run_store::get_run(db.reader(), run_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        run_store::STATUS_QUEUED
    );

    // Снимаем паузу → тот же хендлер на повторном handle доводит прогон до терминала.
    paused.store(false, Ordering::Relaxed);
    h.handle(&job_for(run_id)).await.expect("после un-pause ok");
    let r = run_store::get_run(db.reader(), run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        r.status, STATUS_DONE,
        "после un-pause прогон возобновлён → done"
    );
    assert_eq!(r.outcome.as_deref(), Some("возобновлён"));
}

/// **KILL-SWITCH чек-пойнт #2/#3 интегрально (актуатор ВКЛ): пауза мид-ран → НИ ОДНОЙ записи.**
/// Провайдер скриптует note.create (Auto-тир, autonomy=auto) — БЕЗ паузы это записало бы файл
/// (см. `auto_run_flag_on_auto_tier_applies_via_gate`). Со ВЗВЕДЁННОЙ паузой: цикл-чек #2 и
/// актуатор-чек #3 fail-safe → файл НЕ создан, ledger пуст, прогон ре-кьюен в queued (не done).
#[tokio::test]
async fn paused_actuator_run_writes_nothing_and_requeues() {
    let (_d, root, db) = open_vault().await;
    let paused = Arc::new(AtomicBool::new(true)); // ПАУЗА
    let provider = Arc::new(FakeToolProvider::scripted(actuator_call_then_final(
        "note.create",
        r#"{"path":"Notes/N.md","content":"тело"}"#,
    )));
    let ai = ai_with_tools(Some(provider));
    let src: Arc<dyn crate::actuator::DecisionSource> = Arc::new(crate::actuator::PolicyDefault);
    let h = handler_with_actuator_paused(&db, ai, root.clone(), 64 * 1024, 16, src, paused);

    let run_id = run_store::create_run(db.writer(), "создай", Some("fake"), Some("auto"))
        .await
        .unwrap();
    h.handle(&job_for(run_id))
        .await
        .expect("джоба ok под паузой");

    assert!(
        !root.join("Notes/N.md").exists(),
        "под паузой актуатор НЕ записал файл (чек-пойнты #2/#3)"
    );
    assert_eq!(
        executed_count(&db, run_id).await,
        0,
        "под паузой нет executed apply-строк"
    );
    // Прогон ре-кьюен в queued (пауза мид-ран — не терминал), не done/error.
    let r = run_store::get_run(db.reader(), run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        r.status,
        run_store::STATUS_QUEUED,
        "пауза мид-ран → прогон возвращён в queued (возобновится на un-pause)"
    );
}

/// Число pending джоб KIND_AGENT_RUN с payload=run_id (для проверки пере-кью под паузой).
async fn pending_agent_run_jobs(db: &Database, run_id: i64) -> i64 {
    let payload = run_id.to_string();
    db.reader()
        .query(move |c| {
            c.query_row(
                "SELECT count(*) FROM jobs WHERE kind=?1 AND payload=?2 AND state='pending'",
                rusqlite::params![KIND_AGENT_RUN, payload],
                |r| r.get(0),
            )
        })
        .await
        .unwrap()
}

// ── тест-хелперы корреляции ───────────────────────────────────────────────────────────────

/// run_id всех durable-строк egress_audit в порядке вставки.
async fn durable_run_ids(db: &Database) -> Vec<Option<i64>> {
    db.reader()
        .query(|c| {
            let mut stmt = c.prepare("SELECT run_id FROM egress_audit ORDER BY id")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, Option<i64>>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
        .unwrap()
}

/// (host, run_id) всех durable-строк egress_audit — для проверки per-run атрибуции без кросс-тегирования.
async fn durable_host_run_ids(db: &Database) -> Vec<(String, Option<i64>)> {
    db.reader()
        .query(|c| {
            let mut stmt = c.prepare("SELECT host, run_id FROM egress_audit ORDER BY id")?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
        .unwrap()
}
