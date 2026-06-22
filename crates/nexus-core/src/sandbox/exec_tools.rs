//! exec_tools — 3 exec-инструмента агента (SANDBOX-6c-2e-2, спека §5.2): `shell.run` / `process.spawn` /
//! `git.op`. Зеркало note-инструментов (`actuator::tools`), но для exec-таргетов: держат
//! `Arc<dyn ExecDispatcher>` (шов 6c-2e-1) → транспорт-агностичны (Tier-1-мок без podman/RPC).
//!
//! `invoke`: строгий разбор args (I-4 fail-closed, `deny_unknown_fields`) → типизированный exec-[`Action`] →
//! `ExecDispatcher::dispatch_exec` (decide→execute→ИСПОЛНИТЬ→report host-side+in-container) → свёртка
//! [`ExecToolOutcome`] в текст-результат. Decision-исходы (Rejected/HardBlocked) — НЕ ошибка инструмента
//! (агент видит их как обратную связь `Ok`); ошибка — лишь транспорт/протокол/разбор.
//!
//! **Регистрация при `shell_enable`** (в `child.rs`, gated) + проводка `ProxyExecDispatcher` поверх
//! shared act.sock + `serve_host` (host отвечает host/act+host/exec на одном соединении) — 6c-2f. По
//! умолчанию (`shell_enable=false`) эти инструменты СТРУКТУРНО отсутствуют в реестре (агент их не назовёт).

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use super::exec_proxy::{ExecDispatcher, ExecToolOutcome};
use crate::actuator::Action;
use crate::agent::tool::{Tool, ToolError, ToolSpec};

/// Строгий разбор сырых JSON-аргументов (пусто → `{}` → BadArgs о недостающих полях). Зеркало
/// `actuator::tools::parse_args` (I-4 fail-closed).
fn parse_args<T: for<'de> Deserialize<'de>>(args: &str) -> Result<T, ToolError> {
    let raw = if args.trim().is_empty() { "{}" } else { args };
    serde_json::from_str(raw).map_err(|e| ToolError::BadArgs(e.to_string()))
}

/// Свёртка [`ExecToolOutcome`] в текст-результат для модели. `Executed` (любой exit) — РЕЗУЛЬТАТ команды,
/// не ошибка инструмента; Rejected/HardBlocked — обратная связь. Сырой вывод — усечённые хвосты (host
/// уже капнул их `output_cap_bytes`); в ledger он не персистится (приватность, 6c-2d).
fn format_outcome(outcome: ExecToolOutcome) -> String {
    match outcome {
        ExecToolOutcome::Executed {
            exit_code,
            stdout_tail,
            stderr_tail,
            stdout_truncated,
            stderr_truncated,
            timed_out,
        } => {
            let mut s = format!("exit_code: {exit_code}");
            if timed_out {
                s.push_str(" (УБИТО ПО ТАЙМАУТУ)");
            }
            if !stdout_tail.is_empty() {
                s.push_str(&format!(
                    "\n--- stdout{} ---\n{}",
                    if stdout_truncated {
                        " (усечён)"
                    } else {
                        ""
                    },
                    stdout_tail
                ));
            }
            if !stderr_tail.is_empty() {
                s.push_str(&format!(
                    "\n--- stderr{} ---\n{}",
                    if stderr_truncated {
                        " (усечён)"
                    } else {
                        ""
                    },
                    stderr_tail
                ));
            }
            s
        }
        ExecToolOutcome::Rejected(summary) => format!("exec не одобрен: {summary}"),
        ExecToolOutcome::HardBlocked(reason) => format!("exec заблокирован: {reason}"),
    }
}

// ── shell.run ────────────────────────────────────────────────────────────────────────────────────
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShellRunArgs {
    /// argv БЕЗ шелла: argv[0] — программа, argv[1..] — аргументы (НЕ `sh -c`; метасимволы инертны).
    argv: Vec<String>,
    /// Рабочий каталог (vault-rel; конфайнится host/exec_child в scratch). Опционально.
    #[serde(default)]
    cwd_rel: Option<String>,
}

/// `shell.run` — исполнить argv ВНУТРИ песочницы (БЕЗ шелла). Требует одобрения (Confirm) + `shell_enable`.
pub struct ShellRunTool {
    dispatcher: Arc<dyn ExecDispatcher>,
}

impl ShellRunTool {
    pub fn new(dispatcher: Arc<dyn ExecDispatcher>) -> Self {
        Self { dispatcher }
    }
}

