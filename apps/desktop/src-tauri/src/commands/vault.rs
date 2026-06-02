//! Команды vault: открытие хранилища и ленивый листинг каталогов.

use std::path::{Path, PathBuf};

use tauri::State;

use crate::db::Database;
use crate::state::{AppState, VaultContext};
use crate::vault::{self, FileEntry, NoteRef, VaultInfo};

/// Открывает vault: канонизирует папку, открывает БД в `.nexus/nexus.db`, сохраняет в state.
#[tauri::command]
pub async fn open_vault(state: State<'_, AppState>, path: String) -> Result<VaultInfo, String> {
    let root = PathBuf::from(&path)
        .canonicalize()
        .map_err(|e| format!("vault path: {e}"))?;
    if !root.is_dir() {
        return Err("vault: путь не является каталогом".into());
    }

    let db = Database::open(root.join(".nexus").join("nexus.db"))
        .await
        .map_err(|e| e.to_string())?;

    let info = VaultInfo {
        root: root.to_string_lossy().into_owned(),
        name: vault::vault_name(&root),
    };

    // Запускаем watcher + фоновую индексацию (начальный скан + инкрементальные события).
    crate::indexer::spawn(&db, root.clone());

    *state.vault.write().await = Some(VaultContext { root, db });
    tracing::info!(vault = %info.root, "opened vault");
    Ok(info)
}

/// Ленивый листинг каталога vault (`dir_path` относительный; пустая строка = корень).
#[tauri::command]
pub async fn list_dir(
    state: State<'_, AppState>,
    dir_path: String,
) -> Result<Vec<FileEntry>, String> {
    // Копируем корень и отпускаем лок: ФС-обход уводим в blocking-пул.
    let root = {
        let guard = state.vault.read().await;
        guard.as_ref().ok_or("vault не открыт")?.root.clone()
    };
    tokio::task::spawn_blocking(move || vault::list_dir(&root, Path::new(&dir_path)))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Читает содержимое файла vault (путь относительный; анти-traversal через resolve).
#[tauri::command]
pub async fn read_file(state: State<'_, AppState>, path: String) -> Result<String, String> {
    let root = current_root(&state).await?;
    let abs = vault::resolve_vault_path(&root, Path::new(&path)).map_err(|e| e.to_string())?;
    tokio::fs::read_to_string(&abs)
        .await
        .map_err(|e| e.to_string())
}

/// Пишет содержимое файла vault (целевой путь может ещё не существовать).
#[tauri::command]
pub async fn write_file(
    state: State<'_, AppState>,
    path: String,
    content: String,
) -> Result<(), String> {
    let root = current_root(&state).await?;
    let abs =
        vault::resolve_vault_path_for_write(&root, Path::new(&path)).map_err(|e| e.to_string())?;
    tokio::fs::write(&abs, content)
        .await
        .map_err(|e| e.to_string())
}

/// Все заметки vault (path + title) — для автокомплита `[[wikilink]]` и поиска.
#[tauri::command]
pub async fn list_notes(state: State<'_, AppState>) -> Result<Vec<NoteRef>, String> {
    let reader = {
        let guard = state.vault.read().await;
        guard.as_ref().ok_or("vault не открыт")?.db.reader().clone()
    };
    reader
        .query(|c| {
            let mut stmt =
                c.prepare("SELECT path, title FROM files WHERE is_deleted=0 ORDER BY path")?;
            let notes = stmt
                .query_map([], |r| {
                    Ok(NoteRef {
                        path: r.get(0)?,
                        title: r.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(notes)
        })
        .await
        .map_err(|e| e.to_string())
}

/// Корень текущего открытого vault (или ошибка, если не открыт).
async fn current_root(state: &State<'_, AppState>) -> Result<PathBuf, String> {
    let guard = state.vault.read().await;
    Ok(guard.as_ref().ok_or("vault не открыт")?.root.clone())
}
