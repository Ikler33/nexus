//! Команда HOME-дашборда (H1): статические/динамические виджеты (stats/recent/goals) одним запросом.
//! LLM-виджеты и кэш — отдельными срезами (H2+, см. `docs/dev/HOME_BACKEND_PLAN.md`).

use tauri::State;

use crate::error::AppResult;
use crate::home::{self, HomeData};
use crate::state::AppState;

/// Данные HOME для статических/динамических зон (без LLM). Без открытого vault — ошибка.
#[tauri::command]
pub async fn get_home_data(state: State<'_, AppState>) -> AppResult<HomeData> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(home::home_data(&reader).await?)
}
