//! Команда предложений связей (Ф1-9, режим 1 max-sim). Считается из готовых векторов usearch.

use tauri::State;

use crate::state::AppState;
use crate::suggest::{self, LinkSuggestion};

/// Кандидаты на связь для файла `path` (семантически близкие незалинкованные заметки).
/// `limit` по умолчанию 5, потолок 20. Без RAG-индекса (нет векторов) — пусто.
#[tauri::command]
pub async fn get_link_suggestions(
    state: State<'_, AppState>,
    path: String,
    limit: Option<usize>,
) -> Result<Vec<LinkSuggestion>, String> {
    let (reader, vectors) = {
        let guard = state.vault.read().await;
        let ctx = guard.as_ref().ok_or("vault не открыт")?;
        (ctx.db.reader().clone(), ctx.vectors.clone())
    };
    let Some(vectors) = vectors else {
        return Ok(Vec::new());
    };
    let limit = limit.unwrap_or(5).min(20);
    suggest::get_link_suggestions(&reader, vectors.as_ref(), path, limit)
        .await
        .map_err(|e| e.to_string())
}
