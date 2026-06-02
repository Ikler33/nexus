//! Команды графа/беклинков (ADR-004).

use tauri::State;

use crate::graph::{self, BacklinkEntry};
use crate::state::AppState;

/// Беклинки файла (источник истины — SQLite, запрос по idx_links_target).
#[tauri::command]
pub async fn get_backlinks(
    state: State<'_, AppState>,
    path: String,
) -> Result<Vec<BacklinkEntry>, String> {
    let reader = {
        let guard = state.vault.read().await;
        guard.as_ref().ok_or("vault не открыт")?.db.reader().clone()
    };
    graph::get_backlinks(&reader, path)
        .await
        .map_err(|e| e.to_string())
}
