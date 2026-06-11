//! Команды планировщика для StatusBar (ADR-007, срез 5): сводка очереди по состояниям +
//! детали dead-джоб (модалка за «⚠ N»: список ошибок, ручной retry, очистка).

use tauri::State;

use crate::error::AppResult;
use crate::scheduler::{self, ActiveJob, DeadJob, JobCounts};
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
    Ok(scheduler::counts(&reader, scheduler::now_secs()).await?)
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

/// Перезапуск воркера планировщика (N1, кнопка в модалке очереди): аварийная мера, если фоновые
/// задачи зависли (инцидент 2026-06-12). Рвёт старый супервизор + дропает его shutdown-канал,
/// поднимает новый воркер тем же конфигом — БЕЗ переоткрытия vault. На старте новый цикл делает
/// crash-recovery (running→pending) и тут же клеймит готовые джобы. Без vault — no-op.
#[tauri::command]
pub async fn restart_scheduler(state: State<'_, AppState>) -> AppResult<()> {
    let g = state.vault.read().await;
    let Some(ctx) = g.as_ref() else {
        return Ok(());
    };
    let fresh = ctx.lifecycle.scheduler_spawner.start();
    {
        let mut slot = ctx
            .lifecycle
            .scheduler_worker
            .lock()
            .expect("scheduler_worker lock");
        // Гасим старый: явный shutdown (на случай живого цикла) + abort супервизора.
        let _ = slot.shutdown.send(true);
        slot.supervisor.abort();
        *slot = fresh;
    }
    tracing::info!("scheduler перезапущен вручную (UI)");
    Ok(())
}

/// Активные джобы (running/pending) — модалка очереди за «N задач» в статусбаре. Без vault — пусто.
#[tauri::command]
pub async fn get_active_jobs(state: State<'_, AppState>) -> AppResult<Vec<ActiveJob>> {
    let reader = {
        let g = state.vault.read().await;
        match g.as_ref() {
            Some(ctx) => ctx.db.reader().clone(),
            None => return Ok(Vec::new()),
        }
    };
    Ok(scheduler::list_active(&reader).await?)
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
