//! git-sync (Фаза 3, §8): vault как git-репозиторий. Это **фундамент** — локальные операции:
//! open/init, управляемый `.gitignore`, `status`. Выборочный коммит + secret-scan — Ф3-2;
//! pull/push + разрешение конфликтов (диск vs грязный буфер) — Ф3-3.
//!
//! Весь libgit2-I/O синхронный → из Tauri-команд вызывается в `spawn_blocking` (как §8 «всё в
//! spawn_blocking»). git-sync — **core module**, НЕ sandbox-плагин (ADR/§8).

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
}
