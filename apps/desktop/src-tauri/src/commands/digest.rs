//! Команды «Дайджеста изменений» (#35): получить последний + поставить генерацию сейчас.

use tauri::State;

use crate::digest::{self, Digest, KIND_DIGEST};
use crate::error::AppResult;
use crate::scheduler;
use crate::state::AppState;

/// Последний сгенерированный дайджест (или `null`). Без открытого vault — ошибка.
#[tauri::command]
pub async fn get_latest_digest(state: State<'_, AppState>) -> AppResult<Option<Digest>> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(digest::latest(&reader).await?)
}

/// Ставит генерацию дайджеста в очередь сейчас (воркер выполнит на ближайшем тике). Требует
/// сконфигурированного chat (LLM) — иначе понятная ошибка вместо тихого dead-letter.
#[tauri::command]
pub async fn generate_digest(state: State<'_, AppState>) -> AppResult<()> {
    let (writer, reader, has_chat) = {
        let ctx = state.vault().await?;
        (
            ctx.db.writer().clone(),
            ctx.db.reader().clone(),
            ctx.ai.chat.is_some(),
        )
    };
    if !has_chat {
        return Err("chat (LLM) не сконфигурирован — настройте в «AI / Модели»".into());
    }
    // Дедуп (slice 6): если дайджест уже готов к запуску/выполняется — повторный клик no-op.
    if scheduler::has_ready_job(&reader, KIND_DIGEST, scheduler::now_secs()).await? {
        return Ok(());
    }
    scheduler::enqueue(&writer, KIND_DIGEST, "", 0, 2).await?;
    Ok(())
}
