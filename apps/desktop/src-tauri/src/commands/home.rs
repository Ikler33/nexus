//! Команды HOME-дашборда. H1: статические/динамические виджеты (stats/recent/goals) одним запросом.
//! H2: кэш LLM-виджетов — `get_widget` (мгновенно из кэша) + `refresh_widget` (ручной refresh поверх
//! планировщика ADR-007). См. `docs/dev/HOME_BACKEND_PLAN.md`.

use tauri::State;

use crate::error::AppResult;
use crate::home::widgets::{self, Widget};
use crate::home::{self, HomeData};
use crate::scheduler;
use crate::state::AppState;

/// Данные HOME для статических/динамических зон (без LLM). Без открытого vault — ошибка.
#[tauri::command]
pub async fn get_home_data(state: State<'_, AppState>) -> AppResult<HomeData> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(home::home_data(&reader).await?)
}

/// Кэшированный LLM-виджет по ключу (или `null`, если ещё не генерировался). Мгновенно — НЕ ждёт LLM
/// (генерация идёт фоном; готовность прилетает событием `home:widget-updated`). Без vault — `null`
/// (виджет просто не покажется).
#[tauri::command]
pub async fn get_widget(state: State<'_, AppState>, key: String) -> AppResult<Option<Widget>> {
    let reader = {
        let g = state.vault.read().await;
        match g.as_ref() {
            Some(ctx) => ctx.db.reader().clone(),
            None => return Ok(None),
        }
    };
    Ok(widgets::get(&reader, &key).await?)
}

/// Ручной refresh виджета (режим manual): ставит фоновую генерацию в очередь. Ключ должен быть
/// зарегистрированным виджетом (иначе понятная ошибка вместо тихого dead-letter). Дедуп: если генерация
/// уже готова к запуску/выполняется — повторный клик no-op. Результат — событием `home:widget-updated`.
#[tauri::command]
pub async fn refresh_widget(state: State<'_, AppState>, key: String) -> AppResult<()> {
    let (writer, reader, known) = {
        let ctx = state.vault().await?;
        (
            ctx.db.writer().clone(),
            ctx.db.reader().clone(),
            ctx.widgets.contains(&key),
        )
    };
    if !known {
        return Err(format!("неизвестный HOME-виджет: {key}").into());
    }
    let kind = widgets::widget_kind(&key);
    if scheduler::has_ready_job(&reader, &kind, scheduler::now_secs()).await? {
        return Ok(()); // уже в очереди/выполняется — дедуп
    }
    scheduler::enqueue(&writer, &kind, "", 0, 2).await?;
    Ok(())
}
