//! Песочные CLI-входы agentd (`--sandbox-child` / `--sandbox-run` / `--sandbox-undo`), отделены от
//! wiring `main.rs` (R-11). Весь модуль Unix-only (AF_UNIX / rootless-podman — Linux-host фича).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use nexus_core::bootstrap::load_local_config;
use nexus_core::db::Database;
use nexus_core::net::{EgressAudit, EgressPolicy};

/// In-container точка входа песочницы (`--sandbox-child`). Argv (после флага):
/// `<run_id> <base_url> <model> <ctx_window> <task>` (позиционно, как формирует
/// [`nexus_core::sandbox::runner::SandboxChildArgs::to_argv`]). Коннектится к 3 сокетам по ФИКСИРОВАННЫМ
/// путям (`/run/nexus/{egress,act,event}.sock`) и крутит [`run_sandbox_child_session`]. Возвращает код
/// выхода контейнера: 0 — `Final`; 1 — Error/прерывание (host-коннектор решает статус прогона по событиям).
pub(crate) async fn run_sandbox_child() -> Result<i32, String> {
    use nexus_core::agent::connect::connect_unix;
    use nexus_core::agent::runner::LoopOutcome;
    use nexus_core::sandbox::child::{run_sandbox_child_session, SandboxChildSpec};
    use nexus_core::sandbox::{CONTAINER_RUN_DIR, SOCKET_ACT, SOCKET_EGRESS, SOCKET_EVENT};

    let args: Vec<String> = std::env::args().skip(2).collect();
    let [run_id, base_url, model, ctx_window, task, shell_enable] =
        <[String; 6]>::try_from(args).map_err(|a| {
            format!(
                "--sandbox-child: ожидалось 6 аргументов <run_id> <base_url> <model> <ctx_window> <task> <shell_enable>, получено {}",
                a.len()
            )
        })?;
    let run_id: i64 = run_id
        .parse()
        .map_err(|e| format!("run_id не i64 ({run_id:?}): {e}"))?;
    let context_window: usize = ctx_window
        .parse()
        .map_err(|e| format!("ctx_window не usize ({ctx_window:?}): {e}"))?;
    // shell_enable: строгий парс bool (fail-closed — любое не-"true"/"false" → ошибка, не молчаливый OFF).
    let shell_enable: bool = shell_enable
        .parse()
        .map_err(|e| format!("shell_enable не bool ({shell_enable:?}): {e}"))?;

    let dir = Path::new(CONTAINER_RUN_DIR);
    let egress = connect_unix(dir.join(SOCKET_EGRESS))
        .await
        .map_err(|e| format!("connect egress.sock: {e}"))?;
    let act = connect_unix(dir.join(SOCKET_ACT))
        .await
        .map_err(|e| format!("connect act.sock: {e}"))?;
    let event = connect_unix(dir.join(SOCKET_EVENT))
        .await
        .map_err(|e| format!("connect event.sock: {e}"))?;

    let spec = SandboxChildSpec {
        run_id,
        task,
        base_url,
        model,
        temperature: None,
        context_window: Some(context_window),
        // 6c-2f-3: из 6-го CLI-арга (host рендерит SandboxChildArgs::to_argv из config.shell_enable).
        shell_enable,
    };
    let outcome = run_sandbox_child_session(&spec, egress, act, event).await;
    tracing::info!(?outcome, "--sandbox-child: прогон завершён");
    Ok(match outcome {
        LoopOutcome::Final(_) => 0,
        _ => 1,
    })
}

