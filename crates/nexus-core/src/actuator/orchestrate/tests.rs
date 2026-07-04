use super::*;
use crate::actuator::audit::{lookup, STATE_EXECUTED, STATE_PROPOSED};
use crate::actuator::decision::{BatchDecision, ChannelDecision, PolicyDefault};
use crate::db::Database;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// SANDBOX-6a: новые Phase-3 BlockReason'ы дают осмысленные фенсенные сообщения (вокабуляр для 6b).
#[test]
fn block_message_covers_phase3_reasons() {
    assert!(block_message(&BlockReason::ShellDisabled).contains("shell_enable"));
    assert!(block_message(&BlockReason::SandboxUnavailable).contains("песочница"));
}

/// SL-7: skills-причины дают осмысленные сообщения; `with_skills_flags` ставит поля (дефолт OFF),
/// `change_kind(SkillSave)` → свой токен. Пинит builder (его прод-вызыватель — session.rs в SL-7d).
#[test]
fn skills_block_messages_and_policy_flags() {
    assert!(block_message(&BlockReason::LearningDisabled).contains("learning_enabled"));
    assert!(block_message(&BlockReason::SkillsRootUnconfigured).contains("agent_skills_dir"));
    assert!(block_message(&BlockReason::InvalidSkillTarget).contains("SKILL.md"));

    let p = DispatchPolicy::new(None, 64, 16);
    assert!(
        !p.learning_enabled && !p.skills_root_configured,
        "дефолт skills-флагов — OFF (fail-safe)"
    );
    let p2 = p.with_skills_flags(true, true);
    assert!(p2.learning_enabled && p2.skills_root_configured);

    assert_eq!(
        change_kind(&Action::skill_save("s/SKILL.md", "b")),
        ChangeKind::SkillSave
    );
}

/// 6c-2g: `change_kind` exec-таргетов → [`ChangeKind::Exec`] (токен "exec"); vault → New/Edit.
#[test]
fn change_kind_classifies_exec() {
    assert_eq!(
        change_kind(&Action::shell_run(vec!["ls".into()], None)),
        ChangeKind::Exec
    );
    assert_eq!(
        change_kind(&Action::process_spawn("git", vec![], None)),
        ChangeKind::Exec
    );
    assert_eq!(
        change_kind(&Action::git_op("status", vec![])),
        ChangeKind::Exec
    );
    assert_eq!(
        change_kind(&Action::note_create("A.md", "x")),
        ChangeKind::New
    );
    assert_eq!(
        change_kind(&Action::note_edit("B.md", "y")),
        ChangeKind::Edit
    );
    assert_eq!(ChangeKind::Exec.as_str(), "exec");
}

/// 6c-2g приватность: `exec_proposal_summary` несёт силуэт (имя инструмента / git `op` + счётчик), НЕ
/// сырые argv-значения (плантованный секрет ОТСУТСТВУЕТ в summary).
#[test]
fn exec_proposal_summary_is_content_free() {
    let secret = "TOPSECRET-VALUE-42";
    let s = exec_proposal_summary(&Action::shell_run(vec!["echo".into(), secret.into()], None));
    assert!(s.contains("shell.run"), "несёт имя инструмента: {s:?}");
    assert!(!s.contains(secret), "НЕ несёт сырое argv-значение: {s:?}");
    // git: op-токен присутствует (bounded shape), argv-значения — нет.
    let g = exec_proposal_summary(&Action::git_op("status", vec![secret.into()]));
    assert!(
        g.contains("git.op") && g.contains("status"),
        "git op-силуэт: {g:?}"
    );
    assert!(!g.contains(secret), "git argv-значение НЕ в summary: {g:?}");
}

/// Временный vault + БД + sink. canon_root КАНОНИЗИРОВАН (предусловие resolve_vault_path_for_write).
async fn setup() -> (TempDir, PathBuf, AuditSink) {
    let dir = TempDir::new().unwrap();
    let canon_root = dir.path().canonicalize().unwrap();
    let db = Database::open(canon_root.join(".nexus/nexus.db"))
        .await
        .unwrap();
    let sink = AuditSink::new(db.writer().clone(), db.reader().clone());
    std::mem::forget(db); // writer/reader клонированы в sink — актор жив, пока жив клон.
    (dir, canon_root, sink)
}

fn write_existing(root: &Path, rel: &str, content: &str) {
    let abs = root.join(rel);
    if let Some(p) = abs.parent() {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(abs, content).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap()
}

/// Стандартный порог теста (мал, чтобы крупная правка легко перешагнула).
const T: usize = 100;
/// Ёмкость токен-бакета теста.
const CAP: u32 = 3;

fn policy(autonomy: Option<&str>) -> DispatchPolicy {
    DispatchPolicy::new(autonomy, T, CAP)
}

fn approve(action_id: i64) -> BatchDecision {
    BatchDecision::from_pairs([(action_id, ItemDecision::Approve)])
}

/// Снять единственный action_id из эмитированного Proposal (для адресации решения в тесте).
fn proposed_action_id(sink: &CollectingSink) -> i64 {
    for ev in sink.events() {
        if let AgentEvent::Proposal { files, .. } = ev {
            return files[0].action_id;
        }
    }
    panic!("Proposal не эмитирован");
}

// Примечание про адресацию решения в тестах: строка `proposed` — первый INSERT в пустую БД ⇒
// action_id=1, поэтому Approve-решения засеиваются по id=1 (для надёжности тесты также читают id
// из эмитированного Proposal). Источники: PolicyDefault (reject-all) или ChannelDecision (засев).

/// confirm-run + Auto-тир ⇒ ПРЕДЛАГАЕТ (Proposal+Diff, ledger proposed, файл НЕ записан до Approve).
#[tokio::test]
async fn confirm_run_auto_tier_proposes_not_applied() {
    let (_d, root, sink) = setup().await;
    let events = CollectingSink::new();
    // Источник, который ОТКЛОНЯЕТ (чтобы проверить «не записано до Approve»).
    let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault);
    let action = Action::note_create("Notes/N.md", "hi");

    let out = dispatch_action(
        &action,
        1,
        &policy(Some("confirm")),
        &src,
        &events,
        &sink,
        &root,
    )
    .await
    .unwrap();

    // Предложено и отклонено (PolicyDefault) — файл НЕ создан.
    assert!(matches!(out, DispatchOutcome::Rejected(_)), "out={out:?}");
    assert!(!root.join("Notes/N.md").exists(), "файл НЕ записан");

    // Эмитированы Proposal + Diff с корректной формой.
    let evs = events.events();
    let proposal = evs
        .iter()
        .find(|e| matches!(e, AgentEvent::Proposal { .. }))
        .expect("Proposal эмитирован");
    if let AgentEvent::Proposal { run_id, files } = proposal {
        assert_eq!(*run_id, 1);
        assert_eq!(files[0].path, "Notes/N.md");
        assert_eq!(files[0].status, FileStatus::New);
        assert_eq!(files[0].add, 1, "одна строка добавлена (create)");
    }
    assert!(
        evs.iter().any(|e| matches!(e, AgentEvent::Diff { .. })),
        "Diff эмитирован"
    );

    // Ledger: строка proposed → rejected (терминал с исходом).
    let key = proposal_key(1, &action, "");
    let row = lookup(&sink_reader(&sink), &key).await.unwrap().unwrap();
    assert_eq!(row.state, STATE_REJECTED);
    assert!(row.outcome.is_some());
}

