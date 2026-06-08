//! Команды поиска: по метаданным (Ф0: title/path/tags) и гибридный по телу (Ф1-6).

use tauri::State;

use crate::error::AppResult;
use crate::search::{self, SearchFilter, SearchHit, SearchOptions};
use crate::state::AppState;
use crate::vault::NoteRef;

/// Поиск заметок по подстроке (path/title/tags).
#[tauri::command]
pub async fn search_vault(state: State<'_, AppState>, query: String) -> AppResult<Vec<NoteRef>> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(search::search_notes(&reader, query).await?)
}

/// Гибридный поиск по ТЕЛУ заметок (вектор + FTS5 → RRF, §6.2). `limit` по умолчанию 10, потолок 50.
/// Опционально: `folder`/`tag` — префильтр по метаданным ДО KNN (AC-Б6-2); `center` — открытый файл,
/// включающий граф-ранг 3-м источником. Без эмбеддера работает FTS-ветвь; нет чанков — пусто.
#[tauri::command]
pub async fn search_content(
    state: State<'_, AppState>,
    query: String,
    limit: Option<usize>,
    folder: Option<String>,
    tag: Option<String>,
    center: Option<String>,
) -> AppResult<Vec<SearchHit>> {
    // Снимаем нужное из контекста и отпускаем лок ДО сетевого эмбеддинга запроса.
    let (reader, vectors, embedder) = {
        let ctx = state.vault().await?;
        (
            ctx.db.reader().clone(),
            ctx.vectors.clone(),
            ctx.embedder.clone(),
        )
    };
    let filter = (folder.is_some() || tag.is_some()).then_some(SearchFilter { folder, tag });
    let opts = SearchOptions {
        limit: limit.unwrap_or(10).min(50),
        filter,
        center,
    };
    Ok(search::hybrid_search(
        &reader,
        vectors.as_deref(),
        embedder.as_deref(),
        query,
        opts,
    )
    .await?)
}
