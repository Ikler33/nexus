//! exec_proxy — IN-SANDBOX клиент `host/exec` (SANDBOX-6c-2e-1, спека §5.2).
//!
//! Зеркало [`super::act::ProxyActuator`] для exec-таргетов: работает ВНУТРИ `--network=none` контейнера,
//! оркеструет 3-актный host/exec поток поверх act.sock-транспорта + ЛОКАЛЬНО исполняет одобренную команду
//! через [`super::exec_child::ExecRunner`]. **КЛЮЧЕВАЯ ИНВЕРСИЯ §5.2:** host РЕШАЕТ (decide/execute/report —
//! classify/redeem/finalize ledger host-side), КОНТЕЙНЕР ИСПОЛНЯЕТ (`ExecRunner::run` здесь, ВНУТРИ песочницы).
//!
//! Поток `dispatch_exec`:
//!  1. **decide** → `host/exec {phase:decide, action}` → host classify→approval. Rejected/HardBlocked →
//!     возвращаем как [`ExecToolOutcome`] (агент увидит отказ, НЕ ошибку). Approved → одноразовый `exec_token`.
//!  2. **execute** → `host/exec {phase:execute, exec_token}` → host redeem (consume токена + ledger
//!     APPROVED→EXECUTING) → host-authority [`WireExecGo`] (argv из СОХРАНЁННОГО действия — мы НЕ переподаём).
//!  3. **ИСПОЛНЕНИЕ** → `ExecRunner::run(go)` ЛОКАЛЬНО (in-container; ЕДИНСТВЕННОЕ место реального Command —
//!     `exec_child`, host НИКОГДА не спавнит).
//!  4. **report** → `host/exec {phase:report, exec_token, exit, tails}` → host финализирует ledger.
//!
//! 6c-2e-1: шов [`ExecDispatcher`] + `ProxyExecDispatcher` (Tier-1 через channel_pair + mock-host +
//! MockExecRunner). 3 exec-инструмента (`shell.run`/`process.spawn`/`git.op`) поверх `Arc<dyn ExecDispatcher>`
//! + регистрация в `child.rs` при `shell_enable` — 6c-2e-2. Проводка `serve_host` (act+exec на одном
//! соединении) — 6c-2f.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::exec_child::ExecRunner;
use super::exec_host::{
    is_git_sha, WireExecAction, WireExecDecision, WireExecGo, WireExecPhase, WireExecRequest,
    WireExecResult, HOST_EXEC,
};
use crate::actuator::{Action, ActionTarget};
use crate::agent::connect::{RpcMessage, Transport};
use crate::agent::tool::ToolError;

/// Исход прогона exec-таргета через host/exec — для свёртки exec-инструментом (6c-2e-2) в tool-результат.
/// Decision-исходы (Rejected/HardBlocked) — это НЕ ошибки транспорта: агент должен их увидеть как обратную
/// связь («команда не одобрена»), поэтому они `Ok`-варианты, а не `ToolError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecToolOutcome {
    /// Команда исполнена ВНУТРИ песочницы — исход (exit + усечённые хвосты).
    Executed {
        exit_code: i32,
        stdout_tail: String,
        stderr_tail: String,
        stdout_truncated: bool,
        stderr_truncated: bool,
        timed_out: bool,
    },
    /// Отклонено (человек/policy на фазе decide) — резюме.
    Rejected(String),
    /// Жёстко заблокировано (shell_enable=false / песочница недоступна) — фенсенная причина.
    HardBlocked(String),
}

/// Шов exec-диспетча: exec-инструменты (6c-2e-2) держат `Arc<dyn ExecDispatcher>` → Tier-1-тесты мокают
/// его БЕЗ транспорта/раннера; прод — [`ProxyExecDispatcher`] (in-sandbox, host/exec RPC + local run).
#[async_trait]
pub trait ExecDispatcher: Send + Sync {
    /// Полный цикл decide→execute→ИСПОЛНИТЬ→report для одного exec-действия. `Err` — только сбой
    /// транспорта/протокола; decision-исходы — в [`ExecToolOutcome`].
    async fn dispatch_exec(&self, action: Action) -> Result<ExecToolOutcome, ToolError>;
}

