//! Команда плагинов (Ф0-13: чтение манифестов + совместимость; без исполнения — Ф2).

use tauri::State;

use crate::plugin::{self, PluginInfo};
use crate::state::AppState;

/// Список установленных плагинов vault (`.nexus/plugins/*`) с их статусом совместимости.
#[tauri::command]
pub async fn list_plugins(state: State<'_, AppState>) -> Result<Vec<PluginInfo>, String> {
    let root = {
        let guard = state.vault.read().await;
        guard.as_ref().ok_or("vault не открыт")?.root.clone()
    };
    let dir = root.join(".nexus").join("plugins");
    tokio::task::spawn_blocking(move || plugin::scan_plugins(&dir))
        .await
        .map_err(|e| e.to_string())
}