/// HOST-режим (`--sandbox-run <vault> <task>`): собирает `SandboxRunner` с РЕАЛЬНЫМИ backend'ами
/// (GuardedProxy поверх GuardedClient / HostActServer поверх dispatch_action / event-лог) и гонит ОДНУ
/// задачу в хардненном контейнере. Tier-2 — нужен Podman + образ `nexus-agentd:local`. Default-OFF (по
/// флагу). Это композиционный корень host-стороны песочницы (тот же, что позже подключит коннектор при
/// `ai.sandbox_enabled`); сейчас — one-shot для live-валидации каркаса на .28.
pub(crate) async fn run_sandbox_host() -> Result<i32, String> {
    use nexus_core::actuator::{
        AuditSink, DispatchPolicy, GatedToolCtx, PolicyDefault, TracingEventSink,
        OVERWRITE_THRESHOLD,
    };
    use nexus_core::agent::connect::{RpcMessage, Transport, TransportError};
    use nexus_core::agent::run_store;
    use nexus_core::net::{EgressFeature, GuardedClient};
    use nexus_core::sandbox::act::{DispatchActuatorBackend, HostActServer};
    use nexus_core::sandbox::exec_host::{DispatchExecBackend, HostExecServer};
    use nexus_core::sandbox::proxy::{EgressBudget, GuardedClientBackend, GuardedProxy};
    use nexus_core::sandbox::runner::{SandboxChildArgs, SandboxRunner};
    use nexus_core::sandbox::{ResourceCaps, SandboxConfig, DEFAULT_SANDBOX_IMAGE};

    let vault = std::env::args()
        .nth(2)
        .ok_or("--sandbox-run: нужен <vault>")?;
    let task = std::env::args()
        .nth(3)
        .ok_or("--sandbox-run: нужен <task>")?;
    let root = PathBuf::from(&vault)
        .canonicalize()
        .map_err(|e| format!("vault {vault}: {e}"))?;

    let db = Database::open(root.join(".nexus").join("nexus.db"))
        .await
        .map_err(|e| format!("открытие БД: {e}"))?;

    // Egress-граница (как run()): политика + audit + allowlist из конфига.
    let egress_offline = Arc::new(AtomicBool::new(false));
    let egress_policy = Arc::new(EgressPolicy::new(egress_offline.clone()));
    let egress_audit = Arc::new(EgressAudit::default());
    egress_audit.set_writer(db.writer().clone());
    let cfg = load_local_config(&root)
        .await
        .ok_or("нет .nexus/local.json (нужен ai.chat.url/model)")?;
    egress_policy.set_allowlist(cfg.egress_hosts());
    let chat = cfg.ai.chat.as_ref().ok_or("нет ai.chat в конфиге")?;
    let model = chat.model.clone().unwrap_or_else(|| "chat".into());
    let base_url = chat.url.clone();
    let context_window = chat.context_window.unwrap_or(32768);

    // run_id (ledger-корреляция актуатора).
    let run_id = run_store::create_run(db.writer(), &task, Some(&model), Some("auto"))
        .await
        .map_err(|e| format!("create_run: {e}"))?;

    // egress.sock backend: GuardedProxy поверх настоящего GuardedClient (chokepoint цел).
    let client = GuardedClient::for_chat(
        egress_policy.clone(),
        egress_audit.clone(),
        chat.connect_timeout(),
    )
    .map_err(|e| format!("GuardedClient: {e}"))?;
    let proxy = GuardedProxy::new(
        GuardedClientBackend::new(client),
        run_id,
        vec![EgressFeature::Chat],
        EgressBudget::new(16 * 1024 * 1024, 64),
    );

    // act.sock backend: DispatchActuatorBackend поверх GatedToolCtx (auto-тир, PolicyDefault, tracing).
    let gate = GatedToolCtx::new(
        root.clone(),
        AuditSink::new(db.writer().clone(), db.reader().clone()),
        run_id,
        DispatchPolicy::new(
            Some("auto"),
            OVERWRITE_THRESHOLD,
            nexus_core::ai::AiConfig::DEFAULT_BLAST_RADIUS_CAP,
        )
        // Фаза-3 (6b): exec-флаги. shell_enable из конфига (default false → exec HardBlocked);
        // sandbox_available=true — мы здесь ВНУТРИ host-раннера песочницы на Linux (#[cfg(unix)]).
        // В 6b exec всё равно не исполняется (apply fail-closed); проводка демонстрирует паттерн для 6c.
        // SL-7: skills-флаги ЗДЕСЬ НЕ ставим намеренно (ревью SL-7a+b) — `SkillSave` непредставим на
        // sandbox-wire (Err на host/act+host/exec) и `skill_save`-инструмент НЕ в sandbox-реестре
        // (in-process only). In-process `with_skills_flags` (session.rs DispatchPolicy) — в SL-7d.
        .with_exec_flags(cfg.ai.shell_enable, true),
        Arc::new(PolicyDefault),
        Arc::new(TracingEventSink::new()),
    );
    let act_server = HostActServer::new(DispatchActuatorBackend::new(gate.clone()));
    // host/exec backend (6c-2f-3): ТОЛЬКО при shell_enable (default-OFF → None → serve_host отвечает
    // host/exec method_not_found, fail-closed). Делит ТОТ ЖЕ GatedToolCtx (общий ledger/policy/kill-switch
    // agent_paused через Clone) → exec и vault под единым гейтом. PolicyDefault headless → Confirm=DENY.
    let exec_server: Option<HostExecServer<DispatchExecBackend>> = if cfg.ai.shell_enable {
        Some(HostExecServer::new(DispatchExecBackend::new(gate)))
    } else {
        None
    };

    // event.sock out: лог-транспорт (события агента в tracing; десктопа в one-shot нет).
    struct LogTransport;
    #[async_trait::async_trait]
    impl Transport for LogTransport {
        async fn send(&self, msg: RpcMessage) -> Result<(), TransportError> {
            if let RpcMessage::Notification { method, params } = &msg {
                tracing::info!(%method, %params, "sandbox-run: событие агента");
            }
            Ok(())
        }
        async fn recv(&self) -> Option<RpcMessage> {
            std::future::pending().await // host только шлёт в out; recv не зовётся
        }
    }
    let event_out: Arc<dyn Transport> = Arc::new(LogTransport);

    // Конфиг контейнера: образ + per-run каталог сокетов вне vault (runtime_base = XDG_RUNTIME_DIR|/tmp).
    let runtime_base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let config = SandboxConfig::for_run(
        DEFAULT_SANDBOX_IMAGE,
        format!("r{run_id}"),
        root.clone(),
        Path::new(&runtime_base),
        ResourceCaps::default(),
    )
    .map_err(|e| format!("SandboxConfig: {e}"))?;

    tracing::info!(run_id, %model, %base_url, image = DEFAULT_SANDBOX_IMAGE, "sandbox-run: старт песочного прогона");
    let status = SandboxRunner::new(config)
        .run(
            SandboxChildArgs {
                run_id: run_id.to_string(),
                base_url,
                model,
                context_window,
                task,
                // host рендерит shell_enable в argv контейнера; зеркалит host exec_server-гейт.
                shell_enable: cfg.ai.shell_enable,
            },
            proxy,
            act_server,
            event_out,
            exec_server,
        )
        .await?;
    tracing::info!(?status, "sandbox-run: контейнер завершился");
    Ok(if status.success() { 0 } else { 1 })
}

