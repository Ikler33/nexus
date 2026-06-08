//! Команда планировщика для StatusBar (ADR-007, срез 5): сводка очереди по состояниям.

use tauri::State;

use crate::error::AppResult;
use crate::scheduler::{self, JobCounts};
use crate::state::AppState;

/// Счётчики джоб (pending/running/dead) для индикатора в StatusBar. Без открытого vault — нули
/// (а не ошибка): индикатор просто не показывается.
#[tauri::command]
pub async fn get_job_counts(state: State<'_, AppState>) -> AppResult<JobCounts> {
    let reader = {
        let g = state.vault.read().await;
        match g.as_ref() {
            Some(ctx) => ctx.db.reader().clone(),
            None => return Ok(JobCounts::default()),
        }
    };
    Ok(scheduler::counts(&reader).await?)
}

/// Идёт ли ещё работа над `kind` (pending|running) — для UI «Генерирую…» дайджеста/противоречий:
/// когда джоба завершилась/упала, фронт гасит индикатор, даже если нового результата нет. Без vault — `false`.
#[tauri::command]
pub async fn job_active(state: State<'_, AppState>, kind: String) -> AppResult<bool> {
    let reader = {
        let g = state.vault.read().await;
        match g.as_ref() {
            Some(ctx) => ctx.db.reader().clone(),
            None => return Ok(false),
        }
    };
    Ok(scheduler::is_kind_busy(&reader, &kind).await?)
}
