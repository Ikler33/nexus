//! Команда «Прогресс целей» (#35, vision-волна 2): кросс-файловый список заметок-целей (#goal).

use tauri::State;

use crate::error::AppResult;
use crate::goals::{self, Goal};
use crate::state::AppState;

/// Все заметки-цели (инлайн-тег `#goal`) с прогрессом 0–100 (`null` — нет валидного значения, D7).
/// Без открытого vault — ошибка. Чистый SQL-read (офлайн, без LLM/сети).
#[tauri::command]
pub async fn list_goals(state: State<'_, AppState>) -> AppResult<Vec<Goal>> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(goals::list_goals(&reader).await?)
}
