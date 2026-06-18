//! Команды «Поиска противоречий» (#vision, спека `docs/specs/contradictions.md`): список найденных +
//! ручной запуск (D1). Бэкенд — фоновый kind планировщика (`contradictions::ContradictionHandler`).

use tauri::State;

use crate::contradictions::{self, Contradiction, KIND_CONTRA};
use crate::error::AppResult;
use crate::scheduler;
use crate::state::AppState;

/// Найденные противоречия (или пусто). Без открытого vault — пусто (панель просто не покажет).
#[tauri::command]
pub async fn get_contradictions(state: State<'_, AppState>) -> AppResult<Vec<Contradiction>> {
    let reader = {
        let g = state.vault.read().await;
        match g.as_ref() {
            Some(ctx) => ctx.db.reader().clone(),
            None => return Ok(Vec::new()),
        }
    };
    Ok(contradictions::list(&reader).await?)
}

/// Ставит поиск противоречий в очередь (вручную, D1). Требует chat (LLM) + эмбеддинги (векторы);
/// дедуп активной джобы (AC-CT-6) — повторный клик при уже идущем поиске no-op.
#[tauri::command]
pub async fn generate_contradictions(state: State<'_, AppState>) -> AppResult<()> {
    let (writer, reader, ready) = {
        let ctx = state.vault().await?;
        (
            ctx.db.writer().clone(),
            ctx.db.reader().clone(),
            ctx.ai.chat.is_some() && ctx.vectors.is_some(),
        )
    };
    if !ready {
        return Err("нужны chat (LLM) и эмбеддинги — настройте в «AI / Модели»".into());
    }
    // Тоггл OFF → ручной запуск no-op (хендлер всё равно NOOP-гейтит, но не плодим джобу зря).
    if !contradictions::is_enabled(&reader).await {
        return Ok(());
    }
    if scheduler::has_ready_job(&reader, KIND_CONTRA, scheduler::now_secs()).await? {
        return Ok(()); // уже в очереди/выполняется — дедуп
    }
    scheduler::enqueue(&writer, KIND_CONTRA, "", 0, 2).await?;
    Ok(())
}

/// Текущее состояние тоггла «Поиск противоречий» (persisted). Дефолт OFF.
#[tauri::command]
pub async fn contradictions_get_enabled(state: State<'_, AppState>) -> AppResult<bool> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(contradictions::is_enabled(&reader).await)
}

/// Переключить «Поиск противоречий». Persist `contradictions.enabled` + при ВКЛЮЧЕНИИ — enqueue kick
/// (зеркало `episode_set_enabled`: сид/recurring гейтятся флагом и регистрируются лишь на открытии vault,
/// поэтому без kick включение в работающем приложении не запустит поиск до перезапуска). Хендлер сам
/// рано выйдет NOOP, если состояние рассинхронится.
#[tauri::command]
pub async fn contradictions_set_enabled(state: State<'_, AppState>, on: bool) -> AppResult<()> {
    let (writer, ready) = {
        let ctx = state.vault().await?;
        (
            ctx.db.writer().clone(),
            ctx.ai.chat.is_some() && ctx.vectors.is_some(),
        )
    };
    contradictions::set_enabled(&writer, on).await?;
    if on && ready {
        let _ = scheduler::enqueue(&writer, KIND_CONTRA, "", 0, 2).await;
    }
    Ok(())
}
