//! `nexus agent` — ТЕРМИНАЛЬНЫЙ агент (W-28, срез 1): запуск агента из терминала, отдельно от
//! десктоп-GUI, для разработки и тестирования. One-shot: даём задачу — видим поток
//! (токены/вызовы инструментов/результаты/финал) в stdout.
//!
//! Это ТРЕТИЙ потребитель транспорт-агностичного ядра [`run_agent_session`] рядом с desktop
//! (`drive_run`) и agentd — со своими реализациями вывода ([`StdoutForwarder`]). Сборка зависимостей
//! зеркалит `nexus-agentd --sandbox-run` (egress-политика + audit + общий
//! [`build_agent_tool_provider`]), но БЕЗ песочницы и БЕЗ актуатора.
//!
//! **SAFE BY DEFAULT (срез 1):** `actuator_enabled=false` → агент работает БЕЗ инструментов записи
//! (пустой реестр, B7), vault не
//! трогается, гейт подтверждения не дёргается ([`PolicyDefault`] всё равно fail-closed). Единственный
//! побочный эффект — строка в `agent_runs` (как у любого прогона). Живой актуатор + TTY-аппрув —
//! срез 2 (W-29). REPL — срез 3 (W-30).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use nexus_core::agent::{AgentEvent, AgentEventForwarder};
use nexus_core::ai::ChatMessage;

/// Опции прогона из флагов командной строки (W-29).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct AgentOpts {
    /// `--actuator` — включить АКТУАТОР (живые правки vault через гейт). По умолчанию OFF (без
    /// инструментов записи).
    actuator: bool,
    /// `--auto` — автономия `auto` (low-risk применяется без спроса; Confirm-тир всё равно спрашивает).
    auto: bool,
    /// `--yes` — неинтерактивно одобрять ВСЕ предложения (`ApproveAll`). Имеет смысл только с `--actuator`.
    yes: bool,
}

/// Подкоманда `nexus agent [флаги] "<задача>"`. Задача = позиционные аргументы, склеенные пробелом
/// (флаги отфильтрованы). Vault по умолчанию — текущий каталог.
pub(crate) fn cmd_agent(args: &[&str]) -> Result<(), String> {
    if args.iter().any(|a| matches!(*a, "--help" | "-h")) {
        print_agent_help();
        return Ok(());
    }
    let opts = AgentOpts {
        actuator: crate::has_flag(args, "--actuator"),
        auto: crate::has_flag(args, "--auto"),
        yes: crate::has_flag(args, "--yes"),
    };
    let task = parse_task(args)?;
    let vault = crate::resolve_vault(args)?;

    let rt = tokio::runtime::Runtime::new().map_err(|e| format!("tokio: {e}"))?;
    // Пустая задача → диалоговый REPL (W-30); иначе — one-shot (W-28/29).
    if task.trim().is_empty() {
        rt.block_on(run_repl(vault, opts))
    } else {
        rt.block_on(run_once(vault, task, opts))
    }
}

/// Известные булевы флаги подкоманды (без значения) — пропускаются при сборке задачи.
const BOOL_FLAGS: &[&str] = &["--actuator", "--auto", "--yes"];

/// Извлекает задачу из аргументов: пропускает `--vault <val>` и булевы флаги [`BOOL_FLAGS`], отвергает
/// прочие `--флаги`, остальное склеивает пробелом. **Пустая задача допустима** → вызывающий уходит в
/// REPL (W-30). Отдельная функция — для юнит-тестов.
fn parse_task(args: &[&str]) -> Result<String, String> {
    let mut parts: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "--vault" => i += 2, // флаг + значение (обрабатывает resolve_vault)
            t if BOOL_FLAGS.contains(&t) => i += 1, // булев флаг — не часть задачи
            t if t.starts_with("--") => {
                return Err(format!(
                    "неизвестный флаг {t} (поддержаны: --vault, --actuator, --auto, --yes)"
                ))
            }
            t => {
                parts.push(t);
                i += 1;
            }
        }
    }
    Ok(parts.join(" "))
}

