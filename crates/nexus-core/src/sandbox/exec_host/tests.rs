use super::*;

fn exec_actions() -> Vec<Action> {
    vec![
        Action::shell_run(vec!["ls".into(), "-la".into()], Some("Notes".into())),
        Action::process_spawn("git", vec!["status".into()], None),
        Action::git_op("log", vec!["--oneline".into()]),
    ]
}

#[test]
fn wire_exec_action_roundtrip_all_exec_targets() {
    for a in exec_actions() {
        let wire = WireExecAction::try_from(&a).unwrap();
        let json = serde_json::to_string(&wire).unwrap();
        let back: WireExecAction = serde_json::from_str(&json).unwrap();
        let a2: Action = back.try_into().unwrap();
        assert_eq!(a, a2, "round-trip Action↔WireExecAction↔JSON: {a:?}");
    }
}

#[test]
fn vault_target_not_representable_on_host_exec() {
    for a in [
        Action::note_create("A.md", "b"),
        Action::note_edit("B.md", "c"),
        Action::frontmatter("C.md", "k", "v"),
    ] {
        assert!(
            WireExecAction::try_from(&a).is_err(),
            "vault не на host/exec: {a:?}"
        );
    }
}

#[test]
fn wire_exec_request_rejects_unknown_field() {
    let json = r#"{"phase":"decide","action":{"kind":"git_op","op":"status"},"bogus":1}"#;
    assert!(serde_json::from_str::<WireExecRequest>(json).is_err());
}

/// Мок-бэкенд: возвращает заданное решение (без classify/ledger).
struct MockExec(WireExecDecision);
#[async_trait]
impl ExecBackend for MockExec {
    async fn decide(&self, _action: &Action) -> WireExecDecision {
        self.0.clone()
    }
}

#[tokio::test]
async fn host_exec_server_decide_maps_approved() {
    let srv = HostExecServer::new(MockExec(WireExecDecision::Approved {
        exec_token: "tok-1".into(),
        ledger_action_id: 7,
    }));
    let req = WireExecRequest {
        phase: WireExecPhase::Decide,
        action: Some(WireExecAction::try_from(&Action::git_op("status", vec![])).unwrap()),
        exec_token: None,
        exit_code: None,
        stdout_tail: None,
        stderr_tail: None,
        undo_ref: None,
    };
    let out = srv
        .handle(HOST_EXEC, serde_json::to_value(req).unwrap())
        .await
        .unwrap();
    let dec: WireExecDecision = serde_json::from_value(out).unwrap();
    assert_eq!(
        dec,
        WireExecDecision::Approved {
            exec_token: "tok-1".into(),
            ledger_action_id: 7
        }
    );
}

/// HostExecServer роутит фазу Execute в backend.execute(); MockExec НЕ переопределяет execute →
/// default-impl `invalid_params` (мок/6c-1-уровень инертен). Реальный redeem-путь — DispatchExecBackend.
#[tokio::test]
async fn host_exec_server_execute_routes_to_backend_default_inert() {
    let srv = HostExecServer::new(MockExec(WireExecDecision::Rejected {
        summary: "x".into(),
    }));
    let req = WireExecRequest {
        phase: WireExecPhase::Execute,
        action: None,
        exec_token: Some("tok".into()),
        exit_code: None,
        stdout_tail: None,
        stderr_tail: None,
        undo_ref: None,
    };
    assert!(srv
        .handle(HOST_EXEC, serde_json::to_value(req).unwrap())
        .await
        .is_err());
}

#[tokio::test]
async fn host_exec_unknown_method_not_found() {
    let srv = HostExecServer::new(MockExec(WireExecDecision::Rejected {
        summary: "x".into(),
    }));
    assert!(srv.handle("host/act", Value::Null).await.is_err());
}

