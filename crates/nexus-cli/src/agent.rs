//! `nexus agent` — ТЕРМИНАЛЬНЫЙ агент (W-28, срез 1): запуск агента из терминала, отдельно от
//! десктоп-GUI, для разработки и тестирования. One-shot: даём задачу — видим поток
//! (токены/вызовы инструментов/результаты/финал) в stdout.
//!
//! Это ТРЕТИЙ потребитель транспорт-агностичного ядра [`run_agent_session`] рядом с desktop
//! (`drive_run`) и agentd — со своими реализациями вывода ([`StdoutForwarder`]). Сборка зависимостей
//! зеркалит `nexus-agentd --sandbox-run` (egress-политика + audit + общий
//! [`build_agent_tool_provider`]), но БЕЗ песочницы и БЕЗ актуатора.
//!
//! **SAFE BY DEFAULT (срез 1):** `actuator_enabled=false` → агент работает на СТАБАХ, vault не
//! трогается, гейт подтверждения не дёргается ([`PolicyDefault`] всё равно fail-closed). Единственный
//! побочный эффект — строка в `agent_runs` (как у любого прогона). Живой актуатор + TTY-аппрув —
//! срез 2 (W-29). REPL — срез 3 (W-30).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use nexus_core::agent::{AgentEvent, AgentEventForwarder};

/// Подкоманда `nexus agent [--vault P] "<задача>"`. Задача = позиционные аргументы, склеенные
/// пробелом (флаги `--vault`/`--help` отфильтрованы). Vault по умолчанию — текущий каталог.
pub(crate) fn cmd_agent(args: &[&str]) -> Result<(), String> {
    if args.iter().any(|a| matches!(*a, "--help" | "-h")) {
        print_agent_help();
        return Ok(());
    }
    let task = parse_task(args)?;
    let vault = crate::resolve_vault(args)?;

    let rt = tokio::runtime::Runtime::new().map_err(|e| format!("tokio: {e}"))?;
    rt.block_on(run_agent(vault, task))
}

/// Извлекает задачу из аргументов: пропускает `--vault <val>`, отвергает прочие `--флаги` (зарезерв.
/// под срезы 2+: `--actuator`/`--auto`/`--yes`), остальное склеивает пробелом. Пусто → ошибка.
/// Выделено отдельной функцией для юнит-тестов разбора.
fn parse_task(args: &[&str]) -> Result<String, String> {
    let mut parts: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "--vault" => i += 2, // флаг + значение (обрабатывает resolve_vault)
            t if t.starts_with("--") => {
                return Err(format!(
                    "неизвестный флаг {t} (в срезе 1 поддержан только --vault; \
                     --actuator/--auto/--yes придут в W-29)"
                ))
            }
            t => {
                parts.push(t);
                i += 1;
            }
        }
    }
    let task = parts.join(" ");
    if task.trim().is_empty() {
        return Err("нужна задача: nexus agent [--vault P] \"что сделать\"".into());
    }
    Ok(task)
}

