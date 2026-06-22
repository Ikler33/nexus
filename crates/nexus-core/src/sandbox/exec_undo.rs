//! exec_undo — РЕАЛЬНЫЙ откат exec-GitOp (SANDBOX-6c-3d-2): прод-[`UndoExecDriver`], исполняющий
//! `git reset --hard <pre-op-ref>` в ИЗОЛИРОВАННОМ хардненном контейнере под host-апрувом.
//!
//! **Инверсия §5.2 сохранена**: host РЕШАЕТ (синтез `git_op("reset",…)` → `dispatch_exec_decision`
//! classify→Confirm→approve→ledger), КОНТЕЙНЕР ИСПОЛНЯЕТ (`git` бежит ВНУТРИ `--network=none` контейнера,
//! не на host). Откат GitOp — сам мутирующий GitOp ⇒ RE-ENTER'ит ТОТ ЖЕ гейт (НЕ привилегированный путь):
//!  - `is_git_sha` ре-валидация ref host-side (`UndoExecDriver`-контракт; ledger мог быть подменён);
//!  - classify → Confirm НИКОГДА Auto ⇒ скомпрометированный агент не само-апрувит свой undo; одобряет ТОЛЬКО
//!    оператор через `--sandbox-undo --approve` ([`crate::actuator::ApproveAll`]); без флага → PolicyDefault
//!    DENY → `Deferred` (строка остаётся executed, безопасно);
//!  - ledger: синтезированный reset — СВОЯ строка `agent_actions` (proposed→approved→executing→executed|
//!    failed); исходную GitOp-строку `undo_run` помечает `undone` ТОЛЬКО при `Restored` (двухстрочный аудит);
//!  - vault ВСЕГДА `:ro`: писать может ТОЛЬКО отдельный owner-сконфигурированный `ai.git_worktree` rw-mount.
//!
//! `GitResetRunner`-шов изолирует ЕДИНСТВЕННЫЙ podman-launch (прод [`PodmanGitResetRunner`], marked) → вся
//! host-логика гейта/ledger Tier-1-тестируема через [`MockGitResetRunner`] без podman. Реальный контейнерный
//! reset — Tier-2 на .28 (docs/runbooks/sandbox-tier2.md).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use crate::actuator::audit::{self, STATE_APPROVED, STATE_EXECUTED, STATE_EXECUTING, STATE_FAILED};
use crate::actuator::{
    dispatch_exec_decision, Action, AuditSink, DecisionSource, DispatchPolicy, EventSink,
    ExecDecision, UndoExecDriver, UndoStatus,
};

use super::exec_host::is_git_sha;
use super::DEFAULT_SANDBOX_IMAGE;

/// Шов исполнителя git-reset ВНУТРИ контейнера. Прод — [`PodmanGitResetRunner`] (единственный podman-launch);
/// Tier-1 — [`MockGitResetRunner`] (скриптованный exit, без podman). Возвращает exit-код процесса `git`
/// (`Err` — не удалось ДАЖЕ запустить контейнер: тогда вызывающий трактует как провал, ledger→FAILED).
#[async_trait]
pub trait GitResetRunner: Send + Sync {
    /// Исполнить `git reset --hard <reference>` в `worktree` (rw-mount) внутри хардненного контейнера.
    async fn run_reset(&self, worktree: &Path, reference: &str) -> Result<i32, String>;
}

/// Прод-исполнитель: запускает `git reset --hard` в эфемерном хардненном `--network=none` контейнере с
/// `worktree` rw-смонтированным в `/work` (vault НЕ монтируется — это ОТДЕЛЬНЫЙ rw-mount). git бежит под
/// uid:gid владельца worktree (no dubious-ownership) + `-c safe.directory=/work`.
pub struct PodmanGitResetRunner {
    image: String,
}

impl PodmanGitResetRunner {
    pub fn new(image: impl Into<String>) -> Self {
        Self {
            image: image.into(),
        }
    }
}

impl Default for PodmanGitResetRunner {
    fn default() -> Self {
        Self::new(DEFAULT_SANDBOX_IMAGE)
    }
}

