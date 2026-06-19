//! [`AgentRunHandler`] — [`scheduler::JobHandler`] прогона цикла агента (AGENT-2).
//!
//! AGENT-1 крутил `run_agent_loop` ин-процесс (smoke). AGENT-2 делает прогон ДОЛГОВЕЧНОЙ запланированной
//! джобой планировщика: payload джобы несёт `run_id` (id строки `agent_runs`), хендлер по нему ведёт
//! прогон через статус-машину (run_store) и корректирует `EgressAudit::set_run` так, чтобы весь эгресс
//! ВНУТРИ прогона атрибутировался на этот run_id в durable-журнале.
//!
//! # Идемпотентность / replay (контракт)
//! `handle` идемпотентен на УРОВНЕ ПРОГОНА: если строка прогона уже терминальна
//! (`done`/`error`/`cancelled`), хендлер немедленно возвращает `Ok` — НЕ перезапускает цикл. Это
//! защита от двойного исполнения (повторная доставка джобы, requeue после краша воркера и т.п.).
//!
//! **AGENT-2 replay перезапускает цикл С НАЧАЛА** (не возобновляет с шага N). Это безопасно ТОЛЬКО
//! потому, что инструменты AGENT-1 — безопасные стабы БЕЗ побочных эффектов (echo/noop): повторный
//! прогон не дублирует никакого внешнего эффекта. **AGENT-3 (актуатор) ОБЯЗАН** сделать
//! side-effecting инструменты идемпотентными per-op-group (или сверяться с applied-ledger ДО
//! применения), прежде чем полагаться на этот replay — иначе requeue после краша применит изменение
//! дважды. Леджер op-group здесь НЕ строится (scaffold-нота под AGENT-3).
//!
//! # Сброс set_run (RAII-гард)
//! `set_run(Some(run_id))` ставится в начале прогона и ОБЯЗАН сброситься в `None` на ЛЮБОМ пути выхода
//! (успех/ошибка/ранний return/паника), иначе ПОСЛЕДУЮЩИЙ эгресс (другой джобы/фона) ложно
//! атрибутировался бы на завершённый run_id. Гарантируется [`RunScope`] — RAII-гард: его `Drop`
//! зовёт `set_run(None)`. Замечание о гонке: `EgressAudit::set_run` — процессный single-slot (не
//! per-task), поэтому КОНКУРЕНТНЫЕ прогоны перетёрли бы run_id друг друга. В AGENT-2 это не
//! возникает: воркер планировщика исполняет джобы ПОСЛЕДОВАТЕЛЬНО (один claim→handle→complete за
//! раз в `run_due`), а agentd регистрирует один agent_run-хендлер. Параллельные прогоны — будущий
//! срез (потребует per-run audit-контекста вместо процессного слота); зафиксировано как остаточный
//! риск в отчёте.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;

use crate::ai::{AIClient, ChatMessage, ContextBudget, QwenTokenizer};
use crate::db::{ReadPool, WriteActor};
use crate::net::EgressAudit;
use crate::scheduler::{self, Job, JobHandler};

use super::event::AgentEvent;
use super::memory::AgentMemory;
use super::registry::ToolRegistry;
use super::run_store::{self, STATUS_CANCELLED, STATUS_DONE, STATUS_ERROR};
use super::runner::{run_agent_loop, BudgetKind, LoopBounds, LoopOutcome};
use super::stubs::{EchoTool, NoopTool};

/// Kind джобы прогона агента (значение колонки `jobs.kind`).
pub const KIND_AGENT_RUN: &str = "agent_run";

/// Токен-бюджет ПОД ПАМЯТЬ в начальном контексте прогона (AGENT-MEM-1). Скромный кусок окна: память
/// агента — это ФОН (факты/прошлые разговоры/эпизоды), а не основной материал прогона; основное окно
/// оставляем под задачу + tool-результаты цикла. recall не превышает этот бюджет (дропает слои);
/// весь начальный контекст потом всё равно проходит общий `ContextBudget::fit` цикла.
const RECALL_BUDGET_TOKENS: usize = 1500;