/// Гонит один прогон агента и финализирует его в `run_store` (зеркало desktop `drive_run` +
/// `finish_in_store`, минус Channel/UI-DecisionSource — здесь stdout + PolicyDefault).
async fn run_agent(root: PathBuf, task: String) -> Result<(), String> {
    use nexus_core::actuator::{DecisionSource, PolicyDefault, OVERWRITE_THRESHOLD};
    use nexus_core::agent::{run_agent_session, run_store, BudgetKind, LoopOutcome, SessionSpec};
    use nexus_core::ai::tools::build_agent_tool_provider;
    use nexus_core::ai::AiConfig;
    use nexus_core::db::Database;
    use nexus_core::net::{EgressAudit, EgressPolicy};

    let db = Database::open(root.join(".nexus").join("nexus.db"))
        .await
        .map_err(|e| format!("открытие БД {}: {e}", root.display()))?;
    let cfg = load_local_config(&root)?;

    // Egress-граница (как `--sandbox-run`): политика + audit + allowlist из конфига.
    let egress_policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
    let egress_audit = Arc::new(EgressAudit::default());
    egress_audit.set_writer(db.writer().clone());
    egress_policy.set_allowlist(cfg.egress_hosts());

    // Общий tool-провайдер (ai/tools.rs) — тот же, что зовёт desktop. None → нет ai.chat.
    let provider = build_agent_tool_provider(&cfg, &egress_policy, &egress_audit).ok_or(
        "нет ai.chat в .nexus/local.json (url/model) — агенту нечем думать; задай эндпоинт LLM",
    )?;
    let chat = cfg.ai.chat.as_ref(); // build_* уже проверил, что Some
    let model = chat
        .and_then(|c| c.model.clone())
        .unwrap_or_else(|| "chat".into());
    let context_window = chat.and_then(|c| c.context_window);

    // Строка `agent_runs` (ledger-корреляция). NB: создаём ТОЛЬКО строку, БЕЗ джобы `KIND_AGENT_RUN`
    // — поэтому при прерывании CLI (Ctrl-C до finish_run) осиротевшая `queued`-строка демоном НЕ
    // подхватится: agentd-воркер клеймит ДЖОБЫ (jobs.KIND_AGENT_RUN payload=run_id), а не сканирует
    // `agent_runs.status` (job.rs:431). Журнал append-only, реапера в one-shot нет — безвредно.
    let run_id = run_store::create_run(db.writer(), &task, Some(&model), Some("confirm"))
        .await
        .map_err(|e| format!("create_run: {e}"))?;

    eprintln!(
        "nexus agent · vault={} · model={model} · run_id={run_id} · actuator=OFF (stub)\n\
         ── задача ──\n{task}\n",
        root.display()
    );

    let spec = SessionSpec {
        run_id,
        task,
        history: Vec::new(),
        autonomy: Some("confirm".into()),
        actuator_enabled: false, // срез 1: стабы, vault не трогается
        overwrite_threshold: OVERWRITE_THRESHOLD,
        blast_cap: AiConfig::DEFAULT_BLAST_RADIUS_CAP,
        context_window,
        canon_root: root.clone(),
        skills_learning_enabled: false,
    };
    let paused = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    let forwarder: Arc<dyn AgentEventForwarder> = Arc::new(StdoutForwarder::new());
    // Стабы не предлагают changeset → PolicyDefault (fail-closed) ни разу не спрашивается; TTY-аппрув
    // появится в W-29 вместе с живым актуатором.
    let decision: Arc<dyn DecisionSource> = Arc::new(PolicyDefault);

    let outcome = run_agent_session(
        &spec,
        provider.as_ref(),
        None, // memory — recall пуст (срез 1; VaultAgentMemory придёт позже)
        None, // skills
        None, // web
        decision,
        db.writer(),
        db.reader(),
        &paused,
        &cancel,
        forwarder,
        None, // subagent
        None, // delegation
        None, // research
    )
    .await;

    // Финализация в run_store (зеркало `finish_in_store`).
    let (status, text) = match &outcome {
        LoopOutcome::Final(s) => (run_store::STATUS_DONE, s.clone()),
        LoopOutcome::BudgetExhausted {
            kind: BudgetKind::Cancelled,
            partial,
        } => (
            run_store::STATUS_CANCELLED,
            format!("отменён; частичный ответ: {partial}"),
        ),
        LoopOutcome::BudgetExhausted { kind, partial } => (
            run_store::STATUS_ERROR,
            format!("бюджет исчерпан ({kind:?}); частичный ответ: {partial}"),
        ),
        LoopOutcome::Error(e) => (run_store::STATUS_ERROR, e.clone()),
    };
    let _ = run_store::finish_run(db.writer(), run_id, status, Some(&text)).await;
    println!(); // завершающий перевод строки
    if status == run_store::STATUS_DONE {
        Ok(())
    } else {
        Err(format!("прогон завершился: {status} — {text}"))
    }
}