/// Распарсенные аргументы `--sandbox-undo <vault> <run_id> [--approve]`. Вынесено для Tier-1-теста
/// (разбор без БД/IO). `approve` — оператор ЯВНО согласен на исполнение отката (иначе exec-GitOp откат
/// остаётся `Deferred`: PolicyDefault DENY). Unix-only (как весь sandbox-host-путь).
#[derive(Debug, PartialEq, Eq)]
struct SandboxUndoArgs {
    vault: String,
    run_id: i64,
    approve: bool,
}

/// Чистый разбор argv `--sandbox-undo`. `args` — БЕЗ ведущего флага (т.е. `[vault, run_id, ...]`). Vault и
/// run_id обязательны; `--approve` — опц. флаг в любой позиции. Ошибка → понятное сообщение (БД не трогаем).
fn parse_sandbox_undo_args(args: &[String]) -> Result<SandboxUndoArgs, String> {
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let vault = positional
        .first()
        .ok_or("--sandbox-undo: нужен <vault>")?
        .to_string();
    let run_id = positional
        .get(1)
        .ok_or("--sandbox-undo: нужен <run_id>")?
        .parse::<i64>()
        .map_err(|_| "--sandbox-undo: <run_id> должен быть числом")?;
    let approve = args.iter().any(|a| a == "--approve");
    Ok(SandboxUndoArgs {
        vault,
        run_id,
        approve,
    })
}