#[async_trait]
impl GitResetRunner for PodmanGitResetRunner {
    async fn run_reset(&self, worktree: &Path, reference: &str) -> Result<i32, String> {
        // uid:gid владельца worktree → git бежит как владелец (нет 'dubious ownership'); Unix-only.
        #[cfg(unix)]
        let user = {
            use std::os::unix::fs::MetadataExt;
            let md = std::fs::metadata(worktree).map_err(|e| format!("worktree metadata: {e}"))?;
            format!("{}:{}", md.uid(), md.gid())
        };
        #[cfg(not(unix))]
        let user = "0:0".to_string();

        let mount = format!("{}:/work:rw", worktree.display());
        let args = [
            "run",
            "--rm",
            "--network=none",
            "--read-only", // rootfs ro; единственный writable real-mount — /work (worktree) + tmpfs /tmp
            "--tmpfs",
            "/tmp",
            "--cap-drop=ALL",
            "--security-opt=no-new-privileges",
            "--user",
            &user,
            "-v",
            &mount,
            "-w",
            "/work",
            "-e",
            "HOME=/tmp",
            &self.image,
            "git",
            "-c",
            "safe.directory=/work",
            "reset",
            "--hard",
            reference,
        ];
        // sandbox-exec-lint: allow podman-launch (запуск САМОГО podman для undo-контейнера; git бежит ВНУТРИ
        // него, host не спавнит git напрямую — INV-UNDO-NO-HOST-GIT).
        let out = tokio::process::Command::new("podman")
            .args(args)
            .output()
            .await
            .map_err(|e| format!("podman run (git reset): {e}"))?;
        Ok(out.status.code().unwrap_or(-1))
    }
}

/// Прод-[`UndoExecDriver`] (6c-3d-2): синтезирует `git reset --hard <ref>`, прогоняет через host/exec гейт
/// (classify→Confirm→approve→ledger) и исполняет в контейнере через [`GitResetRunner`]. `worktree=None`
/// (default `ai.git_worktree`) ⇒ `Deferred` (откат не настроен — vault `:ro`, безопасно).
pub struct SandboxUndoExecDriver<R: GitResetRunner> {
    ledger: Arc<AuditSink>,
    run_id: i64,
    canon_root: PathBuf,
    policy: DispatchPolicy,
    decision: Arc<dyn DecisionSource>,
    events: Arc<dyn EventSink>,
    /// Owner-сконфигурированный rw git-worktree (`ai.git_worktree`); `None` ⇒ откат Deferred.
    worktree: Option<PathBuf>,
    runner: R,
}

impl<R: GitResetRunner> SandboxUndoExecDriver<R> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ledger: Arc<AuditSink>,
        run_id: i64,
        canon_root: PathBuf,
        policy: DispatchPolicy,
        decision: Arc<dyn DecisionSource>,
        events: Arc<dyn EventSink>,
        worktree: Option<PathBuf>,
        runner: R,
    ) -> Self {
        Self {
            ledger,
            run_id,
            canon_root,
            policy,
            decision,
            events,
            worktree,
            runner,
        }
    }
}