/// Системный преамбул цикла агента (AGENT-2): минимальная инструкция. Богаче (skills/политика
/// автономности) — поздние срезы; здесь — каркас, доказывающий проводку прогона.
const AGENT_PREAMBLE: &str =
    "Ты — автономный агент-ассистент Nexus. Реши задачу пользователя, при \
    необходимости вызывая доступные инструменты. Когда задача решена — дай финальный ответ.";

/// RAII-гард корреляции эгресса: ставит `set_run(Some(run_id))` при создании и ГАРАНТИРОВАННО
/// сбрасывает `set_run(None)` в `Drop` — на любом пути выхода (успех/ошибка/ранний return/паника).
/// Так эгресс ПОСЛЕ прогона не атрибутируется на завершённый run_id.
struct RunScope {
    audit: Arc<EgressAudit>,
}

impl RunScope {
    /// Входит в скоуп прогона: с этого момента эгресс ядра аудитится с `run_id`.
    fn enter(audit: Arc<EgressAudit>, run_id: i64) -> Self {
        audit.set_run(Some(run_id));
        Self { audit }
    }
}

impl Drop for RunScope {
    fn drop(&mut self) {
        // Сброс на ЛЮБОМ выходе — иначе последующий эгресс ложно нёс бы завершённый run_id.
        self.audit.set_run(None);
    }
}

/// Реестр стаб-инструментов прогона (AGENT-2): echo + noop. Актуаторные инструменты — AGENT-3.
fn stub_registry() -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    reg.insert(Arc::new(EchoTool));
    reg.insert(Arc::new(NoopTool));
    reg
}

/// Хендлер прогона агента: держит зависимости для прогона цикла как долговечной джобы.
///
/// `defer_under_interactive() = true` — прогон агента уступает интерактивному LLM (S5 backpressure):
/// он НЕ должен забивать модель, пока пользователь активно чатится (см. модульный док backpressure).
pub struct AgentRunHandler {
    writer: WriteActor,
    reader: ReadPool,
    ai: Arc<AIClient>,
    audit: Arc<EgressAudit>,
    /// Контекстное окно модели (токены) — из конфига; `None` → консервативный дефолт ContextBudget.
    context_window: Option<usize>,
    /// Память агента (AGENT-MEM-1): recall в начальный контекст + Add-only запись. `None` →
    /// прогон стартует с «голым» контекстом (поведение AGENT-2, без регрессии). Композиционный
    /// корень (agentd) собирает [`super::VaultAgentMemory`] из ридера/райтера/эмбеддера/индексов.
    memory: Option<Arc<dyn AgentMemory>>,
}

impl AgentRunHandler {
    /// Собирает хендлер из ядровых зависимостей. `context_window` — окно модели агента из конфига
    /// (`ai.chat.context_window`), `None` → дефолт [`ContextBudget::from_context_window`].
    /// `memory` — мост к памяти (`None` → прогон без recall, как AGENT-2: нет регрессии).
    pub fn new(
        writer: WriteActor,
        reader: ReadPool,
        ai: Arc<AIClient>,
        audit: Arc<EgressAudit>,
        context_window: Option<usize>,
        memory: Option<Arc<dyn AgentMemory>>,
    ) -> Self {
        Self {
            writer,
            reader,
            ai,
            audit,
            context_window,
            memory,
        }
    }