/// In-sandbox реализация [`ExecDispatcher`]: оркеструет host/exec поверх `Transport` (act.sock) + локальный
/// [`ExecRunner`]. `scratch_root`/`vault_ro_root` — КОНТЕЙНЕРНЫЕ корни (`/tmp` / `/vault`), резолв-базы cwd.
pub struct ProxyExecDispatcher<T: Transport> {
    transport: T,
    runner: Arc<dyn ExecRunner>,
    scratch_root: PathBuf,
    vault_ro_root: PathBuf,
    next_id: Mutex<i64>,
}

impl<T: Transport> ProxyExecDispatcher<T> {
    pub fn new(
        transport: T,
        runner: Arc<dyn ExecRunner>,
        scratch_root: PathBuf,
        vault_ro_root: PathBuf,
    ) -> Self {
        Self {
            transport,
            runner,
            scratch_root,
            vault_ro_root,
            next_id: Mutex::new(1),
        }
    }

    fn next_id(&self) -> i64 {
        let mut g = self.next_id.lock().expect("id mutex");
        let id = *g;
        *g += 1;
        id
    }

    /// Один host/exec round-trip: сериализует запрос, шлёт, ждёт Response с тем же id, парсит result в `R`.
    /// `RpcError` хоста (invalid_params/internal) → `ToolError::Exec` (фенсенная ошибка инструменту).
    async fn rpc<R: serde::de::DeserializeOwned>(
        &self,
        req: WireExecRequest,
    ) -> Result<R, ToolError> {
        let id = self.next_id();
        let params = serde_json::to_value(req)
            .map_err(|e| ToolError::Exec(format!("host/exec сериализация: {e}")))?;
        self.transport
            .send(RpcMessage::request(id, HOST_EXEC, params))
            .await
            .map_err(|_| ToolError::Exec("host/exec транспорт (send)".into()))?;
        let msg = self
            .transport
            .recv()
            .await
            .ok_or_else(|| ToolError::Exec("host/exec транспорт закрыт".into()))?;
        match msg {
            RpcMessage::Response {
                id: resp_id,
                result,
            } => {
                if resp_id != id {
                    return Err(ToolError::Exec("host/exec: id ответа не совпал".into()));
                }
                match result {
                    Ok(v) => serde_json::from_value(v)
                        .map_err(|e| ToolError::Exec(format!("host/exec ответ: {e}"))),
                    Err(e) => Err(ToolError::Exec(format!("host/exec отказ: {}", e.message))),
                }
            }
            _ => Err(ToolError::Exec("host/exec: ожидался Response".into())),
        }
    }

    /// 6c-2h (§5.5): снять pre-op git-ref (`git rev-parse HEAD`) read-only-инвокацией через ТОТ ЖЕ confined
    /// [`ExecRunner`] (тот же cwd/env/кэпы, что у одобренного `go`) — для обратимости GitOp. **Read-only**:
    /// не мутирует. **Best-effort**: ненулевой код / пустой / не-hex вывод (не git-репо, detached HEAD,
    /// ошибка) → `None` (действие помечается необратимым, НЕ падаем). Контейнер и так может гонять локальные
    /// команды — host лишь ДОВЕРЯЕТ репортнутому ref как best-effort undo-подсказке (лживый ref → будущий
    /// `git reset` 6c-3 просто не сработает, не больше). Вызывается ТОЛЬКО для GitOp (вызыватель проверяет).
    async fn capture_pre_op_gitref(&self, go: &WireExecGo) -> Option<String> {
        let probe = WireExecGo {
            argv: vec!["git".into(), "rev-parse".into(), "HEAD".into()],
            cwd: go.cwd.clone(),
            env: go.env.clone(),
            timeout_ms: go.timeout_ms,
            output_cap_bytes: go.output_cap_bytes,
        };
        let res = self
            .runner
            .run(&probe, &self.scratch_root, &self.vault_ro_root)
            .await;
        let sha = res.stdout_tail.trim();
        // Тот же предикат, что host-side [`is_git_sha`] (единый источник правила). Это best-effort
        // ранний-выход — АВТОРИТЕТНАЯ проверка всё равно на host (он не доверяет контейнеру).
        if res.exit_code == 0 && is_git_sha(sha) {
            Some(sha.to_string())
        } else {
            None // не git-репо / detached / ошибка — необратимо (best-effort, не падаем)
        }
    }
}

