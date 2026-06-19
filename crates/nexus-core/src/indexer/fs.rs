//! Файловые помощники индексатора: обход vault за .md и нормализация путей/времени.

use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::watcher;

/// Рекурсивно собирает относительные пути всех .md под `dir` (пропуская игнорируемые — `.nexus/` и т.п.).
pub(super) fn collect_md(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if watcher::is_ignored(&path) {
            continue;
        }
        if path.is_dir() {
            collect_md(root, &path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Some(rel) = rel_of(root, &path) {
                out.push(rel);
            }
        }
    }
}

/// Относительный путь `abs` от корня vault в POSIX-форме (`\`→`/`), либо `None` (вне корня).
pub(super) fn rel_of(root: &Path, abs: &Path) -> Option<String> {
    abs.strip_prefix(root)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
}

/// mtime файла в секундах эпохи (0 при недоступности — безопасный фолбэк для mtime-шортката).
pub(super) fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Текущее время в секундах эпохи (0 при сбое часов).
pub(super) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
