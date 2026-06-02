//! Vault: файловая система хранилища заметок (ленивый листинг + единая канонизация путей).
//!
//! `list_dir` отдаёт содержимое ОДНОГО каталога (ленивость — не 50k одним IPC, §4.1/§10).
//! [`resolve_vault_path`] — единственная точка канонизации/анти-traversal для всех
//! host-функций и Tauri-команд (§7.4, AC-SEC-1).

use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

/// Ошибки работы с vault.
#[derive(Debug, Error)]
pub enum VaultError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("путь вне vault заблокирован (traversal/симлинк)")]
    PathEscape,

    #[error("не каталог: {0}")]
    NotADir(String),
}

/// Результат vault-операций.
pub type VaultResult<T> = Result<T, VaultError>;

/// Запись файлового дерева для ленивого `list_dir`. Сериализуется в camelCase под фронт.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    /// Имя (последний компонент пути).
    pub name: String,
    /// Путь относительно корня vault, всегда с разделителем `/`.
    pub path: String,
    pub is_dir: bool,
    /// Для каталогов: есть ли внутри неигнорируемые элементы (affordance раскрытия).
    pub has_children: bool,
    /// Размер файла в байтах (для каталога — 0).
    pub size_bytes: u64,
}

/// Сведения об открытом vault.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultInfo {
    pub root: String,
    pub name: String,
}

/// Скрываемые в дереве элементы: служебные каталоги (`.nexus`/`.git`), прочие dotfiles
/// и merge-конфликты. (Watcher §4.2 дополнительно игнорит `*.db*` — это его забота.)
pub fn is_ignored(name: &str) -> bool {
    name.starts_with('.') || name.ends_with(".conflict")
}

/// Канонизирует `rel` относительно `root` и проверяет, что результат ВНУТРИ vault.
///
/// Резолвит `..` и симлинки (`canonicalize`), блокирует абсолютные пути и побег наружу
/// (`../../.ssh`, симлинк за пределы). Единая граница для всех команд/host-функций (AC-SEC-1).
/// `root` должен быть уже канонизирован (это делает `open_vault`).
pub fn resolve_vault_path(root: &Path, rel: &Path) -> VaultResult<PathBuf> {
    if rel.is_absolute() {
        return Err(VaultError::PathEscape);
    }
    let full = root.join(rel).canonicalize()?;
    if !full.starts_with(root) {
        return Err(VaultError::PathEscape);
    }
    Ok(full)
}

/// Лёгкая ссылка на заметку (для автокомплита `[[wikilink]]` и поиска).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteRef {
    pub path: String,
    pub title: Option<String>,
}

/// Канонизация пути для ЗАПИСИ: целевой файл может не существовать, поэтому канонизируем
/// РОДИТЕЛЯ (он обязан существовать) и проверяем его принадлежность vault; имя добавляем
/// после. Та же анти-traversal граница, что и [`resolve_vault_path`] (AC-SEC-1).
pub fn resolve_vault_path_for_write(root: &Path, rel: &Path) -> VaultResult<PathBuf> {
    if rel.is_absolute() {
        return Err(VaultError::PathEscape);
    }
    let full = root.join(rel);
    let file_name = full.file_name().ok_or(VaultError::PathEscape)?.to_owned();
    let parent = full.parent().ok_or(VaultError::PathEscape)?;
    let parent_canon = parent.canonicalize()?;
    if !parent_canon.starts_with(root) {
        return Err(VaultError::PathEscape);
    }
    Ok(parent_canon.join(file_name))
}

/// Имя vault = имя его корневого каталога.
pub fn vault_name(root: &Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned())
}