/// HOST-режим (`--sandbox-undo <vault> <run_id> [--approve]`, SANDBOX-6c-3d-2): откатывает действия прогона
/// `run_id`. Vault-действия — обычный restore; exec-GitOp — реальный `git reset --hard <pre-op-ref>` в
/// хардненном контейнере через [`SandboxUndoExecDriver`] (ТОЛЬКО при `ai.shell_enable=true` +
/// `ai.git_worktree` + `--approve`). Unix-only. Default-safe: без любого из условий exec-GitOp откат
/// остаётся честным `Deferred` (не `Failed`), с подсказкой какие флаги включить.
pub(crate) async fn run_sandbox_undo() -> Result<i32, String> {
    use nexus_core::actuator::{
        undo_run, ApproveAll, AuditSink, DecisionSource, DispatchPolicy, PolicyDefault,
        TracingEventSink, UndoOpts, OVERWRITE_THRESHOLD,
    };
    use nexus_core::actuator::{EventSink, UndoExecDriver};
    use nexus_core::sandbox::exec_undo::{PodmanGitResetRunner, SandboxUndoExecDriver};
    use nexus_core::sandbox::DEFAULT_SANDBOX_IMAGE;

    let argv: Vec<String> = std::env::args().skip(2).collect();
    let parsed = parse_sandbox_undo_args(&argv)?;
    let root = PathBuf::from(&parsed.vault)
        .canonicalize()
        .map_err(|e| format!("vault {}: {e}", parsed.vault))?;

    let db = Database::open(root.join(".nexus").join("nexus.db"))
        .await
        .map_err(|e| format!("открытие БД: {e}"))?;
    let cfg = load_local_config(&root)
        .await
        .ok_or("нет .nexus/local.json")?;

    let ledger = Arc::new(AuditSink::new(db.writer().clone(), db.reader().clone()));
    // exec-флаги: shell_enable из конфига; sandbox_available=true (мы на Linux-host-раннере). Без --approve —
    // PolicyDefault DENY → exec-GitOp откат Deferred. С --approve — ApproveAll (оператор явно согласен).
    let policy = DispatchPolicy::new(
        Some("auto"),
        OVERWRITE_THRESHOLD,
        nexus_core::ai::AiConfig::DEFAULT_BLAST_RADIUS_CAP,
    )
    .with_exec_flags(cfg.ai.shell_enable, true);
    let decision: Arc<dyn DecisionSource> = if parsed.approve {
        Arc::new(ApproveAll)
    } else {
        Arc::new(PolicyDefault)
    };
    let events: Arc<dyn EventSink> = Arc::new(TracingEventSink::new());
    let worktree = cfg.ai.git_worktree.as_ref().map(PathBuf::from);

    let driver = SandboxUndoExecDriver::new(
        ledger.clone(),
        parsed.run_id,
        root.clone(),
        policy,
        decision,
        events,
        worktree,
        PodmanGitResetRunner::new(DEFAULT_SANDBOX_IMAGE),
    );

    tracing::info!(
        run_id = parsed.run_id,
        approve = parsed.approve,
        worktree = ?cfg.ai.git_worktree,
        "sandbox-undo: старт отката прогона"
    );
    let driver_ref: &dyn UndoExecDriver = &driver;
    let outcome = undo_run(
        parsed.run_id,
        &root,
        &ledger,
        UndoOpts::new().with_driver(driver_ref),
    )
    .await;
    tracing::info!(
        restored = outcome.restored(),
        deferred = outcome.deferred(),
        failed = outcome.failed(),
        "sandbox-undo: откат завершён"
    );
    for a in &outcome.actions {
        tracing::info!(tool = %a.tool_name, target = ?a.target_rel, status = ?a.status, "sandbox-undo: действие");
    }
    // exit 0, если нет настоящих провалов (Deferred — не провал: откат отложен честно).
    Ok(if outcome.failed() == 0 { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 6c-3d-2: `--sandbox-undo` требует `<vault>` и числовой `<run_id>`; кривые аргументы → ошибка ДО БД.
    #[test]
    fn sandbox_undo_requires_vault_and_run_id() {
        assert!(parse_sandbox_undo_args(&[]).is_err(), "нет vault");
        assert!(
            parse_sandbox_undo_args(&["v".into()]).is_err(),
            "нет run_id"
        );
        assert!(
            parse_sandbox_undo_args(&["v".into(), "notnum".into()]).is_err(),
            "run_id не число"
        );
        let ok = parse_sandbox_undo_args(&["/vault".into(), "42".into()]).unwrap();
        assert_eq!(ok.vault, "/vault");
        assert_eq!(ok.run_id, 42);
        assert!(!ok.approve, "без флага approve=false (default-safe)");
    }

    /// 6c-3d-2: `--approve` парсится в любой позиции (позиционные vault/run_id не сбиваются); иначе false.
    #[test]
    fn sandbox_undo_approve_flag_parsed() {
        let a = parse_sandbox_undo_args(&["/v".into(), "1".into(), "--approve".into()]).unwrap();
        assert!(a.approve);
        let b = parse_sandbox_undo_args(&["--approve".into(), "/v".into(), "1".into()]).unwrap();
        assert!(
            b.approve && b.vault == "/v" && b.run_id == 1,
            "флаг в любой позиции"
        );
        let c = parse_sandbox_undo_args(&["/v".into(), "1".into()]).unwrap();
        assert!(!c.approve, "без флага approve=false");
    }
}
