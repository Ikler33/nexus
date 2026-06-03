//! git-sync (Фаза 3, §8): vault как git-репозиторий. Это **фундамент** — локальные операции:
//! open/init, управляемый `.gitignore`, `status`. Выборочный коммит + secret-scan — Ф3-2;
//! pull/push + разрешение конфликтов (диск vs грязный буфер) — Ф3-3.
//!
//! Весь libgit2-I/O синхронный → из Tauri-команд вызывается в `spawn_blocking` (как §8 «всё в
//! spawn_blocking»). git-sync — **core module**, НЕ sandbox-плагин (ADR/§8).

pub mod creds;

use std::path::{Path, PathBuf};

use git2::{Repository, StatusOptions};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git: {0}")]
    Git(#[from] git2::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type GitResult<T> = Result<T, GitError>;

/// Маркер управляемого блока в `.gitignore` — для идемпотентного обновления (не затираем
/// пользовательские правила, не дублируем свой блок).
const NEXUS_IGNORE_MARKER: &str = "# >>> nexus (managed) >>>";

/// Управляемый блок `.gitignore`: внутренние данные Nexus НЕ синхронизируются (индекс, секреты,
/// **код плагинов** — AC-Б3-1), но декларация плагинов `.nexus/config.json` синхронизируется.
const NEXUS_IGNORE_BLOCK: &str = "\
# >>> nexus (managed) >>>
# Внутреннее Nexus — НЕ в git: индекс/векторы/БД, секреты (local.json), код плагинов (AC-Б3-1, AC-SEC-3).
.nexus/*
# Декларация установленных плагинов (id@version#sha256) — синхронизируется (AC-Б3-1).
!.nexus/config.json
# <<< nexus (managed) <<<
";

/// Категория изменения файла в рабочем дереве (упрощённо, для UI/синка).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    New,
    Modified,
    Deleted,
    Renamed,
    Other,
}

/// Статус одного файла (путь относительно корня vault, разделитель `/`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StatusEntry {
    pub path: String,
    pub kind: ChangeKind,
}

/// Тип обнаруженного секрета (высокоточные форматы — мало ложных срабатываний).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SecretKind {
    PrivateKey,
    OpenAiKey,
    GithubToken,
    AwsAccessKey,
    SlackToken,
}

/// Находка секрета: строка (1-based) и тип.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SecretFinding {
    pub line: usize,
    pub kind: SecretKind,
}

/// Файл с найденными секретами (для отчёта о блокировке коммита).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileSecret {
    pub path: String,
    pub findings: Vec<SecretFinding>,
}

/// Исход авто-коммита (для UI/команды).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum CommitOutcome {
    NothingToCommit,
    /// Найдены секреты — коммит НЕ сделан (AC-SEC-3, secret-scan коммитов).
    BlockedBySecrets {
        findings: Vec<FileSecret>,
    },
    Committed {
        oid: String,
        message: String,
        files: usize,
    },
}

/// vault как git-репозиторий. Держит открытый `Repository` (libgit2).
pub struct GitSync {
    repo: Repository,
    root: PathBuf,
}

impl GitSync {
    /// Открывает репозиторий в `root` или инициализирует новый (`git init`). Включение синка для
    /// vault и означает «сделать его git-репозиторием».
    pub fn open_or_init(root: &Path) -> GitResult<Self> {
        let repo = match Repository::open(root) {
            Ok(r) => r,
            Err(_) => Repository::init(root)?,
        };
        Ok(Self {
            repo,
            root: root.to_path_buf(),
        })
    }

    /// Идемпотентно добавляет управляемый блок в `.gitignore` (если его ещё нет). Пользовательские
    /// правила сохраняются; наш блок не дублируется.
    pub fn ensure_gitignore(&self) -> GitResult<()> {
        let path = self.root.join(".gitignore");
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        if existing.contains(NEXUS_IGNORE_MARKER) {
            return Ok(());
        }
        let mut out = existing;
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(NEXUS_IGNORE_BLOCK);
        std::fs::write(&path, out)?;
        Ok(())
    }

    /// Изменённые/новые/удалённые файлы рабочего дерева, БЕЗ игнорируемых (`.gitignore` в силе).
    /// Пути — относительные от корня vault, разделитель `/` (libgit2 нормализует).
    pub fn status(&self) -> GitResult<Vec<StatusEntry>> {
        let mut opts = StatusOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .include_ignored(false)
            .exclude_submodules(true);
        let statuses = self.repo.statuses(Some(&mut opts))?;

        let mut out = Vec::new();
        for entry in statuses.iter() {
            let Some(path) = entry.path() else { continue };
            let s = entry.status();
            let kind = if s.is_wt_new() || s.is_index_new() {
                ChangeKind::New
            } else if s.is_wt_deleted() || s.is_index_deleted() {
                ChangeKind::Deleted
            } else if s.is_wt_renamed() || s.is_index_renamed() {
                ChangeKind::Renamed
            } else if s.is_wt_modified() || s.is_index_modified() {
                ChangeKind::Modified
            } else {
                ChangeKind::Other
            };
            out.push(StatusEntry {
                path: path.to_string(),
                kind,
            });
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// Корень vault.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Авто-коммит: стейджит все не-игнорируемые изменения, **сканирует содержимое на секреты**
    /// (AC-SEC-3) — при находке коммит НЕ делается; иначе коммитит с авто-сообщением. Идемпотентно
    /// (нет изменений → `NothingToCommit`). Удаления тоже учитываются (`update_all`).
    pub fn commit_all(&self) -> GitResult<CommitOutcome> {
        let status = self.status()?;
        if status.is_empty() {
            return Ok(CommitOutcome::NothingToCommit);
        }

        // Secret-scan по добавляемым/изменённым файлам (удалённые и бинарные — пропуск).
        let mut secrets = Vec::new();
        for e in &status {
            if e.kind == ChangeKind::Deleted {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(self.root.join(&e.path)) else {
                continue;
            };
            let findings = scan_secrets(&content);
            if !findings.is_empty() {
                secrets.push(FileSecret {
                    path: e.path.clone(),
                    findings,
                });
            }
        }
        if !secrets.is_empty() {
            return Ok(CommitOutcome::BlockedBySecrets { findings: secrets });
        }

        // Стейдж: add_all (new/modified, уважает .gitignore) + update_all (удаления tracked-файлов).
        let mut index = self.repo.index()?;
        index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
        index.update_all(["*"].iter(), None)?;
        index.write()?;
        let tree = self.repo.find_tree(index.write_tree()?)?;

        let sig = self.signature()?;
        let message = auto_message(&status);
        let parent = self
            .repo
            .head()
            .ok()
            .and_then(|h| h.target())
            .and_then(|oid| self.repo.find_commit(oid).ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        let oid = self
            .repo
            .commit(Some("HEAD"), &sig, &sig, &message, &tree, &parents)?;

        Ok(CommitOutcome::Committed {
            oid: oid.to_string(),
            message,
            files: status.len(),
        })
    }

    /// Подпись из git-config репозитория, иначе дефолт `Nexus <nexus@local>` (чтобы коммит не падал
    /// при незаданных user.name/email).
    fn signature(&self) -> GitResult<git2::Signature<'static>> {
        if let Ok(sig) = self.repo.signature() {
            return Ok(sig);
        }
        Ok(git2::Signature::now("Nexus", "nexus@local")?)
    }
}

/// Скан текста на распространённые секреты. Высокоточные форматы (явные префиксы ключей/токенов и
/// PEM private key) — НЕ детектит общие «high-entropy» строки (это шумит ложными). По строкам.
pub fn scan_secrets(text: &str) -> Vec<SecretFinding> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let ln = i + 1;
        if line.contains("PRIVATE KEY-----") {
            out.push(SecretFinding {
                line: ln,
                kind: SecretKind::PrivateKey,
            });
        }
        if let Some(kind) = detect_token(line) {
            out.push(SecretFinding { line: ln, kind });
        }
    }
    out
}

/// Ищет в строке токен известного формата (по «словам»). Возвращает первый найденный тип.
fn detect_token(line: &str) -> Option<SecretKind> {
    let is_word_sep = |c: char| c.is_whitespace() || "\"'`,;()[]{}<>=".contains(c);
    for word in line.split(is_word_sep) {
        let w = word.trim();
        if w.len() < 16 {
            continue;
        }
        if let Some(rest) = w.strip_prefix("sk-") {
            if rest.len() >= 20
                && rest
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Some(SecretKind::OpenAiKey);
            }
        }
        if let Some(rest) = w.strip_prefix("ghp_") {
            if rest.len() >= 36 && rest.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Some(SecretKind::GithubToken);
            }
        }
        if w.starts_with("github_pat_") && w.len() > 30 {
            return Some(SecretKind::GithubToken);
        }
        if let Some(rest) = w.strip_prefix("AKIA") {
            if rest.len() == 16
                && rest
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            {
                return Some(SecretKind::AwsAccessKey);
            }
        }
        if (w.starts_with("xoxb-") || w.starts_with("xoxp-") || w.starts_with("xoxa-"))
            && w.len() >= 24
        {
            return Some(SecretKind::SlackToken);
        }
    }
    None
}

/// Авто-сообщение коммита из статуса: `Vault sync: +N new, ~M changed, -K deleted`.
fn auto_message(status: &[StatusEntry]) -> String {
    let (mut new, mut modified, mut deleted) = (0u32, 0u32, 0u32);
    for e in status {
        match e.kind {
            ChangeKind::New => new += 1,
            ChangeKind::Deleted => deleted += 1,
            _ => modified += 1,
        }
    }
    let mut parts = Vec::new();
    if new > 0 {
        parts.push(format!("+{new} new"));
    }
    if modified > 0 {
        parts.push(format!("~{modified} changed"));
    }
    if deleted > 0 {
        parts.push(format!("-{deleted} deleted"));
    }
    format!("Vault sync: {}", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn init_gitignore_and_status_excludes_nexus_internals() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let git = GitSync::open_or_init(root).unwrap();
        git.ensure_gitignore().unwrap();

        // .gitignore содержит наш блок: исключает .nexus/*, но оставляет config.json.
        let gi = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(gi.contains(".nexus/*"));
        assert!(gi.contains("!.nexus/config.json"));

        // Заметка + внутренние данные Nexus + декларация плагинов.
        std::fs::write(root.join("note.md"), "# заметка").unwrap();
        std::fs::create_dir_all(root.join(".nexus/plugins/evil")).unwrap();
        std::fs::write(root.join(".nexus/local.json"), "{\"secret\":1}").unwrap();
        std::fs::write(root.join(".nexus/plugins/evil/main.js"), "steal()").unwrap();
        std::fs::write(root.join(".nexus/config.json"), "{\"plugins\":[]}").unwrap();

        let paths: Vec<String> = git.status().unwrap().into_iter().map(|e| e.path).collect();

        // Синкается: заметка, .gitignore, декларация плагинов.
        assert!(paths.iter().any(|p| p == "note.md"), "заметка синкается");
        assert!(paths.iter().any(|p| p == ".gitignore"));
        assert!(
            paths.iter().any(|p| p == ".nexus/config.json"),
            "config.json синкается (AC-Б3-1)"
        );
        // НЕ синкается: секреты и код плагинов (AC-Б3-1 / AC-SEC-3).
        assert!(
            !paths.iter().any(|p| p.contains("local.json")),
            "секреты не синкаются"
        );
        assert!(
            !paths.iter().any(|p| p.contains("plugins/")),
            "код плагина не синкается (AC-Б3-1)"
        );
    }

    #[test]
    fn ensure_gitignore_is_idempotent_and_keeps_user_rules() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".gitignore"), "# мой блок\n*.tmp\n").unwrap();
        let git = GitSync::open_or_init(root).unwrap();

        git.ensure_gitignore().unwrap();
        git.ensure_gitignore().unwrap(); // повторно — не дублирует

        let gi = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert_eq!(
            gi.matches(NEXUS_IGNORE_MARKER).count(),
            1,
            "блок не задвоился"
        );
        assert!(gi.contains("*.tmp"), "пользовательские правила сохранены");
    }

    #[test]
    fn open_or_init_opens_existing_repo() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        GitSync::open_or_init(root).unwrap(); // init
        let again = GitSync::open_or_init(root); // open existing
        assert!(again.is_ok());
        assert_eq!(again.unwrap().root(), root);
    }

    #[test]
    fn scan_secrets_detects_common_formats_no_false_positives() {
        assert!(scan_secrets("-----BEGIN RSA PRIVATE KEY-----")
            .iter()
            .any(|f| f.kind == SecretKind::PrivateKey));
        assert!(scan_secrets("token = sk-abcdefghijklmnopqrstuvwxyz123456")
            .iter()
            .any(|f| f.kind == SecretKind::OpenAiKey));
        assert!(scan_secrets("ghp_0123456789012345678901234567890123ab")
            .iter()
            .any(|f| f.kind == SecretKind::GithubToken));
        assert!(scan_secrets("AKIAIOSFODNN7EXAMPLE")
            .iter()
            .any(|f| f.kind == SecretKind::AwsAccessKey));
        // Чистый текст и URL — без ложных срабатываний.
        assert!(
            scan_secrets("обычная заметка про кошек; ссылка https://example.com/a/b").is_empty()
        );
    }

    #[test]
    fn commit_all_commits_then_blocks_secret() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let git = GitSync::open_or_init(root).unwrap();
        git.ensure_gitignore().unwrap();
        std::fs::write(root.join("a.md"), "# чистая заметка").unwrap();

        match git.commit_all().unwrap() {
            CommitOutcome::Committed { files, .. } => assert!(files >= 1),
            other => panic!("ожидали Committed, получили {other:?}"),
        }
        assert_eq!(git.commit_all().unwrap(), CommitOutcome::NothingToCommit);

        // Заметка с секретом → блокировка, коммит НЕ сделан.
        std::fs::write(
            root.join("leak.md"),
            "ключ: sk-ABCDEFGHIJKLMNOPQRSTUVWX1234567890",
        )
        .unwrap();
        match git.commit_all().unwrap() {
            CommitOutcome::BlockedBySecrets { findings } => {
                assert!(findings.iter().any(|f| f.path == "leak.md"));
            }
            other => panic!("ожидали BlockedBySecrets, получили {other:?}"),
        }
        // Секрет не закоммичен → всё ещё в статусе.
        assert!(git.status().unwrap().iter().any(|e| e.path == "leak.md"));
    }
}