/// fail-closed: decide-запрос с execute/report-полем (exec_token) отвергается (кросс-фазовый mix).
#[tokio::test]
async fn host_exec_decide_rejects_cross_phase_fields() {
    let srv = HostExecServer::new(MockExec(WireExecDecision::Rejected {
        summary: "unreached".into(),
    }));
    let json = serde_json::json!({
        "phase": "decide",
        "action": {"kind": "git_op", "op": "status"},
        "exec_token": "smuggled",
    });
    assert!(
        srv.handle(HOST_EXEC, json).await.is_err(),
        "decide с exec_token → invalid_params"
    );
}

// ── DispatchExecBackend end-to-end (Tier-1: настоящий vault+БД+ledger, classify_exec+decision) ──
use crate::actuator::{
    AuditSink, ChannelDecision, CollectingSink, DecisionSource, DispatchPolicy, EventSink,
    GatedToolCtx, ItemDecision, PolicyDefault, TracingEventSink, OVERWRITE_THRESHOLD,
};
use crate::agent::event::AgentEvent;
use crate::db::Database;
use std::sync::Arc;
use tempfile::TempDir;

/// Реальный GatedToolCtx с exec-флагами (shell_enable/sandbox_available) + источником решений.
async fn exec_gate(
    shell_enable: bool,
    sandbox_available: bool,
    decision: Arc<dyn DecisionSource>,
) -> (TempDir, DispatchExecBackend) {
    let dir = TempDir::new().unwrap();
    let canon_root = dir.path().canonicalize().unwrap();
    let db = Database::open(canon_root.join(".nexus/nexus.db"))
        .await
        .unwrap();
    let ledger = AuditSink::new(db.writer().clone(), db.reader().clone());
    std::mem::forget(db);
    let policy = DispatchPolicy::new(Some("auto"), OVERWRITE_THRESHOLD, 16)
        .with_exec_flags(shell_enable, sandbox_available);
    let events: Arc<dyn EventSink> = Arc::new(TracingEventSink::new());
    let ctx = GatedToolCtx::new(canon_root, ledger, 1, policy, decision, events);
    (dir, DispatchExecBackend::new(ctx))
}

/// Как [`exec_gate`], но с ВНЕШНИМ kill-switch `paused` (тест взводит его между decide и execute).
async fn exec_gate_with_pause(
    decision: Arc<dyn DecisionSource>,
    paused: Arc<std::sync::atomic::AtomicBool>,
) -> (TempDir, DispatchExecBackend) {
    let dir = TempDir::new().unwrap();
    let canon_root = dir.path().canonicalize().unwrap();
    let db = Database::open(canon_root.join(".nexus/nexus.db"))
        .await
        .unwrap();
    let ledger = AuditSink::new(db.writer().clone(), db.reader().clone());
    std::mem::forget(db);
    let policy = DispatchPolicy::with_paused(Some("auto"), OVERWRITE_THRESHOLD, 16, paused)
        .with_exec_flags(true, true);
    let events: Arc<dyn EventSink> = Arc::new(TracingEventSink::new());
    let ctx = GatedToolCtx::new(canon_root, ledger, 1, policy, decision, events);
    (dir, DispatchExecBackend::new(ctx))
}

/// Как [`exec_gate`] (shell_enable+sandbox), но с [`CollectingSink`] — тест читает эмитированные
/// ExecProposal/ExecResult (6c-2g наблюдаемость).
async fn exec_gate_collecting(
    decision: Arc<dyn DecisionSource>,
) -> (TempDir, DispatchExecBackend, Arc<CollectingSink>) {
    let dir = TempDir::new().unwrap();
    let canon_root = dir.path().canonicalize().unwrap();
    let db = Database::open(canon_root.join(".nexus/nexus.db"))
        .await
        .unwrap();
    let ledger = AuditSink::new(db.writer().clone(), db.reader().clone());
    std::mem::forget(db);
    let policy =
        DispatchPolicy::new(Some("auto"), OVERWRITE_THRESHOLD, 16).with_exec_flags(true, true);
    let sink = Arc::new(CollectingSink::new());
    let events: Arc<dyn EventSink> = sink.clone();
    let ctx = GatedToolCtx::new(canon_root, ledger, 1, policy, decision, events);
    (dir, DispatchExecBackend::new(ctx), sink)
}