#[async_trait]
impl<T: Transport> ExecDispatcher for ProxyExecDispatcher<T> {
    async fn dispatch_exec(&self, action: Action) -> Result<ExecToolOutcome, ToolError> {
        // 1. decide — fail-closed: vault-таргет не представим на host/exec → ToolError.
        let wire_action =
            WireExecAction::try_from(&action).map_err(|e| ToolError::Exec(e.to_string()))?;
        let decision: WireExecDecision = self
            .rpc(WireExecRequest {
                phase: WireExecPhase::Decide,
                action: Some(wire_action),
                exec_token: None,
                exit_code: None,
                stdout_tail: None,
                stderr_tail: None,
                undo_ref: None,
            })
            .await?;
        let exec_token = match decision {
            WireExecDecision::Approved { exec_token, .. } => exec_token,
            WireExecDecision::Rejected { summary } => {
                return Ok(ExecToolOutcome::Rejected(summary))
            }
            WireExecDecision::HardBlocked { reason } => {
                return Ok(ExecToolOutcome::HardBlocked(reason))
            }
        };

        // 2. execute — redeem токена host-side → host-authority WireExecGo (argv НЕ переподаём).
        let go: WireExecGo = self
            .rpc(WireExecRequest {
                phase: WireExecPhase::Execute,
                action: None,
                exec_token: Some(exec_token.clone()),
                exit_code: None,
                stdout_tail: None,
                stderr_tail: None,
                undo_ref: None,
            })
            .await?;

        // 2.5 (6c-2h §5.5): для GitOp снять pre-op git-ref ДО мутации (read-only, ТОТ ЖЕ confined cwd) →
        // undo_ref для обратимости. shell/process НЕОБРАТИМЫ (undo_ref=None; classify их и так не Auto).
        let undo_ref = if matches!(action.target, ActionTarget::GitOp { .. }) {
            self.capture_pre_op_gitref(&go).await
        } else {
            None
        };

        // 3. ИСПОЛНЕНИЕ ЛОКАЛЬНО (in-container): ЕДИНСТВЕННОЕ место реального Command — exec_child.
        let result = self
            .runner
            .run(&go, &self.scratch_root, &self.vault_ro_root)
            .await;

        // 4. report — host финализирует ledger (EXECUTED|FAILED) + персистит undo_ref как UndoCols
        // (host-authority: только если СОХРАНЁННОЕ действие — GitOp; см. exec_host.report).
        let _finalized: WireExecResult = self
            .rpc(WireExecRequest {
                phase: WireExecPhase::Report,
                action: None,
                exec_token: Some(exec_token),
                exit_code: Some(result.exit_code),
                stdout_tail: Some(result.stdout_tail.clone()),
                stderr_tail: Some(result.stderr_tail.clone()),
                undo_ref,
            })
            .await?;

        Ok(ExecToolOutcome::Executed {
            exit_code: result.exit_code,
            stdout_tail: result.stdout_tail,
            stderr_tail: result.stderr_tail,
            stdout_truncated: result.stdout_truncated,
            stderr_truncated: result.stderr_truncated,
            timed_out: result.timed_out,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actuator::Action;
    use crate::agent::connect::{channel_pair, ChannelTransport, RpcError};
    use crate::sandbox::exec_child::{ExecResult, ExecRunner, MockExecRunner};
    use crate::sandbox::exec_host::{ExecCwd, WireExecKind};
    use async_trait::async_trait;

    /// Тест-раннер 6c-2h: пишет argv КАЖДОГО вызова + скриптует rev-parse-исход (sha) ОТДЕЛЬНО от самого
    /// op — чтобы доказать, что pre-op probe реально запущен (`git rev-parse HEAD`) ПЕРЕД мутацией.
    struct RecordingRunner {
        calls: Mutex<Vec<Vec<String>>>,
        revparse_exit: i32,
        revparse_stdout: String,
    }

    #[async_trait]
    impl ExecRunner for RecordingRunner {
        async fn run(
            &self,
            go: &WireExecGo,
            _scratch: &std::path::Path,
            _vault: &std::path::Path,
        ) -> ExecResult {
            self.calls.lock().unwrap().push(go.argv.clone());
            let is_revparse = go.argv == ["git", "rev-parse", "HEAD"];
            ExecResult {
                exit_code: if is_revparse { self.revparse_exit } else { 0 },
                stdout_tail: if is_revparse {
                    self.revparse_stdout.clone()
                } else {
                    "op-output".into()
                },
                stderr_tail: String::new(),
                stdout_truncated: false,
                stderr_truncated: false,
                timed_out: false,
            }
        }
    }

    fn recording(exit: i32, stdout: &str) -> Arc<RecordingRunner> {
        Arc::new(RecordingRunner {
            calls: Mutex::new(Vec::new()),
            revparse_exit: exit,
            revparse_stdout: stdout.into(),
        })
    }

    /// Мок-host: читает host/exec-запросы и отвечает по фазе из скрипта. Останавливается при закрытии
    /// транспорта. `decide` отдаёт заданное решение; execute — заданный WireExecGo; report — finalized.
    /// Захватывает последний report-запрос (для ассертов exit/tails, переданных контейнером).
    async fn mock_host(
        transport: ChannelTransport,
        decision: WireExecDecision,
        go: WireExecGo,
        last_report: Arc<Mutex<Option<WireExecRequest>>>,
    ) {
        while let Some(msg) = transport.recv().await {
            let RpcMessage::Request { id, method, params } = msg else {
                continue;
            };
            assert_eq!(method, HOST_EXEC, "мок-host обслуживает только host/exec");
            let req: WireExecRequest = serde_json::from_value(params).expect("WireExecRequest");
            let result: Result<serde_json::Value, RpcError> = match req.phase {
                WireExecPhase::Decide => Ok(serde_json::to_value(&decision).unwrap()),
                WireExecPhase::Execute => Ok(serde_json::to_value(&go).unwrap()),
                WireExecPhase::Report => {
                    *last_report.lock().unwrap() = Some(req.clone());
                    Ok(serde_json::to_value(WireExecResult {
                        exit_code: req.exit_code.unwrap_or(0),
                        finalized: true,
                    })
                    .unwrap())
                }
            };
            if transport
                .send(RpcMessage::Response { id, result })
                .await
                .is_err()
            {
                break;
            }
        }
    }

    fn go_echo() -> WireExecGo {
        WireExecGo {
            argv: vec!["/bin/echo".into(), "hi".into()],
            cwd: ExecCwd::ScratchTmpfs { rel: String::new() },
            env: vec![],
            timeout_ms: 1000,
            output_cap_bytes: 1024,
        }
    }

    fn exec_result(exit: i32) -> ExecResult {
        ExecResult {
            exit_code: exit,
            stdout_tail: "captured-stdout".into(),
            stderr_tail: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            timed_out: false,
        }
    }

    /// Полный цикл decide(approve)→execute→ЛОКАЛЬНЫЙ run(mock)→report: Executed с исходом раннера;
    /// контейнер на execute предъявил ТОЛЬКО токен; report донёс exit раннера host'у.
    #[tokio::test]
    async fn dispatch_exec_full_cycle_executed() {
        let (host_t, sbx_t) = channel_pair();
        let last_report = Arc::new(Mutex::new(None));
        let host = tokio::spawn(mock_host(
            host_t,
            WireExecDecision::Approved {
                exec_token: "tok-xyz".into(),
                ledger_action_id: 1,
            },
            go_echo(),
            last_report.clone(),
        ));
        // MockExecRunner: исход exit=0; захватит поданный WireExecGo (argv host-authority).
        let runner = Arc::new(MockExecRunner::new(exec_result(0)));
        let proxy = ProxyExecDispatcher::new(
            sbx_t,
            runner.clone(),
            PathBuf::from("/tmp"),
            PathBuf::from("/vault"),
        );
        let outcome = proxy
            .dispatch_exec(Action::shell_run(vec!["echo".into(), "hi".into()], None))
            .await
            .expect("dispatch ok");
        match outcome {
            ExecToolOutcome::Executed {
                exit_code,
                stdout_tail,
                ..
            } => {
                assert_eq!(exit_code, 0);
                assert_eq!(stdout_tail, "captured-stdout");
            }
            other => panic!("ожидался Executed, получено {other:?}"),
        }
        // Раннер получил host-authority go (argv из WireExecGo, не из контейнерного действия).
        let captured = runner.last.lock().unwrap().clone().expect("runner вызван");
        assert_eq!(captured.argv, vec!["/bin/echo", "hi"]);
        // report донёс exit раннера + НЕ нёс action (только токен+исход).
        let rep = last_report.lock().unwrap().clone().expect("report получен");
        assert_eq!(rep.exit_code, Some(0));
        assert_eq!(rep.exec_token.as_deref(), Some("tok-xyz"));
        assert!(rep.action.is_none(), "report не нёс action");
        drop(proxy); // закрыть транспорт → mock_host выйдет
        let _ = host.await;
    }

    /// decide=Rejected → Ok(Rejected), раннер НЕ вызван (нет execute/run).
    #[tokio::test]
    async fn dispatch_exec_rejected_no_run() {
        let (host_t, sbx_t) = channel_pair();
        let last_report = Arc::new(Mutex::new(None));
        let host = tokio::spawn(mock_host(
            host_t,
            WireExecDecision::Rejected {
                summary: "не одобрено".into(),
            },
            go_echo(),
            last_report,
        ));
        let runner = Arc::new(MockExecRunner::new(exec_result(0)));
        let proxy = ProxyExecDispatcher::new(
            sbx_t,
            runner.clone(),
            PathBuf::from("/tmp"),
            PathBuf::from("/vault"),
        );
        let outcome = proxy
            .dispatch_exec(Action::shell_run(vec!["ls".into()], None))
            .await
            .expect("dispatch ok");
        assert!(
            matches!(outcome, ExecToolOutcome::Rejected(_)),
            "outcome={outcome:?}"
        );
        assert!(
            runner.last.lock().unwrap().is_none(),
            "раннер НЕ вызван при Rejected"
        );
        drop(proxy);
        let _ = host.await;
    }

    /// decide=HardBlocked → Ok(HardBlocked), раннер НЕ вызван.
    #[tokio::test]
    async fn dispatch_exec_hardblocked_no_run() {
        let (host_t, sbx_t) = channel_pair();
        let host = tokio::spawn(mock_host(
            host_t,
            WireExecDecision::HardBlocked {
                reason: "shell выключен".into(),
            },
            go_echo(),
            Arc::new(Mutex::new(None)),
        ));
        let runner = Arc::new(MockExecRunner::new(exec_result(0)));
        let proxy = ProxyExecDispatcher::new(
            sbx_t,
            runner.clone(),
            PathBuf::from("/tmp"),
            PathBuf::from("/vault"),
        );
        let outcome = proxy
            .dispatch_exec(Action::shell_run(vec!["ls".into()], None))
            .await
            .expect("dispatch ok");
        assert!(
            matches!(outcome, ExecToolOutcome::HardBlocked(_)),
            "outcome={outcome:?}"
        );
        assert!(
            runner.last.lock().unwrap().is_none(),
            "раннер НЕ вызван при HardBlocked"
        );
        drop(proxy);
        let _ = host.await;
    }

    /// fail-closed: vault-таргет не представим на host/exec → ToolError (НЕ уходит на провод).
    #[tokio::test]
    async fn dispatch_exec_vault_target_fails() {
        let (host_t, sbx_t) = channel_pair();
        let _host = tokio::spawn(mock_host(
            host_t,
            WireExecDecision::Rejected {
                summary: "x".into(),
            },
            go_echo(),
            Arc::new(Mutex::new(None)),
        ));
        let runner = Arc::new(MockExecRunner::new(exec_result(0)));
        let proxy = ProxyExecDispatcher::new(
            sbx_t,
            runner,
            PathBuf::from("/tmp"),
            PathBuf::from("/vault"),
        );
        assert!(
            proxy
                .dispatch_exec(Action::note_create("A.md", "x"))
                .await
                .is_err(),
            "vault-таргет на host/exec → ToolError"
        );
    }

    /// Транспорт закрыт (host недоступен) → ToolError (не паника).
    #[tokio::test]
    async fn dispatch_exec_dead_transport_errors() {
        let (host_t, sbx_t) = channel_pair();
        drop(host_t); // host мёртв
        let runner = Arc::new(MockExecRunner::new(exec_result(0)));
        let proxy = ProxyExecDispatcher::new(
            sbx_t,
            runner,
            PathBuf::from("/tmp"),
            PathBuf::from("/vault"),
        );
        assert!(
            proxy
                .dispatch_exec(Action::shell_run(vec!["ls".into()], None))
                .await
                .is_err(),
            "мёртвый транспорт → ToolError"
        );
    }

    /// 6c-2h: GitOp снимает pre-op git-ref ДО мутации (`git rev-parse HEAD` ПЕРВЫМ), затем сам op; trimmed
    /// sha доносится в report как undo_ref.
    #[tokio::test]
    async fn gitop_captures_pre_op_ref() {
        let (host_t, sbx_t) = channel_pair();
        let last_report = Arc::new(Mutex::new(None));
        let host = tokio::spawn(mock_host(
            host_t,
            WireExecDecision::Approved {
                exec_token: "tok".into(),
                ledger_action_id: 1,
            },
            go_echo(),
            last_report.clone(),
        ));
        let runner = recording(0, "a1b2c3d4e5\n"); // hex sha + trailing newline (как реальный rev-parse)
        let proxy = ProxyExecDispatcher::new(
            sbx_t,
            runner.clone(),
            PathBuf::from("/tmp"),
            PathBuf::from("/vault"),
        );
        proxy
            .dispatch_exec(Action::git_op("status", vec![]))
            .await
            .expect("dispatch ok");
        let calls = runner.calls.lock().unwrap().clone();
        assert_eq!(
            calls.first().map(|c| c.as_slice()),
            Some(["git".to_string(), "rev-parse".into(), "HEAD".into()].as_slice()),
            "pre-op probe — rev-parse ПЕРВЫМ"
        );
        assert_eq!(calls.len(), 2, "probe + сам op");
        let rep = last_report.lock().unwrap().clone().expect("report");
        assert_eq!(
            rep.undo_ref.as_deref(),
            Some("a1b2c3d4e5"),
            "trimmed sha донесён как undo_ref"
        );
        drop(proxy);
        let _ = host.await;
    }

    /// 6c-2h best-effort: rev-parse падает (не git-репо/detached) → undo_ref None, dispatch НЕ падает.
    #[tokio::test]
    async fn gitop_rev_parse_failure_irreversible() {
        let (host_t, sbx_t) = channel_pair();
        let last_report = Arc::new(Mutex::new(None));
        let host = tokio::spawn(mock_host(
            host_t,
            WireExecDecision::Approved {
                exec_token: "tok".into(),
                ledger_action_id: 1,
            },
            go_echo(),
            last_report.clone(),
        ));
        let runner = recording(128, "fatal: not a git repository");
        let proxy = ProxyExecDispatcher::new(
            sbx_t,
            runner,
            PathBuf::from("/tmp"),
            PathBuf::from("/vault"),
        );
        let outcome = proxy
            .dispatch_exec(Action::git_op("status", vec![]))
            .await
            .expect("dispatch ok несмотря на провал probe");
        assert!(matches!(outcome, ExecToolOutcome::Executed { .. }));
        let rep = last_report.lock().unwrap().clone().expect("report");
        assert!(
            rep.undo_ref.is_none(),
            "rev-parse fail → undo_ref None (необратимо)"
        );
        drop(proxy);
        let _ = host.await;
    }

    /// 6c-2h: не-GitOp (shell.run) НЕ снимает pre-op ref (probe не запущен) → report.undo_ref None.
    #[tokio::test]
    async fn non_gitop_skips_pre_op_probe() {
        let (host_t, sbx_t) = channel_pair();
        let last_report = Arc::new(Mutex::new(None));
        let host = tokio::spawn(mock_host(
            host_t,
            WireExecDecision::Approved {
                exec_token: "tok".into(),
                ledger_action_id: 1,
            },
            go_echo(),
            last_report.clone(),
        ));
        let runner = recording(0, "deadbeef");
        let proxy = ProxyExecDispatcher::new(
            sbx_t,
            runner.clone(),
            PathBuf::from("/tmp"),
            PathBuf::from("/vault"),
        );
        proxy
            .dispatch_exec(Action::shell_run(vec!["ls".into()], None))
            .await
            .expect("dispatch ok");
        let calls = runner.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1, "shell.run — только op, без probe");
        assert!(
            !calls.iter().any(|c| c == &["git", "rev-parse", "HEAD"]),
            "нет rev-parse для не-GitOp"
        );
        let rep = last_report.lock().unwrap().clone().expect("report");
        assert!(rep.undo_ref.is_none(), "shell необратим — undo_ref None");
        drop(proxy);
        let _ = host.await;
    }

    /// WireExecKind в проводе — sanity (decide шлёт exec-вид, не vault).
    #[test]
    fn wire_action_is_exec_kind() {
        let w = WireExecAction::try_from(&Action::git_op("status", vec![])).unwrap();
        assert_eq!(w.kind, WireExecKind::GitOp);
    }
}
