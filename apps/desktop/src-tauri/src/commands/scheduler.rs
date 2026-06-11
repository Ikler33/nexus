//! Команды планировщика для StatusBar (ADR-007, срез 5): сводка очереди по состояниям +
//! детали dead-джоб (модалка за «⚠ N»: список ошибок, ручной retry, очистка).

use tauri::State;

use crate::error::AppResult;
use crate::scheduler::{self, DeadJob, JobCounts};
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

/// Детали dead-джоб (kind/ошибка/попытки/когда) для модалки за «⚠ N» (S7: смерть разбираема,
/// не только видима). Без vault — пустой список.
#[tauri::command]
pub async fn get_dead_jobs(state: State<'_, AppState>) -> AppResult<Vec<DeadJob>> {
    let reader = {
        let g = state.vault.read().await;
        match g.as_ref() {
            Some(ctx) => ctx.db.reader().clone(),
            None => return Ok(Vec::new()),
        }
    };
    Ok(scheduler::list_dead(&reader).await?)
}

/// «Повторить» из модалки: dead → pending с чистыми attempts (жмут после исправления причины —
/// напр. URL модели в Настройках). `false` — джоба уже не dead (гонка/повторный клик), не ошибка.
#[tauri::command]
pub async fn retry_dead_job(state: State<'_, AppState>, id: i64) -> AppResult<bool> {
    let writer = {
        let g = state.vault.read().await;
        match g.as_ref() {
            Some(ctx) => ctx.db.writer().clone(),
            None => return Ok(false),
        }
    };
    Ok(scheduler::retry_dead(&writer, id, scheduler::now_secs()).await?)
}

/// «Очистить» из модалки: удалить все dead-джобы (осознанное «видел, чинить не буду»).
#[tauri::command]
pub async fn clear_dead_jobs(state: State<'_, AppState>) -> AppResult<usize> {
    let writer = {
        let g = state.vault.read().await;
        match g.as_ref() {
            Some(ctx) => ctx.db.writer().clone(),
            None => return Ok(0),
        }
    };
    Ok(scheduler::clear_dead(&writer).await?)
}