/// shell_enable=false → HardBlocked(ShellDisabled), токен НЕ выдан (pending пуст).
#[tokio::test]
async fn dispatch_exec_shell_disabled_hardblocked_no_token() {
    let (_d, backend) = exec_gate(false, true, Arc::new(PolicyDefault)).await;
    let dec = backend.decide(&Action::git_op("status", vec![])).await;
    assert!(
        matches!(dec, WireExecDecision::HardBlocked { .. }),
        "dec={dec:?}"
    );
    assert_eq!(backend.pending_count(), 0, "HardBlocked не минтит токен");
}

/// shell_enable+sandbox, но PolicyDefault (DENY headless) → Rejected, токен НЕ выдан.
#[tokio::test]
async fn dispatch_exec_policy_default_rejected_no_token() {
    let (_d, backend) = exec_gate(true, true, Arc::new(PolicyDefault)).await;
    let dec = backend
        .decide(&Action::shell_run(vec!["ls".into()], None))
        .await;
    assert!(
        matches!(dec, WireExecDecision::Rejected { .. }),
        "dec={dec:?}"
    );
    assert_eq!(backend.pending_count(), 0, "Rejected не минтит токен");
}

/// shell_enable+sandbox + Approve (ChannelDecision) → Approved + одноразовый токен сохранён.
#[tokio::test]
async fn dispatch_exec_approved_mints_token() {
    // action_id первой proposed-строки в пустой БД = 1; засеваем Approve по id=1.
    let (chan, tx) = ChannelDecision::new(1);
    tx.send(crate::actuator::BatchDecision::from_pairs([(
        1,
        ItemDecision::Approve,
    )]))
    .await
    .unwrap();
    let (_d, backend) = exec_gate(true, true, Arc::new(chan)).await;
    let dec = backend
        .decide(&Action::shell_run(vec!["echo".into(), "hi".into()], None))
        .await;
    match dec {
        WireExecDecision::Approved {
            exec_token,
            ledger_action_id,
        } => {
            assert!(!exec_token.is_empty(), "токен непуст");
            assert_eq!(ledger_action_id, 1);
            assert_eq!(
                backend.pending_count(),
                1,
                "одобренный exec сохранён под токеном"
            );
            // 6c-2b: PendingExec несёт непустой propose_key (ledger-фенс redeem/finalize).
            assert!(
                backend
                    .only_pending_propose_key()
                    .is_some_and(|k| !k.is_empty()),
                "propose_key сохранён непустым"
            );
        }
        other => panic!("ожидался Approved, получено {other:?}"),
    }
}

// ── 6c-2b: build_exec_env (allow-list §5.4) ──────────────────────────────────────────────────
#[test]
fn build_exec_env_is_allowlist_only() {
    std::env::set_var("NEXUS_FAKE_SECRET", "leaked");
    let env = build_exec_env("/tmp", &[]);
    std::env::remove_var("NEXUS_FAKE_SECRET");
    let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, vec!["PATH", "LANG", "HOME"], "только фикс-набор");
    assert!(
        !env.iter().any(|(k, _)| k == "NEXUS_FAKE_SECRET"),
        "host-секрет НЕ просочился (build_exec_env не читает std::env)"
    );
}

#[test]
fn build_exec_env_home_is_scratch() {
    let env = build_exec_env("/tmp", &[]);
    assert_eq!(
        env.iter()
            .find(|(k, _)| k == "HOME")
            .map(|(_, v)| v.as_str()),
        Some("/tmp"),
        "HOME = scratch (не host HOME)"
    );
}

#[test]
fn build_exec_env_includes_declared_passthrough() {
    let env = build_exec_env("/tmp", &[("FOO".into(), "bar".into())]);
    assert!(
        env.iter().any(|(k, v)| k == "FOO" && v == "bar"),
        "объявленный passthrough присутствует"
    );
}