/// Читает/парсит `.nexus/local.json` (зеркало desktop/agentd `load_local_config`, но СИНХРОННО —
/// один разовый read на старте CLI, без tokio-fs-фичи). Нет файла / битый JSON → внятная ошибка
/// (агенту нужен ai.chat, иначе нечем думать).
fn load_local_config(root: &Path) -> Result<nexus_core::ai::LocalConfig, String> {
    let path = root.join(".nexus").join("local.json");
    let raw = std::fs::read_to_string(&path).map_err(|_| {
        format!(
            "нет {} — задай LLM-эндпоинт (онбординг приложения или вручную ai.chat.url/model)",
            path.display()
        )
    })?;
    nexus_core::ai::LocalConfig::parse(&raw)
        .map_err(|e| format!("{}: битый JSON ({e})", path.display()))
}

// ── Рендер потока событий в терминал ────────────────────────────────────────────────────────────

/// Форвардер событий прогона в stdout. `AssistantToken` печатается ИНЛАЙН (стрим), структурные
/// события — отдельными строками; `mid_line` отслеживает, нужен ли перевод строки перед структурной
/// строкой (чтобы не липла к незавершённой строке токенов).
struct StdoutForwarder {
    mid_line: Mutex<bool>,
}

impl StdoutForwarder {
    fn new() -> Self {
        Self {
            mid_line: Mutex::new(false),
        }
    }
}

impl AgentEventForwarder for StdoutForwarder {
    fn forward(&self, ev: &AgentEvent) {
        use std::io::Write;
        let mut mid = self.mid_line.lock().unwrap_or_else(|e| e.into_inner());
        if let AgentEvent::AssistantToken(s) = ev {
            print!("{s}");
            let _ = std::io::stdout().flush();
            *mid = !s.is_empty() && !s.ends_with('\n');
            return;
        }
        if let Some(line) = render_line(ev) {
            if *mid {
                println!();
                *mid = false;
            }
            println!("{line}");
        }
    }
}

/// Усечение до `n` символов (по char-границам, не байтам) с многоточием.
fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n).collect::<String>())
    } else {
        s.to_string()
    }
}

/// Чистый рендер ОДНОГО структурного события в строку терминала (всё, кроме `AssistantToken`,
/// который печатается инлайн). `None` → событие не отображаем. Выделено для юнит-тестов.
/// `_` catch-all обязателен: `AgentEvent` помечен `#[non_exhaustive]`.
fn render_line(ev: &AgentEvent) -> Option<String> {
    match ev {
        AgentEvent::AssistantToken(_) => None,
        AgentEvent::ToolCall { kind, args, .. } => Some(format!("→ {kind} {}", clip(args, 120))),
        AgentEvent::ToolResult {
            is_error, content, ..
        } => Some(format!(
            "  {} {}",
            if *is_error { "✗" } else { "✓" },
            clip(content, 200)
        )),
        AgentEvent::ContextUsage { used, window } => Some(format!("  [контекст {used}/{window}]")),
        AgentEvent::Proposal { files, .. } => {
            Some(format!("≋ предложение записи: {} файл(ов)", files.len()))
        }
        AgentEvent::Diff {
            path,
            add,
            del,
            status,
        } => Some(format!("  ± {path} (+{add}/-{del}) [{status:?}]")),
        AgentEvent::Final(s) => Some(format!("\n─── ответ ───\n{s}")),
        AgentEvent::Error(s) => Some(format!("✗ ошибка: {s}")),
        AgentEvent::ExecProposal { summary, .. } => {
            Some(format!("≋ exec-предложение: {}", clip(summary, 120)))
        }
        AgentEvent::ExecResult {
            exit_code,
            finalized,
            ..
        } => Some(format!("  exec → код {exit_code} (finalized={finalized})")),
        AgentEvent::PlanProposed { steps, .. } => Some(format!("⌗ план: {} шаг(ов)", steps.len())),
        AgentEvent::PlanStepStatus { id, status } => Some(format!("  шаг {id}: {status:?}")),
        AgentEvent::SubagentStatus { goal, status, .. } => {
            Some(format!("⌗ субагент [{status:?}]: {}", clip(goal, 80)))
        }
        AgentEvent::Report {
            title,
            path,
            sources_count,
            rounds,
            ..
        } => Some(format!(
            "▣ отчёт «{}» → {path} ({sources_count} источн., {rounds} раунд.)",
            clip(title, 80)
        )),
        _ => None,
    }
}

