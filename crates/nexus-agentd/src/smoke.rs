//! Headless smoke-харнесс agentd (`NEXUS_AGENTD_SMOKE=1` в default-профиле ИЛИ cargo-feature `smoke`,
//! которая форсит smoke на компиляции), отделён от wiring `main.rs` (R-11). Гоняет офлайн (без сети/
//! модели) три проверки — actuator-gate apply, цикл агента, долговечный agent_run до терминала — и
//! выходит 0. Движок [`drive_actuator_gate_run`] совместно используется smoke-путём и CI-тестом
//! `live_actuator_gate_applies_via_gate`.
//!
//! B8 (R-11): smoke-функции НЕ паникуют в release — при провале возвращают `Err` с диагностикой,
//! которую `run()`→`main()` логируют и выходят с кодом 1 (вместо `panic!`/`assert!`-падения).

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use nexus_core::ai::tools::ToolCapableProvider;
use nexus_core::ai::{self, AIClient};
use nexus_core::db::Database;

/// Сколько тиков прокрутить в smoke-режиме перед выходом. Тик планировщика — `TICK_SECS` (5 с в ядре),
/// поэтому ждём с запасом, чтобы воркер успел стартовать (crash-recovery) и хотя бы раз тикнуть.
const SMOKE_TICKS_DEADLINE: Duration = Duration::from_secs(8);

/// Headless smoke-прогон (`NEXUS_AGENTD_SMOKE=1` / feature `smoke`): три офлайн-проверки + долговечный
/// agent_run до терминала, затем graceful-останов воркера и выход 0. Потребляет `worker`/`shutdown_tx`
/// (гасит луп дропом sender'а). Целевой vault/`db` уже открыты в `run()`.
pub(crate) async fn run_smoke(
    db: &Database,
    ai_client: &Arc<AIClient>,
    worker: tokio::task::JoinHandle<()>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
) -> Result<(), String> {
    // AGENT-3e smoke: actuator GO-LIVE ЧЕРЕЗ ГЕЙТ (offline, без сети) — доказывает, что включённый
    // флаг + autonomy=auto + Auto-тир note.create реально пишет в vault ИМЕННО через
    // dispatch_action (ledger Executed), а PolicyDefault не препятствует Auto-тиру. Использует
    // СВОЙ временный vault (не трогает целевой root) и фейк-провайдер (без модели/сети).
    actuator_gate_smoke().await?;
    // AGENT-1 smoke: цикл агента крутится end-to-end против СТАБ-провайдера (offline, без сети) и
    // безопасного реестра (echo) — доказывает execute→feed-back→Final без живой модели/актуатора.
    agent_loop_smoke().await?;
    // Smoke: ставим одну health-джобу — пульс воркера. Выход 0.
    nexus_core::scheduler::enqueue(
        db.writer(),
        crate::health::KIND_HEALTH,
        "",
        nexus_core::scheduler::now_secs(),
        3,
    )
    .await
    .map_err(|e| format!("smoke: enqueue health: {e}"))?;

    // AGENT-2 smoke: ставим ДОЛГОВЕЧНЫЙ прогон агента через настоящий путь enqueue_agent_run
    // (строка agent_runs=queued + джоба KIND_AGENT_RUN payload=run_id). Воркер заклеймит и
    // проведёт его AgentRunHandler'ом до ТЕРМИНАЛА. Деградирует чисто: если agent_tools=None
    // (нет ai.chat в конфиге → offline-smoke), прогон финишируется 'error' ("agent tools
    // unavailable") — что всё равно ДОКАЗЫВАЕТ жизненный цикл джобы + RunCtx-проводку. Если
    // провайдер сконфигурирован и сделал эгресс — durable egress_audit-строки несут run_id.
    let run_id = nexus_core::agent::enqueue_agent_run(
        db.writer(),
        "smoke: проверь связку прогона агента",
        ai_client
            .agent_tools
            .as_ref()
            .map(|p| p.model_id())
            .or(Some("none")),
        Some("auto"),
    )
    .await
    .map_err(|e| format!("smoke: enqueue_agent_run: {e}"))?;
    tracing::info!(
        run_id,
        deadline_secs = SMOKE_TICKS_DEADLINE.as_secs(),
        "nexus-agentd: AGENT-2 smoke — прогон поставлен, крутим воркер до терминала"
    );

    // Ждём, пока воркер доведёт прогон до терминала (или дедлайн). Опрашиваем БД.
    let terminal = wait_for_terminal_run(db, run_id, SMOKE_TICKS_DEADLINE).await;

    // Дроп sender'а гасит воркер-луп (changed()→Err→break) — graceful stop, как при закрытии vault.
    drop(shutdown_tx);
    let _ = worker.await;

    match terminal {
        Some(run) => {
            // Корреляция: сколько durable egress-строк несут этот run_id (0 в offline-smoke без модели).
            let correlated = count_egress_for_run(db, run_id).await;
            tracing::info!(
                run_id,
                status = %run.status,
                step = run.step,
                egress_with_run_id = correlated,
                "nexus-agentd: AGENT-2 smoke — прогон достиг терминала (lifecycle + RunCtx-корреляция проверены)"
            );
        }
        None => {
            return Err(format!(
                "smoke: agent_run {run_id} НЕ достиг терминала за {}с (воркер не диспатчит?)",
                SMOKE_TICKS_DEADLINE.as_secs()
            ));
        }
    }
    tracing::info!("nexus-agentd: smoke завершён (vault открыт, воркер тикал) — выход 0");
    Ok(())
}