/// fail-closed: skill-passthrough НЕ переопределяет зарезервированные PATH/HOME/LANG.
#[test]
fn build_exec_env_passthrough_cannot_override_reserved() {
    let env = build_exec_env(
        "/tmp",
        &[
            ("PATH".into(), "/evil".into()),
            ("HOME".into(), "/evil".into()),
        ],
    );
    let path = env
        .iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.as_str());
    let home = env
        .iter()
        .find(|(k, _)| k == "HOME")
        .map(|(_, v)| v.as_str());
    assert_eq!(
        path,
        Some("/usr/local/bin:/usr/bin:/bin"),
        "PATH из фикс-набора, не из skill"
    );
    assert_eq!(home, Some("/tmp"), "HOME из scratch, не из skill");
    // и НЕ продублирован
    assert_eq!(
        env.iter().filter(|(k, _)| k == "PATH").count(),
        1,
        "PATH не задублирован"
    );
}

// ── 6c-2b: build_exec_go (argv host-authority + дефолты) ──────────────────────────────────────
#[test]
fn build_exec_go_argv_from_action() {
    let g = build_exec_go(&Action::git_op("status", vec!["--short".into()]), &[]);
    assert_eq!(g.argv, vec!["git", "status", "--short"]);
    let g = build_exec_go(
        &Action::shell_run(vec!["ls".into(), "-la".into()], None),
        &[],
    );
    assert_eq!(g.argv, vec!["ls", "-la"]);
    let g = build_exec_go(&Action::process_spawn("rg", vec!["foo".into()], None), &[]);
    assert_eq!(g.argv, vec!["rg", "foo"]);
}

#[test]
fn build_exec_go_defaults_scratch_cwd_and_caps() {
    let g = build_exec_go(
        &Action::shell_run(vec!["ls".into()], Some("sub".into())),
        &[],
    );
    assert_eq!(g.cwd, ExecCwd::ScratchTmpfs { rel: "sub".into() });
    assert_eq!(g.timeout_ms, super::super::DEFAULT_EXEC_TIMEOUT_MS);
    assert_eq!(g.output_cap_bytes, super::super::DEFAULT_EXEC_OUTPUT_CAP);
    // env = allow-list
    let keys: Vec<&str> = g.env.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, vec!["PATH", "LANG", "HOME"]);
}

#[test]
fn build_exec_go_no_cwd_defaults_empty_scratch() {
    let g = build_exec_go(&Action::git_op("log", vec![]), &[]);
    assert_eq!(g.cwd, ExecCwd::ScratchTmpfs { rel: String::new() });
}

/// Soft-cap: при заполненном store decide отказывает ДО записи ledger (новый токен не добавлен).
#[tokio::test]
async fn dispatch_exec_pending_soft_cap_rejects() {
    // PolicyDefault — но до источника решений не дойдём: cap-чек срабатывает раньше.
    let (_d, backend) = exec_gate(true, true, Arc::new(PolicyDefault)).await;
    backend.force_fill_pending(MAX_PENDING_EXEC);
    let dec = backend
        .decide(&Action::shell_run(vec!["ls".into()], None))
        .await;
    assert!(
        matches!(dec, WireExecDecision::Rejected { .. }),
        "at-cap → Rejected, dec={dec:?}"
    );
    assert_eq!(
        backend.pending_count(),
        MAX_PENDING_EXEC,
        "кэп не превышен — новый exec не добавлен"
    );
}

// ── 6c-2c: execute (redeem токена) ───────────────────────────────────────────────────────────
/// Approve→execute: токен консьюмнут (pending пуст), in_flight=1 (ledger approved→executing прошёл),
/// WireExecGo argv из СОХРАНЁННОГО действия (host-authority).
#[tokio::test]
async fn execute_redeems_approved_token() {
    let (chan, tx) = ChannelDecision::new(1);
    tx.send(crate::actuator::BatchDecision::from_pairs([(
        1,
        ItemDecision::Approve,
    )]))
    .await
    .unwrap();
    let (_d, backend) = exec_gate(true, true, Arc::new(chan)).await;
    let token = match backend
        .decide(&Action::shell_run(vec!["ls".into(), "-la".into()], None))
        .await
    {
        WireExecDecision::Approved { exec_token, .. } => exec_token,
        other => panic!("ожидался Approved, получено {other:?}"),
    };
    let go = backend.execute(&token).await.expect("execute redeem ok");
    assert_eq!(go.argv, vec!["ls", "-la"], "argv из СОХРАНЁННОГО действия");
    assert_eq!(backend.pending_count(), 0, "токен консьюмнут из pending");
    assert_eq!(
        backend.in_flight_count(),
        1,
        "переведён в in_flight (EXECUTING)"
    );
}