fn print_agent_help() {
    eprintln!(
        "nexus agent — запуск агента в терминале (срез 1: one-shot, без записи)\n\n\
         ИСПОЛЬЗОВАНИЕ:\n  \
         nexus agent [--vault PATH] \"<задача>\"\n\n\
         ФЛАГИ:\n  \
         --vault PATH   корень vault (по умолчанию — текущий каталог)\n  \
         -h, --help     эта справка\n\n\
         ПРИМЕР:\n  \
         nexus agent --vault ~/SA-Vault \"перечисли мои заметки про Rust\"\n\n\
         Срез 1: actuator ВЫКЛЮЧЕН (стабы) — vault не изменяется. Нужен .nexus/local.json с ai.chat \
         (url/model). Живые правки с подтверждением — в следующем срезе (--actuator)."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_task_joins_positionals() {
        assert_eq!(parse_task(&["сделай", "X"]).unwrap(), "сделай X");
    }

    #[test]
    fn parse_task_skips_vault_and_value() {
        assert_eq!(
            parse_task(&["--vault", "/tmp/v", "найди", "заметки"]).unwrap(),
            "найди заметки"
        );
    }

    #[test]
    fn parse_task_empty_is_error() {
        assert!(parse_task(&[]).is_err());
        assert!(parse_task(&["--vault", "/tmp/v"]).is_err());
    }

    #[test]
    fn parse_task_rejects_unknown_flag() {
        let e = parse_task(&["--actuator", "do"]).unwrap_err();
        assert!(e.contains("неизвестный флаг"), "got: {e}");
    }

    #[test]
    fn render_token_is_inline_none() {
        assert!(render_line(&AgentEvent::AssistantToken("hi".into())).is_none());
    }

    #[test]
    fn render_tool_call_and_results() {
        let call = render_line(&AgentEvent::ToolCall {
            id: "1".into(),
            kind: "fs.read".into(),
            args: "{\"path\":\"a.md\"}".into(),
        })
        .unwrap();
        assert!(call.starts_with("→ fs.read"), "got: {call}");

        let ok = render_line(&AgentEvent::ToolResult {
            id: "1".into(),
            content: "готово".into(),
            is_error: false,
        })
        .unwrap();
        assert!(ok.contains('✓') && ok.contains("готово"), "got: {ok}");

        let err = render_line(&AgentEvent::ToolResult {
            id: "1".into(),
            content: "облом".into(),
            is_error: true,
        })
        .unwrap();
        assert!(err.contains('✗'), "got: {err}");
    }

    #[test]
    fn render_final_and_error() {
        let f = render_line(&AgentEvent::Final("итог".into())).unwrap();
        assert!(f.contains("ответ") && f.contains("итог"), "got: {f}");
        let e = render_line(&AgentEvent::Error("упал".into())).unwrap();
        assert!(e.contains("ошибка") && e.contains("упал"), "got: {e}");
    }

    #[test]
    fn clip_respects_char_boundaries() {
        // 5 кириллических букв, лимит 3 → 3 буквы + многоточие (не паника на байтах).
        assert_eq!(clip("абвгд", 3), "абв…");
        assert_eq!(clip("абв", 3), "абв");
    }
}