    /// Ведёт прогон цикла: статус-машина run_store + корреляция эгресса + run_agent_loop. Возвращает
    /// `Ok(())` всегда, когда ЖИЗНЕННЫЙ ЦИКЛ прогона корректно доведён до терминала (включая исход
    /// `error` цикла — это НЕ сбой джобы, а штатный терминал прогона; джоба → `done`). `Err` —
    /// только инфраструктурный сбой (БД и т.п.), чтобы планировщик ретраил саму джобу.
    async fn drive(&self, run_id: i64) -> Result<(), String> {
        // 1. Идемпотентность: уже терминальный прогон — НЕ перезапускаем (replay-safety).
        let run = run_store::get_run(&self.reader, run_id)
            .await
            .map_err(|e| format!("agent_run {run_id}: чтение прогона: {e}"))?;
        let Some(run) = run else {
            // Нет строки прогона — payload указывает в пустоту. Не ретраим (ретрай не поможет):
            // возвращаем Ok, джоба уходит в done (видимого «вечного dead» не плодим).
            tracing::warn!(run_id, "agent_run: строки прогона нет — пропуск (no-op)");
            return Ok(());
        };
        if run_store::is_terminal(&run.status) {
            tracing::info!(
                run_id,
                status = %run.status,
                "agent_run: прогон уже терминален — идемпотентный no-op (replay-safe)"
            );
            return Ok(());
        }

        // 2. running + корреляция эгресса. RunScope::Drop сбросит set_run(None) на любом выходе ниже.
        run_store::mark_running(&self.writer, run_id)
            .await
            .map_err(|e| format!("agent_run {run_id}: mark_running: {e}"))?;
        let _scope = RunScope::enter(self.audit.clone(), run_id);

        // 3. Провайдер инструментов: нет — финишируем прогон с error (НЕ сбой джобы — деградируем
        //    чисто, доказываем lifecycle + set_run-проводку даже без живой модели).
        let Some(provider) = self.ai.agent_tools.clone() else {
            run_store::finish_run(
                &self.writer,
                run_id,
                STATUS_ERROR,
                Some("agent tools unavailable"),
            )
            .await
            .map_err(|e| format!("agent_run {run_id}: finish(error): {e}"))?;
            tracing::warn!(run_id, "agent_run: agent_tools=None → finish error");
            return Ok(());
        };

        // 4. Входы цикла. Память (AGENT-MEM-1) recall'ится по задаче и встаёт МЕЖДУ системным
        //    преамбулом и задачей: [system преамбул] + [recall-блоки роли user] + [user задача].
        //    recall — только чтение, никогда не ошибка (деградирует в пусто). Нет memory → пусто →
        //    поведение AGENT-2 (голый контекст), без регрессии. recall сам держится в своём бюджете;
        //    весь начальный контекст потом проходит общий ContextBudget::fit внутри цикла.
        let recalled = match &self.memory {
            Some(mem) => mem.recall(&run.task, RECALL_BUDGET_TOKENS).await,
            None => Vec::new(),
        };
        let mut messages = Vec::with_capacity(recalled.len() + 2);
        messages.push(ChatMessage::system(AGENT_PREAMBLE));
        messages.extend(recalled);
        messages.push(ChatMessage::user(&run.task));
        let bounds = LoopBounds::default();
        let budget = ContextBudget::from_context_window(self.context_window);
        let tk = QwenTokenizer::embedded();
        let registry = stub_registry();
        let cancel = Arc::new(AtomicBool::new(false));

        // on_event: считаем результаты инструментов синхронно в общий счётчик (наблюдаемость/replay).
        // Запись шага в БД делаем НЕ из синхронного `on_event` (он не может await), а ПОСЛЕ цикла
        // одним awaited bump_step — это снимает гонку «fire-and-forget bump после finish (терминал-гард
        // отверг бы его)», которая иначе оставила бы step=0. `on_event` мог бы ещё стримить в UI —
        // это UI-1; здесь только счётчик шагов.
        //
        // Счётчик стартует с 0 (НЕ с `run.step`): replay перезапускает цикл С НАЧАЛА (messages
        // пересобраны, инструменты исполняются заново), поэтому `step` означает «результатов
        // инструментов В ЭТОЙ попытке прогона», а не high-water между перезапусками — иначе он бы
        // раздувался при каждом requeue, не отражая ни «шаги этого прогона», ни «всего».
        let steps = Arc::new(std::sync::atomic::AtomicI64::new(0));
        let steps_for_events = steps.clone();
        let mut on_event = move |e: AgentEvent| {
            if matches!(e, AgentEvent::ToolResult { .. }) {
                steps_for_events.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        };

        // 5. Прогон цикла.
        let outcome = run_agent_loop(
            provider.as_ref(),
            &registry,
            messages,
            bounds,
            &budget,
            &tk,
            &cancel,
            &mut on_event,
        )
        .await;

        // 6. Персистим достигнутый шаг ДО финала (bump_step терминал-гард не отвергнет — строка ещё
        //    running). Ошибка бампа не валит прогон (наблюдаемость, не корректность).
        let reached = steps.load(std::sync::atomic::Ordering::Relaxed);
        if reached > 0 {
            if let Err(err) = run_store::bump_step(&self.writer, run_id, reached).await {
                tracing::warn!(run_id, error = %err, "agent_run: bump_step не удался (наблюдаемость)");
            }
        }

        // 7. Терминал прогона по исходу цикла. Отмена (cancel) → STATUS_CANCELLED (отдельный
        //    терминал, не error): таксономия статусов не врёт. Прочее исчерпание бюджета (steps/
        //    wall_clock/tokens) → error (прогон не довёл задачу).
        let (status, outcome_text) = match outcome {
            LoopOutcome::Final(s) => (STATUS_DONE, s),
            LoopOutcome::BudgetExhausted {
                kind: BudgetKind::Cancelled,
                partial,
            } => (
                STATUS_CANCELLED,
                format!("прогон отменён; частичный ответ: {partial}"),
            ),
            LoopOutcome::BudgetExhausted { kind, partial } => (
                STATUS_ERROR,
                format!("бюджет исчерпан ({kind:?}); частичный ответ: {partial}"),
            ),
            LoopOutcome::Error(e) => (STATUS_ERROR, e),
        };
        run_store::finish_run(&self.writer, run_id, status, Some(&outcome_text))
            .await
            .map_err(|e| format!("agent_run {run_id}: finish({status}): {e}"))?;
        tracing::info!(run_id, status, "agent_run: прогон завершён");
        // _scope дропается здесь → set_run(None).
        Ok(())
    }
}

#[async_trait]
impl JobHandler for AgentRunHandler {
    async fn handle(&self, job: &Job) -> Result<(), String> {
        let run_id: i64 = job
            .payload
            .trim()
            .parse()
            .map_err(|e| format!("agent_run: payload не run_id ('{}'): {e}", job.payload))?;
        self.drive(run_id).await
    }

