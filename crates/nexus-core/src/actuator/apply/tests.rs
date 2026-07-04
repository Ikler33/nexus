use super::*;
use crate::db::Database;
use crate::vault::history::{list_snapshots, read_snapshot};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// Открывает временный vault + БД; возвращает (dir, canon_root, AuditSink). canon_root
/// КАНОНИЗИРОВАН (предусловие resolve_vault_path_for_write — на macOS /tmp → /private/tmp).
async fn setup() -> (TempDir, PathBuf, AuditSink) {
    let dir = TempDir::new().unwrap();
    let canon_root = dir.path().canonicalize().unwrap();
    let db = Database::open(canon_root.join(".nexus/nexus.db"))
        .await
        .unwrap();
    let sink = AuditSink::new(db.writer().clone(), db.reader().clone());
    // db дропнется в конце функции — но writer/reader клонированы в sink и переживут.
    // Держим db живой через утечку хэндлов: возвращаем sink (клон), db дропается, но актор
    // живёт пока жив хотя бы один клон writer. Для теста этого достаточно.
    std::mem::forget(db);
    (dir, canon_root, sink)
}

fn abs_of(root: &Path, rel: &str) -> PathBuf {
    root.join(rel)
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(abs_of(root, rel)).unwrap()
}

fn write_existing(root: &Path, rel: &str, content: &str) {
    let abs = abs_of(root, rel);
    if let Some(p) = abs.parent() {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(abs, content).unwrap();
}

/// Прочитать строку ledger по ключу (через reader sink'а напрямую — для проверок состояния).
async fn ledger_row(sink: &AuditSink, key: &str) -> Option<super::super::audit::ActionRow> {
    super::super::audit::lookup(&sink.reader, key)
        .await
        .unwrap()
}

/// Восстановить ключ так же, как его построит apply (для проверки строки ledger).
fn key_for(run_id: i64, action: &Action, on_disk: Option<&str>, planned: Option<&str>) -> String {
    let rel = action.target.rel();
    let payload = action_payload(action);
    let target_hash = on_disk
        .map(|c| crate::vault::content_hash(c.as_bytes()))
        .unwrap_or_else(|| crate::vault::content_hash(planned.unwrap_or("").as_bytes()));
    let args = canonical_args(Some(rel), payload);
    idempotency_key(run_id, action.target.tool_name(), &args, &target_hash)
}

/// CREATE: пишет новую заметку; ledger Executed; undo=Trash; re-create-over-existing fails-closed.
#[tokio::test]
async fn create_writes_note_ledger_executed_undo_trash() {
    let (_d, root, sink) = setup().await;
    let action = Action::note_create("Notes/New.md", "hello body");

    let outcome = apply_action(&action, 1, &root, &sink, None).await;
    match &outcome {
        ApplyOutcome::Executed { undo, .. } => {
            assert_eq!(
                *undo,
                UndoHandle::Trash {
                    trash_rel: "Notes/New.md".to_string()
                },
                "undo create = Trash"
            );
        }
        other => panic!("ожидался Executed, получено {other:?}"),
    }
    assert_eq!(read(&root, "Notes/New.md"), "hello body");

    // ledger: строка Executed с undo=trash.
    let key = key_for(1, &action, None, Some("hello body"));
    let row = ledger_row(&sink, &key).await.expect("ledger-строка есть");
    assert_eq!(row.state, "executed");
    assert!(row.outcome.is_some(), "outcome зафиксирован");
    assert_eq!(row.undo_kind.as_deref(), Some("trash"));

    // re-create поверх существующего → fails-closed (диск не тронут). run_id другой, чтобы не
    // упереться в идемпотентность того же ключа.
    let again = apply_action(&action, 2, &root, &sink, None).await;
    assert!(
        matches!(again, ApplyOutcome::Failed(_)),
        "create поверх существующего fails-closed, было {again:?}"
    );
    assert_eq!(
        read(&root, "Notes/New.md"),
        "hello body",
        "файл не перезаписан"
    );
}

/// EDIT: снапшот ПРЕД-edit контента (manual=true), перезапись; ledger Executed + undo=Snapshot{ts}.
#[tokio::test]
async fn edit_snapshots_pre_content_and_overwrites() {
    let (_d, root, sink) = setup().await;
    write_existing(&root, "Notes/E.md", "ORIGINAL");
    let action = Action::note_edit("Notes/E.md", "EDITED");

    let outcome = apply_action(&action, 1, &root, &sink, None).await;
    let undo = match &outcome {
        ApplyOutcome::Executed { undo, .. } => undo.clone(),
        other => panic!("ожидался Executed, получено {other:?}"),
    };
    // Содержимое перезаписано.
    assert_eq!(read(&root, "Notes/E.md"), "EDITED");
    // undo = Snapshot{ts}; снапшот держит ПРЕД-edit контент.
    let ts = match undo {
        UndoHandle::Snapshot { ts, rel } => {
            assert_eq!(rel, "Notes/E.md");
            ts as u64
        }
        other => panic!("ожидался Snapshot, получено {other:?}"),
    };
    assert_eq!(
        read_snapshot(&root, "Notes/E.md", ts).unwrap(),
        "ORIGINAL",
        "снапшот держит ПРЕД-edit содержимое (обратимость)"
    );

    let key = key_for(1, &action, Some("ORIGINAL"), None);
    let row = ledger_row(&sink, &key).await.expect("ledger-строка");
    assert_eq!(row.state, "executed");
    assert_eq!(row.undo_kind.as_deref(), Some("snapshot"));
    assert_eq!(row.undo_ref.as_deref(), Some(ts.to_string().as_str()));
}

/// AGENT-6 MANDATORY (приватность): применяем NoteEdit с УЗНАВАЕМЫМ СЕКРЕТОМ в содержимом и
/// УБЕЖДАЕМСЯ, что НИ ОДНА TEXT-колонка долговечной строки `agent_actions` не несёт этот секрет.
/// Проверяем КАЖДУЮ текстовую колонку схемы (migration 022) сырым SELECT'ом — содержимое заметки
/// не должно попасть в аудит-БД ни через diff_summary, ни через outcome, ни через любую иную колонку.
/// diff_summary при этом == структурная форма "+N -M (kind)" (счётчики, не содержимое).
#[tokio::test]
async fn secret_content_never_lands_in_durable_ledger() {
    const SECRET: &str = "SECRET-TOKEN-123";
    let (_d, root, sink) = setup().await;
    // Заметка существует; правим её, ВСТАВЛЯЯ секрет в новое содержимое (тело правки).
    write_existing(&root, "Notes/S.md", "old line one\nold line two\n");
    let new_content = format!("line A\n{SECRET}\nline B\n");
    let action = Action::note_edit("Notes/S.md", &new_content);

    let outcome = apply_action(&action, 1, &root, &sink, None).await;
    assert!(
        matches!(outcome, ApplyOutcome::Executed { .. }),
        "правка применена, было {outcome:?}"
    );
    // Файл реально содержит секрет (доказывает, что секрет — настоящий контент действия).
    assert!(
        read(&root, "Notes/S.md").contains(SECRET),
        "секрет действительно записан в заметку (контроль теста)"
    );

    // Сырой SELECT ВСЕХ TEXT-колонок строки (схема 022) — проверяем каждую на отсутствие секрета.
    let cols: Vec<(String, Option<String>)> = sink
        .reader
        .query(|c| {
            c.query_row(
                "SELECT idempotency_key, tool_name, target_rel, risk_tier, state, content_hash, \
                 undo_kind, undo_ref, outcome, diff_summary FROM agent_actions WHERE state='executed'",
                [],
                |r| {
                    Ok(vec![
                        ("idempotency_key".to_string(), r.get::<_, Option<String>>(0)?),
                        ("tool_name".to_string(), r.get::<_, Option<String>>(1)?),
                        ("target_rel".to_string(), r.get::<_, Option<String>>(2)?),
                        ("risk_tier".to_string(), r.get::<_, Option<String>>(3)?),
                        ("state".to_string(), r.get::<_, Option<String>>(4)?),
                        ("content_hash".to_string(), r.get::<_, Option<String>>(5)?),
                        ("undo_kind".to_string(), r.get::<_, Option<String>>(6)?),
                        ("undo_ref".to_string(), r.get::<_, Option<String>>(7)?),
                        ("outcome".to_string(), r.get::<_, Option<String>>(8)?),
                        ("diff_summary".to_string(), r.get::<_, Option<String>>(9)?),
                    ])
                },
            )
        })
        .await
        .unwrap();

    for (name, val) in &cols {
        if let Some(v) = val {
            assert!(
                !v.contains(SECRET),
                "СЕКРЕТ ПРОТЁК в долговечную колонку `{name}` = {v:?}"
            );
        }
    }

    // diff_summary — именно структурная форма "+N -M (kind)". Правка edit (2 строки → 3 строки):
    // редакция-гвард рендерит счётчики, не содержимое.
    let diff = cols
        .iter()
        .find(|(n, _)| n == "diff_summary")
        .and_then(|(_, v)| v.clone())
        .expect("diff_summary заполнен");
    assert!(
        diff.starts_with('+') && diff.contains(" -") && diff.ends_with("(edit)"),
        "diff_summary — структурная форма +N -M (edit), получено {diff:?}"
    );
    // И не содержит ни одной буквы из секрета-как-текста (только цифры/+-()/{new,edit}).
    assert!(
        diff.chars()
            .all(|c| c.is_ascii_digit() || " +-()newdit".contains(c)),
        "diff_summary структурно чист: {diff:?}"
    );
}

/// KEYSTONE throttle-bypass: две быстрые правки подряд ОБЕ оставляют снапшот (manual=true бьёт
/// 90с-троттл). С manual=false второй снапшот схлопнулся бы — тест бы упал.
#[tokio::test]
async fn rapid_edits_both_snapshot_throttle_bypass() {
    let (_d, root, sink) = setup().await;
    write_existing(&root, "R.md", "v0");

    // Правка 1: v0 → v1 (run 1).
    let a1 = Action::note_edit("R.md", "v1");
    assert!(matches!(
        apply_action(&a1, 1, &root, &sink, None).await,
        ApplyOutcome::Executed { .. }
    ));
    // Правка 2 СРАЗУ ЖЕ: v1 → v2 (run 2). В пределах 90с — авто-троттл бы пропустил снапшот v1.
    let a2 = Action::note_edit("R.md", "v2");
    assert!(matches!(
        apply_action(&a2, 2, &root, &sink, None).await,
        ApplyOutcome::Executed { .. }
    ));

    let snaps = list_snapshots(&root, "R.md").unwrap();
    assert_eq!(
        snaps.len(),
        2,
        "manual=true: обе быстрые правки оставили снапшот (троттл байпас); с manual=false было бы 1"
    );
    // Снапшоты держат ПРЕД-правочные контенты: v0 (перед 1-й) и v1 (перед 2-й).
    let mut contents: Vec<String> = snaps
        .iter()
        .map(|s| read_snapshot(&root, "R.md", s.ts).unwrap())
        .collect();
    contents.sort();
    assert_eq!(contents, vec!["v0".to_string(), "v1".to_string()]);
    assert_eq!(
        read(&root, "R.md"),
        "v2",
        "финальное содержимое — последняя правка"
    );
}

/// FRONTMATTER: через set_frontmatter_field; снапшот-перед; ledger Executed + undo=Snapshot.
#[tokio::test]
async fn set_frontmatter_writes_and_snapshots() {
    let (_d, root, sink) = setup().await;
    write_existing(&root, "F.md", "---\ntitle: T\n---\n\nbody\n");
    let action = Action::frontmatter("F.md", "status", "done");

    let outcome = apply_action(&action, 1, &root, &sink, None).await;
    let undo = match &outcome {
        ApplyOutcome::Executed { undo, .. } => undo.clone(),
        other => panic!("ожидался Executed, получено {other:?}"),
    };
    let new = read(&root, "F.md");
    assert!(new.contains("status: done"), "ключ записан: {new:?}");
    assert!(new.contains("title: T"), "остальной YAML сохранён");

    // Снапшот держит ПРЕД-правочный контент.
    let ts = match undo {
        UndoHandle::Snapshot { ts, .. } => ts as u64,
        other => panic!("ожидался Snapshot, получено {other:?}"),
    };
    assert_eq!(
        read_snapshot(&root, "F.md", ts).unwrap(),
        "---\ntitle: T\n---\n\nbody\n"
    );
}

/// SYMLINK RAMPART (критический security-тест): симлинк ВНУТРИ vault наружу. classify (лексический)
/// видит Auto, НО resolve_vault_path_for_write резолвит родителя→цель НАРУЖИ → PathEscape, внешний
/// файл НЕ тронут. Доказывает двух-рубежную защиту (lexical classify слеп к симлинку).
#[cfg(unix)]
#[tokio::test]
async fn symlink_rampart_blocks_write_outside_vault() {
    use std::os::unix::fs::symlink;

    let (_d, root, sink) = setup().await;
    // Внешняя цель ВНЕ vault, с известным содержимым.
    let outside_dir = TempDir::new().unwrap();
    let outside_target = outside_dir.path().canonicalize().unwrap().join("secret.md");
    fs::write(&outside_target, "OUTSIDE-UNTOUCHED").unwrap();

    // Симлинк ВНУТРИ vault: vault/evil.md -> /…/outside/secret.md.
    symlink(&outside_target, abs_of(&root, "evil.md")).unwrap();

    // Лексический classify видит "evil.md" как обычное имя в корне → Auto (слеп к симлинку).
    let action = Action::note_edit("evil.md", "PWNED");
    let ctx = super::super::classify::ClassifyCtx {
        root: &root,
        overwrite_threshold: 64 * 1024,
        shell_enable: false,
        sandbox_available: false,
        learning_enabled: false,
        skills_root_configured: false,
    };
    assert_eq!(
        super::super::classify::classify(&action, &ctx),
        super::super::classify::RiskTier::Auto,
        "classify лексически слеп к симлинку — говорит Auto"
    );

    // НО apply: рубеж 1 видит leaf-симлинк (symlink_metadata) → PathEscape, НИ ОДНОГО write/чтения
    // по симлинку. (Родитель evil.md — vault-корень, ВНУТРИ; resolve канонизирует только родителя,
    // поэтому leaf-симлинк ловит ДОПОЛНИТЕЛЬНАЯ symlink_metadata-проверка рубежа 1.)
    let outcome = apply_action(&action, 1, &root, &sink, None).await;

    // Главный инвариант: внешний файл вне vault НЕ изменён.
    assert_eq!(
        fs::read_to_string(&outside_target).unwrap(),
        "OUTSIDE-UNTOUCHED",
        "симлинк-побег: внешний файл вне vault НЕ должен быть перезаписан"
    );
    // Сильное утверждение: apply отверг как PathEscape (рубеж, а не побочный эффект rename).
    assert_eq!(
        outcome,
        ApplyOutcome::PathEscape,
        "leaf-симлинк наружу → PathEscape (двух-рубежная защита), получено {outcome:?}"
    );
    // И симлинк НЕ заменён реальным файлом — мы вообще не писали.
    assert!(
        fs::symlink_metadata(abs_of(&root, "evil.md"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "симлинк не тронут (write не выполнялся)"
    );
}

/// SYMLINK-КАТАЛОГ наружу: `vault/dirlink -> /outside`, запись `vault/dirlink/x.md`. Здесь побег
/// ловит САМ resolve_vault_path_for_write (канонизирует РОДИТЕЛЯ `dirlink` → наружу → не starts_with
/// root). Дополняет leaf-кейс выше — обе формы симлинк-побега → PathEscape, внешний каталог нетронут.
#[cfg(unix)]
#[tokio::test]
async fn symlink_dir_rampart_blocks_write_outside_vault() {
    use std::os::unix::fs::symlink;
    let (_d, root, sink) = setup().await;
    let outside_dir = TempDir::new().unwrap();
    let outside = outside_dir.path().canonicalize().unwrap();
    // Симлинк-КАТАЛОГ внутри vault → внешний каталог.
    symlink(&outside, abs_of(&root, "dirlink")).unwrap();

    // create в symlinked-каталог: classify лексически Auto (видит "dirlink/x.md" как обычный путь).
    let action = Action::note_create("dirlink/x.md", "PWNED");
    let outcome = apply_action(&action, 1, &root, &sink, None).await;
    assert_eq!(
        outcome,
        ApplyOutcome::PathEscape,
        "симлинк-каталог наружу → PathEscape, получено {outcome:?}"
    );
    assert!(
        !outside.join("x.md").exists(),
        "файл НЕ создан во внешнем каталоге"
    );
}

/// LEDGER write-before-act: строка (state=Executing) существует ДО/НЕЗАВИСИМО от файлового write.
/// Проверяем через ручной record_before + проверку строки до finish.
#[tokio::test]
async fn ledger_row_exists_before_act() {
    let (_d, root, sink) = setup().await;
    write_existing(&root, "L.md", "before");
    let action = Action::note_edit("L.md", "after");

    // Ручной write-before-act (как сделает apply шагом 4), затем проверяем строку ДО любого write.
    let key = key_for(1, &action, Some("before"), None);
    let entry = ActionEntry {
        run_id: 1,
        idempotency_key: key.clone(),
        tool_name: action.target.tool_name().to_string(),
        target_rel: Some("L.md".to_string()),
        risk_tier: super::super::audit::TIER_AUTO.to_string(),
        state: STATE_EXECUTING.to_string(),
        content_hash: Some(crate::vault::content_hash("before".as_bytes())),
        diff_summary: None,
    };
    sink.record_before(entry).await.unwrap();

    // Строка есть, state=executing, outcome=NULL — ДО любого write. Файл всё ещё исходный.
    let row = ledger_row(&sink, &key).await.expect("строка-якорь");
    assert_eq!(row.state, "executing");
    assert!(row.outcome.is_none(), "outcome NULL до finish");
    assert_eq!(read(&root, "L.md"), "before", "файл ещё не тронут");

    // Симулируем краш сразу после write-before (НЕ вызываем finish) → строка CrashedMidExecute.
    match sink.replay_decision(&key).await.unwrap() {
        ReplayDecision::CrashedMidExecute(r) => {
            assert_eq!(
                r.content_hash.as_deref(),
                Some(crate::vault::content_hash("before".as_bytes()).as_str()),
                "строка несёт content_hash для re-check (восстановима)"
            );
        }
        other => panic!("ожидался CrashedMidExecute, получено {other:?}"),
    }
}

/// REPLAY AlreadyDone: то же действие дважды (тот же run_id/key) → второй вызов AlreadyDone, файл
/// записан РОВНО один раз (no double-write).
#[tokio::test]
async fn replay_already_done_no_double_write() {
    let (_d, root, sink) = setup().await;
    write_existing(&root, "D.md", "orig");
    let action = Action::note_edit("D.md", "v1");

    let first = apply_action(&action, 1, &root, &sink, None).await;
    assert!(matches!(first, ApplyOutcome::Executed { .. }));
    assert_eq!(read(&root, "D.md"), "v1");

    // Второй идентичный вызов (тот же run_id ⇒ тот же ключ; и тот же on-disk hash на момент 2-го
    // classify? — нет: файл теперь "v1", а ключ строится из on-disk hash. Чтобы воспроизвести
    // ИМЕННО replay-ветку, ключ должен совпасть → действие должно стартовать с тем же on-disk
    // состоянием. Здесь после 1-й записи on-disk="v1", target_hash другой → НОВЫЙ ключ → это уже
    // НЕ дубль, а новое действие "v1→v1". Поэтому для AlreadyDone-ветки берём create (target_hash
    // от planned content, стабилен).
    let create = Action::note_create("New2.md", "fixed");
    let c1 = apply_action(&create, 5, &root, &sink, None).await;
    assert!(matches!(c1, ApplyOutcome::Executed { .. }));
    let first_content = read(&root, "New2.md");

    // Тот же create (run 5, тот же ключ) повторно: цель теперь существует, НО ключ совпал →
    // ledger record_before отобьётся UNIQUE → replay → AlreadyDone (НЕ повторная запись/ошибка).
    let c2 = apply_action(&create, 5, &root, &sink, None).await;
    assert!(
        matches!(c2, ApplyOutcome::AlreadyDone(_)),
        "повтор того же действия → AlreadyDone, было {c2:?}"
    );
    assert_eq!(
        read(&root, "New2.md"),
        first_content,
        "файл не переписан повторно"
    );
}

/// REPLAY CrashedMidExecute + on-disk drift → Failed (no clobber). Берём CREATE: его ключ НЕ зависит
/// от on-disk hash (target_hash = отпечаток ПЛАНИРУЕМОГО контента), поэтому ключ СТАБИЛЕН при дрейфе
/// диска — что и нужно, чтобы попасть в CrashedMidExecute-ветку (у edit дрейф диска «уводит» ключ →
/// Fresh, а не replay; для edit/frontmatter ранний дрейф ловит Рубеж 3 по classify_hash). Симулируем
/// краш: вставляем строку write-before (outcome=NULL) с content_hash от «состояния на момент попытки»,
/// затем на диске ИНОЕ содержимое (дрейф), затем apply тем же ключом → CrashedMidExecute → re-check
/// hash ≠ → Failed, диск не тронут.
#[tokio::test]
async fn replay_crashed_with_drift_fails_no_clobber() {
    let (_d, root, sink) = setup().await;
    let action = Action::note_create("C.md", "fixed-planned");

    // Ключ create (classify_hash=None) = blake3(.., target_hash=hash("fixed-planned")) — стабилен.
    let key = key_for(1, &action, None, Some("fixed-planned"));
    // Строка-якорь крашнутой попытки: content_hash от «того, что было на диске на момент попытки».
    let entry = ActionEntry {
        run_id: 1,
        idempotency_key: key.clone(),
        tool_name: action.target.tool_name().to_string(),
        target_rel: Some("C.md".to_string()),
        risk_tier: super::super::audit::TIER_AUTO.to_string(),
        state: STATE_EXECUTING.to_string(),
        content_hash: Some(crate::vault::content_hash("at-attempt-state".as_bytes())),
        diff_summary: None,
    };
    sink.record_before(entry).await.unwrap(); // outcome=NULL → CrashedMidExecute при replay

    // На диске ИНОЕ содержимое (дрейф vs то, что зафиксировала крашнутая строка).
    write_existing(&root, "C.md", "DRIFTED-EXTERNALLY");

    // apply тем же ключом: record_before отобьётся UNIQUE → replay CrashedMidExecute → re-check
    // on-disk hash (от "DRIFTED…") ≠ row.content_hash (от "at-attempt-state") → Failed, НЕ клобберим.
    let outcome = apply_action(&action, 1, &root, &sink, None).await;
    assert!(
        matches!(outcome, ApplyOutcome::Failed(_)),
        "drift при CrashedMidExecute → Failed, было {outcome:?}"
    );
    assert_eq!(
        read(&root, "C.md"),
        "DRIFTED-EXTERNALLY",
        "содержимое на диске НЕ затёрто (no clobber)"
    );
    // Строка теперь терминальна (Failed) — повторный finish не нужен.
    let row = ledger_row(&sink, &key).await.unwrap();
    assert_eq!(row.state, "failed");
    assert!(row.outcome.is_some());
}

/// REPLAY CrashedMidExecute БЕЗ дрейфа → complete-forward: re-check hash совпал → доводим write.
/// CREATE: вставляем крашнутую строку с content_hash, совпадающим с тем, что на диске СЕЙЧАС, затем
/// apply тем же ключом → CrashedMidExecute → hash match → запись доводится, строка → Executed.
#[tokio::test]
async fn replay_crashed_no_drift_completes_forward() {
    let (_d, root, sink) = setup().await;
    let action = Action::note_create("CF.md", "planned-body");
    let key = key_for(1, &action, None, Some("planned-body"));

    // На диске уже лежит «состояние на момент крашнутой попытки» (как будто write частично/повторно).
    write_existing(&root, "CF.md", "partial-on-disk");
    let entry = ActionEntry {
        run_id: 1,
        idempotency_key: key.clone(),
        tool_name: action.target.tool_name().to_string(),
        target_rel: Some("CF.md".to_string()),
        risk_tier: super::super::audit::TIER_AUTO.to_string(),
        state: STATE_EXECUTING.to_string(),
        // content_hash СОВПАДАЕТ с on-disk сейчас → нет дрейфа → complete-forward.
        content_hash: Some(crate::vault::content_hash("partial-on-disk".as_bytes())),
        diff_summary: None,
    };
    sink.record_before(entry).await.unwrap();

    let outcome = apply_action(&action, 1, &root, &sink, None).await;
    assert!(
        matches!(outcome, ApplyOutcome::Executed { .. }),
        "CrashedMidExecute без дрейфа → complete-forward (Executed), было {outcome:?}"
    );
    assert_eq!(read(&root, "CF.md"), "planned-body", "запись доведена");
    let row = ledger_row(&sink, &key).await.unwrap();
    assert_eq!(row.state, "executed");
}

/// DRIFT при classify_hash: at-classify hash отличается от on-disk сейчас → Failed (не клобберим),
/// файл не тронут — прямой drift-рубеж (без replay-ветки).
#[tokio::test]
async fn classify_hash_drift_aborts() {
    let (_d, root, sink) = setup().await;
    write_existing(&root, "G.md", "current-on-disk");
    let action = Action::note_edit("G.md", "new");

    // classify_hash от УСТАРЕВШЕГО состояния (не совпадает с on-disk "current-on-disk").
    let stale = crate::vault::content_hash("stale-view".as_bytes());
    let outcome = apply_action(&action, 1, &root, &sink, Some(&stale)).await;
    assert!(
        matches!(outcome, ApplyOutcome::Failed(_)),
        "stale classify_hash → Failed, было {outcome:?}"
    );
    assert_eq!(
        read(&root, "G.md"),
        "current-on-disk",
        "файл не тронут при дрейфе"
    );

    // А при СОВПАДАЮЩЕМ classify_hash — проходит.
    let fresh = crate::vault::content_hash("current-on-disk".as_bytes());
    let ok = apply_action(&action, 2, &root, &sink, Some(&fresh)).await;
    assert!(
        matches!(ok, ApplyOutcome::Executed { .. }),
        "свежий hash → Executed"
    );
    assert_eq!(read(&root, "G.md"), "new");
}

/// FIX 1 — симлинк-безопасный create_dir_all: предсуществующий компонент-каталог родителя — симлинк
/// НАРУЖУ vault (`vault/sub -> /outside`); create на ГЛУБОКИЙ путь `sub/a/b/new.md`. Без проверки
/// create_dir_all создал бы `/outside/a/b` (пустые каталоги ВНЕ vault) ДО конфайнмент-reject. С Fix 1
/// reject_symlinked_components ловит `sub`-симлинк ДО create_dir_all → PathEscape, ничего не создано
/// под /outside. Доказывает: НЕТ создания каталогов вне vault через симлинк-компонент.
#[cfg(unix)]
#[tokio::test]
async fn create_dir_all_symlink_component_no_outside_dirs() {
    use std::os::unix::fs::symlink;
    let (_d, root, sink) = setup().await;
    let outside_dir = TempDir::new().unwrap();
    let outside = outside_dir.path().canonicalize().unwrap();
    // Предсуществующий компонент-СИМЛИНК наружу: vault/sub -> /outside (каталог).
    symlink(&outside, abs_of(&root, "sub")).unwrap();

    // create на глубокий путь СКВОЗЬ симлинк-компонент: sub/a/b — ещё не существуют.
    let action = Action::note_create("sub/a/b/new.md", "PWNED");
    let outcome = apply_action(&action, 1, &root, &sink, None).await;

    // Главный инвариант: НИ ОДНОГО каталога не создано под /outside.
    assert!(
        !outside.join("a").exists(),
        "Fix 1: create_dir_all НЕ должен создавать каталоги ВНЕ vault сквозь симлинк-компонент"
    );
    assert!(
        !outside.join("a/b").exists(),
        "глубже тоже ничего не создано"
    );
    assert!(
        !outside.join("a/b/new.md").exists(),
        "файл вне vault не создан"
    );
    // И apply отверг как PathEscape (рубеж, а не побочный rename).
    assert_eq!(
        outcome,
        ApplyOutcome::PathEscape,
        "симлинк-компонент родителя наружу → PathEscape, получено {outcome:?}"
    );
}

/// FIX 2 (focused unit) — ре-рид-перед-записью: helper детектит дрейф on-disk vs ожидаемый хеш.
/// Это БЕЗУСЛОВНЫЙ фенс (не зависит от classify_hash). Доказывает: совпадение → НЕ дрейф (запись
/// разрешена), внешняя правка → дрейф (запись отменяется), удаление файла → дрейф.
#[tokio::test]
async fn reread_drift_helper_detects_external_change() {
    let (_d, root, _sink) = setup().await;
    write_existing(&root, "RR.md", "BASE");
    let abs = abs_of(&root, "RR.md");
    let base_hash = crate::vault::content_hash("BASE".as_bytes());

    // Совпадение: диск == то, что прочитали в Рубеже 2 → НЕ дрейф (запись пойдёт).
    assert!(
        !reread_drift_detected(&abs, Some(&base_hash)).await,
        "без внешней правки дрейфа нет"
    );

    // Внешняя правка в окне read→write: диск изменился → ДРЕЙФ (запись должна отмениться).
    fs::write(&abs, "EXTERNALLY-CHANGED").unwrap();
    assert!(
        reread_drift_detected(&abs, Some(&base_hash)).await,
        "внешняя правка → дрейф (re-read fence срабатывает безусловно)"
    );

    // Внешнее удаление: fresh=None vs Some(expected) → ДРЕЙФ (не пишем поверх гонки удаления).
    fs::remove_file(&abs).unwrap();
    assert!(
        reread_drift_detected(&abs, Some(&base_hash)).await,
        "внешнее удаление → дрейф"
    );
}

/// FIX 2 (no-false-positive regression) — стабильный диск: re-read фенс НЕ должен ложно блокировать
/// легитимную правку (между внутренним read Рубежа 2 и ре-ридом диск не менялся → хеши совпадают →
/// запись проходит). Дополняет drift-ветку helper-теста выше: вместе они фиксируют, что фенс ловит
/// ИМЕННО дрейф и не калечит нормальный overwrite.
#[tokio::test]
async fn reread_fence_allows_legitimate_overwrite() {
    let (_d, root, sink) = setup().await;
    write_existing(&root, "OK.md", "STABLE");
    let action = Action::note_edit("OK.md", "NEW-CONTENT");
    let outcome = apply_action(&action, 1, &root, &sink, None).await;
    assert!(
        matches!(outcome, ApplyOutcome::Executed { .. }),
        "стабильный диск: re-read фенс НЕ должен ложно блокировать легитимную правку, было {outcome:?}"
    );
    assert_eq!(
        read(&root, "OK.md"),
        "NEW-CONTENT",
        "легитимная правка записана"
    );
}

/// FIX 3 (cfg unix) — hardlink-reject на overwrite: ВНУТРИ vault создаём ХАРДЛИНК на ВНЕШНИЙ файл
/// (`vault/hard.md` == inode внешнего secret.md). symlink_metadata().is_symlink() для хардлинка ЛОЖЕН,
/// поэтому leaf-симлинк-проверка его НЕ ловит. Fix 3 сверяет nlink>1 → PathEscape: внешний файл НЕ
/// затронут, его содержимое НЕ утекает в снапшот. Доказывает defense-in-depth против info-leak.
#[cfg(unix)]
#[tokio::test]
async fn hardlink_overwrite_rejected_no_outside_leak() {
    let (_d, root, sink) = setup().await;
    let outside_dir = TempDir::new().unwrap();
    let outside = outside_dir.path().canonicalize().unwrap().join("secret.md");
    fs::write(&outside, "OUTSIDE-SECRET").unwrap();

    // Хардлинк ВНУТРИ vault на ВНЕШНИЙ inode: vault/hard.md → тот же inode, что outside/secret.md.
    // (hard_link следует обычной семантике: оба имени делят inode, nlink=2.)
    fs::hard_link(&outside, abs_of(&root, "hard.md")).unwrap();
    // Sanity: это НЕ симлинк (иначе ловила бы leaf-проверка, а не Fix 3).
    assert!(
        !fs::symlink_metadata(abs_of(&root, "hard.md"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "хардлинк — не симлинк (иначе тест проверял бы не ту гарду)"
    );

    let action = Action::note_edit("hard.md", "PWNED-THROUGH-HARDLINK");
    let outcome = apply_action(&action, 1, &root, &sink, None).await;

    // Внешний файл (тот же inode) НЕ изменён.
    assert_eq!(
        fs::read_to_string(&outside).unwrap(),
        "OUTSIDE-SECRET",
        "Fix 3: внешний файл за хардлинком НЕ должен быть перезаписан"
    );
    // apply отверг как PathEscape (nlink>1), а не как побочный эффект.
    assert_eq!(
        outcome,
        ApplyOutcome::PathEscape,
        "хардлинк (nlink>1) на overwrite → PathEscape, получено {outcome:?}"
    );
    // Снапшот внешнего содержимого НЕ создан (чтения по хардлинку не было до reject).
    let snaps = list_snapshots(&root, "hard.md").unwrap();
    assert!(
        snaps.is_empty(),
        "info-leak: НЕ должно быть снапшота внешнего содержимого, есть {snaps:?}"
    );
}

/// AGENT-6 (cfg windows) — зеркало unix hardlink-reject: ВНУТРИ vault хардлинк на ВНЕШНИЙ файл
/// (`std::fs::hard_link`, nNumberOfLinks=2). leaf-симлинк-проверка хардлинк НЕ ловит; Windows-рубеж 3
/// (`GetFileInformationByHandle`/nNumberOfLinks>1) → PathEscape: внешний файл НЕ затронут, его
/// содержимое НЕ утекает в снапшот. Закрывает Windows info-leak-щель (раньше check был только unix).
#[cfg(windows)]
#[tokio::test]
async fn hardlink_overwrite_rejected_no_outside_leak_windows() {
    let (_d, root, sink) = setup().await;
    let outside_dir = TempDir::new().unwrap();
    let outside = outside_dir.path().canonicalize().unwrap().join("secret.md");
    fs::write(&outside, "OUTSIDE-SECRET").unwrap();

    // Хардлинк ВНУТРИ vault на ВНЕШНИЙ inode (на Windows hard_link тоже даёт nNumberOfLinks=2).
    fs::hard_link(&outside, abs_of(&root, "hard.md")).unwrap();
    // Sanity: это НЕ симлинк (иначе ловила бы leaf-проверка, а не Windows-рубеж 3).
    assert!(
        !fs::symlink_metadata(abs_of(&root, "hard.md"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "хардлинк — не симлинк"
    );

    let action = Action::note_edit("hard.md", "PWNED-THROUGH-HARDLINK");
    let outcome = apply_action(&action, 1, &root, &sink, None).await;

    assert_eq!(
        fs::read_to_string(&outside).unwrap(),
        "OUTSIDE-SECRET",
        "Windows: внешний файл за хардлинком НЕ должен быть перезаписан"
    );
    assert_eq!(
        outcome,
        ApplyOutcome::PathEscape,
        "хардлинк (nNumberOfLinks>1) на overwrite → PathEscape, получено {outcome:?}"
    );
    let snaps = list_snapshots(&root, "hard.md").unwrap();
    assert!(
        snaps.is_empty(),
        "info-leak: НЕ должно быть снапшота внешнего содержимого, есть {snaps:?}"
    );
}

/// **Фаза-3 RUBEZH-0 (SANDBOX-6b):** `apply_action` отвергает exec-таргеты
/// (`ShellRun`/`ProcessSpawn`/`GitOp`) top-guard'ом ДО любого vault-IO → `Failed`, БЕЗ файла и БЕЗ
/// ledger-строки. ПИНИТ guard, от которого зависят `unreachable!()`-армы в WRITE/success_summary:
/// если guard уберут рефактором — этот тест покраснеет ДО того, как exec упрётся в panic. (Сегодня
/// exec и так HardBlocked на classify по умолчанию, но apply — единственный путь к диску, и его
/// defense-in-depth обязан быть зафиксирован.)
#[tokio::test]
async fn exec_apply_is_fail_closed() {
    let (_d, root, sink) = setup().await;
    for action in [
        Action::shell_run(vec!["ls".into()], None),
        Action::process_spawn("git", vec!["status".into()], None),
        Action::git_op("status", vec![]),
    ] {
        let outcome = apply_action(&action, 1, &root, &sink, Some("")).await;
        assert!(
            matches!(outcome, ApplyOutcome::Failed(_)),
            "exec {:?} должен быть Failed (RUBEZH-0), получено {outcome:?}",
            action.target
        );
    }
    // Ни одной заметки не создано (exec не пишет vault — top-guard вернул до РУБЕЖА 1).
    let md_count = fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
        .count();
    assert_eq!(md_count, 0, "exec НЕ должен создавать файлы в vault");
}

/// **SL-7 RUBEZH-0-bis:** `apply_action` отвергает `SkillSave` top-guard'ом ДО любого vault-IO →
/// `Failed`, БЕЗ файла в vault и БЕЗ ledger-строки. ПИНИТ guard, от которого зависят
/// `unreachable!()`-армы SkillSave в WRITE/success_summary: уберут guard рефактором — покраснеет ДО
/// panic. SkillSave НИКОГДА не пишется vault-путём (его путь — `apply_skill_save`/skills_root, SL-7c).
#[tokio::test]
async fn skill_save_apply_is_fail_closed() {
    let (_d, root, sink) = setup().await;
    // rel формы навыка; даже выглядящий «как заметка» — apply_action не должен его писать в vault.
    let action = Action::skill_save("MySkill/SKILL.md", "body of skill");
    let outcome = apply_action(&action, 1, &root, &sink, Some("")).await;
    assert!(
        matches!(outcome, ApplyOutcome::Failed(_)),
        "SkillSave должен быть Failed (RUBEZH-0-bis), получено {outcome:?}"
    );
    // Ни в vault-корне, ни во вложенном MySkill/ ничего не создано.
    assert!(
        !root.join("MySkill").exists(),
        "SkillSave НЕ должен создавать каталог/файл в vault"
    );
    let any_file = fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.path().is_file());
    assert!(
        !any_file,
        "SkillSave НЕ должен создавать файлы в vault-корне"
    );
}

// ── SL-7c: apply_skill_save (skills_root-confined обратимая запись) ──────────────────────────
const VALID_SKILL: &str = "---\nname: myskill\ndescription: does things\n---\nBODY";

/// Never-paused kill-switch для тестов apply_skill_save.
fn np() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// CREATE: новый навык записан в skills_root; ledger Executed; undo = Trash.
#[tokio::test]
async fn apply_skill_save_create_writes_and_trash_undo() {
    let (_d, skills_root, sink) = setup().await;
    let action = Action::skill_save("myskill/SKILL.md", VALID_SKILL);
    let out = apply_skill_save(&action, 1, &skills_root, &sink, None, &np()).await;
    match out {
        ApplyOutcome::Executed { undo, .. } => {
            assert!(
                matches!(undo, UndoHandle::Trash { .. }),
                "create → Trash undo"
            );
        }
        o => panic!("ожидалось Executed, получено {o:?}"),
    }
    assert_eq!(
        fs::read_to_string(skills_root.join("myskill/SKILL.md")).unwrap(),
        VALID_SKILL,
        "файл навыка записан под skills_root"
    );
}

/// OVERWRITE: перезапись существующего навыка снапшотит ПРЕД-контент (undo=Snapshot восстановим).
#[tokio::test]
async fn apply_skill_save_overwrite_snapshots_undo() {
    let (_d, skills_root, sink) = setup().await;
    let old = "---\nname: myskill\ndescription: old\n---\nOLD";
    let abs = skills_root.join("myskill/SKILL.md");
    fs::create_dir_all(abs.parent().unwrap()).unwrap();
    fs::write(&abs, old).unwrap();

    let new = "---\nname: myskill\ndescription: new\n---\nNEW";
    let out = apply_skill_save(
        &Action::skill_save("myskill/SKILL.md", new),
        1,
        &skills_root,
        &sink,
        None,
        &np(),
    )
    .await;
    match out {
        ApplyOutcome::Executed { undo, .. } => {
            assert!(
                matches!(undo, UndoHandle::Snapshot { .. }),
                "overwrite → Snapshot undo"
            );
        }
        o => panic!("ожидалось Executed, получено {o:?}"),
    }
    assert_eq!(fs::read_to_string(&abs).unwrap(), new, "навык перезаписан");
    // Снапшот ПРЕД-контента (old) лежит в skills_root/.nexus/history — обратимость.
    let snaps = crate::vault::history::list_snapshots(&skills_root, "myskill/SKILL.md").unwrap();
    let bodies: Vec<String> = snaps
        .iter()
        .map(|s| {
            crate::vault::history::read_snapshot(&skills_root, "myskill/SKILL.md", s.ts).unwrap()
        })
        .collect();
    assert!(
        bodies.iter().any(|b| b == old),
        "пред-контент снапшотнут: {bodies:?}"
    );
}

/// MALFORMED: SKILL.md без валидного frontmatter → Failed, НИЧЕГО не записано (parse-фенс до диска).
#[tokio::test]
async fn apply_skill_save_malformed_no_write() {
    let (_d, skills_root, sink) = setup().await;
    let out = apply_skill_save(
        &Action::skill_save("bad/SKILL.md", "просто текст без frontmatter"),
        1,
        &skills_root,
        &sink,
        None,
        &np(),
    )
    .await;
    assert!(
        matches!(out, ApplyOutcome::Failed(_)),
        "битый навык → Failed: {out:?}"
    );
    assert!(
        !skills_root.join("bad").exists(),
        "битый навык не создал ни файла, ни каталога"
    );
}

/// PATH-ESCAPE: `../` rel → PathEscape, БЕЗ записи и БЕЗ создания каталога ВНЕ skills_root.
#[tokio::test]
async fn apply_skill_save_pathescape_no_write() {
    let (_d, skills_root, sink) = setup().await;
    let out = apply_skill_save(
        &Action::skill_save("../escape/SKILL.md", VALID_SKILL),
        1,
        &skills_root,
        &sink,
        None,
        &np(),
    )
    .await;
    assert_eq!(out, ApplyOutcome::PathEscape, "../ → PathEscape: {out:?}");
    assert!(
        !skills_root.parent().unwrap().join("escape").exists(),
        "create_dir_all НЕ создал каталог ВНЕ skills_root (лексический гард до ФС)"
    );
}
