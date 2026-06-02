//! Команда поиска (Ф0: title/path/tags).

use tauri::State;

use crate::search;
use crate::state::AppState;
use crate::vault::NoteRef;

/// Поиск заметок по подстроке (path/title/tags).
#[tauri::command]
pub async fn search_vault(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<NoteRef>, String> {
    let reader = {
        let guard = state.vault.read().await;
        guard.as_ref().ok_or("vault не открыт")?.db.reader().clone()
    };
    search::search_notes(&reader, query)
        .await
        .map_err(|e| e.to_string())
}