#[async_trait]
impl Tool for ShellRunTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "shell.run".into(),
            description: "Исполнить команду argv ВНУТРИ изолированной песочницы (--network=none, vault \
                          только-чтение). БЕЗ шелла: argv[0] — программа, argv[1..] — аргументы. Требует \
                          одобрения; вывод усечён."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "argv": {
                        "type": "array", "items": { "type": "string" },
                        "description": "argv команды (argv[0]=программа); НЕ шелл-строка"
                    },
                    "cwd_rel": { "type": "string", "description": "рабочий каталог (vault-rel), опционально" }
                },
                "required": ["argv"],
                "additionalProperties": false
            }),
        }
    }

    async fn invoke(&self, args: &str) -> Result<String, ToolError> {
        let a: ShellRunArgs = parse_args(args)?;
        if a.argv.is_empty() {
            return Err(ToolError::BadArgs("argv пуст".into()));
        }
        let outcome = self
            .dispatcher
            .dispatch_exec(Action::shell_run(a.argv, a.cwd_rel))
            .await?;
        Ok(format_outcome(outcome))
    }
}

// ── process.spawn ──────────────────────────────────────────────────────────────────────────────
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessSpawnArgs {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    cwd_rel: Option<String>,
}

/// `process.spawn` — запустить процесс `program`+`args` ВНУТРИ песочницы (БЕЗ шелла).
pub struct ProcessSpawnTool {
    dispatcher: Arc<dyn ExecDispatcher>,
}

impl ProcessSpawnTool {
    pub fn new(dispatcher: Arc<dyn ExecDispatcher>) -> Self {
        Self { dispatcher }
    }
}

#[async_trait]
impl Tool for ProcessSpawnTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "process.spawn".into(),
            description: "Запустить процесс (program + args) ВНУТРИ изолированной песочницы (--network=none, \
                          vault только-чтение). БЕЗ шелла. Требует одобрения; вывод усечён."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "program": { "type": "string", "description": "путь/имя программы" },
                    "args": { "type": "array", "items": { "type": "string" }, "description": "аргументы" },
                    "cwd_rel": { "type": "string", "description": "рабочий каталог (vault-rel), опционально" }
                },
                "required": ["program"],
                "additionalProperties": false
            }),
        }
    }

    async fn invoke(&self, args: &str) -> Result<String, ToolError> {
        let a: ProcessSpawnArgs = parse_args(args)?;
        if a.program.trim().is_empty() {
            return Err(ToolError::BadArgs("program пуст".into()));
        }
        let outcome = self
            .dispatcher
            .dispatch_exec(Action::process_spawn(a.program, a.args, a.cwd_rel))
            .await?;
        Ok(format_outcome(outcome))
    }
}

// ── git.op ─────────────────────────────────────────────────────────────────────────────────────
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitOpArgs {
    op: String,
    #[serde(default)]
    args: Vec<String>,
}

/// `git.op` — git-операция `op`+`args` ВНУТРИ песочницы. НИКОГДА не Auto (классификация Confirm/Block).
pub struct GitOpTool {
    dispatcher: Arc<dyn ExecDispatcher>,
}

impl GitOpTool {
    pub fn new(dispatcher: Arc<dyn ExecDispatcher>) -> Self {
        Self { dispatcher }
    }
}

