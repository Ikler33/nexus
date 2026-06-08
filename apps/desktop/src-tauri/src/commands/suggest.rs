//! Команда предложений связей (Ф1-9, режим 1 max-sim). Считается из готовых векторов usearch.

use tauri::State;

use crate::error::AppResult;
use crate::state::AppState;
use crate::suggest::{self, LinkSuggestion};

/// Кандидаты на связь для файла `path` (семантически близкие незалинкованные заметки).
/// `limit` по умолчанию 5, потолок 20. Без RAG-индекса (нет векторов) — пусто.
#[tauri::command]
pub async fn get_link_suggestions(
    state: State<'_, AppState>,
    path: String,
    limit: Option<usize>,
) -> AppResult<Vec<LinkSuggestion>> {
    let (reader, vectors) = {
        let ctx = state.vault().await?;
        (ctx.db.reader().clone(), ctx.vectors.clone())
    };
    let Some(vectors) = vectors else {
        return Ok(Vec::new());
    };
    let limit = limit.unwrap_or(5).min(20);
    Ok(suggest::get_link_suggestions(&reader, vectors.as_ref(), path, limit).await?)
}

/// «Похожие заметки» (#35, дискавери): семантически близкие заметки ВКЛЮЧАЯ уже связанные. Порог —
/// на стороне UI (настройка), бэкенд отдаёт топ-`limit` по max-sim. `limit` по умолчанию 12, потолок 20.
#[tauri::command]
pub async fn get_related_notes(
    state: State<'_, AppState>,
    path: String,
    limit: Option<usize>,
) -> AppResult<Vec<LinkSuggestion>> {
    let (reader, vectors) = {
        let ctx = state.vault().await?;
        (ctx.db.reader().clone(), ctx.vectors.clone())
    };
    let Some(vectors) = vectors else {
        return Ok(Vec::new());
    };
    let limit = limit.unwrap_or(12).min(20);
    Ok(suggest::get_related_notes(&reader, vectors.as_ref(), path, limit).await?)
}