/// confirm-run + Auto-тир + Approve ⇒ ПРИМЕНЯЕТ (файл записан, ledger executed для apply-строки).
#[tokio::test]
async fn confirm_run_approve_applies() {
    let (_d, root, sink) = setup().await;
    let events = CollectingSink::new();
    // action_id строки proposed в пустой БД = 1 (первый INSERT). Засеваем Approve по id=1.
    let (chan, tx) = ChannelDecision::new(1);
    tx.send(approve(1)).await.unwrap();
    let src: Arc<dyn DecisionSource> = Arc::new(chan);
    let action = Action::note_create("Notes/N.md", "hello");

    let out = dispatch_action(
        &action,
        1,
        &policy(Some("confirm")),
        &src,
        &events,
        &sink,
        &root,
    )
    .await
    .unwrap();

    assert!(matches!(out, DispatchOutcome::Applied(_)), "out={out:?}");
    assert_eq!(
        read(&root, "Notes/N.md"),
        "hello",
        "файл записан после Approve"
    );
    assert_eq!(proposed_action_id(&events), 1, "action_id предложения = 1");

    // Ledger: proposed-строка одобрена (approved), а apply записал СВОЮ executed-строку.
    let pkey = proposal_key(1, &action, "");
    let prow = lookup(&sink_reader(&sink), &pkey).await.unwrap().unwrap();
    assert_eq!(prow.state, STATE_APPROVED, "proposed→approved");
    assert!(
        prow.outcome.is_none(),
        "approved НЕ терминальна (apply отдельно)"
    );
}

/// confirm-run + Reject ⇒ ledger rejected, файл НЕ записан.
#[tokio::test]
async fn confirm_run_reject_no_write() {
    let (_d, root, sink) = setup().await;
    let events = CollectingSink::new();
    let (chan, tx) = ChannelDecision::new(1);
    tx.send(BatchDecision::from_pairs([(1, ItemDecision::Reject)]))
        .await
        .unwrap();
    let src: Arc<dyn DecisionSource> = Arc::new(chan);
    let action = Action::note_create("R.md", "x");

    let out = dispatch_action(
        &action,
        1,
        &policy(Some("confirm")),
        &src,
        &events,
        &sink,
        &root,
    )
    .await
    .unwrap();
    assert!(matches!(out, DispatchOutcome::Rejected(_)));
    assert!(!root.join("R.md").exists());
    let key = proposal_key(1, &action, "");
    let row = lookup(&sink_reader(&sink), &key).await.unwrap().unwrap();
    assert_eq!(row.state, STATE_REJECTED);
}

/// auto-run + Auto-тир ⇒ ПРИМЕНЯЕТ напрямую (НЕ предложение), токен бакета потрачен.
#[tokio::test]
async fn auto_run_auto_tier_applies_directly_spends_token() {
    let (_d, root, sink) = setup().await;
    let events = CollectingSink::new();
    let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault); // не должен быть спрошен.
    let pol = policy(Some("auto"));
    let action = Action::note_create("A.md", "auto-body");

    let out = dispatch_action(&action, 1, &pol, &src, &events, &sink, &root)
        .await
        .unwrap();
    assert!(matches!(out, DispatchOutcome::Applied(_)), "out={out:?}");
    assert_eq!(read(&root, "A.md"), "auto-body");
    assert_eq!(
        pol.token_bucket.available(),
        CAP - 1,
        "один токен заклеймлен (CAP={CAP})"
    );
    // НИ Proposal, НИ Diff (применено напрямую).
    assert!(
        !events
            .events()
            .iter()
            .any(|e| matches!(e, AgentEvent::Proposal { .. } | AgentEvent::Diff { .. })),
        "авто-применение НЕ эмитит предложение"
    );
}

/// auto-run + Auto-тир с ПУСТЫМ бакетом (capacity=0) ⇒ ФОРСИРУЕТ предложение (анти-усталость).
#[tokio::test]
async fn auto_run_empty_bucket_forces_proposal() {
    let (_d, root, sink) = setup().await;
    let events = CollectingSink::new();
    let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault); // reject all.
                                                                // Ёмкость = 0 ⇒ даже первое Auto-действие не может заклеймить токен ⇒ предложение.
    let pol = DispatchPolicy::new(Some("auto"), T, 0);
    let action = Action::note_create("Cap.md", "x");

    let out = dispatch_action(&action, 1, &pol, &src, &events, &sink, &root)
        .await
        .unwrap();
    // PolicyDefault reject ⇒ Rejected, файл НЕ записан (форс-предложение реально предложило).
    assert!(matches!(out, DispatchOutcome::Rejected(_)), "out={out:?}");
    assert!(!root.join("Cap.md").exists());
    assert!(
        events
            .events()
            .iter()
            .any(|e| matches!(e, AgentEvent::Proposal { .. })),
        "пустой бакет — предложение"
    );
}

