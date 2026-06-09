//! Интеграционные тесты git-sync (#12) — прогон ПУБЛИЧНОГО API `GitSync` как ВНЕШНИЙ потребитель
//! крейта (отдельная цель `tests/`, линкуется с `nexus_desktop_lib`). Ловит две вещи, которых нет в
//! unit-тестах `git/mod.rs`:
//!   1. случайную приватизацию pub-поверхности (тест не скомпилируется),
//!   2. РЕАЛЬНЫЙ сетевой round-trip `push`/`pull`/fast-forward/`MergeRequired` между двумя клонами.
//!
//! Remote — ЛОКАЛЬНЫЙ bare-репозиторий: для local-транспорта libgit2 НЕ дёргает credentials-callback,
//! поэтому токен-заглушка достаточна (CI не нужен ни сетевой доступ, ни git-identity — `GitSync`
//! сам ставит подпись по умолчанию). git2 в dev-deps нужен только чтобы СОЗДАТЬ bare-remote и клон.

use std::fs;
use std::path::Path;

use nexus_desktop_lib::git::{CommitOutcome, GitSync, PullOutcome};
use tempfile::TempDir;

/// Путь как URL remote (forward-slash — надёжно и на Windows для git2).
fn url_of(dir: &Path) -> String {
    dir.to_string_lossy().replace('\\', "/")
}

/// Содержимое файла с нормализацией EOL: git на Windows чекаутит рабочее дерево с CRLF (autocrlf),
/// поэтому сравниваем КОНТЕНТ, а не байты переводов строк (round-trip контента — вот что важно).
fn read_norm(p: std::path::PathBuf) -> String {
    fs::read_to_string(p).unwrap().replace("\r\n", "\n")
}

/// Свежий bare-remote во временном каталоге.
fn bare_remote() -> TempDir {
    let dir = TempDir::new().unwrap();
    git2::Repository::init_bare(dir.path()).unwrap();
    dir
}

/// Локальный flow без сети (внешний потребитель): commit → status → secret-scan блокирует коммит.
#[test]
fn local_commit_status_and_secret_block() {
    let v = TempDir::new().unwrap();
    let git = GitSync::open_or_init(v.path()).unwrap();
    git.ensure_gitignore().unwrap();
    fs::write(v.path().join("note.md"), "# Заметка\n\nчистый текст\n").unwrap();

    // Новый файл виден в статусе.
    assert!(
        git.status().unwrap().iter().any(|e| e.path == "note.md"),
        "новый файл в статусе"
    );

    // Коммит создаётся (подпись — дефолтная из GitSync, identity в окружении не нужна).
    match git.commit_all().unwrap() {
        CommitOutcome::Committed { files, .. } => assert!(files >= 1),
        other => panic!("ожидали Committed, получили {other:?}"),
    }
    assert_eq!(git.commit_all().unwrap(), CommitOutcome::NothingToCommit);

    // Секрет → коммит заблокирован (AC-SEC-3), файл остаётся незакоммиченным.
    fs::write(
        v.path().join("leak.md"),
        "ключ: sk-ABCDEFGHIJKLMNOPQRSTUVWX1234567890",
    )
    .unwrap();
    match git.commit_all().unwrap() {
        CommitOutcome::BlockedBySecrets { findings } => {
            assert!(findings.iter().any(|f| f.path == "leak.md"));
        }
        other => panic!("ожидали BlockedBySecrets, получили {other:?}"),
    }
    assert!(git.status().unwrap().iter().any(|e| e.path == "leak.md"));
}