#[async_trait]
impl Tool for GitOpTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "git.op".into(),
            description:
                "Выполнить git-операцию (op + args, напр. op='status') ВНУТРИ изолированной \
                          песочницы. Требует одобрения; вывод усечён."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "op": { "type": "string", "description": "git-подкоманда (напр. status, log, diff)" },
                    "args": { "type": "array", "items": { "type": "string" }, "description": "доп. аргументы" }
                },
                "required": ["op"],
                "additionalProperties": false
            }),
        }
    }

    async fn invoke(&self, args: &str) -> Result<String, ToolError> {
        let a: GitOpArgs = parse_args(args)?;
        if a.op.trim().is_empty() {
            return Err(ToolError::BadArgs("op пуст".into()));
        }
        let outcome = self
            .dispatcher
            .dispatch_exec(Action::git_op(a.op, a.args))
            .await?;
        Ok(format_outcome(outcome))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actuator::ActionTarget;
    use std::sync::Mutex;

    /// Мок-диспетчер: захватывает поданное действие + отдаёт скриптованный исход (без транспорта/раннера).
    struct MockExecDispatcher {
        last: Mutex<Option<Action>>,
        outcome: ExecToolOutcome,
    }
    impl MockExecDispatcher {
        fn new(outcome: ExecToolOutcome) -> Self {
            Self {
                last: Mutex::new(None),
                outcome,
            }
        }
    }
    #[async_trait]
    impl ExecDispatcher for MockExecDispatcher {
        async fn dispatch_exec(&self, action: Action) -> Result<ExecToolOutcome, ToolError> {
            *self.last.lock().unwrap() = Some(action);
            Ok(self.outcome.clone())
        }
    }

    fn executed(exit: i32) -> ExecToolOutcome {
        ExecToolOutcome::Executed {
            exit_code: exit,
            stdout_tail: "out".into(),
            stderr_tail: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            timed_out: false,
        }
    }

    #[tokio::test]
    async fn shell_run_builds_action_and_formats() {
        let d = Arc::new(MockExecDispatcher::new(executed(0)));
        let tool = ShellRunTool::new(d.clone());
        let out = tool
            .invoke(r#"{"argv":["ls","-la"],"cwd_rel":"Notes"}"#)
            .await
            .unwrap();
        assert!(out.contains("exit_code: 0"), "{out}");
        assert!(out.contains("out"), "хвост stdout в результате: {out}");
        let captured = d.last.lock().unwrap().clone().unwrap();
        match captured.target {
            ActionTarget::ShellRun { argv, cwd_rel } => {
                assert_eq!(argv, vec!["ls", "-la"]);
                assert_eq!(cwd_rel.as_deref(), Some("Notes"));
            }
            other => panic!("ожидался ShellRun, {other:?}"),
        }
    }

    #[tokio::test]
    async fn shell_run_rejects_empty_argv() {
        let d = Arc::new(MockExecDispatcher::new(executed(0)));
        let tool = ShellRunTool::new(d);
        assert!(matches!(
            tool.invoke(r#"{"argv":[]}"#).await,
            Err(ToolError::BadArgs(_))
        ));
    }

    #[tokio::test]
    async fn shell_run_bad_args_unknown_field() {
        let d = Arc::new(MockExecDispatcher::new(executed(0)));
        let tool = ShellRunTool::new(d);
        assert!(matches!(
            tool.invoke(r#"{"argv":["ls"],"bogus":1}"#).await,
            Err(ToolError::BadArgs(_))
        ));
    }

    #[tokio::test]
    async fn process_spawn_builds_action() {
        let d = Arc::new(MockExecDispatcher::new(executed(0)));
        let tool = ProcessSpawnTool::new(d.clone());
        tool.invoke(r#"{"program":"rg","args":["foo"]}"#)
            .await
            .unwrap();
        let captured = d.last.lock().unwrap().clone().unwrap();
        match captured.target {
            ActionTarget::ProcessSpawn { program, args, .. } => {
                assert_eq!(program, "rg");
                assert_eq!(args, vec!["foo"]);
            }
            other => panic!("ожидался ProcessSpawn, {other:?}"),
        }
    }

    #[tokio::test]
    async fn process_spawn_rejects_empty_program() {
        let d = Arc::new(MockExecDispatcher::new(executed(0)));
        let tool = ProcessSpawnTool::new(d);
        assert!(matches!(
            tool.invoke(r#"{"program":"  "}"#).await,
            Err(ToolError::BadArgs(_))
        ));
    }

    #[tokio::test]
    async fn git_op_builds_action() {
        let d = Arc::new(MockExecDispatcher::new(executed(0)));
        let tool = GitOpTool::new(d.clone());
        tool.invoke(r#"{"op":"status","args":["--short"]}"#)
            .await
            .unwrap();
        let captured = d.last.lock().unwrap().clone().unwrap();
        match captured.target {
            ActionTarget::GitOp { op, args } => {
                assert_eq!(op, "status");
                assert_eq!(args, vec!["--short"]);
            }
            other => panic!("ожидался GitOp, {other:?}"),
        }
    }

    #[tokio::test]
    async fn formats_nonzero_exit_and_stderr() {
        let d = Arc::new(MockExecDispatcher::new(ExecToolOutcome::Executed {
            exit_code: 1,
            stdout_tail: String::new(),
            stderr_tail: "boom".into(),
            stdout_truncated: false,
            stderr_truncated: true,
            timed_out: false,
        }));
        let tool = ShellRunTool::new(d);
        let out = tool.invoke(r#"{"argv":["false"]}"#).await.unwrap();
        assert!(out.contains("exit_code: 1"), "{out}");
        assert!(out.contains("stderr (усечён)"), "усечение помечено: {out}");
        assert!(out.contains("boom"), "{out}");
    }

    #[tokio::test]
    async fn rejected_and_hardblocked_are_ok_feedback() {
        let rej = Arc::new(MockExecDispatcher::new(ExecToolOutcome::Rejected(
            "нет".into(),
        )));
        let out = ShellRunTool::new(rej)
            .invoke(r#"{"argv":["ls"]}"#)
            .await
            .unwrap();
        assert!(out.contains("не одобрен"), "{out}");
        let blk = Arc::new(MockExecDispatcher::new(ExecToolOutcome::HardBlocked(
            "выкл".into(),
        )));
        let out = ShellRunTool::new(blk)
            .invoke(r#"{"argv":["ls"]}"#)
            .await
            .unwrap();
        assert!(out.contains("заблокирован"), "{out}");
    }

    /// timed_out помечается в результате.
    #[tokio::test]
    async fn formats_timed_out() {
        let d = Arc::new(MockExecDispatcher::new(ExecToolOutcome::Executed {
            exit_code: -1,
            stdout_tail: String::new(),
            stderr_tail: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            timed_out: true,
        }));
        let out = ShellRunTool::new(d)
            .invoke(r#"{"argv":["sleep","99"]}"#)
            .await
            .unwrap();
        assert!(out.contains("ТАЙМАУТ"), "{out}");
    }
}