/// Ленивый листинг каталога `rel` (пустая строка = корень vault). Скрывает игнорируемые
/// элементы; вложенные каталоги НЕ раскрываются (их грузит отдельный `list_dir`).
pub fn list_dir(root: &Path, rel: &Path) -> VaultResult<Vec<FileEntry>> {
    let dir = resolve_vault_path(root, rel)?;
    if !dir.is_dir() {
        return Err(VaultError::NotADir(dir.to_string_lossy().into_owned()));
    }

    let mut entries = Vec::new();
    for de in std::fs::read_dir(&dir)? {
        let de = de?;
        let raw_name = de.file_name();
        let name = raw_name.to_string_lossy();
        if is_ignored(&name) {
            continue;
        }
        let file_type = de.file_type()?;
        let is_dir = file_type.is_dir();
        let abs = de.path();
        let rel_path = abs.strip_prefix(root).unwrap_or(&abs);
        let path = to_unix(rel_path);

        let (has_children, size_bytes) = if is_dir {
            (dir_has_children(&abs), 0)
        } else {
            (false, de.metadata().map(|m| m.len()).unwrap_or(0))
        };

        entries.push(FileEntry {
            name: name.into_owned(),
            path,
            is_dir,
            has_children,
            size_bytes,
        });
    }
    Ok(entries)
}

/// Есть ли в каталоге хотя бы один неигнорируемый элемент (короткое замыкание на первом).
fn dir_has_children(dir: &Path) -> bool {
    match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .any(|e| !is_ignored(&e.file_name().to_string_lossy())),
        Err(_) => false,
    }
}

/// Относительный путь → строка с разделителем `/` (на Windows меняет `\` на `/`).
fn to_unix(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_vault() -> TempDir {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("Notes")).unwrap();
        fs::write(root.join("Notes/A.md"), "# A").unwrap();
        fs::create_dir(root.join("Notes/Sub")).unwrap();
        fs::write(root.join("Notes/Sub/B.md"), "# B").unwrap();
        fs::write(root.join("root.md"), "# root").unwrap();
        fs::create_dir(root.join(".nexus")).unwrap(); // служебный — скрыт
        fs::write(root.join(".hidden"), "x").unwrap(); // dotfile — скрыт
        fs::create_dir(root.join("Empty")).unwrap();
        dir
    }

    #[test]
    fn lists_root_lazily_hiding_ignored() {
        let dir = make_vault();
        let root = dir.path().canonicalize().unwrap();
        let mut entries = list_dir(&root, Path::new("")).unwrap();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Empty", "Notes", "root.md"]); // .nexus/.hidden скрыты

        let notes = entries.iter().find(|e| e.name == "Notes").unwrap();
        assert!(notes.is_dir && notes.has_children);
        let empty = entries.iter().find(|e| e.name == "Empty").unwrap();
        assert!(empty.is_dir && !empty.has_children);
        let root_md = entries.iter().find(|e| e.name == "root.md").unwrap();
        assert!(!root_md.is_dir && root_md.size_bytes > 0);
    }

    #[test]
    fn lists_subdir_only_not_recursive() {
        let dir = make_vault();
        let root = dir.path().canonicalize().unwrap();
        let entries = list_dir(&root, Path::new("Notes")).unwrap();
        let names: std::collections::HashSet<_> = entries.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains("A.md"));
        assert!(names.contains("Sub"));
        assert!(!names.contains("B.md")); // ленивость: вложенное не возвращается
        let a = entries.iter().find(|e| e.name == "A.md").unwrap();
        assert_eq!(a.path, "Notes/A.md"); // относительный, '/'
    }

    /// AC-SEC-1 (часть для vault-команд): traversal и абсолютные пути отклонены.
    #[test]
    fn resolve_blocks_traversal_and_absolute() {
        let dir = make_vault();
        let root = dir.path().canonicalize().unwrap();
        assert!(resolve_vault_path(&root, Path::new("Notes/A.md")).is_ok());
        assert!(resolve_vault_path(&root, Path::new("../../etc/passwd")).is_err());
        assert!(resolve_vault_path(&root, Path::new("/etc/passwd")).is_err());
        assert!(matches!(
            resolve_vault_path(&root, Path::new("/etc/passwd")),
            Err(VaultError::PathEscape)
        ));
    }

    /// Запись: новый файл в существующем каталоге vault — ок; побег наружу — отказ.
    #[test]
    fn write_resolve_allows_new_file_blocks_escape() {
        let dir = make_vault();
        let root = dir.path().canonicalize().unwrap();
        assert!(resolve_vault_path_for_write(&root, Path::new("Notes/New.md")).is_ok());
        assert!(resolve_vault_path_for_write(&root, Path::new("../escape.md")).is_err());
        assert!(resolve_vault_path_for_write(&root, Path::new("/tmp/x.md")).is_err());
    }
}