#[async_trait]
impl<R: GitResetRunner> UndoExecDriver for SandboxUndoExecDriver<R> {
    async fn undo_gitref(&self, reference: &str) -> UndoStatus {
        // 0. worktree не настроен ⇒ откат честно Deferred (vault :ro, scratch эфемерен — reset некуда писать).
        let Some(worktree) = self.worktree.as_deref() else {
            return UndoStatus::Deferred(format!(
                "exec-GitOp откат отложен: настройте `ai.git_worktree` (owner-gated rw-репозиторий) для \
                 реального `git reset --hard {reference}`"
            ));
        };
        // 1. HOST-AUTHORITY над ref (defense-in-depth поверх undo_run): мусор ⇒ Failed, контейнер не трогаем.
        if !is_git_sha(reference) {
            return UndoStatus::Failed(format!(
                "exec-undo: невалидный git-ref ({reference:?}) — откат невозможен (fail-closed)"
            ));
        }
        // 2. Синтез + ГЕЙТ: git reset — сам мутирующий GitOp ⇒ classify→Confirm (НИКОГДА Auto). Approve
        //    выдаёт ТОЛЬКО оператор (ApproveAll под --approve); иначе PolicyDefault DENY → Deferred.
        let action = Action::git_op("reset", vec!["--hard".to_string(), reference.to_string()]);
        let propose_key = match dispatch_exec_decision(
            &action,
            self.run_id,
            &self.policy,
            &self.decision,
            self.ledger.as_ref(),
            &self.canon_root,
            self.events.as_ref(),
        )
        .await
        {
            ExecDecision::Approved { propose_key, .. } => propose_key,
            ExecDecision::Rejected(s) => {
                // Не одобрено (нет --approve / пауза) — НЕ провал, откат отложен (строка остаётся executed).
                return UndoStatus::Deferred(format!("exec-GitOp откат не одобрен: {s}"));
            }
            ExecDecision::HardBlocked(r) => {
                // HardBlocked = НЕ-ВКЛЮЧЁННАЯ конфигурация (shell_enable=false / sandbox недоступна), а НЕ
                // провал отката. Честно Deferred (как no-worktree): «настройте и повторите», НЕ exit-1.
                // Контейнер не трогаем. (review MAJOR: было Failed → вводило в заблуждение при default-config.)
                return UndoStatus::Deferred(format!(
                    "exec-GitOp откат отложен: {r} (нужен `ai.shell_enable=true` + `ai.git_worktree` + `--approve`)"
                ));
            }
        };
        // 3. ledger APPROVED→EXECUTING (write-before-act) ДО запуска контейнера.
        let promoted = audit::transition(
            &self.ledger.writer_handle(),
            &propose_key,
            STATE_APPROVED,
            STATE_EXECUTING,
        )
        .await
        .unwrap_or(false);
        if !promoted {
            return UndoStatus::Failed(
                "exec-undo: ledger approved→executing не применён (гонка/состояние)".into(),
            );
        }
        // 4. ИСПОЛНЕНИЕ в контейнере (git ВНУТРИ песочницы, не на host).
        let exit = self.runner.run_reset(worktree, reference).await;
        // 5. ledger EXECUTING→EXECUTED|FAILED (структурный outcome, без сырого вывода).
        let (state, status) = match exit {
            Ok(0) => (STATE_EXECUTED, UndoStatus::Restored),
            Ok(code) => (
                STATE_FAILED,
                UndoStatus::Failed(format!("git reset завершился с кодом {code}")),
            ),
            Err(e) => (STATE_FAILED, UndoStatus::Failed(format!("exec-undo: {e}"))),
        };
        let outcome = match &status {
            UndoStatus::Restored => "exec-undo: git reset --hard OK".to_string(),
            _ => "exec-undo: git reset не удался".to_string(),
        };
        let _ = audit::finish(
            &self.ledger.writer_handle(),
            &propose_key,
            state,
            &outcome,
            None,
        )
        .await;
        status
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actuator::{
        ApproveAll, DispatchPolicy, PolicyDefault, TracingEventSink, OVERWRITE_THRESHOLD,
    };
    use crate::db::Database;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    /// Mock git-runner: скриптованный exit + счётчик вызовов (для «инъекц-ref/no-worktree → не вызван»).
    struct MockGitResetRunner {
        exit: Result<i32, String>,
        calls: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl GitResetRunner for MockGitResetRunner {
        async fn run_reset(&self, _worktree: &Path, reference: &str) -> Result<i32, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert!(
                is_git_sha(reference),
                "runner получает валидный ref: {reference:?}"
            );
            self.exit.clone()
        }
    }

    async fn driver(
        exit: Result<i32, String>,
        worktree: Option<PathBuf>,
        approve: bool,
    ) -> (
        TempDir,
        SandboxUndoExecDriver<MockGitResetRunner>,
        Arc<AtomicUsize>,
    ) {
        driver_cfg(exit, worktree, approve, true).await
    }

    async fn driver_cfg(
        exit: Result<i32, String>,
        worktree: Option<PathBuf>,
        approve: bool,
        shell_enable: bool,
    ) -> (
        TempDir,
        SandboxUndoExecDriver<MockGitResetRunner>,
        Arc<AtomicUsize>,
    ) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let db = Database::open(root.join(".nexus/nexus.db")).await.unwrap();
        let ledger = Arc::new(AuditSink::new(db.writer().clone(), db.reader().clone()));
        std::mem::forget(db);
        let policy = DispatchPolicy::new(Some("auto"), OVERWRITE_THRESHOLD, 16)
            .with_exec_flags(shell_enable, true);
        let decision: Arc<dyn DecisionSource> = if approve {
            Arc::new(ApproveAll)
        } else {
            Arc::new(PolicyDefault)
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let runner = MockGitResetRunner {
            exit,
            calls: calls.clone(),
        };
        let drv = SandboxUndoExecDriver::new(
            ledger,
            1,
            root,
            policy,
            decision,
            Arc::new(TracingEventSink::new()),
            worktree,
            runner,
        );
        (dir, drv, calls)
    }

    /// worktree=None ⇒ Deferred, контейнер НЕ запускается (runner 0 вызовов).
    #[tokio::test]
    async fn no_worktree_is_deferred() {
        let (_d, drv, calls) = driver(Ok(0), None, true).await;
        assert!(matches!(
            drv.undo_gitref("abc123").await,
            UndoStatus::Deferred(_)
        ));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "без worktree git не запускается"
        );
    }