/// Полный сетевой round-trip через локальный bare-remote: A пушит → B клонирует → B пушит правку →
/// A подтягивает fast-forward. Покрывает `set_remote`/`push`/`pull`/FF — то, чего нет в unit-тестах.
#[test]
fn push_clone_pull_fast_forward_roundtrip() {
    let remote = bare_remote();
    let url = url_of(remote.path());

    // Vault A: заметка → коммит → push в bare-remote.
    let a = TempDir::new().unwrap();
    let ga = GitSync::open_or_init(a.path()).unwrap();
    ga.ensure_gitignore().unwrap();
    fs::write(a.path().join("note.md"), "v1 from A\n").unwrap();
    ga.set_remote(&url).unwrap();
    assert!(matches!(
        ga.commit_all().unwrap(),
        CommitOutcome::Committed { .. }
    ));
    ga.push("local-no-auth").unwrap();

    // Vault B: клон remote → видит заметку A.
    let b = TempDir::new().unwrap();
    git2::Repository::clone(&url, b.path()).unwrap();
    assert_eq!(read_norm(b.path().join("note.md")), "v1 from A\n");

    // B правит → коммит → push (remote уходит вперёд).
    let gb = GitSync::open_or_init(b.path()).unwrap();
    fs::write(b.path().join("note.md"), "v1 from A\nv2 from B\n").unwrap();
    assert!(matches!(
        gb.commit_all().unwrap(),
        CommitOutcome::Committed { .. }
    ));
    gb.push("local-no-auth").unwrap();

    // A подтягивает → fast-forward, рабочее дерево обновлено правкой B.
    match ga.pull("local-no-auth").unwrap() {
        PullOutcome::FastForward { .. } | PullOutcome::UpToDate => {}
        other => panic!("ожидали FastForward, получили {other:?}"),
    }
    assert_eq!(
        read_norm(a.path().join("note.md")),
        "v1 from A\nv2 from B\n",
        "A получил правку B по pull"
    );
}

/// Расхождение истории (оба ветвятся от общего коммита и коммитят своё) → `pull` сигналит
/// `MergeRequired` (без авто-слияния; разрешение — отдельный поток Ф3-3b-3).
#[test]
fn divergent_history_signals_merge_required() {
    let remote = bare_remote();
    let url = url_of(remote.path());

    // A: базовый коммит → push.
    let a = TempDir::new().unwrap();
    let ga = GitSync::open_or_init(a.path()).unwrap();
    ga.ensure_gitignore().unwrap();
    fs::write(a.path().join("n.md"), "base\n").unwrap();
    ga.set_remote(&url).unwrap();
    ga.commit_all().unwrap();
    ga.push("x").unwrap();

    // B: клон → своя правка → push (remote = B-ветка).
    let b = TempDir::new().unwrap();
    git2::Repository::clone(&url, b.path()).unwrap();
    let gb = GitSync::open_or_init(b.path()).unwrap();
    fs::write(b.path().join("n.md"), "base\nB\n").unwrap();
    gb.commit_all().unwrap();
    gb.push("x").unwrap();

    // A: СВОЯ правка поверх базового (расходится с B) → pull видит расхождение.
    fs::write(a.path().join("n.md"), "base\nA\n").unwrap();
    ga.commit_all().unwrap();
    assert!(
        matches!(ga.pull("x").unwrap(), PullOutcome::MergeRequired),
        "расходящиеся истории → MergeRequired"
    );
}

/// #10 (внешний потребитель): выборочный коммит `commit_paths` стейджит только выбранный путь;
/// прочие изменения остаются не закоммиченными.
#[test]
fn selective_commit_stages_only_chosen_paths() {
    let v = TempDir::new().unwrap();
    let git = GitSync::open_or_init(v.path()).unwrap();
    git.ensure_gitignore().unwrap();
    fs::write(v.path().join("a.md"), "# A\n").unwrap();
    fs::write(v.path().join("b.md"), "# B\n").unwrap();

    let out = git.commit_paths(&["a.md".to_string()]).unwrap();
    assert!(
        matches!(out, CommitOutcome::Committed { files: 1, .. }),
        "закоммичен ровно один файл (a.md)"
    );
    let pending: Vec<String> = git.status().unwrap().into_iter().map(|e| e.path).collect();
    assert!(
        pending.iter().any(|p| p == "b.md"),
        "b.md остался не закоммичен"
    );
    assert!(!pending.iter().any(|p| p == "a.md"), "a.md закоммичен");
}