/// AGENT-2 smoke: опрашивает БД, пока прогон `run_id` не достигнет терминала ('done'/'error'/
/// 'cancelled') ИЛИ не истечёт `deadline`. Возвращает терминальный снимок или `None` (дедлайн).
/// Короткий интервал опроса (тик планировщика 5 с — smoke-прогон мгновенен после клейма).
async fn wait_for_terminal_run(
    db: &Database,
    run_id: i64,
    deadline: Duration,
) -> Option<nexus_core::agent::AgentRun> {
    let start = std::time::Instant::now();
    loop {
        if let Ok(Some(run)) = nexus_core::agent::run_store::get_run(db.reader(), run_id).await {
            if nexus_core::agent::run_store::is_terminal(&run.status) {
                return Some(run);
            }
        }
        if start.elapsed() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// AGENT-2 smoke: число durable egress_audit-строк, скоррелированных на `run_id` (доказательство
/// RunCtx-проводки, когда прогон делал эгресс; 0 в offline-smoke без сконфигурированной модели).
async fn count_egress_for_run(db: &Database, run_id: i64) -> i64 {
    db.reader()
        .query(move |c| {
            c.query_row(
                "SELECT count(*) FROM egress_audit WHERE run_id=?1",
                [run_id],
                |r| r.get(0),
            )
        })
        .await
        .unwrap_or(0)
}

/// AGENT-1 offline smoke: гоняет цикл агента против ФЕЙКОВОГО провайдера (без сети) и безопасного
/// реестра (echo) — ToolCalls на ходу 1, Final на ходу 2. Доказывает, что headless умеет крутить цикл
/// execute→feed-back→Final. Сети не касается (стаб-провайдер). Логирует исход; при провале — `Err` с
/// диагностикой (B8: smoke НЕ паникует в release-бинаре — `run()`→`main` логирует и exit-1).
async fn agent_loop_smoke() -> Result<(), String> {
    use nexus_core::agent::tool::{ToolCall, ToolSpec};
    use nexus_core::agent::{
        run_agent_loop, AgentEvent, EchoTool, LoopBounds, LoopOutcome, ToolRegistry,
    };
    use nexus_core::ai::tools::ToolTurn;
    use nexus_core::ai::{ChatMessage, ContextBudget};
    use nexus_core::chunker::WordTokenizer;
    use nexus_core::net::RunCtx;
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;

    /// Стаб-провайдер: ToolCalls([echo]) → Final («ok»). Без сети.
    struct SmokeProvider {
        turns: Mutex<std::collections::VecDeque<nexus_core::ai::AiResult<ToolTurn>>>,
    }
    #[async_trait::async_trait]
    impl ToolCapableProvider for SmokeProvider {
        async fn stream_chat_tools(
            &self,
            _messages: &[ChatMessage],
            _tools: &[ToolSpec],
            _on_token: &mut (dyn FnMut(String) + Send),
            _cancel: &Arc<AtomicBool>,
            _ctx: RunCtx,
        ) -> nexus_core::ai::AiResult<ToolTurn> {
            self.turns
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(ToolTurn::Final("ok".into())))
        }
        fn model_id(&self) -> &str {
            "smoke"
        }
    }

    let provider = SmokeProvider {
        turns: Mutex::new(
            vec![
                Ok(ToolTurn::ToolCalls(vec![ToolCall {
                    id: "s1".into(),
                    name: "debug.echo".into(),
                    arguments: r#"{"text":"agent-1 smoke"}"#.into(),
                }])),
                Ok(ToolTurn::Final("ok".into())),
            ]
            .into_iter()
            .collect(),
        ),
    };
    let mut registry = ToolRegistry::new();
    registry.insert(Arc::new(EchoTool));
    let tk = WordTokenizer;
    let cancel = Arc::new(AtomicBool::new(false));
    let agent_paused = Arc::new(AtomicBool::new(false));
    let mut tool_results = 0usize;
    let outcome = run_agent_loop(
        &provider,
        &registry,
        vec![ChatMessage::user("smoke: вызови echo")],
        LoopBounds::default(),
        &ContextBudget::from_context_window(Some(32768)),
        &tk,
        &cancel,
        &agent_paused,
        RunCtx::NONE,
        &mut |e| {
            if matches!(e, AgentEvent::ToolResult { .. }) {
                tool_results += 1;
            }
        },
    )
    .await;
    if !(matches!(outcome, LoopOutcome::Final(ref s) if s == "ok") && tool_results == 1) {
        return Err(format!(
            "AGENT-1 smoke: цикл должен исполнить инструмент и финализировать (получено: {outcome:?}, tool_results={tool_results})"
        ));
    }
    tracing::info!(
        "nexus-agentd: AGENT-1 smoke цикла агента пройден (execute→feed-back→Final, offline)"
    );
    Ok(())
}

/// AGENT-3e ФЕЙК-провайдер: ход 1 — ToolCalls([note.create]); ход 2 — Final. Без сети/модели —
/// детерминированно скриптует один `note.create` (Auto-тир), доказывая живой путь tool→dispatch_action
/// →apply offline. Совместно используется headless-smoke ([`actuator_gate_smoke`]) и CI-тестом
/// ([`tests::live_actuator_gate_applies_via_gate`]) — единый источник проводки, без дублирования.
struct CreateThenFinalProvider {
    turns:
        std::sync::Mutex<std::collections::VecDeque<nexus_core::ai::AiResult<ai::tools::ToolTurn>>>,
}

impl CreateThenFinalProvider {
    /// Скрипт «создать `rel` с телом `content`, затем Final». Эмитит ОДИН note.create-tool_call.
    /// `rel`/`content` — простые тестовые значения без JSON-спецсимволов (кавычек/бэкслэшей), поэтому
    /// собираем args прямым `format!` — nexus-agentd намеренно БЕЗ `serde_json` (минимум зависимостей).
    fn note_create(rel: &str, content: &str) -> Self {
        use ai::tools::ToolTurn;
        use nexus_core::agent::tool::ToolCall;
        let args = format!(r#"{{"path":"{rel}","content":"{content}"}}"#);
        Self {
            turns: std::sync::Mutex::new(
                vec![
                    Ok(ToolTurn::ToolCalls(vec![ToolCall {
                        id: "n1".into(),
                        name: "note.create".into(),
                        arguments: args,
                    }])),
                    Ok(ToolTurn::Final("готово".into())),
                ]
                .into_iter()
                .collect(),
            ),
        }
    }
}

#[async_trait::async_trait]
impl ToolCapableProvider for CreateThenFinalProvider {
    async fn stream_chat_tools(
        &self,
        _m: &[ai::ChatMessage],
        _t: &[nexus_core::agent::tool::ToolSpec],
        _o: &mut (dyn FnMut(String) + Send),
        _c: &Arc<AtomicBool>,
        _ctx: nexus_core::net::RunCtx,
    ) -> nexus_core::ai::AiResult<ai::tools::ToolTurn> {
        self.turns
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(ai::tools::ToolTurn::Final("ok".into())))
    }
    fn model_id(&self) -> &str {
        "actuator-gate-fake"
    }
}

/// Результат прогона actuator-гейта в temp-vault: что записано на диск + сколько executed-строк ledger
/// + терминальный статус прогона. Используется и smoke-, и тест-вызывателем для ассертов.
struct ActuatorGateResult {
    written: Option<String>,
    executed: i64,
    status: Option<String>,
}

/// AGENT-3e ЖИВОЙ путь актуатора ЧЕРЕЗ ГЕЙТ (offline, без сети/модели) — ЕДИНЫЙ движок для headless-
/// smoke и CI-теста. Строит ВКЛЮЧЁННЫЙ [`AgentRunHandler`] (`actuator_enabled=true`,
/// `decision_source=PolicyDefault`) над уже-открытым `db` с КАНОНИЗИРОВАННЫМ `canon_root` и фейк-
/// провайдером, скриптующим `note.create` (Auto-тир). autonomy=auto → гейт авто-применяет Auto-тир
/// напрямую: файл записан, ledger executed, classify_hash протянут. Возвращает [`ActuatorGateResult`]
/// (вызыватель ассертит). Реальный vault пользователя НЕ трогаем — caller даёт временный root.
async fn drive_actuator_gate_run(
    canon_root: &Path,
    db: &Database,
    rel: &str,
    content: &str,
) -> Result<ActuatorGateResult, String> {
    use nexus_core::agent::{enqueue_agent_run, run_store, AgentRunHandler, KIND_AGENT_RUN};
    use nexus_core::net::EgressPolicy;
    use nexus_core::scheduler::{Job, JobHandler};

    let provider = Arc::new(CreateThenFinalProvider::note_create(rel, content));
    let ai = Arc::new(AIClient {
        chat: None,
        chat_fast: None,
        chat_util: None,
        embedder: None,
        agent_tools: Some(provider),
        policy: Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false)))),
    });
    let handler = AgentRunHandler::new(
        db.writer().clone(),
        db.reader().clone(),
        ai,
        Some(32768),
        None,
        canon_root.to_path_buf(),
        true, // actuator ВКЛ (go-live флаг)
        nexus_core::actuator::OVERWRITE_THRESHOLD,
        nexus_core::ai::AiConfig::DEFAULT_BLAST_RADIUS_CAP,
        // PolicyDefault: НЕ спрашивается для Auto-тира в auto-прогоне (применяется напрямую под кэпом);
        // подтверждает, что go-live-проводка применяет Auto-тир, а не блокирует его auto-DENY.
        Arc::new(nexus_core::actuator::PolicyDefault),
        // KILL-SWITCH (AGENT-5): smoke/CI-путь — kill-switch НЕ взведён (проверяем go-live apply).
        Arc::new(AtomicBool::new(false)),
        // SKILL-2: actuator-gate smoke не про скиллы → без skills.
        None,
        // EGR-AGENT-2: actuator-gate smoke не про веб → без веб-инструментов.
        None,
        // SL-7d: actuator-gate smoke не про авторство навыков.
        false,
        // SUB-3b-2b: actuator-gate smoke не про делегирование (default-OFF).
        nexus_core::ai::DelegationConfig::default(),
        // RES-5: actuator-gate smoke не про deep-research (default-OFF).
        nexus_core::ai::ResearchConfig::default(),
    );

    let run_id = enqueue_agent_run(
        db.writer(),
        "создай заметку",
        Some("actuator-gate-fake"),
        Some("auto"),
    )
    .await
    .map_err(|e| format!("drive: enqueue_agent_run: {e}"))?;
    let job = Job {
        id: 1,
        kind: KIND_AGENT_RUN.into(),
        payload: run_id.to_string(),
        state: "running".into(),
        run_at: 0,
        attempts: 0,
        max_attempts: 3,
        last_error: None,
    };
    handler
        .handle(&job)
        .await
        .map_err(|e| format!("drive: actuator run: {e}"))?;

    let written = std::fs::read_to_string(canon_root.join(rel)).ok();
    let executed: i64 = db
        .reader()
        .query(move |c| {
            c.query_row(
                "SELECT count(*) FROM agent_actions WHERE run_id=?1 AND state='executed'",
                [run_id],
                |r| r.get(0),
            )
        })
        .await
        .unwrap_or(-1);
    let status = run_store::get_run(db.reader(), run_id)
        .await
        .ok()
        .flatten()
        .map(|r| r.status);
    Ok(ActuatorGateResult {
        written,
        executed,
        status,
    })
}