    /// Невалидный ref ⇒ Failed, контейнер НЕ запускается.
    #[tokio::test]
    async fn invalid_ref_is_failed_no_run() {
        let wt = TempDir::new().unwrap();
        let (_d, drv, calls) = driver(Ok(0), Some(wt.path().to_path_buf()), true).await;
        assert!(matches!(
            drv.undo_gitref("HEAD; rm -rf ~").await,
            UndoStatus::Failed(_)
        ));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "инъекц-ref → git не запускается"
        );
    }

    /// --approve + git exit 0 ⇒ Restored; синтезированная reset-строка ledger → executed.
    #[tokio::test]
    async fn approve_exit0_is_restored() {
        let wt = TempDir::new().unwrap();
        let (_d, drv, calls) = driver(Ok(0), Some(wt.path().to_path_buf()), true).await;
        assert_eq!(drv.undo_gitref("a1b2c3d4").await, UndoStatus::Restored);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "git reset запущен");
    }

    /// --approve + git exit!=0 ⇒ Failed.
    #[tokio::test]
    async fn approve_nonzero_is_failed() {
        let wt = TempDir::new().unwrap();
        let (_d, drv, _c) = driver(Ok(1), Some(wt.path().to_path_buf()), true).await;
        assert!(matches!(
            drv.undo_gitref("a1b2c3d4").await,
            UndoStatus::Failed(_)
        ));
    }

    /// БЕЗ --approve (PolicyDefault DENY) ⇒ Deferred (Confirm не одобрен), контейнер НЕ запускается.
    #[tokio::test]
    async fn no_approve_is_deferred_no_run() {
        let wt = TempDir::new().unwrap();
        let (_d, drv, calls) = driver(Ok(0), Some(wt.path().to_path_buf()), false).await;
        assert!(matches!(
            drv.undo_gitref("a1b2c3d4").await,
            UndoStatus::Deferred(_)
        ));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "без апрува git не запускается (Confirm-never-Auto)"
        );
    }

    /// shell_enable=false (default-config!) даже с worktree+approve ⇒ classify HardBlocked(ShellDisabled) →
    /// честный Deferred (НЕ Failed/exit-1, review MAJOR), контейнер НЕ запускается. Сообщение зовёт включить
    /// `ai.shell_enable` — пиннит прод-проводку, что раньше была непокрыта (все прочие тесты shell_enable=true).
    #[tokio::test]
    async fn shell_disabled_is_deferred_no_run() {
        let wt = TempDir::new().unwrap();
        let (_d, drv, calls) = driver_cfg(Ok(0), Some(wt.path().to_path_buf()), true, false).await;
        match drv.undo_gitref("a1b2c3d4").await {
            UndoStatus::Deferred(m) => assert!(
                m.contains("shell_enable"),
                "честная подсказка про shell_enable: {m}"
            ),
            other => {
                panic!("ожидался Deferred (не Failed) при shell_enable=false, получено {other:?}")
            }
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "shell_enable=false → git не запускается"
        );
    }
}