/// Токен-бакет ТОЧНАЯ граница ёмкости (общий бакет прогона): capacity=2 ⇒ ПЕРВЫЕ ДВА Auto
/// авто-применяются, ТРЕТЬЕ форсирует предложение (кумулятивно по диспетчам одной политики). На
/// [`ManualClock`] (без продвижения) рефилл НЕ срабатывает — проверяем чистую ёмкость.
#[tokio::test]
async fn token_bucket_boundary_capacity_then_propose() {
    let (_d, root, sink) = setup().await;
    let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault); // reject (для 3-го предложения).
                                                                // capacity=2, рефилл 1/окно, но часы НЕ двигаем → рефилла нет (чистая ёмкость).
    let clock = Arc::new(ManualClock::new());
    let bucket = TokenBucket::with_clock(2, 1, Duration::from_secs(60), clock);
    let pol = DispatchPolicy::with_bucket(Some("auto"), T, bucket);

    // Действие 1 и 2 — Auto, под ёмкостью ⇒ применяются.
    for (i, rel) in ["B1.md", "B2.md"].iter().enumerate() {
        let events = CollectingSink::new();
        let action = Action::note_create(*rel, "x");
        let out = dispatch_action(&action, (i + 1) as i64, &pol, &src, &events, &sink, &root)
            .await
            .unwrap();
        assert!(matches!(out, DispatchOutcome::Applied(_)), "{rel}: {out:?}");
        assert!(root.join(rel).exists(), "{rel} записан");
    }
    assert_eq!(
        pol.token_bucket.available(),
        0,
        "два токена потрачены — бакет пуст"
    );

    // Действие 3 — Auto, но бакет ПУСТ ⇒ предложение (PolicyDefault reject ⇒ не записано).
    let events = CollectingSink::new();
    let action = Action::note_create("B3.md", "x");
    let out = dispatch_action(&action, 3, &pol, &src, &events, &sink, &root)
        .await
        .unwrap();
    assert!(matches!(out, DispatchOutcome::Rejected(_)), "3-е: {out:?}");
    assert!(
        !root.join("B3.md").exists(),
        "3-е НЕ записано (бакет пуст → предложено)"
    );
    assert!(
        events
            .events()
            .iter()
            .any(|e| matches!(e, AgentEvent::Proposal { .. })),
        "3-е действие предложено"
    );
    assert_eq!(
        pol.token_bucket.available(),
        0,
        "предложение не тратит токен (бакет остался пуст)"
    );
}

/// auto-run + Confirm-тир (крупная перезапись) ⇒ ВСЁ РАВНО предлагает (auto НЕ перекрывает Confirm).
#[tokio::test]
async fn auto_run_confirm_tier_still_proposes() {
    let (_d, root, sink) = setup().await;
    write_existing(&root, "E.md", "orig");
    let events = CollectingSink::new();
    let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault); // reject all.
    let pol = policy(Some("auto")); // auto, но Confirm-тир НЕ должен авто-примениться.
    let big = "y".repeat(T + 1);
    let action = Action::note_edit("E.md", big);

    let out = dispatch_action(&action, 1, &pol, &src, &events, &sink, &root)
        .await
        .unwrap();
    // Предложено (Confirm) и отклонено PolicyDefault ⇒ файл НЕ перезаписан, токен НЕ потрачен.
    assert!(matches!(out, DispatchOutcome::Rejected(_)), "out={out:?}");
    assert_eq!(read(&root, "E.md"), "orig", "Confirm в auto НЕ применился");
    assert_eq!(
        pol.token_bucket.available(),
        CAP,
        "Confirm не клеймит токен (бакет полон)"
    );
    assert!(
        events
            .events()
            .iter()
            .any(|e| matches!(e, AgentEvent::Proposal { .. })),
        "Confirm-тир в auto — предложение"
    );
}

/// PolicyDefault: confirm-run под ним НИКОГДА не применяет Confirm-тир (fail-closed).
#[tokio::test]
async fn policy_default_never_applies_confirm() {
    let (_d, root, sink) = setup().await;
    write_existing(&root, "E.md", "orig");
    let events = CollectingSink::new();
    let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault);
    let big = "z".repeat(T + 1);
    let action = Action::note_edit("E.md", big);

    let out = dispatch_action(
        &action,
        1,
        &policy(Some("confirm")),
        &src,
        &events,
        &sink,
        &root,
    )
    .await
    .unwrap();
    assert!(matches!(out, DispatchOutcome::Rejected(_)));
    assert_eq!(
        read(&root, "E.md"),
        "orig",
        "PolicyDefault не применил Confirm"
    );
}

/// classify_hash threaded: дрейф МЕЖДУ propose и approve ⇒ apply отменяет Failed(drift), без клоббера.
#[tokio::test]
async fn drift_between_propose_and_approve_aborts() {
    let (_d, root, sink) = setup().await;
    write_existing(&root, "E.md", "orig-content");
    let events = CollectingSink::new();
    // Источник, который ПЕРЕД ответом Approve портит файл на диске (внешний писатель) — но решение
    // шлём через канал ПОСЛЕ ручной мутации. Здесь: засеваем Approve по id=1, а дрейф вносим
    // мутацией файла ДО диспетча? Нет — нужен дрейф ПОСЛЕ classify, ДО apply. Делаем кастомный
    // источник, который мутирует файл внутри decide(), затем одобряет.
    struct DriftThenApprove {
        root: PathBuf,
    }
    #[async_trait::async_trait]
    impl DecisionSource for DriftThenApprove {
        async fn decide(&self, batch: &ProposalBatch) -> BatchDecision {
            // Внешний писатель меняет файл МЕЖДУ classify (в dispatch) и apply (после approve).
            fs::write(self.root.join("E.md"), "EXTERNALLY-CHANGED").unwrap();
            BatchDecision::from_pairs([(batch.items[0].action_id, ItemDecision::Approve)])
        }
    }
    let src: Arc<dyn DecisionSource> = Arc::new(DriftThenApprove { root: root.clone() });
    // Малая правка ⇒ Auto-тир, но confirm-run ⇒ предложение (чтобы пройти propose→approve→apply).
    let action = Action::note_edit("E.md", "small new body");

    let out = dispatch_action(
        &action,
        1,
        &policy(Some("confirm")),
        &src,
        &events,
        &sink,
        &root,
    )
    .await
    .unwrap();
    // apply Рубеж 3: on-disk hash (EXTERNALLY-CHANGED) != classify_hash (orig-content) ⇒ Failed(drift).
    assert!(matches!(out, DispatchOutcome::Failed(_)), "out={out:?}");
    assert_eq!(
        read(&root, "E.md"),
        "EXTERNALLY-CHANGED",
        "наша правка НЕ затёрла внешнюю (анти-клоббер)"
    );
}

