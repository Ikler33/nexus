//! Локальная история версий заметок (SAFE-5): снапшоты в `.nexus/history/<rel>/<unixms>.md`.
//!
//! Сохраняем точки восстановления, чтобы правка/перезапись не была необратимой (последний камень
//! фундамента доверия P1). `.nexus` игнорируется вотчером и листингом ([`super::is_ignored`]) → снапшоты
//! не плодят реиндексацию и не видны в дереве. Снапшоты пишутся тем же атомарным [`super::atomic_write`].
//!
//! Политика (решения плана): дедуп по контенту (идентичный последнему снапшоту — пропускаем);
//! троттл автосейва ≤1 снапшот/90с (ручной Ctrl-S/палитра — всегда при изменении); ретенция —
//! последние [`MAX_SNAPSHOTS`] ∪ всё за [`KEEP_DAYS`] дней. БД-индекс не нужен (файлы = истина, мандат 6).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::{atomic_write, VaultError, VaultResult};

/// Корень истории внутри vault.
const HISTORY_ROOT: &str = ".nexus/history";
/// Троттл автоснапшота: не чаще одного за столько секунд (ручной save игнорирует).
const AUTO_THROTTLE_SECS: u64 = 90;
/// Сколько последних снапшотов хранить всегда (сверх 7-дневного окна).
const MAX_SNAPSHOTS: usize = 50;
/// Сколько дней хранить все снапшоты (сверх последних [`MAX_SNAPSHOTS`]).
const KEEP_DAYS: u64 = 7;

/// Метаданные снапшота для UI (camelCase под фронт).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotMeta {
    /// Unix-время снапшота в миллисекундах (= имя файла).
    pub ts: u64,
    /// Размер снапшота в байтах.
    pub size: u64,
}

/// `.nexus/history/<rel>/` для заметки `rel` (структура vault сохранена внутри истории).
fn history_dir(root: &Path, rel: &str) -> PathBuf {
    root.join(HISTORY_ROOT).join(rel)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Снапшоты заметки по убыванию времени (новейший первым). Нет каталога → пусто.
pub fn list_snapshots(root: &Path, rel: &str) -> VaultResult<Vec<SnapshotMeta>> {
    let dir = history_dir(root, rel);
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(&dir) {
        Ok(r) => r,
        Err(_) => return Ok(out), // каталога нет — истории ещё нет
    };
    for de in rd.flatten() {
        let name = de.file_name().to_string_lossy().into_owned();
        if let Some(stem) = name.strip_suffix(".md") {
            if let Ok(ts) = stem.parse::<u64>() {
                let size = de.metadata().map(|m| m.len()).unwrap_or(0);
                out.push(SnapshotMeta { ts, size });
            }
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.ts));
    Ok(out)
}

/// Содержимое снапшота по его `ts`.
pub fn read_snapshot(root: &Path, rel: &str, ts: u64) -> VaultResult<String> {
    let path = history_dir(root, rel).join(format!("{ts}.md"));
    Ok(std::fs::read_to_string(path)?)
}

/// Записать снапшот заметки, если есть смысл: контент отличается от последнего снапшота И
/// (`manual` ИЛИ прошло ≥[`AUTO_THROTTLE_SECS`] с последнего). Затем GC. Дедуп/троттл → `Ok(())`
/// без записи (не ошибка). Best-effort у вызывающего: сбой истории не должен валить сам save.
pub fn snapshot(root: &Path, rel: &str, content: &str, manual: bool) -> VaultResult<()> {
    let snaps = list_snapshots(root, rel)?; // по убыванию ts
    if let Some(last) = snaps.first() {
        // Дедуп: идентичный последнему снапшоту контент — не плодим копию.
        if read_snapshot(root, rel, last.ts).unwrap_or_default() == content {
            return Ok(());
        }
        // Троттл автосейва (ручной save фиксирует точку всегда при изменении контента).
        if !manual && now_ms().saturating_sub(last.ts) < AUTO_THROTTLE_SECS * 1000 {
            return Ok(());
        }
    }
    let dir = history_dir(root, rel);
    std::fs::create_dir_all(&dir)?;
    // Уникальный ts: два снапшота одной заметки в одну мс не должны затирать друг друга. Резервируем
    // имя АТОМАРНО через O_EXCL (create_new) — `exists()`-проверка была TOCTOU: два потока выбирали
    // один ts и второй atomic_write затирал первый снапшот (находка аудита). Занят → +1 и повтор.
    let mut ts = now_ms();
    let target = loop {
        let p = dir.join(format!("{ts}.md"));
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&p)
        {
            Ok(_) => break p, // имя ts закреплено за нами; ниже atomic_write заменит пустышку контентом
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                ts += 1;
                continue;
            }
            Err(e) => return Err(VaultError::Io(e)),
        }
    };
    atomic_write(&target, content.as_bytes())?;
    gc(root, rel)?;
    Ok(())
}