/// AGENT-3e offline smoke: actuator GO-LIVE ЧЕРЕЗ ГЕЙТ. Открывает СВОЙ временный vault и гоняет
/// [`drive_actuator_gate_run`] (флаг ВКЛ + autonomy=auto + `note.create`). Доказывает живую проводку
/// tool→dispatch_action→apply БЕЗ модели/сети. Провал → `Err` с диагностикой (B8: smoke НЕ паникует в
/// release — валит `run()` честным exit-1, не panic): это акцептанс go-live. Целевой root НЕ трогаем
/// (свой temp vault). CI-эквивалент — [`tests::live_actuator_gate_applies_via_gate`].
async fn actuator_gate_smoke() -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("nexus-actuator-smoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("smoke: temp vault: {e}"))?;
    let canon_root = dir
        .canonicalize()
        .map_err(|e| format!("smoke: canonicalize vault: {e}"))?;
    let db = Database::open(canon_root.join(".nexus").join("nexus.db"))
        .await
        .map_err(|e| format!("smoke: open db: {e}"))?;

    let res = drive_actuator_gate_run(&canon_root, &db, "Notes/Smoke.md", "создано гейтом").await?;
    let _ = std::fs::remove_dir_all(&dir);

    if res.written.as_deref() != Some("создано гейтом") {
        return Err(format!(
            "AGENT-3e smoke: note.create ДОЛЖНА быть записана ЧЕРЕЗ ГЕЙТ (флаг ВКЛ, autonomy=auto); получено: {:?}",
            res.written
        ));
    }
    if res.executed != 1 {
        return Err(format!(
            "AGENT-3e smoke: ожидалась ровно одна executed apply-строка ledger (apply через dispatch_action), получено {}",
            res.executed
        ));
    }
    tracing::info!(
        status = res.status.as_deref().unwrap_or("?"),
        "nexus-agentd: AGENT-3e actuator smoke пройден (tool→dispatch_action→apply, ledger executed, offline)"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// **AGENT-3e Fix-1 (HIGH — CI покрывает ЖИВОЙ write-путь актуатора).** Гоняет тот же движок, что
    /// headless-smoke ([`drive_actuator_gate_run`]), но как `#[tokio::test]` — поэтому
    /// `cargo test -p nexus-agentd` (CI через `--workspace`) теперь упражняет полную проводку
    /// `tool → dispatch_action → apply` НА УРОВНЕ agentd, а не только за рантайм-флагом
    /// `NEXUS_AGENTD_SMOKE=1`. Доказывает: ВКЛЮЧЁННЫЙ флаг актуатора + `autonomy=auto` + Auto-тир
    /// `note.create` → файл реально записан в vault + ровно одна `executed` apply-строка ledger (apply
    /// прошёл ЧЕРЕЗ ГЕЙТ — `dispatch_action`, не в обход) + classify_hash протянут (иначе drift-рубеж
    /// отменил бы запись). Полностью ОФЛАЙН: фейк-провайдер ([`CreateThenFinalProvider`]) скриптует ходы
    /// без модели/сети; vault — `TempDir` (целевой root пользователя не трогаем). Регрессия, ломающая
    /// живой apply-путь, теперь ВАЛИТ CI (раньше прошла бы все гейты — это и был пробел go-live-ревью).
    #[tokio::test]
    async fn live_actuator_gate_applies_via_gate() {
        let dir = TempDir::new().unwrap();
        // canon_root КАНОНИЗИРОВАН — предусловие гейта/apply (на macOS /tmp → /private/tmp).
        let canon_root = dir.path().canonicalize().unwrap();
        let db = Database::open(canon_root.join(".nexus").join("nexus.db"))
            .await
            .unwrap();

        let res = drive_actuator_gate_run(&canon_root, &db, "Notes/Gate.md", "создано гейтом (CI)")
            .await
            .expect("drive_actuator_gate_run (CI): движок не должен падать инфраструктурно");

        assert_eq!(
            res.written.as_deref(),
            Some("создано гейтом (CI)"),
            "флаг ВКЛ + autonomy=auto + Auto-тир: note.create записана ЧЕРЕЗ ГЕЙТ (dispatch_action→apply)"
        );
        assert_eq!(
            res.executed, 1,
            "ровно одна executed apply-строка ledger (apply прошёл через гейт, classify_hash протянут)"
        );
        assert_eq!(
            res.status.as_deref(),
            Some("done"),
            "прогон дошёл до терминала done после применённого действия"
        );
        // Vault внутри TempDir — дроп `dir` чистит за собой; никакого egress (фейк-провайдер).
    }
}