/// Какой источник решений по changeset'у использовать (чистый выбор — для юнит-теста).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecisionMode {
    /// Актуатор выключен → инструментов записи нет (пустой реестр, B7), предлагать нечему → источник
    /// не спрашивается (fail-closed `PolicyDefault`).
    Off,
    /// Актуатор включён, интерактивно → `TtyDecisionSource` (y/N по каждому айтему).
    TtyConfirm,
    /// Актуатор включён + `--yes` → `ApproveAll` (неинтерактивно одобрить всё).
    ApproveAllYes,
}

/// Выбор режима решения по флагам. Без `--actuator` — всегда `Off` (vault не трогается), даже если
/// заданы `--yes`/`--auto` (они тогда no-op — предупреждаем отдельно).
fn select_decision_mode(opts: AgentOpts) -> DecisionMode {
    if !opts.actuator {
        DecisionMode::Off
    } else if opts.yes {
        DecisionMode::ApproveAllYes
    } else {
        DecisionMode::TtyConfirm
    }
}

/// Общие зависимости прогона: строятся ОДИН раз, в REPL переиспользуются между ходами. `pub(crate)`:
/// переиспользуется `acp.rs` (ACP-2 сервер) — та же композиция vault/DB/egress/provider, что у `nexus agent`.
pub(crate) struct Deps {
    pub(crate) db: nexus_core::db::Database,
    pub(crate) provider: Arc<dyn nexus_core::ai::tools::ToolCapableProvider>,
    pub(crate) canon_root: PathBuf,
    pub(crate) model: String,
    pub(crate) context_window: Option<usize>,
}

/// Исход одного хода. `run_id` — для `/undo` и истории; `done` — терминал `done` (иначе cancelled/error).
struct TurnOutcome {
    run_id: i64,
    text: String,
    done: bool,
}

/// Собирает зависимости из vault: БД + egress-граница + общий tool-провайдер (зеркало `--sandbox-run`).
/// `None`-провайдер (нет ai.chat) → внятная ошибка. `pub(crate)`: shared с `acp.rs` (ACP-2 сервер).
pub(crate) async fn build_deps(root: PathBuf) -> Result<Deps, String> {
    use nexus_core::ai::tools::build_agent_tool_provider;
    use nexus_core::db::Database;
    use nexus_core::net::{EgressAudit, EgressPolicy};

    let db = Database::open(root.join(".nexus").join("nexus.db"))
        .await
        .map_err(|e| format!("открытие БД {}: {e}", root.display()))?;
    let cfg = load_local_config(&root)?;

    let egress_policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
    let egress_audit = Arc::new(EgressAudit::default());
    egress_audit.set_writer(db.writer().clone());
    egress_policy.set_allowlist(cfg.egress_hosts());

    let provider = build_agent_tool_provider(&cfg, &egress_policy, &egress_audit).ok_or(
        "нет ai.chat в .nexus/local.json (url/model) — агенту нечем думать; задай эндпоинт LLM",
    )?;
    let chat = cfg.ai.chat.as_ref(); // build_* уже проверил, что Some
    let model = chat
        .and_then(|c| c.model.clone())
        .unwrap_or_else(|| "chat".into());
    let context_window = chat.and_then(|c| c.context_window);
    Ok(Deps {
        db,
        provider,
        canon_root: root,
        model,
        context_window,
    })
}

/// Источник решений по текущему состоянию (`actuator` + `--yes`). Все варианты fail-closed (см.
/// [`select_decision_mode`]/[`TtyDecisionSource`]). `auto` тут не важен (он про автономию, не про решение).
fn make_decision(actuator: bool, yes: bool) -> Arc<dyn nexus_core::actuator::DecisionSource> {
    use nexus_core::actuator::{ApproveAll, PolicyDefault};
    let mode = select_decision_mode(AgentOpts {
        actuator,
        yes,
        auto: false,
    });
    match mode {
        DecisionMode::Off => Arc::new(PolicyDefault),
        DecisionMode::TtyConfirm => Arc::new(TtyDecisionSource),
        DecisionMode::ApproveAllYes => Arc::new(ApproveAll),
    }
}

