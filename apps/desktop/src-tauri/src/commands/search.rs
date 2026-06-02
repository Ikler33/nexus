//! Команды поиска: по метаданным (Ф0: title/path/tags) и гибридный по телу (Ф1-6).

use tauri::State;

use crate::search::{self, SearchFilter, SearchHit, SearchOptions};
use crate::state::AppState;
use crate::vault::NoteRef;

/// Поиск заметок по подстроке (path/title/tags).
#[tauri::command]
pub async fn search_vault(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<NoteRef>, String> {
    let reader = {
        let guard = state.vault.read().await;
        guard.as_ref().ok_or("vault не открыт")?.db.reader().clone()
    };
    search::search_notes(&reader, query)
        .await
        .map_err(|e| e.to_string())
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
) -> Result<Vec<SearchHit>, String> {
    // Снимаем нужное из контекста и отпускаем лок ДО сетевого эмбеддинга запроса.
    let (reader, vectors, embedder) = {
        let guard = state.vault.read().await;
        let ctx = guard.as_ref().ok_or("vault не открыт")?;
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
    search::hybrid_search(
        &reader,
        vectors.as_deref(),
        embedder.as_deref(),
        query,
        opts,
    )
    .await
    .map_err(|e| e.to_string())
}
