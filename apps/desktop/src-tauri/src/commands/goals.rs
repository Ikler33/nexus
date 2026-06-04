//! Команда «Прогресс целей» (#35, vision-волна 2): кросс-файловый список заметок-целей (#goal).

use tauri::State;

use crate::goals::{self, Goal};
use crate::state::AppState;

/// Все заметки-цели (инлайн-тег `#goal`) с прогрессом 0–100 (`null` — нет валидного значения, D7).
/// Без открытого vault — пусто. Чистый SQL-read (офлайн, без LLM/сети).
#[tauri::command]
pub async fn list_goals(state: State<'_, AppState>) -> Result<Vec<Goal>, String> {
    let reader = {
        let guard = state.vault.read().await;
        guard.as_ref().ok_or("vault не открыт")?.db.reader().clone()
    };
    goals::list_goals(&reader).await.map_err(|e| e.to_string())
}
