//! Команда канбан-доски (BOARD-2): кросс-файловый список заметок-задач (frontmatter `status`).

use tauri::State;

use crate::board::{self, TaskCard, DEFAULT_STATUS_KEY};
use crate::error::AppResult;
use crate::state::AppState;

/// Все заметки-задачи (есть frontmatter-ключ `status_key`, по умолч. `status`) с полями для доски.
/// Без открытого vault — ошибка. Чистый SQL-read (офлайн, без LLM/сети). Колонкование — на фронте.
#[tauri::command]
pub async fn list_board(
    state: State<'_, AppState>,
    status_key: Option<String>,
) -> AppResult<Vec<TaskCard>> {
    let reader = state.vault().await?.db.reader().clone();
    let key = status_key
        .filter(|k| !k.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_STATUS_KEY.to_string());
    Ok(board::list_board(&reader, key).await?)
}
