//! Файловый watcher (§4.2, Б9): `notify-debouncer-full` + ignore-список + нормализация
//! событий ПО ПУТИ.
//!
//! Игнор обязателен: `nexus.db` лежит ВНУТРИ vault и постоянно пишет `*.db-wal/-shm`;
//! рекурсивный watcher без фильтра словил бы свои же записи (цикл реиндексации) — AC-Б9-2.
//! Нормализация схлопывает шторм и пару remove+create (atomic-save tmp→rename) в один
//! `Upsert` по пути (AC-Б9-3); стабильность `file_id` обеспечивает UPSERT индексатора (AC-Б9-1).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify_debouncer_full::notify::event::ModifyKind;
use notify_debouncer_full::notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use notify_debouncer_full::{
    new_debouncer, DebounceEventResult, DebouncedEvent, Debouncer, FileIdMap,
};
use tokio::sync::mpsc::UnboundedSender;

/// Нормализованное событие vault (по итоговому состоянию пути, не по «сырому» событию ФС).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultEvent {
    /// Файл создан или изменён (целевая часть atomic-save тоже сюда).
    Upsert(PathBuf),
    /// Файл удалён.
    Deleted(PathBuf),
}

/// Сырое изменение до нормализации (промежуточное, тестируемое представление).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawChange {
    Created(PathBuf),
    Modified(PathBuf),
    Removed(PathBuf),
}

/// Должен ли путь игнорироваться watcher'ом: служебные каталоги `.nexus`/`.git`,
/// файлы БД (`*.db`, `*.db-wal`, `*.db-shm`), прочие dotfiles и `.conflict`.
pub fn is_ignored(path: &Path) -> bool {
    let in_service_dir = path.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s == ".nexus" || s == ".git"
    });
    if in_service_dir {
        return true;
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if name.starts_with('.') || name.ends_with(".conflict") {
            return true;
        }
    }
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if ext == "db" || ext.starts_with("db-") {
            return true;
        }
    }
    false
}

/// Нормализует пачку сырых изменений в события по пути. Последнее состояние пути
/// побеждает (remove→create = `Upsert`; create→remove = `Deleted`); дубли схлопываются.
pub fn normalize(changes: &[RawChange]) -> Vec<VaultEvent> {
    let mut exists: BTreeMap<PathBuf, bool> = BTreeMap::new();
    for change in changes {
        match change {
            RawChange::Created(p) | RawChange::Modified(p) => {
                exists.insert(p.clone(), true);
            }
            RawChange::Removed(p) => {
                exists.insert(p.clone(), false);
            }
        }
    }
    exists
        .into_iter()
        .map(|(path, exists)| {
            if exists {
                VaultEvent::Upsert(path)
            } else {
                VaultEvent::Deleted(path)
            }
        })
        .collect()
}

/// Преобразует одно debounced-событие в сырые изменения, отбрасывая игнорируемые пути.
fn to_raw_changes(event: &DebouncedEvent) -> Vec<RawChange> {
    let keep = |p: &PathBuf| !is_ignored(p);
    match event.kind {
        EventKind::Create(_) => event
            .paths
            .iter()
            .filter(|p| keep(p))
            .map(|p| RawChange::Created(p.clone()))
            .collect(),
        EventKind::Remove(_) => event
            .paths
            .iter()
            .filter(|p| keep(p))
            .map(|p| RawChange::Removed(p.clone()))
            .collect(),
        // Переименование: новый путь существует → Created, старый исчез → Removed.
        EventKind::Modify(ModifyKind::Name(_)) => event
            .paths
            .iter()
            .filter(|p| keep(p))
            .map(|p| {
                if p.exists() {
                    RawChange::Created(p.clone())
                } else {
                    RawChange::Removed(p.clone())
                }
            })
            .collect(),
        EventKind::Modify(_) => event
            .paths
            .iter()
            .filter(|p| keep(p))
            .map(|p| RawChange::Modified(p.clone()))
            .collect(),
        _ => Vec::new(),
    }
}

/// Watcher vault: один debouncer (400 мс) на рекурсивный обход; нормализованные события
/// уходят в `tx`. Держите возвращённое значение живым — на дропе watcher останавливается.
pub struct VaultWatcher {
    _debouncer: Debouncer<RecommendedWatcher, FileIdMap>,
}

impl VaultWatcher {
    /// Запускает наблюдение за `root`, шлёт `VaultEvent` в `tx`.
    pub fn new(
        root: &Path,
        tx: UnboundedSender<VaultEvent>,
    ) -> notify_debouncer_full::notify::Result<Self> {
        let mut debouncer = new_debouncer(
            Duration::from_millis(400),
            None,
            move |result: DebounceEventResult| {
                if let Ok(events) = result {
                    let changes: Vec<RawChange> = events.iter().flat_map(to_raw_changes).collect();
                    for event in normalize(&changes) {
                        let _ = tx.send(event);
                    }
                }
            },
        )?;
        debouncer.watcher().watch(root, RecursiveMode::Recursive)?;
        debouncer.cache().add_root(root, RecursiveMode::Recursive);
        Ok(Self {
            _debouncer: debouncer,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AC-Б9-2: записи в служебные пути не порождают событий индексации.
    #[test]
    fn ignores_service_paths() {
        assert!(is_ignored(Path::new("/vault/.nexus/nexus.db")));
        assert!(is_ignored(Path::new("/vault/.nexus/nexus.db-wal")));
        assert!(is_ignored(Path::new("/vault/.nexus/nexus.db-shm")));
        assert!(is_ignored(Path::new("/vault/.git/HEAD")));
        assert!(is_ignored(Path::new("/vault/Notes/.hidden.md")));
        assert!(is_ignored(Path::new("/vault/Notes/A.md.conflict")));

        assert!(!is_ignored(Path::new("/vault/Notes/A.md")));
        assert!(!is_ignored(Path::new("/vault/Projects/Plan.md")));
    }

    /// AC-Б9-3: шторм событий схлопывается; пара remove+create по одному пути → один Upsert.
    #[test]
    fn normalizes_storm_and_atomic_save() {
        let p = PathBuf::from("/vault/Note.md");
        // atomic-save: remove(старый) затем create(новый) того же пути + лишний Modify-шум
        let changes = vec![
            RawChange::Modified(p.clone()),
            RawChange::Removed(p.clone()),
            RawChange::Created(p.clone()),
        ];
        assert_eq!(normalize(&changes), vec![VaultEvent::Upsert(p.clone())]);

        // несколько Modify одного пути → один Upsert
        let many = vec![
            RawChange::Modified(p.clone()),
            RawChange::Modified(p.clone()),
            RawChange::Modified(p.clone()),
        ];
        assert_eq!(normalize(&many), vec![VaultEvent::Upsert(p.clone())]);
    }

    #[test]
    fn normalizes_delete_and_create_then_remove() {
        let a = PathBuf::from("/vault/A.md");
        let b = PathBuf::from("/vault/B.md");
        let changes = vec![
            RawChange::Removed(a.clone()), // только удаление
            RawChange::Created(b.clone()), // создан…
            RawChange::Removed(b.clone()), // …затем удалён → Deleted
        ];
        let mut got = normalize(&changes);
        got.sort_by(|x, y| format!("{x:?}").cmp(&format!("{y:?}")));
        assert!(got.contains(&VaultEvent::Deleted(a)));
        assert!(got.contains(&VaultEvent::Deleted(b)));
    }
}
