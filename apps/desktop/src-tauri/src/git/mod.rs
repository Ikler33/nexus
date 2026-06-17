//! git-sync (Фаза 3, §8): vault как git-репозиторий. Это **фундамент** — локальные операции:
//! open/init, управляемый `.gitignore`, `status`. Выборочный коммит + secret-scan — Ф3-2;
//! pull/push + разрешение конфликтов (диск vs грязный буфер) — Ф3-3.
//!
//! Весь libgit2-I/O синхронный → из Tauri-команд вызывается в `spawn_blocking` (как §8 «всё в
//! spawn_blocking»). git-sync — **core module**, НЕ sandbox-плагин (ADR/§8).

pub mod creds;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use git2::{Repository, StatusOptions};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git: {0}")]
    Git(#[from] git2::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Рабочее дерево содержит несохранённые изменения — force-checkout (FF/merge) затёр бы их молча
    /// (находка аудита). Сначала закоммитьте/отмените. Типизирована для понятного UI синхронизации.
    #[error("в рабочем дереве есть несохранённые изменения — закоммитьте или отмените их перед синхронизацией")]
    DirtyTree,
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

/// Исход pull (fetch + merge-analysis). Разрешение конфликта (нормальный merge) — Ф3-3b-3.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum PullOutcome {
    /// Локально уже актуально.
    UpToDate,
    /// Fast-forward применён (рабочее дерево обновлено).
    FastForward { oid: String },
    /// Нужен настоящий merge (расхождение истории) — возможен конфликт; решается отдельно (Ф3-3b-3).
    MergeRequired,
}

/// Один конфликтный файл (3-way). Содержимое — текст (markdown); `None` = файла нет в этой версии
/// (новый/удалённый). Бинарь представляем как пустую строку (UI покажет «бинарный»).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConflictFile {
    pub path: String,
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
}

