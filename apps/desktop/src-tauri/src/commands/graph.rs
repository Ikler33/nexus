//! Команды графа/беклинков (ADR-004).

use tauri::State;

use crate::error::AppResult;
use crate::graph::{self, BacklinkEntry, FullGraph, GraphData};
use crate::state::AppState;

/// Беклинки файла (источник истины — SQLite, запрос по idx_links_target).
#[tauri::command]
pub async fn get_backlinks(
    state: State<'_, AppState>,
    path: String,
) -> AppResult<Vec<BacklinkEntry>> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(graph::get_backlinks(&reader, path).await?)
}

/// Локальный N-hop граф вокруг файла (ADR-004).
#[tauri::command]
pub async fn get_local_graph(
    state: State<'_, AppState>,
    center: String,
    hops: u32,
) -> AppResult<GraphData> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(graph::get_local_graph(&reader, center, hops).await?)
}

/// Единый граф всего vault (AC-DOD-Ф3): топ-`limit` файлов по связности + рёбра.
#[tauri::command]
pub async fn get_full_graph(state: State<'_, AppState>, limit: usize) -> AppResult<FullGraph> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(graph::get_full_graph(&reader, limit).await?)
}