/// overwrite_threshold ИЗ КОНФИГА уважается: правка > threshold ⇒ Confirm (предложение), даже в auto.
#[tokio::test]
async fn config_overwrite_threshold_respected() {
    let (_d, root, sink) = setup().await;
    write_existing(&root, "E.md", "orig");
    let events = CollectingSink::new();
    let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault);
    // Порог из конфига = 10 байт; правка 11 байт ⇒ Confirm.
    let pol = DispatchPolicy::new(Some("auto"), 10, CAP);
    let action = Action::note_edit("E.md", "12345678901"); // 11 байт > 10.

    let out = dispatch_action(&action, 1, &pol, &src, &events, &sink, &root)
        .await
        .unwrap();
    assert!(
        matches!(out, DispatchOutcome::Rejected(_)),
        "Confirm из конфиг-порога"
    );
    assert!(events
        .events()
        .iter()
        .any(|e| matches!(e, AgentEvent::Proposal { .. })));

    // Та же правка под БОЛЬШИМ порогом (1000) ⇒ Auto ⇒ авто-применяется в auto-прогоне.
    let events2 = CollectingSink::new();
    let src2: Arc<dyn DecisionSource> = Arc::new(PolicyDefault);
    let pol2 = DispatchPolicy::new(Some("auto"), 1000, CAP);
    let out2 = dispatch_action(&action, 2, &pol2, &src2, &events2, &sink, &root)
        .await
        .unwrap();
    assert!(
        matches!(out2, DispatchOutcome::Applied(_)),
        "под порогом — Auto-apply"
    );
    assert_eq!(read(&root, "E.md"), "12345678901");
}

/// HardBlocked (escape) ⇒ ToolError при ЛЮБОЙ автономии; диск не тронут; нет предложения.
#[tokio::test]
async fn hardblocked_errors_any_autonomy() {
    let (_d, root, sink) = setup().await;
    let events = CollectingSink::new();
    let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault);
    let action = Action::note_create("../escape.md", "x");

    for autonomy in [Some("auto"), Some("confirm"), None] {
        let r = dispatch_action(&action, 1, &policy(autonomy), &src, &events, &sink, &root).await;
        assert!(
            matches!(r, Err(ToolError::Exec(_))),
            "autonomy={autonomy:?}"
        );
    }
    assert!(!root.join("../escape.md").exists());
    assert!(
        events.events().is_empty(),
        "HardBlocked не эмитит предложение"
    );
}

/// None автономии трактуется как confirm (безопаснее): Auto-тир предлагается, не авто-применяется.
#[tokio::test]
async fn none_autonomy_defaults_to_confirm() {
    let (_d, root, sink) = setup().await;
    let events = CollectingSink::new();
    let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault);
    let action = Action::note_create("N.md", "x");

    let out = dispatch_action(&action, 1, &policy(None), &src, &events, &sink, &root)
        .await
        .unwrap();
    assert!(
        matches!(out, DispatchOutcome::Rejected(_)),
        "None ⇒ confirm ⇒ предложение"
    );
    assert!(!root.join("N.md").exists());
}

/// Диф line-count: create (пусто → N строк) и edit (правка строк).
#[test]
fn line_diff_counts() {
    assert_eq!(line_diff("", "a\nb\nc"), (3, 0), "create — 3 add");
    assert_eq!(line_diff("a\nb\nc", ""), (0, 3), "очистка — 3 del");
    assert_eq!(line_diff("a\nb\nc", "a\nX\nc"), (1, 1), "1 строка изменена");
    assert_eq!(line_diff("same", "same"), (0, 0), "идентично — 0/0");
}