/// Неизвестный/непрогнозируемый токен → invalid_params (fail-closed), без побочек.
#[tokio::test]
async fn execute_unknown_token_fails() {
    let (_d, backend) = exec_gate(true, true, Arc::new(PolicyDefault)).await;
    assert!(
        backend.execute("nope-not-a-real-token").await.is_err(),
        "неизвестный токен → ошибка"
    );
    assert_eq!(backend.in_flight_count(), 0);
}

/// Одноразовость: второй execute тем же токеном → ошибка (токен консьюмнут first-call).
#[tokio::test]
async fn execute_token_replay_fails() {
    let (chan, tx) = ChannelDecision::new(1);
    tx.send(crate::actuator::BatchDecision::from_pairs([(
        1,
        ItemDecision::Approve,
    )]))
    .await
    .unwrap();
    let (_d, backend) = exec_gate(true, true, Arc::new(chan)).await;
    let token = match backend
        .decide(&Action::shell_run(vec!["ls".into()], None))
        .await
    {
        WireExecDecision::Approved { exec_token, .. } => exec_token,
        other => panic!("ожидался Approved, получено {other:?}"),
    };
    assert!(backend.execute(&token).await.is_ok(), "первый redeem ok");
    assert!(
        backend.execute(&token).await.is_err(),
        "повторный redeem того же токена → ошибка (one-shot)"
    );
    assert_eq!(backend.in_flight_count(), 1, "in_flight не задвоился");
}

/// KILL-SWITCH LAST-MOMENT: пауза взведена ПОСЛЕ approve → execute консьюмит токен, ловит паузу
/// last-moment-проверкой (ПОСЛЕ consume, ПЕРЕД ledger EXECUTING) → ВОЗВРАЩАЕТ токен в pending (pending=1,
/// in_flight=0, ledger не тронут). un-pause → тот же токен redeem'ится. Пинит «paused ⇒ нет EXECUTING-
/// записи» для exec-пути (review MAJOR: window до transition закрыт зеркалом apply_now).
#[tokio::test]
async fn execute_when_paused_refuses_and_keeps_token() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let paused = Arc::new(AtomicBool::new(false));
    let (chan, tx) = ChannelDecision::new(1);
    tx.send(crate::actuator::BatchDecision::from_pairs([(
        1,
        ItemDecision::Approve,
    )]))
    .await
    .unwrap();
    let (_d, backend) = exec_gate_with_pause(Arc::new(chan), paused.clone()).await;
    let token = match backend
        .decide(&Action::shell_run(vec!["ls".into()], None))
        .await
    {
        WireExecDecision::Approved { exec_token, .. } => exec_token,
        other => panic!("ожидался Approved, получено {other:?}"),
    };
    paused.store(true, Ordering::Relaxed); // взводим kill-switch ПОСЛЕ approve
    assert!(
        backend.execute(&token).await.is_err(),
        "под паузой execute отказывает"
    );
    assert_eq!(
        backend.pending_count(),
        1,
        "токен НЕ тронут (retry после un-pause)"
    );
    assert_eq!(
        backend.in_flight_count(),
        0,
        "ничего не переведено в EXECUTING"
    );
    // un-pause → тот же токен redeem'ится.
    paused.store(false, Ordering::Relaxed);
    assert!(
        backend.execute(&token).await.is_ok(),
        "после un-pause тот же токен ok"
    );
    assert_eq!(backend.in_flight_count(), 1);
}

