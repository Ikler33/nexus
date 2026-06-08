//! Команды «Дайджеста изменений» (#35): получить последний + поставить генерацию сейчас.

use tauri::State;

use crate::digest::{self, Digest, KIND_DIGEST};
use crate::scheduler;
use crate::state::AppState;

/// Последний сгенерированный дайджест (или `null`). Без открытого vault — ошибка.
#[tauri::command]
pub async fn get_latest_digest(state: State<'_, AppState>) -> Result<Option<Digest>, String> {
    let reader = {
        let g = state.vault.read().await;
        g.as_ref().ok_or("vault не открыт")?.db.reader().clone()
    };
    digest::latest(&reader).await.map_err(|e| e.to_string())
}

/// Ставит генерацию дайджеста в очередь сейчас (воркер выполнит на ближайшем тике). Требует
/// сконфигурированного chat (LLM) — иначе понятная ошибка вместо тихого dead-letter.
#[tauri::command]
pub async fn generate_digest(state: State<'_, AppState>) -> Result<(), String> {
    let (writer, reader, has_chat) = {
        let g = state.vault.read().await;
        let ctx = g.as_ref().ok_or("vault не открыт")?;
        (
            ctx.db.writer().clone(),
            ctx.db.reader().clone(),
            ctx.chat.is_some(),
        )
    };
    if !has_chat {
        return Err("chat (LLM) не сконфигурирован — настройте в «AI / Модели»".into());
    }
    // Дедуп (slice 6): если дайджест уже готов к запуску/выполняется — повторный клик no-op.
    if scheduler::has_ready_job(&reader, KIND_DIGEST, scheduler::now_secs())
        .await
        .map_err(|e| e.to_string())?
    {
        return Ok(());
    }
    scheduler::enqueue(&writer, KIND_DIGEST, "", 0, 2)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}