/// Превью merge при расхождении истории. **In-memory** (`merge_commits`) — репозиторий и рабочее
/// дерево НЕ трогаются до явного `apply_merge`. `theirs` — oid их коммита (нужен для apply).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum MergePreview {
    /// Уже актуально — сливать нечего.
    UpToDate,
    /// Чистый merge без конфликтов — можно применить сразу (`apply_merge` с пустыми resolutions).
    Clean { theirs: String },
    /// Конфликты — нужен resolver.
    Conflicts {
        theirs: String,
        files: Vec<ConflictFile>,
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
            let Ok(path) = entry.path() else { continue };
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

    /// Авто-коммит ВСЕХ не-игнорируемых изменений (как раньше). Делегирует в [`Self::commit_selected`].
    pub fn commit_all(&self) -> GitResult<CommitOutcome> {
        self.commit_selected(None, None)
    }

    /// Коммит всех изменений с пользовательским сообщением (DP-10, макет sync.jsx);
    /// пустое/пробельное сообщение → авто-саммари как раньше.
    pub fn commit_all_with_message(&self, message: Option<&str>) -> GitResult<CommitOutcome> {
        self.commit_selected(None, message)
    }

    /// Выборочный коммит (#10): стейджит и коммитит ТОЛЬКО `paths` (пересечение с реальными изменениями
    /// из `status`); прочие изменения остаются НЕ закоммиченными (видны в следующем `status`). Secret-scan
    /// — по коммитимым файлам (секрет в НЕвыбранном файле не блокирует). Пустое пересечение (нечего из
    /// выбранного коммитить / устаревший выбор) → `NothingToCommit`. Пути — как в `status` (рел. от корня).
    pub fn commit_paths(&self, paths: &[String]) -> GitResult<CommitOutcome> {
        self.commit_selected(Some(paths), None)
    }

    /// Выборочный коммит с пользовательским сообщением (DP-10).
    pub fn commit_paths_with_message(
        &self,
        paths: &[String],
        message: Option<&str>,
    ) -> GitResult<CommitOutcome> {
        self.commit_selected(Some(paths), message)
    }

    /// Ядро коммита: стейджит изменения (все при `select=None`, иначе только пути из `select`),
    /// **сканирует коммитимое на секреты** (AC-SEC-3) — при находке коммит НЕ делается; иначе коммитит с
    /// авто-сообщением. Идемпотентно (нечего коммитить → `NothingToCommit`).
    fn commit_selected(
        &self,
        select: Option<&[String]>,
        message: Option<&str>,
    ) -> GitResult<CommitOutcome> {
        let mut status = self.status()?;
        if let Some(sel) = select {
            let set: HashSet<&str> = sel.iter().map(String::as_str).collect();
            status.retain(|e| set.contains(e.path.as_str()));
        }
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

        // Стейдж.
        let mut index = self.repo.index()?;
        match select {
            // Выборочно: индекс к HEAD (не тащим случайно застейдженное/невыбранное), затем только
            // выбранные пути: существующие → add_path, удалённые → remove_path.
            Some(_) => {
                if let Ok(tree) = self.repo.head().and_then(|h| h.peel_to_tree()) {
                    index.read_tree(&tree)?;
                }
                for e in &status {
                    let p = Path::new(&e.path);
                    if e.kind == ChangeKind::Deleted {
                        index.remove_path(p)?;
                    } else {
                        index.add_path(p)?;
                    }
                }
            }
            // Всё: add_all (new/modified, уважает .gitignore) + update_all (удаления tracked-файлов).
            None => {
                index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
                index.update_all(["*"].iter(), None)?;
            }
        }
        index.write()?;
        let tree = self.repo.find_tree(index.write_tree()?)?;

        let sig = self.signature()?;
        let message = message
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(String::from)
            .unwrap_or_else(|| auto_message(&status));
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

    /// Устанавливает URL remote `origin` (создаёт или переписывает).
    pub fn set_remote(&self, url: &str) -> GitResult<()> {
        if self.repo.find_remote("origin").is_ok() {
            self.repo.remote_set_url("origin", url)?;
        } else {
            self.repo.remote("origin", url)?;
        }
        Ok(())
    }

    /// URL remote `origin`, если задан.
    pub fn get_remote(&self) -> GitResult<Option<String>> {
        match self.repo.find_remote("origin") {
            Ok(r) => Ok(r.url().ok().map(str::to_string)),
            Err(_) => Ok(None),
        }
    }

    /// Имя текущей ветки (shorthand HEAD). Ошибка, если HEAD «unborn» (нет коммитов).
    fn current_branch(&self) -> GitResult<String> {
        let head = self.repo.head()?;
        Ok(head.shorthand().unwrap_or("main").to_string())
    }

    /// credentials-callback: токен как пароль https (GitHub PAT: username игнорируется). Замыкание
    /// владеет токеном → `'static`.
    fn auth_callbacks(token: &str) -> git2::RemoteCallbacks<'static> {
        let token = token.to_string();
        let mut cbs = git2::RemoteCallbacks::new();
        cbs.credentials(move |_url, _username, _allowed| {
            git2::Cred::userpass_plaintext("x-access-token", &token)
        });
        cbs
    }

    /// Push текущей ветки в `origin` по https-токену. Требует хотя бы один коммит.
    pub fn push(&self, token: &str) -> GitResult<()> {
        let branch = self.current_branch()?;
        let mut remote = self.repo.find_remote("origin")?;
        let mut opts = git2::PushOptions::new();
        opts.remote_callbacks(Self::auth_callbacks(token));
        let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
        remote.push(&[&refspec], Some(&mut opts))?;
        Ok(())
    }

    /// Гард перед force-checkout (FF/merge): нет ли несохранённых правок ОТСЛЕЖИВАЕМЫХ файлов, которые
    /// `checkout_head` с `.force()` молча затёр бы (находка аудита — потеря данных при sync). Блокируем
    /// только Modified/Deleted/Renamed — их перезапишет checkout; новые (untracked) файлы checkout
    /// сохраняет, поэтому они синхронизации не мешают (иначе любая несохранённая заметка ломала бы pull).
    fn ensure_clean_tree(&self) -> GitResult<()> {
        let dirty = self.status()?.iter().any(|e| {
            matches!(
                e.kind,
                ChangeKind::Modified | ChangeKind::Deleted | ChangeKind::Renamed
            )
        });
        if dirty {
            Err(GitError::DirtyTree)
        } else {
            Ok(())
        }
    }

    /// Pull: fetch `origin/<branch>` + merge-analysis → up-to-date / fast-forward (применяется) /
    /// merge-required (расхождение истории — разрешение в Ф3-3b-3, тут только сигнал).
    pub fn pull(&self, token: &str) -> GitResult<PullOutcome> {
        let branch = self.current_branch()?;
        let mut remote = self.repo.find_remote("origin")?;
        let mut fo = git2::FetchOptions::new();
        fo.remote_callbacks(Self::auth_callbacks(token));
        remote.fetch(&[&branch], Some(&mut fo), None)?;

        let fetch_head = self.repo.find_reference("FETCH_HEAD")?;
        let fetch_commit = self.repo.reference_to_annotated_commit(&fetch_head)?;
        let (analysis, _) = self.repo.merge_analysis(&[&fetch_commit])?;

        if analysis.is_up_to_date() {
            return Ok(PullOutcome::UpToDate);
        }
        if analysis.is_fast_forward() {
            self.ensure_clean_tree()?; // не затирать несохранённые правки force-checkout'ом (аудит)
            let refname = format!("refs/heads/{branch}");
            let mut reference = self.repo.find_reference(&refname)?;
            reference.set_target(fetch_commit.id(), "fast-forward")?;
            self.repo.set_head(&refname)?;
            self.repo
                .checkout_head(Some(git2::build::CheckoutBuilder::default().force()))?;
            return Ok(PullOutcome::FastForward {
                oid: fetch_commit.id().to_string(),
            });
        }
        Ok(PullOutcome::MergeRequired)
    }

    /// Превью merge с уже известным «их» коммитом — **in-memory** (`merge_commits`), без сети и без
    /// мутаций репозитория/рабочего дерева. Ядро для теста и `merge_preview`.
    fn merge_with(&self, their: &git2::Commit) -> GitResult<MergePreview> {
        let our = self.repo.head()?.peel_to_commit()?;
        if our.id() == their.id() || self.repo.graph_descendant_of(our.id(), their.id())? {
            return Ok(MergePreview::UpToDate);
        }
        let index = self.repo.merge_commits(&our, their, None)?;
        let theirs = their.id().to_string();
        if !index.has_conflicts() {
            return Ok(MergePreview::Clean { theirs });
        }
        let blob_text = |entry: Option<&git2::IndexEntry>| -> Option<String> {
            let e = entry?;
            let blob = self.repo.find_blob(e.id).ok()?;
            Some(String::from_utf8_lossy(blob.content()).into_owned())
        };
        let mut files = Vec::new();
        for c in index.conflicts()? {
            let c = c?;
            let path_bytes = c
                .our
                .as_ref()
                .or(c.their.as_ref())
                .or(c.ancestor.as_ref())
                .map(|e| e.path.clone())
                .unwrap_or_default();
            files.push(ConflictFile {
                path: String::from_utf8_lossy(&path_bytes).into_owned(),
                base: blob_text(c.ancestor.as_ref()),
                ours: blob_text(c.our.as_ref()),
                theirs: blob_text(c.their.as_ref()),
            });
        }
        Ok(MergePreview::Conflicts { theirs, files })
    }

    /// Превью merge с `origin/<branch>`: fetch + `merge_with`. Ничего не применяет (это `apply_merge`).
    pub fn merge_preview(&self, token: &str) -> GitResult<MergePreview> {
        let branch = self.current_branch()?;
        let mut remote = self.repo.find_remote("origin")?;
        let mut fo = git2::FetchOptions::new();
        fo.remote_callbacks(Self::auth_callbacks(token));
        remote.fetch(&[&branch], Some(&mut fo), None)?;
        let fetch_head = self.repo.find_reference("FETCH_HEAD")?;
        let their = fetch_head.peel_to_commit()?;
        self.merge_with(&their)
    }

    /// Применяет merge: пере-сливает (in-memory) HEAD с `their_oid`, накладывает `resolutions`
    /// (`path` → итоговое содержимое) на конфликтные файлы, проверяет отсутствие остаточных
    /// конфликтов, создаёт merge-коммит (2 родителя), двигает ветку и форс-чекаутит рабочее дерево.
    /// Возвращает oid коммита. **Атомарно** — до этого вызова репозиторий не в состоянии merge.
    pub fn apply_merge(
        &self,
        their_oid: &str,
        resolutions: &[(String, String)],
    ) -> GitResult<String> {
        self.ensure_clean_tree()?; // правки, сделанные во время разрешения конфликта, не теряем (аудит)
        let their = self.repo.find_commit(git2::Oid::from_str(their_oid)?)?;
        let our = self.repo.head()?.peel_to_commit()?;
        let mut index = self.repo.merge_commits(&our, &their, None)?;

        for (path, content) in resolutions {
            let blob = self.repo.blob(content.as_bytes())?;
            // Снимаем все стадии конфликта (1/2/3) для пути, затем кладём разрешённую стадию 0.
            index.remove_path(Path::new(path)).ok();
            let entry = git2::IndexEntry {
                ctime: git2::IndexTime::new(0, 0),
                mtime: git2::IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode: 0o100644,
                uid: 0,
                gid: 0,
                file_size: content.len() as u32,
                id: blob,
                flags: 0,
                flags_extended: 0,
                path: path.clone().into_bytes(),
            };
            index.add(&entry)?;
        }

        if index.has_conflicts() {
            return Err(GitError::Git(git2::Error::from_str(
                "остались неразрешённые конфликты",
            )));
        }

        let tree = self.repo.find_tree(index.write_tree_to(&self.repo)?)?;
        let sig = self.signature()?;
        let branch = self.current_branch()?;
        let refname = format!("refs/heads/{branch}");
        let message = format!(
            "Merge origin/{branch} (resolved {} conflict(s))",
            resolutions.len()
        );
        let oid = self
            .repo
            .commit(Some(&refname), &sig, &sig, &message, &tree, &[&our, &their])?;
        self.repo.set_head(&refname)?;
        self.repo
            .checkout_head(Some(git2::build::CheckoutBuilder::default().force()))?;
        Ok(oid.to_string())
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

    /// #10: выборочный коммит стейджит ТОЛЬКО выбранные пути; прочее остаётся; устаревший выбор → no-op.
    #[test]
    fn commit_paths_stages_only_selected() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let git = GitSync::open_or_init(root).unwrap();
        git.ensure_gitignore().unwrap();
        std::fs::write(root.join("a.md"), "# A").unwrap();
        std::fs::write(root.join("b.md"), "# B").unwrap();

        // Коммитим только a.md → b.md и .gitignore остаются не закоммиченными.
        match git.commit_paths(&["a.md".into()]).unwrap() {
            CommitOutcome::Committed { files, .. } => assert_eq!(files, 1, "только a.md"),
            other => panic!("ожидали Committed, получили {other:?}"),
        }
        let pending: Vec<String> = git.status().unwrap().into_iter().map(|e| e.path).collect();
        assert!(!pending.contains(&"a.md".to_string()), "a.md закоммичен");
        assert!(pending.contains(&"b.md".to_string()), "b.md остался");
        assert!(
            pending.iter().any(|p| p == ".gitignore"),
            ".gitignore остался"
        );

        // Устаревший / несовпавший выбор → нечего коммитить.
        assert_eq!(
            git.commit_paths(&["ghost.md".into()]).unwrap(),
            CommitOutcome::NothingToCommit
        );
    }

    /// #10: выборочный коммит коммитит удаление; секрет в НЕвыбранном файле не блокирует, в выбранном — да.
    #[test]
    fn commit_paths_handles_delete_and_scopes_secret_scan() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let git = GitSync::open_or_init(root).unwrap();
        git.ensure_gitignore().unwrap();
        std::fs::write(root.join("keep.md"), "# keep").unwrap();
        std::fs::write(root.join("drop.md"), "# drop").unwrap();
        git.commit_all().unwrap(); // обе в истории

        // Удаляем drop.md и кладём секрет в keep.md.
        std::fs::remove_file(root.join("drop.md")).unwrap();
        std::fs::write(
            root.join("keep.md"),
            "ключ: sk-ABCDEFGHIJKLMNOPQRSTUVWX1234567890",
        )
        .unwrap();

        // Выбор только удаления drop.md → секрет в keep.md (не выбран) НЕ блокирует.
        match git.commit_paths(&["drop.md".into()]).unwrap() {
            CommitOutcome::Committed { files, .. } => assert_eq!(files, 1, "удаление закоммичено"),
            other => panic!("ожидали Committed, получили {other:?}"),
        }
        assert!(!root.join("drop.md").exists());
        assert!(
            git.status().unwrap().iter().any(|e| e.path == "keep.md"),
            "правка keep.md ещё не закоммичена"
        );

        // Теперь выбор keep.md с секретом → блок (коммит не сделан).
        match git.commit_paths(&["keep.md".into()]).unwrap() {
            CommitOutcome::BlockedBySecrets { findings } => {
                assert!(findings.iter().any(|f| f.path == "keep.md"));
            }
            other => panic!("ожидали BlockedBySecrets, получили {other:?}"),
        }
    }

    #[test]
    fn set_and_get_remote_origin() {
        let dir = TempDir::new().unwrap();
        let git = GitSync::open_or_init(dir.path()).unwrap();
        assert_eq!(git.get_remote().unwrap(), None);

        git.set_remote("https://example.com/vault.git").unwrap();
        assert_eq!(
            git.get_remote().unwrap().as_deref(),
            Some("https://example.com/vault.git")
        );

        // Повторный set — переписывает (не дублирует remote).
        git.set_remote("https://example.com/other.git").unwrap();
        assert_eq!(
            git.get_remote().unwrap().as_deref(),
            Some("https://example.com/other.git")
        );
    }

    /// Ф4-8: 3-way merge resolver. Строим реальный конфликт (base→ours / base→theirs по одной строке),
    /// проверяем превью (ours/theirs/base) и применение резолва (merge-коммит, рабочий файл, 2 родителя).
    #[test]
    fn merge_conflict_preview_and_resolve() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let gs = GitSync::open_or_init(&root).unwrap();
        let repo = &gs.repo;
        let sig = git2::Signature::now("T", "t@t").unwrap();

        let commit_head = |content: &str, parents: &[&git2::Commit], msg: &str| -> git2::Oid {
            std::fs::write(root.join("a.md"), content).unwrap();
            let mut idx = repo.index().unwrap();
            idx.add_path(Path::new("a.md")).unwrap();
            idx.write().unwrap();
            let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, parents)
                .unwrap()
        };

        // base → ours (HEAD движется по ветке).
        let base = repo
            .find_commit(commit_head("line1\n", &[], "base"))
            .unwrap();
        commit_head("OUR\n", &[&base], "ours");

        // theirs — сиблинг от base, БЕЗ мутаций рабочего дерева (treebuilder).
        let their_blob = repo.blob("THEIR\n".as_bytes()).unwrap();
        let mut tb = repo.treebuilder(Some(&base.tree().unwrap())).unwrap();
        tb.insert("a.md", their_blob, 0o100644).unwrap();
        let their_tree = repo.find_tree(tb.write().unwrap()).unwrap();
        let their_oid = repo
            .commit(None, &sig, &sig, "theirs", &their_tree, &[&base])
            .unwrap();
        let their_commit = repo.find_commit(their_oid).unwrap();

        // Превью → конфликт по a.md с тремя версиями.
        match gs.merge_with(&their_commit).unwrap() {
            MergePreview::Conflicts { files, .. } => {
                assert_eq!(files.len(), 1);
                assert_eq!(files[0].path, "a.md");
                assert_eq!(files[0].base.as_deref(), Some("line1\n"));
                assert_eq!(files[0].ours.as_deref(), Some("OUR\n"));
                assert_eq!(files[0].theirs.as_deref(), Some("THEIR\n"));
            }
            other => panic!("ожидали Conflicts, получили {other:?}"),
        }

        // Резолв → merge-коммит, рабочий файл = RESOLVED, 2 родителя, без остаточных конфликтов.
        let oid = gs
            .apply_merge(
                &their_oid.to_string(),
                &[("a.md".to_string(), "RESOLVED\n".to_string())],
            )
            .unwrap();
        // Нормализуем CRLF: на Windows-раннере git core.autocrlf переписывает рабочий файл при
        // checkout (это политика git, не баг merge) → сверяем содержимое независимо от EOL.
        assert_eq!(
            std::fs::read_to_string(root.join("a.md"))
                .unwrap()
                .replace("\r\n", "\n"),
            "RESOLVED\n"
        );
        let merged = repo
            .find_commit(git2::Oid::from_str(&oid).unwrap())
            .unwrap();
        assert_eq!(merged.parent_count(), 2);

        // Теперь мы содержим их коммит → повторное превью «уже актуально».
        assert_eq!(
            gs.merge_with(&their_commit).unwrap(),
            MergePreview::UpToDate
        );
    }
}