// ── 6c-2d: report (finalize) ─────────────────────────────────────────────────────────────────
/// Approve→execute→захват propose_key→report. Helper: возвращает (backend, token, propose_key).
async fn approve_execute(action: Action) -> (TempDir, DispatchExecBackend, String, String) {
    let (chan, tx) = ChannelDecision::new(1);
    tx.send(crate::actuator::BatchDecision::from_pairs([(
        1,
        ItemDecision::Approve,
    )]))
    .await
    .unwrap();
    let (dir, backend) = exec_gate(true, true, Arc::new(chan)).await;
    let token = match backend.decide(&action).await {
        WireExecDecision::Approved { exec_token, .. } => exec_token,
        other => panic!("ожидался Approved, получено {other:?}"),
    };
    let propose_key = backend
        .only_pending_propose_key()
        .expect("propose_key до execute");
    backend.execute(&token).await.expect("execute ok");
    (dir, backend, token, propose_key)
}

/// report(exit=0): in_flight консьюмнут, ledger → EXECUTED.
#[tokio::test]
async fn report_finalizes_executed() {
    let (_d, backend, token, pk) =
        approve_execute(Action::shell_run(vec!["ls".into()], None)).await;
    let res = backend
        .report(&token, 0, "ok-output", "", None)
        .await
        .expect("report ok");
    assert_eq!(res.exit_code, 0);
    assert!(res.finalized);
    assert_eq!(backend.in_flight_count(), 0, "in_flight консьюмнут");
    let row = backend.ledger_row(&pk).await.expect("ledger-строка");
    assert_eq!(row.state, STATE_EXECUTED);
}

/// report(exit!=0): ledger → FAILED.
#[tokio::test]
async fn report_finalizes_failed() {
    let (_d, backend, token, pk) =
        approve_execute(Action::shell_run(vec!["false".into()], None)).await;
    backend
        .report(&token, 1, "", "boom", None)
        .await
        .expect("report ok");
    let row = backend.ledger_row(&pk).await.expect("ledger-строка");
    assert_eq!(row.state, STATE_FAILED);
}

/// ПРИВАТНОСТЬ: сырой stdout/stderr НЕ попадает в ledger outcome (только exit + байт-счётчики).
#[tokio::test]
async fn report_does_not_persist_raw_tails() {
    let secret = "SUPER-SECRET-TOKEN-abc123";
    let (_d, backend, token, pk) =
        approve_execute(Action::shell_run(vec!["cat".into()], None)).await;
    backend
        .report(&token, 0, secret, secret, None)
        .await
        .expect("report ok");
    let row = backend.ledger_row(&pk).await.expect("ledger-строка");
    let outcome = row.outcome.unwrap_or_default();
    assert!(
        !outcome.contains(secret),
        "сырой хвост НЕ персистится в ledger: {outcome:?}"
    );
    assert!(
        outcome.contains("exit=0"),
        "структурное резюме: {outcome:?}"
    );
    // undo_ref пока не персистится (6c-2h).
    assert!(row.undo_ref.is_none(), "undo не персистится в 6c-2d");
}

// ── 6c-2g: события ExecProposal/ExecResult (наблюдаемость + приватность) ──────────────────────
/// decide эмитит [`AgentEvent::ExecProposal`] (ПОСЛЕ proposed-строки, ДО решения) с редакция-
/// безопасным `summary` (имя инструмента + счётчики), БЕЗ сырого argv-значения (плантованный секрет
/// в argv ОТСУТСТВУЕТ в summary). `action_id` адресует proposed-строку (id=1).
#[tokio::test]
async fn exec_decide_emits_exec_proposal() {
    let (chan, tx) = ChannelDecision::new(1);
    tx.send(crate::actuator::BatchDecision::from_pairs([(
        1,
        ItemDecision::Approve,
    )]))
    .await
    .unwrap();
    let (_d, backend, sink) = exec_gate_collecting(Arc::new(chan)).await;
    let secret = "SUPER-SECRET-IN-ARGV-xyz789";
    backend
        .decide(&Action::shell_run(vec!["echo".into(), secret.into()], None))
        .await;
    let proposal = sink.events().into_iter().find_map(|e| match e {
        AgentEvent::ExecProposal {
            action_id, summary, ..
        } => Some((action_id, summary)),
        _ => None,
    });
    let (action_id, summary) = proposal.expect("ExecProposal эмитировано");
    assert_eq!(action_id, 1, "адресует proposed-строку");
    assert!(
        summary.contains("shell.run"),
        "summary несёт имя инструмента: {summary:?}"
    );
    assert!(
        !summary.contains(secret),
        "summary НЕ несёт сырой argv-секрет: {summary:?}"
    );
}