/// Ретенция: удаляем снапшот, только если он И за пределами последних [`MAX_SNAPSHOTS`], И старше
/// [`KEEP_DAYS`] дней (т.е. храним объединение «последние N» ∪ «за 7 дней»).
fn gc(root: &Path, rel: &str) -> VaultResult<()> {
    let snaps = list_snapshots(root, rel)?; // по убыванию ts
    let cutoff = now_ms().saturating_sub(KEEP_DAYS * 24 * 3600 * 1000);
    let dir = history_dir(root, rel);
    for (i, s) in snaps.iter().enumerate() {
        if i >= MAX_SNAPSHOTS && s.ts < cutoff {
            let _ = std::fs::remove_file(dir.join(format!("{}.md", s.ts)));
        }
    }
    Ok(())
}

/// Переносит каталог истории `.nexus/history/<from_rel>` → `<to_rel>` при rename/move заметки или
/// поддерева (CURATE-2): иначе история версий теряет связь с новым путём (SAFE-5/6). Один rename
/// поддерева покрывает и файл (`<rel>` = `.nexus/history/<file>`), и каталог (всё поддерево разом).
/// Нет истории у `from` — no-op (`Ok`). Цель занята — best-effort удаляем старую перед переносом
/// (история по новому пути авторитетнее древней осиротевшей).
pub fn move_history(root: &Path, from_rel: &str, to_rel: &str) -> VaultResult<()> {
    if from_rel == to_rel {
        return Ok(());
    }
    let from_dir = history_dir(root, from_rel);
    if !from_dir.exists() {
        return Ok(());
    }
    let to_dir = history_dir(root, to_rel);
    if let Some(parent) = to_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if to_dir.exists() {
        std::fs::remove_dir_all(&to_dir)?;
    }
    std::fs::rename(&from_dir, &to_dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn snapshot_creates_dedups_and_reads_back() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        // Первый снапшот.
        snapshot(root, "Notes/A.md", "v1", true).unwrap();
        let snaps = list_snapshots(root, "Notes/A.md").unwrap();
        assert_eq!(snaps.len(), 1);
        assert_eq!(
            read_snapshot(root, "Notes/A.md", snaps[0].ts).unwrap(),
            "v1"
        );

        // Идентичный контент — дедуп, второго снапшота нет.
        snapshot(root, "Notes/A.md", "v1", true).unwrap();
        assert_eq!(list_snapshots(root, "Notes/A.md").unwrap().len(), 1);

        // .nexus-история игнорируется деревом/вотчером.
        assert!(super::super::is_ignored(".nexus"));
    }

    #[test]
    fn move_history_follows_rename_and_clears_orphan() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        snapshot(root, "Notes/A.md", "v1", true).unwrap();
        // Rename A.md → B.md: история переезжает, по старому пути пусто, по новому — снапшот цел.
        move_history(root, "Notes/A.md", "Notes/B.md").unwrap();
        assert!(list_snapshots(root, "Notes/A.md").unwrap().is_empty());
        let moved = list_snapshots(root, "Notes/B.md").unwrap();
        assert_eq!(moved.len(), 1);
        assert_eq!(
            read_snapshot(root, "Notes/B.md", moved[0].ts).unwrap(),
            "v1"
        );
        // Нет истории у from → no-op (не ошибка).
        move_history(root, "Ghost.md", "Other.md").unwrap();
        assert!(list_snapshots(root, "Other.md").unwrap().is_empty());
    }

    #[test]
    fn manual_save_bypasses_throttle_changed_content() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        snapshot(root, "B.md", "one", true).unwrap();
        // Ручной save с НОВЫМ контентом сразу же фиксирует точку (троттл не для ручного).
        snapshot(root, "B.md", "two", true).unwrap();
        let snaps = list_snapshots(root, "B.md").unwrap();
        assert_eq!(
            snaps.len(),
            2,
            "ручной save фиксирует изменённый контент без троттла"
        );
    }

    #[test]
    fn auto_save_throttled_within_window() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        snapshot(root, "C.md", "one", false).unwrap();
        // Автосейв с новым контентом в пределах окна троттла — снапшот пропускается.
        snapshot(root, "C.md", "two", false).unwrap();
        assert_eq!(
            list_snapshots(root, "C.md").unwrap().len(),
            1,
            "автосейв чаще 90с не плодит снапшоты"
        );
    }

    #[test]
    fn list_empty_for_unknown_file() {
        let dir = TempDir::new().unwrap();
        assert!(list_snapshots(dir.path(), "Nope.md").unwrap().is_empty());
    }

    /// Аудит 2026-06: конкурентные снапшоты одной заметки (вероятно в одну мс) НЕ теряются —
    /// имя `<ts>.md` резервируется атомарно O_EXCL вместо TOCTOU `exists()`-проверки. Контент у
    /// каждого уникален → дедуп не схлопывает; ждём все N снапшотов.
    #[test]
    fn concurrent_snapshots_do_not_lose_each_other() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        const N: usize = 16;
        std::thread::scope(|s| {
            for i in 0..N {
                s.spawn(move || {
                    snapshot(root, "Race.md", &format!("content-{i}"), true).unwrap();
                });
            }
        });
        assert_eq!(
            list_snapshots(root, "Race.md").unwrap().len(),
            N,
            "ни один из {N} конкурентных снапшотов не должен затереться (TOCTOU закрыт O_EXCL)"
        );
    }
}