    fn defer_under_interactive(&self) -> bool {
        // S5: прогон агента — тяжёлый LLM-фон, уступает интерактивному чату (не стартует, пока busy).
        true
    }
}

/// Ставит прогон агента в очередь: создаёт строку `agent_runs` (queued) → энкьюит джобу
/// `KIND_AGENT_RUN` с payload = run_id → возвращает run_id (для UI/корреляции). `max_attempts` —
/// небольшое (прогон replay-safe для AGENT-1 стабов; см. контракт replay).
pub async fn enqueue_agent_run(
    writer: &WriteActor,
    task: &str,
    model: Option<&str>,
    autonomy: Option<&str>,
) -> crate::db::DbResult<i64> {
    let run_id = run_store::create_run(writer, task, model, autonomy).await?;
    scheduler::enqueue(
        writer,
        KIND_AGENT_RUN,
        &run_id.to_string(),
        scheduler::now_secs(),
        3,
    )
    .await?;
    Ok(run_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tool::{ToolCall, ToolSpec};
    use crate::ai::tools::{ToolCapableProvider, ToolTurn};
    use crate::ai::AiResult;
    use crate::db::Database;
    use crate::net::{EgressFeature, EgressPolicy, GuardedClient};
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;
    use tempfile::TempDir;

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
        ) -> AiResult<ToolTurn> {
            if let Some((client, url)) = &self.egress {
                // Реальный guarded-эгресс на loopback-мок: durable-строка понесёт текущий run_id.
                let _ = client.get(url, EgressFeature::Chat).await;
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

    fn handler(db: &Database, ai: Arc<AIClient>, audit: Arc<EgressAudit>) -> AgentRunHandler {
        AgentRunHandler::new(
            db.writer().clone(),
            db.reader().clone(),
            ai,
            audit,
            Some(32768),
            None,
        )
    }

    /// Хендлер с подключённой памятью (AGENT-MEM-1): доказывает, что recall попадает в начальный
    /// контекст между system-преамбулом и задачей.
    fn handler_with_memory(
        db: &Database,
        ai: Arc<AIClient>,
        audit: Arc<EgressAudit>,
        memory: Arc<dyn AgentMemory>,
    ) -> AgentRunHandler {
        AgentRunHandler::new(
            db.writer().clone(),
            db.reader().clone(),
            ai,
            audit,
            Some(32768),
            Some(memory),
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
    /// step бампнут (через ToolResult).
    #[tokio::test]
    async fn handle_drives_loop_to_done() {
        let (_d, db) = open().await;
        let audit = Arc::new(EgressAudit::default());
        let provider = Arc::new(FakeToolProvider::scripted(vec![
            Ok(ToolTurn::ToolCalls(vec![echo_call("c1")])),
            Ok(ToolTurn::Final("итог".into())),
        ]));
        let ai = ai_with_tools(Some(provider));
        let h = handler(&db, ai, audit);

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
        let audit = Arc::new(EgressAudit::default());
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
            ) -> AiResult<ToolTurn> {
                panic!("провайдер не должен вызываться для терминального прогона");
            }
            fn model_id(&self) -> &str {
                "panic"
            }
        }
        let ai = ai_with_tools(Some(Arc::new(PanicProvider)));
        let h = handler(&db, ai, audit);

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

    /// Деградация: agent_tools=None → прогон финишируется error чисто (джоба ok — lifecycle доказан),
    /// и set_run сброшен (последующий эгресс — run_id NULL).
    #[tokio::test]
    async fn handle_without_tools_finishes_error_and_resets_set_run() {
        let (_d, db) = open().await;
        let audit = Arc::new(EgressAudit::default());
        let ai = ai_with_tools(None);
        let h = handler(&db, ai, audit.clone());

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

        // set_run сброшен в None после прогона: durable-запись последующего эгресса несёт run_id=NULL.
        audit.set_writer(db.writer().clone());
        let (policy, _) = (
            Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false)))),
            (),
        );
        let (addr, server) = serve_once();
        let client = GuardedClient::new(policy, audit.clone(), |b| b).unwrap();
        client
            .get(&format!("http://{addr}/x"), EgressFeature::Probe)
            .await
            .expect("loopback ok");
        server.join().unwrap();
        let run_ids = durable_run_ids(&db).await;
        assert_eq!(
            run_ids.last(),
            Some(&None),
            "эгресс после прогона: run_id=NULL (set_run сброшен): {run_ids:?}"
        );
    }

