//! Команды vault: открытие хранилища и ленивый листинг каталогов.

use std::path::{Path, PathBuf};

use tauri::State;

use crate::db::Database;
use crate::state::{AppState, VaultContext};
use crate::vault::{self, FileEntry, VaultInfo};

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
