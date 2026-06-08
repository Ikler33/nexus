//! Команды «Поиска противоречий» (#vision, спека `docs/specs/contradictions.md`): список найденных +
//! ручной запуск (D1). Бэкенд — фоновый kind планировщика (`contradictions::ContradictionHandler`).

use tauri::State;

use crate::contradictions::{self, Contradiction, KIND_CONTRA};
use crate::scheduler;
use crate::state::AppState;

/// Найденные противоречия (или пусто). Без открытого vault — пусто (панель просто не покажет).
#[tauri::command]
pub async fn get_contradictions(state: State<'_, AppState>) -> Result<Vec<Contradiction>, String> {
    let reader = {
        let g = state.vault.read().await;
        match g.as_ref() {
            Some(ctx) => ctx.db.reader().clone(),
            None => return Ok(Vec::new()),
        }
    };
    contradictions::list(&reader)
        .await
        .map_err(|e| e.to_string())
}

/// Ставит поиск противоречий в очередь (вручную, D1). Требует chat (LLM) + эмбеддинги (векторы);
/// дедуп активной джобы (AC-CT-6) — повторный клик при уже идущем поиске no-op.
#[tauri::command]
pub async fn generate_contradictions(state: State<'_, AppState>) -> Result<(), String> {
    let (writer, reader, ready) = {
        let g = state.vault.read().await;
        let ctx = g.as_ref().ok_or("vault не открыт")?;
        (
            ctx.db.writer().clone(),
            ctx.db.reader().clone(),
            ctx.chat.is_some() && ctx.vectors.is_some(),
        )
    };
    if !ready {
        return Err("нужны chat (LLM) и эмбеддинги — настройте в «AI / Модели»".into());
    }
    if scheduler::has_ready_job(&reader, KIND_CONTRA, scheduler::now_secs())
        .await
        .map_err(|e| e.to_string())?
    {
        return Ok(()); // уже в очереди/выполняется — дедуп
    }
    scheduler::enqueue(&writer, KIND_CONTRA, "", 0, 2)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}