/// report эмитит [`AgentEvent::ExecResult`] {exit_code, finalized} — СОДЕРЖИМОЕ-СВОБОДЕН: даже передав
/// сырой stdout/stderr в report, событие несёт ТОЛЬКО exit+finalized (нет stdout-поля by-design).
#[tokio::test]
async fn exec_report_emits_exec_result() {
    let (chan, tx) = ChannelDecision::new(1);
    tx.send(crate::actuator::BatchDecision::from_pairs([(
        1,
        ItemDecision::Approve,
    )]))
    .await
    .unwrap();
    let (_d, backend, sink) = exec_gate_collecting(Arc::new(chan)).await;
    let token = match backend
        .decide(&Action::shell_run(vec!["ls".into()], None))
        .await
    {
        WireExecDecision::Approved { exec_token, .. } => exec_token,
        other => panic!("ожидался Approved, получено {other:?}"),
    };
    backend.execute(&token).await.expect("execute ok");
    backend
        .report(&token, 0, "RAW-STDOUT-secret", "RAW-STDERR", None)
        .await
        .expect("report ok");
    let result = sink.events().into_iter().find_map(|e| match e {
        AgentEvent::ExecResult {
            action_id,
            exit_code,
            finalized,
            ..
        } => Some((action_id, exit_code, finalized)),
        _ => None,
    });
    let (action_id, exit_code, finalized) = result.expect("ExecResult эмитировано");
    assert_eq!(action_id, 1);
    assert_eq!(exit_code, 0);
    assert!(finalized);
}

/// report без execute (нет in_flight) → invalid_params.
#[tokio::test]
async fn report_without_execute_fails() {
    let (_d, backend) = exec_gate(true, true, Arc::new(PolicyDefault)).await;
    assert!(
        backend
            .report("no-such-token", 0, "", "", None)
            .await
            .is_err(),
        "report без execute → ошибка"
    );
}

// ── 6c-2h: GitOp pre-op-ref undo (host-authority над обратимостью) ───────────────────────────
/// GitOp report с undo_ref → ledger undo_kind=exec_gitref + undo_ref=sha (read-back = round-trip).
#[tokio::test]
async fn gitop_report_persists_gitref() {
    let (_d, backend, token, pk) = approve_execute(Action::git_op("status", vec![])).await;
    backend
        .report(&token, 0, "", "", Some("abc1234def"))
        .await
        .expect("report ok");
    let row = backend.ledger_row(&pk).await.expect("ledger-строка");
    assert_eq!(row.state, STATE_EXECUTED);
    assert_eq!(
        row.undo_kind.as_deref(),
        Some("exec_gitref"),
        "GitOp → exec_gitref undo-дискриминант"
    );
    assert_eq!(
        row.undo_ref.as_deref(),
        Some("abc1234def"),
        "pre-op ref восстановим из ledger (round-trip)"
    );
}