    /// run_id-корреляция + сброс: во время прогона guarded-эгресс несёт run_id == id прогона; после
    /// прогона (RunScope::Drop) следующий эгресс несёт run_id=NULL.
    #[tokio::test]
    async fn egress_during_run_is_correlated_then_reset() {
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
        let h = handler(&db, ai, audit.clone());

        let run_id = run_store::create_run(db.writer(), "t", None, None)
            .await
            .unwrap();
        h.handle(&job_for(run_id)).await.expect("джоба ok");
        server.join().unwrap();

        // Эгресс ПОСЛЕ прогона: должен нести run_id=NULL (set_run сброшен Drop'ом RunScope).
        let (addr2, server2) = serve_once();
        client
            .get(&format!("http://{addr2}/after"), EgressFeature::Probe)
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
            "эгресс ПОСЛЕ прогона: run_id=NULL (сброшен): {run_ids:?}"
        );
    }

    /// RunScope::Drop сбрасывает set_run даже при ПАНИКЕ внутри скоупа (no-leak run_id). Читаем
    /// выставленный run_id косвенно: in-memory audit-запись (через guarded denied-эгресс — сети не
    /// касается) несёт текущий run_id из слота. Эгрессы — ДО/ПОСЛЕ catch_unwind (async), а сам скоуп
    /// создаётся/паникует/дропается СИНХРОННО внутри catch_unwind.
    #[tokio::test]
    async fn run_scope_resets_set_run_on_panic() {
        let audit = Arc::new(EgressAudit::default());
        let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
        let client = GuardedClient::new(policy, audit.clone(), |b| b).unwrap();

        // Скоуп создаётся и паникует синхронно; Drop отрабатывает при разворачивании стека.
        let audit_in = audit.clone();
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _scope = RunScope::enter(audit_in, 42);
            panic!("boom");
        }));
        assert!(res.is_err(), "паника проброшена");

        // После панического Drop слот сброшен → denied-эгресс (без сокета) несёт run_id=None.
        let _ = client
            .get("http://blocked.example.com/y", EgressFeature::Probe)
            .await;
        assert_eq!(
            audit.entries().last().and_then(|e| e.run_id),
            None,
            "Drop сбросил set_run даже при панике (последующий эгресс run_id=None)"
        );

        // Контр-проба: внутри живого скоупа эгресс несёт run_id.
        {
            let _scope = RunScope::enter(audit.clone(), 7);
            let _ = client
                .get("http://blocked.example.com/x", EgressFeature::Probe)
                .await;
            assert_eq!(
                audit.entries().last().and_then(|e| e.run_id),
                Some(7),
                "внутри скоупа эгресс несёт run_id"
            );
        }
        // Скоуп вышел нормально → снова None.
        let _ = client
            .get("http://blocked.example.com/z", EgressFeature::Probe)
            .await;
        assert_eq!(audit.entries().last().and_then(|e| e.run_id), None);
    }

    /// AGENT-MEM-1: с подключённой MockAgentMemory recall попадает в НАЧАЛЬНЫЙ контекст прогона —
    /// провайдер на первом ходу видит сообщения `[system преамбул, recall-факт (user), задача
    /// (user)]` именно в этом порядке. Доказывает проводку recall между system и task.
    #[tokio::test]
    async fn handler_injects_recall_between_system_and_task() {
        use crate::agent::memory::MockAgentMemory;

        let (_d, db) = open().await;
        let audit = Arc::new(EgressAudit::default());

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
        let h = handler_with_memory(&db, ai, audit, mem);

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
        let audit = Arc::new(EgressAudit::default());
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
        let h = handler(&db, ai, audit); // memory=None

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
        let audit = Arc::new(EgressAudit::default());
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
            ) -> AiResult<ToolTurn> {
                panic!("под backpressure цикл не должен стартовать");
            }
            fn model_id(&self) -> &str {
                "panic"
            }
        }
        let ai = ai_with_tools(Some(Arc::new(PanicProvider)));
        let h: Arc<dyn JobHandler> = Arc::new(handler(&db, ai, audit));
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
        let audit = Arc::new(EgressAudit::default());
        let provider = Arc::new(FakeToolProvider::scripted(vec![Ok(ToolTurn::Final(
            "готово".into(),
        ))]));
        let ai = ai_with_tools(Some(provider));
        let h: Arc<dyn JobHandler> = Arc::new(handler(&db, ai, audit));
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
        let audit = Arc::new(EgressAudit::default());
        let provider = Arc::new(FakeToolProvider::scripted(vec![Ok(ToolTurn::Final(
            "восстановлено".into(),
        ))]));
        let ai = ai_with_tools(Some(provider));
        let h: Arc<dyn JobHandler> = Arc::new(handler(&db, ai, audit));
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
}