/// Гонит ОДИН ход агента (create_run → сессия → finish_run). Переиспользуется one-shot и REPL.
/// `Err` — только сбой подготовки (create_run); сам исход прогона (cancelled/error) едет в `TurnOutcome`.
async fn run_turn(
    deps: &Deps,
    task: &str,
    history: Vec<ChatMessage>,
    autonomy: &str,
    actuator: bool,
    decision: Arc<dyn nexus_core::actuator::DecisionSource>,
) -> Result<TurnOutcome, String> {
    use nexus_core::actuator::OVERWRITE_THRESHOLD;
    use nexus_core::agent::{run_agent_session, run_store, BudgetKind, LoopOutcome, SessionSpec};
    use nexus_core::ai::AiConfig;

    // NB: создаём строку `agent_runs` БЕЗ джобы `KIND_AGENT_RUN` → осиротевшая `queued`-строка (Ctrl-C
    // до finish_run) демоном НЕ подхватится: воркер клеймит ДЖОБЫ, а не сканирует `agent_runs.status`
    // (job.rs:431). Журнал append-only, реапера в one-shot нет — безвредно.
    let run_id = run_store::create_run(deps.db.writer(), task, Some(&deps.model), Some(autonomy))
        .await
        .map_err(|e| format!("create_run: {e}"))?;

    let spec = SessionSpec {
        run_id,
        task: task.to_string(),
        history,
        autonomy: Some(autonomy.to_string()),
        actuator_enabled: actuator, // --actuator → живые правки через гейт; иначе без инструментов записи
        overwrite_threshold: OVERWRITE_THRESHOLD,
        blast_cap: AiConfig::DEFAULT_BLAST_RADIUS_CAP,
        context_window: deps.context_window,
        canon_root: deps.canon_root.clone(),
        skills_learning_enabled: false,
    };
    let paused = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    let forwarder: Arc<dyn AgentEventForwarder> = Arc::new(StdoutForwarder::new());

    let outcome = run_agent_session(
        &spec,
        deps.provider.as_ref(),
        None, // memory
        None, // skills
        None, // web
        decision,
        deps.db.writer(),
        deps.db.reader(),
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
    let _ = run_store::finish_run(deps.db.writer(), run_id, status, Some(&text)).await;
    Ok(TurnOutcome {
        run_id,
        text,
        done: status == run_store::STATUS_DONE,
    })
}

/// ONE-SHOT: один ход и выход (W-28/29). Код возврата: done→0, иначе→ошибка.
async fn run_once(root: PathBuf, task: String, opts: AgentOpts) -> Result<(), String> {
    // --yes/--auto без --actuator — no-op (предлагать нечего): честно предупреждаем.
    if !opts.actuator && (opts.yes || opts.auto) {
        eprintln!("nexus agent: --yes/--auto без --actuator ничего не меняют (актуатор выключен)");
    }
    let deps = build_deps(root).await?;
    let autonomy = if opts.auto { "auto" } else { "confirm" };
    let (actuator_label, decision_label) = match select_decision_mode(opts) {
        DecisionMode::Off => ("OFF", "—"),
        DecisionMode::TtyConfirm => ("ON", "TTY y/N"),
        DecisionMode::ApproveAllYes => ("ON", "--yes (auto-approve)"),
    };
    eprintln!(
        "nexus agent · vault={} · model={} · actuator={actuator_label} · autonomy={autonomy} · \
         решение={decision_label}\n── задача ──\n{task}\n",
        deps.canon_root.display(),
        deps.model
    );
    let decision = make_decision(opts.actuator, opts.yes);
    let t = run_turn(&deps, &task, Vec::new(), autonomy, opts.actuator, decision).await?;
    println!(); // завершающий перевод строки
    if t.done {
        Ok(())
    } else {
        Err(t.text)
    }
}

/// REPL (W-30): диалоговый режим. Задачи построчно, история между ходами; slash-команды управляют
/// сессией. `Deps` строятся один раз и переиспользуются. EOF (Ctrl-D) / `/quit` → выход.
async fn run_repl(root: PathBuf, opts: AgentOpts) -> Result<(), String> {
    /// Кэп истории переписки (сообщений) — как desktop ограничивает мультитёрн-окно.
    const HISTORY_MAX_MSGS: usize = 16;

    let deps = build_deps(root).await?;
    let mut autonomy_auto = opts.auto;
    let mut actuator = opts.actuator;
    let mut history: Vec<ChatMessage> = Vec::new();
    let mut last_run_id: Option<i64> = None;

    eprintln!(
        "nexus agent REPL · vault={} · model={} · actuator={} · autonomy={}{}\n\
         задачи вводи построчно; /help — команды, /quit — выход.\n",
        deps.canon_root.display(),
        deps.model,
        if actuator { "ON" } else { "OFF" },
        if autonomy_auto { "auto" } else { "confirm" },
        // Честное раскрытие: --yes взведён на всю сессию и сработает, как только actuator=ON.
        if opts.yes {
            " · ⚠ --yes: при actuator=ON ВСЕ правки одобряются АВТОМАТИЧЕСКИ, без запроса [y/N]"
        } else {
            ""
        },
    );

    loop {
        let Some(raw) = read_line("agent> ").await else {
            break; // EOF (Ctrl-D)
        };
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('/') {
            match parse_slash(line) {
                SlashCmd::Quit => break,
                SlashCmd::Help => print_repl_help(),
                SlashCmd::New => {
                    history.clear();
                    eprintln!("· история диалога очищена");
                }
                SlashCmd::ToggleAuto => {
                    autonomy_auto = !autonomy_auto;
                    eprintln!(
                        "· autonomy = {}",
                        if autonomy_auto { "auto" } else { "confirm" }
                    );
                }
                SlashCmd::ToggleActuator => {
                    actuator = !actuator;
                    // Сообщение ЧЕСТНО отражает режим решения: при --yes правки НЕ спрашивают.
                    let note = if !actuator {
                        " — без инструментов записи, vault не трогается"
                    } else if opts.yes {
                        " — АВТО-ОДОБРЕНИЕ всех правок (--yes), БЕЗ запроса [y/N]"
                    } else {
                        " — правки в vault требуют подтверждения [y/N]"
                    };
                    eprintln!(
                        "· actuator = {}{}",
                        if actuator { "ON" } else { "OFF" },
                        note
                    );
                }
                SlashCmd::Undo => match last_run_id {
                    Some(rid) => {
                        let n = undo_last_run(&deps, rid).await;
                        eprintln!("· откат прогона {rid}: восстановлено {n} действий");
                    }
                    None => eprintln!("· нет прогона для отката в этой сессии"),
                },
                SlashCmd::Unknown(c) => eprintln!("· неизвестная команда «{c}» (см. /help)"),
            }
            continue;
        }

        // Обычный ход агента.
        let autonomy = if autonomy_auto { "auto" } else { "confirm" };
        let decision = make_decision(actuator, opts.yes);
        match run_turn(&deps, line, history.clone(), autonomy, actuator, decision).await {
            Ok(t) => {
                println!();
                last_run_id = Some(t.run_id);
                if t.done {
                    // История держится ТОЛЬКО на успешных ходах (мультитёрн-контекст для модели).
                    history.push(ChatMessage::user(line));
                    history.push(ChatMessage::assistant(&t.text));
                    if history.len() > HISTORY_MAX_MSGS {
                        let drop = history.len() - HISTORY_MAX_MSGS;
                        history.drain(0..drop);
                    }
                } else {
                    eprintln!("· ход не завершён: {}", t.text);
                }
            }
            Err(e) => eprintln!("· ошибка хода: {e}"),
        }
    }
    eprintln!("· до встречи");
    Ok(())
}

/// Печатает приглашение в stderr и читает строку из stdin (через `spawn_blocking`). `None` — EOF/ошибка.
async fn read_line(prompt: &str) -> Option<String> {
    use std::io::Write;
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    tokio::task::spawn_blocking(|| {
        let mut s = String::new();
        match std::io::stdin().read_line(&mut s) {
            Ok(0) => None, // EOF
            Ok(_) => Some(s),
            Err(_) => None,
        }
    })
    .await
    .ok()
    .flatten()
}

/// Откатывает применённые действия прогона `run_id` (зеркало desktop `agent_undo`: `actuator::undo_run`
/// над тем же writer/reader). Возвращает число восстановленных действий. Идемпотентно.
async fn undo_last_run(deps: &Deps, run_id: i64) -> usize {
    use nexus_core::actuator::{undo_run, AuditSink};
    let ledger = AuditSink::new(deps.db.writer().clone(), deps.db.reader().clone());
    undo_run(run_id, &deps.canon_root, &ledger).await.restored()
}

/// Slash-команда REPL (разбор — чистая функция, для юнит-теста).
#[derive(Debug, PartialEq, Eq)]
enum SlashCmd {
    Help,
    Quit,
    New,
    ToggleAuto,
    ToggleActuator,
    Undo,
    Unknown(String),
}

fn parse_slash(line: &str) -> SlashCmd {
    match line.trim() {
        "/help" | "/h" | "/?" => SlashCmd::Help,
        "/quit" | "/exit" | "/q" => SlashCmd::Quit,
        "/new" | "/reset" => SlashCmd::New,
        "/auto" => SlashCmd::ToggleAuto,
        "/actuator" => SlashCmd::ToggleActuator,
        "/undo" => SlashCmd::Undo,
        other => SlashCmd::Unknown(other.to_string()),
    }
}

fn print_repl_help() {
    eprintln!(
        "команды REPL:\n  \
         /help            эта справка\n  \
         /auto            переключить автономию confirm↔auto (low-risk без спроса)\n  \
         /actuator        включить/выключить живые правки vault (по умолчанию из флага --actuator)\n  \
         /undo            откатить последний прогон (применённые правки)\n  \
         /new             очистить историю диалога\n  \
         /quit            выход (или Ctrl-D)\n\
         (пауза мид-рана недоступна в построчном REPL — это kill-switch GUI/agentd)"
    );
}

/// Читает/парсит `.nexus/local.json` (зеркало desktop/agentd `load_local_config`, но СИНХРОННО —
/// один разовый read на старте CLI, без tokio-fs-фичи). Нет файла / битый JSON → внятная ошибка
/// (агенту нужен ai.chat, иначе нечем думать).
pub(crate) fn load_local_config(root: &Path) -> Result<nexus_core::ai::LocalConfig, String> {
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

// ── TTY-аппрув (W-29) ───────────────────────────────────────────────────────────────────────────

/// Источник решений по changeset'у через ТЕРМИНАЛ: на каждый предложенный айтем спрашивает `[y/N]`
/// (приглашение в stderr, ответ из stdin). **Fail-closed:** не-«да» / EOF / ошибка чтения → Reject —
/// диск не трогаем (рубеж 2 [`BatchDecision`] тоже отклоняет не-перечисленные). Прямой аналог desktop
/// `UiDecisionSource`, но вход — stdin вместо mpsc. К моменту вызова `StdoutForwarder` уже напечатал
/// сам changeset (Proposal/Diff), поэтому приглашение краткое (путь + тир + ±).
struct TtyDecisionSource;

#[async_trait::async_trait]
impl nexus_core::actuator::DecisionSource for TtyDecisionSource {
    async fn decide(
        &self,
        batch: &nexus_core::actuator::ProposalBatch,
    ) -> nexus_core::actuator::BatchDecision {
        use nexus_core::actuator::ItemDecision;
        let mut pairs: Vec<(i64, ItemDecision)> = Vec::with_capacity(batch.items.len());
        for item in &batch.items {
            let prompt = format!(
                "  применить запись в {} (+{}/-{}, тир {:?})? [y/N] ",
                item.target_rel, item.add, item.del, item.tier
            );
            let approved = prompt_yes_no(&prompt).await;
            pairs.push((
                item.action_id,
                if approved {
                    ItemDecision::Approve
                } else {
                    ItemDecision::Reject
                },
            ));
        }
        nexus_core::actuator::BatchDecision::from_pairs(pairs)
    }
}

/// Печатает приглашение в stderr и читает строку из stdin (через `spawn_blocking`, чтобы не блокировать
/// исполнитель). EOF/ошибка → `false` (fail-closed). Разбор ответа — [`parse_answer`].
async fn prompt_yes_no(prompt: &str) -> bool {
    use std::io::Write;
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let line = tokio::task::spawn_blocking(|| {
        let mut s = String::new();
        std::io::stdin().read_line(&mut s).ok().map(|_| s)
    })
    .await
    .ok()
    .flatten();
    line.map(|s| parse_answer(&s)).unwrap_or(false)
}

/// «Да» только при явном y/yes/да/д (регистронезависимо). Всё прочее (пусто, n, мусор) → false
/// (fail-closed: молчание/опечатка НЕ применяет changeset).
fn parse_answer(s: &str) -> bool {
    matches!(s.trim().to_lowercase().as_str(), "y" | "yes" | "д" | "да")
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
        "nexus agent — запуск агента в терминале (one-shot)\n\n\
         ИСПОЛЬЗОВАНИЕ:\n  \
         nexus agent [ФЛАГИ] \"<задача>\"   one-shot: один ход и выход\n  \
         nexus agent [ФЛАГИ]               REPL: диалог построчно, история между ходами (/help внутри)\n\n\
         ФЛАГИ:\n  \
         --vault PATH   корень vault (по умолчанию — текущий каталог)\n  \
         --actuator     включить живые правки vault (через гейт подтверждения); без него правок нет\n  \
         --auto         автономия `auto` (low-risk применяется без спроса; Confirm-тир всё равно спрашивает)\n  \
         --yes          неинтерактивно одобрять ВСЕ предложения (только с --actuator)\n  \
         -h, --help     эта справка\n\n\
         ПРИМЕРЫ:\n  \
         nexus agent --vault ~/SA-Vault \"перечисли мои заметки про Rust\"\n  \
         nexus agent --vault ~/SA-Vault --actuator \"создай заметку Идеи.md\"   # спросит [y/N] перед записью\n\n\
         Без --actuator vault НЕ изменяется (инструментов записи нет). Нужен .nexus/local.json с ai.chat (url/model).\n  \
         С --actuator каждая запись требует подтверждения [y/N] (fail-closed: не-«да» = отказ)."
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
    fn parse_task_empty_ok_for_repl() {
        // W-30: пустая задача допустима (→ REPL), не ошибка.
        assert_eq!(parse_task(&[]).unwrap(), "");
        assert_eq!(parse_task(&["--vault", "/tmp/v"]).unwrap(), "");
        assert_eq!(parse_task(&["--actuator"]).unwrap(), "");
    }

    #[test]
    fn parse_slash_commands() {
        assert_eq!(parse_slash("/help"), SlashCmd::Help);
        assert_eq!(parse_slash("/h"), SlashCmd::Help);
        assert_eq!(parse_slash("/quit"), SlashCmd::Quit);
        assert_eq!(parse_slash("/q"), SlashCmd::Quit);
        assert_eq!(parse_slash("/exit"), SlashCmd::Quit);
        assert_eq!(parse_slash("/new"), SlashCmd::New);
        assert_eq!(parse_slash("/auto"), SlashCmd::ToggleAuto);
        assert_eq!(parse_slash("/actuator"), SlashCmd::ToggleActuator);
        assert_eq!(parse_slash("  /undo  "), SlashCmd::Undo);
        assert_eq!(parse_slash("/bogus"), SlashCmd::Unknown("/bogus".into()));
    }

    #[test]
    fn parse_task_rejects_unknown_flag() {
        let e = parse_task(&["--bogus", "do"]).unwrap_err();
        assert!(e.contains("неизвестный флаг"), "got: {e}");
    }

    #[test]
    fn parse_task_skips_bool_flags() {
        // W-29: булевы флаги не попадают в текст задачи и не считаются неизвестными.
        assert_eq!(
            parse_task(&["--actuator", "--auto", "--yes", "создай", "X"]).unwrap(),
            "создай X"
        );
        assert_eq!(
            parse_task(&["--vault", "/v", "--actuator", "пиши"]).unwrap(),
            "пиши"
        );
    }

    #[test]
    fn select_decision_mode_by_flags() {
        let off = AgentOpts::default();
        assert_eq!(select_decision_mode(off), DecisionMode::Off);
        // --yes/--auto без актуатора всё равно Off (vault не трогается).
        assert_eq!(
            select_decision_mode(AgentOpts {
                yes: true,
                auto: true,
                ..off
            }),
            DecisionMode::Off
        );
        assert_eq!(
            select_decision_mode(AgentOpts {
                actuator: true,
                ..off
            }),
            DecisionMode::TtyConfirm
        );
        assert_eq!(
            select_decision_mode(AgentOpts {
                actuator: true,
                yes: true,
                ..off
            }),
            DecisionMode::ApproveAllYes
        );
    }

    #[test]
    fn parse_answer_is_fail_closed() {
        for yes in ["y", "Y", "yes", "YES", " да ", "Д"] {
            assert!(parse_answer(yes), "{yes:?} должно быть да");
        }
        for no in ["", "n", "no", "нет", "x", "yep", "yeah"] {
            assert!(!parse_answer(no), "{no:?} должно быть НЕ-да (fail-closed)");
        }
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