/// HOST-AUTHORITY: shell.run report с undo_ref ИГНОРИРУЕТСЯ (не GitOp → необратимо). Контейнер НЕ
/// сделает shell «обратимым» claim'ом undo_ref — обратимость решает СОХРАНЁННОЕ действие host-side.
#[tokio::test]
async fn non_gitop_report_ignores_undo_ref() {
    let (_d, backend, token, pk) =
        approve_execute(Action::shell_run(vec!["ls".into()], None)).await;
    backend
        .report(&token, 0, "", "", Some("spoofed-ref"))
        .await
        .expect("report ok");
    let row = backend.ledger_row(&pk).await.expect("ledger-строка");
    assert!(
        row.undo_kind.is_none(),
        "shell необратим — undo_kind None даже при claim'е undo_ref"
    );
    assert!(
        row.undo_ref.is_none(),
        "claim undo_ref проигнорирован (host-authority над обратимостью)"
    );
}

/// 6c-3e: exec-GitOp откат = `git reset --hard <ref>` как ОБЫЧНОЕ exec-действие; под shell_enable+
/// sandbox+PolicyDefault → Confirm-gated → Rejected. Доказывает: синтезированный откат НИКОГДА не Auto
/// (скомпрометированный агент не само-апрувит свой undo) — он проходит ТОТ ЖЕ гейт, что форвардный GitOp.
/// (Конструкцию `git_op("reset",[...])` гоняет прод-драйвер `SandboxUndoExecDriver` в 6c-3d; здесь
/// пинится её КЛАССИФИКАЦИЯ как never-Auto.)
#[tokio::test]
async fn undo_reset_action_gated_never_auto() {
    let action = Action::git_op("reset", vec!["--hard".into(), "a1b2c3d4".into()]);
    let (_d, backend) = exec_gate(true, true, Arc::new(PolicyDefault)).await;
    let dec = backend.decide(&action).await;
    assert!(
        matches!(dec, WireExecDecision::Rejected { .. }),
        "synthesized reset под PolicyDefault → Rejected (Confirm-gated, never Auto): {dec:?}"
    );
}

/// HOST-AUTHORITY над СОДЕРЖИМЫМ ref (review MAJOR): GitOp report с НЕ-hex undo_ref (инъекц-строка) →
/// host РЕ-валидирует сам ([`is_git_sha`]) → undo НЕ персистится. Скомпрометированный контейнер не
/// пронесёт `git reset --hard <инъекция>` в долговечный ledger мимо in-container probe.
#[tokio::test]
async fn gitop_report_rejects_nonhex_undo_ref() {
    let (_d, backend, token, pk) = approve_execute(Action::git_op("status", vec![])).await;
    backend
        .report(&token, 0, "", "", Some("HEAD; rm -rf ~"))
        .await
        .expect("report ok");
    let row = backend.ledger_row(&pk).await.expect("ledger-строка");
    assert!(
        row.undo_kind.is_none(),
        "не-hex ref отвергнут host-side — undo_kind None (fail-closed)"
    );
    assert!(
        row.undo_ref.is_none(),
        "инъекц-строка НЕ персистится (host-authority над СОДЕРЖИМЫМ ref)"
    );
}

/// [`is_git_sha`] — host-authority предикат валидности git-ref (непустой, ≤64 hex).
#[test]
fn is_git_sha_validates() {
    assert!(is_git_sha("a1b2c3d4"));
    assert!(is_git_sha(&"a".repeat(40)), "SHA-1");
    assert!(is_git_sha(&"f".repeat(64)), "SHA-256");
    assert!(!is_git_sha(""), "пусто");
    assert!(!is_git_sha("HEAD; rm -rf ~"), "инъекция отвергнута");
    assert!(!is_git_sha("not-hex-zz"), "не-hex отвергнут");
    assert!(!is_git_sha(&"a".repeat(65)), "слишком длинно отвергнуто");
}

/// Повторный report тем же токеном → ошибка (in_flight консьюмнут first-call, one-shot финализация).
#[tokio::test]
async fn report_replay_fails() {
    let (_d, backend, token, _pk) =
        approve_execute(Action::shell_run(vec!["ls".into()], None)).await;
    assert!(
        backend.report(&token, 0, "", "", None).await.is_ok(),
        "первый report ok"
    );
    assert!(
        backend.report(&token, 0, "", "", None).await.is_err(),
        "повторный report → ошибка (one-shot)"
    );
}