/// apply-строка после Approve реально executed (полный путь propose→approve→apply→ledger).
#[tokio::test]
async fn approved_apply_row_is_executed() {
    let (_d, root, sink) = setup().await;
    let events = CollectingSink::new();
    let (chan, tx) = ChannelDecision::new(1);
    tx.send(approve(1)).await.unwrap();
    let src: Arc<dyn DecisionSource> = Arc::new(chan);
    let action = Action::note_create("Ok.md", "done");

    dispatch_action(
        &action,
        1,
        &policy(Some("confirm")),
        &src,
        &events,
        &sink,
        &root,
    )
    .await
    .unwrap();
    assert_eq!(read(&root, "Ok.md"), "done");
    // apply-ключ (без propose-префикса): для create — target_hash = хеш планируемого тела (apply
    // fallback при None? нет — здесь classify_hash="" передан). Найдём executed-строку по run_id.
    let n_executed: i64 = sink_reader(&sink)
        .query(|c| {
            c.query_row(
                "SELECT count(*) FROM agent_actions WHERE run_id=1 AND state=?1",
                [STATE_EXECUTED],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(n_executed, 1, "ровно одна executed apply-строка");
}

// ── AGENT-5: KILL-SWITCH (чек-пойнт #3 — актуатор НЕ пишет под паузой) ─────────────────────────

/// Политика с ВЗВЕДЁННЫМ kill-switch (пауза) — auto-прогон, но писать нельзя.
fn paused_policy(autonomy: Option<&str>) -> DispatchPolicy {
    DispatchPolicy::with_paused(autonomy, T, CAP, Arc::new(AtomicBool::new(true)))
}

/// **KILL-SWITCH чек-пойнт #3 (auto-тир, auto-прогон, ПАУЗА)**: даже Auto-тир в auto-прогоне НЕ
/// авто-применяется под паузой — форс-предложение, и под PolicyDefault (auto-DENY) файл НЕ записан.
/// Токен НЕ потрачен (claim не зовётся под паузой). Доказывает: пауза блокирует авто-запись.
#[tokio::test]
async fn paused_auto_tier_does_not_apply() {
    let (_d, root, sink) = setup().await;
    let events = CollectingSink::new();
    let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault);
    let pol = paused_policy(Some("auto"));
    let action = Action::note_create("Paused.md", "x");

    let out = dispatch_action(&action, 1, &pol, &src, &events, &sink, &root)
        .await
        .unwrap();
    // Под паузой Auto уходит в propose; PolicyDefault reject ⇒ Rejected, файл НЕ записан.
    assert!(matches!(out, DispatchOutcome::Rejected(_)), "out={out:?}");
    assert!(
        !root.join("Paused.md").exists(),
        "под паузой файл НЕ записан"
    );
    assert_eq!(
        pol.token_bucket.available(),
        CAP,
        "под паузой claim не зовётся — токен НЕ потрачен"
    );
}

/// **KILL-SWITCH чек-пойнт #3 (ОДОБРЕНО, но ПАУЗА → НЕ записано)** — самый жёсткий тест: даже
/// DecisionSource, который ОДОБРЯЕТ (Approve), НЕ пробивает паузу в запись. Re-check паузы ПОСЛЕ
/// decide() ПЕРЕД apply ⇒ Rejected, файл НЕ записан, строка остаётся `proposed` (можно одобрить на
/// un-pause). Это гарантия «paused ⇒ нет записи» даже при approving-источнике.
#[tokio::test]
async fn paused_approved_proposal_still_not_written() {
    let (_d, root, sink) = setup().await;
    let events = CollectingSink::new();
    // Источник, который ОДОБРЯЕТ id=1 (строка proposed в пустой БД = 1).
    let (chan, tx) = ChannelDecision::new(1);
    tx.send(approve(1)).await.unwrap();
    let src: Arc<dyn DecisionSource> = Arc::new(chan);
    // confirm-прогон + ПАУЗА: идёт по propose-пути, источник одобряет — но пауза блокирует apply.
    let pol = paused_policy(Some("confirm"));
    let action = Action::note_create("ApprovedButPaused.md", "hi");

    let out = dispatch_action(&action, 1, &pol, &src, &events, &sink, &root)
        .await
        .unwrap();
    assert!(
        matches!(out, DispatchOutcome::Rejected(_)),
        "одобрено, но пауза ⇒ запись подавлена: {out:?}"
    );
    assert!(
        !root.join("ApprovedButPaused.md").exists(),
        "ОДОБРЕНО, но ПАУЗА → файл НЕ записан (kill-switch пробивает даже Approve)"
    );
    // Строка осталась `proposed` (НЕ approved/executed) — её можно одобрить снова на un-pause.
    let key = proposal_key(1, &action, "");
    let row = lookup(&sink_reader(&sink), &key).await.unwrap().unwrap();
    assert_eq!(
        row.state, STATE_PROPOSED,
        "под паузой строка не повышена до approved (остаётся proposed)"
    );
}

/// **KILL-SWITCH LAST-MOMENT GUARD (apply_now, TOCTOU-сужение)** — пауза флипается в `true` ПОСЛЕ
/// решения Approve, но ДО фактической записи в `apply_now`. Инъекция флипа: кастомный DecisionSource
/// одобряет и ставит в строй «отложенный флип», который проворачиваем перетиранием флага ПЕРЕД тем,
/// как `apply_now` доберётся до `apply_action`. Поскольку и approved-путь (:779), и `apply_now` читают
/// ОДИН Arc, мы доказываем сам guard `apply_now`, вызывая его НАПРЯМУЮ со взведённым флагом: запись
/// НЕ происходит (файл не создан), `apply_action` не зовётся (ledger executed-строки нет) → no-op
/// Rejected. Это и есть финальный страж окна между проверкой вызывателя и записью.
#[tokio::test]
async fn apply_now_late_pause_blocks_write() {
    let (_d, root, sink) = setup().await;
    // Флаг стартует НЕ на паузе (как если бы проверка вызывателя на :617/:779 уже прошла), затем
    // флипается в паузу В ОКНЕ перед записью — эмулируем это, взводя флаг ДО прямого вызова apply_now.
    let agent_paused = Arc::new(AtomicBool::new(false));
    // Симулируем «вызыватель проверил — было НЕ на паузе»: читаем флаг (false), затем флипаем.
    assert!(
        !agent_paused.load(Ordering::Relaxed),
        "до флипа: НЕ на паузе (как при проверке вызывателя)"
    );
    agent_paused.store(true, Ordering::Relaxed); // пауза взведена В ОКНЕ перед записью.

    let action = Action::note_create("LateP.md", "should-not-be-written");
    // apply_now — единственный применяющий путь; зовём напрямую (как из approved-ветки), classify_hash
    // = "" (create-конвенция). LAST-MOMENT guard читает флаг → Rejected, БЕЗ записи.
    let out = apply_now(&action, 1, &root, &sink, "", &agent_paused).await;

    assert!(
        matches!(out, DispatchOutcome::Rejected(_)),
        "пауза в последний момент ⇒ no-op Rejected: {out:?}"
    );
    assert!(
        !root.join("LateP.md").exists(),
        "LAST-MOMENT guard: файл НЕ записан (пауза взведена после проверки вызывателя, до записи)"
    );
    // apply_action не зван → НИ ОДНОЙ executed-строки ledger для этого прогона.
    let n_executed: i64 = sink_reader(&sink)
        .query(|c| {
            c.query_row(
                "SELECT count(*) FROM agent_actions WHERE run_id=1 AND state=?1",
                [STATE_EXECUTED],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(
        n_executed, 0,
        "LAST-MOMENT guard: apply_action не зван — ledger executed-строки нет"
    );
}

/// Контр-проверка: тот же путь apply_now БЕЗ паузы реально ПИШЕТ (guard не ложно-срабатывает) —
/// доказывает, что блокировка выше обусловлена именно паузой, а не сломанным apply_now.
#[tokio::test]
async fn apply_now_not_paused_writes() {
    let (_d, root, sink) = setup().await;
    let agent_paused = Arc::new(AtomicBool::new(false)); // НЕ на паузе.
    let action = Action::note_create("LiveP.md", "written");
    let out = apply_now(&action, 1, &root, &sink, "", &agent_paused).await;
    assert!(matches!(out, DispatchOutcome::Applied(_)), "out={out:?}");
    assert_eq!(
        read(&root, "LiveP.md"),
        "written",
        "без паузы apply_now пишет"
    );
}

/// **End-to-end флип ПОСЛЕ решения через dispatch_action**: DecisionSource одобряет и ВНУТРИ decide()
/// взводит ОБЩИЙ с политикой `agent_paused`. Это эмулирует паузу, взведённую в окне принятия решения /
/// перед записью. Оба стража (re-check :779 И last-moment apply_now) читают этот Arc ⇒ запись
/// подавлена: файл НЕ создан, строка остаётся proposed. Дополняет прямой unit-тест guard'а сверху
/// полным путём dispatch→propose→approve→(пауза)→НЕ-запись.
#[tokio::test]
async fn dispatch_pause_flip_during_decide_blocks_write() {
    let (_d, root, sink) = setup().await;
    let events = CollectingSink::new();
    let agent_paused = Arc::new(AtomicBool::new(false));

    // Источник: одобряет id=1 И взводит общий флаг паузы ВНУТРИ decide() (флип после предложения).
    struct ApproveThenPause {
        flag: Arc<AtomicBool>,
    }
    #[async_trait::async_trait]
    impl DecisionSource for ApproveThenPause {
        async fn decide(&self, batch: &ProposalBatch) -> BatchDecision {
            self.flag.store(true, Ordering::Relaxed); // пауза взведена в окне решения.
            BatchDecision::from_pairs([(batch.items[0].action_id, ItemDecision::Approve)])
        }
    }
    let src: Arc<dyn DecisionSource> = Arc::new(ApproveThenPause {
        flag: agent_paused.clone(),
    });
    // confirm-прогон с ОБЩИМ флагом паузы (стартует НЕ на паузе): идёт propose→decide→(флип)→страж.
    let pol = DispatchPolicy::with_paused(Some("confirm"), T, CAP, agent_paused.clone());
    let action = Action::note_create("FlipDuringDecide.md", "x");

    let out = dispatch_action(&action, 1, &pol, &src, &events, &sink, &root)
        .await
        .unwrap();
    assert!(
        matches!(out, DispatchOutcome::Rejected(_)),
        "пауза взведена в окне решения ⇒ запись подавлена: {out:?}"
    );
    assert!(
        !root.join("FlipDuringDecide.md").exists(),
        "флип после решения ⇒ файл НЕ записан"
    );
    // Строка остаётся proposed (re-check :779 перехватил до transition) — можно одобрить на un-pause.
    let key = proposal_key(1, &action, "");
    let row = lookup(&sink_reader(&sink), &key).await.unwrap().unwrap();
    assert_eq!(
        row.state, STATE_PROPOSED,
        "флип в окне решения: строка осталась proposed"
    );
}

// ── AGENT-5: токен-бакет (анти-усталость, claim-before-apply, рефилл, конкурентность) ──────────

/// **НЕ-Applied НЕ тратит токен (рефанд через dispatch).** auto-прогон, Auto-тир, но apply ПАДАЕТ
/// (drift: classify_hash≠on-disk) ⇒ Failed. Токен заклеймлен ДО apply, но Failed ⇒ РЕФАНД: бакет
/// остаётся ПОЛНЫМ. Доказывает, что «потрачен только реально применённый Auto».
#[tokio::test]
async fn non_applied_outcome_refunds_token() {
    let (_d, root, sink) = setup().await;
    write_existing(&root, "E.md", "orig-content");
    let events = CollectingSink::new();
    // Источник не спрашивается (Auto-тир в auto-прогоне). Дрейф вносим внешним писателем ВНУТРИ
    // несуществующего decide? Нет — Auto-тир НЕ предлагает. Дрейф провоцируем иначе: классифай
    // прочитает "orig-content", а apply Рубеж 3 сверит хэш — совпадёт. Чтобы получить Failed без
    // решения, делаем edit НЕсуществующего файла → apply Failed(не существует). Это НЕ-Applied.
    let pol = policy(Some("auto")); // capacity=CAP, полный.
    assert_eq!(pol.token_bucket.available(), CAP, "бакет стартует полным");
    // note.edit по ОТСУТСТВУЮЩЕМУ файлу: Auto-тир (малый размер) → claim → apply Failed (нет файла).
    let action = Action::note_edit("Missing.md", "small");
    let out = dispatch_action(&action, 1, &pol, &src_reject(), &events, &sink, &root)
        .await
        .unwrap();
    assert!(
        matches!(out, DispatchOutcome::Failed(_)),
        "edit отсутствующего → Failed: {out:?}"
    );
    assert_eq!(
        pol.token_bucket.available(),
        CAP,
        "НЕ-Applied (Failed) → токен возвращён (рефанд): бакет полон"
    );
}

/// PolicyDefault как `Arc<dyn DecisionSource>` (reject-all) — хелпер для тестов выше.
fn src_reject() -> Arc<dyn DecisionSource> {
    Arc::new(PolicyDefault)
}

/// Чистая единица: N claim'ов на ёмкости N успешны, (N+1)-й — нет (бакет пуст). Без apply/БД.
#[test]
fn token_bucket_capacity_n_then_empty() {
    let clock = Arc::new(ManualClock::new());
    let b = TokenBucket::with_clock(3, 1, Duration::from_secs(60), clock);
    assert_eq!(b.available(), 3, "стартует полным");
    assert!(b.try_claim(), "claim 1");
    assert!(b.try_claim(), "claim 2");
    assert!(b.try_claim(), "claim 3");
    assert!(!b.try_claim(), "claim 4 — бакет пуст");
    assert_eq!(b.available(), 0);
}

/// Чистая единица: после РЕФИЛЛ-окна (продвижение ManualClock) ёмкость восстанавливается — но НЕ
/// выше capacity. Рефилл по времени детерминирован (ручные часы, без sleep/Instant::now()).
#[test]
fn token_bucket_refills_after_window() {
    let clock = Arc::new(ManualClock::new());
    // capacity=2, рефилл 1 токен за 10 с.
    let b = TokenBucket::with_clock(2, 1, Duration::from_secs(10), clock.clone());
    assert!(b.try_claim() && b.try_claim(), "опустошаем бакет");
    assert!(!b.try_claim(), "пуст");

    // Прошло одно окно (10 с) → доначислен 1 токен.
    clock.advance(Duration::from_secs(10));
    assert_eq!(b.available(), 1, "одно окно → +1 токен");
    assert!(b.try_claim(), "claim восстановленного токена");
    assert!(!b.try_claim(), "снова пуст");

    // Прошло ТРИ окна сразу (30 с) → доначислено 3, но потолок capacity=2.
    clock.advance(Duration::from_secs(30));
    assert_eq!(
        b.available(),
        2,
        "много окон → не выше capacity (потолок 2)"
    );
}

/// Чистая единица: ДРОБНОЕ окно (меньше refill_per) НЕ доначисляет токен, а остаток времени НЕ
/// теряется — накопившись до полного окна, токен доначисляется. (`last_refill` продвигается на
/// целые окна, не на текущий момент.)
#[test]
fn token_bucket_partial_window_does_not_credit_but_accumulates() {
    let clock = Arc::new(ManualClock::new());
    let b = TokenBucket::with_clock(1, 1, Duration::from_secs(10), clock.clone());
    assert!(b.try_claim(), "опустошаем (capacity=1)");
    assert!(!b.try_claim(), "пуст");

    // 6 с < 10 с → НЕ доначисляет.
    clock.advance(Duration::from_secs(6));
    assert_eq!(b.available(), 0, "дробное окно (6с) не доначисляет");
    // ещё 6 с → суммарно 12 с ≥ одно окно (10 с) → +1 токен (остаток 2 с не потерян).
    clock.advance(Duration::from_secs(6));
    assert_eq!(b.available(), 1, "накоплено полное окно → +1 токен");
}

/// Чистая единица: refund НЕ превышает capacity (потолок). Рефанд без предшествующего claim не
/// «раздувает» бакет сверх ёмкости.
#[test]
fn token_bucket_refund_capped_at_capacity() {
    let clock = Arc::new(ManualClock::new());
    let b = TokenBucket::with_clock(2, 0, Duration::ZERO, clock); // без рефилла по времени.
    assert_eq!(b.available(), 2, "полон");
    b.refund(); // уже полон → потолок не превышен.
    assert_eq!(
        b.available(),
        2,
        "refund на полном бакете не превышает capacity"
    );
    assert!(b.try_claim());
    b.refund();
    assert_eq!(b.available(), 2, "claim+refund ⇒ обратно полон, не выше");
}

/// **CONCURRENCY-SAFETY (ключевой тест AGENT-5)**: МНОГО потоков конкурентно зовут `try_claim()` на
/// бакете ёмкости N — суммарно успешных claim'ов РОВНО N (НЕ больше). Доказывает, что
/// compare_exchange сериализует декремент: гонка check-then-act 3d (два диспетча оба видят
/// `count<cap` и оба применяют) НЕВОЗМОЖНА. Без рефилла (часы не двигаем) — чистая ёмкость.
#[test]
fn concurrent_claims_never_exceed_capacity() {
    use std::sync::atomic::AtomicU32 as A32;
    const CAPACITY: u32 = 50;
    const THREADS: usize = 16;
    const PER_THREAD: usize = 20; // 16*20 = 320 попыток на 50 токенов.
    let clock = Arc::new(ManualClock::new()); // не двигаем → рефилла нет.
    let bucket = TokenBucket::with_clock(CAPACITY, 1, Duration::from_secs(60), clock);
    let claimed = Arc::new(A32::new(0));

    std::thread::scope(|s| {
        for _ in 0..THREADS {
            let bucket = bucket.clone();
            let claimed = claimed.clone();
            s.spawn(move || {
                for _ in 0..PER_THREAD {
                    if bucket.try_claim() {
                        claimed.fetch_add(1, Ordering::SeqCst);
                    }
                }
            });
        }
    });

    assert_eq!(
        claimed.load(Ordering::SeqCst),
        CAPACITY,
        "конкурентные claim'ы НЕ превышают ёмкость (РОВНО {CAPACITY})"
    );
    assert_eq!(bucket.available(), 0, "бакет пуст после ровно N claim'ов");
}

/// **CONCURRENCY-SAFETY рефилла**: конкурентные claim'ы ПОСЛЕ рефилл-окна не доначисляют дважды —
/// `last_refill` продвигается CAS'ом (одно окно учитывается ровно раз). Опустошаем, продвигаем
/// часы на 1 окно (+capacity токенов суммарно но ≤ capacity), затем конкурентно клеймим: суммарно
/// успешных ≤ capacity (не capacity*threads из-за двойного начисления).
#[test]
fn concurrent_refill_no_double_credit() {
    use std::sync::atomic::AtomicU32 as A32;
    const CAPACITY: u32 = 8;
    let clock = Arc::new(ManualClock::new());
    // Рефилл сразу ВСЕЙ ёмкости за одно окно (10 с) — чтобы после опустошения одно окно вернуло
    // весь бакет; проверяем, что конкурентные claim'ы не «увидят» это окно несколько раз.
    let bucket =
        TokenBucket::with_clock(CAPACITY, CAPACITY, Duration::from_secs(10), clock.clone());
    // Опустошаем.
    for _ in 0..CAPACITY {
        assert!(bucket.try_claim());
    }
    assert!(!bucket.try_claim(), "пуст");
    // Одно окно прошло → доначислится CAPACITY (но не выше потолка).
    clock.advance(Duration::from_secs(10));

    let claimed = Arc::new(A32::new(0));
    std::thread::scope(|s| {
        for _ in 0..16 {
            let bucket = bucket.clone();
            let claimed = claimed.clone();
            s.spawn(move || {
                for _ in 0..10 {
                    if bucket.try_claim() {
                        claimed.fetch_add(1, Ordering::SeqCst);
                    }
                }
            });
        }
    });
    assert_eq!(
        claimed.load(Ordering::SeqCst),
        CAPACITY,
        "одно рефилл-окно вернуло РОВНО capacity — нет двойного начисления при гонке"
    );
}

// Доступ к reader sink'а для проверок ledger в тестах (зеркало apply.rs).
fn sink_reader(sink: &AuditSink) -> crate::db::ReadPool {
    sink.reader_handle()
}

// ── SL-7c: dispatch_skill_save (skills_root-confined гейт + apply) ──────────────────────────
const VALID_SKILL: &str = "---\nname: myskill\ndescription: d\n---\nBODY";

/// learning_enabled=false (дефолт) ⇒ classify HardBlocked(LearningDisabled) ⇒ Err, файл НЕ записан.
#[tokio::test]
async fn skill_save_learning_disabled_is_blocked() {
    let (_d, root, sink) = setup().await;
    let events = CollectingSink::new();
    let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault);
    let action = Action::skill_save("myskill/SKILL.md", VALID_SKILL);
    // policy без skills-флагов (learning false, root false) → HardBlocked.
    let out = dispatch_skill_save(
        &action,
        1,
        &policy(Some("confirm")),
        &src,
        &events,
        &sink,
        &root,
    )
    .await;
    assert!(
        out.is_err(),
        "learning off → Err (HardBlocked), получено {out:?}"
    );
    assert!(!root.join("myskill").exists(), "файл навыка НЕ записан");
}

/// learning ON + root ON + Approve ⇒ навык записан под skills_root; DispatchOutcome::Applied.
#[tokio::test]
async fn skill_save_approve_writes() {
    let (_d, root, sink) = setup().await;
    let events = CollectingSink::new();
    let (chan, tx) = ChannelDecision::new(1);
    tx.send(approve(1)).await.unwrap();
    let src: Arc<dyn DecisionSource> = Arc::new(chan);
    let pol = policy(Some("confirm")).with_skills_flags(true, true);
    let action = Action::skill_save("myskill/SKILL.md", VALID_SKILL);

    let (out, _real) = dispatch_skill_save(&action, 1, &pol, &src, &events, &sink, &root)
        .await
        .unwrap();
    assert!(matches!(out, DispatchOutcome::Applied(_)), "out={out:?}");
    assert_eq!(
        read(&root, "myskill/SKILL.md"),
        VALID_SKILL,
        "навык записан под skills_root после Approve"
    );
    // Эмитирован Proposal (поверхность апрува).
    assert!(
        events
            .events()
            .iter()
            .any(|e| matches!(e, AgentEvent::Proposal { .. })),
        "Proposal эмитирован"
    );
}

/// learning ON + root ON + Reject (PolicyDefault) ⇒ Rejected, файл НЕ записан.
#[tokio::test]
async fn skill_save_reject_no_write() {
    let (_d, root, sink) = setup().await;
    let events = CollectingSink::new();
    let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault);
    let pol = policy(Some("confirm")).with_skills_flags(true, true);
    let action = Action::skill_save("myskill/SKILL.md", VALID_SKILL);

    let (out, _real) = dispatch_skill_save(&action, 1, &pol, &src, &events, &sink, &root)
        .await
        .unwrap();
    assert!(matches!(out, DispatchOutcome::Rejected(_)), "out={out:?}");
    assert!(
        !root.join("myskill").exists(),
        "отклонённый навык НЕ записан"
    );
}

/// vendor/-неймспейс ⇒ HardBlocked(InvalidSkillTarget) ⇒ Err даже при learning ON (keystone).
#[tokio::test]
async fn skill_save_vendor_blocked_even_when_enabled() {
    let (_d, root, sink) = setup().await;
    let events = CollectingSink::new();
    let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault);
    let pol = policy(Some("confirm")).with_skills_flags(true, true);
    let action = Action::skill_save("vendor/kepano/x/SKILL.md", VALID_SKILL);
    let out = dispatch_skill_save(&action, 1, &pol, &src, &events, &sink, &root).await;
    assert!(out.is_err(), "vendor → Err (InvalidSkillTarget): {out:?}");
    assert!(!root.join("vendor").exists(), "vendor-навык НЕ записан");
}

/// KILL-SWITCH (ревью SL-7c MAJOR): learning ON + Approve, но агент НА ПАУЗЕ ⇒ навык НЕ записан
/// (Rejected); re-check паузы ПОСЛЕ decide и ПЕРЕД transition/apply держит «paused ⇒ нет записи».
#[tokio::test]
async fn skill_save_paused_approved_not_written() {
    let (_d, root, sink) = setup().await;
    let events = CollectingSink::new();
    let (chan, tx) = ChannelDecision::new(1);
    tx.send(approve(1)).await.unwrap();
    let src: Arc<dyn DecisionSource> = Arc::new(chan);
    let paused = Arc::new(AtomicBool::new(true));
    let pol =
        DispatchPolicy::with_paused(Some("confirm"), T, CAP, paused).with_skills_flags(true, true);
    let action = Action::skill_save("myskill/SKILL.md", VALID_SKILL);

    let (out, _real) = dispatch_skill_save(&action, 1, &pol, &src, &events, &sink, &root)
        .await
        .unwrap();
    assert!(
        matches!(out, DispatchOutcome::Rejected(_)),
        "пауза → Rejected: {out:?}"
    );
    assert!(
        !root.join("myskill").exists(),
        "под паузой (kill-switch) навык НЕ записан"
    );
}
